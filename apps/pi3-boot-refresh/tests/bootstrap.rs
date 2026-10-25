mod support;

use support::*;
use wor_pi3_boot_refresh::{build_bootstrap, refresh_bootstrap};

#[test]
fn replacement_is_protective_only_and_exactly_relocates_fat_geometry() {
    let (mut disk, layout) = fixture(Geometry::default());
    let before = disk.image.borrow().clone();
    let plan = build_bootstrap(&mut disk, &layout).unwrap();
    let boot = before[&2048];
    assert_eq!(plan.original, before[&0]);
    assert_eq!(plan.replacement[..11], [0; 11]);
    assert_eq!(plan.replacement[440..446], before[&0][440..446]);
    assert_eq!(plan.replacement[446], 0);
    assert_eq!(plan.replacement[450], 0xee);
    assert_eq!(get32(&plan.replacement, 454), 1);
    assert_eq!(
        get32(&plan.replacement, 458),
        (layout.bytes / 512 - 1) as u32
    );
    assert_eq!(plan.replacement[462..510], [0; 48]);
    assert_eq!(plan.replacement[510..], [0x55, 0xaa]);
    let mut expected_bpb = boot[11..90].to_vec();
    put16(&mut expected_bpb, 3, 2048 + 32);
    put32(&mut expected_bpb, 21, 2048 + 131072);
    put16(&mut expected_bpb, 37, 2048 + 1);
    put16(&mut expected_bpb, 39, 2048 + 6);
    assert_eq!(plan.replacement[11..90], expected_bpb);
    assert_eq!(*disk.image.borrow(), before, "building a plan wrote data");
    assert_eq!(disk.writes, 0);
}

#[test]
fn only_sector_zero_changes_backup_precedes_write_and_second_run_is_read_only() {
    let (mut disk, layout) = fixture(Geometry::default());
    let mut before = disk.image.borrow().clone();
    let original = before.remove(&0).unwrap();
    let mut backups = vec![];
    assert!(
        refresh_bootstrap(&mut disk, &layout, |data| {
            backups.push(*data);
            Ok(())
        })
        .unwrap()
    );
    assert_eq!(backups, vec![original]);
    assert_eq!(disk.writes, 1);
    assert_eq!(disk.syncs, 1);
    let mut after = disk.image.borrow().clone();
    after.remove(&0);
    assert_eq!(after, before);
    assert!(
        !refresh_bootstrap(&mut disk, &layout, |_| panic!("idempotent run backed up")).unwrap()
    );
    assert_eq!(disk.writes, 1);
    assert_eq!(disk.syncs, 1);
}

#[test]
fn installed_basic_data_boot_with_msr_and_temporary_partition_is_repaired() {
    let (mut old, old_layout) = fixture(Geometry {
        count: 3145728,
        cluster: 8,
        fat: 4096,
        ..Geometry::default()
    });
    let stale = build_bootstrap(&mut old, &old_layout).unwrap().replacement;
    let (mut disk, mut layout) = fixture(Geometry {
        count: 262144,
        fat: 2048,
        ..Geometry::default()
    });
    let backup_lba = old_layout.bytes / 512 - 1;
    layout.bytes = old_layout.bytes;
    disk.bytes = layout.bytes;
    let boot_end = (layout.boot.offset + layout.boot.length) / 512 - 1;
    let windows_start = boot_end + 1 + 32768;
    let temporary_start = backup_lba - 32768;
    layout.windows.offset = windows_start * 512;
    layout.windows.length = (temporary_start - windows_start) * 512;
    {
        let mut image = disk.image.borrow_mut();
        image.insert(0, stale);
        let table = image.get_mut(&2).unwrap();
        table[..16].copy_from_slice(&BASIC_TYPE);
        let windows_entry: [u8; 128] = table[128..256].try_into().unwrap();
        table[128..256].fill(0);
        table[128..144].copy_from_slice(&[
            0x16, 0xe3, 0xc9, 0xe3, 0x5c, 0x0b, 0xb8, 0x4d, 0x81, 0x7d, 0xf9, 0x2d, 0xf0, 0x02,
            0x15, 0xae,
        ]);
        put64(table, 160, boot_end + 1);
        put64(table, 168, windows_start - 1);
        table[256..272].copy_from_slice(&BASIC_TYPE);
        table[272..288].fill(0x55);
        put64(table, 288, temporary_start);
        put64(table, 296, backup_lba - 33);
        table[384..512].copy_from_slice(&windows_entry);
        put64(table, 416, windows_start);
        put64(table, 424, temporary_start - 1);
        let header = image.get_mut(&1).unwrap();
        put64(header, 32, backup_lba);
        put64(header, 48, backup_lba - 33);
        table_crc(&mut image);
    }
    let mut backups = vec![];
    assert!(
        refresh_bootstrap(&mut disk, &layout, |data| {
            backups.push(*data);
            Ok(())
        })
        .unwrap()
    );
    assert_eq!(backups, vec![stale]);
    assert_eq!(get32(&disk.image.borrow()[&0], 32), 262144 + 2048);
}

#[test]
fn extended_and_partial_gpt_tables_skip_entry_padding_but_checksum_every_byte() {
    for entry_size in [128_u32, 256, 384, 512, 640, 768] {
        let (mut disk, layout) = fixture(Geometry::default());
        let mut image = disk.image.borrow_mut();
        let boot: [u8; 128] = image[&2][..128].try_into().unwrap();
        let windows: [u8; 128] = image[&2][128..256].try_into().unwrap();
        let mut table = vec![0x67; entry_size as usize * 2];
        table[..128].copy_from_slice(&boot);
        table[entry_size as usize..entry_size as usize + 128].copy_from_slice(&windows);
        for (index, part) in table.chunks(512).enumerate() {
            let mut sector = [0xa9; 512];
            sector[..part.len()].copy_from_slice(part);
            image.insert(2 + index as u64, sector);
        }
        let header = image.get_mut(&1).unwrap();
        put32(header, 80, 2);
        put32(header, 84, entry_size);
        table_crc(&mut image);
        drop(image);
        let plan = build_bootstrap(&mut disk, &layout).unwrap();
        assert_eq!(get32(&plan.replacement, 32), 2048 + 131072);
    }
    for header_size in [128_u32, 512] {
        let (mut disk, layout) = fixture(Geometry::default());
        {
            let mut image = disk.image.borrow_mut();
            let header = image.get_mut(&1).unwrap();
            put32(header, 12, header_size);
            header[92..header_size as usize].fill(0x67);
            header_crc(&mut image);
        }
        build_bootstrap(&mut disk, &layout).unwrap();
    }
}

#[test]
fn offset_limit_fat_sentinels_active_fats_and_cluster_lower_bound_are_preserved() {
    for (geometry, flags, fat_count, pointers) in [
        (
            Geometry {
                start: 65503,
                ..Geometry::default()
            },
            0_u16,
            2_u8,
            [1_u16, 6],
        ),
        (
            Geometry {
                start: 63,
                reserved: 64,
                ..Geometry::default()
            },
            0x81,
            2,
            [0, 0xffff],
        ),
        (
            Geometry {
                root: 5,
                ..Geometry::default()
            },
            0x80,
            1,
            [0xffff, 0],
        ),
        (
            Geometry {
                count: 65525 + 32 + 2048,
                ..Geometry::default()
            },
            0x0f,
            2,
            [1, 6],
        ),
    ] {
        let (mut disk, layout) = fixture(geometry);
        {
            let mut image = disk.image.borrow_mut();
            let boot = image.get_mut(&geometry.start).unwrap();
            put16(boot, 40, flags);
            boot[16] = fat_count;
            put16(boot, 48, pointers[0]);
            put16(boot, 50, pointers[1]);
        }
        let plan = build_bootstrap(&mut disk, &layout).unwrap();
        for (offset, pointer) in [48, 50].into_iter().zip(pointers) {
            let expected = if pointer == 0 || pointer == 0xffff {
                pointer
            } else {
                geometry.start as u16 + pointer
            };
            assert_eq!(
                u16::from_le_bytes(plan.replacement[offset..offset + 2].try_into().unwrap()),
                expected
            );
        }
    }
}

#[test]
fn protective_mbr_caps_chs_and_uint32_sector_count_on_large_sparse_media() {
    let (mut disk, mut layout) = fixture(Geometry::default());
    let backup_lba = u64::from(u32::MAX) + 8192;
    layout.bytes = (backup_lba + 1) * 512;
    layout.windows.length = (backup_lba - 33 + 1) * 512 - layout.windows.offset;
    disk.bytes = layout.bytes;
    {
        let mut image = disk.image.borrow_mut();
        let header = image.get_mut(&1).unwrap();
        put64(header, 32, backup_lba);
        put64(header, 48, backup_lba - 33);
        put64(image.get_mut(&2).unwrap(), 168, backup_lba - 33);
        table_crc(&mut image);
    }
    let plan = build_bootstrap(&mut disk, &layout).unwrap();
    assert_eq!(get32(&plan.replacement, 458), u32::MAX);
    assert_eq!(plan.replacement[451..454], [0xfe, 0xff, 0xff]);
    assert_eq!(plan.replacement[447..450], [0, 2, 0]);
}
