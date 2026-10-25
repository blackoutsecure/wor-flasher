use crate::archive::safe_archive_path;
use crate::files::{copy_file, copy_tree, make_directory, private_file, required_file};
use crate::options::absolute_path;
use crate::{MediaError, check_cancelled};
use reqwest::Url;
use reqwest::blocking::Client;
use std::collections::BTreeSet;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::AtomicBool;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Clone)]
pub struct DownloadRequest {
    pub url: Url,
    pub component: &'static str,
    pub verify_tls: bool,
    pub maximum_bytes: u64,
    pub timeout: Duration,
}

impl fmt::Debug for DownloadRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DownloadRequest")
            .field("url", &"[redacted]")
            .field("component", &self.component)
            .field("verify_tls", &self.verify_tls)
            .field("maximum_bytes", &self.maximum_bytes)
            .field("timeout", &self.timeout)
            .finish()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WimOperation {
    Info,
    Extract,
    Export,
    Delete,
    Update,
    Verify,
}

impl WimOperation {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Extract => "extract",
            Self::Export => "export",
            Self::Delete => "delete",
            Self::Update => "update",
            Self::Verify => "verify",
        }
    }
}

/// Native provider seam: no provider discovers a target disk or writes one.
pub trait MediaProvider {
    fn fetch(
        &mut self,
        request: &DownloadRequest,
        destination: &Path,
        cancelled: &AtomicBool,
    ) -> Result<(), MediaError>;
    fn wim(
        &mut self,
        operation: WimOperation,
        args: &[OsString],
        working_dir: &Path,
        cancelled: &AtomicBool,
    ) -> Result<Vec<u8>, MediaError>;
    fn extract_iso(
        &mut self,
        image: &Path,
        destination: &Path,
        cancelled: &AtomicBool,
    ) -> Result<(), MediaError>;
    fn available_space(&mut self, path: &Path) -> Result<u64, MediaError>;

    fn file_size_limit(
        &mut self,
        _path: &Path,
        cancelled: &AtomicBool,
    ) -> Result<Option<u64>, MediaError> {
        check_cancelled(cancelled)?;
        Ok(None)
    }

    fn unix_time(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }
}

#[derive(Clone, Debug)]
pub struct NativeTools {
    pub wimlib: PathBuf,
    /// `bsdtar`/libarchive; ISO members are streamed to Rust-owned files, not
    /// extracted directly into the filesystem. UDF-only images use a private
    /// read-only native mount if libarchive cannot open them.
    pub archive: PathBuf,
    /// `hdiutil` on macOS; `sudo` on Linux, used only for fixed read-only source
    /// mount/umount operations. The parent owns authentication.
    pub mount: PathBuf,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MountStyle {
    MacOs,
    Linux,
}

impl Default for NativeTools {
    fn default() -> Self {
        Self {
            wimlib: PathBuf::from("wimlib-imagex"),
            archive: PathBuf::from("bsdtar"),
            mount: PathBuf::from(if cfg!(target_os = "macos") {
                "hdiutil"
            } else {
                "sudo"
            }),
        }
    }
}

pub struct NativeProvider {
    pub tools: NativeTools,
    verified_client: Client,
    unverified_client: Client,
    mount_style: MountStyle,
}

impl NativeProvider {
    pub fn new() -> Result<Self, MediaError> {
        Self::with_tools(NativeTools::default())
    }

    pub fn with_tools(tools: NativeTools) -> Result<Self, MediaError> {
        Ok(Self {
            tools,
            verified_client: client(true)?,
            unverified_client: client(false)?,
            mount_style: if cfg!(target_os = "macos") {
                MountStyle::MacOs
            } else {
                MountStyle::Linux
            },
        })
    }

    pub fn with_mount_style(mut self, style: MountStyle) -> Self {
        self.mount_style = style;
        self
    }

    #[allow(clippy::too_many_arguments)]
    fn command(
        &self,
        program: &Path,
        args: &[OsString],
        working_dir: &Path,
        cancelled: &AtomicBool,
        component: &'static str,
        operation: &'static str,
        output: Option<&Path>,
    ) -> Result<Vec<u8>, MediaError> {
        check_cancelled(cancelled)?;
        make_directory(working_dir)?;
        let capture = tempfile::NamedTempFile::new_in(working_dir)
            .map_err(|e| MediaError::io("create native-tool output", e))?;
        let errors = match tempfile::NamedTempFile::new_in(working_dir) {
            Ok(errors) => errors,
            Err(error) => {
                let result = Err(MediaError::io(
                    "create native-tool diagnostic capture",
                    error,
                ));
                return close_capture(capture, result);
            }
        };
        let result = (|| {
            let stdout = if let Some(path) = output {
                private_file(path)?
            } else {
                capture
                    .reopen()
                    .map_err(|e| MediaError::io("open native-tool output", e))?
            };
            // Tools inherit the worker group. Diagnostics can contain signed
            // URLs or credentials and never cross the MediaError boundary.
            let mut child = Command::new(program)
                .args(args)
                .current_dir(working_dir)
                .env("LC_ALL", "C")
                .env("TMPDIR", working_dir)
                .env("TMP", working_dir)
                .env("TEMP", working_dir)
                .stdin(Stdio::null())
                .stdout(Stdio::from(stdout))
                .stderr(Stdio::from(
                    errors
                        .reopen()
                        .map_err(|e| MediaError::io("open diagnostic capture", e))?,
                ))
                .spawn()
                .map_err(|_| MediaError::Tool {
                    tool: component,
                    operation,
                    status: None,
                })?;
            let output_path = output.unwrap_or(capture.path());
            let limit = if output.is_some() {
                32 * 1024 * 1024 * 1024
            } else {
                256 * 1024 * 1024
            };
            let status = loop {
                let guard = (|| {
                    check_cancelled(cancelled)?;
                    let length = fs::metadata(output_path)
                        .map_err(|e| MediaError::io("measure native-tool output", e))?
                        .len();
                    let diagnostics = errors
                        .as_file()
                        .metadata()
                        .map_err(|e| MediaError::io("measure native-tool diagnostics", e))?
                        .len();
                    if length > limit || diagnostics > 16 * 1024 * 1024 {
                        return Err(MediaError::InvalidMetadata("native-tool output limit"));
                    }
                    Ok(())
                })();
                if let Err(error) = guard {
                    stop_child(&mut child)?;
                    return Err(error);
                }
                match child.try_wait() {
                    Ok(Some(status)) => break status,
                    Ok(None) => std::thread::sleep(Duration::from_millis(30)),
                    Err(error) => {
                        stop_child(&mut child)?;
                        return Err(MediaError::io("wait for native image tool", error));
                    }
                }
            };
            if !status.success() {
                return Err(MediaError::Tool {
                    tool: component,
                    operation,
                    status: status.code(),
                });
            }
            check_cancelled(cancelled)?;
            if output.is_some() {
                return Ok(Vec::new());
            }
            if capture
                .as_file()
                .metadata()
                .map_err(|e| MediaError::io("measure image-tool output", e))?
                .len()
                > limit
            {
                return Err(MediaError::InvalidMetadata("native-tool output limit"));
            }
            fs::read(capture.path()).map_err(|e| MediaError::io("read image-tool output", e))
        })();
        let result = close_capture(capture, result);
        close_capture(errors, result)
    }

    fn archive_iso(
        &self,
        image: &Path,
        destination: &Path,
        cancelled: &AtomicBool,
    ) -> Result<(), MediaError> {
        let work = destination
            .parent()
            .ok_or(MediaError::InvalidMetadata("ISO staging parent"))?;
        let names = self.command(
            &self.tools.archive,
            &["-tf".into(), image.as_os_str().into()],
            work,
            cancelled,
            "libarchive",
            "list ISO",
            None,
        )?;
        let details = self.command(
            &self.tools.archive,
            &["-tvf".into(), image.as_os_str().into()],
            work,
            cancelled,
            "libarchive",
            "inspect ISO entry types",
            None,
        )?;
        let names = std::str::from_utf8(&names)
            .map_err(|_| MediaError::InvalidMetadata("ISO entry names"))?;
        let details = std::str::from_utf8(&details)
            .map_err(|_| MediaError::InvalidMetadata("ISO entry types"))?;
        let types: Vec<_> = details
            .lines()
            .map(|line| line.as_bytes().first().copied())
            .collect();
        let names: Vec<_> = names.lines().collect();
        if names.len() != types.len() || names.is_empty() || names.len() > 300_000 {
            return Err(MediaError::InvalidMetadata("ISO directory listing"));
        }
        let mut seen = BTreeSet::new();
        let mut members = Vec::new();
        for (name, kind) in names.into_iter().zip(types) {
            if name == "." || name == "./" {
                continue;
            }
            if !matches!(kind, Some(b'-' | b'd')) || name.contains(['*', '?', '[', ']', '\\']) {
                return Err(MediaError::InvalidMetadata("unsafe ISO entry"));
            }
            let path = safe_archive_path(name)?;
            let folded = path.to_string_lossy().to_lowercase();
            if !seen.insert(folded.clone()) {
                return Err(MediaError::InvalidMetadata("case-ambiguous ISO entry"));
            }
            if kind == Some(b'-')
                && (folded.starts_with("boot/")
                    || folded.starts_with("efi/")
                    || matches!(
                        folded.as_str(),
                        "sources/boot.wim" | "sources/install.wim" | "sources/install.esd"
                    ))
            {
                members.push((name, PathBuf::from(folded)));
            }
        }
        if members.is_empty() {
            return Err(MediaError::InvalidMetadata("ISO Windows startup payload"));
        }
        make_directory(destination)?;
        let mut total = 0u64;
        for (member, relative) in members {
            check_cancelled(cancelled)?;
            let output = destination.join(relative);
            make_directory(
                output
                    .parent()
                    .ok_or(MediaError::InvalidMetadata("ISO member parent"))?,
            )?;
            self.command(
                &self.tools.archive,
                &[
                    "-xOf".into(),
                    image.as_os_str().into(),
                    "--".into(),
                    member.into(),
                ],
                work,
                cancelled,
                "libarchive",
                "read ISO member",
                Some(&output),
            )?;
            let length = output
                .metadata()
                .map_err(|e| MediaError::io("measure ISO member", e))?
                .len();
            total = total
                .checked_add(length)
                .ok_or(MediaError::InvalidMetadata("ISO size overflow"))?;
            if length > 32 * 1024 * 1024 * 1024 || total > 64 * 1024 * 1024 * 1024 {
                return Err(MediaError::InvalidMetadata("ISO expansion limit"));
            }
        }
        Ok(())
    }

    fn mounted_iso(
        &self,
        image: &Path,
        destination: &Path,
        cancelled: &AtomicBool,
    ) -> Result<(), MediaError> {
        let work = destination
            .parent()
            .ok_or(MediaError::InvalidMetadata("ISO mount parent"))?;
        check_cancelled(cancelled)?;
        let mount = crate::files::private_tempdir(work, ".iso-mount-")?;
        let mount_path = mount.path().to_path_buf();
        let not_cancelled = AtomicBool::new(false);
        let mounted = if self.mount_style == MountStyle::MacOs {
            self.command(
                &self.tools.mount,
                &[
                    "attach".into(),
                    "-nobrowse".into(),
                    "-readonly".into(),
                    "-mountpoint".into(),
                    mount_path.as_os_str().into(),
                    image.as_os_str().into(),
                ],
                work,
                &not_cancelled,
                "hdiutil",
                "mount read-only source ISO",
                None,
            )
        } else {
            let args: Vec<OsString> = vec![
                "-n".into(),
                "mount".into(),
                "-o".into(),
                "loop,ro,nosuid,nodev,noexec".into(),
                "--".into(),
                image.as_os_str().into(),
                mount_path.as_os_str().into(),
            ];
            let first = self.command(
                &self.tools.mount,
                &args,
                work,
                &not_cancelled,
                "mount",
                "mount read-only source ISO",
                None,
            );
            if matches!(first, Err(MediaError::Tool { .. })) && check_cancelled(cancelled).is_ok() {
                let _ = self.command(
                    &self.tools.mount,
                    &["-n".into(), "modprobe".into(), "udf".into()],
                    work,
                    &not_cancelled,
                    "modprobe",
                    "load UDF source-image support",
                    None,
                );
                self.command(
                    &self.tools.mount,
                    &args,
                    work,
                    &not_cancelled,
                    "mount",
                    "mount read-only source ISO",
                    None,
                )
            } else {
                first
            }
        };
        let result = match (check_cancelled(cancelled), mounted) {
            (Err(error), _) => Err(error),
            (Ok(()), Ok(_)) => copy_iso_payload(&mount_path, destination, cancelled),
            (Ok(()), Err(error)) => Err(error),
        };
        let detached = if self.mount_style == MountStyle::MacOs {
            self.command(
                &self.tools.mount,
                &["detach".into(), mount_path.as_os_str().into()],
                work,
                &not_cancelled,
                "hdiutil",
                "detach private source ISO",
                None,
            )
        } else {
            self.command(
                &self.tools.mount,
                &[
                    "-n".into(),
                    "umount".into(),
                    "--".into(),
                    mount_path.as_os_str().into(),
                ],
                work,
                &not_cancelled,
                "umount",
                "unmount private source ISO",
                None,
            )
        };
        if detached.is_err() {
            // Do not let TempDir recursively remove a possibly still-mounted
            // source. Preserve the mountpoint for explicit operator cleanup.
            let _retained_mount = mount.keep();
            return Err(MediaError::Cleanup {
                primary: result.err().map(Box::new),
                source: std::io::Error::other(format!(
                    "native source-ISO unmount failed; private mount retained at {}",
                    mount_path.display(),
                )),
            });
        }
        match mount.close() {
            Ok(()) => result,
            Err(source) => Err(MediaError::Cleanup {
                primary: result.err().map(Box::new),
                source,
            }),
        }
    }
}

fn client(verify_tls: bool) -> Result<Client, MediaError> {
    Client::builder()
        .user_agent("WoR-Flasher native desktop media preparation")
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(6 * 60 * 60))
        .danger_accept_invalid_certs(!verify_tls)
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() >= 10
                || attempt.url().scheme() != "https"
                || !attempt.url().username().is_empty()
                || attempt.url().password().is_some()
            {
                attempt.error("unsafe download redirect")
            } else {
                attempt.follow()
            }
        }))
        .build()
        .map_err(|_| MediaError::Download {
            component: "HTTP client",
            reason: "TLS client initialization failed",
        })
}

fn stop_child(child: &mut Child) -> Result<(), MediaError> {
    if child.try_wait().is_ok_and(|status| status.is_some()) {
        return Ok(());
    }
    if let Err(error) = child.kill()
        && !child.try_wait().is_ok_and(|status| status.is_some())
    {
        return Err(MediaError::io("stop owned native image tool", error));
    }
    child
        .wait()
        .map_err(|e| MediaError::io("reap owned native image tool", e))?;
    Ok(())
}

fn close_capture<T>(
    file: tempfile::NamedTempFile,
    result: Result<T, MediaError>,
) -> Result<T, MediaError> {
    match file.close() {
        Ok(()) => result,
        Err(source) => Err(MediaError::Cleanup {
            primary: result.err().map(Box::new),
            source,
        }),
    }
}

pub(crate) fn safe_url(text: &str) -> Result<Url, MediaError> {
    let url = Url::parse(text).map_err(|_| MediaError::InvalidMetadata("download URL"))?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(MediaError::InvalidMetadata("HTTPS download URL"));
    }
    Ok(url)
}

impl MediaProvider for NativeProvider {
    fn fetch(
        &mut self,
        request: &DownloadRequest,
        destination: &Path,
        cancelled: &AtomicBool,
    ) -> Result<(), MediaError> {
        check_cancelled(cancelled)?;
        safe_url(request.url.as_str())?;
        let client = if request.verify_tls {
            &self.verified_client
        } else {
            &self.unverified_client
        };
        let mut response = client
            .get(request.url.clone())
            .timeout(request.timeout)
            .header(reqwest::header::ACCEPT, "application/vnd.github+json")
            .send()
            .and_then(reqwest::blocking::Response::error_for_status)
            .map_err(|_| MediaError::Download {
                component: request.component,
                reason: "HTTPS request failed",
            })?;
        if response
            .content_length()
            .is_some_and(|length| length > request.maximum_bytes)
        {
            return Err(MediaError::Download {
                component: request.component,
                reason: "payload exceeds its size limit",
            });
        }
        let mut file = private_file(destination)?;
        let mut buffer = vec![0u8; 1024 * 1024];
        let mut total = 0u64;
        loop {
            check_cancelled(cancelled)?;
            let count = response
                .read(&mut buffer)
                .map_err(|_| MediaError::Download {
                    component: request.component,
                    reason: "HTTPS transfer interrupted",
                })?;
            if count == 0 {
                break;
            }
            total = total
                .checked_add(count as u64)
                .ok_or(MediaError::InvalidMetadata("download size overflow"))?;
            if total > request.maximum_bytes {
                return Err(MediaError::Download {
                    component: request.component,
                    reason: "payload exceeds its size limit",
                });
            }
            file.write_all(&buffer[..count])
                .map_err(|e| MediaError::io("save downloaded payload", e))?;
        }
        if total == 0 {
            return Err(MediaError::Download {
                component: request.component,
                reason: "empty response",
            });
        }
        file.sync_all()
            .map_err(|e| MediaError::io("flush downloaded payload", e))
    }

    fn wim(
        &mut self,
        operation: WimOperation,
        args: &[OsString],
        working_dir: &Path,
        cancelled: &AtomicBool,
    ) -> Result<Vec<u8>, MediaError> {
        let mut argv = vec![OsString::from(operation.name())];
        argv.extend_from_slice(args);
        self.command(
            &self.tools.wimlib,
            &argv,
            working_dir,
            cancelled,
            "wimlib-imagex",
            operation.name(),
            None,
        )
    }

    fn extract_iso(
        &mut self,
        image: &Path,
        destination: &Path,
        cancelled: &AtomicBool,
    ) -> Result<(), MediaError> {
        let image = absolute_path(image)?;
        required_file(&image, "ISO source")?;
        let mut input = File::open(&image).map_err(|e| MediaError::io("open ISO source", e))?;
        let mut recognized = false;
        for sector in 16..32 {
            check_cancelled(cancelled)?;
            input
                .seek(SeekFrom::Start(sector * 2048 + 1))
                .map_err(|e| MediaError::io("inspect ISO volume descriptor", e))?;
            let mut signature = [0u8; 5];
            if input.read_exact(&mut signature).is_err() {
                break;
            }
            if matches!(&signature, b"CD001" | b"BEA01" | b"NSR02" | b"NSR03") {
                recognized = true;
                break;
            }
        }
        if !recognized {
            return Err(MediaError::InvalidMetadata("ISO9660/UDF volume descriptor"));
        }
        match self.archive_iso(&image, destination, cancelled) {
            Err(MediaError::Tool { .. }) => {
                if destination.exists() {
                    fs::remove_dir_all(destination)
                        .map_err(|e| MediaError::io("discard incomplete ISO extraction", e))?;
                }
                self.mounted_iso(&image, destination, cancelled)
            }
            result => result,
        }
    }

    fn available_space(&mut self, path: &Path) -> Result<u64, MediaError> {
        fs2::available_space(path).map_err(|e| MediaError::io("measure free preparation space", e))
    }

    fn file_size_limit(
        &mut self,
        path: &Path,
        cancelled: &AtomicBool,
    ) -> Result<Option<u64>, MediaError> {
        check_cancelled(cancelled)?;
        #[cfg(target_os = "linux")]
        {
            let result = self.command(
                Path::new("df"),
                &["-PT".into(), "--".into(), path.as_os_str().into()],
                path,
                cancelled,
                "df",
                "inspect download filesystem",
                None,
            );
            match result {
                Ok(bytes) => {
                    let kind = std::str::from_utf8(&bytes)
                        .ok()
                        .and_then(|text| text.lines().nth(1))
                        .and_then(|line| line.split_whitespace().nth(1))
                        .unwrap_or("");
                    Ok(matches!(
                        kind,
                        "vfat" | "msdos" | "msdosfs" | "fat" | "fat16" | "fat32"
                    )
                    .then_some(4 * 1024 * 1024 * 1024 - 1))
                }
                Err(MediaError::Tool { .. }) => Ok(None),
                Err(error) => Err(error),
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = path;
            Ok(None)
        }
    }
}

pub(crate) fn find_case_insensitive(root: &Path, relative: &str) -> Result<PathBuf, MediaError> {
    let mut current = root.to_path_buf();
    for part in relative.split('/') {
        let mut matches = fs::read_dir(&current)
            .map_err(|e| MediaError::io("inspect media payload", e))?
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_str()
                    .is_some_and(|n| n.eq_ignore_ascii_case(part))
            })
            .map(|entry| entry.path());
        let next = matches.next().ok_or_else(|| MediaError::MissingRequired {
            component: "Windows media payload",
            path: root.join(relative),
        })?;
        if matches.next().is_some() {
            return Err(MediaError::InvalidMetadata("case-ambiguous media payload"));
        }
        if fs::symlink_metadata(&next)
            .map_err(|e| MediaError::io("inspect media path component", e))?
            .file_type()
            .is_symlink()
        {
            return Err(MediaError::InvalidMetadata(
                "symlink in required media path",
            ));
        }
        current = next;
    }
    Ok(current)
}

pub(crate) fn copy_iso_payload(
    source: &Path,
    destination: &Path,
    cancelled: &AtomicBool,
) -> Result<(), MediaError> {
    make_directory(destination)?;
    for directory in ["boot", "efi"] {
        copy_tree(
            &find_case_insensitive(source, directory)?,
            &destination.join(directory),
            cancelled,
        )?;
    }
    make_directory(&destination.join("sources"))?;
    copy_file(
        &find_case_insensitive(source, "sources/boot.wim")?,
        &destination.join("sources/boot.wim"),
        cancelled,
    )?;
    let image = find_case_insensitive(source, "sources/install.wim")
        .or_else(|_| find_case_insensitive(source, "sources/install.esd"))?;
    required_file(&image, "ISO installation image")?;
    let extension = image
        .extension()
        .and_then(OsStr::to_str)
        .unwrap_or("wim")
        .to_ascii_lowercase();
    copy_file(
        &image,
        &destination.join(format!("sources/install.{extension}")),
        cancelled,
    )?;
    Ok(())
}
