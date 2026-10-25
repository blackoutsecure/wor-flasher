mod support;

use std::cell::Cell;
use std::fs;
use std::io;

use support::FixtureDirectory;
use wor_pi3_boot_refresh::{Error, files};

#[test]
fn installed_kernel_and_firmware_checks_require_nonempty_regular_files() {
    let directory = FixtureDirectory::new();
    let file = directory.0.join("RPI_EFI.fd");
    assert!(files::require_file(&file).is_err());
    fs::write(&file, []).unwrap();
    assert!(matches!(
        files::require_file(&file),
        Err(Error::RequiredFile(_))
    ));
    assert!(matches!(
        files::require_file(&directory.0),
        Err(Error::RequiredFile(_))
    ));
    fs::write(&file, b"synthetic firmware, not a redistributable payload").unwrap();
    files::require_file(&file).unwrap();
    #[cfg(unix)]
    {
        let link = directory.0.join("firmware-link");
        std::os::unix::fs::symlink(&file, &link).unwrap();
        files::require_file(&link).unwrap();
    }
}

#[test]
fn backups_are_unique_complete_durable_files_with_private_creation_modes() {
    let directory = FixtureDirectory::new();
    let backups = directory.0.join("Windows/Logs/WoR-Flasher");
    let original = [0x67; 512];
    let close_calls = Cell::new(0);
    let mut events = vec![];
    for _ in 0..2 {
        files::save_backup(
            &backups,
            &original,
            |file| {
                close_calls.set(close_calls.get() + 1);
                drop(file);
                Ok(())
            },
            |level, message| {
                events.push((level.to_owned(), message.to_owned()));
            },
        )
        .unwrap();
    }
    assert_eq!(close_calls.get(), 2);
    let paths = fs::read_dir(&backups)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect::<Vec<_>>();
    assert_eq!(paths.len(), 2);
    assert_ne!(paths[0], paths[1]);
    for path in paths {
        assert!(
            path.file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("pi3-sector0-before-")
        );
        assert_eq!(fs::read(&path).unwrap(), original);
        assert!(events.iter().any(
            |(level, message)| level == "LogInfo" && message.ends_with(path.to_str().unwrap())
        ));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
            assert_eq!(
                fs::metadata(&backups).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
    }
}

#[test]
fn backup_directory_and_close_failures_are_not_suppressed() {
    let directory = FixtureDirectory::new();
    let not_directory = directory.0.join("file");
    fs::write(&not_directory, b"not a directory").unwrap();
    assert!(
        files::save_backup(
            &not_directory.join("child"),
            &[0; 512],
            |_| panic!("no file should be opened"),
            |_, _| panic!("no backup was saved"),
        )
        .is_err()
    );
    let close_called = Cell::new(false);
    let events = Cell::new(0);
    let error = files::save_backup(
        &directory.0.join("backup"),
        &[0; 512],
        |file| {
            close_called.set(true);
            drop(file);
            Err(Error::io(
                "close backup",
                io::Error::other("injected close failure"),
            ))
        },
        |_, _| {
            events.set(events.get() + 1);
        },
    )
    .unwrap_err();
    assert!(close_called.get());
    assert_eq!(events.get(), 1);
    assert!(error.to_string().contains("injected close failure"));
}
