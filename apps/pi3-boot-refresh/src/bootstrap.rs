use sha2::{Digest, Sha256};

use crate::{
    BootstrapDisk, DiskLayout, Error, ReadAt, Result, SECTOR_SIZE, Sector, VolumeExtent, le16,
    le32, le64,
};

const EFI_TYPE: [u8; 16] = [
    0x28, 0x73, 0x2a, 0xc1, 0x1f, 0xf8, 0xd2, 0x11, 0xba, 0x4b, 0, 0xa0, 0xc9, 0x3e, 0xc9, 0x3b,
];
const BASIC_TYPE: [u8; 16] = [
    0xa2, 0xa0, 0xd0, 0xeb, 0xe5, 0xb9, 0x33, 0x44, 0x87, 0xc0, 0x68, 0xb6, 0xb7, 0x26, 0x99, 0xc7,
];

pub fn read_sector(disk: &mut impl ReadAt, lba: u64) -> Result<Sector> {
    if lba > i64::MAX as u64 / 512 {
        return Err(Error::SectorRange);
    }
    let mut data = [0; SECTOR_SIZE];
    let bytes = disk
        .read_at(&mut data, lba * 512)
        .map_err(|source| Error::ReadSector { lba, source })?;
    if bytes != SECTOR_SIZE {
        return Err(Error::ShortRead { lba, bytes });
    }
    Ok(data)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BootstrapPlan {
    pub original: Sector,
    pub replacement: Sector,
    pub metadata: [u8; 32],
}

pub fn build_bootstrap(disk: &mut impl ReadAt, layout: &DiskLayout) -> Result<BootstrapPlan> {
    layout.validate()?;
    let header = read_sector(disk, 1)?;
    if &header[..8] != b"EFI PART" {
        return Err(Error::GptSignature);
    }
    let header_size = le32(&header, 12) as usize;
    if !(92..=SECTOR_SIZE).contains(&header_size) {
        return Err(Error::GptHeaderSize);
    }
    let mut checksum_header = header;
    checksum_header[16..20].fill(0);
    if crc32fast::hash(&checksum_header[..header_size]) != le32(&header, 16) {
        return Err(Error::GptHeaderChecksum);
    }
    let backup_lba = le64(&header, 32);
    let first_usable = le64(&header, 40);
    let last_usable = le64(&header, 48);
    if le64(&header, 24) != 1
        || first_usable < 34
        || first_usable > last_usable
        || last_usable >= backup_lba
        || backup_lba != layout.bytes / 512 - 1
    {
        return Err(Error::GptBounds);
    }
    let entries_lba = le64(&header, 72);
    let entry_count = u64::from(le32(&header, 80));
    let entry_size = u64::from(le32(&header, 84));
    let table_size = entry_count
        .checked_mul(entry_size)
        .ok_or(Error::GptTableGeometry)?;
    if entry_count == 0
        || entry_size < 128
        || !entry_size.is_multiple_of(128)
        || entries_lba < 2
        || entries_lba >= first_usable
        || table_size.div_ceil(512) > first_usable - entries_lba
        || first_usable > layout.boot.offset / 512
        || layout.boot.offset / 512 > 65535
    {
        return Err(Error::GptTableGeometry);
    }
    let mut digest = Sha256::new();
    let mut checksum = crc32fast::Hasher::new();
    digest.update(header);
    let mut boot_matches = 0;
    let mut windows_matches = 0;
    for offset in (0..table_size).step_by(SECTOR_SIZE) {
        let block = read_sector(disk, entries_lba + offset / 512)?;
        let count = (table_size - offset).min(512) as usize;
        checksum.update(&block[..count]);
        digest.update(&block[..count]);
        for within in (0..count).step_by(128) {
            if !(offset + within as u64).is_multiple_of(entry_size) {
                continue;
            }
            let entry = &block[within..within + 128];
            if entry[..16] == [0; 16] {
                continue;
            }
            let start = le64(entry, 32);
            let end = le64(entry, 40);
            if start < first_usable || start > end || end > last_usable {
                return Err(Error::GptPartitionBounds);
            }
            let matches = |extent: VolumeExtent| {
                start * 512 == extent.offset && (end - start + 1) * 512 == extent.length
            };
            if matches(layout.boot) {
                if offset + within as u64 != 0
                    || (entry[..16] != EFI_TYPE && entry[..16] != BASIC_TYPE)
                {
                    return Err(Error::BootPartitionType);
                }
                boot_matches += 1;
            }
            if matches(layout.windows) {
                if entry[..16] != BASIC_TYPE {
                    return Err(Error::WindowsPartitionType);
                }
                windows_matches += 1;
            }
        }
    }
    if checksum.finalize() != le32(&header, 88) {
        return Err(Error::GptTableChecksum);
    }
    if boot_matches != 1 || windows_matches != 1 {
        return Err(Error::GptExtentMatch);
    }

    let start_lba = layout.boot.offset / 512;
    let boot = read_sector(disk, start_lba)?;
    digest.update(boot);
    if boot[510] != 0x55 || boot[511] != 0xaa || le16(&boot, 11) != 512 {
        return Err(Error::FatSignature);
    }
    let cluster_size = u64::from(boot[13]);
    let reserved = u64::from(le16(&boot, 14));
    let fat_count = u64::from(boot[16]);
    let total = u64::from(le32(&boot, 32));
    let fat_size = u64::from(le32(&boot, 36));
    let fat_flags = le16(&boot, 40);
    let root_cluster = u64::from(le32(&boot, 44));
    if !cluster_size.is_power_of_two()
        || reserved == 0
        || (fat_count != 1 && fat_count != 2)
        || fat_size == 0
    {
        return Err(Error::FatAllocation);
    }
    if le16(&boot, 17) != 0 || le16(&boot, 19) != 0 || le16(&boot, 22) != 0 || le16(&boot, 42) != 0
    {
        return Err(Error::FatVersion);
    }
    if fat_flags & 0x80 != 0 && u64::from(fat_flags & 0xf) >= fat_count {
        return Err(Error::FatActiveIndex);
    }
    if total > layout.boot.length / 512 || total <= reserved + fat_count * fat_size {
        return Err(Error::FatBounds);
    }
    let clusters = (total - reserved - fat_count * fat_size) / cluster_size;
    if !(65525..0x0fff_fff5).contains(&clusters)
        || (clusters + 2) * 4 > fat_size * 512
        || root_cluster < 2
        || root_cluster >= clusters + 2
    {
        return Err(Error::FatClusters);
    }
    if start_lba + reserved > 65535 || start_lba + total > u64::from(u32::MAX) {
        return Err(Error::BootstrapOffsets);
    }
    let original = read_sector(disk, 0)?;
    let mut replacement = [0; SECTOR_SIZE];
    replacement[440..446].copy_from_slice(&original[440..446]);
    replacement[11..90].copy_from_slice(&boot[11..90]);
    let shifted_reserved =
        u16::try_from(start_lba + reserved).map_err(|_| Error::BootstrapOffsets)?;
    let shifted_total = u32::try_from(start_lba + total).map_err(|_| Error::BootstrapOffsets)?;
    replacement[14..16].copy_from_slice(&shifted_reserved.to_le_bytes());
    replacement[32..36].copy_from_slice(&shifted_total.to_le_bytes());
    for offset in [48, 50] {
        let relative = u64::from(le16(&boot, offset));
        if relative != 0 && relative != 0xffff {
            if relative >= reserved {
                return Err(Error::FatPointer);
            }
            let shifted = u16::try_from(start_lba + relative).map_err(|_| Error::FatPointer)?;
            replacement[offset..offset + 2].copy_from_slice(&shifted.to_le_bytes());
        }
    }
    // Match the desktop helper: never add a legacy FAT/hybrid partition entry.
    let protective_size = backup_lba.min(u64::from(u32::MAX)) as u32;
    replacement[447..450].copy_from_slice(&lba_to_chs(1));
    replacement[450] = 0xee;
    replacement[451..454].copy_from_slice(&lba_to_chs(u64::from(protective_size)));
    replacement[454..458].copy_from_slice(&1_u32.to_le_bytes());
    replacement[458..462].copy_from_slice(&protective_size.to_le_bytes());
    replacement[510..512].copy_from_slice(&[0x55, 0xaa]);
    Ok(BootstrapPlan {
        original,
        replacement,
        metadata: digest.finalize().into(),
    })
}

pub fn lba_to_chs(lba: u64) -> [u8; 3] {
    let sector = lba % 63 + 1;
    let track = lba / 63;
    let head = track % 255;
    let cylinder = track / 255;
    if cylinder > 1023 {
        return [0xfe, 0xff, 0xff];
    }
    [
        head as u8,
        (sector | ((cylinder >> 2) & 0xc0)) as u8,
        cylinder as u8,
    ]
}

/// Writes only sector zero after a durable backup and a second complete validation.
/// No rollback/retry is attempted after a partial or unverifiable write.
pub fn refresh_bootstrap(
    disk: &mut impl BootstrapDisk,
    layout: &DiskLayout,
    mut backup: impl FnMut(&Sector) -> Result<()>,
) -> Result<bool> {
    let plan = build_bootstrap(disk, layout)?;
    if plan.original == plan.replacement {
        return Ok(false);
    }
    backup(&plan.original).map_err(|source| Error::Backup(Box::new(source)))?;
    let current = build_bootstrap(disk, layout)?;
    if current != plan {
        return Err(Error::MetadataChanged);
    }
    let bytes = disk.write_at(&plan.replacement, 0).map_err(Error::Write)?;
    if bytes != SECTOR_SIZE {
        return Err(Error::ShortWrite(bytes));
    }
    disk.sync().map_err(Error::Flush)?;
    let verified =
        build_bootstrap(disk, layout).map_err(|source| Error::PostWrite(Box::new(source)))?;
    if verified.original != plan.replacement
        || verified.replacement != plan.replacement
        || verified.metadata != plan.metadata
    {
        return Err(Error::ReadBack);
    }
    Ok(true)
}
