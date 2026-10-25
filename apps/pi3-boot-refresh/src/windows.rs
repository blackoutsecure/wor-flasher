//! The only raw-device/unsafe boundary. Nothing in this module is built on macOS.

use std::fs::{File, OpenOptions};
use std::io;
use std::os::windows::fs::{FileExt, OpenOptionsExt};
use std::os::windows::io::{AsRawHandle, IntoRawHandle};
use std::path::Path;
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_SHARE_READ, FILE_SHARE_WRITE, GetLogicalDrives,
};
use windows_sys::Win32::System::IO::DeviceIoControl;
use windows_sys::Win32::System::Registry::{
    HKEY_LOCAL_MACHINE, KEY_QUERY_VALUE, RegCloseKey, RegOpenKeyExW,
};

use crate::workflow::{self, PhysicalDisk, Platform, Volume, VolumeAccess};
use crate::{
    BootstrapDisk, Error, ReadAt, Result, SECTOR_SIZE, Sector, VolumeExtent, VolumeIdentity,
    emit_event, files, le32, parse_volume_extent,
};

const IOCTL_DISK_GEOMETRY: u32 = 0x70000;
const IOCTL_DISK_LENGTH: u32 = 0x7405c;
const IOCTL_STORAGE_NUMBER: u32 = 0x2d1080;
const IOCTL_VOLUME_EXTENTS: u32 = 0x560000;

struct Device {
    file: Option<File>,
    name: String,
}

impl Device {
    fn open(name: String, writable: bool) -> Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .write(writable)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .open(&name)
            .map_err(|error| Error::io(name.clone(), error))?;
        Ok(Self {
            file: Some(file),
            name,
        })
    }

    fn file(&self) -> io::Result<&File> {
        self.file
            .as_ref()
            .ok_or_else(|| io::Error::from(io::ErrorKind::NotConnected))
    }

    fn info<const N: usize>(&self, control: u32) -> Result<[u8; N]> {
        let size = u32::try_from(N).map_err(|_| Error::IoctlBuffer)?;
        if size == 0 {
            return Err(Error::IoctlBuffer);
        }
        let file = self.file().map_err(|error| Error::io(&self.name, error))?;
        let mut data = [0_u8; N];
        let mut returned = 0;
        // SAFETY: the file owns a live synchronous handle. The output array and
        // byte count remain writable for the call; no input/OVERLAPPED is used.
        let succeeded = unsafe {
            DeviceIoControl(
                file.as_raw_handle() as HANDLE,
                control,
                null(),
                0,
                data.as_mut_ptr().cast(),
                size,
                &mut returned,
                null_mut(),
            )
        };
        if succeeded == 0 {
            return Err(Error::io(
                format!("{} ioctl {control:#x}", self.name),
                io::Error::last_os_error(),
            ));
        }
        if returned != size {
            return Err(Error::IoctlLength {
                name: self.name.clone(),
                control,
                returned,
                expected: N,
            });
        }
        Ok(data)
    }

    fn sync(&self) -> Result<()> {
        self.file()
            .and_then(File::sync_all)
            .map_err(|error| Error::io(&self.name, error))
    }

    fn close(&mut self) -> Result<()> {
        if let Some(file) = self.file.take() {
            close_file(file)?;
        }
        Ok(())
    }
}

fn close_file(file: File) -> Result<()> {
    let handle = file.into_raw_handle();
    // SAFETY: into_raw_handle transfers this uniquely owned file handle.
    // The File will no longer drop it; CloseHandle is attempted exactly once.
    if unsafe { CloseHandle(handle as HANDLE) } == 0 {
        return Err(Error::io("closing file/device", io::Error::last_os_error()));
    }
    Ok(())
}

struct WindowsVolume(Device);

impl Volume for WindowsVolume {
    fn identity(&mut self, drive: &str) -> Result<VolumeIdentity> {
        let data = self.0.info::<12>(IOCTL_STORAGE_NUMBER)?;
        Ok(VolumeIdentity {
            drive: drive.into(),
            device_type: le32(&data, 0),
            disk_index: le32(&data, 4),
            partition_index: le32(&data, 8),
        })
    }

    fn extent(&mut self) -> Result<VolumeExtent> {
        parse_volume_extent(&self.0.info::<32>(IOCTL_VOLUME_EXTENTS)?)
    }

    fn sync(&mut self) -> Result<()> {
        self.0.sync()
    }

    fn close(&mut self) -> Result<()> {
        self.0.close()
    }
}

struct WindowsDisk(Device);

impl ReadAt for WindowsDisk {
    fn read_at(&mut self, data: &mut [u8], offset: u64) -> io::Result<usize> {
        if data.len() != SECTOR_SIZE || !offset.is_multiple_of(512) {
            return Err(io::Error::from(io::ErrorKind::InvalidInput));
        }
        self.0.file()?.seek_read(data, offset)
    }
}

impl BootstrapDisk for WindowsDisk {
    fn write_at(&mut self, data: &[u8], offset: u64) -> io::Result<usize> {
        if data.len() != SECTOR_SIZE || offset != 0 {
            return Err(io::Error::from(io::ErrorKind::InvalidInput));
        }
        self.0.file()?.seek_write(data, offset)
    }

    fn sync(&mut self) -> io::Result<()> {
        self.0.file()?.sync_all()
    }
}

impl PhysicalDisk for WindowsDisk {
    fn device_number(&mut self) -> Result<[u8; 12]> {
        self.0.info(IOCTL_STORAGE_NUMBER)
    }

    fn geometry(&mut self) -> Result<[u8; 24]> {
        self.0.info(IOCTL_DISK_GEOMETRY)
    }

    fn length(&mut self) -> Result<[u8; 8]> {
        self.0.info(IOCTL_DISK_LENGTH)
    }

    fn close(&mut self) -> Result<()> {
        self.0.close()
    }
}

struct WindowsPlatform;

impl Platform for WindowsPlatform {
    type Volume = WindowsVolume;
    type Disk = WindowsDisk;

    fn require_windows_pe(&mut self) -> Result<()> {
        let mini_nt: Vec<u16> = r"SYSTEM\CurrentControlSet\Control\MiniNT"
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let mut key = null_mut();
        // SAFETY: HKLM is predefined, mini_nt is a terminated static registry
        // path, and key is writable. Only query permission is requested.
        let status = unsafe {
            RegOpenKeyExW(
                HKEY_LOCAL_MACHINE,
                mini_nt.as_ptr(),
                0,
                KEY_QUERY_VALUE,
                &mut key,
            )
        };
        if status != 0 {
            return Err(Error::io(
                "opening MiniNT registry key",
                io::Error::from_raw_os_error(status as i32),
            ));
        }
        // SAFETY: successful RegOpenKeyExW returned this owned registry handle.
        let status = unsafe { RegCloseKey(key) };
        if status != 0 {
            return Err(Error::io(
                "closing MiniNT registry key",
                io::Error::from_raw_os_error(status as i32),
            ));
        }
        Ok(())
    }

    fn require_file(&mut self, path: &str) -> Result<()> {
        files::require_file(Path::new(path))
    }

    fn logical_drives(&mut self) -> Result<u32> {
        // SAFETY: GetLogicalDrives takes no pointers or ownership.
        let mask = unsafe { GetLogicalDrives() };
        if mask == 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(0) {
                return Err(Error::io("enumerating mounted drive letters", error));
            }
            return Err(Error::NoLogicalDrives);
        }
        Ok(mask)
    }

    fn open_volume(&mut self, drive: &str, access: VolumeAccess) -> Result<Self::Volume> {
        Device::open(format!(r"\\.\{drive}"), access == VolumeAccess::ReadWrite).map(WindowsVolume)
    }

    fn open_disk(&mut self, index: u32) -> Result<Self::Disk> {
        Device::open(format!(r"\\.\PhysicalDrive{index}"), true).map(WindowsDisk)
    }

    fn save_backup(&mut self, directory: &str, sector: &Sector) -> Result<()> {
        files::save_backup(Path::new(directory), sector, close_file, emit_event)
    }

    fn event(&mut self, level: &str, message: &str) {
        emit_event(level, message);
    }
}

pub fn run() -> Result<&'static str> {
    workflow::run(&mut WindowsPlatform, |name| {
        std::env::var(name).unwrap_or_default()
    })
}
