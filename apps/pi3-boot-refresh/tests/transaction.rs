mod support;

use std::io;

use support::*;
use wor_pi3_boot_refresh::{Error, refresh_bootstrap};

#[test]
fn failed_backup_prevents_every_device_write() {
    let (mut disk, layout) = fixture(Geometry::default());
    let error = refresh_bootstrap(&mut disk, &layout, |_| {
        Err(Error::io("fixture backup", io::Error::other("no space")))
    })
    .unwrap_err();
    assert!(matches!(error, Error::Backup(_)));
    assert!(error.to_string().contains("no boot-sector write attempted"));
    assert_eq!(disk.writes, 0);
    assert_eq!(disk.syncs, 0);
}

#[test]
fn changes_to_original_or_valid_gpt_fat_metadata_during_backup_prevent_write() {
    for name in [
        "sector-zero",
        "header-padding",
        "header-guid",
        "table-guid",
        "fat-serial",
    ] {
        let (mut disk, layout) = fixture(Geometry::default());
        let image = disk.image.clone();
        let error = refresh_bootstrap(&mut disk, &layout, |_| {
            let mut image = image.borrow_mut();
            match name {
                "sector-zero" => image.get_mut(&0).unwrap()[440] ^= 1,
                "header-padding" => image.get_mut(&1).unwrap()[300] ^= 1,
                "header-guid" => {
                    image.get_mut(&1).unwrap()[56] ^= 1;
                    header_crc(&mut image);
                }
                "table-guid" => {
                    image.get_mut(&2).unwrap()[16] ^= 1;
                    table_crc(&mut image);
                }
                "fat-serial" => image.get_mut(&2048).unwrap()[67] ^= 1,
                _ => unreachable!(),
            }
            Ok(())
        })
        .unwrap_err();
        assert!(matches!(error, Error::MetadataChanged), "{name}: {error}");
        assert_eq!(disk.writes, 0, "{name}");
        assert_eq!(disk.syncs, 0);
    }
}

#[test]
fn newly_invalid_metadata_during_backup_is_revalidated_before_write() {
    let (mut disk, layout) = fixture(Geometry::default());
    let image = disk.image.clone();
    let error = refresh_bootstrap(&mut disk, &layout, |_| {
        image.borrow_mut().get_mut(&1).unwrap()[56] ^= 1;
        Ok(())
    })
    .unwrap_err();
    assert!(matches!(error, Error::GptHeaderChecksum));
    assert_eq!(disk.writes, 0);
}

#[test]
fn short_failed_dropped_and_unflushed_writes_never_report_success_or_retry() {
    for name in ["short", "error", "dropped", "flush"] {
        let (mut disk, layout) = fixture(Geometry::default());
        match name {
            "short" => disk.write_fault = WriteFault::Short,
            "error" => disk.write_fault = WriteFault::Error,
            "dropped" => disk.write_fault = WriteFault::Dropped,
            "flush" => disk.sync_error = true,
            _ => unreachable!(),
        }
        let error = refresh_bootstrap(&mut disk, &layout, |_| Ok(())).unwrap_err();
        assert!(
            error.to_string().contains("do not reboot"),
            "{name}: {error}"
        );
        assert_eq!(disk.writes, 1, "{name}: write was retried");
        assert_eq!(
            disk.syncs,
            usize::from(name == "flush" || name == "dropped")
        );
        match name {
            "short" => assert!(matches!(error, Error::ShortWrite(511))),
            "error" => assert!(matches!(error, Error::Write(_))),
            "dropped" => assert!(matches!(error, Error::ReadBack)),
            "flush" => assert!(matches!(error, Error::Flush(_))),
            _ => unreachable!(),
        }
    }
}

#[test]
fn postwrite_validation_checks_original_replacement_and_full_metadata_fingerprint() {
    for name in [
        "invalid-gpt",
        "valid-header-padding",
        "valid-table-guid",
        "valid-fat-serial",
    ] {
        let (mut disk, layout) = fixture(Geometry::default());
        disk.after_write = Some(match name {
            "invalid-gpt" => |image| {
                image.get_mut(&1).unwrap()[56] ^= 1;
            },
            "valid-header-padding" => |image| {
                image.get_mut(&1).unwrap()[300] ^= 1;
            },
            "valid-table-guid" => |image| {
                image.get_mut(&2).unwrap()[16] ^= 1;
                table_crc(image);
            },
            "valid-fat-serial" => |image| {
                image.get_mut(&2048).unwrap()[67] ^= 1;
            },
            _ => unreachable!(),
        });
        let error = refresh_bootstrap(&mut disk, &layout, |_| Ok(())).unwrap_err();
        if name == "invalid-gpt" {
            assert!(matches!(error, Error::PostWrite(_)), "{name}: {error}");
        } else {
            assert!(matches!(error, Error::ReadBack), "{name}: {error}");
        }
        assert!(error.to_string().contains("do not reboot"));
        assert_eq!(disk.writes, 1);
    }
}
