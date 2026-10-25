//! Narrow Unix boundary: no-follow descriptors, native stat IDs, locks and disk ioctls.

use std::{fs::File, io, path::Path};

use crate::{Error, Result};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Metadata {
    pub device: i128,
    pub inode: u64,
    pub raw_device: i128,
    pub mode: u32,
    pub size: i64,
    pub modified: (i64, i64),
    pub changed: (i64, i64),
    pub links: u64,
}

impl Metadata {
    pub fn regular(&self) -> bool {
        self.mode & 0o170000 == 0o100000
    }

    pub fn symlink(&self) -> bool {
        self.mode & 0o170000 == 0o120000
    }

    pub fn block(&self) -> bool {
        self.mode & 0o170000 == 0o060000
    }

    pub fn character(&self) -> bool {
        self.mode & 0o170000 == 0o020000
    }

    pub fn same_node(&self, other: &Self) -> bool {
        (self.device, self.inode, self.mode, self.raw_device)
            == (other.device, other.inode, other.mode, other.raw_device)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Host {
    Linux,
    Macos,
    Unsupported,
}

pub(crate) fn host() -> Host {
    if cfg!(target_os = "linux") {
        Host::Linux
    } else if cfg!(target_os = "macos") {
        Host::Macos
    } else {
        Host::Unsupported
    }
}

pub(crate) trait Ioctl {
    fn number(&self, file: &File, request: u64, width: usize) -> Result<u64>;
    fn command(&self, file: &File, request: u64) -> Result<()>;
}

pub(crate) struct NativeIoctl;

pub(crate) fn geometry_with(file: &File, host: Host, ioctl: &dyn Ioctl) -> Result<(u64, u64)> {
    match host {
        Host::Macos => {
            let sector = ioctl.number(file, 0x4004_6418, 4)?;
            let count = ioctl.number(file, 0x4008_6419, 8)?;
            let bytes = sector.checked_mul(count).ok_or_else(|| {
                Error::Target("native device geometry overflows supported bounds".into())
            })?;
            Ok((bytes, sector))
        }
        Host::Linux => Ok((
            ioctl.number(file, 0x8000_1272 | ((size_of::<usize>() as u64) << 16), 8)?,
            ioctl.number(file, 0x1268, 4)?,
        )),
        Host::Unsupported => Err(Error::Target("unsupported host for device geometry".into())),
    }
}

pub(crate) fn cache_sync_with(
    file: &File,
    host: Host,
    fixture: bool,
    before_readback: bool,
    ioctl: &dyn Ioctl,
) -> Result<()> {
    if !fixture {
        match host {
            Host::Macos => ioctl.command(file, 0x2000_6416)?,
            Host::Linux if before_readback => ioctl.command(file, 0x1261)?,
            Host::Linux => (),
            Host::Unsupported => {
                return Err(Error::Target(
                    "unsupported host for cache synchronization".into(),
                ));
            }
        }
    }
    Ok(())
}

#[cfg(unix)]
mod imp {
    use super::*;
    use std::{
        ffi::{CStr, CString, OsStr},
        mem::MaybeUninit,
        os::{
            fd::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd},
            unix::{ffi::OsStrExt, fs::FileExt},
        },
        path::{Component, PathBuf},
    };

    fn c_string(value: &OsStr) -> Result<CString> {
        CString::new(value.as_bytes()).map_err(|_| {
            Error::io(
                "open path",
                io::Error::new(io::ErrorKind::InvalidInput, "path contains NUL"),
            )
        })
    }

    fn owned_fd(fd: libc::c_int, operation: &'static str) -> Result<OwnedFd> {
        if fd == -1 {
            Err(Error::io(operation, io::Error::last_os_error()))
        } else {
            // SAFETY: a successful open/openat returns a new descriptor owned here.
            Ok(unsafe { OwnedFd::from_raw_fd(fd) })
        }
    }

    // Native stat field widths differ across Linux and Darwin.
    #[allow(clippy::unnecessary_cast)]
    fn convert_stat(value: libc::stat) -> Metadata {
        Metadata {
            // Do not reinterpret Darwin's signed dev_t as an unsigned integer.
            device: value.st_dev as i128,
            inode: value.st_ino as u64,
            raw_device: value.st_rdev as i128,
            mode: value.st_mode as u32,
            size: value.st_size as i64,
            modified: (value.st_mtime as i64, value.st_mtime_nsec as i64),
            changed: (value.st_ctime as i64, value.st_ctime_nsec as i64),
            links: value.st_nlink as u64,
        }
    }

    pub fn metadata(file: &File) -> Result<Metadata> {
        let mut value = MaybeUninit::<libc::stat>::uninit();
        // SAFETY: fstat writes a full stat to a valid pointer on success.
        if unsafe { libc::fstat(file.as_raw_fd(), value.as_mut_ptr()) } == -1 {
            return Err(Error::io(
                "stat opened file/device",
                io::Error::last_os_error(),
            ));
        }
        // SAFETY: fstat succeeded.
        Ok(convert_stat(unsafe { value.assume_init() }))
    }

    pub struct Parent {
        pub fd: OwnedFd,
        pub name: CString,
        pub absolute: PathBuf,
    }

    pub fn parent(path: &Path, source: bool) -> Result<Parent> {
        let absolute = if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()
                .map_err(|error| Error::io("resolve absolute path", error))?
                .join(path)
        };
        let components: Vec<_> = absolute.components().collect();
        let invalid = components.len() > 64
            || components
                .iter()
                .any(|part| matches!(part, Component::ParentDir | Component::Prefix(_)));
        if invalid {
            let message = "path traversal or excessive path depth is unsupported".into();
            return Err(if source {
                Error::Source(message)
            } else {
                Error::Target(message)
            });
        }
        let mut names = components
            .iter()
            .filter_map(|part| match part {
                Component::Normal(name) => Some(*name),
                _ => None,
            })
            .collect::<Vec<_>>();
        let name = c_string(names.pop().ok_or_else(|| {
            if source {
                Error::Source("path has no filename".into())
            } else {
                Error::Target("path has no filename".into())
            }
        })?)?;
        let flags = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC;
        // SAFETY: static NUL-terminated path; no creation flags.
        let mut fd = owned_fd(
            unsafe { libc::open(c"/".as_ptr(), flags) },
            "open path root",
        )?;
        for part in names {
            let part = c_string(part)?;
            // SAFETY: fd owns a directory and part is NUL-terminated; no-follow applies per component.
            fd = owned_fd(
                unsafe { libc::openat(fd.as_raw_fd(), part.as_ptr(), flags) },
                "open no-follow parent",
            )?;
        }
        Ok(Parent { fd, name, absolute })
    }

    pub fn stat_entry(parent: &Parent) -> Result<Metadata> {
        let mut value = MaybeUninit::<libc::stat>::uninit();
        // SAFETY: parent fd/name are valid; output is only used after success.
        if unsafe {
            libc::fstatat(
                parent.fd.as_raw_fd(),
                parent.name.as_ptr(),
                value.as_mut_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        } == -1
        {
            return Err(Error::io("stat no-follow path", io::Error::last_os_error()));
        }
        // SAFETY: fstatat succeeded.
        Ok(convert_stat(unsafe { value.assume_init() }))
    }

    pub fn open_entry(parent: &Parent, write: bool, exclusive_device: bool) -> Result<File> {
        #[cfg(target_os = "linux")]
        let large_file = libc::O_LARGEFILE;
        #[cfg(not(target_os = "linux"))]
        let large_file = 0;
        let flags = if write { libc::O_RDWR } else { libc::O_RDONLY }
            | libc::O_NOFOLLOW
            | libc::O_CLOEXEC
            | libc::O_NONBLOCK
            | large_file
            | if exclusive_device { libc::O_EXCL } else { 0 };
        // SAFETY: valid owned directory/name; deliberately no O_CREAT or O_TRUNC.
        let fd = owned_fd(
            unsafe { libc::openat(parent.fd.as_raw_fd(), parent.name.as_ptr(), flags) },
            "open no-follow file/device",
        )?;
        Ok(File::from(fd))
    }

    pub fn lock(file: &File, exclusive: bool) -> Result<()> {
        let operation = if exclusive {
            libc::LOCK_EX
        } else {
            libc::LOCK_SH
        } | libc::LOCK_NB;
        // SAFETY: flock uses a live descriptor and integer flags only.
        if unsafe { libc::flock(file.as_raw_fd(), operation) } == -1 {
            return Err(Error::io("lock source/fixture", io::Error::last_os_error()));
        }
        Ok(())
    }

    pub fn read_at(file: &File, data: &mut [u8], offset: u64) -> io::Result<usize> {
        file.read_at(data, offset)
    }

    pub fn write_at(file: &File, data: &[u8], offset: u64) -> io::Result<usize> {
        file.write_at(data, offset)
    }

    pub fn close(file: File) -> Result<()> {
        let fd = file.into_raw_fd();
        // SAFETY: ownership is consumed exactly once. Never retry close after
        // EINTR: the descriptor may already be released and reused by a thread.
        if unsafe { libc::close(fd) } == -1 {
            return Err(Error::io(
                "close opened file/device",
                io::Error::last_os_error(),
            ));
        }
        Ok(())
    }

    impl Ioctl for NativeIoctl {
        fn number(&self, file: &File, request: u64, width: usize) -> Result<u64> {
            let mut data = [0_u8; 8];
            if width != 4 && width != 8 {
                return Err(Error::Target("invalid native ioctl width".into()));
            }
            // SAFETY: the reviewed requests write exactly 4 or 8 bytes into an 8-byte buffer.
            if unsafe {
                libc::ioctl(
                    file.as_raw_fd(),
                    request as libc::c_ulong,
                    data.as_mut_ptr(),
                )
            } == -1
            {
                return Err(Error::io(
                    "query device geometry",
                    io::Error::last_os_error(),
                ));
            }
            Ok(if width == 4 {
                u32::from_ne_bytes(data[..4].try_into().expect("four bytes")) as u64
            } else {
                u64::from_ne_bytes(data)
            })
        }

        fn command(&self, file: &File, request: u64) -> Result<()> {
            // SAFETY: these reviewed flush/invalidate requests take no third argument.
            if unsafe { libc::ioctl(file.as_raw_fd(), request as libc::c_ulong) } == -1 {
                return Err(Error::io(
                    "flush/invalidate device cache",
                    io::Error::last_os_error(),
                ));
            }
            Ok(())
        }
    }

    pub fn release() -> Result<String> {
        let mut value = MaybeUninit::<libc::utsname>::uninit();
        // SAFETY: uname initializes utsname on success.
        if unsafe { libc::uname(value.as_mut_ptr()) } == -1 {
            return Err(Error::io(
                "identify native host",
                io::Error::last_os_error(),
            ));
        }
        // SAFETY: uname succeeded and release is a NUL-terminated system field.
        Ok(
            unsafe { CStr::from_ptr(value.assume_init().release.as_ptr()) }
                .to_string_lossy()
                .into_owned(),
        )
    }
}

#[cfg(unix)]
pub(crate) use imp::*;

#[cfg(not(unix))]
mod imp {
    use super::*;
    pub struct Parent {
        pub absolute: std::path::PathBuf,
    }
    fn unsupported<T>() -> Result<T> {
        Err(Error::Source(
            "host lacks required no-follow file-opening safeguards".into(),
        ))
    }
    pub fn parent(_path: &Path, _source: bool) -> Result<Parent> {
        unsupported()
    }
    pub fn stat_entry(_parent: &Parent) -> Result<Metadata> {
        unsupported()
    }
    pub fn open_entry(_parent: &Parent, _write: bool, _exclusive_device: bool) -> Result<File> {
        unsupported()
    }
    pub fn metadata(_file: &File) -> Result<Metadata> {
        unsupported()
    }
    pub fn lock(_file: &File, _exclusive: bool) -> Result<()> {
        unsupported()
    }
    pub fn read_at(_file: &File, _data: &mut [u8], _offset: u64) -> io::Result<usize> {
        Err(io::ErrorKind::Unsupported.into())
    }
    pub fn write_at(_file: &File, _data: &[u8], _offset: u64) -> io::Result<usize> {
        Err(io::ErrorKind::Unsupported.into())
    }
    pub fn release() -> Result<String> {
        unsupported()
    }
    pub fn close(_file: File) -> Result<()> {
        unsupported()
    }
    impl Ioctl for NativeIoctl {
        fn number(&self, _file: &File, _request: u64, _width: usize) -> Result<u64> {
            unsupported()
        }
        fn command(&self, _file: &File, _request: u64) -> Result<()> {
            unsupported()
        }
    }
}

#[cfg(not(unix))]
pub(crate) use imp::*;
