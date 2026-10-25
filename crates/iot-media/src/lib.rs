//! Native, digest-pinned IoT media acquisition. No disk access or Windows installer execution.
//!
//! The ISO/MSI is an opaque reviewed wrapper. Only its bounded CFB cabinet chain is
//! reconstructed; CAB decompression is in Rust and never materializes member paths.

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
compile_error!("native IoT acquisition supports Linux and macOS only");

mod archive;
mod fs;
mod network;
mod verifier;

pub use network::{DownloadProvider, MicrosoftHttps};
pub use verifier::{FfuVerifier, NativeFfuVerifier};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

pub type Result<T> = std::result::Result<T, MediaError>;
const PROVENANCE: &[u8] = include_bytes!("../../../src/config/iot-core-images.json");
const PROVENANCE_SHA256: &str = "74e9324c77e3e560a34742de1cd13e6c234789bf5089a75ba334d80c098ca41a";
const MAX_PACKAGE: u64 = 1024 * 1024 * 1024;
const MAX_FFU: u64 = 2 * 1024 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum MediaError {
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    Acquisition(String),
    #[error("IoT media acquisition encountered an I/O error")]
    Io(#[from] std::io::Error),
    #[error("IoT media acquisition interrupted")]
    Cancelled,
}

impl MediaError {
    pub fn exit_status(&self) -> i32 {
        match self {
            Self::Invalid(_) => 2,
            Self::Cancelled => 130,
            _ => 4,
        }
    }
    pub(crate) fn invalid(message: impl Into<String>) -> Self {
        Self::Invalid(message.into())
    }
    pub(crate) fn acquisition(message: impl Into<String>) -> Self {
        Self::Acquisition(message.into())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Package {
    pub size: u64,
    pub sha256: String,
}

#[derive(Clone, Debug)]
pub struct OfficialImage {
    iso: Package,
    msi: Package,
    cabinet: Package,
    msi_offset: u64,
    cabinet_sector: u32,
    url: String,
    member: String,
    ffu: Package,
}

impl OfficialImage {
    pub fn reviewed() -> Result<Self> {
        if format!("{:x}", Sha256::digest(PROVENANCE)) != PROVENANCE_SHA256 {
            return Err(MediaError::invalid(
                "bundled IoT provenance manifest is missing, modified, or untrusted",
            ));
        }
        Self::from_manifest(PROVENANCE)
    }

    fn from_manifest(raw: &[u8]) -> Result<Self> {
        let manifest: Value = serde_json::from_slice(raw)
            .map_err(|_| MediaError::invalid("invalid bundled IoT provenance"))?;
        let images = manifest
            .get("images")
            .and_then(Value::as_array)
            .filter(|images| images.len() == 1 && manifest["schema_version"] == 1)
            .ok_or_else(|| {
                MediaError::invalid("acquisition requires the single reviewed bundled image")
            })?;
        let entry = &images[0];
        let number = |key: &str, max: u64| -> Result<u64> {
            entry[key]
                .as_u64()
                .filter(|number| *number > 0 && *number <= max)
                .ok_or_else(|| {
                    MediaError::invalid(format!("invalid bounded {key} in bundled provenance"))
                })
        };
        let digest = |key: &str| -> Result<String> {
            entry[key]
                .as_str()
                .filter(|text| {
                    text.len() == 64
                        && text
                            .bytes()
                            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
                })
                .map(str::to_owned)
                .ok_or_else(|| {
                    MediaError::invalid(format!("missing or invalid {key} in bundled provenance"))
                })
        };
        let package = |size, hash, max| -> Result<Package> {
            Ok(Package {
                size: number(size, max)?,
                sha256: digest(hash)?,
            })
        };
        let url = entry["resolved_source_url"]
            .as_str()
            .ok_or_else(|| MediaError::invalid("missing official download URL"))?;
        network::check_url(url)?;
        let member = entry["source_cab_member"]
            .as_str()
            .ok_or_else(|| MediaError::invalid("missing exact cabinet member"))?;
        if member.is_empty()
            || member.len() > 128
            || !member.as_bytes()[0].is_ascii_alphanumeric()
            || !member
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"_.-".contains(&c))
        {
            return Err(MediaError::invalid(
                "bundled cabinet member must be an exact, non-pattern filename",
            ));
        }
        if entry["format"] != "ffu-v1"
            || entry["profile"] != "iot-core"
            || entry["architecture"] != "arm32"
        {
            return Err(MediaError::invalid(
                "only approved ARM32 Windows 10 IoT Core V1 images are supported",
            ));
        }
        Ok(Self {
            iso: package("source_bytes", "source_sha256", MAX_PACKAGE)?,
            msi: package("source_msi_bytes", "source_msi_sha256", MAX_PACKAGE)?,
            cabinet: package("source_cab_bytes", "source_cab_sha256", MAX_PACKAGE)?,
            msi_offset: number("source_msi_offset", MAX_PACKAGE)?,
            cabinet_sector: number("source_cab_start_sector", MAX_PACKAGE / 4096)? as u32,
            url: url.to_owned(),
            member: member.to_owned(),
            ffu: package("ffu_bytes", "sha256", MAX_FFU)?,
        })
    }

    pub fn cache_name(&self) -> String {
        format!("iot-core-{}.ffu", self.ffu.sha256)
    }
    pub fn ffu_sha256(&self) -> &str {
        &self.ffu.sha256
    }
    pub fn download_url(&self) -> &str {
        &self.url
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Inspection {
    pub format: String,
    pub architecture: String,
    pub profile: String,
    pub sha256: String,
    pub file_bytes: u64,
    pub trust: String,
    #[serde(flatten)]
    pub details: BTreeMap<String, Value>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Acquisition {
    LocalFfu,
    LocalIso,
    LocalMsi,
    Download,
    Cache,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AcquiredMedia {
    #[serde(flatten)]
    pub inspection: Inspection,
    pub source_file: PathBuf,
    pub acquisition: Acquisition,
}

#[derive(Clone, Debug)]
pub struct MediaOptions {
    pub cache_dir: PathBuf,
    /// Explicit native CLI; the default is WOR_IOT_NATIVE_COMPANION or current_exe().
    pub companion_executable: PathBuf,
}

impl MediaOptions {
    pub fn new(cache_dir: impl Into<PathBuf>) -> Result<Self> {
        Ok(Self {
            cache_dir: cache_dir.into(),
            companion_executable: std::env::var_os("WOR_IOT_NATIVE_COMPANION")
                .map(PathBuf::from)
                .map(Ok)
                .unwrap_or_else(std::env::current_exe)?,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Progress {
    pub label: &'static str,
    pub percent: u8,
}

pub(crate) fn check_cancel(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::SeqCst) {
        Err(MediaError::Cancelled)
    } else {
        Ok(())
    }
}

fn report(
    progress: &mut dyn FnMut(Progress),
    label: &'static str,
    count: u64,
    size: u64,
    complete: bool,
) {
    progress(Progress {
        label,
        percent: if complete {
            100
        } else {
            (count.saturating_mul(100) / size).min(99) as u8
        },
    });
}

pub(crate) fn copy_verified(
    source: &mut dyn Read,
    target: &mut File,
    package: &Package,
    label: &'static str,
    progress: &mut dyn FnMut(Progress),
    cancel: &AtomicBool,
    timeout: Duration,
) -> Result<()> {
    let started = Instant::now();
    let mut hash = Sha256::new();
    let mut total = 0;
    let mut previous = 0;
    let mut buffer = vec![0; 1024 * 1024];
    report(progress, label, 0, package.size, false);
    loop {
        check_cancel(cancel)?;
        if started.elapsed() >= timeout {
            return Err(MediaError::acquisition(format!("{label} timed out")));
        }
        let limit = buffer.len().min((package.size - total + 1) as usize);
        let count = source.read(&mut buffer[..limit])?;
        if count == 0 {
            break;
        }
        total += count as u64;
        if total > package.size {
            return Err(MediaError::invalid("package exceeds its pinned size"));
        }
        hash.update(&buffer[..count]);
        target.write_all(&buffer[..count])?;
        let percent = total.saturating_mul(100) / package.size;
        if percent / 5 != previous / 5 {
            report(progress, label, total, package.size, false);
            previous = percent;
        }
    }
    if total != package.size {
        return Err(MediaError::invalid(
            "package is truncated or has the wrong pinned size",
        ));
    }
    if format!("{:x}", hash.finalize()) != package.sha256 {
        return Err(MediaError::invalid(
            "package SHA256 does not match bundled official provenance",
        ));
    }
    check_cancel(cancel)?;
    target.flush()?;
    target.sync_all()?;
    fs::readonly(target)?;
    target.seek(SeekFrom::Start(0))?;
    report(progress, label, total, package.size, true);
    check_cancel(cancel)
}

fn hash_file(file: &mut File, package: &Package, cancel: &AtomicBool) -> Result<()> {
    if file.metadata()?.len() != package.size {
        return Err(MediaError::invalid("file has the wrong pinned size"));
    }
    file.seek(SeekFrom::Start(0))?;
    let mut hash = Sha256::new();
    let mut buffer = vec![0; 1024 * 1024];
    let mut total = 0;
    loop {
        check_cancel(cancel)?;
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        total += count as u64;
        if total > package.size {
            return Err(MediaError::invalid("file exceeds its pinned size"));
        }
        hash.update(&buffer[..count]);
    }
    if total != package.size || format!("{:x}", hash.finalize()) != package.sha256 {
        return Err(MediaError::invalid(
            "file SHA256 does not match bundled official provenance",
        ));
    }
    file.seek(SeekFrom::Start(0))?;
    Ok(())
}

/// Service providers are injectable, but the production image allowlist is not.
pub struct MediaService<'a> {
    pub verifier: &'a dyn FfuVerifier,
    pub download_provider: &'a dyn DownloadProvider,
}

impl MediaService<'_> {
    fn inspect(
        &self,
        directory: &fs::Directory,
        name: &OsStr,
        private: bool,
        image: &OfficialImage,
        progress: &mut dyn FnMut(Progress),
        cancel: &AtomicBool,
    ) -> Result<Inspection> {
        let mut source = directory.source(name, private)?;
        report(
            progress,
            "Validating complete approved FFU",
            0,
            image.ffu.size,
            false,
        );
        hash_file(&mut source.file, &image.ffu, cancel)?;
        let info = self.verifier.inspect(&directory.path.join(name), cancel)?;
        source.check()?;
        if info.sha256 != image.ffu.sha256
            || info.file_bytes != image.ffu.size
            || info.architecture != "arm32"
            || info.format != "ffu-v1"
            || info.profile != "iot-core"
            || info.trust != "pinned-official-sha256"
        {
            return Err(MediaError::invalid(
                "FFU does not match this acquisition's pinned image",
            ));
        }
        report(
            progress,
            "Validating complete approved FFU",
            image.ffu.size,
            image.ffu.size,
            true,
        );
        check_cancel(cancel)?;
        Ok(info)
    }

    fn cached(
        &self,
        cache: &fs::Directory,
        image: &OfficialImage,
        progress: &mut dyn FnMut(Progress),
        cancel: &AtomicBool,
    ) -> Result<Option<AcquiredMedia>> {
        let name = image.cache_name();
        if !cache.exists(&name)? {
            return Ok(None);
        }
        let inspection = self
            .inspect(cache, OsStr::new(&name), true, image, progress, cancel)
            .map_err(|error| match error {
                MediaError::Invalid(_) => MediaError::invalid(
                    "cached FFU failed verification; remove that cache entry and retry",
                ),
                other => other,
            })?;
        Ok(Some(AcquiredMedia {
            inspection,
            source_file: cache.path.join(name),
            acquisition: Acquisition::Cache,
        }))
    }

    fn publish(
        &self,
        cache: &fs::Directory,
        work: &fs::Directory,
        image: &OfficialImage,
        acquisition: Acquisition,
        progress: &mut dyn FnMut(Progress),
        cancel: &AtomicBool,
    ) -> Result<AcquiredMedia> {
        check_cancel(cancel)?;
        cache.check()?;
        work.check()?;
        let staged = work.source(OsStr::new("image.ffu"), true)?;
        let stamp = fs::Stamp::of(&staged.file)?;
        let name = image.cache_name();
        match work.link_to("image.ffu", cache, &name) {
            Err(MediaError::Io(error)) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                return self.cached(cache, image, progress, cancel)?.ok_or_else(|| {
                    MediaError::acquisition("cache entry changed during atomic publication")
                });
            }
            Err(error) => return Err(error),
            Ok(()) => {}
        }
        let result = (|| {
            work.unlink("image.ffu")?;
            cache.file.sync_all()?;
            let inspection =
                self.inspect(cache, OsStr::new(&name), true, image, progress, cancel)?;
            cache.check()?;
            check_cancel(cancel)?;
            Ok(AcquiredMedia {
                inspection,
                source_file: cache.path.join(&name),
                acquisition,
            })
        })();
        if result.is_err() && cache.exists(&name)? {
            // Never remove a competing publisher's inode, even during rollback.
            let current = cache.source(OsStr::new(&name), false)?;
            if stamp.same_inode(&fs::Stamp::of(&current.file)?) {
                cache.unlink(&name)?;
                cache.file.sync_all()?;
            }
        }
        result
    }

    fn acquire(
        &self,
        cache: &fs::Directory,
        image: &OfficialImage,
        mut source: Option<&mut fs::Source<'_>>,
        acquisition: Acquisition,
        progress: &mut dyn FnMut(Progress),
        cancel: &AtomicBool,
    ) -> Result<AcquiredMedia> {
        check_cancel(cancel)?;
        if source.is_none()
            && let Some(cached) = self.cached(cache, image, progress, cancel)?
        {
            return Ok(cached);
        }
        let package = if acquisition == Acquisition::LocalMsi {
            &image.msi
        } else {
            &image.iso
        };
        if let Some(source) = &source
            && source.file.metadata()?.len() != package.size
        {
            return Err(MediaError::invalid(
                "local package has the wrong pinned size",
            ));
        }
        let work = fs::Workspace::new(cache)?;
        let result = (|| {
            let mut snapshot = work.directory.new_file("source.package")?;
            if let Some(source) = &mut source {
                copy_verified(
                    &mut source.file,
                    &mut snapshot,
                    package,
                    "Copying and verifying official package",
                    progress,
                    cancel,
                    Duration::from_secs(1200),
                )?;
                source.check()?;
                if let Some(cached) = self.cached(cache, image, progress, cancel)? {
                    return Ok(cached);
                }
            } else {
                self.download_provider.download(
                    &image.url,
                    package,
                    &mut snapshot,
                    progress,
                    cancel,
                )?;
                hash_file(&mut snapshot, package, cancel)?;
                fs::readonly(&snapshot)?;
            }
            let stamp = fs::Stamp::of(&snapshot)?;
            let mut cabinet = work.directory.new_file("source.cab")?;
            archive::cabinet(
                &snapshot,
                image,
                acquisition == Acquisition::LocalMsi,
                &mut cabinet,
                progress,
                cancel,
            )?;
            if fs::Stamp::of(&snapshot)? != stamp {
                return Err(MediaError::acquisition(
                    "verified wrapper snapshot changed during cabinet reconstruction",
                ));
            }
            let mut target = work.directory.new_file("image.ffu")?;
            archive::extract(&mut cabinet, &mut target, image, progress, cancel)?;
            self.inspect(
                &work.directory,
                OsStr::new("image.ffu"),
                true,
                image,
                progress,
                cancel,
            )?;
            if let Some(source) = &source {
                source.check()?;
            }
            self.publish(cache, &work.directory, image, acquisition, progress, cancel)
        })();
        let cleanup = work.close();
        match (result, cleanup) {
            (_, Err(error)) => Err(error),
            (result, Ok(())) => result,
        }
    }

    pub fn prepare(
        &self,
        source: &Path,
        options: &MediaOptions,
        progress: &mut dyn FnMut(Progress),
        cancel: &AtomicBool,
    ) -> Result<AcquiredMedia> {
        self.prepare_image(
            source,
            options,
            &OfficialImage::reviewed()?,
            progress,
            cancel,
        )
    }

    fn prepare_image(
        &self,
        source: &Path,
        options: &MediaOptions,
        image: &OfficialImage,
        progress: &mut dyn FnMut(Progress),
        cancel: &AtomicBool,
    ) -> Result<AcquiredMedia> {
        let path = fs::absolute(source)?;
        let suffix = path
            .extension()
            .and_then(OsStr::to_str)
            .unwrap_or("")
            .to_ascii_lowercase();
        let parent = path
            .parent()
            .ok_or_else(|| MediaError::invalid("source must name a file"))?;
        let name = path
            .file_name()
            .ok_or_else(|| MediaError::invalid("source must name a file"))?;
        if suffix == "ffu" {
            let directory = fs::Directory::open(parent, false, false)?;
            let inspection = self.inspect(&directory, name, false, image, progress, cancel)?;
            return Ok(AcquiredMedia {
                inspection,
                source_file: path,
                acquisition: Acquisition::LocalFfu,
            });
        }
        let acquisition = match suffix.as_str() {
            "iso" => Acquisition::LocalIso,
            "msi" => Acquisition::LocalMsi,
            _ => {
                return Err(MediaError::invalid(
                    "source must be an approved local .ffu, .iso, or .msi file",
                ));
            }
        };
        let directory = fs::Directory::open(parent, false, false)?;
        let mut source = directory.source(name, false)?;
        let cache = fs::Directory::open(&fs::cache_path(&options.cache_dir)?, true, true)?;
        self.acquire(
            &cache,
            image,
            Some(&mut source),
            acquisition,
            progress,
            cancel,
        )
    }

    pub fn download(
        &self,
        options: &MediaOptions,
        progress: &mut dyn FnMut(Progress),
        cancel: &AtomicBool,
    ) -> Result<AcquiredMedia> {
        let image = OfficialImage::reviewed()?;
        let cache = fs::Directory::open(&fs::cache_path(&options.cache_dir)?, true, true)?;
        self.acquire(
            &cache,
            &image,
            None,
            Acquisition::Download,
            progress,
            cancel,
        )
    }
}

pub fn prepare(
    source: &Path,
    options: &MediaOptions,
    progress: &mut dyn FnMut(Progress),
    cancel: &AtomicBool,
) -> Result<AcquiredMedia> {
    MediaService {
        verifier: &NativeFfuVerifier::new(&options.companion_executable),
        download_provider: &MicrosoftHttps,
    }
    .prepare(source, options, progress, cancel)
}

pub fn download(
    options: &MediaOptions,
    progress: &mut dyn FnMut(Progress),
    cancel: &AtomicBool,
) -> Result<AcquiredMedia> {
    MediaService {
        verifier: &NativeFfuVerifier::new(&options.companion_executable),
        download_provider: &MicrosoftHttps,
    }
    .download(options, progress, cancel)
}

/// Legacy helper command adapter. Errors are rendered to stderr; success alone emits JSON/path.
pub fn run(args: &[OsString]) -> Result<i32> {
    let usage = "usage: iot-media prepare SOURCE --cache-dir DIR [--json] | download --cache-dir DIR [--json]";
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        writeln!(std::io::stdout().lock(), "{usage}")?;
        return Ok(0);
    }
    let Some(command) = args.first().and_then(|arg| arg.to_str()) else {
        writeln!(std::io::stderr().lock(), "{usage}")?;
        return Ok(2);
    };
    let mut index = 1;
    let source = if command == "prepare" {
        let result = args.get(index).map(PathBuf::from);
        index += 1;
        result
    } else {
        None
    };
    if command != "prepare" && command != "download" {
        writeln!(std::io::stderr().lock(), "{usage}")?;
        return Ok(2);
    }
    let mut cache = None;
    let mut json = false;
    while index < args.len() {
        match args[index].to_str() {
            Some("--cache-dir") => {
                index += 1;
                cache = args
                    .get(index)
                    .filter(|arg| arg.as_os_str() == "-" || !arg.to_string_lossy().starts_with('-'))
                    .map(PathBuf::from);
                if cache.is_none() {
                    writeln!(std::io::stderr().lock(), "{usage}")?;
                    return Ok(2);
                }
            }
            Some(value) if value.starts_with("--cache-dir=") => {
                cache = Some(PathBuf::from(&value["--cache-dir=".len()..]));
            }
            Some("--json") => json = true,
            _ => {
                writeln!(std::io::stderr().lock(), "{usage}")?;
                return Ok(2);
            }
        }
        index += 1;
    }
    let Some(cache) = cache else {
        writeln!(std::io::stderr().lock(), "{usage}")?;
        return Ok(2);
    };
    if command == "prepare" && source.is_none() {
        writeln!(std::io::stderr().lock(), "{usage}")?;
        return Ok(2);
    }
    let cancel = Arc::new(AtomicBool::new(false));
    let signal = Arc::new(AtomicUsize::new(0));
    let mut registrations = Vec::new();
    for number in [signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM] {
        registrations.push(signal_hook::flag::register(number, cancel.clone())?);
        registrations.push(signal_hook::flag::register_usize(
            number,
            signal.clone(),
            number as usize,
        )?);
    }
    let result = (|| {
        let options = MediaOptions::new(cache)?;
        let mut progress = |event: Progress| {
            let _ = writeln!(
                std::io::stderr().lock(),
                "{}: {}%",
                event.label,
                event.percent
            );
        };
        if let Some(source) = source {
            prepare(&source, &options, &mut progress, &cancel)
        } else {
            download(&options, &mut progress, &cancel)
        }
    })();
    for registration in registrations {
        signal_hook::low_level::unregister(registration);
    }
    if signal.load(Ordering::SeqCst) != 0 {
        writeln!(
            std::io::stderr().lock(),
            "IoT media error: acquisition interrupted"
        )?;
        return Ok(128 + signal.load(Ordering::SeqCst) as i32);
    }
    match result {
        Ok(result) => {
            if json {
                writeln!(
                    std::io::stdout().lock(),
                    "{}",
                    serde_json::to_string(&result)
                        .map_err(|_| MediaError::acquisition("cannot encode acquisition result"))?
                )?;
            } else {
                writeln!(std::io::stdout().lock(), "{}", result.source_file.display())?;
            }
            Ok(0)
        }
        Err(error) => {
            writeln!(std::io::stderr().lock(), "IoT media error: {error}")?;
            Ok(error.exit_status())
        }
    }
}

#[cfg(test)]
mod tests;
