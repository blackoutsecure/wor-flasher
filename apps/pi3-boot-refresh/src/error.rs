use std::io;
use std::num::ParseIntError;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("missing or unsupported WOR_DEVICE_TYPE; refusing to guess the target board")]
    Board,
    #[error("missing or unsupported WoR partition scheme")]
    PartitionScheme,
    #[error("the Pi 3 bootstrap requires a desktop ARM64 installation")]
    Architecture,
    #[error("WOR_DISK_INDEX must explicitly identify the selected disk")]
    DiskIndexRequired,
    #[error("invalid WOR_DISK_INDEX: {0}")]
    DiskIndex(#[source] ParseIntError),
    #[error("{0} must be a drive letter followed by a colon")]
    DriveLetter(&'static str),
    #[error("boot-volume discovery returned an invalid drive letter")]
    DiscoveryDrive,
    #[error("the Windows volume is the selected disk's boot partition")]
    WindowsIsBoot,
    #[error("multiple mounted volumes claim the selected disk's first partition")]
    AmbiguousBoot,
    #[error("no mounted volume identifies partition 1 of selected disk {0}")]
    MissingBoot(u32),
    #[error("the exported boot drive does not identify the selected disk's first partition")]
    BootExport,
    #[error("the raw device is not the selected physical disk")]
    RawDiskIdentity,
    #[error("the raw device is a partition, not a whole disk")]
    NotWholeDisk,
    #[error("a target volume must have exactly one physical-disk extent")]
    ExtentCount,
    #[error("invalid or unaligned volume extent")]
    ExtentGeometry,
    #[error("Pi 3 GPT bootstrap requires a valid physical disk with 512-byte logical sectors")]
    DiskGeometry,
    #[error("boot and Windows volume extents must belong to the selected physical disk")]
    ExtentBinding,
    #[error("Windows must follow, and not overlap, the boot partition")]
    ExtentOverlap,
    #[error("sector offset exceeds the supported disk range")]
    SectorRange,
    #[error("read sector {lba}: {source}")]
    ReadSector {
        lba: u64,
        #[source]
        source: io::Error,
    },
    #[error("short read at sector {lba}: {bytes} bytes")]
    ShortRead { lba: u64, bytes: usize },
    #[error("target does not contain a GPT header at LBA 1")]
    GptSignature,
    #[error("GPT header size is invalid")]
    GptHeaderSize,
    #[error("GPT header checksum is invalid")]
    GptHeaderChecksum,
    #[error("GPT bounds do not match the selected physical disk")]
    GptBounds,
    #[error("GPT partition-entry geometry is invalid for the Pi 3 bootstrap")]
    GptTableGeometry,
    #[error("GPT partition has invalid bounds")]
    GptPartitionBounds,
    #[error("the boot volume must be GPT partition 1 with an EFI or Basic Data type")]
    BootPartitionType,
    #[error("the Windows volume must be a Microsoft Basic Data partition")]
    WindowsPartitionType,
    #[error("GPT partition-entry checksum is invalid")]
    GptTableChecksum,
    #[error("GPT partitions do not uniquely match both selected volume extents")]
    GptExtentMatch,
    #[error("FAT boot-sector signature or sector size is invalid")]
    FatSignature,
    #[error("FAT allocation geometry is invalid")]
    FatAllocation,
    #[error("boot partition must contain a FAT32 version 0 filesystem")]
    FatVersion,
    #[error("FAT32 active FAT index is invalid")]
    FatActiveIndex,
    #[error("FAT filesystem exceeds its partition bounds or has no data region")]
    FatBounds,
    #[error("FAT32 data-cluster count, table size or root cluster is invalid")]
    FatClusters,
    #[error("Pi 3 FAT bootstrap offsets exceed their 16-bit or 32-bit limits")]
    BootstrapOffsets,
    #[error("FAT32 metadata pointer is outside the reserved region")]
    FatPointer,
    #[error("boot-sector backup failed; no boot-sector write attempted: {0}")]
    Backup(#[source] Box<Error>),
    #[error("disk metadata changed while backing up; no boot-sector write attempted")]
    MetadataChanged,
    #[error("boot-sector write failed; preserve the backup and do not reboot: {0}")]
    Write(#[source] io::Error),
    #[error("short boot-sector write: {0} of 512 bytes; do not reboot")]
    ShortWrite(usize),
    #[error("boot-sector flush failed; do not reboot: {0}")]
    Flush(#[source] io::Error),
    #[error("post-write validation failed; do not reboot: {0}")]
    PostWrite(#[source] Box<Error>),
    #[error("boot-sector read-back or GPT/FAT metadata verification failed; do not reboot")]
    ReadBack,
    #[error("{operation}: {source}")]
    Io {
        operation: String,
        #[source]
        source: io::Error,
    },
    #[error("{operation}: {source}")]
    Context {
        operation: &'static str,
        #[source]
        source: Box<Error>,
    },
    #[error("{primary}\n{close}")]
    Close {
        #[source]
        primary: Box<Error>,
        close: Box<Error>,
    },
    #[error("{name} ioctl {control:#x} returned {returned} bytes, expected {expected}")]
    IoctlLength {
        name: String,
        control: u32,
        returned: u32,
        expected: usize,
    },
    #[error("invalid device-control buffer length")]
    IoctlBuffer,
    #[error("required installation file is absent or empty: {0}")]
    RequiredFile(String),
    #[error("GetLogicalDrives returned no mounted volumes")]
    NoLogicalDrives,
    #[error("the Windows volume does not belong to the selected disk")]
    WindowsVolumeDisk,
    #[error("WoR boot/Windows drive letters do not belong to the selected disk")]
    VolumeDisks,
    #[error("Pi 3 boot finalization requires Windows PE; no disk accessed")]
    UnsupportedPlatform,
}

impl Error {
    pub fn io(operation: impl Into<String>, source: io::Error) -> Self {
        Self::Io {
            operation: operation.into(),
            source,
        }
    }

    pub fn context(self, operation: &'static str) -> Self {
        Self::Context {
            operation,
            source: Box::new(self),
        }
    }
}
