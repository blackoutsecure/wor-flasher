use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

use crate::{Error, Result, SECTOR_SIZE, Sector, workflow::finish};

pub fn require_file(path: &Path) -> Result<()> {
    let info = fs::metadata(path).map_err(|error| Error::io(path.display().to_string(), error))?;
    if !info.is_file() || info.len() == 0 {
        return Err(Error::RequiredFile(path.display().to_string()));
    }
    Ok(())
}

/// Uses create-new semantics, a single checked write and a durable flush.
/// The Windows caller supplies a checked CloseHandle, rather than ignoring a
/// close failure in File::drop after the finalization result was determined.
pub fn save_backup(
    directory: &Path,
    sector: &Sector,
    close_file: impl FnOnce(File) -> Result<()>,
    mut event: impl FnMut(&str, &str),
) -> Result<()> {
    let mut directories = fs::DirBuilder::new();
    directories.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        directories.mode(0o700);
    }
    directories
        .create(directory)
        .map_err(|error| Error::io(directory.display().to_string(), error))?;
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let timestamp = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let mut opened = None;
    for _ in 0..10000 {
        let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = directory.join(format!(
            "pi3-sector0-before-{timestamp}-{}-{sequence}.bin",
            std::process::id()
        ));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            use windows_sys::Win32::Storage::FileSystem::{FILE_SHARE_READ, FILE_SHARE_WRITE};
            options.share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE);
        }
        match options.open(&path) {
            Ok(file) => {
                opened = Some((path, file));
                break;
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(Error::io(path.display().to_string(), error)),
        }
    }
    let (path, mut file) = opened.ok_or_else(|| {
        Error::io(
            "creating unique Pi 3 boot-sector backup",
            io::Error::new(
                io::ErrorKind::AlreadyExists,
                "too many backup-name collisions",
            ),
        )
    })?;
    let result = (|| {
        let bytes = file
            .write(sector)
            .map_err(|error| Error::io(path.display().to_string(), error))?;
        if bytes != SECTOR_SIZE {
            return Err(Error::io(
                format!("short backup write to {}: {bytes} bytes", path.display()),
                io::Error::from(io::ErrorKind::WriteZero),
            ));
        }
        file.sync_all()
            .map_err(|error| Error::io(path.display().to_string(), error))?;
        event(
            "LogInfo",
            &format!("Original Pi 3 boot sector backed up to {}", path.display()),
        );
        Ok(())
    })();
    finish(result, close_file(file))
}
