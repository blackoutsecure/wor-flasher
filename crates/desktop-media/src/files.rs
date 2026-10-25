use crate::{MediaError, check_cancelled};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

pub(crate) fn required_file(path: &Path, component: &'static str) -> Result<u64, MediaError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| MediaError::MissingRequired {
        component,
        path: path.to_path_buf(),
    })?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() == 0 {
        return Err(MediaError::MissingRequired {
            component,
            path: path.to_path_buf(),
        });
    }
    Ok(metadata.len())
}

pub(crate) fn require_directory(path: &Path, component: &'static str) -> Result<(), MediaError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| MediaError::MissingRequired {
        component,
        path: path.to_path_buf(),
    })?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(MediaError::MissingRequired {
            component,
            path: path.to_path_buf(),
        });
    }
    Ok(())
}

pub(crate) fn read_required(
    path: &Path,
    component: &'static str,
    maximum: u64,
) -> Result<Vec<u8>, MediaError> {
    if required_file(path, component)? > maximum {
        return Err(MediaError::InvalidMetadata(component));
    }
    fs::read(path).map_err(|e| MediaError::io("read required preparation data", e))
}

pub(crate) fn make_directory(path: &Path) -> Result<(), MediaError> {
    fs::create_dir_all(path).map_err(|e| MediaError::io("create preparation directory", e))?;
    require_directory(path, "preparation directory")
}

pub(crate) fn private_tempdir(
    parent: &Path,
    prefix: &str,
) -> Result<tempfile::TempDir, MediaError> {
    let directory = tempfile::Builder::new()
        .prefix(prefix)
        .tempdir_in(parent)
        .map_err(|e| MediaError::io("create private preparation workspace", e))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(error) = fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700))
        {
            return finish_temp(
                directory,
                Err(MediaError::io(
                    "protect private preparation workspace",
                    error,
                )),
            );
        }
    }
    Ok(directory)
}

pub(crate) fn private_file(path: &Path) -> Result<File, MediaError> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
        .open(path)
        .map_err(|e| MediaError::io("create private preparation file", e))
}

pub(crate) fn write_private(path: &Path, contents: &[u8]) -> Result<(), MediaError> {
    let parent = path
        .parent()
        .ok_or(MediaError::InvalidMetadata("private path"))?;
    make_directory(parent)?;
    if path.exists() {
        required_file(path, "private preparation file")?;
        fs::remove_file(path).map_err(|e| MediaError::io("replace private preparation file", e))?;
    }
    let mut file = private_file(path)?;
    file.write_all(contents)
        .map_err(|e| MediaError::io("write private preparation file", e))?;
    file.sync_all()
        .map_err(|e| MediaError::io("flush private preparation file", e))
}

pub(crate) fn copy_file(
    source: &Path,
    destination: &Path,
    cancelled: &AtomicBool,
) -> Result<u64, MediaError> {
    check_cancelled(cancelled)?;
    let metadata =
        fs::symlink_metadata(source).map_err(|e| MediaError::io("inspect copy source", e))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(MediaError::MissingRequired {
            component: "regular source file",
            path: source.to_path_buf(),
        });
    }
    make_directory(
        destination
            .parent()
            .ok_or(MediaError::InvalidMetadata("copy destination"))?,
    )?;
    if fs::symlink_metadata(destination).is_ok() {
        let metadata = fs::symlink_metadata(destination)
            .map_err(|e| MediaError::io("inspect destination", e))?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(MediaError::InvalidMetadata("unsafe copy destination"));
        }
        fs::remove_file(destination).map_err(|e| MediaError::io("replace private file", e))?;
    }
    let mut input = File::open(source).map_err(|e| MediaError::io("open copy source", e))?;
    let mut output = private_file(destination)?;
    let mut buffer = vec![0u8; 1024 * 1024];
    let mut total = 0u64;
    loop {
        check_cancelled(cancelled)?;
        let count = input
            .read(&mut buffer)
            .map_err(|e| MediaError::io("read copy source", e))?;
        if count == 0 {
            break;
        }
        output
            .write_all(&buffer[..count])
            .map_err(|e| MediaError::io("copy private media file", e))?;
        total = total
            .checked_add(count as u64)
            .ok_or(MediaError::InvalidMetadata("file size"))?;
    }
    output
        .sync_all()
        .map_err(|e| MediaError::io("flush private media file", e))?;
    if total != metadata.len()
        || input
            .metadata()
            .map_err(|e| MediaError::io("recheck copy source", e))?
            .len()
            != total
    {
        return Err(MediaError::Integrity {
            component: "source copy",
        });
    }
    Ok(total)
}

pub(crate) fn is_cache_control(name: &str) -> bool {
    matches!(
        name,
        ".wor-flasher-version" | ".wor-flasher-sha256" | ".wor-flasher-sha256.tmp"
    )
}

pub(crate) fn tree_files(root: &Path) -> Result<Vec<(String, PathBuf)>, MediaError> {
    require_directory(root, "payload directory")?;
    let mut files = Vec::new();
    let mut names = std::collections::BTreeSet::new();
    walk(root, root, &mut files, &mut names, 0)?;
    files.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(files)
}

fn walk(
    root: &Path,
    directory: &Path,
    files: &mut Vec<(String, PathBuf)>,
    names: &mut std::collections::BTreeSet<String>,
    depth: usize,
) -> Result<(), MediaError> {
    if depth > 64 || files.len() > 300_000 {
        return Err(MediaError::InvalidMetadata("payload tree limits"));
    }
    for entry in fs::read_dir(directory).map_err(|e| MediaError::io("list payload directory", e))? {
        let entry = entry.map_err(|e| MediaError::io("read payload entry", e))?;
        let metadata = entry
            .file_type()
            .map_err(|e| MediaError::io("inspect payload entry", e))?;
        let path = entry.path();
        let relative = path
            .strip_prefix(root)
            .map_err(|_| MediaError::InvalidMetadata("payload path"))?;
        let name = relative
            .to_str()
            .ok_or(MediaError::InvalidMetadata("non-UTF-8 payload path"))?;
        crate::archive::safe_archive_path(name)?;
        if !names.insert(name.to_lowercase()) || names.len() > 300_000 {
            return Err(MediaError::InvalidMetadata(
                "case-ambiguous payload path or entry limit",
            ));
        }
        if metadata.is_dir() {
            walk(root, &path, files, names, depth + 1)?;
        } else if metadata.is_file() {
            if name.chars().any(|c| c.is_control() || c == '\\') {
                return Err(MediaError::InvalidMetadata("payload filename"));
            }
            files.push((name.into(), path));
        } else {
            return Err(MediaError::InvalidMetadata(
                "symlink or special payload entry",
            ));
        }
    }
    Ok(())
}

pub(crate) fn copy_tree(
    source: &Path,
    destination: &Path,
    cancelled: &AtomicBool,
) -> Result<(), MediaError> {
    let files = tree_files(source)?;
    make_directory(destination)?;
    let mut folded = std::collections::BTreeSet::new();
    for (name, path) in files {
        check_cancelled(cancelled)?;
        if is_cache_control(path.file_name().and_then(|n| n.to_str()).unwrap_or("")) {
            continue;
        }
        if !folded.insert(name.to_lowercase()) {
            return Err(MediaError::InvalidMetadata("case-ambiguous payload paths"));
        }
        copy_file(&path, &destination.join(name), cancelled)?;
    }
    Ok(())
}

pub(crate) fn copy_tree_merge(
    source: &Path,
    destination: &Path,
    cancelled: &AtomicBool,
) -> Result<(), MediaError> {
    make_directory(destination)?;
    for (name, path) in tree_files(source)? {
        if is_cache_control(path.file_name().and_then(|n| n.to_str()).unwrap_or("")) {
            continue;
        }
        let mut target = destination.to_path_buf();
        let components: Vec<_> = name.split('/').collect();
        for (index, part) in components.iter().enumerate() {
            let mut matches = fs::read_dir(&target)
                .map_err(|e| MediaError::io("merge private startup tree", e))?
                .filter_map(Result::ok)
                .filter(|entry| {
                    entry
                        .file_name()
                        .to_str()
                        .is_some_and(|name| name.eq_ignore_ascii_case(part))
                })
                .map(|entry| entry.path());
            target = if let Some(found) = matches.next() {
                if matches.next().is_some() {
                    return Err(MediaError::InvalidMetadata("case-ambiguous startup tree"));
                }
                found
            } else {
                target.join(part)
            };
            if index + 1 < components.len() {
                make_directory(&target)?;
            }
        }
        copy_file(&path, &target, cancelled)?;
    }
    Ok(())
}

pub(crate) fn sha256_file(path: &Path, cancelled: &AtomicBool) -> Result<String, MediaError> {
    let mut file = File::open(path).map_err(|e| MediaError::io("open payload for hashing", e))?;
    let mut hash = Sha256::new();
    let mut buffer = vec![0u8; 1024 * 1024];
    loop {
        check_cancelled(cancelled)?;
        let count = file
            .read(&mut buffer)
            .map_err(|e| MediaError::io("hash payload", e))?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

pub(crate) fn tree_bytes(path: &Path) -> Result<u64, MediaError> {
    tree_files(path)?
        .into_iter()
        .try_fold(0u64, |total, (_, file)| {
            let size = file
                .metadata()
                .map_err(|e| MediaError::io("measure payload size", e))?
                .len();
            total
                .checked_add(size)
                .ok_or(MediaError::InvalidMetadata("payload size overflow"))
        })
}

pub(crate) fn require_space(required: u64, available: u64) -> Result<(), MediaError> {
    if available < required {
        Err(MediaError::Space {
            required,
            available,
        })
    } else {
        Ok(())
    }
}

pub(crate) fn finish_temp<T>(
    staging: tempfile::TempDir,
    result: Result<T, MediaError>,
) -> Result<T, MediaError> {
    if matches!(result, Err(MediaError::Cleanup { .. })) {
        // A failed unmount can leave source media attached below this tree.
        // Never recursively delete a parent of that mount.
        let _retained = staging.keep();
        return result;
    }
    match staging.close() {
        Ok(()) => result,
        Err(source) => Err(MediaError::Cleanup {
            primary: result.err().map(Box::new),
            source,
        }),
    }
}
