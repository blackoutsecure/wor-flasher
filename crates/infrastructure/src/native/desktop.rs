use super::{NativeContext, NativeError, fingerprint};
use crate::device::{DeviceInspector, DeviceSnapshot, DeviceSystem, Host, RealDeviceSystem};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use wor_desktop_media::{PrepareOptions, PreparedMedia};
use wor_domain::progress::Progress;
use wor_pi3_boot_refresh::{BootstrapDisk, DiskLayout, ReadAt, VolumeExtent, refresh_bootstrap};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolStep {
    pub program: String,
    pub arguments: Vec<OsString>,
}

fn step(program: &str, args: Vec<OsString>) -> ToolStep {
    ToolStep {
        program: program.into(),
        arguments: args,
    }
}
fn args(values: &[&str]) -> Vec<OsString> {
    values.iter().map(OsString::from).collect()
}

pub fn partition_plan(
    host: Host,
    target: &DeviceSnapshot,
    boot_bytes: u64,
    direct: bool,
) -> Result<Vec<ToolStep>, NativeError> {
    if target.sector_size == 0 || target.bytes < 8 * 1024_u64.pow(3) {
        return Err(NativeError::Failed("Invalid target geometry".into()));
    }
    let device = target.path.as_os_str().to_os_string();
    if host == Host::Linux {
        let end = if direct { "19000MB" } else { "7000MB" };
        return Ok(vec![
            step(
                "parted",
                vec!["-s".into(), device.clone(), "mklabel".into(), "gpt".into()],
            ),
            step(
                "parted",
                vec![
                    "-s".into(),
                    device.clone(),
                    "mkpart".into(),
                    "primary".into(),
                    "1MB".into(),
                    "1000MB".into(),
                ],
            ),
            step(
                "parted",
                vec![
                    "-s".into(),
                    device.clone(),
                    "set".into(),
                    "1".into(),
                    "esp".into(),
                    "on".into(),
                ],
            ),
            step(
                "parted",
                vec![
                    "-s".into(),
                    device.clone(),
                    "mkpart".into(),
                    "primary".into(),
                    "1000MB".into(),
                    end.into(),
                ],
            ),
            step(
                "parted",
                vec![
                    "-s".into(),
                    device.clone(),
                    "set".into(),
                    "2".into(),
                    "msftdata".into(),
                    "on".into(),
                ],
            ),
            step("partprobe", vec![device]),
        ]);
    }
    let boot_mb = (boot_bytes / (1024 * 1024))
        .checked_add(512)
        .ok_or_else(|| NativeError::Failed("Boot payload size overflow".into()))?
        .max(1536);
    let install_mb = if direct { 18000_u64 } else { 6000 };
    if boot_mb
        .checked_add(install_mb)
        .and_then(|v| v.checked_mul(1024 * 1024))
        .and_then(|v| v.checked_add(1024 * 1024 * 1024))
        .is_none_or(|v| v > target.bytes)
    {
        return Err(NativeError::Failed(
            "The target lacks space for boot/install partitions and required unallocated space"
                .into(),
        ));
    }
    Ok(vec![
        step(
            "sgdisk",
            vec!["--zap-all".into(), target.raw_path.as_os_str().into()],
        ),
        step(
            "sgdisk",
            vec![
                "-og".into(),
                "-n".into(),
                format!("1:0:+{boot_mb}M").into(),
                "-t".into(),
                "1:ef00".into(),
                "-c".into(),
                "1:WOR_BOOT".into(),
                "-n".into(),
                format!("2:0:+{install_mb}M").into(),
                "-t".into(),
                "2:0700".into(),
                "-c".into(),
                "2:WOR_INSTALL".into(),
                "-A".into(),
                "1:set:63".into(),
                "-A".into(),
                "2:set:63".into(),
                target.raw_path.as_os_str().into(),
            ],
        ),
    ])
}

pub fn partition_paths(host: Host, target: &Path) -> Result<(PathBuf, PathBuf), NativeError> {
    let text = target
        .to_str()
        .ok_or_else(|| NativeError::Failed("Target must be valid UTF-8".into()))?;
    let suffix = match host {
        Host::Macos => "s",
        Host::Linux if text.as_bytes().last().is_some_and(u8::is_ascii_digit) => "p",
        Host::Linux => "",
    };
    Ok((
        format!("{text}{suffix}1").into(),
        format!("{text}{suffix}2").into(),
    ))
}

pub fn catalog(
    context: &NativeContext,
    kind: &str,
    cancelled: &AtomicBool,
) -> Result<(), NativeError> {
    if kind == "languages" {
        println!(
            "{}",
            serde_json::json!({"languages":wor_domain::configuration::LANGUAGES.iter().map(|(code,label)|serde_json::json!({"value":code,"label":label})).collect::<Vec<_>>()})
        );
        return Ok(());
    }
    let options = PrepareOptions {
        runtime_dir: context.runtime.root().into(),
        download_dir: context.config.get("DL_DIR").into(),
        settings: context.config.values().clone(),
    };
    let mut progress_error = None;
    let value = wor_desktop_media::catalog(
        &options,
        kind,
        &mut |progress| {
            if let Err(error) = context.emit(progress) {
                progress_error = Some(error);
                cancelled.store(true, Ordering::Release);
            }
        },
        cancelled,
    )
    .map_err(|error| NativeError::Failed(error.to_string()))?;
    if let Some(error) = progress_error {
        return Err(error);
    }
    println!("{value}");
    Ok(())
}

fn plist(context: &NativeContext, values: &[OsString]) -> Result<Value, NativeError> {
    let bytes = context.tool(Path::new("/usr/sbin/diskutil"), values, None, false)?;
    Ok(serde_json::from_slice(&context.tool(
        Path::new("/usr/bin/plutil"),
        &args(&["-convert", "json", "-o", "-", "-"]),
        Some(&bytes),
        false,
    )?)?)
}

fn info(context: &NativeContext, path: &Path) -> Result<Value, NativeError> {
    plist(
        context,
        &[
            OsString::from("info"),
            OsString::from("-plist"),
            path.as_os_str().into(),
        ],
    )
}

fn number(value: Option<&Value>, field: &str) -> Result<u64, NativeError> {
    value
        .and_then(|value| {
            value
                .as_u64()
                .or_else(|| value.as_str().and_then(|v| v.parse().ok()))
        })
        .ok_or_else(|| NativeError::Failed(format!("Invalid partition {field}")))
}

fn guard_target(
    context: &NativeContext,
    target: &DeviceSnapshot,
    layout: bool,
) -> Result<(), NativeError> {
    let paths = context.protected_paths(
        (!context.config.get("SOURCE_FILE").is_empty())
            .then(|| Path::new(context.config.get("SOURCE_FILE"))),
    );
    DeviceInspector {
        host: context.host,
        system: RealDeviceSystem,
    }
    .revalidate(target, &paths, false, layout)?;
    Ok(())
}

pub fn unmount_device(context: &NativeContext, target: &DeviceSnapshot) -> Result<(), NativeError> {
    if context.host == Host::Macos {
        context.tool(
            Path::new("/usr/sbin/diskutil"),
            &[
                OsString::from("unmountDisk"),
                target.path.as_os_str().into(),
            ],
            None,
            true,
        )?;
    } else {
        let bytes = context.tool(
            Path::new("lsblk"),
            &[
                OsString::from("--json"),
                OsString::from("--paths"),
                OsString::from("--output"),
                OsString::from("NAME,MOUNTPOINTS"),
                target.path.as_os_str().into(),
            ],
            None,
            false,
        )?;
        let data: Value = serde_json::from_slice(&bytes)?;
        fn collect(value: &Value, paths: &mut Vec<PathBuf>) -> Result<(), NativeError> {
            if let Some(entries) = value.get("children").and_then(Value::as_array) {
                for entry in entries {
                    collect(entry, paths)?;
                }
            }
            if value
                .get("mountpoints")
                .and_then(Value::as_array)
                .is_some_and(|values| {
                    values
                        .iter()
                        .any(|v| v.as_str().is_some_and(|v| !v.is_empty()))
                })
            {
                paths.push(
                    value
                        .get("name")
                        .and_then(Value::as_str)
                        .ok_or_else(|| {
                            NativeError::Failed("Missing mounted partition identity".into())
                        })?
                        .into(),
                );
            }
            Ok(())
        }
        let mut mounted = Vec::new();
        for entry in data
            .get("blockdevices")
            .and_then(Value::as_array)
            .ok_or_else(|| NativeError::Failed("Missing target mount topology".into()))?
        {
            collect(entry, &mut mounted)?;
        }
        for path in mounted {
            context.tool(Path::new("umount"), &[path.as_os_str().into()], None, true)?;
        }
    }
    Ok(())
}

pub fn eject_device(context: &NativeContext, target: &DeviceSnapshot) -> Result<(), NativeError> {
    unmount_device(context, target)?;
    if context.host == Host::Macos {
        context.tool(
            Path::new("/usr/sbin/diskutil"),
            &[OsString::from("eject"), target.path.as_os_str().into()],
            None,
            false,
        )?;
    } else {
        context.tool(
            Path::new("eject"),
            &[target.path.as_os_str().into()],
            None,
            true,
        )?;
    }
    Ok(())
}

#[derive(Clone)]
struct FileRecord {
    length: u64,
    digest: [u8; 32],
}

#[cfg(test)]
std::thread_local! {
    static PAYLOAD_BUFFER_ALLOCATIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

fn payload_buffer() -> Vec<u8> {
    #[cfg(test)]
    PAYLOAD_BUFFER_ALLOCATIONS.with(|count| count.set(count.get() + 1));
    vec![0; 1024 * 1024]
}

fn digest_file(path: &Path, cancelled: &AtomicBool) -> Result<FileRecord, NativeError> {
    digest_file_buffered(path, cancelled, &mut payload_buffer())
}

fn digest_file_buffered(
    path: &Path,
    cancelled: &AtomicBool,
    buffer: &mut [u8],
) -> Result<FileRecord, NativeError> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(NativeError::Failed(
            "Payload paths must be regular, non-symlink files".into(),
        ));
    }
    let mut input = File::open(path)?;
    let mut hash = Sha256::new();
    let mut length = 0_u64;
    loop {
        if cancelled.load(Ordering::Acquire) {
            return Err(NativeError::Canceled);
        }
        let count = input.read(buffer)?;
        if count == 0 {
            break;
        }
        length = length
            .checked_add(
                u64::try_from(count)
                    .map_err(|_| NativeError::Failed("Payload length overflow".into()))?,
            )
            .ok_or_else(|| NativeError::Failed("Payload length overflow".into()))?;
        hash.update(&buffer[..count]);
    }
    if input.metadata()?.len() != metadata.len() || length != metadata.len() {
        return Err(NativeError::Failed(
            "Source changed during verification".into(),
        ));
    }
    Ok(FileRecord {
        length,
        digest: hash.finalize().into(),
    })
}

fn tree_manifest(
    root: &Path,
    cancelled: &AtomicBool,
) -> Result<BTreeMap<PathBuf, FileRecord>, NativeError> {
    fn walk(
        root: &Path,
        path: &Path,
        out: &mut BTreeMap<PathBuf, FileRecord>,
        cancelled: &AtomicBool,
        scratch: &mut Option<Vec<u8>>,
    ) -> Result<(), NativeError> {
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path)?;
            if metadata.file_type().is_symlink() {
                return Err(NativeError::Failed("Symlink in prepared payload".into()));
            }
            if metadata.is_dir() {
                walk(root, &path, out, cancelled, scratch)?;
            } else {
                out.insert(
                    path.strip_prefix(root)
                        .map_err(|_| {
                            NativeError::Failed("Payload escaped staging directory".into())
                        })?
                        .into(),
                    digest_file_buffered(
                        &path,
                        cancelled,
                        scratch.get_or_insert_with(payload_buffer),
                    )?,
                );
            }
        }
        Ok(())
    }
    let mut result = BTreeMap::new();
    walk(root, root, &mut result, cancelled, &mut None)?;
    Ok(result)
}

fn copy_manifest(
    source: &Path,
    target: &Path,
    manifest: &BTreeMap<PathBuf, FileRecord>,
    cancelled: &AtomicBool,
) -> Result<(), NativeError> {
    let mut scratch = None;
    for (relative, expected) in manifest {
        if cancelled.load(Ordering::Acquire) {
            return Err(NativeError::Canceled);
        }
        let from = source.join(relative);
        let to = target.join(relative);
        let buffer = scratch.get_or_insert_with(payload_buffer);
        let current = digest_file_buffered(&from, cancelled, buffer)?;
        if current.digest != expected.digest || current.length != expected.length {
            return Err(NativeError::Failed(
                "Prepared source changed before copy".into(),
            ));
        }
        fs::create_dir_all(
            to.parent()
                .ok_or_else(|| NativeError::Failed("Missing payload parent".into()))?,
        )?;
        if fs::symlink_metadata(&to).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
            return Err(NativeError::Failed(
                "Target payload path became a symlink".into(),
            ));
        }
        let mut input = File::open(&from)?;
        let mut output = File::create(&to)?;
        let mut hash = Sha256::new();
        loop {
            if cancelled.load(Ordering::Acquire) {
                return Err(NativeError::Canceled);
            }
            let count = input.read(buffer)?;
            if count == 0 {
                break;
            }
            output.write_all(&buffer[..count])?;
            hash.update(&buffer[..count]);
        }
        output.sync_all()?;
        let copied: [u8; 32] = hash.finalize().into();
        if copied != expected.digest {
            return Err(NativeError::Failed(
                "Source changed while copying; target is incomplete".into(),
            ));
        }
    }
    Ok(())
}

fn verify_manifest(
    target: &Path,
    manifest: &BTreeMap<PathBuf, FileRecord>,
    cancelled: &AtomicBool,
) -> Result<(), NativeError> {
    let mut scratch = None;
    for (relative, expected) in manifest {
        let actual = digest_file_buffered(
            &target.join(relative),
            cancelled,
            scratch.get_or_insert_with(payload_buffer),
        )?;
        if actual.length != expected.length || actual.digest != expected.digest {
            return Err(NativeError::Failed(format!(
                "Written payload differs from its frozen source: {}",
                relative.display()
            )));
        }
    }
    Ok(())
}

struct Mounts<'a> {
    context: &'a NativeContext,
    partitions: Vec<PathBuf>,
    directory: Option<PathBuf>,
}

impl Mounts<'_> {
    fn finish(&mut self) -> Result<(), NativeError> {
        let mut errors = Vec::new();
        while let Some(partition) = self.partitions.pop() {
            let result = if self.context.host == Host::Macos {
                self.context.tool(
                    Path::new("/usr/sbin/diskutil"),
                    &[OsString::from("unmount"), partition.as_os_str().into()],
                    None,
                    false,
                )
            } else {
                self.context.tool(
                    Path::new("umount"),
                    &[partition.as_os_str().into()],
                    None,
                    true,
                )
            };
            if let Err(error) = result {
                errors.push(error.to_string());
            }
        }
        if errors.is_empty() {
            if let Some(root) = self.directory.take() {
                fs::remove_dir(root.join("boot"))?;
                fs::remove_dir(root.join("install"))?;
                fs::remove_dir(root)?;
            }
            Ok(())
        } else {
            Err(NativeError::Cleanup(errors.join("; ")))
        }
    }
}

impl Drop for Mounts<'_> {
    fn drop(&mut self) {
        if !self.partitions.is_empty()
            && let Err(error) = self.finish()
        {
            tracing::error!(error=%error,"Target cleanup failed; mounted media was not recursively deleted");
        }
    }
}

fn mounted_path(context: &NativeContext, partition: &Path) -> Result<PathBuf, NativeError> {
    let details = info(context, partition)?;
    let path = PathBuf::from(
        details
            .get("MountPoint")
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty())
            .ok_or_else(|| NativeError::Failed("Target partition is not mounted".into()))?,
    );
    let backing = DeviceInspector {
        host: context.host,
        system: RealDeviceSystem,
    }
    .backing_disks(&path)?;
    if !backing.contains(Path::new(context.config.get("DEVICE"))) {
        return Err(NativeError::Failed(
            "Mounted volume no longer belongs to the selected disk".into(),
        ));
    }
    Ok(path)
}

fn mount<'a>(
    context: &'a NativeContext,
    part1: &Path,
    part2: &Path,
) -> Result<(Mounts<'a>, PathBuf, PathBuf), NativeError> {
    let mut mounts = Mounts {
        context,
        partitions: Vec::new(),
        directory: None,
    };
    if context.host == Host::Macos {
        for partition in [part1, part2] {
            context.tool(
                Path::new("/usr/sbin/diskutil"),
                &[OsString::from("mount"), partition.as_os_str().into()],
                None,
                false,
            )?;
            mounts.partitions.push(partition.into());
        }
        let boot = mounted_path(context, part1)?;
        let install = mounted_path(context, part2)?;
        return Ok((mounts, boot, install));
    }
    let root = tempfile::tempdir()?.keep();
    fs::create_dir(root.join("boot"))?;
    fs::create_dir(root.join("install"))?;
    mounts.directory = Some(root.clone());
    let uid = String::from_utf8(context.tool(Path::new("id"), &args(&["-u"]), None, false)?)
        .map_err(|_| NativeError::Failed("Invalid user ID".into()))?;
    let gid = String::from_utf8(context.tool(Path::new("id"), &args(&["-g"]), None, false)?)
        .map_err(|_| NativeError::Failed("Invalid group ID".into()))?;
    if !uid.trim().bytes().all(|v| v.is_ascii_digit())
        || !gid.trim().bytes().all(|v| v.is_ascii_digit())
    {
        return Err(NativeError::Failed("Invalid mount ownership".into()));
    }
    let options = format!("uid={},gid={}", uid.trim(), gid.trim());
    context.tool(
        Path::new("mount"),
        &[
            OsString::from("-o"),
            options.clone().into(),
            part1.as_os_str().into(),
            root.join("boot").into_os_string(),
        ],
        None,
        true,
    )?;
    mounts.partitions.push(part1.into());
    context.tool(
        Path::new("mount.exfat-fuse"),
        &[
            OsString::from("-o"),
            options.into(),
            part2.as_os_str().into(),
            root.join("install").into_os_string(),
        ],
        None,
        true,
    )?;
    mounts.partitions.push(part2.into());
    Ok((mounts, root.join("boot"), root.join("install")))
}

fn nonempty(path: &Path) -> Result<(), NativeError> {
    let metadata = fs::metadata(path)?;
    if !metadata.is_file() || metadata.len() == 0 {
        return Err(NativeError::Failed(format!(
            "Required image artifact is absent or empty: {}",
            path.display()
        )));
    }
    Ok(())
}

fn geometry(
    context: &NativeContext,
    target: &DeviceSnapshot,
    part1: &Path,
    part2: &Path,
) -> Result<(u64, u64, u64, u64), NativeError> {
    if context.host == Host::Macos {
        let first = info(context, part1)?;
        let second = info(context, part2)?;
        for (value, label, filesystem) in [
            (&first, "WOR_BOOT", "msdos"),
            (&second, "WOR_INSTALL", "exfat"),
        ] {
            if value.get("VolumeName").and_then(Value::as_str) != Some(label)
                || value.get("FilesystemType").and_then(Value::as_str) != Some(filesystem)
            {
                return Err(NativeError::Failed(
                    "Written partition label/filesystem does not match its role".into(),
                ));
            }
        }
        let listing = plist(
            context,
            &[
                OsString::from("list"),
                OsString::from("-plist"),
                target.path.as_os_str().into(),
            ],
        )?;
        if listing
            .pointer("/AllDisksAndPartitions/0/Partitions")
            .and_then(Value::as_array)
            .is_none_or(|v| v.len() != 2)
        {
            return Err(NativeError::Failed(
                "Expected exactly two target partitions".into(),
            ));
        }
        if !matches!(
            first.get("Content").and_then(Value::as_str),
            Some("EFI" | "Microsoft Basic Data")
        ) || second.get("Content").and_then(Value::as_str) != Some("Microsoft Basic Data")
        {
            return Err(NativeError::Failed(
                "Written partition roles are incorrect".into(),
            ));
        }
        return Ok((
            number(first.get("PartitionMapPartitionOffset"), "boot offset")?,
            number(first.get("Size"), "boot size")?,
            number(second.get("PartitionMapPartitionOffset"), "install offset")?,
            number(second.get("Size"), "install size")?,
        ));
    }
    let bytes = context.tool(
        Path::new("parted"),
        &[
            OsString::from("-ms"),
            target.path.as_os_str().into(),
            OsString::from("unit"),
            OsString::from("B"),
            OsString::from("print"),
        ],
        None,
        true,
    )?;
    let text = String::from_utf8(bytes)
        .map_err(|_| NativeError::Failed("Invalid partition geometry".into()))?;
    let mut found = BTreeMap::new();
    for line in text.lines() {
        let fields = line.trim_end_matches(';').split(':').collect::<Vec<_>>();
        if fields.first().is_some_and(|value| {
            !value.is_empty() && value.bytes().all(|character| character.is_ascii_digit())
        }) && found.insert(fields[0], fields).is_some()
        {
            return Err(NativeError::Failed(
                "Duplicate written partition number".into(),
            ));
        }
    }
    if found.len() != 2 {
        return Err(NativeError::Failed(
            "Expected exactly two written partitions".into(),
        ));
    }
    let first = found
        .get("1")
        .ok_or_else(|| NativeError::Failed("Missing boot partition".into()))?;
    let second = found
        .get("2")
        .ok_or_else(|| NativeError::Failed("Missing installation partition".into()))?;
    if !first.last().is_some_and(|v| v.contains("esp"))
        || !second.last().is_some_and(|v| v.contains("msftdata"))
    {
        return Err(NativeError::Failed(
            "Written GPT partition flags are incorrect".into(),
        ));
    }
    for (partition, label, filesystem) in
        [(part1, "WOR_BOOT", "vfat"), (part2, "WOR_INSTALL", "exfat")]
    {
        let fs_type = context.tool(
            Path::new("blkid"),
            &[
                OsString::from("-s"),
                OsString::from("TYPE"),
                OsString::from("-o"),
                OsString::from("value"),
                partition.as_os_str().into(),
            ],
            None,
            true,
        )?;
        let fs_label = context.tool(
            Path::new("blkid"),
            &[
                OsString::from("-s"),
                OsString::from("LABEL"),
                OsString::from("-o"),
                OsString::from("value"),
                partition.as_os_str().into(),
            ],
            None,
            true,
        )?;
        if String::from_utf8_lossy(&fs_type).trim() != filesystem
            || String::from_utf8_lossy(&fs_label).trim() != label
        {
            return Err(NativeError::Failed(
                "Written filesystem/label mismatch".into(),
            ));
        }
    }
    let parse = |fields: &Vec<&str>, index: usize| -> Result<u64, NativeError> {
        fields
            .get(index)
            .and_then(|v| v.strip_suffix('B'))
            .and_then(|v| v.parse().ok())
            .ok_or_else(|| NativeError::Failed("Invalid written partition byte geometry".into()))
    };
    Ok((
        parse(first, 1)?,
        parse(first, 3)?,
        parse(second, 1)?,
        parse(second, 3)?,
    ))
}

struct VerificationInputs<'a> {
    prepared: &'a PreparedMedia,
    boot: &'a BTreeMap<PathBuf, FileRecord>,
    extras: &'a BTreeMap<PathBuf, FileRecord>,
    image: &'a FileRecord,
}

fn verify(
    context: &NativeContext,
    target: &DeviceSnapshot,
    parts: (&Path, &Path),
    mounts: (&Path, &Path),
    source: VerificationInputs<'_>,
    cancelled: &AtomicBool,
) -> Result<(u64, u64, u64, u64), NativeError> {
    context.tool(Path::new("sync"), &[], None, false)?;
    let layout = geometry(context, target, parts.0, parts.1)?;
    let end = layout
        .2
        .checked_add(layout.3)
        .filter(|v| *v <= target.bytes)
        .ok_or_else(|| NativeError::Failed("Written partitions exceed target geometry".into()))?;
    if target.bytes - end < 1024 * 1024 * 1024 {
        return Err(NativeError::Failed(
            "Less than 1 GiB remains for the Windows installation partition".into(),
        ));
    }
    for required in [
        "RPI_EFI.fd",
        "EFI/BOOT/BOOTAA64.EFI",
        "EFI/Microsoft/Boot/bcd",
        "sources/boot.wim",
    ] {
        nonempty(&mounts.0.join(required))?;
    }
    for required in match context.config.model()? {
        3 => vec!["bootcode.bin", "start.elf"],
        4 => vec!["start4.elf", "fixup4.dat"],
        _ => vec!["bcm2712-rpi-5-b.dtb"],
    } {
        nonempty(&mounts.0.join(required))?;
    }
    verify_manifest(mounts.0, source.boot, cancelled)?;
    verify_manifest(mounts.1, source.extras, cancelled)?;
    if context.config.model()? == 4 {
        for path in [
            "/drivers/bcmgenet/bcmgenet.inf",
            "/drivers/mcci_dwchsotg/mcci_dwchsotg_hcd.inf",
            "/drivers/mcci_dwchsotg/mcci_dwchsotg_hub.inf",
            "/drivers/rpiuxflt/rpiuxflt.inf",
        ] {
            context.tool(
                Path::new("wimdir"),
                &[
                    mounts.0.join("sources/boot.wim").into_os_string(),
                    OsString::from("2"),
                    format!("--path={path}").into(),
                ],
                None,
                false,
            )?;
        }
    }
    for path in [
        mounts.0.join("sources/boot.wim"),
        mounts.1.join("install.wim"),
    ] {
        context.tool(
            Path::new("wimverify"),
            &[path.into_os_string()],
            None,
            false,
        )?;
    }
    let written = digest_file(&mounts.1.join("install.wim"), cancelled)?;
    if written.digest != source.image.digest
        || written.length != source.image.length
        || digest_file(&source.prepared.install_wim, cancelled)?.digest != source.image.digest
    {
        return Err(NativeError::Failed(
            "Installation image readback/source identity mismatch".into(),
        ));
    }
    Ok(layout)
}

#[derive(Serialize, Deserialize)]
pub struct BootstrapJob {
    pub target: DeviceSnapshot,
    pub protected_paths: Vec<PathBuf>,
    pub offsets: (u64, u64, u64, u64),
}

struct HostDisk(File);
impl ReadAt for HostDisk {
    fn read_at(&mut self, data: &mut [u8], offset: u64) -> std::io::Result<usize> {
        #[cfg(unix)]
        {
            std::os::unix::fs::FileExt::read_at(&self.0, data, offset)
        }
        #[cfg(not(unix))]
        {
            let _ = (data, offset);
            Err(std::io::ErrorKind::Unsupported.into())
        }
    }
}
impl BootstrapDisk for HostDisk {
    fn write_at(&mut self, data: &[u8], offset: u64) -> std::io::Result<usize> {
        if data.len() != 512 || offset != 0 {
            return Err(std::io::ErrorKind::InvalidInput.into());
        }
        #[cfg(unix)]
        {
            std::os::unix::fs::FileExt::write_at(&self.0, data, offset)
        }
        #[cfg(not(unix))]
        {
            let _ = (data, offset);
            Err(std::io::ErrorKind::Unsupported.into())
        }
    }
    fn sync(&mut self) -> std::io::Result<()> {
        self.0.sync_all()
    }
}

pub fn host_bootstrap(job: BootstrapJob, backup_dir: &Path) -> Result<(), NativeError> {
    let host = Host::detect()?;
    let mut paths = job.protected_paths;
    paths.push(std::env::current_exe()?);
    DeviceInspector {
        host,
        system: RealDeviceSystem,
    }
    .revalidate(&job.target, &paths, false, false)?;
    let mut options = OpenOptions::new();
    options.read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_NONBLOCK);
    }
    let mut disk = HostDisk(options.open(&job.target.raw_path)?);
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let metadata = disk.0.metadata()?;
        if metadata.dev() != job.target.raw_node.device
            || metadata.ino() != job.target.raw_node.inode
            || metadata.rdev() != job.target.raw_node.rdev
        {
            return Err(crate::device::DeviceError::ChangedTarget.into());
        }
    }
    let (boot_offset, boot_length, win_offset, win_length) = job.offsets;
    let layout = DiskLayout {
        disk_index: 0,
        bytes: job.target.bytes,
        bytes_per_sector: job.target.sector_size,
        boot: VolumeExtent {
            disk_index: 0,
            offset: boot_offset,
            length: boot_length,
        },
        windows: VolumeExtent {
            disk_index: 0,
            offset: win_offset,
            length: win_length,
        },
    };
    fs::create_dir_all(backup_dir)?;
    refresh_bootstrap(&mut disk, &layout, |sector| {
        wor_pi3_boot_refresh::files::save_backup(
            backup_dir,
            sector,
            |file| {
                #[cfg(unix)]
                {
                    use std::os::fd::IntoRawFd;
                    nix::unistd::close(file.into_raw_fd()).map_err(|error| {
                        wor_pi3_boot_refresh::Error::io(
                            "close native bootstrap backup",
                            error.into(),
                        )
                    })
                }
                #[cfg(not(unix))]
                {
                    let _ = file;
                    Err(wor_pi3_boot_refresh::Error::UnsupportedPlatform)
                }
            },
            wor_pi3_boot_refresh::emit_event,
        )
    })
    .map_err(|error| NativeError::Failed(format!("Pi 3 bootstrap rejected: {error}")))?;
    Ok(())
}

pub fn run(context: &mut NativeContext, cancelled: &AtomicBool) -> Result<(), NativeError> {
    context.config.validate_profile()?;
    let initial = if context.config.get("DEVICE").is_empty() {
        None
    } else {
        Some(context.inspect_target(false)?)
    };
    if let Some(target) = &initial {
        context.config.validate_capacity(target.bytes, None)?;
        if let Some(expected) = &context.expected_target
            && fingerprint(target)? != *expected
        {
            return Err(crate::device::DeviceError::ChangedTarget.into());
        }
        if context.config.model()? == 3 && target.sector_size != 512 {
            return Err(NativeError::Failed(
                "Pi 3 bootstrapping requires 512-byte sectors".into(),
            ));
        }
    }
    if !context.config.flag("DRY_RUN")? && initial.is_none() {
        return Err(NativeError::Failed(
            "Choose a safe target before writing".into(),
        ));
    }
    context.emit(Progress::Step {
        current: 1,
        total: 5,
        message: "Prepare verified Windows media, firmware and drivers".into(),
    })?;
    let options = PrepareOptions {
        runtime_dir: context.runtime.root().into(),
        download_dir: context.config.get("DL_DIR").into(),
        settings: context.config.values().clone(),
    };
    let mut progress_error = None;
    let prepared = wor_desktop_media::prepare(
        &options,
        &mut |progress| {
            if let Err(error) = context.emit(progress) {
                progress_error = Some(error);
                cancelled.store(true, Ordering::Release);
            }
        },
        cancelled,
    )
    .map_err(|error| NativeError::Failed(error.to_string()))?;
    if let Some(error) = progress_error {
        prepared
            .cleanup()
            .map_err(|cleanup| NativeError::Cleanup(format!("{error}; {cleanup}")))?;
        return Err(error);
    }
    let prepared_run = (|| -> Result<(), NativeError> {
        let boot_manifest = tree_manifest(&prepared.boot_dir, cancelled)?;
        let extras = tree_manifest(&prepared.install_extras, cancelled)?;
        let image = digest_file(&prepared.install_wim, cancelled)?;
        if image.length == 0 || boot_manifest.is_empty() {
            return Err(NativeError::Failed("Prepared media is empty".into()));
        }
        let extras_bytes = extras
            .values()
            .try_fold(0_u64, |sum, file| sum.checked_add(file.length))
            .ok_or_else(|| NativeError::Failed("Setup payload size overflow".into()))?;
        let install_limit = if context.config.flag("CAN_INSTALL_ON_SAME_DRIVE")? {
            18_000_u64
        } else {
            6_000
        }
        .checked_mul(if context.host == Host::Linux {
            1_000_000
        } else {
            1024 * 1024
        })
        .ok_or_else(|| NativeError::Failed("Install partition size overflow".into()))?;
        if image
            .length
            .checked_add(extras_bytes)
            .and_then(|bytes| bytes.checked_add(64 * 1024 * 1024))
            .is_none_or(|bytes| bytes > install_limit)
        {
            return Err(NativeError::Failed("The verified Windows image and setup files exceed the selected installation partition; target was not modified".into()));
        }
        if context.config.flag("DRY_RUN")? {
            return Ok(());
        }
        context.canceled(cancelled)?;
        let target =
            initial.ok_or_else(|| NativeError::Failed("Missing confirmed target".into()))?;
        guard_target(context, &target, true)?;
        context.emit(Progress::Step {
            current: 2,
            total: 5,
            message: "Partition and format the consent-bound target".into(),
        })?;
        unmount_device(context, &target)?;
        context.emit(Progress::DiskWrite {
            active: true,
            device: target.path.to_string_lossy().into_owned(),
        })?;
        let boot_bytes = boot_manifest
            .values()
            .try_fold(0_u64, |sum, file| sum.checked_add(file.length))
            .ok_or_else(|| NativeError::Failed("Boot size overflow".into()))?;
        let plan = partition_plan(
            context.host,
            &target,
            boot_bytes,
            context.config.flag("CAN_INSTALL_ON_SAME_DRIVE")?,
        )?;
        for (index, step) in plan.into_iter().enumerate() {
            context.canceled(cancelled)?;
            guard_target(context, &target, index == 0)?;
            context.tool(Path::new(&step.program), &step.arguments, None, true)?;
        }
        let (part1, part2) = partition_paths(context.host, &target.path)?;
        for attempt in 0..150 {
            if RealDeviceSystem.node(&part1).is_ok() && RealDeviceSystem.node(&part2).is_ok() {
                break;
            }
            context.canceled(cancelled)?;
            if attempt == 149 {
                return Err(NativeError::Failed("Partition nodes did not appear".into()));
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        guard_target(context, &target, false)?;
        if context.host == Host::Macos {
            unmount_device(context, &target)?;
            context.tool(
                Path::new("/usr/sbin/diskutil"),
                &[
                    OsString::from("eraseVolume"),
                    OsString::from("MS-DOS"),
                    OsString::from("WOR_BOOT"),
                    part1.as_os_str().into(),
                ],
                None,
                true,
            )?;
            guard_target(context, &target, false)?;
            context.tool(
                Path::new("/usr/sbin/diskutil"),
                &[
                    OsString::from("eraseVolume"),
                    OsString::from("ExFAT"),
                    OsString::from("WOR_INSTALL"),
                    part2.as_os_str().into(),
                ],
                None,
                true,
            )?;
        } else {
            context.tool(
                Path::new("mkfs.fat"),
                &[
                    OsString::from("-F"),
                    OsString::from("32"),
                    OsString::from("-n"),
                    OsString::from("WOR_BOOT"),
                    part1.as_os_str().into(),
                ],
                None,
                true,
            )?;
            guard_target(context, &target, false)?;
            context.tool(
                Path::new("mkfs.exfat"),
                &[
                    OsString::from("-n"),
                    OsString::from("WOR_INSTALL"),
                    part2.as_os_str().into(),
                ],
                None,
                true,
            )?;
        }
        let (mut mounts, boot, mut install) = mount(context, &part1, &part2)?;
        let operation = (|| -> Result<(), NativeError> {
            context.emit(Progress::Step {
                current: 3,
                total: 5,
                message: "Copy the frozen prepared image".into(),
            })?;
            copy_manifest(&prepared.boot_dir, &boot, &boot_manifest, cancelled)?;
            context.tool(
                Path::new("wiminfo"),
                &[
                    boot.join("sources/boot.wim").into_os_string(),
                    OsString::from("2"),
                    OsString::from("--xml"),
                ],
                None,
                false,
            )?;
            let image_manifest = BTreeMap::from([(PathBuf::from("install.wim"), image.clone())]);
            let source = prepared
                .install_wim
                .parent()
                .ok_or_else(|| NativeError::Failed("Missing image source directory".into()))?;
            if prepared.install_wim.file_name() != Some(std::ffi::OsStr::new("install.wim")) {
                return Err(NativeError::Failed(
                    "Prepared install image must use its canonical filename".into(),
                ));
            }
            for attempt in 0..3 {
                match copy_manifest(source, &install, &image_manifest, cancelled) {
                    Ok(()) => break,
                    Err(_) if context.host == Host::Macos && !install.is_dir() && attempt < 2 => {
                        context.emit(Progress::Status(
                            "The installation volume disappeared; remounting the same partition"
                                .into(),
                        ))?;
                        context.tool(
                            Path::new("/usr/sbin/diskutil"),
                            &[OsString::from("mount"), part2.as_os_str().into()],
                            None,
                            false,
                        )?;
                        install = mounted_path(context, &part2)?;
                    }
                    Err(error) => return Err(error),
                }
            }
            copy_manifest(&prepared.install_extras, &install, &extras, cancelled)?;
            context.emit(Progress::Step {
                current: 4,
                total: 5,
                message: "Verify partition roles and written image".into(),
            })?;
            let layout = if context.config.flag("SKIP_IMAGE_VERIFICATION")? {
                println!("Written-image verification: Not Assessed (explicitly skipped).");
                geometry(context, &target, &part1, &part2)?
            } else {
                verify(
                    context,
                    &target,
                    (&part1, &part2),
                    (&boot, &install),
                    VerificationInputs {
                        prepared: &prepared,
                        boot: &boot_manifest,
                        extras: &extras,
                        image: &image,
                    },
                    cancelled,
                )?
            };
            if context.config.model()? == 3 {
                guard_target(context, &target, false)?;
                let job = BootstrapJob {
                    target: target.clone(),
                    protected_paths: context.protected_paths(Some(&prepared.install_wim)),
                    offsets: layout,
                };
                let mut file = tempfile::NamedTempFile::new()?;
                serde_json::to_writer(&mut file, &job)?;
                file.flush()?;
                context.helper(
                    &[
                        OsString::from("host-bootstrap"),
                        OsString::from("--job-file"),
                        file.path().as_os_str().into(),
                        OsString::from("--backup-dir"),
                        PathBuf::from(context.config.get("DL_DIR"))
                            .join("logs")
                            .into_os_string(),
                    ],
                    None,
                    true,
                )?;
            }
            Ok(())
        })();
        let cleanup = mounts.finish();
        if let Err(cleanup) = cleanup {
            return Err(NativeError::Cleanup(format!(
                "{}; {cleanup}",
                operation.err().map_or_else(
                    || "Target was written but cleanup is not verified".into(),
                    |error| error.to_string()
                )
            )));
        }
        operation?;
        context.canceled(cancelled)?;
        guard_target(context, &target, false)?;
        if context.host == Host::Macos {
            context.tool(
                Path::new("sgdisk"),
                &[
                    OsString::from("-t"),
                    OsString::from("1:ef00"),
                    OsString::from("-t"),
                    OsString::from("2:0700"),
                    OsString::from("-A"),
                    OsString::from("1:clear:63"),
                    OsString::from("-A"),
                    OsString::from("2:clear:63"),
                    target.raw_path.as_os_str().into(),
                ],
                None,
                true,
            )?;
            let first = info(context, &part1)?;
            let second = info(context, &part2)?;
            if first["Content"] != "EFI" || second["Content"] != "Microsoft Basic Data" {
                return Err(NativeError::Failed(
                    "Final GPT partition roles are not verified".into(),
                ));
            }
            for number in ["1", "2"] {
                let output = context.tool(
                    Path::new("sgdisk"),
                    &[
                        format!("--info={number}").into(),
                        target.raw_path.as_os_str().into(),
                    ],
                    None,
                    true,
                )?;
                let text = std::str::from_utf8(&output)
                    .map_err(|_| NativeError::Failed("Invalid final GPT attributes".into()))?;
                let attributes = text
                    .lines()
                    .find_map(|line| line.trim().strip_prefix("Attribute flags:"))
                    .and_then(|value| u64::from_str_radix(value.trim(), 16).ok())
                    .ok_or_else(|| {
                        NativeError::Failed(
                            "Final GPT visibility attributes were not assessed".into(),
                        )
                    })?;
                if attributes & (1_u64 << 63) != 0 {
                    return Err(NativeError::Failed("Final GPT attribute 63 remains set; Windows cannot assign the required drive letters".into()));
                }
            }
        }
        eject_device(context, &target)?;
        context.emit(Progress::DiskWrite {
            active: false,
            device: target.path.to_string_lossy().into_owned(),
        })?;
        Ok(())
    })();
    let source_cleanup = prepared.cleanup();
    match (prepared_run, source_cleanup) {
        (Ok(()), Ok(())) => {
            context.emit(Progress::Step {
                current: 5,
                total: 5,
                message: "Native workflow and owned cleanup completed".into(),
            })?;
            if context.config.flag("DRY_RUN")? {
                println!(
                    "Native media preparation and cleanup completed. No drive was written. Hardware and written-image verification are Not Assessed."
                );
            } else {
                println!(
                    "Native media workflow and cleanup completed. Hardware boot: Not Assessed. Written-image verification: {}.",
                    if context.config.flag("SKIP_IMAGE_VERIFICATION")? {
                        "Not Assessed (explicit skip)"
                    } else {
                        "Verified"
                    }
                );
            }
            Ok(())
        }
        (Err(error), Ok(())) => Err(error),
        (result, Err(cleanup)) => Err(NativeError::Cleanup(format!(
            "{}; {cleanup}",
            result.err().map_or_else(
                || "Source preparation cleanup failed after the target workflow".into(),
                |error| error.to_string()
            )
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::{NodeIdentity, NodeKind};
    fn disk(path: &str, host: Host) -> DeviceSnapshot {
        let node = NodeIdentity {
            device: 1,
            inode: 2,
            rdev: 3,
            kind: NodeKind::Block,
        };
        DeviceSnapshot {
            path: path.into(),
            raw_path: if host == Host::Macos {
                "/dev/rdisk9".into()
            } else {
                path.into()
            },
            bytes: 64 * 1024_u64.pow(3),
            sector_size: 512,
            layout: "gpt".into(),
            model: "fixture".into(),
            node: node.clone(),
            raw_node: node,
        }
    }
    #[test]
    fn native_partition_plans_keep_partition_one_and_unallocated_install_space() {
        let target = disk("/dev/sdz", Host::Linux);
        let plan = partition_plan(Host::Linux, &target, 1024, false).expect("plan");
        assert!(
            plan.iter()
                .any(|step| step.arguments.contains(&OsString::from("7000MB")))
        );
        assert!(
            plan.iter()
                .any(|step| step.arguments.contains(&OsString::from("esp")))
        );
        let target = disk("/dev/disk9", Host::Macos);
        let plan = partition_plan(Host::Macos, &target, 1024, true).expect("plan");
        assert!(plan[1].arguments.contains(&OsString::from("1:0:+1536M")));
        assert!(plan[1].arguments.contains(&OsString::from("2:0:+18000M")));
        assert!(plan[1].arguments.contains(&OsString::from("1:set:63")));
        assert!(
            plan[1]
                .arguments
                .contains(&target.raw_path.into_os_string())
        );
    }
    #[test]
    fn linux_partition_names_use_kernel_digit_suffix_rules() {
        assert_eq!(
            partition_paths(Host::Linux, Path::new("/dev/sdz"))
                .expect("SCSI")
                .0,
            PathBuf::from("/dev/sdz1")
        );
        assert_eq!(
            partition_paths(Host::Linux, Path::new("/dev/nvme2n1"))
                .expect("NVMe")
                .1,
            PathBuf::from("/dev/nvme2n1p2")
        );
        assert_eq!(
            partition_paths(Host::Linux, Path::new("/dev/mmcblk1"))
                .expect("MMC")
                .0,
            PathBuf::from("/dev/mmcblk1p1")
        );
        assert_eq!(
            partition_paths(Host::Macos, Path::new("/dev/disk9"))
                .expect("Mac")
                .1,
            PathBuf::from("/dev/disk9s2")
        );
    }
    #[test]
    fn payload_copy_and_readback_use_the_frozen_source_not_recomputed_trust() {
        let temp = tempfile::tempdir().expect("fixture");
        let source = temp.path().join("source");
        let target = temp.path().join("target");
        fs::create_dir_all(source.join("sources")).expect("directory");
        fs::create_dir(&target).expect("target");
        fs::write(source.join("sources/boot.wim"), "synthetic fixture only").expect("payload");
        let cancelled = AtomicBool::new(false);
        let manifest = tree_manifest(&source, &cancelled).expect("manifest");
        copy_manifest(&source, &target, &manifest, &cancelled).expect("copy");
        verify_manifest(&target, &manifest, &cancelled).expect("readback");
        fs::write(target.join("sources/boot.wim"), "corrupt").expect("tamper");
        assert!(verify_manifest(&target, &manifest, &cancelled).is_err());
        fs::write(source.join("sources/boot.wim"), "source changed").expect("source tamper");
        assert!(copy_manifest(&source, &target, &manifest, &cancelled).is_err());
    }

    #[test]
    fn payload_operations_reuse_one_buffer_across_mixed_file_sizes_without_reusing_trust() {
        let fixture = tempfile::tempdir().expect("fixture");
        let source = fixture.path().join("source");
        let target = fixture.path().join("target");
        fs::create_dir_all(source.join("nested")).expect("source");
        fs::create_dir_all(&target).expect("target");
        let files = [
            ("a-large.bin", vec![0xa5; 1024 * 1024 + 17]),
            ("b-tiny.bin", vec![0x5a; 3]),
            ("c-empty.bin", Vec::new()),
            ("nested/d-large.bin", vec![0x33; 2 * 1024 * 1024 + 31]),
            ("nested/e-short.bin", vec![0x77; 7]),
        ];
        for (name, bytes) in &files {
            fs::write(source.join(name), bytes).expect("source payload");
        }
        let cancelled = AtomicBool::new(false);
        PAYLOAD_BUFFER_ALLOCATIONS.with(|count| count.set(0));
        let manifest = tree_manifest(&source, &cancelled).expect("manifest");
        let manifest_allocations = PAYLOAD_BUFFER_ALLOCATIONS.with(std::cell::Cell::get);
        PAYLOAD_BUFFER_ALLOCATIONS.with(|count| count.set(0));
        copy_manifest(&source, &target, &manifest, &cancelled).expect("copy");
        let copy_allocations = PAYLOAD_BUFFER_ALLOCATIONS.with(std::cell::Cell::get);
        PAYLOAD_BUFFER_ALLOCATIONS.with(|count| count.set(0));
        verify_manifest(&target, &manifest, &cancelled).expect("readback");
        let verify_allocations = PAYLOAD_BUFFER_ALLOCATIONS.with(std::cell::Cell::get);
        for (name, expected) in &files {
            assert_eq!(fs::read(target.join(name)).expect("copied file"), *expected);
        }
        fs::write(target.join("b-tiny.bin"), [0xff; 3]).expect("same-size corruption");
        assert!(verify_manifest(&target, &manifest, &cancelled).is_err());
        fs::write(source.join("b-tiny.bin"), [0xfe; 3]).expect("same-size source replacement");
        assert!(copy_manifest(&source, &target, &manifest, &cancelled).is_err());
        assert_eq!(
            fs::read(target.join("b-tiny.bin")).expect("rejected copy leaves target untouched"),
            [0xff; 3]
        );
        cancelled.store(true, Ordering::Release);
        assert!(matches!(
            tree_manifest(&source, &cancelled),
            Err(NativeError::Canceled)
        ));
        assert!(matches!(
            copy_manifest(&source, &target, &manifest, &cancelled),
            Err(NativeError::Canceled)
        ));
        assert!(matches!(
            verify_manifest(&target, &manifest, &cancelled),
            Err(NativeError::Canceled)
        ));
        eprintln!(
            "Payload work for {} files: manifest={manifest_allocations}, copy={copy_allocations}, readback={verify_allocations}",
            files.len()
        );
        assert_eq!(manifest_allocations, 1);
        assert_eq!(copy_allocations, 1);
        assert_eq!(verify_allocations, 1);
    }

    #[test]
    fn empty_payload_operations_do_not_allocate_streaming_scratch() {
        let root = tempfile::tempdir().expect("empty fixture");
        let cancelled = AtomicBool::new(false);
        PAYLOAD_BUFFER_ALLOCATIONS.with(|count| count.set(0));
        let manifest = tree_manifest(root.path(), &cancelled).expect("empty source");
        assert!(manifest.is_empty());
        copy_manifest(root.path(), root.path(), &manifest, &cancelled).expect("empty copy");
        verify_manifest(root.path(), &manifest, &cancelled).expect("empty readback");
        assert_eq!(PAYLOAD_BUFFER_ALLOCATIONS.with(std::cell::Cell::get), 0);
    }
}
