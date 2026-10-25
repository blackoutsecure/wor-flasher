use std::collections::BTreeMap;

use wor_pi3_boot_refresh::{
    InstallContext, VolumeExtent, VolumeIdentity, parse_context, parse_volume_extent,
    select_boot_drive, validate_disk_number,
};

fn valid() -> BTreeMap<&'static str, String> {
    [
        ("WOR_DEVICE_TYPE", "RPi3-ARM64"),
        ("WOR_INSTALLOPTIONS_PARTITIONSCHEME", "GPT"),
        ("WOR_IMAGE_ARCH", "ARM64"),
        ("WOR_DISK_INDEX", "7"),
        ("WOR_DISK_BOOTPARTITION", "B:"),
        ("WOR_DISK_WINDOWSPARTITION", "W:"),
    ]
    .map(|(key, value)| (key, value.to_owned()))
    .into()
}

#[test]
fn valid_context_and_duplicate_wor11_export_are_retained_until_discovery() {
    let mut values = valid();
    let context = parse_context(|name| values.get(name).cloned().unwrap_or_default())
        .unwrap()
        .unwrap();
    assert_eq!(context.disk_index, 7);
    assert_eq!(context.boot_drive, "B:");
    values.insert("WOR_DISK_INDEX", "0".into());
    values.insert("WOR_DISK_BOOTPARTITION", "d:".into());
    values.insert("WOR_DISK_WINDOWSPARTITION", "d:".into());
    assert_eq!(
        parse_context(|name| values.get(name).cloned().unwrap_or_default()).unwrap(),
        Some(InstallContext {
            disk_index: 0,
            boot_drive: "D:".into(),
            windows_drive: "D:".into(),
        })
    );
    values.insert("WOR_DISK_INDEX", "0004294967295".into());
    assert_eq!(
        parse_context(|name| values.get(name).cloned().unwrap_or_default())
            .unwrap()
            .unwrap()
            .disk_index,
        u32::MAX
    );
}

#[test]
fn context_never_guesses_a_board_disk_architecture_scheme_or_drive() {
    for (key, invalid) in [
        (
            "WOR_DEVICE_TYPE",
            vec!["", "RPi3-ARM32", "unknown", " RPi3-ARM64"],
        ),
        (
            "WOR_INSTALLOPTIONS_PARTITIONSCHEME",
            vec!["", "unknown", "GPT "],
        ),
        ("WOR_IMAGE_ARCH", vec!["", "x64", "ARM", "ARM64 "]),
        (
            "WOR_DISK_INDEX",
            vec!["", "-1", "+1", "1&exit", " 1", "1 ", "１", "4294967296"],
        ),
        (
            "WOR_DISK_BOOTPARTITION",
            vec!["", r"B:\", "B:folder", r"\\.\PhysicalDrive0", "1:", "é:"],
        ),
        ("WOR_DISK_WINDOWSPARTITION", vec!["", r"W:\", "WW:", ":"]),
    ] {
        for value in invalid {
            let mut values = valid();
            values.insert(key, value.into());
            assert!(
                parse_context(|name| values.get(name).cloned().unwrap_or_default()).is_err(),
                "{key}={value:?} was accepted"
            );
        }
    }
}

#[test]
fn unrelated_boards_skip_before_other_context_and_mbr_skips_before_architecture() {
    for board in ["RPi4-ARM64", "rpi5-arm64"] {
        assert_eq!(
            parse_context(|name| if name == "WOR_DEVICE_TYPE" {
                board.into()
            } else {
                String::new()
            })
            .unwrap(),
            None
        );
    }
    assert_eq!(
        parse_context(|name| match name {
            "WOR_DEVICE_TYPE" => "rpi3-arm64".into(),
            "WOR_INSTALLOPTIONS_PARTITIONSCHEME" => "mbr".into(),
            _ => String::new(),
        })
        .unwrap(),
        None
    );
    assert!(parse_context(|_| String::new()).is_err());
}

fn volume(drive: &str, device_type: u32, disk_index: u32, partition_index: u32) -> VolumeIdentity {
    VolumeIdentity {
        drive: drive.into(),
        device_type,
        disk_index,
        partition_index,
    }
}

#[test]
fn discovery_binds_to_selected_physical_disk_partition_one_not_adjacent_letter() {
    let context = InstallContext {
        disk_index: 0,
        boot_drive: "D:".into(),
        windows_drive: "D:".into(),
    };
    let volumes = vec![
        volume("A:", 7, 9, 1),
        volume("B:", 7, 0, 1),
        volume("C:", 7, 0, 3),
        volume("D:", 7, 0, 4),
        volume("X:", 0x24, 0, 1),
    ];
    assert_eq!(select_boot_drive(&context, &volumes).unwrap(), "B:");
    for name in [
        "wrong-disk",
        "missing-boot",
        "ambiguous",
        "windows-is-boot",
        "invalid-drive",
        "lowercase-drive",
        "wrong-export",
    ] {
        let mut current = context.clone();
        let mut candidates = volumes.clone();
        match name {
            "wrong-disk" => candidates[1].disk_index = 2,
            "missing-boot" => candidates[1].partition_index = 2,
            "ambiguous" => candidates.push(volume("E:", 7, 0, 1)),
            "windows-is-boot" => candidates[3].partition_index = 1,
            "invalid-drive" => candidates[1].drive = r"B:\other".into(),
            "lowercase-drive" => candidates[1].drive = "b:".into(),
            "wrong-export" => current.boot_drive = "C:".into(),
            _ => unreachable!(),
        }
        assert!(select_boot_drive(&current, &candidates).is_err(), "{name}");
    }
    let recovery = InstallContext {
        disk_index: 7,
        boot_drive: "Z:".into(),
        windows_drive: "Z:".into(),
    };
    assert_eq!(
        select_boot_drive(
            &recovery,
            &[
                volume("B:", 7, 0, 1),
                volume("R:", 7, 7, 1),
                volume("Z:", 7, 7, 3)
            ]
        )
        .unwrap(),
        "R:"
    );
    let explicit = InstallContext {
        disk_index: 0,
        boot_drive: "B:".into(),
        windows_drive: "D:".into(),
    };
    assert_eq!(select_boot_drive(&explicit, &volumes[1..2]).unwrap(), "B:");
}

#[test]
fn windows_arm64_ioctl_layouts_require_exact_lengths_and_one_aligned_extent() {
    let mut data = [0; 32];
    data[..4].copy_from_slice(&1_u32.to_le_bytes());
    data[8..12].copy_from_slice(&7_u32.to_le_bytes());
    data[16..24].copy_from_slice(&1048576_u64.to_le_bytes());
    data[24..].copy_from_slice(&134217728_u64.to_le_bytes());
    assert_eq!(
        parse_volume_extent(&data).unwrap(),
        VolumeExtent {
            disk_index: 7,
            offset: 1048576,
            length: 134217728
        }
    );
    let mut oversized = data.to_vec();
    oversized.push(0);
    assert!(parse_volume_extent(&oversized).is_err());
    for count in [0_u32, 2, u32::MAX] {
        let mut invalid = data;
        invalid[..4].copy_from_slice(&count.to_le_bytes());
        assert!(parse_volume_extent(&invalid).is_err());
    }
    for length in 0..32 {
        assert!(parse_volume_extent(&data[..length]).is_err());
    }
    assert!(parse_volume_extent(&[0; 33]).is_err());
    for offset in [16, 24] {
        for value in [0_u64, 1, 513, i64::MAX as u64 + 1, u64::MAX] {
            let mut invalid = data;
            invalid[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
            assert!(
                parse_volume_extent(&invalid).is_err(),
                "offset={offset} value={value}"
            );
        }
    }
}

#[test]
fn raw_device_number_requires_disk_type_exact_index_and_whole_disk_sentinel() {
    let mut data = [0; 12];
    data[..4].copy_from_slice(&7_u32.to_le_bytes());
    data[4..8].copy_from_slice(&7_u32.to_le_bytes());
    for partition in [0_u32, u32::MAX] {
        data[8..].copy_from_slice(&partition.to_le_bytes());
        validate_disk_number(&data, 7).unwrap();
        let mut oversized = data.to_vec();
        oversized.push(0);
        assert!(validate_disk_number(&oversized, 7).is_err());
    }
    data[8..].copy_from_slice(&1_u32.to_le_bytes());
    assert!(
        validate_disk_number(&data, 7)
            .unwrap_err()
            .to_string()
            .contains("whole disk")
    );
    data[8..].fill(0);
    assert!(validate_disk_number(&data, 8).is_err());
    data[0] = 0x24;
    assert!(validate_disk_number(&data, 7).is_err());
    for size in 0..12 {
        assert!(validate_disk_number(&data[..size], 7).is_err());
    }
    assert!(validate_disk_number(&[0; 13], 0).is_err());
}
