use crate::device::{DeviceError, DeviceInspector, DeviceSnapshot, Host, RealDeviceSystem};
use crate::{Runtime, RuntimeError};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use thiserror::Error;
use wor_application::{Operation, Request};
use wor_domain::configuration::{ConfigError, ImageFamily, NativeConfig, SETTINGS};
use wor_domain::progress::Progress;

mod desktop;

pub fn bootstrap_from_file(path: &Path, backup_dir: &Path) -> Result<(), NativeError> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > 1024 * 1024 {
        return Err(NativeError::Failed(
            "Host bootstrap requires a bounded regular private job file".into(),
        ));
    }
    let mut bytes = Vec::new();
    File::open(path)?
        .take(1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 1024 * 1024 {
        return Err(NativeError::Failed(
            "Bootstrap job exceeded its bound".into(),
        ));
    }
    desktop::host_bootstrap(serde_json::from_slice(&bytes)?, backup_dir)
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", content = "arguments")]
pub enum NativeAction {
    ListDevices,
    DeviceInventory,
    DescribeDevice(PathBuf),
    Summary,
    Preview,
    Catalog(String),
    Run(Vec<String>),
    IotAccount(Vec<String>),
}

#[derive(Serialize, Deserialize)]
pub struct NativeJob {
    pub action: NativeAction,
    pub runtime_dir: PathBuf,
    pub runtime_manifest: Option<PathBuf>,
    pub config: Option<PathBuf>,
    pub environment: BTreeMap<String, String>,
    pub progress_file: Option<PathBuf>,
    pub graphical: bool,
    pub sudo_password: Option<String>,
    pub private_input: Option<String>,
    pub allow_native_write: bool,
    pub target_fingerprint: Option<String>,
}

#[derive(Debug, Error)]
pub enum NativeError {
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error(transparent)]
    Device(#[from] DeviceError),
    #[error(transparent)]
    Runtime(#[from] RuntimeError),
    #[error("Native engine I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("Invalid native protocol data: {0}")]
    Json(#[from] serde_json::Error),
    #[error("{0}")]
    Failed(String),
    #[error("Operation canceled; any interrupted target is incomplete")]
    Canceled,
    #[error(
        "Native physical writes are not hardware-qualified. Explicit --allow-native-write consent is required; use dry run otherwise"
    )]
    Qualification,
    #[error("Target cleanup is not verified: {0}")]
    Cleanup(String),
}

impl NativeJob {
    pub fn from_request(request: &Request, runtime: &Runtime) -> Result<Self, NativeError> {
        let strings = |args: &[OsString]| -> Result<Vec<String>, NativeError> {
            args.iter()
                .map(|value| {
                    value.to_str().map(str::to_owned).ok_or_else(|| {
                        NativeError::Failed("Native command arguments must be valid UTF-8".into())
                    })
                })
                .collect()
        };
        let action = match &request.operation {
            Operation::ListDevices => NativeAction::ListDevices,
            Operation::DeviceInventory => NativeAction::DeviceInventory,
            Operation::DescribeDevice(path) => NativeAction::DescribeDevice(path.clone()),
            Operation::Summary => NativeAction::Summary,
            Operation::Preview => NativeAction::Preview,
            Operation::Catalog(kind) => NativeAction::Catalog(kind.clone()),
            Operation::Run(args) => NativeAction::Run(strings(args)?),
            Operation::IotAccount(args) => NativeAction::IotAccount(strings(args)?),
            Operation::ClassicWizard => {
                return Err(NativeError::Failed(
                    "Launch the native desktop companion for the guided wizard".into(),
                ));
            }
        };
        let mut environment = BTreeMap::new();
        for spec in SETTINGS {
            match std::env::var(spec.name) {
                Ok(value) => {
                    environment.insert(spec.name.into(), value);
                }
                Err(std::env::VarError::NotPresent) => {}
                Err(_) => {
                    return Err(NativeError::Failed(format!(
                        "{} must be valid UTF-8",
                        spec.name
                    )));
                }
            }
        }
        for setting in &request.settings {
            if !SETTINGS.iter().any(|spec| spec.name == setting.name()) {
                return Err(NativeError::Failed(format!(
                    "Unsupported native setting: {}",
                    setting.name()
                )));
            }
            environment.insert(setting.name().into(), setting.value().into());
        }
        Ok(Self {
            action,
            runtime_dir: runtime.root().into(),
            runtime_manifest: runtime.manifest_path().map(Path::to_owned),
            config: request.config.clone(),
            environment,
            progress_file: request.progress_file.clone(),
            graphical: request.graphical,
            sudo_password: request.sudo_password.clone(),
            private_input: request.private_input.clone(),
            allow_native_write: request.allow_native_write,
            target_fingerprint: request.target_fingerprint.clone(),
        })
    }
}

pub struct NativeContext {
    pub runtime: Runtime,
    pub config: NativeConfig,
    pub host: Host,
    pub executable: PathBuf,
    pub progress: Option<PathBuf>,
    pub sudo_password: Option<String>,
    pub graphical: bool,
    pub allow_native_write: bool,
    pub expected_target: Option<String>,
    pub original_config: Option<PathBuf>,
    pub log_path: Option<PathBuf>,
}

impl NativeContext {
    pub fn emit(&self, progress: Progress) -> Result<(), NativeError> {
        let clean = |value: String| value.replace(['\r', '\n'], " ");
        let line = match progress {
            Progress::Status(message) => format!("STATUS\t{}\n", clean(message)),
            Progress::Step {
                current,
                total,
                message,
            } => format!("STEP\t{current}\t{total}\t{}\n", clean(message)),
            Progress::Substep(percent) => format!("SUBSTEP\t{percent}\n"),
            Progress::Task { percent, label } => format!("TASK\t{percent}\t{}\n", clean(label)),
            Progress::DiskWrite { active, device } => {
                format!("DISK_WRITE\t{}\t{}\n", u8::from(active), clean(device))
            }
        };
        if let Some(path) = &self.progress {
            OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)?
                .write_all(line.as_bytes())?;
        }
        self.log(&line)?;
        eprint!("{line}");
        Ok(())
    }
    pub fn log(&self, text: &str) -> Result<(), NativeError> {
        if let Some(path) = &self.log_path {
            let mut options = OpenOptions::new();
            options.create(true).append(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600).custom_flags(nix::libc::O_NOFOLLOW);
            }
            options.open(path)?.write_all(text.as_bytes())?;
        }
        Ok(())
    }
    pub fn canceled(&self, cancelled: &AtomicBool) -> Result<(), NativeError> {
        if cancelled.load(Ordering::Acquire) {
            Err(NativeError::Canceled)
        } else {
            Ok(())
        }
    }
    pub fn protected_paths(&self, source: Option<&Path>) -> Vec<PathBuf> {
        let mut paths = vec![
            self.runtime.root().into(),
            self.executable.clone(),
            PathBuf::from(self.config.get("DL_DIR")),
        ];
        if let Some(path) = source {
            paths.push(path.into());
        }
        if let Some(path) = &self.original_config {
            paths.push(path.clone());
        }
        let log = self.config.get("WOR_LOG_FILE");
        if !log.is_empty() {
            paths.push(log.into());
        }
        paths
    }
    pub fn inspect_target(&self, iot: bool) -> Result<DeviceSnapshot, NativeError> {
        let inspector = DeviceInspector {
            host: self.host,
            system: RealDeviceSystem,
        };
        let source = (!self.config.get("SOURCE_FILE").is_empty())
            .then(|| Path::new(self.config.get("SOURCE_FILE")));
        let protected = inspector.protected_disks(&self.protected_paths(source))?;
        Ok(inspector.inspect(Path::new(self.config.get("DEVICE")), &protected, iot)?)
    }
    pub fn helper(
        &self,
        args: &[OsString],
        input: Option<&[u8]>,
        privileged: bool,
    ) -> Result<Vec<u8>, NativeError> {
        self.tool(&self.executable, args, input, privileged)
    }
    pub fn helper_json(&self, args: &[&str]) -> Result<Value, NativeError> {
        let args = args.iter().map(OsString::from).collect::<Vec<_>>();
        Ok(serde_json::from_slice(&self.helper(&args, None, false)?)?)
    }
    pub fn tool(
        &self,
        program: &Path,
        args: &[OsString],
        input: Option<&[u8]>,
        privileged: bool,
    ) -> Result<Vec<u8>, NativeError> {
        if privileged && input.is_some() && self.sudo_password.is_some() {
            return Err(NativeError::Failed("Privileged native helpers must receive private data through an owned job file, not the sudo password stream".into()));
        }
        let mut command = if privileged {
            let mut command = Command::new("/usr/bin/sudo");
            if self.sudo_password.is_some() {
                command.args(["-S", "-p", ""]);
            } else if self.graphical {
                command.arg("-n");
            }
            command.arg("--").arg(program);
            command
        } else {
            Command::new(program)
        };
        command
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let password = if privileged {
            self.sudo_password.as_ref()
        } else {
            None
        };
        if input.is_some() || password.is_some() {
            command.stdin(Stdio::piped());
        } else {
            command.stdin(Stdio::inherit());
        }
        let mut child = command.spawn().map_err(|error| {
            NativeError::Failed(format!("Cannot start {}: {error}", program.display()))
        })?;
        if input.is_some() || password.is_some() {
            let mut stdin = child
                .stdin
                .take()
                .ok_or_else(|| NativeError::Failed("Private tool input is unavailable".into()))?;
            if let Some(password) = password {
                stdin.write_all(password.as_bytes())?;
                stdin.write_all(b"\n")?;
            }
            if let Some(input) = input {
                stdin.write_all(input)?;
            }
        }
        let output = child.wait_with_output()?;
        if !output.stderr.is_empty() {
            std::io::stderr().write_all(&output.stderr)?;
            self.log(&String::from_utf8_lossy(&output.stderr))?;
        }
        if !output.status.success() {
            return Err(NativeError::Failed(format!(
                "{} failed; no second implementation was tried",
                program.file_name().unwrap_or_default().to_string_lossy()
            )));
        }
        Ok(output.stdout)
    }
}

pub fn fingerprint(snapshot: &DeviceSnapshot) -> Result<String, NativeError> {
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(snapshot)?)
    ))
}

fn read_document(path: &Path) -> Result<Value, NativeError> {
    let file = fs::File::open(path)?;
    if !file.metadata()?.is_file() {
        return Err(NativeError::Failed(
            "Configuration must be a regular JSON file".into(),
        ));
    }
    let mut bytes = Vec::new();
    file.take(1024 * 1024 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > 1024 * 1024 {
        return Err(NativeError::Failed("Configuration exceeds 1 MiB".into()));
    }
    let value: Value = serde_json::from_slice(&bytes)?;
    if !value.is_object() {
        return Err(ConfigError::InvalidDocument.into());
    }
    Ok(value)
}

pub fn default_config_path(runtime: &Runtime) -> Result<PathBuf, NativeError> {
    let cwd = std::env::current_dir()?;
    for path in [
        runtime.root().join("config.json"),
        cwd.join("config.json"),
        runtime.root().join("config-templates/config.json"),
        cwd.join("config-templates/config.json"),
    ] {
        if path.is_file() {
            return Ok(path);
        }
    }
    Err(NativeError::Failed(
        "No default configuration exists in the selected complete runtime".into(),
    ))
}

pub fn context(
    mut job: NativeJob,
) -> Result<(NativeContext, NativeAction, Option<String>), NativeError> {
    let runtime = match job.runtime_manifest {
        Some(path) => Runtime::verified(&job.runtime_dir, &path)?,
        None => Runtime::source(&job.runtime_dir)?,
    };
    if let NativeAction::Run(args) = &job.action {
        let mut index = 0;
        while index < args.len() {
            match args[index].as_str() {
                "--iot-core" => {
                    job.environment
                        .insert("WOR_IMAGE_FAMILY".into(), "iot-core".into());
                }
                "--iot-core-download" => {
                    job.environment
                        .insert("WOR_IMAGE_FAMILY".into(), "iot-core".into());
                    job.environment
                        .insert("WOR_IOT_DOWNLOAD".into(), "1".into());
                }
                "--config" => {
                    index += 1;
                    job.config = Some(
                        args.get(index)
                            .filter(|v| !v.is_empty())
                            .ok_or_else(|| {
                                NativeError::Failed("--config requires a file path".into())
                            })?
                            .into(),
                    );
                }
                arg if arg.starts_with("--config=") => {
                    let value = &arg[9..];
                    if value.is_empty() {
                        return Err(NativeError::Failed("--config requires a file path".into()));
                    }
                    job.config = Some(value.into());
                }
                "--version" | "--help" | "-h" => {}
                _ => return Err(NativeError::Failed("Unknown native run option".into())),
            }
            index += 1;
        }
    }
    let path = job
        .config
        .clone()
        .map_or_else(|| default_config_path(&runtime), Ok)?;
    let document = read_document(&path)?;
    let metadata: Value =
        serde_json::from_slice(&fs::read(runtime.root().join("src/config/metadata.json"))?)?;
    let home = std::env::var("HOME").map_err(|_| NativeError::Failed("HOME is required".into()))?;
    let locale = crate::desktop_system::host_locale();
    if !job.environment.contains_key("WOR_RUN_ID") {
        let stamp = time::OffsetDateTime::now_utc()
            .format(time::macros::format_description!(
                "[year][month][day]T[hour][minute][second]Z"
            ))
            .map_err(|error| NativeError::Failed(error.to_string()))?;
        job.environment.insert("WOR_RUN_ID".into(), stamp);
    }
    let config = NativeConfig::resolve(
        &document,
        &job.environment,
        &metadata,
        &home,
        &locale,
        cfg!(target_os = "macos"),
    )?;
    Ok((
        NativeContext {
            runtime,
            config,
            host: Host::detect()?,
            executable: std::env::current_exe()?,
            progress: job.progress_file,
            sudo_password: job.sudo_password,
            graphical: job.graphical,
            allow_native_write: job.allow_native_write,
            expected_target: job.target_fingerprint,
            original_config: Some(path),
            log_path: None,
        },
        job.action,
        job.private_input,
    ))
}

pub fn run(job: NativeJob, cancelled: &AtomicBool) -> Result<i32, NativeError> {
    let (mut context, action, private_input) = context(job)?;
    context.canceled(cancelled)?;
    let is_run = matches!(&action,NativeAction::Run(args) if !args.iter().any(|arg|matches!(arg.as_str(),"--version"|"--help"|"-h")));
    if is_run {
        let path = if context.config.get("WOR_LOG_FILE").is_empty() {
            PathBuf::from(context.config.get("DL_DIR"))
                .join("logs")
                .join(format!(
                    "wor-flasher-{}.log",
                    context.config.get("WOR_RUN_ID")
                ))
        } else {
            context.config.get("WOR_LOG_FILE").into()
        };
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        context.log_path = Some(path);
        context
            .log("Native Rust imaging run. Credentials and private job data are not logged.\n")?;
    }
    let result = (|| -> Result<i32, NativeError> {
        match action {
            NativeAction::ListDevices => {
                let (devices, warnings) = DeviceInspector {
                    host: context.host,
                    system: RealDeviceSystem,
                }
                .discover()?;
                for warning in warnings {
                    eprintln!("{warning}");
                }
                for device in devices {
                    println!("{}", device.path.display());
                }
            }
            NativeAction::DeviceInventory => {
                let (devices, warnings) = DeviceInspector {
                    host: context.host,
                    system: RealDeviceSystem,
                }
                .discover()?;
                println!("{}", json!({"devices":devices,"warnings":warnings}));
            }
            NativeAction::DescribeDevice(path) => {
                let inspector = DeviceInspector {
                    host: context.host,
                    system: RealDeviceSystem,
                };
                match inspector.inspect(&path, &Default::default(), false) {
                    Ok(device) => println!("{}", device.description()),
                    Err(error) => {
                        eprintln!("Device description unavailable: {error}");
                        println!("{}", path.display());
                    }
                }
            }
            NativeAction::Summary | NativeAction::Preview => {
                let preview = matches!(action, NativeAction::Preview);
                let mut snapshot = None;
                let description = if context.config.get("DEVICE").is_empty() {
                    "Not selected".into()
                } else {
                    match context.inspect_target(context.config.family()? == ImageFamily::IotCore) {
                        Ok(device) => {
                            let description = device.description();
                            snapshot = Some(device);
                            description
                        }
                        Err(error) if !preview => {
                            eprintln!("Device description unavailable: {error}");
                            context.config.get("DEVICE").into()
                        }
                        Err(error) => return Err(error),
                    }
                };
                let summary = summary(&context, &description)?;
                if preview {
                    context.config.validate_profile()?;
                    if let Some(device) = &snapshot {
                        let minimum = if context.config.family()? == ImageFamily::IotCore {
                            Some(
                                profile(&context)?["minimum_disk_bytes"]
                                    .as_u64()
                                    .ok_or_else(|| {
                                        NativeError::Failed(
                                            "Invalid approved IoT profile capacity".into(),
                                        )
                                    })?,
                            )
                        } else {
                            None
                        };
                        context.config.validate_capacity(device.bytes, minimum)?;
                    }
                    println!(
                        "{}",
                        json!({"summary":summary,"target_fingerprint":snapshot.as_ref().map(fingerprint).transpose()?,"device":snapshot})
                    );
                } else {
                    print!("{summary}");
                }
            }
            NativeAction::Catalog(kind) => desktop::catalog(&context, &kind, cancelled)?,
            NativeAction::IotAccount(arguments) => {
                let mut args = vec![OsString::from("iot-account")];
                args.extend(arguments.into_iter().map(OsString::from));
                let bytes =
                    context.helper(&args, private_input.as_deref().map(str::as_bytes), false)?;
                std::io::stdout().write_all(&bytes)?;
            }
            NativeAction::Run(arguments) => {
                if arguments.iter().any(|arg| arg == "--version") {
                    println!("{} {}", context.config.name(), context.config.version());
                    return Ok(0);
                }
                if arguments.iter().any(|arg| arg == "--help" || arg == "-h") {
                    println!(
                        "Native run: use --config FILE, --iot-core, --iot-core-download. DRY_RUN=1 prepares/inspects only. Physical native writes additionally require --allow-native-write."
                    );
                    return Ok(0);
                }
                if !context.graphical {
                    complete_terminal_choices(&mut context)?;
                }
                context.config.validate_profile()?;
                if !context.config.flag("DRY_RUN")? && !context.allow_native_write {
                    return Err(NativeError::Qualification);
                }
                let _target_lease = if context.config.flag("DRY_RUN")? {
                    None
                } else {
                    Some(target_lease(&mut context)?)
                };
                if !context.config.flag("NO_UPDATE")? {
                    match crate::desktop_system::release_notice(&context.config) {
                        Ok(Some(message)) => eprintln!("{message}"),
                        Ok(None) => {}
                        Err(error) => eprintln!("Read-only release check: Not Assessed ({error})"),
                    }
                }
                if context.config.family()? == ImageFamily::IotCore {
                    run_iot(&mut context, cancelled)?;
                } else {
                    desktop::run(&mut context, cancelled)?;
                }
            }
        }
        Ok(0)
    })();
    if is_run {
        if let Err(error) = &result {
            context.log(&format!("Operation failed: {error}\n"))?;
        } else {
            context.log(
                "Operation returned successfully. Hardware qualification remains Not Assessed.\n",
            )?;
        }
        if let Some(path) = &context.log_path {
            let parent = PathBuf::from(context.config.get("DL_DIR"));
            fs::create_dir_all(&parent)?;
            let mut latest = tempfile::NamedTempFile::new_in(&parent)?;
            std::io::copy(&mut File::open(path)?, &mut latest)?;
            latest.flush()?;
            latest
                .persist(parent.join("last-run.log"))
                .map_err(|error| error.error)?;
        }
    }
    result
}

fn terminal_choices_needed(config: &NativeConfig) -> Result<bool, NativeError> {
    let iot = config.family()? == ImageFamily::IotCore;
    Ok(config.get("RPI_MODEL").is_empty()
        || (iot
            && !matches!(
                config.get("WOR_TARGET_BOARD"),
                "pi2-v1.1" | "pi2-v1.2" | "pi3-b"
            ))
        || (!iot
            && (config.get("BID").is_empty()
                || config.get("WIN_LANG").is_empty()
                || config.get("CAN_INSTALL_ON_SAME_DRIVE").is_empty()))
        || (!config.flag("DRY_RUN")? && config.get("DEVICE").is_empty())
        || (iot && config.get("SOURCE_FILE").is_empty() && !config.flag("WOR_IOT_DOWNLOAD")?)
        || (iot
            && !config.flag("DRY_RUN")?
            && (!config.flag("WOR_IOT_CONFIRM_ERASE")?
                || (config.flag("IOT_CORE_WIPE_DRIVE")?
                    && !config.flag("WOR_IOT_CONFIRM_WIPE")?))))
}

fn complete_terminal_choices(context: &mut NativeContext) -> Result<(), NativeError> {
    let iot = context.config.family()? == ImageFamily::IotCore;
    if !terminal_choices_needed(&context.config)? {
        return Ok(());
    }

    let mut terminal=OpenOptions::new().read(true).write(true).open("/dev/tty")
        .map_err(|_|NativeError::Failed("Required imaging choices are missing. Use a complete private config or the native wizard; no hidden console prompt was attempted.".into()))?;
    fn ask(terminal: &mut File, prompt: &str) -> Result<String, NativeError> {
        use std::io::BufRead;
        terminal.write_all(prompt.as_bytes())?;
        terminal.flush()?;
        let mut reader = std::io::BufReader::new(terminal.try_clone()?.take(4097));
        let mut value = String::new();
        reader.read_line(&mut value)?;
        if value.len() > 4096 || value.contains('\0') {
            return Err(NativeError::Failed(
                "Terminal input exceeded its bound".into(),
            ));
        }
        if value.is_empty() {
            return Err(NativeError::Canceled);
        }
        Ok(value.trim_end_matches(['\r', '\n']).to_owned())
    }
    if iot && context.config.get("WOR_TARGET_BOARD").is_empty() {
        let board = ask(
            &mut terminal,
            "IoT board: 1 Pi2 v1.1, 2 Pi2 v1.2, 3 Pi3 Model B: ",
        )?;
        let (board, model) = match board.as_str() {
            "1" => ("pi2-v1.1", "2"),
            "2" => ("pi2-v1.2", "3"),
            "3" => ("pi3-b", "3"),
            _ => {
                return Err(NativeError::Failed(
                    "Unsupported IoT board selection".into(),
                ));
            }
        };
        context.config.set_derived("WOR_TARGET_BOARD", board);
        context.config.set_derived("RPI_MODEL", model);
    } else if !iot && context.config.get("RPI_MODEL").is_empty() {
        let board = ask(
            &mut terminal,
            "Desktop board: 2 Pi2 v1.2, 3 Pi3, 4 Pi4/400, 5 Pi5: ",
        )?;
        match board.as_str() {
            "2" => {
                context.config.set_derived("RPI_MODEL", "3");
                context.config.set_derived("WOR_TARGET_BOARD", "pi2-v1.2");
            }
            "3" | "4" | "5" => context.config.set_derived("RPI_MODEL", board),
            _ => {
                return Err(NativeError::Failed(
                    "Unsupported Raspberry Pi selection".into(),
                ));
            }
        }
    }
    if !iot && context.config.get("BID").is_empty() {
        context.config.set_derived(
            "BID",
            ask(&mut terminal, "Windows build ID (for example 22631.2861): ")?,
        );
    }
    if !iot && context.config.get("WIN_LANG").is_empty() {
        let value = ask(&mut terminal, "Windows language code [en-us]: ")?;
        context.config.set_derived(
            "WIN_LANG",
            if value.is_empty() {
                "en-us".into()
            } else {
                value
            },
        );
    }
    if iot
        && context.config.get("SOURCE_FILE").is_empty()
        && !context.config.flag("WOR_IOT_DOWNLOAD")?
    {
        let value = ask(
            &mut terminal,
            "Reviewed local FFU/ISO/MSI path, or type download for the official image: ",
        )?;
        if value == "download" {
            context.config.set_derived("WOR_IOT_DOWNLOAD", "1");
        } else {
            context.config.set_derived("SOURCE_FILE", value);
        }
    }
    if context.config.get("DEVICE").is_empty() && !context.config.flag("DRY_RUN")? {
        let (devices, warnings) = DeviceInspector {
            host: context.host,
            system: RealDeviceSystem,
        }
        .discover()?;
        for warning in warnings {
            writeln!(terminal, "{warning}")?;
        }
        for (index, device) in devices.iter().enumerate() {
            writeln!(terminal, "{}: {}", index + 1, device.description())?;
        }
        let selected = ask(
            &mut terminal,
            "Select the whole target drive number, or q to quit: ",
        )?;
        if selected == "q" {
            return Err(NativeError::Canceled);
        }
        let index = selected
            .parse::<usize>()
            .ok()
            .and_then(|v| v.checked_sub(1))
            .and_then(|index| devices.get(index))
            .ok_or_else(|| NativeError::Failed("Invalid safe target selection".into()))?;
        context
            .config
            .set_derived("DEVICE", index.path.to_string_lossy().into_owned());
    }
    if !iot && context.config.get("CAN_INSTALL_ON_SAME_DRIVE").is_empty() {
        let value = ask(
            &mut terminal,
            "Installation mode: 1 install on this drive (25 GiB+), 0 recovery media (8 GiB+): ",
        )?;
        if !matches!(value.as_str(), "0" | "1") {
            return Err(NativeError::Failed("Invalid installation mode".into()));
        }
        context
            .config
            .set_derived("CAN_INSTALL_ON_SAME_DRIVE", value);
    }
    if iot && !context.config.flag("DRY_RUN")? && !context.config.flag("WOR_IOT_CONFIRM_ERASE")? {
        if ask(
            &mut terminal,
            &format!(
                "Type ERASE to confirm the selected IoT target {}: ",
                context.config.get("DEVICE")
            ),
        )? != "ERASE"
        {
            return Err(NativeError::Canceled);
        }
        context.config.set_derived("WOR_IOT_CONFIRM_ERASE", "1");
    }
    if iot
        && !context.config.flag("DRY_RUN")?
        && context.config.flag("IOT_CORE_WIPE_DRIVE")?
        && !context.config.flag("WOR_IOT_CONFIRM_WIPE")?
    {
        let target = context.config.get("DEVICE");
        let answer = ask(
            &mut terminal,
            &format!(
                "Every sector on {target} will be zeroed and verified. This cannot repair failing hardware. Type the full device path {target} to confirm the entire-drive wipe: "
            ),
        )?;
        if answer != target {
            return Err(NativeError::Canceled);
        }
        context.config.set_derived("WOR_IOT_CONFIRM_WIPE", "1");
    }
    Ok(())
}

fn target_lease(context: &mut NativeContext) -> Result<File, NativeError> {
    let path = Path::new(context.config.get("DEVICE")).canonicalize()?;
    context
        .config
        .set_derived("DEVICE", path.to_string_lossy().into_owned());
    let home = std::env::var_os("HOME")
        .ok_or_else(|| NativeError::Failed("HOME is required for the native target lock".into()))?;
    let directory = PathBuf::from(home).join(".cache/wor-flasher-native/locks");
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(&directory)?;
    let metadata = fs::symlink_metadata(&directory)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(NativeError::Failed(
            "Native target-lock directory is unsafe".into(),
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.uid() != unsafe { nix::libc::geteuid() } || metadata.mode() & 0o077 != 0 {
            return Err(NativeError::Failed(
                "Native target-lock directory must be private to the current user".into(),
            ));
        }
    }
    let name = format!("{:x}", Sha256::digest(path.as_os_str().as_encoded_bytes()));
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(nix::libc::O_NOFOLLOW);
    }
    let file = options.open(directory.join(format!("{name}.lock")))?;
    fs2::FileExt::try_lock_exclusive(&file).map_err(|_| {
        NativeError::Failed(
            "Another native operation owns this target. No write was started.".into(),
        )
    })?;
    Ok(file)
}

fn profile(context: &NativeContext) -> Result<Value, NativeError> {
    context.helper_json(&[
        "iot-ffu",
        "profile",
        "--board",
        context.config.get("WOR_TARGET_BOARD"),
        "--json",
    ])
}

fn summary(context: &NativeContext, description: &str) -> Result<String, NativeError> {
    if context.config.family()? == ImageFamily::Desktop {
        return Ok(context
            .config
            .desktop_summary(description)?
            .into_iter()
            .map(|(key, value)| format!("{key}\t{value}\n"))
            .collect());
    }
    let info = profile(context)?;
    Ok(format!(
        "{} version\t{}\nTarget drive\t{}\nTarget hardware\t{}\nOperating system\tWindows 10 IoT Core ARM32 (legacy), build {} (planned)\nInstallation mode\tApply complete FFU to this drive\nWindows source\t{}\nDownload folder\t{}\nSource validation\tNot Assessed until image inspection\nImage verification\tMandatory approved SHA-256 and written-image readback\nDrive reset\t{}\nIoT administrator setup\t{}\nIoT language\t{}\nHDMI display\t{}\nDry run\t{}\n",
        context.config.name(),
        context.config.version(),
        description,
        context.config.board_label(),
        info["expected_build"]
            .as_str()
            .ok_or_else(|| NativeError::Failed("Invalid approved IoT build".into()))?,
        if context.config.get("SOURCE_FILE").is_empty() {
            "Official Microsoft image (download after Flash)"
        } else {
            context.config.get("SOURCE_FILE")
        },
        context.config.get("DL_DIR"),
        if context.config.flag("IOT_CORE_WIPE_DRIVE")? {
            "Full wipe; separate confirmation required"
        } else {
            "Normal validated partition metadata cleanup"
        },
        if context.config.flag("IOT_CORE_ACCOUNT_SETUP")? {
            "Pending after first boot; SSH identity and new login must be verified"
        } else {
            "Image default; no account changes requested"
        },
        if context.config.flag("IOT_CORE_LANGUAGE_SETUP")? {
            if context.config.get("IOT_CORE_LANGUAGE").is_empty() {
                context.config.get("WINDOWS_LOCALE")
            } else {
                context.config.get("IOT_CORE_LANGUAGE")
            }
        } else {
            "Image default"
        },
        context.config.get("IOT_CORE_HDMI_MODE"),
        if context.config.flag("DRY_RUN")? {
            "Yes (no changes will be written)"
        } else {
            "No"
        },
    ))
}

fn run_iot(context: &mut NativeContext, cancelled: &AtomicBool) -> Result<(), NativeError> {
    context.emit(Progress::Step {
        current: 1,
        total: 4,
        message: "Verify approved IoT image and source".into(),
    })?;
    let initial = if context.config.get("DEVICE").is_empty() {
        None
    } else {
        Some(context.inspect_target(true)?)
    };
    if !context.config.flag("DRY_RUN")? && initial.is_none() {
        return Err(NativeError::Failed(
            "Select an explicit safe IoT target drive".into(),
        ));
    }
    let approved_profile = profile(context)?;
    if let Some(device) = &initial {
        context.config.validate_capacity(
            device.bytes,
            Some(
                approved_profile["minimum_disk_bytes"]
                    .as_u64()
                    .ok_or_else(|| NativeError::Failed("Invalid IoT profile capacity".into()))?,
            ),
        )?;
        if device.sector_size != 512 {
            return Err(NativeError::Failed(
                "IoT Core requires 512-byte logical sectors".into(),
            ));
        }
        if let Some(expected) = &context.expected_target
            && fingerprint(device)? != *expected
        {
            return Err(DeviceError::ChangedTarget.into());
        }
    }
    context.canceled(cancelled)?;
    let cache = PathBuf::from(context.config.get("DL_DIR")).join("iot-core");
    let information = if context.config.flag("WOR_IOT_DOWNLOAD")? {
        let args = vec![
            OsString::from("iot-media"),
            OsString::from("download"),
            OsString::from("--cache-dir"),
            cache.as_os_str().into(),
            OsString::from("--json"),
        ];
        serde_json::from_slice::<Value>(&context.helper(&args, None, false)?)?
    } else {
        let source = Path::new(context.config.get("SOURCE_FILE")).canonicalize()?;
        if source
            .extension()
            .and_then(|v| v.to_str())
            .is_some_and(|v| v.eq_ignore_ascii_case("ffu"))
        {
            let args = vec![
                OsString::from("iot-ffu"),
                OsString::from("inspect"),
                source.as_os_str().into(),
                OsString::from("--json"),
            ];
            let mut info = serde_json::from_slice::<Value>(&context.helper(&args, None, false)?)?;
            info["source_file"] = json!(source);
            info
        } else {
            let args = vec![
                OsString::from("iot-media"),
                OsString::from("prepare"),
                source.as_os_str().into(),
                OsString::from("--cache-dir"),
                cache.as_os_str().into(),
                OsString::from("--json"),
            ];
            serde_json::from_slice::<Value>(&context.helper(&args, None, false)?)?
        }
    };
    let board = context.config.get("WOR_TARGET_BOARD");
    let digest = information["sha256"]
        .as_str()
        .filter(|value| {
            value.len() == 64
                && value
                    .bytes()
                    .all(|v| v.is_ascii_hexdigit() && !v.is_ascii_uppercase())
        })
        .ok_or_else(|| NativeError::Failed("Invalid verified IoT image identity".into()))?
        .to_owned();
    if information["architecture"] != "arm32"
        || information["profile"] != "iot-core"
        || information["format"] != "ffu-v1"
        || information["sector_size"] != 512
        || information["trust"] != "pinned-official-sha256"
        || !information["recommended_boards"]
            .as_array()
            .is_some_and(|boards| boards.iter().any(|value| value == board))
    {
        return Err(NativeError::Failed(
            "The inspected IoT image is not approved for this board".into(),
        ));
    }
    let source = PathBuf::from(
        information["source_file"]
            .as_str()
            .filter(|value| Path::new(value).is_absolute())
            .ok_or_else(|| NativeError::Failed("Missing absolute verified FFU path".into()))?,
    );
    context
        .config
        .set_derived("SOURCE_FILE", source.to_string_lossy().into_owned());
    context.canceled(cancelled)?;
    let hdmi = vec![
        OsString::from("iot-ffu"),
        OsString::from("inspect"),
        source.as_os_str().into(),
        OsString::from("--hdmi-mode"),
        OsString::from(context.config.get("IOT_CORE_HDMI_MODE")),
        OsString::from("--hdmi-config"),
        OsString::from(context.config.get("IOT_CORE_HDMI_CONFIG")),
    ];
    context.helper(&hdmi, None, false)?;
    if context.config.flag("DRY_RUN")? {
        println!(
            "Approved IoT FFU inspected. No drive was written. Post-boot personalization is Not Assessed."
        );
        return Ok(());
    }
    if !context.config.flag("WOR_IOT_CONFIRM_ERASE")? {
        return Err(NativeError::Failed(
            "Explicit IoT erase confirmation is required".into(),
        ));
    }
    if context.config.flag("IOT_CORE_WIPE_DRIVE")?
        && !context.config.flag("WOR_IOT_CONFIRM_WIPE")?
    {
        return Err(NativeError::Failed(
            "A full drive wipe requires separate explicit confirmation".into(),
        ));
    }
    let expected =
        initial.ok_or_else(|| NativeError::Failed("Missing selected IoT target".into()))?;
    let inspector = DeviceInspector {
        host: context.host,
        system: RealDeviceSystem,
    };
    let current = inspector.revalidate(
        &expected,
        &context.protected_paths(Some(&source)),
        true,
        true,
    )?;
    context.config.validate_capacity(
        current.bytes,
        Some(
            information["minimum_disk_bytes"]
                .as_u64()
                .ok_or_else(|| NativeError::Failed("Missing verified IoT capacity".into()))?,
        ),
    )?;
    let identity = context.helper_json(&[
        "iot-ffu",
        "identify-target",
        current
            .raw_path
            .to_str()
            .ok_or_else(|| NativeError::Failed("Non-UTF8 target".into()))?,
        "--json",
    ])?;
    let target_id = identity["target_id"]
        .as_str()
        .ok_or_else(|| NativeError::Failed("Missing native target identity".into()))?
        .to_owned();
    context.canceled(cancelled)?;
    desktop::unmount_device(context, &current)?;
    inspector.revalidate(
        &expected,
        &context.protected_paths(Some(&source)),
        true,
        true,
    )?;
    context.emit(Progress::Step {
        current: 2,
        total: 4,
        message: "Apply and verify the confirmed IoT target".into(),
    })?;
    context.emit(Progress::DiskWrite {
        active: true,
        device: current.path.to_string_lossy().into_owned(),
    })?;
    let mut args = vec![
        OsString::from("iot-ffu"),
        OsString::from("apply"),
        source.as_os_str().into(),
        current.raw_path.as_os_str().into(),
        OsString::from("--target-size"),
        current.bytes.to_string().into(),
        OsString::from("--expected-sha256"),
        digest.into(),
        OsString::from("--expected-target-id"),
        target_id.into(),
        OsString::from("--hdmi-mode"),
        context.config.get("IOT_CORE_HDMI_MODE").into(),
        OsString::from("--hdmi-config"),
        context.config.get("IOT_CORE_HDMI_CONFIG").into(),
    ];
    args.push(
        if context.config.flag("IOT_CORE_WIPE_DRIVE")? {
            "--wipe-entire-drive"
        } else {
            "--allow-gpt-cleanup"
        }
        .into(),
    );
    context.helper(&args, None, true)?;
    desktop::eject_device(context, &current)?;
    context.emit(Progress::DiskWrite {
        active: false,
        device: current.path.to_string_lossy().into_owned(),
    })?;
    context.emit(Progress::Step {
        current: 4,
        total: 4,
        message: "IoT image application and mandatory readback completed".into(),
    })?;
    println!(
        "IoT FFU application and readback completed. Hardware boot and post-boot account/language changes are Not Assessed."
    );
    Ok(())
}

#[cfg(test)]
mod terminal_tests {
    use super::*;

    fn iot() -> NativeConfig {
        let metadata =
            serde_json::from_str(include_str!("../../../src/config/metadata.json")).unwrap();
        let environment = BTreeMap::from([
            ("WOR_IMAGE_FAMILY".into(), "iot-core".into()),
            ("WOR_IMAGE_ARCH".into(), "arm32".into()),
            ("RPI_MODEL".into(), "3".into()),
            ("WOR_TARGET_BOARD".into(), "pi3-b".into()),
            ("SOURCE_FILE".into(), "/reviewed/image.ffu".into()),
            ("DEVICE".into(), "/dev/test-only".into()),
        ]);
        NativeConfig::resolve(
            &json!({}),
            &environment,
            &metadata,
            "/private/test",
            "en_US",
            false,
        )
        .unwrap()
    }

    #[test]
    fn a_complete_iot_selection_still_needs_independent_erase_and_wipe_consent() {
        let mut config = iot();
        assert!(terminal_choices_needed(&config).unwrap());
        config.set_derived("WOR_IOT_CONFIRM_ERASE", "1");
        assert!(!terminal_choices_needed(&config).unwrap());
        config.set_derived("IOT_CORE_WIPE_DRIVE", "1");
        assert!(terminal_choices_needed(&config).unwrap());
        config.set_derived("WOR_IOT_CONFIRM_WIPE", "1");
        assert!(!terminal_choices_needed(&config).unwrap());
    }

    #[test]
    fn an_iot_dry_run_never_requests_destructive_consent() {
        let mut config = iot();
        config.set_derived("DRY_RUN", "1");
        config.set_derived("IOT_CORE_WIPE_DRIVE", "1");
        config.set_derived("DEVICE", "");
        assert!(!terminal_choices_needed(&config).unwrap());
    }
}
