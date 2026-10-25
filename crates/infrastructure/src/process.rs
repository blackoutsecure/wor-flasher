use crate::{Runtime, RuntimeError, runtime::config_bytes};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use tokio::sync::watch;
use wor_application::{EngineEvent, Outcome, Request};
use wor_domain::progress::Progress;

const CAPTURE_LIMIT: usize = 256 * 1024;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum OutputMode {
    Inherit,
    Capture,
}

#[derive(Debug, Error)]
pub enum EngineError {
    #[error(transparent)]
    Runtime(#[from] RuntimeError),
    #[error("Cannot run or observe the engine: {0}")]
    Io(#[from] std::io::Error),
    #[error("The native boot-configuration executable is missing or unsafe: {0}")]
    InvalidHelper(PathBuf),
    #[error("The engine output reader failed: {0}")]
    OutputTask(#[from] tokio::task::JoinError),
    #[error(
        "Cannot stop owned engine processes: {0}; do not remove the target until they have stopped"
    )]
    Cleanup(String),
    #[error("This Rust frontend supports Debian-family Linux and macOS only, not Windows or WSL")]
    UnsupportedHost,
    #[error("Configuration changed after preview; preview it again before continuing")]
    ChangedConfiguration,
    #[error(transparent)]
    Native(#[from] crate::native::NativeError),
    #[error("Invalid native worker message: {0}")]
    Protocol(#[from] serde_json::Error),
}

type EventSink = Arc<dyn Fn(EngineEvent) + Send + Sync>;

#[derive(Clone)]
pub struct Engine {
    runtime: Runtime,
    native_helper: PathBuf,
}

fn require_host() -> Result<(), EngineError> {
    if !cfg!(any(target_os = "linux", target_os = "macos"))
        || std::env::var_os("WSL_DISTRO_NAME").is_some()
        || std::env::var_os("WSLENV").is_some()
        || (cfg!(target_os = "linux")
            && fs::read_to_string("/proc/version")
                .is_ok_and(|v| v.to_ascii_lowercase().contains("microsoft")))
    {
        return Err(EngineError::UnsupportedHost);
    }
    Ok(())
}

impl Engine {
    pub fn new(runtime: Runtime, native_helper: PathBuf) -> Result<Self, EngineError> {
        let metadata = fs::symlink_metadata(&native_helper)
            .map_err(|_| EngineError::InvalidHelper(native_helper.clone()))?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(EngineError::InvalidHelper(native_helper));
        }
        Ok(Self {
            runtime,
            native_helper,
        })
    }

    pub async fn execute(
        &self,
        request: Request,
        mode: OutputMode,
        mut cancellation: watch::Receiver<bool>,
        events: EventSink,
    ) -> Result<Outcome, EngineError> {
        require_host()?;
        self.runtime.validate()?;
        if *cancellation.borrow() {
            return Ok(Outcome {
                exit_code: 130,
                cancelled: true,
                stdout: String::new(),
                stderr: String::new(),
            });
        }
        let private = tempfile::tempdir()?;
        let snapshot = if let Some(expected) = &request.expected_config_digest {
            let source = request
                .config
                .as_ref()
                .ok_or(EngineError::ChangedConfiguration)?;
            let bytes = config_bytes(source)?;
            if format!("{:x}", Sha256::digest(&bytes)) != *expected {
                return Err(EngineError::ChangedConfiguration);
            }
            let mut file = tempfile::NamedTempFile::new_in(private.path())?;
            file.write_all(&bytes)?;
            file.flush()?;
            Some(file)
        } else {
            None
        };
        let progress = tempfile::NamedTempFile::new_in(private.path())?;
        let observe = mode == OutputMode::Capture && request.progress_file.is_none();
        let progress_path = request
            .progress_file
            .as_deref()
            .or_else(|| observe.then_some(progress.path()));
        let mut job_request = request.clone();
        job_request.config = snapshot
            .as_ref()
            .map(|file| file.path().to_owned())
            .or(request.config.clone());
        job_request.progress_file = progress_path.map(PathBuf::from);
        let payload = serde_json::to_vec(&crate::native::NativeJob::from_request(
            &job_request,
            &self.runtime,
        )?)?;
        let mut command = Command::new(&self.native_helper);
        command
            .arg("native-worker")
            .stdin(Stdio::piped())
            .env("WOR_IOT_NATIVE_COMPANION", &self.native_helper);
        command.kill_on_drop(true);
        if mode == OutputMode::Inherit {
            command.stdout(Stdio::inherit()).stderr(Stdio::inherit());
        } else {
            command.stdout(Stdio::piped()).stderr(Stdio::piped());
        }
        #[cfg(unix)]
        command.process_group(0);
        let mut progress_reader = if observe {
            Some(progress.reopen()?)
        } else {
            None
        };
        let mut child = command.spawn()?;
        let id = child
            .id()
            .and_then(|id| i32::try_from(id).ok())
            .filter(|id| *id > 0)
            .ok_or_else(|| {
                EngineError::Cleanup("missing or invalid child process identity".into())
            })?;
        let mut group = ProcessGroup {
            id,
            armed: true,
            privileged_cleanup: request.operation.may_write(),
        };
        let stdout_task = child.stdout.take().map(|pipe| tokio::spawn(capture(pipe)));
        let stderr_task = child.stderr.take().map(|pipe| tokio::spawn(capture(pipe)));
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| EngineError::Cleanup("Missing private native worker pipe".into()))?;
        tokio::time::timeout(Duration::from_secs(5), stdin.write_all(&payload))
            .await
            .map_err(|_| {
                EngineError::Cleanup("Native worker did not accept its private request".into())
            })?
            .map_err(shutdown_error)?;
        stdin.shutdown().await.map_err(shutdown_error)?;
        drop(stdin);
        let mut pending = String::new();
        let mut interval = tokio::time::interval(Duration::from_millis(150));
        let mut cancelled = false;
        let status = loop {
            tokio::select! {
                status = child.wait() => break status.map_err(shutdown_error)?,
                () = cancellation_requested(&mut cancellation) => {
                    cancelled = true;
                    group.signal(false)?;
                    let status = match tokio::time::timeout(Duration::from_secs(5), child.wait()).await {
                        Ok(status) => status.map_err(shutdown_error)?,
                        Err(_) => {
                            group.signal(true)?;
                            let status = tokio::time::timeout(Duration::from_secs(5), child.wait())
                                .await
                                .map_err(|_| EngineError::Cleanup("engine did not stop after cancellation".into()))?;
                            status.map_err(shutdown_error)?
                        }
                    };
                    break status;
                }
                _ = interval.tick(), if progress_reader.is_some() => {
                    if let Some(reader) = &mut progress_reader
                        && let Err(error) = read_progress(reader, &mut pending, &events) {
                            events(EngineEvent::Diagnostic(format!("Progress display failed: {error}")));
                            progress_reader = None;
                        }
                }
            }
        };
        if let Some(reader) = &mut progress_reader
            && let Err(error) = read_progress(reader, &mut pending, &events)
        {
            events(EngineEvent::Diagnostic(format!(
                "Progress display failed: {error}"
            )));
        }
        if !pending.is_empty() {
            events(EngineEvent::Diagnostic(
                "The engine left an incomplete progress record.".into(),
            ));
        }
        group.stop_remaining().await?;
        group.armed = false;
        let stdout = collect(stdout_task).await?;
        let stderr = collect(stderr_task).await?;
        #[cfg(unix)]
        let exit_code = {
            use std::os::unix::process::ExitStatusExt;
            status
                .code()
                .unwrap_or_else(|| 128 + status.signal().unwrap_or(1))
        };
        #[cfg(not(unix))]
        let exit_code = status.code().unwrap_or(1);
        Ok(Outcome {
            exit_code: if cancelled && exit_code == 0 {
                130
            } else {
                exit_code
            },
            cancelled,
            stdout,
            stderr,
        })
    }
}

fn shutdown_error(error: std::io::Error) -> EngineError {
    EngineError::Cleanup(format!(
        "the engine's completion could not be confirmed: {error}"
    ))
}

async fn cancellation_requested(receiver: &mut watch::Receiver<bool>) {
    while !*receiver.borrow_and_update() {
        if receiver.changed().await.is_err() {
            return;
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Interruption {
    Interrupt,
    Terminate,
    Hangup,
}

impl Interruption {
    pub fn exit_code(self) -> u8 {
        match self {
            Self::Interrupt => 130,
            Self::Terminate => 143,
            Self::Hangup => 129,
        }
    }
}

pub fn interruption()
-> Result<impl Future<Output = Result<Interruption, std::io::Error>> + Send, std::io::Error> {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut interrupt = signal(SignalKind::interrupt())?;
        let mut terminate = signal(SignalKind::terminate())?;
        let mut hangup = signal(SignalKind::hangup())?;
        Ok(async move {
            tokio::select! {
                _ = interrupt.recv() => Ok(Interruption::Interrupt),
                _ = terminate.recv() => Ok(Interruption::Terminate),
                _ = hangup.recv() => Ok(Interruption::Hangup),
            }
        })
    }
    #[cfg(not(unix))]
    Ok(async {
        tokio::signal::ctrl_c()
            .await
            .map(|()| Interruption::Interrupt)
    })
}

async fn capture(mut reader: impl AsyncRead + Unpin) -> Result<String, std::io::Error> {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 8192];
    let mut truncated = false;
    loop {
        let count = reader.read(&mut buffer).await?;
        if count == 0 {
            break;
        }
        bytes.extend_from_slice(&buffer[..count]);
        if bytes.len() > CAPTURE_LIMIT {
            let excess = bytes.len() - CAPTURE_LIMIT;
            bytes.drain(..excess);
            truncated = true;
        }
    }
    let text = String::from_utf8_lossy(&bytes);
    Ok(if truncated {
        format!("[Earlier output truncated; see the engine's saved log.]\n{text}")
    } else {
        text.into_owned()
    })
}

async fn collect(
    task: Option<tokio::task::JoinHandle<Result<String, std::io::Error>>>,
) -> Result<String, EngineError> {
    match task {
        Some(task) => Ok(tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .map_err(|_| {
                EngineError::Cleanup("an owned process still holds an output pipe".into())
            })???),
        None => Ok(String::new()),
    }
}

fn read_progress(
    reader: &mut File,
    pending: &mut String,
    events: &EventSink,
) -> Result<(), EngineError> {
    let mut bytes = Vec::new();
    reader.take(64 * 1024).read_to_end(&mut bytes)?;
    pending.push_str(&String::from_utf8_lossy(&bytes));
    if pending.len() > 64 * 1024 {
        return Err(EngineError::Cleanup(
            "engine progress record exceeded 64 KiB".into(),
        ));
    }
    while let Some(end) = pending.find('\n') {
        let line: String = pending.drain(..=end).collect();
        match Progress::parse(&line) {
            Ok(progress) => events(EngineEvent::Progress(progress)),
            Err(error) => events(EngineEvent::Diagnostic(error.to_string())),
        }
    }
    Ok(())
}

struct ProcessGroup {
    id: i32,
    armed: bool,
    privileged_cleanup: bool,
}

impl ProcessGroup {
    #[cfg(unix)]
    fn signal(&self, force: bool) -> Result<(), EngineError> {
        use nix::errno::Errno;
        use nix::sys::signal::{Signal, killpg};
        use nix::unistd::Pid;
        let signal = if force {
            Signal::SIGKILL
        } else {
            Signal::SIGTERM
        };
        match killpg(Pid::from_raw(self.id), signal) {
            Ok(()) | Err(Errno::ESRCH) => Ok(()),
            Err(Errno::EPERM) if self.privileged_cleanup => {
                let status = std::process::Command::new("/usr/bin/sudo")
                    .args([
                        "-n",
                        "/bin/kill",
                        if force { "-KILL" } else { "-TERM" },
                        "--",
                    ])
                    .arg(format!("-{}", self.id))
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .map_err(|e| EngineError::Cleanup(e.to_string()))?;
                if status.success() {
                    Ok(())
                } else {
                    Err(EngineError::Cleanup(
                        "the engine's existing authorization could not stop its privileged descendants".into(),
                    ))
                }
            }
            Err(error) => Err(EngineError::Cleanup(error.to_string())),
        }
    }

    #[cfg(not(unix))]
    fn signal(&self, _force: bool) -> Result<(), EngineError> {
        Err(EngineError::UnsupportedHost)
    }

    async fn stop_remaining(&self) -> Result<(), EngineError> {
        #[cfg(unix)]
        {
            use nix::errno::Errno;
            use nix::sys::signal::killpg;
            use nix::unistd::Pid;
            for attempt in 0..20 {
                match killpg(Pid::from_raw(self.id), None) {
                    Err(Errno::ESRCH) => return Ok(()),
                    Ok(()) | Err(Errno::EPERM) => {
                        self.signal(attempt >= 10)?;
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                    Err(error) => return Err(EngineError::Cleanup(error.to_string())),
                }
            }
            Err(EngineError::Cleanup(
                "owned descendant processes are still running".into(),
            ))
        }
        #[cfg(not(unix))]
        Err(EngineError::UnsupportedHost)
    }
}

impl Drop for ProcessGroup {
    fn drop(&mut self) {
        if self.armed
            && let Err(error) = self.signal(true)
        {
            tracing::error!(error = %error, "Failed to stop the owned engine process group");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wor_application::Operation;

    fn fixture(body: &str) -> (tempfile::TempDir, Engine) {
        let temp = tempfile::tempdir().expect("fixture");
        let root = temp.path().join("runtime");
        let manifest: serde_json::Value = serde_json::from_str(include_str!(concat!(
            env!("OUT_DIR"),
            "/runtime-manifest.json"
        )))
        .expect("compiled inventory");
        for entry in manifest["files"].as_array().expect("files") {
            let relative = entry["path"].as_str().expect("path");
            let target = root.join(relative);
            fs::create_dir_all(target.parent().expect("parent")).expect("directory");
            fs::copy(
                PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("../..")
                    .join(relative),
                target,
            )
            .expect("fixture file");
        }
        let helper = temp.path().join("native-helper");
        fs::write(&helper, format!("#!/bin/sh\nset -eu\n{body}\n")).expect("helper");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&helper, fs::Permissions::from_mode(0o755)).expect("mode");
        }
        let runtime = Runtime::source(&root).expect("complete fixture runtime");
        let engine = Engine::new(runtime, helper).expect("engine");
        (temp, engine)
    }

    #[tokio::test]
    async fn sends_a_native_job_payload_and_captures_worker_output() {
        let temp = tempfile::tempdir().expect("fixture");
        let payload = temp.path().join("payload.json");
        let body = format!("cat > '{}'\nprintf 'native worker\\n'", payload.display());
        let (_runtime_temp, engine) = fixture(&body);
        let (_sender, receiver) = watch::channel(false);
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            engine.execute(
                Request::new(Operation::Run(vec!["--version".into()])),
                OutputMode::Capture,
                receiver,
                Arc::new(|_| {}),
            ),
        )
        .await
        .expect("worker must receive EOF and finish")
        .expect("execute");
        assert!(result.succeeded());
        assert_eq!(result.stdout, "native worker\n");
        let job: serde_json::Value =
            serde_json::from_slice(&fs::read(payload).expect("worker payload")).expect("JSON");
        assert_eq!(job["action"]["kind"], "Run");
        assert_eq!(job["action"]["arguments"][0], "--version");
    }

    #[tokio::test]
    async fn cancellation_stops_owned_children_and_preserves_non_success() {
        let temp = tempfile::tempdir().expect("fixture");
        let ready = temp.path().join("ready");
        let (_runtime_temp, engine) = fixture(&format!(
            "cat >/dev/null\nsleep 60 &\nworker=$!\nprintf '%s' \"$worker\" > '{}'\nwait",
            ready.display()
        ));
        let (sender, receiver) = watch::channel(false);
        let task = tokio::spawn(async move {
            engine
                .execute(
                    Request::new(Operation::Run(Vec::new())),
                    OutputMode::Capture,
                    receiver,
                    Arc::new(|_| {}),
                )
                .await
        });
        tokio::time::timeout(Duration::from_secs(5), async {
            while !ready.is_file() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("owned child started");
        let worker: i32 = fs::read_to_string(&ready)
            .expect("worker identity")
            .parse()
            .expect("worker PID");
        sender.send(true).expect("cancel");
        let result = tokio::time::timeout(Duration::from_secs(8), task)
            .await
            .expect("bounded cancellation")
            .expect("worker")
            .expect("cleanup");
        assert!(result.cancelled);
        assert!(!result.succeeded());
        #[cfg(unix)]
        assert_eq!(
            nix::sys::signal::kill(nix::unistd::Pid::from_raw(worker), None),
            Err(nix::errno::Errno::ESRCH)
        );
    }

    #[tokio::test]
    async fn a_pre_canceled_request_never_starts_the_engine() {
        let temp = tempfile::tempdir().expect("fixture");
        let marker = temp.path().join("started");
        let body = format!("touch '{}'", marker.display());
        let (_runtime_temp, engine) = fixture(&body);
        let (_sender, receiver) = watch::channel(true);
        let result = engine
            .execute(
                Request::new(Operation::Run(Vec::new())),
                OutputMode::Capture,
                receiver,
                Arc::new(|_| {}),
            )
            .await
            .expect("cancel");
        assert!(result.cancelled);
        assert!(result.stdout.is_empty());
        assert!(!marker.exists());
    }

    #[tokio::test]
    async fn changed_config_is_rejected_before_the_engine_is_started() {
        let temp = tempfile::tempdir().expect("fixture");
        let marker = temp.path().join("started");
        let body = format!("touch '{}'", marker.display());
        let (_runtime_temp, engine) = fixture(&body);
        let config = temp.path().join("config.json");
        fs::write(&config, "{}").expect("config");
        let mut request = Request::new(Operation::Run(Vec::new()));
        request.config = Some(config);
        request.expected_config_digest = Some("not-the-reviewed-digest".into());
        let (_sender, receiver) = watch::channel(false);
        let result = engine
            .execute(request, OutputMode::Capture, receiver, Arc::new(|_| {}))
            .await;
        assert!(matches!(result, Err(EngineError::ChangedConfiguration)));
        assert!(!marker.exists());
    }

    #[tokio::test]
    async fn the_engine_reads_a_private_snapshot_not_a_later_config_change() {
        let temp = tempfile::tempdir().expect("fixture");
        let config = temp.path().join("config.json");
        let payload = temp.path().join("payload.json");
        let ready = temp.path().join("ready");
        let release = temp.path().join("release");
        let body = format!(
            "cat > '{}'\ntouch '{}'\nwhile [ ! -f '{}' ];do sleep 0.01;done",
            payload.display(),
            ready.display(),
            release.display()
        );
        let (_runtime_temp, engine) = fixture(&body);
        let original = r#"{"execution":{"dryRun":true}}"#;
        fs::write(&config, original).expect("config");
        let mut request = Request::new(Operation::Summary);
        request.config = Some(config.clone());
        request.expected_config_digest = Some(format!("{:x}", Sha256::digest(original.as_bytes())));
        let (_sender, receiver) = watch::channel(false);
        let task = tokio::spawn(async move {
            engine
                .execute(request, OutputMode::Capture, receiver, Arc::new(|_| {}))
                .await
        });
        tokio::time::timeout(Duration::from_secs(5), async {
            while !ready.is_file() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("snapshot ready");
        let job: serde_json::Value =
            serde_json::from_slice(&fs::read(&payload).expect("worker payload")).expect("JSON");
        let snapshot = PathBuf::from(job["config"].as_str().expect("config path"));
        assert_ne!(snapshot, config);
        assert_eq!(fs::read_to_string(&snapshot).expect("snapshot"), original);
        fs::write(&config, r#"{"execution":{"dryRun":false}}"#).expect("changed");
        assert_eq!(
            fs::read_to_string(&snapshot).expect("private snapshot"),
            original
        );
        fs::write(&release, "continue").expect("release fixture");
        let result = task.await.expect("worker").expect("result");
        assert!(result.succeeded());
    }
}
