use crate::{MediaError, Result};
use std::ffi::{CString, OsStr};
use std::fs::{File, Metadata};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Stamp {
    dev: u64,
    ino: u64,
    mode: u32,
    uid: u32,
    gid: u32,
    size: u64,
    mtime: (i64, i64),
    ctime: (i64, i64),
    links: u64,
}

impl Stamp {
    pub(crate) fn of(file: &File) -> Result<Self> {
        Ok(Self::metadata(&file.metadata()?))
    }

    fn metadata(m: &Metadata) -> Self {
        Self {
            dev: m.dev(),
            ino: m.ino(),
            mode: m.mode(),
            uid: m.uid(),
            gid: m.gid(),
            size: m.size(),
            mtime: (m.mtime(), m.mtime_nsec()),
            ctime: (m.ctime(), m.ctime_nsec()),
            links: m.nlink(),
        }
    }

    pub(crate) fn same_inode(&self, other: &Self) -> bool {
        self.dev == other.dev && self.ino == other.ino
    }
}

fn cstring(name: &OsStr) -> Result<CString> {
    CString::new(name.as_bytes()).map_err(|_| MediaError::invalid("path contains a NUL byte"))
}

fn openat(directory: &File, name: &OsStr, flags: i32, mode: u32) -> Result<File> {
    let name = cstring(name)?;
    let fd = unsafe { libc::openat(directory.as_raw_fd(), name.as_ptr(), flags, mode) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(unsafe { File::from_raw_fd(fd) })
}

fn entry_type(directory: &File, name: &OsStr) -> Result<(u32, u64, u64)> {
    let name = cstring(name)?;
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    let status = unsafe {
        libc::fstatat(
            directory.as_raw_fd(),
            name.as_ptr(),
            stat.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if status != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let stat = unsafe { stat.assume_init() };
    #[cfg(target_os = "macos")]
    let (mode, dev) = (u32::from(stat.st_mode), stat.st_dev as u64);
    #[cfg(target_os = "linux")]
    let (mode, dev) = (stat.st_mode, stat.st_dev);
    Ok((mode, dev, stat.st_ino))
}

pub(crate) fn absolute(path: &Path) -> Result<PathBuf> {
    let path = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()?.join(path)
    };
    if path.components().count() > 64 || path.components().any(|c| c == Component::ParentDir) {
        return Err(MediaError::invalid(
            "path traversal or excessive path depth is unsupported",
        ));
    }
    Ok(path
        .components()
        .filter(|c| *c != Component::CurDir)
        .collect())
}

pub(crate) fn cache_path(path: &Path) -> Result<PathBuf> {
    let path = absolute(path)?;
    #[cfg(target_os = "macos")]
    for alias in ["var", "tmp"] {
        let root = PathBuf::from("/").join(alias);
        if let Ok(relative) = path.strip_prefix(&root) {
            let before = std::fs::symlink_metadata(&root)?;
            if before.file_type().is_symlink() {
                let target = std::fs::read_link(&root)?;
                if before.uid() != 0
                    || ![
                        PathBuf::from(format!("private/{alias}")),
                        PathBuf::from(format!("/private/{alias}")),
                    ]
                    .contains(&target)
                    || Stamp::metadata(&before)
                        != Stamp::metadata(&std::fs::symlink_metadata(&root)?)
                {
                    return Err(MediaError::invalid(
                        "unsafe or changed macOS system cache alias",
                    ));
                }
                return Ok(PathBuf::from("/private").join(alias).join(relative));
            }
        }
    }
    Ok(path)
}

pub(crate) struct Directory {
    pub(crate) path: PathBuf,
    pub(crate) file: File,
    secure: bool,
}

fn owner(metadata: &Metadata, leaf: bool) -> Result<()> {
    let uid = unsafe { libc::geteuid() };
    if ![0, uid].contains(&metadata.uid()) || (leaf && metadata.uid() != uid) {
        return Err(MediaError::invalid(
            "cache path has an unsafe owner; use a current-user cache directory",
        ));
    }
    if metadata.mode() & 0o022 != 0 && (leaf || metadata.mode() & 0o1000 == 0) {
        return Err(MediaError::invalid(
            "cache path is group/world writable without ancestor sticky-bit protection",
        ));
    }
    Ok(())
}

impl Directory {
    pub(crate) fn open(path: &Path, create: bool, secure: bool) -> Result<Self> {
        let path = absolute(path)?;
        let flags = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC;
        let fd = unsafe { libc::open(c"/".as_ptr(), flags) };
        if fd < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        let mut file = unsafe { File::from_raw_fd(fd) };
        let parts: Vec<_> = path
            .components()
            .filter_map(|c| match c {
                Component::Normal(name) => Some(name),
                _ => None,
            })
            .collect();
        if secure {
            owner(&file.metadata()?, parts.is_empty())?;
        }
        for (index, name) in parts.iter().enumerate() {
            if create {
                let name_c = cstring(name)?;
                let status = unsafe { libc::mkdirat(file.as_raw_fd(), name_c.as_ptr(), 0o700) };
                if status != 0
                    && std::io::Error::last_os_error().kind() != std::io::ErrorKind::AlreadyExists
                {
                    return Err(std::io::Error::last_os_error().into());
                }
            }
            let (kind, dev, ino) = entry_type(&file, name)?;
            if kind & 0o170000 != 0o040000 {
                return Err(MediaError::invalid(
                    "symlink/non-directory component in source or cache path",
                ));
            }
            let child = openat(&file, name, flags, 0)?;
            let metadata = child.metadata()?;
            if metadata.dev() != dev || metadata.ino() != ino {
                return Err(MediaError::invalid(
                    "directory identity changed while opening",
                ));
            }
            if secure {
                owner(&metadata, index == parts.len() - 1)?;
            }
            file = child;
        }
        Ok(Self { path, file, secure })
    }

    pub(crate) fn check(&self) -> Result<()> {
        let current = Self::open(&self.path, false, self.secure)?;
        if !Stamp::of(&self.file)?.same_inode(&Stamp::of(&current.file)?) {
            return Err(MediaError::invalid(
                "source/cache directory changed during acquisition",
            ));
        }
        Ok(())
    }

    pub(crate) fn source(&self, name: &OsStr, private: bool) -> Result<Source<'_>> {
        let (mode, dev, ino) = entry_type(&self.file, name)?;
        if mode & 0o170000 != 0o100000 {
            return Err(MediaError::invalid(
                "source/cache entry must be a regular file, not a symlink or device",
            ));
        }
        let file = openat(
            &self.file,
            name,
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
            0,
        )?;
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.dev() != dev || metadata.ino() != ino {
            return Err(MediaError::acquisition(
                "source/cache identity changed while opening",
            ));
        }
        if private
            && (metadata.uid() != unsafe { libc::geteuid() }
                || metadata.nlink() != 1
                || metadata.mode() & 0o077 != 0)
        {
            return Err(MediaError::invalid(
                "cached FFU must be private, single-link, and owned by the current user",
            ));
        }
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_SH | libc::LOCK_NB) } != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        let stamp = Stamp::of(&file)?;
        Ok(Source {
            directory: self,
            file,
            name: name.to_owned(),
            stamp,
        })
    }

    pub(crate) fn new_file(&self, name: &str) -> Result<File> {
        openat(
            &self.file,
            OsStr::new(name),
            libc::O_RDWR | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0o600,
        )
    }

    pub(crate) fn exists(&self, name: &str) -> Result<bool> {
        match entry_type(&self.file, OsStr::new(name)) {
            Ok(_) => Ok(true),
            Err(MediaError::Io(ref e)) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e),
        }
    }

    pub(crate) fn unlink(&self, name: &str) -> Result<()> {
        let name = cstring(OsStr::new(name))?;
        if unsafe { libc::unlinkat(self.file.as_raw_fd(), name.as_ptr(), 0) } != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(())
    }

    pub(crate) fn link_to(&self, name: &str, target: &Self, target_name: &str) -> Result<()> {
        let from = cstring(OsStr::new(name))?;
        let to = cstring(OsStr::new(target_name))?;
        if unsafe {
            libc::linkat(
                self.file.as_raw_fd(),
                from.as_ptr(),
                target.file.as_raw_fd(),
                to.as_ptr(),
                0,
            )
        } != 0
        {
            return Err(std::io::Error::last_os_error().into());
        }
        Ok(())
    }
}

pub(crate) struct Source<'a> {
    pub(crate) directory: &'a Directory,
    pub(crate) file: File,
    pub(crate) name: std::ffi::OsString,
    stamp: Stamp,
}

impl Source<'_> {
    pub(crate) fn check(&self) -> Result<()> {
        let named = self.directory.source(&self.name, false)?;
        if Stamp::of(&self.file)? != self.stamp || named.stamp != self.stamp {
            return Err(MediaError::acquisition(
                "source/cache file changed during acquisition",
            ));
        }
        self.directory.check()
    }
}

pub(crate) struct Workspace {
    pub(crate) directory: Directory,
    parent: File,
    name: std::ffi::OsString,
    cleaned: bool,
}

impl Workspace {
    pub(crate) fn new(cache: &Directory) -> Result<Self> {
        cache.check()?;
        let temp = tempfile::Builder::new()
            .prefix(".iot-media-")
            .tempdir_in(&cache.path)?;
        let directory = Directory::open(temp.path(), false, true)?;
        let name = temp.path().file_name().expect("workspace name").to_owned();
        cache.check()?;
        let parent = cache.file.try_clone()?;
        let _path = temp.keep();
        Ok(Self {
            directory,
            parent,
            name,
            cleaned: false,
        })
    }

    fn cleanup(&mut self) -> Result<()> {
        if self.cleaned {
            return Ok(());
        }
        for name in ["source.package", "source.cab", "image.ffu"] {
            if self.directory.exists(name)? {
                self.directory.unlink(name)?;
            }
        }
        let (_, dev, ino) = entry_type(&self.parent, &self.name)?;
        let metadata = self.directory.file.metadata()?;
        if (dev, ino) != (metadata.dev(), metadata.ino()) {
            return Err(MediaError::acquisition(
                "workspace identity changed before cleanup",
            ));
        }
        let name = cstring(&self.name)?;
        if unsafe { libc::unlinkat(self.parent.as_raw_fd(), name.as_ptr(), libc::AT_REMOVEDIR) }
            != 0
        {
            return Err(std::io::Error::last_os_error().into());
        }
        self.cleaned = true;
        Ok(())
    }

    pub(crate) fn close(mut self) -> Result<()> {
        let identity = self.directory.check();
        self.cleanup()?;
        identity
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = self.cleanup();
    }
}

pub(crate) fn readonly(file: &File) -> Result<()> {
    file.set_permissions(std::fs::Permissions::from_mode(0o400))?;
    Ok(())
}
