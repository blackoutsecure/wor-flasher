use std::{
    fmt,
    fs::File,
    io::{Seek, SeekFrom},
    path::Path,
};

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::{
    Cancel, Error, ParsedFfu, Result,
    parser::read_exact,
    unix::{self, Host, Metadata},
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct TargetIdentity {
    pub device: i128,
    pub inode: u64,
    pub raw_device: i128,
}

impl fmt::Display for TargetIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{}:{}:{}",
            self.device, self.inode, self.raw_device
        )
    }
}

impl From<&Metadata> for TargetIdentity {
    fn from(value: &Metadata) -> Self {
        Self {
            device: value.device,
            inode: value.inode,
            raw_device: value.raw_device,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct TargetIdentityReport {
    pub target_id: String,
}

pub(crate) fn valid_identity(value: &str) -> bool {
    let parts: Vec<_> = value.split(':').collect();
    parts.len() == 3
        && parts.iter().enumerate().all(|(index, part)| {
            let digits = if index != 1 {
                part.strip_prefix('-').unwrap_or(part)
            } else {
                part
            };
            !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit())
        })
}

fn raw_disk_path(path: &Path) -> bool {
    path.to_str()
        .and_then(|text| text.strip_prefix("/dev/rdisk"))
        .is_some_and(|suffix| {
            !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit())
        })
}

pub(crate) fn valid_device(metadata: &Metadata, path: &Path, host: Host) -> bool {
    match host {
        Host::Macos => metadata.character() && raw_disk_path(path),
        Host::Linux => metadata.block(),
        Host::Unsupported => false,
    }
}

/// Snapshot native device-node metadata only; never open the device for I/O.
pub fn identify_target(path: &Path) -> Result<TargetIdentityReport> {
    let parent = unix::parent(path, false)?;
    let value = unix::stat_entry(&parent)?;
    if !valid_device(&value, &parent.absolute, unix::host()) {
        return Err(Error::Target(
            "target identity requires a native device node, not a file or symlink".into(),
        ));
    }
    Ok(TargetIdentityReport {
        target_id: TargetIdentity::from(&value).to_string(),
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Source,
    Fixture,
    Device,
}

pub(crate) fn open_path(path: &Path, kind: Kind, source: Option<&Metadata>) -> Result<File> {
    open_path_checked(path, kind, source, || {})
}

pub(crate) fn open_path_checked(
    path: &Path,
    kind: Kind,
    source: Option<&Metadata>,
    before_open: impl FnOnce(),
) -> Result<File> {
    let parent = unix::parent(path, kind == Kind::Source)?;
    let before = unix::stat_entry(&parent)?;
    let fail = |message: &str| {
        if kind == Kind::Source {
            Error::Source(message.into())
        } else {
            Error::Target(message.into())
        }
    };
    if before.symlink() {
        return Err(fail("symlink paths are not allowed"));
    }
    if source.is_some_and(|stamp| (stamp.device, stamp.inode) == (before.device, before.inode)) {
        return Err(Error::Target("source and target are the same file".into()));
    }
    match kind {
        Kind::Source | Kind::Fixture => {
            if !before.regular() {
                return Err(fail("source/fixture must be an existing regular file"));
            }
            if kind == Kind::Fixture && before.links != 1 {
                return Err(Error::Target(
                    "fixture target must not have hard-link aliases".into(),
                ));
            }
        }
        Kind::Device => {
            if unix::host() == Host::Unsupported {
                return Err(Error::Target(
                    "only native Linux/macOS device targets are supported".into(),
                ));
            }
            if !valid_device(&before, &parent.absolute, unix::host()) {
                return Err(Error::Target(
                    "production apply requires a block device or macOS whole /dev/rdiskN; regular files are refused".into(),
                ));
            }
        }
    }
    before_open();
    let file = unix::open_entry(
        &parent,
        kind != Kind::Source,
        kind == Kind::Device && unix::host() == Host::Linux,
    )?;
    if !before.same_node(&unix::metadata(&file)?) {
        return Err(fail("file/device identity changed while opening"));
    }
    Ok(file)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SourceStamp {
    pub device: i128,
    pub inode: u64,
    pub mode: u32,
    pub size: i64,
    pub modified: (i64, i64),
    pub changed: (i64, i64),
    pub links: u64,
}

impl From<&Metadata> for SourceStamp {
    fn from(value: &Metadata) -> Self {
        Self {
            device: value.device,
            inode: value.inode,
            mode: value.mode,
            size: value.size,
            modified: value.modified,
            changed: value.changed,
            links: value.links,
        }
    }
}

pub(crate) struct Source {
    pub file: File,
    pub stamp: SourceStamp,
    pub metadata: Metadata,
}

impl Source {
    pub fn open(path: &Path) -> Result<Self> {
        let file = open_path(path, Kind::Source, None)?;
        unix::lock(&file, false)?;
        let metadata = unix::metadata(&file)?;
        Ok(Self {
            stamp: SourceStamp::from(&metadata),
            metadata,
            file,
        })
    }

    pub fn check(&self, cancel: Cancel<'_>) -> Result<()> {
        cancel.check()?;
        if SourceStamp::from(&unix::metadata(&self.file)?) != self.stamp {
            return Err(Error::Apply(
                "source image changed; stop and do not boot an incomplete target".into(),
            ));
        }
        Ok(())
    }

    pub fn close(self) -> Result<()> {
        unix::close(self.file)
    }

    pub fn payload(&mut self, info: &ParsedFfu, block: u64, cancel: Cancel<'_>) -> Result<Vec<u8>> {
        self.check(cancel)?;
        self.file
            .seek(SeekFrom::Start(
                info.payload_offset + block * info.chunk_size,
            ))
            .map_err(|error| Error::io("seek source payload", error))?;
        let data = read_exact(&mut self.file, info.chunk_size as usize, cancel)?;
        let index = ((info.payload_offset - info.image_offset) / info.chunk_size + block) as usize;
        if Sha256::digest(&data)[..] != info.chunk_hashes[index * 32..(index + 1) * 32] {
            return Err(Error::Apply(
                "source payload changed after preflight; target may be incomplete".into(),
            ));
        }
        self.check(cancel)?;
        Ok(data)
    }
}
