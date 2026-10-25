use crate::files::{make_directory, private_file};
use crate::{MediaError, check_cancelled};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

#[derive(Clone, Copy, Debug)]
pub struct ArchiveLimits {
    pub entries: usize,
    pub total_bytes: u64,
    pub file_bytes: u64,
}

impl Default for ArchiveLimits {
    fn default() -> Self {
        Self {
            entries: 300_000,
            total_bytes: 64 * 1024 * 1024 * 1024,
            file_bytes: 32 * 1024 * 1024 * 1024,
        }
    }
}

pub(crate) fn safe_archive_path(name: &str) -> Result<PathBuf, MediaError> {
    let name = name.replace('\\', "/");
    if name.is_empty()
        || name.len() > 4096
        || name.starts_with('/')
        || name.chars().any(|c| c.is_control() || c == ':')
    {
        return Err(MediaError::InvalidMetadata("unsafe archive path"));
    }
    let mut path = PathBuf::new();
    for component in name.trim_end_matches('/').split('/') {
        if component.is_empty()
            || component == ".."
            || component.ends_with([' ', '.']) && component != "."
        {
            return Err(MediaError::InvalidMetadata("unsafe archive component"));
        }
        if component == "." {
            if path.as_os_str().is_empty() {
                continue;
            }
            return Err(MediaError::InvalidMetadata("unsafe archive dot component"));
        }
        let stem = component.split('.').next().unwrap().to_ascii_uppercase();
        if matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            || stem.len() == 4
                && (stem.starts_with("COM") || stem.starts_with("LPT"))
                && stem.as_bytes()[3].is_ascii_digit()
        {
            return Err(MediaError::InvalidMetadata("Windows-reserved archive path"));
        }
        path.push(component);
    }
    if path.as_os_str().is_empty() {
        return Err(MediaError::InvalidMetadata("empty archive member"));
    }
    Ok(path)
}

pub(crate) fn insert_path(
    paths: &mut BTreeMap<String, bool>,
    name: &str,
    directory: bool,
) -> Result<(), MediaError> {
    let folded = name.to_lowercase();
    if paths.insert(folded.clone(), directory).is_some() {
        return Err(MediaError::InvalidMetadata(
            "duplicate or case-ambiguous archive entry",
        ));
    }

    let mut parent = Path::new(&folded).parent();
    while let Some(path) = parent {
        if paths.get(&path.to_string_lossy().into_owned()) == Some(&false) {
            return Err(MediaError::InvalidMetadata(
                "archive file/directory collision",
            ));
        }
        parent = path.parent();
    }
    if !directory
        && paths
            .keys()
            .any(|key| key.starts_with(&format!("{folded}/")))
    {
        return Err(MediaError::InvalidMetadata(
            "archive directory/file collision",
        ));
    }
    Ok(())
}

fn insert_casing(paths: &mut BTreeMap<String, String>, path: &Path) -> Result<(), MediaError> {
    let mut relative = PathBuf::new();
    for part in path {
        relative.push(part);
        let name = relative
            .to_str()
            .ok_or(MediaError::InvalidMetadata("archive filename encoding"))?;
        match paths.insert(name.to_lowercase(), name.into()) {
            Some(previous) if previous != name => {
                return Err(MediaError::InvalidMetadata(
                    "case-ambiguous archive directory",
                ));
            }
            _ => {}
        }
    }
    Ok(())
}

/// Extract a ZIP without trusting the extractor with archive-controlled paths.
/// Links, devices, ambiguous Windows paths, encrypted entries and bombs fail
/// before any member is written.
pub fn extract_zip(
    archive: &Path,
    destination: &Path,
    limits: ArchiveLimits,
    cancelled: &AtomicBool,
) -> Result<(), MediaError> {
    check_cancelled(cancelled)?;
    let file = File::open(archive).map_err(|e| MediaError::io("open downloaded archive", e))?;
    let mut zip =
        zip::ZipArchive::new(file).map_err(|_| MediaError::InvalidMetadata("ZIP archive"))?;
    if zip.is_empty() || zip.len() > limits.entries {
        return Err(MediaError::InvalidMetadata("ZIP entry limit"));
    }
    let mut total = 0u64;
    let mut names = BTreeMap::new();
    let mut casing = BTreeMap::new();
    let mut entries = Vec::new();
    for index in 0..zip.len() {
        check_cancelled(cancelled)?;
        let entry = zip
            .by_index(index)
            .map_err(|_| MediaError::InvalidMetadata("ZIP member"))?;
        let path = safe_archive_path(entry.name())?;
        insert_casing(&mut casing, &path)?;
        let name = path
            .to_str()
            .ok_or(MediaError::InvalidMetadata("ZIP filename encoding"))?;
        insert_path(&mut names, name, entry.is_dir())?;
        if let Some(mode) = entry.unix_mode() {
            let kind = mode & 0o170000;
            if kind != 0 && kind != 0o100000 && !(kind == 0o040000 && entry.is_dir()) {
                return Err(MediaError::InvalidMetadata("ZIP link or special entry"));
            }
        }
        total = total
            .checked_add(entry.size())
            .ok_or(MediaError::InvalidMetadata("ZIP size overflow"))?;
        if total > limits.total_bytes || entry.size() > limits.file_bytes {
            return Err(MediaError::InvalidMetadata("ZIP expansion limit"));
        }
        entries.push((index, path, entry.is_dir(), entry.size()));
    }
    make_directory(destination)?;
    for (index, path, directory, expected_size) in entries {
        check_cancelled(cancelled)?;
        let path = destination.join(path);
        if directory {
            make_directory(&path)?;
            continue;
        }
        make_directory(
            path.parent()
                .ok_or(MediaError::InvalidMetadata("ZIP parent"))?,
        )?;
        let mut output = private_file(&path)?;
        let mut input = zip
            .by_index(index)
            .map_err(|_| MediaError::InvalidMetadata("ZIP member"))?;
        let mut buffer = vec![0u8; 1024 * 1024];
        let mut size = 0u64;
        loop {
            check_cancelled(cancelled)?;
            let count = input.read(&mut buffer).map_err(|_| MediaError::Integrity {
                component: "ZIP member",
            })?;
            if count == 0 {
                break;
            }
            size = size
                .checked_add(count as u64)
                .ok_or(MediaError::InvalidMetadata("ZIP size overflow"))?;
            if size > expected_size {
                return Err(MediaError::Integrity {
                    component: "ZIP expansion size",
                });
            }
            output
                .write_all(&buffer[..count])
                .map_err(|e| MediaError::io("extract private archive file", e))?;
        }
        if size != expected_size {
            return Err(MediaError::Integrity {
                component: "ZIP member size",
            });
        }
        output
            .sync_all()
            .map_err(|e| MediaError::io("flush extracted archive", e))?;
    }
    Ok(())
}
