use crate::{
    BootstrapDisk, DiskLayout, Error, InstallContext, Result, Sector, VolumeExtent, VolumeIdentity,
    le32, le64, parse_context, refresh_bootstrap, select_boot_drive, validate_disk_number,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VolumeAccess {
    Read,
    ReadWrite,
}

pub trait Volume {
    fn identity(&mut self, drive: &str) -> Result<VolumeIdentity>;
    fn extent(&mut self) -> Result<VolumeExtent>;
    fn sync(&mut self) -> Result<()>;
    fn close(&mut self) -> Result<()>;
}

pub trait PhysicalDisk: BootstrapDisk {
    fn device_number(&mut self) -> Result<[u8; 12]>;
    fn geometry(&mut self) -> Result<[u8; 24]>;
    fn length(&mut self) -> Result<[u8; 8]>;
    fn close(&mut self) -> Result<()>;
}

pub trait Platform {
    type Volume: Volume;
    type Disk: PhysicalDisk;

    fn require_windows_pe(&mut self) -> Result<()>;
    fn require_file(&mut self, path: &str) -> Result<()>;
    fn logical_drives(&mut self) -> Result<u32>;
    fn open_volume(&mut self, drive: &str, access: VolumeAccess) -> Result<Self::Volume>;
    fn open_disk(&mut self, index: u32) -> Result<Self::Disk>;
    fn save_backup(&mut self, directory: &str, sector: &Sector) -> Result<()>;
    fn event(&mut self, level: &str, message: &str);
}

pub fn run(
    platform: &mut impl Platform,
    getenv: impl FnMut(&str) -> String,
) -> Result<&'static str> {
    let Some(mut context) = parse_context(getenv)? else {
        return Ok(
            "Pi 3 GPT bootstrap is not applicable to this board or MBR installation; no disk accessed.",
        );
    };
    platform
        .require_windows_pe()
        .map_err(|error| error.context("automatic boot refresh must run in Windows PE"))?;
    let mut windows = platform.open_volume(&context.windows_drive, VolumeAccess::Read)?;
    let result = with_windows(platform, &mut windows, &mut context);
    finish(result, windows.close())
}

fn with_windows<P: Platform>(
    platform: &mut P,
    windows: &mut P::Volume,
    context: &mut InstallContext,
) -> Result<&'static str> {
    if windows.extent()?.disk_index != context.disk_index {
        return Err(Error::WindowsVolumeDisk);
    }
    platform
        .require_file(&format!(
            r"{}\Windows\System32\ntoskrnl.exe",
            context.windows_drive
        ))
        .map_err(|error| {
            error.context("the selected Windows installation is not ready for finalization")
        })?;
    context.boot_drive = discover_boot_drive(platform, context)?;
    platform.event(
        "LogInfo",
        &format!(
            "Verified destination volumes: boot {}, Windows {}, disk {}.",
            context.boot_drive, context.windows_drive, context.disk_index
        ),
    );
    platform
        .require_file(&format!(r"{}\RPI_EFI.fd", context.boot_drive))
        .map_err(|error| error.context("the installed boot partition is missing Pi 3 firmware"))?;
    let mut boot = platform.open_volume(&context.boot_drive, VolumeAccess::ReadWrite)?;
    let result = with_boot(platform, &mut boot, windows, context);
    finish(result, boot.close())
}

fn discover_boot_drive(platform: &mut impl Platform, context: &InstallContext) -> Result<String> {
    let duplicate_export = context.boot_drive == context.windows_drive;
    let drives = if duplicate_export {
        // WoR-PE 1.1 exports WindowsPartitionLetter for both partition variables.
        platform.event(
            "LogWarn",
            &format!(
                "WoR-PE exported {} for both boot and Windows; locating partition 1 of disk {}.",
                context.windows_drive, context.disk_index
            ),
        );
        let mask = platform.logical_drives()?;
        if mask == 0 {
            return Err(Error::NoLogicalDrives);
        }
        (0..26_u8)
            .filter(|index| mask & (1 << index) != 0)
            .map(|index| format!("{}:", char::from(b'A' + index)))
            .collect::<Vec<_>>()
    } else {
        vec![context.boot_drive.clone()]
    };
    let mut volumes = vec![];
    for drive in drives {
        let result = platform
            .open_volume(&drive, VolumeAccess::Read)
            .and_then(|mut volume| {
                let result = volume.identity(&drive);
                finish(result, volume.close())
            });
        match result {
            Ok(identity) => volumes.push(identity),
            Err(error) if duplicate_export => platform.event(
                "LogDebug",
                &format!("Boot-volume discovery could not identify {drive}: {error}"),
            ),
            Err(error) => return Err(error),
        }
    }
    select_boot_drive(context, &volumes)
}

fn with_boot<P: Platform>(
    platform: &mut P,
    boot: &mut P::Volume,
    windows: &mut P::Volume,
    context: &InstallContext,
) -> Result<&'static str> {
    let identity = boot.identity(&context.boot_drive)?;
    select_boot_drive(context, &[identity])
        .map_err(|error| error.context("boot-volume identity changed after discovery"))?;
    let boot_extent = boot.extent()?;
    if boot_extent.disk_index != context.disk_index {
        return Err(Error::VolumeDisks);
    }
    let windows_extent = windows.extent()?;
    if windows_extent.disk_index != context.disk_index {
        return Err(Error::VolumeDisks);
    }
    let mut disk = platform.open_disk(context.disk_index)?;
    let result = (|| {
        validate_disk_number(&disk.device_number()?, context.disk_index)?;
        let geometry = disk.geometry()?;
        let length = disk.length()?;
        let layout = DiskLayout {
            disk_index: context.disk_index,
            bytes: le64(&length, 0),
            bytes_per_sector: le32(&geometry, 20),
            boot: boot_extent,
            windows: windows_extent,
        };
        layout.validate()?;
        boot.sync().map_err(|error| {
            error.context(
                "cannot flush the installed boot filesystem before inspecting its geometry",
            )
        })?;
        platform.event(
            "LogInfo",
            "Refreshing the Pi 3 bootstrap for the installed boot partition before reboot.",
        );
        // Sector zero is outside every validated volume. Do not dismount volumes
        // that the remaining WoR-PE hooks still require.
        let changed = refresh_bootstrap(&mut disk, &layout, |sector| {
            platform.save_backup(
                &format!(r"{}\Windows\Logs\WoR-Flasher", context.windows_drive),
                sector,
            )
        })?;
        if !changed {
            Ok(
                "Pi 3 GPT-compatible bootstrap already matches the installed filesystem; no write needed.",
            )
        } else {
            Ok("Pi 3 installed boot-sector repair verified; GPT and FAT metadata are unchanged.")
        }
    })();
    finish(result, disk.close())
}

pub(crate) fn finish<T>(result: Result<T>, close: Result<()>) -> Result<T> {
    match (result, close) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
        (Err(primary), Err(close)) => Err(Error::Close {
            primary: Box::new(primary),
            close: Box::new(close),
        }),
    }
}
