mod boot_config;

use anyhow::Context;
use clap::{CommandFactory, FromArgMatches, Parser, Subcommand};
use std::ffi::OsString;
use std::io::Read;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use tokio::sync::watch;
use wor_application::{Operation, Request, Setting};
use wor_infrastructure::{Engine, OutputMode, Runtime, bundled_product, interruption};

#[derive(Parser)]
#[command(
    name = "WoR-Flasher",
    about = "Native Rust imaging and helpers for WoR-Flasher. Physical writing is experimental and explicitly opt-in."
)]
struct Cli {
    #[arg(
        long,
        help = "Use the retained classic implementation explicitly; failures never fall back"
    )]
    classic: bool,
    #[arg(
        long,
        help = "Explicitly opt into hardware-unqualified native physical writes"
    )]
    allow_native_write: bool,
    #[arg(
        long,
        value_name = "DIRECTORY",
        help = "Use a trusted complete source checkout instead of a verified packaged runtime"
    )]
    runtime_dir: Option<PathBuf>,
    #[arg(long, value_name = "FILE")]
    config: Option<PathBuf>,
    #[arg(long, value_name = "FILE")]
    progress_file: Option<PathBuf>,
    #[arg(
        long = "set",
        value_name = "NAME=VALUE",
        help = "Set an engine preference; use a private config file for credentials"
    )]
    settings: Vec<String>,
    #[command(subcommand)]
    command: Action,
}

#[derive(Subcommand)]
enum Action {
    #[command(about = "Print safe candidate paths; this snapshot is not authorization to erase")]
    ListDevices,
    DescribeDevice {
        device: PathBuf,
    },
    Summary,
    #[command(hide = true)]
    NativeWorker,
    #[command(hide = true)]
    HostBootstrap {
        #[arg(long)]
        job_file: PathBuf,
        #[arg(long)]
        backup_dir: PathBuf,
    },
    #[command(about = "Native approved FFU parser/writer helper")]
    IotFfu {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        arguments: Vec<OsString>,
    },
    #[command(about = "Native verified IoT package acquisition helper")]
    IotMedia {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        arguments: Vec<OsString>,
    },
    #[command(about = "Native fingerprint-pinned IoT account/language helper")]
    IotAccount {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        arguments: Vec<OsString>,
    },
    #[command(about = "Native desktop media preparation helper")]
    DesktopMedia {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        arguments: Vec<OsString>,
    },
    #[command(about = "Query native Windows/firmware/driver/language catalogues")]
    Catalog {
        kind: String,
    },
    #[command(
        about = "Run native preparation/imaging with final target checks and explicit physical-write opt-in"
    )]
    Run {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        arguments: Vec<OsString>,
    },
    #[command(about = "Launch the native Dioxus wizard without a visible terminal")]
    Wizard,
    #[command(about = "Validate the installed runtime without running an engine command")]
    CheckRuntime,
    #[command(about = "Native boot-configuration JSON helper (stdin/stdout)")]
    BootConfig,
}

#[tokio::main]
async fn main() -> ExitCode {
    if std::env::args_os()
        .next()
        .as_deref()
        .and_then(|path| std::path::Path::new(path).file_name())
        .is_some_and(|name| name == "wor-iot-askpass")
    {
        let mut arguments = vec![OsString::from("askpass")];
        arguments.extend(std::env::args_os().skip(1));
        return match tokio::task::spawn_blocking(move || wor_iot_account::run(&arguments)).await {
            Ok(Ok(code)) => ExitCode::from(u8::try_from(code).unwrap_or(1)),
            Ok(Err(error)) => {
                eprintln!("Native SSH credential request failed: {error}");
                ExitCode::FAILURE
            }
            Err(error) => {
                eprintln!("Native SSH credential worker failed: {error}");
                ExitCode::FAILURE
            }
        };
    }
    let filter = match std::env::var("RUST_LOG") {
        Ok(value) => match tracing_subscriber::EnvFilter::try_new(value) {
            Ok(filter) => filter,
            Err(_) => {
                eprintln!("WoR-Flasher: invalid RUST_LOG configuration");
                return ExitCode::FAILURE;
            }
        },
        Err(std::env::VarError::NotPresent) => tracing_subscriber::EnvFilter::new("warn"),
        Err(std::env::VarError::NotUnicode(_)) => {
            eprintln!("WoR-Flasher: RUST_LOG must be valid UTF-8");
            return ExitCode::FAILURE;
        }
    };
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();
    match run().await {
        Ok(code) => ExitCode::from(u8::try_from(code).unwrap_or(1)),
        Err(error) => {
            eprintln!("WoR-Flasher: {error:#}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> anyhow::Result<i32> {
    let product = bundled_product()?;
    let matches = match Cli::command().version(product.version).try_get_matches() {
        Ok(matches) => matches,
        Err(error) => {
            let code = error.exit_code();
            error.print()?;
            return Ok(code);
        }
    };
    let cli = Cli::from_arg_matches(&matches)?;
    match &cli.command {
        Action::IotFfu { arguments } => {
            let arguments = arguments.clone();
            return tokio::task::spawn_blocking(move || {
                wor_ffu::run(&arguments).map_err(anyhow::Error::new)
            })
            .await?;
        }
        Action::IotMedia { arguments } => {
            let arguments = arguments.clone();
            return tokio::task::spawn_blocking(move || {
                wor_iot_media::run(&arguments).map_err(anyhow::Error::new)
            })
            .await?;
        }
        Action::IotAccount { arguments } => {
            let arguments = arguments.clone();
            return tokio::task::spawn_blocking(move || {
                wor_iot_account::run(&arguments).map_err(anyhow::Error::new)
            })
            .await?;
        }
        Action::DesktopMedia { arguments } => {
            let arguments = arguments.clone();
            return tokio::task::spawn_blocking(move || {
                wor_desktop_media::run(&arguments).map_err(anyhow::Error::new)
            })
            .await?;
        }
        Action::HostBootstrap {
            job_file,
            backup_dir,
        } => {
            let job_file = job_file.clone();
            let backup_dir = backup_dir.clone();
            tokio::task::spawn_blocking(move || {
                wor_infrastructure::native::bootstrap_from_file(&job_file, &backup_dir)
            })
            .await??;
            return Ok(0);
        }
        Action::NativeWorker => {
            let mut bytes = Vec::new();
            std::io::stdin()
                .take(4 * 1024 * 1024 + 1)
                .read_to_end(&mut bytes)?;
            if bytes.len() > 4 * 1024 * 1024 {
                anyhow::bail!("Native worker message exceeds its bound");
            }
            let job: wor_infrastructure::native::NativeJob = serde_json::from_slice(&bytes)?;
            let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let monitor = cancelled.clone();
            let interrupt = interruption()?;
            let signal = tokio::spawn(async move {
                match interrupt.await {
                    Ok(_) => monitor.store(true, Ordering::Release),
                    Err(error) => {
                        tracing::error!(error=%error,"Native interruption observer failed");
                        monitor.store(true, Ordering::Release);
                    }
                }
            });
            let result = tokio::task::spawn_blocking(move || {
                wor_infrastructure::native::run(job, &cancelled)
            })
            .await?;
            signal.abort();
            return match result {
                Ok(code) => Ok(code),
                Err(wor_infrastructure::native::NativeError::Canceled) => Ok(130),
                Err(wor_infrastructure::native::NativeError::Cleanup(error)) => {
                    eprintln!("Unsafe target cleanup: {error}");
                    Ok(74)
                }
                Err(error) => Err(error.into()),
            };
        }
        _ => {}
    }
    let mut settings = Vec::<Setting>::new();
    for value in &cli.settings {
        match value.parse() {
            Ok(setting) => settings.push(setting),
            Err(error) => {
                eprintln!("Invalid --set option: {error}");
                return Ok(2);
            }
        }
    }
    if matches!(cli.command, Action::BootConfig) {
        return boot_config::run();
    }
    let executable = std::env::current_exe().context("Finding the native executable")?;
    let runtime = Runtime::resolve(cli.runtime_dir.as_deref(), &executable)?;
    if matches!(cli.command, Action::CheckRuntime) {
        println!("Runtime is complete and valid.");
        return Ok(0);
    }
    if matches!(cli.command, Action::Wizard) && !cli.classic {
        let mut command = tokio::process::Command::new(
            executable
                .parent()
                .context("Finding native desktop")?
                .join("wor-desktop"),
        );
        if let Some(path) = &cli.runtime_dir {
            command.arg("--runtime-dir").arg(path);
        }
        if let Some(path) = &cli.config {
            command.arg("--config").arg(path);
        }
        return Ok(command
            .kill_on_drop(true)
            .status()
            .await
            .context("Build/install the Dioxus desktop companion")?
            .code()
            .unwrap_or(1));
    }
    let operation = match cli.command {
        Action::ListDevices => Operation::ListDevices,
        Action::DescribeDevice { device } => Operation::DescribeDevice(device),
        Action::Summary => Operation::Summary,
        Action::Catalog { kind } => Operation::Catalog(kind),
        Action::Run { arguments } => Operation::Run(arguments),
        Action::Wizard => Operation::ClassicWizard,
        Action::CheckRuntime
        | Action::BootConfig
        | Action::NativeWorker
        | Action::HostBootstrap { .. }
        | Action::IotFfu { .. }
        | Action::IotMedia { .. }
        | Action::IotAccount { .. }
        | Action::DesktopMedia { .. } => unreachable!("handled above"),
    };
    let mut request = Request::new(operation);
    request.config = cli.config;
    request.settings = settings;
    request.progress_file = cli.progress_file;
    request.allow_native_write = cli.allow_native_write;
    let engine = if cli.classic {
        Engine::classic(runtime, executable)?
    } else {
        Engine::new(runtime, executable)?
    };
    let (cancel, receiver) = watch::channel(false);
    let cancellation_code = Arc::new(AtomicU8::new(0));
    let signal_code = cancellation_code.clone();
    let interruption = interruption().context("Registering interruption handlers")?;
    let signal_task = tokio::spawn(async move {
        match interruption.await {
            Ok(reason) => {
                signal_code.store(reason.exit_code(), Ordering::Release);
                if cancel.send(true).is_err() {
                    tracing::warn!(
                        "The engine had already stopped when interruption was requested"
                    );
                }
            }
            Err(error) => {
                tracing::error!(error = %error, "Cannot observe interruption; canceling before continuing");
                signal_code.store(130, Ordering::Release);
                if cancel.send(true).is_err() {
                    tracing::warn!("The engine had already stopped");
                }
            }
        }
    });
    let result = engine
        .execute(request, OutputMode::Inherit, receiver, Arc::new(|_| {}))
        .await;
    signal_task.abort();
    let outcome = result?;
    let code = cancellation_code.load(Ordering::Acquire);
    Ok(if outcome.cancelled && code != 0 {
        i32::from(code)
    } else {
        outcome.exit_code
    })
}
