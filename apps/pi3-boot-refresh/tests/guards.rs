mod support;

use support::*;
use wor_pi3_boot_refresh::{DiskLayout, refresh_bootstrap};

fn reject_before_backup(mut disk: MemoryDisk, layout: DiskLayout, expected: &str) {
    let before = disk.image.borrow().clone();
    let mut backups = 0;
    let error = refresh_bootstrap(&mut disk, &layout, |_| {
        backups += 1;
        Ok(())
    })
    .unwrap_err();
    assert!(
        error.to_string().contains(expected),
        "expected {expected:?}, got {error}"
    );
    assert_eq!(backups, 0);
    assert_eq!(disk.writes, 0);
    assert_eq!(disk.syncs, 0);
    assert_eq!(*disk.image.borrow(), before);
}

#[test]
fn invalid_layouts_fail_before_any_backup_write_or_unbounded_read() {
    for name in [
        "sector-size",
        "unaligned-size",
        "small-disk",
        "signed-overflow",
        "boot-disk",
        "windows-disk",
        "boot-zero-offset",
        "boot-zero-length",
        "windows-zero-offset",
        "windows-zero-length",
        "boot-unaligned-offset",
        "boot-unaligned-length",
        "windows-unaligned-offset",
        "windows-unaligned-length",
        "boot-outside",
        "windows-outside",
        "boot-length",
        "windows-length",
        "overlap",
        "aliased",
        "windows-before-boot",
    ] {
        let (disk, mut layout) = fixture(Geometry::default());
        match name {
            "sector-size" => layout.bytes_per_sector = 4096,
            "unaligned-size" => layout.bytes += 1,
            "small-disk" => layout.bytes = 67 * 512,
            "signed-overflow" => layout.bytes = i64::MAX as u64 + 1,
            "boot-disk" => layout.boot.disk_index += 1,
            "windows-disk" => layout.windows.disk_index += 1,
            "boot-zero-offset" => layout.boot.offset = 0,
            "boot-zero-length" => layout.boot.length = 0,
            "windows-zero-offset" => layout.windows.offset = 0,
            "windows-zero-length" => layout.windows.length = 0,
            "boot-unaligned-offset" => layout.boot.offset += 1,
            "boot-unaligned-length" => layout.boot.length += 1,
            "windows-unaligned-offset" => layout.windows.offset += 1,
            "windows-unaligned-length" => layout.windows.length += 1,
            "boot-outside" => layout.boot.offset = layout.bytes,
            "windows-outside" => layout.windows.offset = layout.bytes,
            "boot-length" => layout.boot.length = u64::MAX - 511,
            "windows-length" => layout.windows.length = layout.bytes,
            "overlap" => layout.windows.offset = layout.boot.offset + 512,
            "aliased" => layout.windows = layout.boot,
            "windows-before-boot" => layout.windows.offset = 512,
            _ => unreachable!(),
        }
        reject_before_backup(
            disk,
            layout,
            if name.contains("overlap") || name == "aliased" || name == "windows-before-boot" {
                "not overlap"
            } else if [
                "sector-size",
                "unaligned-size",
                "small-disk",
                "signed-overflow",
            ]
            .contains(&name)
            {
                "512-byte logical sectors"
            } else {
                "selected physical disk"
            },
        );
    }
}

#[test]
fn gpt_header_and_crc_bounds_cannot_be_bypassed() {
    for name in [
        "signature",
        "size-small",
        "size-large",
        "checksum",
        "current-lba",
        "backup-lba",
        "first-usable",
        "empty-usable",
        "last-usable",
        "entry-count",
        "max-entry-count",
        "zero-entry-size",
        "max-entry-size",
        "entry-size-small",
        "entry-size-alignment",
        "entry-lba-small",
        "entry-lba-high",
        "table-overlap",
        "boot-before-usable",
    ] {
        let (disk, layout) = fixture(Geometry::default());
        let expected;
        {
            let mut image = disk.image.borrow_mut();
            let header = image.get_mut(&1).unwrap();
            expected = match name {
                "signature" => {
                    header[0] = 0;
                    "GPT header at LBA 1"
                }
                "size-small" => {
                    put32(header, 12, 91);
                    "header size"
                }
                "size-large" => {
                    put32(header, 12, 513);
                    "header size"
                }
                "checksum" => {
                    header[56] = 0xff;
                    "header checksum"
                }
                "current-lba" => {
                    put64(header, 24, 0);
                    "GPT bounds"
                }
                "backup-lba" => {
                    put64(header, 32, layout.bytes / 512 - 2);
                    "GPT bounds"
                }
                "first-usable" => {
                    put64(header, 40, 33);
                    "GPT bounds"
                }
                "empty-usable" => {
                    put64(header, 40, layout.bytes / 512 - 30);
                    "GPT bounds"
                }
                "last-usable" => {
                    put64(header, 48, layout.bytes / 512 - 1);
                    "GPT bounds"
                }
                "entry-count" => {
                    put32(header, 80, 0);
                    "partition-entry geometry"
                }
                "max-entry-count" => {
                    put32(header, 80, u32::MAX);
                    "partition-entry geometry"
                }
                "zero-entry-size" => {
                    put32(header, 84, 0);
                    "partition-entry geometry"
                }
                "max-entry-size" => {
                    put32(header, 84, u32::MAX - 127);
                    "partition-entry geometry"
                }
                "entry-size-small" => {
                    put32(header, 84, 64);
                    "partition-entry geometry"
                }
                "entry-size-alignment" => {
                    put32(header, 84, 129);
                    "partition-entry geometry"
                }
                "entry-lba-small" => {
                    put64(header, 72, 1);
                    "partition-entry geometry"
                }
                "entry-lba-high" => {
                    put64(header, 72, 34);
                    "partition-entry geometry"
                }
                "table-overlap" => {
                    put32(header, 84, 256);
                    "partition-entry geometry"
                }
                "boot-before-usable" => {
                    put64(header, 40, layout.boot.offset / 512 + 1);
                    "partition-entry geometry"
                }
                _ => unreachable!(),
            };
            if !["signature", "size-small", "size-large", "checksum"].contains(&name) {
                header_crc(&mut image);
            }
        }
        reject_before_backup(disk, layout, expected);
    }
    let (disk, layout) = fixture(Geometry {
        start: 65536,
        ..Geometry::default()
    });
    reject_before_backup(disk, layout, "partition-entry geometry");
}

#[test]
fn every_used_gpt_partition_and_both_volume_matches_are_checked() {
    for name in [
        "table-checksum",
        "partition-start",
        "partition-end",
        "partition-order",
        "boot-type",
        "windows-type",
        "other-partition-bounds",
        "boot-not-first",
        "missing-windows",
        "duplicate-windows",
        "wrong-boot-offset",
        "wrong-windows-length",
        "wrong-disk-length",
    ] {
        let (disk, mut layout) = fixture(Geometry::default());
        let expected;
        {
            let mut image = disk.image.borrow_mut();
            let table = image.get_mut(&2).unwrap();
            expected = match name {
                "table-checksum" => {
                    table[16] = 0xff;
                    "partition-entry checksum"
                }
                "partition-start" => {
                    put64(table, 32, 33);
                    "partition has invalid bounds"
                }
                "partition-end" => {
                    put64(table, 40, layout.bytes / 512);
                    "partition has invalid bounds"
                }
                "partition-order" => {
                    put64(table, 40, 2047);
                    "partition has invalid bounds"
                }
                "boot-type" => {
                    table[0] ^= 0xff;
                    "boot volume must be GPT partition 1"
                }
                "windows-type" => {
                    table[128..144].copy_from_slice(&EFI_TYPE);
                    "Windows volume must be"
                }
                "other-partition-bounds" => {
                    table[256..272].copy_from_slice(&BASIC_TYPE);
                    "partition has invalid bounds"
                }
                "boot-not-first" => {
                    let boot: [u8; 128] = table[..128].try_into().unwrap();
                    table[..128].fill(0);
                    table[256..384].copy_from_slice(&boot);
                    "boot volume must be GPT partition 1"
                }
                "missing-windows" => {
                    table[128..256].fill(0);
                    "uniquely match"
                }
                "duplicate-windows" => {
                    let windows: [u8; 128] = table[128..256].try_into().unwrap();
                    table[256..384].copy_from_slice(&windows);
                    "uniquely match"
                }
                "wrong-boot-offset" => {
                    layout.boot.offset += 512;
                    "uniquely match"
                }
                "wrong-windows-length" => {
                    layout.windows.length -= 512;
                    "uniquely match"
                }
                "wrong-disk-length" => {
                    layout.bytes += 512;
                    "GPT bounds"
                }
                _ => unreachable!(),
            };
            if name != "table-checksum" {
                table_crc(&mut image);
            }
        }
        reject_before_backup(disk, layout, expected);
    }
}

#[test]
fn fat32_geometry_bounds_version_and_metadata_pointers_fail_closed() {
    for (offset, value, expected) in [
        (510, vec![0, 0], "signature"),
        (11, 4096_u16.to_le_bytes().to_vec(), "sector size"),
        (13, vec![0], "allocation geometry"),
        (13, vec![3], "allocation geometry"),
        (14, vec![0, 0], "allocation geometry"),
        (16, vec![0], "allocation geometry"),
        (16, vec![3], "allocation geometry"),
        (36, 0_u32.to_le_bytes().to_vec(), "allocation geometry"),
        (17, vec![1, 0], "FAT32 version 0"),
        (19, vec![1, 0], "FAT32 version 0"),
        (22, vec![1, 0], "FAT32 version 0"),
        (42, vec![1, 0], "FAT32 version 0"),
        (40, vec![0x82, 0], "active FAT index"),
        (32, 200000_u32.to_le_bytes().to_vec(), "partition bounds"),
        (32, 0_u32.to_le_bytes().to_vec(), "data region"),
        (32, 1_u32.to_le_bytes().to_vec(), "data region"),
        (
            32,
            (32_u32 + 2 * 1024).to_le_bytes().to_vec(),
            "data region",
        ),
        (
            32,
            (65524_u32 + 32 + 2048).to_le_bytes().to_vec(),
            "data-cluster count",
        ),
        (36, 1_u32.to_le_bytes().to_vec(), "table size"),
        (44, 1_u32.to_le_bytes().to_vec(), "root cluster"),
        (44, 200000_u32.to_le_bytes().to_vec(), "root cluster"),
        (48, 32_u16.to_le_bytes().to_vec(), "metadata pointer"),
        (50, 33_u16.to_le_bytes().to_vec(), "metadata pointer"),
    ] {
        let (disk, layout) = fixture(Geometry::default());
        disk.image.borrow_mut().get_mut(&2048).unwrap()[offset..offset + value.len()]
            .copy_from_slice(&value);
        reject_before_backup(disk, layout, expected);
    }
    let (disk, layout) = fixture(Geometry {
        start: 65520,
        ..Geometry::default()
    });
    reject_before_backup(disk, layout, "bootstrap offsets");
    let (disk, layout) = fixture(Geometry {
        count: u32::MAX,
        cluster: 128,
        fat: 262144,
        ..Geometry::default()
    });
    reject_before_backup(disk, layout, "bootstrap offsets");
    let (disk, layout) = fixture(Geometry {
        count: u32::MAX,
        fat: 33554432,
        ..Geometry::default()
    });
    reject_before_backup(disk, layout, "data-cluster count");
}

#[test]
fn metadata_read_errors_and_short_reads_never_reach_backup_or_write() {
    for lba in [1, 2, 2048, 0] {
        for short in [false, true] {
            let (mut disk, layout) = fixture(Geometry::default());
            if short {
                disk.short_read = Some(lba);
            } else {
                disk.read_error = Some(lba);
            }
            reject_before_backup(
                disk,
                layout,
                if short { "short read" } else { "read sector" },
            );
        }
    }
}
