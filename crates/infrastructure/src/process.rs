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
use wor_application::{EngineEvent, Operation, Outcome, Request};
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
    #[error("This operation requires the native Rust backend")]
    NativeOnly,
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
    native_backend: bool,
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
            native_backend: true,
        })
    }

    pub fn classic(runtime: Runtime, native_helper: PathBuf) -> Result<Self, EngineError> {
        let mut engine = Self::new(runtime, native_helper)?;
        engine.native_backend = false;
        Ok(engine)
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
        let abort = private.path().join("abort");
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
        let payload = if self.native_backend {
            Some(serde_json::to_vec(
                &crate::native::NativeJob::from_request(&job_request, &self.runtime)?,
            )?)
        } else {
            None
        };
        let mut command = if payload.is_some() {
            let mut command = Command::new(&self.native_helper);
            command
                .arg("native-worker")
                .stdin(Stdio::piped())
                .env("WOR_IOT_NATIVE_COMPANION", &self.native_helper);
            command
        } else {
            let script = if matches!(request.operation, Operation::ClassicWizard) {
                "install-wor-gui.sh"
            } else {
                "install-wor-hook.sh"
            };
            let mut command = Command::new("/bin/bash");
            command
                .arg(self.runtime.root().join(script))
                .env("WOR_GUI_ABORT_MARKER", &abort);
            if request.graphical {
                command.env("RUN_MODE", "gui").stdin(Stdio::null());
            } else {
                command.stdin(Stdio::inherit());
            }
            if let Some(config) = job_request.config.as_deref() {
                command.arg("--config").arg(config);
            }
            if let Some(path) = progress_path {
                command.env("WOR_GUI_PROGRESS_FILE", path);
            }
            for setting in &request.settings {
                command.env(setting.name(), setting.value());
            }
            command.env("WOR_RUST_BOOT_CONFIG", &self.native_helper);
            match &request.operation {
                Operation::ListDevices => {
                    command.arg("list-devices");
                }
                Operation::DescribeDevice(path) => {
                    command.arg("describe-device").arg(path);
                }
                Operation::Summary => {
                    command.arg("summary");
                }
                Operation::Run(arguments) => {
                    command.arg("run").args(arguments);
                }
                Operation::ClassicWizard => {}
                Operation::DeviceInventory
                | Operation::Preview
                | Operation::Catalog(_)
                | Operation::IotAccount(_) => {
                    return Err(EngineError::NativeOnly);
                }
            }
            command
        };
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
            abort,
        };
        let stdout_task = child.stdout.take().map(|pipe| tokio::spawn(capture(pipe)));
        let stderr_task = child.stderr.take().map(|pipe| tokio::spawn(capture(pipe)));
        if let Some(payload) = payload {
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
        }
        let mut pending = String::new();
        let mut interval = tokio::time::interval(Duration::from_millis(150));
        let mut cancelled = false;
        let status = loop {
            tokio::select! {
                status = child.wait() => break status.map_err(shutdown_error)?,
                () = cancellation_requested(&mut cancellation) => {
                    cancelled = true;
                    File::create(&group.abort).and_then(|mut file| file.flush()).map_err(shutdown_error)?;
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
    abort: PathBuf,
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
        if self.armed {
            if let Err(error) = File::create(&self.abort) {
                tracing::error!(error = %error, "Failed to mark the owned engine as aborted");
            }
            if let Err(error) = self.signal(true) {
                tracing::error!(error = %error, "Failed to stop the owned engine process group");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        fs::write(
            root.join("install-wor-hook.sh"),
            format!("#!/bin/bash\n{body}\n"),
        )
        .expect("hook");
        let runtime = Runtime::source(&root).expect("complete fixture runtime");
        let engine = Engine::classic(runtime, std::env::current_exe().expect("test executable"))
            .expect("engine");
        (temp, engine)
    }

    #[tokio::test]
    async fn forwards_options_as_arguments_and_values_as_environment_without_shell_evaluation() {
        let (_temp, engine) =
            fixture(r#"printf '<%s>\n' "$@"; printf 'setting=<%s>\n' "$SOURCE_FILE"; exit 7"#);
        let mut request = Request::new(Operation::Run(vec!["--version".into()]));
        request.config = Some("/tmp/config with spaces.json".into());
        request.settings.push(
            "SOURCE_FILE=literal; $(not-a-command)"
                .parse()
                .expect("setting"),
        );
        let (_sender, receiver) = watch::channel(false);
        let result = engine
            .execute(request, OutputMode::Capture, receiver, Arc::new(|_| {}))
            .await
            .expect("execute");
        assert_eq!(result.exit_code, 7);
        assert!(!result.succeeded());
        assert_eq!(
            result.stdout,
            "<--config>\n</tmp/config with spaces.json>\n<run>\n<--version>\nsetting=<literal; $(not-a-command)>\n"
        );
    }

    #[tokio::test]
    async fn cancellation_stops_owned_children_and_preserves_non_success() {
        let (temp, engine) = fixture(
            "trap 'kill \"$worker\" 2>/dev/null; wait \"$worker\" 2>/dev/null; exit 143' TERM\nsleep 60 &\nworker=$!\nprintf '%s' \"$worker\" > \"$WOR_TEST_READY\"\nwait",
        );
        let ready = temp.path().join("ready");
        let mut request = Request::new(Operation::Run(Vec::new()));
        request.settings.push(
            format!("WOR_TEST_READY={}", ready.display())
                .parse()
                .expect("setting"),
        );
        let (sender, receiver) = watch::channel(false);
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
        let (_temp, engine) = fixture("printf 'must not execute'; exit 0");
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
    }

    #[tokio::test]
    async fn changed_config_is_rejected_before_the_engine_is_started() {
        let (temp, engine) = fixture("printf 'must not execute'");
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
    }

    #[tokio::test]
    async fn the_engine_reads_a_private_snapshot_not_a_later_config_change() {
        let (temp, engine) = fixture(
            "printf ready > \"$WOR_TEST_READY\"\nwhile [ ! -f \"$WOR_TEST_RELEASE\" ];do sleep 0.01;done\ncat \"$2\"",
        );
        let config = temp.path().join("config.json");
        let ready = temp.path().join("ready");
        let release = temp.path().join("release");
        let original = r#"{"execution":{"dryRun":true}}"#;
        fs::write(&config, original).expect("config");
        let mut request = Request::new(Operation::Summary);
        request.config = Some(config.clone());
        request.expected_config_digest = Some(format!("{:x}", Sha256::digest(original.as_bytes())));
        request.settings.push(
            format!("WOR_TEST_READY={}", ready.display())
                .parse()
                .expect("setting"),
        );
        request.settings.push(
            format!("WOR_TEST_RELEASE={}", release.display())
                .parse()
                .expect("setting"),
        );
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
        fs::write(&config, r#"{"execution":{"dryRun":false}}"#).expect("changed");
        fs::write(&release, "continue").expect("release fixture");
        let result = task.await.expect("worker").expect("result");
        assert_eq!(result.stdout, original);
    }
}
