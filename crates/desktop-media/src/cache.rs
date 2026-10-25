use crate::files::{is_cache_control, read_required, sha256_file, tree_files, write_private};
use crate::{CacheMode, MediaError, check_cancelled};
use fs2::FileExt;
use std::fs::{self, File, OpenOptions};
use std::path::Path;
use std::sync::atomic::AtomicBool;

pub fn cache_manifest(directory: &Path, cancelled: &AtomicBool) -> Result<String, MediaError> {
    let mut manifest = String::new();
    for (name, path) in tree_files(directory)? {
        check_cancelled(cancelled)?;
        if is_cache_control(path.file_name().and_then(|n| n.to_str()).unwrap_or("")) {
            continue;
        }
        manifest.push_str(&format!("{}  ./{}\n", sha256_file(&path, cancelled)?, name));
    }
    if manifest.is_empty() {
        return Err(MediaError::InvalidMetadata("empty cache manifest"));
    }
    Ok(manifest)
}

pub fn mark_cache(
    directory: &Path,
    version: &str,
    cancelled: &AtomicBool,
) -> Result<(), MediaError> {
    if version.is_empty() || version.contains(['\r', '\n', '\0']) {
        return Err(MediaError::InvalidMetadata("cache version"));
    }
    let manifest = cache_manifest(directory, cancelled)?;
    write_private(&directory.join(".wor-flasher-sha256"), manifest.as_bytes())?;
    write_private(
        &directory.join(".wor-flasher-version"),
        format!("{version}\n").as_bytes(),
    )
}

pub fn cache_is_current(
    directory: &Path,
    version: &str,
    mode: CacheMode,
    cancelled: &AtomicBool,
) -> Result<bool, MediaError> {
    check_cancelled(cancelled)?;
    let Ok(metadata) = fs::symlink_metadata(directory) else {
        return Ok(false);
    };
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(MediaError::InvalidMetadata("unsafe cache directory"));
    }
    match mode {
        CacheMode::Refresh => Ok(false),
        CacheMode::Trust => Ok(true),
        CacheMode::Verify => {
            let token = read_required(
                &directory.join(".wor-flasher-version"),
                "cache version",
                16 * 1024,
            );
            let expected = read_required(
                &directory.join(".wor-flasher-sha256"),
                "cache manifest",
                64 * 1024 * 1024,
            );
            let (Ok(token), Ok(expected)) = (token, expected) else {
                return Ok(false);
            };
            if String::from_utf8_lossy(&token).trim_end_matches(['\r', '\n']) != version {
                return Ok(false);
            }
            Ok(cache_manifest(directory, cancelled)?.as_bytes() == expected)
        }
    }
}

pub(crate) struct CacheLock(File);

impl CacheLock {
    pub fn acquire(directory: &Path) -> Result<Self, MediaError> {
        let path = directory.join(".wor-native-cache.lock");
        if let Ok(metadata) = fs::symlink_metadata(&path)
            && (!metadata.is_file() || metadata.file_type().is_symlink())
        {
            return Err(MediaError::InvalidMetadata("unsafe cache lock"));
        }
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options
            .open(path)
            .map_err(|e| MediaError::io("open cache lock", e))?;
        FileExt::try_lock_exclusive(&file)
            .map_err(|e| MediaError::io("lock active preparation cache", e))?;
        Ok(Self(file))
    }
}

impl Drop for CacheLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.0);
    }
}
