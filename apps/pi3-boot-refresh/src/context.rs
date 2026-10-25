use crate::{Error, Result, le32, le64};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InstallContext {
    pub disk_index: u32,
    pub boot_drive: String,
    pub windows_drive: String,
}

pub fn parse_context(mut getenv: impl FnMut(&str) -> String) -> Result<Option<InstallContext>> {
    match getenv("WOR_DEVICE_TYPE").to_uppercase().as_str() {
        "RPI4-ARM64" | "RPI5-ARM64" => return Ok(None),
        "RPI3-ARM64" => {}
        _ => return Err(Error::Board),
    }
    match getenv("WOR_INSTALLOPTIONS_PARTITIONSCHEME")
        .to_uppercase()
        .as_str()
    {
        "MBR" => return Ok(None),
        "GPT" => {}
        _ => return Err(Error::PartitionScheme),
    }
    if !getenv("WOR_IMAGE_ARCH").eq_ignore_ascii_case("ARM64") {
        return Err(Error::Architecture);
    }
    let index = getenv("WOR_DISK_INDEX");
    if index.is_empty() || !index.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(Error::DiskIndexRequired);
    }
    let disk_index = index.parse().map_err(Error::DiskIndex)?;
    let mut drive = |name| {
        let value = getenv(name).to_uppercase();
        if !valid_drive(&value) {
            return Err(Error::DriveLetter(name));
        }
        Ok(value)
    };
    let boot_drive = drive("WOR_DISK_BOOTPARTITION")?;
    let windows_drive = drive("WOR_DISK_WINDOWSPARTITION")?;
    Ok(Some(InstallContext {
        disk_index,
        boot_drive,
        windows_drive,
    }))
}

fn valid_drive(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 2 && bytes[0].is_ascii_uppercase() && bytes[1] == b':'
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VolumeIdentity {
    pub drive: String,
    pub device_type: u32,
    pub disk_index: u32,
    pub partition_index: u32,
}

pub fn select_boot_drive(context: &InstallContext, volumes: &[VolumeIdentity]) -> Result<String> {
    let mut selected = None;
    for volume in volumes {
        if volume.device_type != 7
            || volume.disk_index != context.disk_index
            || volume.partition_index != 1
        {
            continue;
        }
        if !valid_drive(&volume.drive) {
            return Err(Error::DiscoveryDrive);
        }
        if volume.drive == context.windows_drive {
            return Err(Error::WindowsIsBoot);
        }
        if selected.is_some() {
            return Err(Error::AmbiguousBoot);
        }
        selected = Some(volume.drive.clone());
    }
    let selected = selected.ok_or(Error::MissingBoot(context.disk_index))?;
    if context.boot_drive != context.windows_drive && selected != context.boot_drive {
        return Err(Error::BootExport);
    }
    Ok(selected)
}

pub fn validate_disk_number(data: &[u8], expected: u32) -> Result<()> {
    if data.len() != 12 || le32(data, 0) != 7 || le32(data, 4) != expected {
        return Err(Error::RawDiskIdentity);
    }
    let partition = le32(data, 8);
    if partition != 0 && partition != u32::MAX {
        return Err(Error::NotWholeDisk);
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VolumeExtent {
    pub disk_index: u32,
    pub offset: u64,
    pub length: u64,
}

pub fn parse_volume_extent(data: &[u8]) -> Result<VolumeExtent> {
    // ARM64 aligns the DISK_EXTENT member of VOLUME_DISK_EXTENTS to eight bytes.
    if data.len() != 32 || le32(data, 0) != 1 {
        return Err(Error::ExtentCount);
    }
    let extent = VolumeExtent {
        disk_index: le32(data, 8),
        offset: le64(data, 16),
        length: le64(data, 24),
    };
    if extent.offset == 0
        || extent.length == 0
        || extent.offset > i64::MAX as u64
        || extent.length > i64::MAX as u64
        || !extent.offset.is_multiple_of(512)
        || !extent.length.is_multiple_of(512)
    {
        return Err(Error::ExtentGeometry);
    }
    Ok(extent)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DiskLayout {
    pub disk_index: u32,
    pub bytes: u64,
    pub bytes_per_sector: u32,
    pub boot: VolumeExtent,
    pub windows: VolumeExtent,
}

impl DiskLayout {
    pub fn validate(&self) -> Result<()> {
        if self.bytes_per_sector != 512
            || !self.bytes.is_multiple_of(512)
            || self.bytes < 68 * 512
            || self.bytes > i64::MAX as u64
        {
            return Err(Error::DiskGeometry);
        }
        for extent in [self.boot, self.windows] {
            if extent.disk_index != self.disk_index
                || extent.offset == 0
                || extent.length == 0
                || !extent.offset.is_multiple_of(512)
                || !extent.length.is_multiple_of(512)
                || extent.offset >= self.bytes
                || extent.length > self.bytes - extent.offset
            {
                return Err(Error::ExtentBinding);
            }
        }
        if self.boot.offset + self.boot.length > self.windows.offset {
            return Err(Error::ExtentOverlap);
        }
        Ok(())
    }
}
