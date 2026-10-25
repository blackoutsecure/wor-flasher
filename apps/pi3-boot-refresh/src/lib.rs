//! Native Pi 3 finalization core. The shipped Go helper is not replaced by this crate.

#![deny(unsafe_code)]
#![cfg_attr(not(windows), forbid(unsafe_code))]

use std::io;

mod bootstrap;
mod context;
mod error;

pub mod files;
pub mod workflow;

#[cfg(windows)]
#[allow(unsafe_code)]
pub mod windows;

pub use bootstrap::{BootstrapPlan, build_bootstrap, lba_to_chs, read_sector, refresh_bootstrap};
pub use context::{
    DiskLayout, InstallContext, VolumeExtent, VolumeIdentity, parse_context, parse_volume_extent,
    select_boot_drive, validate_disk_number,
};
pub use error::Error;

pub const SECTOR_SIZE: usize = 512;
pub type Sector = [u8; SECTOR_SIZE];
pub type Result<T> = std::result::Result<T, Error>;

pub fn format_event(level: &str, message: &str) -> String {
    let message: String = message
        .chars()
        .map(|character| match character {
            '|' => '/',
            '\r' | '\n' => ' ',
            other => other,
        })
        .collect();
    format!("WoR-Event|{level}|{message}")
}

pub fn emit_event(level: &str, message: &str) {
    use std::io::Write;
    let _ = writeln!(std::io::stdout().lock(), "{}", format_event(level, message));
}

pub trait ReadAt {
    fn read_at(&mut self, data: &mut [u8], offset: u64) -> io::Result<usize>;
}

pub trait BootstrapDisk: ReadAt {
    fn write_at(&mut self, data: &[u8], offset: u64) -> io::Result<usize>;
    fn sync(&mut self) -> io::Result<()>;
}

pub(crate) fn le16(data: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([data[offset], data[offset + 1]])
}

pub(crate) fn le32(data: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ])
}

pub(crate) fn le64(data: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
        data[offset + 4],
        data[offset + 5],
        data[offset + 6],
        data[offset + 7],
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wor_events_cannot_inject_fields_or_additional_lines() {
        assert_eq!(
            format_event("LogFatal", "bad|metadata\r\nkeep backup ✓"),
            "WoR-Event|LogFatal|bad/metadata  keep backup ✓"
        );
        assert_eq!(format_event("LogInfo", ""), "WoR-Event|LogInfo|");
    }

    #[test]
    fn sector_offset_overflow_is_rejected_before_calling_a_reader() {
        struct NeverRead;
        impl ReadAt for NeverRead {
            fn read_at(&mut self, _: &mut [u8], _: u64) -> io::Result<usize> {
                panic!("overflowing offset reached the reader");
            }
        }
        assert!(matches!(
            read_sector(&mut NeverRead, i64::MAX as u64 / 512 + 1),
            Err(Error::SectorRange)
        ));
        assert!(matches!(
            read_sector(&mut NeverRead, u64::MAX),
            Err(Error::SectorRange)
        ));
    }
}
