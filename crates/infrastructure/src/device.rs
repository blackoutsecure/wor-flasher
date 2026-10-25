use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use thiserror::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Host {
    Linux,
    Macos,
}

#[derive(Debug, Error)]
pub enum DeviceError {
    #[error("This host is unsupported; use Debian-family Linux or macOS 13+, never Windows or WSL")]
    UnsupportedHost,
    #[error("Cannot inspect device topology: {0}")]
    Probe(String),
    #[error("Cannot determine the physical disks backing {0}; refusing to write")]
    UnknownBacking(PathBuf),
    #[error("Refusing an unsafe, non-whole, read-only, virtual or protected target: {0}")]
    UnsafeTarget(PathBuf),
    #[error("The selected target changed after confirmation; select and confirm it again")]
    ChangedTarget,
    #[error("Cannot access {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("Invalid device metadata: {0}")]
    Metadata(&'static str),
}

impl Host {
    pub fn detect() -> Result<Self, DeviceError> {
        if cfg!(target_os = "macos") {
            let output = Command::new("/usr/bin/sw_vers")
                .arg("-productVersion")
                .output()
                .map_err(|error| DeviceError::Probe(error.to_string()))?;
            let version = String::from_utf8_lossy(&output.stdout);
            if !output.status.success()
                || version
                    .split('.')
                    .next()
                    .and_then(|v| v.trim().parse::<u32>().ok())
                    .is_none_or(|v| v < 13)
            {
                return Err(DeviceError::UnsupportedHost);
            }
            return Ok(Self::Macos);
        }
        if !cfg!(target_os = "linux")
            || std::env::var_os("WSL_DISTRO_NAME").is_some()
            || std::env::var_os("WSLENV").is_some()
        {
            return Err(DeviceError::UnsupportedHost);
        }
        let version = fs::read_to_string("/proc/version")
            .map_err(|error| DeviceError::Probe(error.to_string()))?;
        if version.to_ascii_lowercase().contains("microsoft")
            || version.to_ascii_lowercase().contains("wsl")
        {
            return Err(DeviceError::UnsupportedHost);
        }
        let release = fs::read_to_string("/etc/os-release")
            .map_err(|error| DeviceError::Probe(error.to_string()))?;
        if !debian_family(&release) {
            return Err(DeviceError::UnsupportedHost);
        }
        Ok(Self::Linux)
    }
}

fn debian_family(release: &str) -> bool {
    release.lines().any(|line| {
        line.strip_prefix("ID=")
            .or_else(|| line.strip_prefix("ID_LIKE="))
            .is_some_and(|value| {
                value
                    .trim_matches(['"', '\''])
                    .split_whitespace()
                    .any(|part| matches!(part, "debian" | "ubuntu" | "linuxmint" | "raspbian"))
            })
    })
}

#[cfg(test)]
mod host_tests {
    use super::debian_family;

    #[test]
    fn ubuntu_and_both_raspberry_pi_os_identities_are_accepted() {
        for release in [
            "ID=ubuntu\nID_LIKE=debian\n",
            "ID=raspbian\nID_LIKE=debian\n",
            "ID=debian\nPRETTY_NAME=\"Debian GNU/Linux 12 (bookworm)\"\n",
            "ID=derivative\nID_LIKE=\"ubuntu debian\"\n",
            "ID=derivative\nID_LIKE='debian'\n",
        ] {
            assert!(debian_family(release), "{release}");
        }
    }

    #[test]
    fn unrelated_distributions_do_not_gain_write_support() {
        for release in [
            "",
            "ID=fedora\n",
            "ID=arch\n",
            "ID=notdebian\n",
            "NAME=ubuntu\n",
        ] {
            assert!(!debian_family(release), "{release}");
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeKind {
    File,
    Directory,
    Block,
    Character,
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeIdentity {
    pub device: u64,
    pub inode: u64,
    pub rdev: u64,
    pub kind: NodeKind,
}

pub trait DeviceSystem {
    fn output(
        &self,
        program: &str,
        args: &[OsString],
        input: Option<&[u8]>,
    ) -> Result<Vec<u8>, DeviceError>;
    fn canonical(&self, path: &Path) -> Result<PathBuf, DeviceError>;
    fn node(&self, path: &Path) -> Result<NodeIdentity, DeviceError>;
}

#[derive(Clone, Copy)]
pub struct RealDeviceSystem;

impl DeviceSystem for RealDeviceSystem {
    fn output(
        &self,
        program: &str,
        args: &[OsString],
        input: Option<&[u8]>,
    ) -> Result<Vec<u8>, DeviceError> {
        let mut command = Command::new(program);
        command
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if input.is_some() {
            command.stdin(Stdio::piped());
        } else {
            command.stdin(Stdio::null());
        }
        let mut child = command
            .spawn()
            .map_err(|error| DeviceError::Probe(format!("{program}: {error}")))?;
        if let Some(input) = input {
            child
                .stdin
                .take()
                .ok_or_else(|| DeviceError::Probe("Missing metadata input pipe".into()))?
                .write_all(input)
                .map_err(|error| DeviceError::Probe(error.to_string()))?;
        }
        let output = child
            .wait_with_output()
            .map_err(|error| DeviceError::Probe(error.to_string()))?;
        if !output.status.success() {
            return Err(DeviceError::Probe(format!(
                "{program} failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        if output.stdout.len() > 4 * 1024 * 1024 {
            return Err(DeviceError::Probe(
                "Device metadata exceeded its bound".into(),
            ));
        }
        Ok(output.stdout)
    }
    fn canonical(&self, path: &Path) -> Result<PathBuf, DeviceError> {
        path.canonicalize().map_err(|source| DeviceError::Io {
            path: path.into(),
            source,
        })
    }
    fn node(&self, path: &Path) -> Result<NodeIdentity, DeviceError> {
        let metadata = fs::symlink_metadata(path).map_err(|source| DeviceError::Io {
            path: path.into(),
            source,
        })?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::{FileTypeExt, MetadataExt};
            let file_type = metadata.file_type();
            let kind = if file_type.is_block_device() {
                NodeKind::Block
            } else if file_type.is_char_device() {
                NodeKind::Character
            } else if file_type.is_file() {
                NodeKind::File
            } else if file_type.is_dir() {
                NodeKind::Directory
            } else {
                NodeKind::Other
            };
            Ok(NodeIdentity {
                device: metadata.dev(),
                inode: metadata.ino(),
                rdev: metadata.rdev(),
                kind,
            })
        }
        #[cfg(not(unix))]
        {
            let _ = metadata;
            Err(DeviceError::UnsupportedHost)
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceSnapshot {
    pub path: PathBuf,
    pub raw_path: PathBuf,
    pub bytes: u64,
    pub sector_size: u32,
    pub layout: String,
    pub model: String,
    pub node: NodeIdentity,
    pub raw_node: NodeIdentity,
}

impl DeviceSnapshot {
    pub fn description(&self) -> String {
        let model = self.model.replace(['\r', '\n', '\t'], " ");
        format!(
            "{} ({} bytes, {})",
            self.path.display(),
            self.bytes,
            model.trim()
        )
    }
    pub fn same_identity(&self, other: &Self) -> bool {
        self.path == other.path
            && self.raw_path == other.raw_path
            && self.bytes == other.bytes
            && self.sector_size == other.sector_size
            && self.node == other.node
            && self.raw_node == other.raw_node
    }
}

pub struct DeviceInspector<S = RealDeviceSystem> {
    pub host: Host,
    pub system: S,
}

fn arguments(values: &[&str]) -> Vec<OsString> {
    values.iter().map(OsString::from).collect()
}

fn json(bytes: &[u8]) -> Result<Value, DeviceError> {
    serde_json::from_slice(bytes)
        .map_err(|error| DeviceError::Probe(format!("Malformed system JSON: {error}")))
}

fn uint(value: Option<&Value>, field: &'static str) -> Result<u64, DeviceError> {
    value
        .and_then(|value| {
            value
                .as_u64()
                .or_else(|| value.as_str().and_then(|v| v.parse().ok()))
        })
        .ok_or(DeviceError::Metadata(field))
}

fn disk_name(value: &str) -> bool {
    value
        .strip_prefix("disk")
        .is_some_and(|suffix| !suffix.is_empty() && suffix.bytes().all(|v| v.is_ascii_digit()))
}

fn linux_disks(value: &Value, found: &mut BTreeSet<PathBuf>) -> Result<(), DeviceError> {
    let entries = value
        .get("blockdevices")
        .or_else(|| value.get("children"))
        .and_then(Value::as_array)
        .ok_or(DeviceError::Metadata("blockdevices/children"))?;
    for entry in entries {
        let kind = entry
            .get("type")
            .and_then(Value::as_str)
            .ok_or(DeviceError::Metadata("type"))?;
        let name = entry
            .get("name")
            .and_then(Value::as_str)
            .ok_or(DeviceError::Metadata("name"))?;
        if !name.starts_with("/dev/") {
            return Err(DeviceError::Metadata("absolute block path"));
        }
        if matches!(kind, "disk" | "loop") {
            found.insert(name.into());
        }
        if entry.get("children").is_some() {
            linux_disks(entry, found)?;
        }
    }
    Ok(())
}

impl<S: DeviceSystem> DeviceInspector<S> {
    fn plist(&self, args: &[OsString]) -> Result<Value, DeviceError> {
        let bytes = self.system.output("/usr/sbin/diskutil", args, None)?;
        let bytes = self.system.output(
            "/usr/bin/plutil",
            &arguments(&["-convert", "json", "-o", "-", "-"]),
            Some(&bytes),
        )?;
        json(&bytes)
    }
    fn mac_info(&self, path: &Path) -> Result<Value, DeviceError> {
        self.plist(&[
            OsString::from("info"),
            OsString::from("-plist"),
            path.as_os_str().into(),
        ])
    }
    fn canonical_disks(
        &self,
        disks: BTreeSet<PathBuf>,
        source: &Path,
    ) -> Result<BTreeSet<PathBuf>, DeviceError> {
        if disks.is_empty() {
            return Err(DeviceError::UnknownBacking(source.into()));
        }
        disks
            .into_iter()
            .map(|disk| self.system.canonical(&disk))
            .collect()
    }
    pub fn backing_disks(&self, path: &Path) -> Result<BTreeSet<PathBuf>, DeviceError> {
        match self.host {
            Host::Linux => {
                let output = self.system.output(
                    "findmnt",
                    &[
                        OsString::from("-n"),
                        OsString::from("-o"),
                        OsString::from("SOURCE"),
                        OsString::from("--target"),
                        path.as_os_str().into(),
                    ],
                    None,
                )?;
                let text = std::str::from_utf8(&output)
                    .map_err(|_| DeviceError::Metadata("mount source"))?;
                let source = text
                    .trim()
                    .split('[')
                    .next()
                    .ok_or_else(|| DeviceError::UnknownBacking(path.into()))?;
                if !source.starts_with("/dev/") {
                    return Err(DeviceError::UnknownBacking(path.into()));
                }
                let output = self.system.output(
                    "lsblk",
                    &arguments(&[
                        "--inverse",
                        "--json",
                        "--paths",
                        "--output",
                        "NAME,TYPE",
                        source,
                    ]),
                    None,
                )?;
                let mut disks = BTreeSet::new();
                linux_disks(&json(&output)?, &mut disks)?;
                self.canonical_disks(disks, path)
            }
            Host::Macos => {
                let info = self.mac_info(path)?;
                let mut stores = BTreeSet::new();
                if let Some(entries) = info.get("APFSPhysicalStores").and_then(Value::as_array) {
                    for entry in entries {
                        let name = entry
                            .as_str()
                            .or_else(|| entry.get("APFSPhysicalStore").and_then(Value::as_str))
                            .or_else(|| entry.get("DeviceIdentifier").and_then(Value::as_str))
                            .ok_or(DeviceError::Metadata("APFS physical store"))?;
                        let store = self.mac_info(Path::new(&format!("/dev/{name}")))?;
                        let disk = store
                            .get("ParentWholeDisk")
                            .or_else(|| store.get("DeviceIdentifier"))
                            .and_then(Value::as_str)
                            .filter(|v| disk_name(v))
                            .ok_or(DeviceError::Metadata("physical store parent"))?;
                        stores.insert(PathBuf::from(format!("/dev/{disk}")));
                    }
                }
                if stores.is_empty() {
                    let disk = info
                        .get("ParentWholeDisk")
                        .or_else(|| info.get("DeviceIdentifier"))
                        .and_then(Value::as_str)
                        .filter(|v| disk_name(v))
                        .ok_or_else(|| DeviceError::UnknownBacking(path.into()))?;
                    let parent_path = PathBuf::from(format!("/dev/{disk}"));
                    let parent = self.mac_info(&parent_path)?;
                    if parent.get("VirtualOrPhysical").and_then(Value::as_str) == Some("Physical") {
                        stores.insert(parent_path);
                    } else {
                        let details = self.plist(&arguments(&["apfs", "list", "-plist", disk]))?;
                        for container in details
                            .get("Containers")
                            .and_then(Value::as_array)
                            .ok_or_else(|| DeviceError::UnknownBacking(path.into()))?
                        {
                            for store in container
                                .get("PhysicalStores")
                                .and_then(Value::as_array)
                                .ok_or(DeviceError::Metadata("APFS stores"))?
                            {
                                let name = store
                                    .get("DeviceIdentifier")
                                    .and_then(Value::as_str)
                                    .ok_or(DeviceError::Metadata("APFS store identifier"))?;
                                let store = self.mac_info(Path::new(&format!("/dev/{name}")))?;
                                let disk = store
                                    .get("ParentWholeDisk")
                                    .or_else(|| store.get("DeviceIdentifier"))
                                    .and_then(Value::as_str)
                                    .filter(|v| disk_name(v))
                                    .ok_or(DeviceError::Metadata("APFS store disk"))?;
                                stores.insert(format!("/dev/{disk}").into());
                            }
                        }
                    }
                }
                self.canonical_disks(stores, path)
            }
        }
    }

    pub fn protected_disks(&self, paths: &[PathBuf]) -> Result<BTreeSet<PathBuf>, DeviceError> {
        let mut disks = self.backing_disks(Path::new("/"))?;
        for path in paths {
            let mut existing = path.as_path();
            loop {
                match self.system.node(existing) {
                    Ok(_) => break,
                    Err(DeviceError::Io { source, .. })
                        if source.kind() == std::io::ErrorKind::NotFound =>
                    {
                        existing = existing
                            .parent()
                            .ok_or_else(|| DeviceError::UnknownBacking(path.clone()))?;
                    }
                    Err(error) => return Err(error),
                }
            }
            disks.extend(self.backing_disks(existing)?);
        }
        Ok(disks)
    }

    pub fn inspect(
        &self,
        path: &Path,
        protected: &BTreeSet<PathBuf>,
        iot: bool,
    ) -> Result<DeviceSnapshot, DeviceError> {
        let path = self.system.canonical(path)?;
        if !path.starts_with("/dev") || protected.contains(&path) {
            return Err(DeviceError::UnsafeTarget(path));
        }
        let node = self.system.node(&path)?;
        if node.kind != NodeKind::Block {
            return Err(DeviceError::UnsafeTarget(path));
        }
        match self.host {
            Host::Linux => {
                let bytes = self.system.output(
                    "lsblk",
                    &[
                        OsString::from("--json"),
                        OsString::from("--bytes"),
                        OsString::from("--paths"),
                        OsString::from("--nodeps"),
                        OsString::from("--output"),
                        OsString::from("NAME,TYPE,RO,SIZE,LOG-SEC,PTTYPE,MODEL"),
                        path.as_os_str().into(),
                    ],
                    None,
                )?;
                let data = json(&bytes)?;
                let entries = data
                    .get("blockdevices")
                    .and_then(Value::as_array)
                    .filter(|v| v.len() == 1)
                    .ok_or(DeviceError::Metadata("single target"))?;
                let info = &entries[0];
                let kind = info
                    .get("type")
                    .and_then(Value::as_str)
                    .ok_or(DeviceError::Metadata("type"))?;
                let readonly = info
                    .get("ro")
                    .and_then(|v| v.as_bool().or_else(|| v.as_u64().map(|v| v != 0)))
                    .ok_or(DeviceError::Metadata("read-only state"))?;
                if readonly || (kind != "disk" && (kind != "loop" || iot)) {
                    return Err(DeviceError::UnsafeTarget(path));
                }
                let reported = info
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or(DeviceError::Metadata("target path"))?;
                if self.system.canonical(Path::new(reported))? != path {
                    return Err(DeviceError::ChangedTarget);
                }
                let bytes = uint(info.get("size"), "size")?;
                let sector_size = u32::try_from(uint(info.get("log-sec"), "logical sector size")?)
                    .map_err(|_| DeviceError::Metadata("sector size"))?;
                if bytes == 0 || sector_size == 0 {
                    return Err(DeviceError::Metadata("positive device geometry"));
                }
                Ok(DeviceSnapshot {
                    raw_path: path.clone(),
                    path,
                    bytes,
                    sector_size,
                    layout: info
                        .get("pttype")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .into(),
                    model: info
                        .get("model")
                        .and_then(Value::as_str)
                        .unwrap_or("Unknown device")
                        .into(),
                    raw_node: node.clone(),
                    node,
                })
            }
            Host::Macos => {
                let name = path
                    .file_name()
                    .and_then(|v| v.to_str())
                    .filter(|v| disk_name(v))
                    .ok_or_else(|| DeviceError::UnsafeTarget(path.clone()))?;
                let info = self.mac_info(&path)?;
                if info.get("WholeDisk") != Some(&Value::Bool(true))
                    || info.get("Internal") != Some(&Value::Bool(false))
                    || info.get("VirtualOrPhysical").and_then(Value::as_str) != Some("Physical")
                    || !(info.get("ReadOnlyMedia") == Some(&Value::Bool(false))
                        || info.get("WritableMedia") == Some(&Value::Bool(true)))
                    || info.get("ReadOnlyMedia") == Some(&Value::Bool(true))
                {
                    return Err(DeviceError::UnsafeTarget(path));
                }
                let bytes = uint(
                    info.get("DiskSize")
                        .or_else(|| info.get("TotalSize"))
                        .or_else(|| info.get("Size")),
                    "size",
                )?;
                let sector_size =
                    u32::try_from(uint(info.get("DeviceBlockSize"), "logical sector size")?)
                        .map_err(|_| DeviceError::Metadata("sector size"))?;
                if bytes == 0 || sector_size == 0 {
                    return Err(DeviceError::Metadata("positive device geometry"));
                }
                let raw_path = PathBuf::from(format!("/dev/r{name}"));
                let raw_node = self.system.node(&raw_path)?;
                if raw_node.kind != NodeKind::Character {
                    return Err(DeviceError::UnsafeTarget(raw_path));
                }
                Ok(DeviceSnapshot {
                    path,
                    raw_path,
                    bytes,
                    sector_size,
                    layout: info
                        .get("Content")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .into(),
                    model: info
                        .get("MediaName")
                        .and_then(Value::as_str)
                        .unwrap_or("Unknown device")
                        .into(),
                    node,
                    raw_node,
                })
            }
        }
    }

    pub fn discover(&self) -> Result<(Vec<DeviceSnapshot>, Vec<String>), DeviceError> {
        let protected = self.backing_disks(Path::new("/"))?;
        let paths = match self.host {
            Host::Linux => {
                let bytes = self.system.output(
                    "lsblk",
                    &arguments(&["--json", "--paths", "--nodeps", "--output", "NAME,TYPE"]),
                    None,
                )?;
                let data = json(&bytes)?;
                let mut paths = Vec::new();
                for entry in data
                    .get("blockdevices")
                    .and_then(Value::as_array)
                    .ok_or(DeviceError::Metadata("blockdevices"))?
                {
                    if entry.get("type").and_then(Value::as_str) == Some("disk") {
                        paths.push(PathBuf::from(
                            entry
                                .get("name")
                                .and_then(Value::as_str)
                                .ok_or(DeviceError::Metadata("name"))?,
                        ));
                    }
                }
                paths
            }
            Host::Macos => {
                let data = self.plist(&arguments(&["list", "-plist", "external", "physical"]))?;
                data.get("AllDisks")
                    .and_then(Value::as_array)
                    .ok_or(DeviceError::Metadata("AllDisks"))?
                    .iter()
                    .filter_map(Value::as_str)
                    .filter(|name| disk_name(name))
                    .map(|name| PathBuf::from(format!("/dev/{name}")))
                    .collect()
            }
        };
        let mut devices = Vec::new();
        let mut warnings = Vec::new();
        for path in paths {
            match self.inspect(&path, &protected, false) {
                Ok(device) => devices.push(device),
                Err(DeviceError::UnsafeTarget(_)) => {}
                Err(error) => warnings.push(format!("Excluded {}: {error}", path.display())),
            }
        }
        Ok((devices, warnings))
    }

    pub fn revalidate(
        &self,
        expected: &DeviceSnapshot,
        paths: &[PathBuf],
        iot: bool,
        check_layout: bool,
    ) -> Result<DeviceSnapshot, DeviceError> {
        let protected = self.protected_disks(paths)?;
        let current = self.inspect(&expected.path, &protected, iot)?;
        if !expected.same_identity(&current) || (check_layout && expected.layout != current.layout)
        {
            return Err(DeviceError::ChangedTarget);
        }
        Ok(current)
    }
}
