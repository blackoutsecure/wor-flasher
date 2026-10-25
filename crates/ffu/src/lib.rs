//! Bounded native FFU parser and verified, single-writer IoT Core application.
//!
//! This is a source-anchored replacement for `src/lib/iot-ffu.py`. Parsing does
//! not authorize an image or a target. Production inspection/application always
//! requires the compiled, digest-pinned official-image provenance. The explicitly
//! named fixture API can open only existing, singly linked regular target files.
//!
//! The caller must separately confirm a whole disk, exclude host/source/runtime/
//! log disks, and unmount volumes. Native device writes remain experimental until
//! hardware-qualified. No function here discovers or selects a disk for the user.

mod cli;
mod error;
mod hdmi;
mod parser;
mod source;
mod trust;
mod unix;
mod writer;

pub use cli::{run, run_with_io};
pub use error::{Error, Result};
pub use hdmi::{HdmiChoice, HdmiOptions, hdmi_options};
pub use parser::{
    DiskLocation, Extent, ParsedFfu, Phase, WriteDescriptor, WritePlan, parse_ffu,
    parse_ffu_with_cancel, resolve_plan,
};
pub use source::{TargetIdentity, TargetIdentityReport, identify_target};
pub use trust::{
    ApprovedImage, ImageProfile, Inspection, InspectionHdmi, PROVENANCE_SHA256, image_profile,
    inspect_ffu, inspect_ffu_with_cancel, load_provenance,
};
pub use writer::{ApplyOptions, Progress, apply_ffu, apply_to_file_fixture};

use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

pub const MAX_IMAGE_BYTES: u64 = 16 * 1024 * 1024 * 1024;
pub const MAX_TARGET_BYTES: u64 = 2 * 1024 * 1024 * 1024 * 1024;
pub const MAX_CATALOG_BYTES: usize = 1024 * 1024;
pub const MAX_MANIFEST_BYTES: usize = 64 * 1024;
pub const MAX_HASH_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_DESCRIPTORS: u32 = 131_072;
pub const MAX_LOCATIONS: u32 = 16;
pub const MAX_EXTENTS: usize = 262_144;
pub const MAX_GPT_TABLE_BYTES: u64 = 1024 * 1024;
pub const WIPE_CHUNK_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_HDMI_CONFIG_BYTES: usize = 512;
pub const MAX_BOOT_CONFIG_BYTES: usize = 4096;
pub const SECTOR_SIZE: u64 = 512;

static NEVER_CANCELLED: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy)]
pub(crate) struct Cancel<'a> {
    pub flag: &'a AtomicBool,
    pub signal: Option<&'a AtomicI32>,
}

impl<'a> Cancel<'a> {
    pub fn new(flag: &'a AtomicBool) -> Self {
        Self { flag, signal: None }
    }

    pub fn check(self) -> Result<()> {
        if let Some(signal) = self.signal {
            let number = signal.load(Ordering::Relaxed);
            if number != 0 {
                return Err(Error::Interrupted { signal: number });
            }
        }
        if self.flag.load(Ordering::Relaxed) {
            return Err(Error::Interrupted { signal: 2 });
        }
        Ok(())
    }
}

pub(crate) fn align(value: u64, alignment: u64) -> u64 {
    value.div_ceil(alignment) * alignment
}

pub(crate) fn u16_at(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(bytes[offset..offset + 2].try_into().expect("bounded field"))
}

pub(crate) fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().expect("bounded field"))
}

pub(crate) fn u64_at(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(bytes[offset..offset + 8].try_into().expect("bounded field"))
}

#[cfg(test)]
mod tests;
