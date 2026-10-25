use std::{
    collections::BTreeMap,
    ffi::OsString,
    io::{self, Write},
    path::PathBuf,
    sync::atomic::{AtomicBool, AtomicI32, Ordering},
};

use serde::Serialize;

use crate::{
    ApplyOptions, Cancel, Error, Result, hdmi_options, identify_target, image_profile, trust,
    writer,
};

const HELP: &str = "\
Native bounded IoT Core FFU helper
Commands:
  hdmi-options [--hdmi-mode official|720p60|1080p60|custom] [--hdmi-config TEXT] [--json]
  profile --board BOARD [--json]
  inspect FILE [--json] [--hdmi-mode MODE] [--hdmi-config TEXT]
  identify-target TARGET [--json]
  apply FILE TARGET --target-size BYTES [--expected-sha256 SHA256]
        [--expected-target-id ID] [--allow-gpt-cleanup | --wipe-entire-drive]
        [--hdmi-mode MODE] [--hdmi-config TEXT]
Exit statuses: 0 success, 2 invalid/unapproved source or arguments, 3 unsafe
target, 4 I/O/write/readback failure, 130 SIGINT, 143 SIGTERM.
The caller must confirm and exclude protected disks and unmount the target.
There are no approval, verification, regular-file or provenance bypass flags.";

struct Arguments {
    command: String,
    positional: Vec<OsString>,
    values: BTreeMap<String, OsString>,
}

impl Arguments {
    fn parse(args: &[OsString]) -> Result<Option<Self>> {
        if args
            .first()
            .is_some_and(|arg| arg == "--help" || arg == "-h")
        {
            return Ok(None);
        }
        let command = args
            .first()
            .and_then(|value| value.to_str())
            .ok_or_else(|| Error::Arguments("a command is required".into()))?;
        let (count, allowed): (usize, &[&str]) = match command {
            "hdmi-options" => (0, &["--hdmi-mode", "--hdmi-config", "--json"]),
            "profile" => (0, &["--board", "--json"]),
            "inspect" => (1, &["--hdmi-mode", "--hdmi-config", "--json"]),
            "identify-target" => (1, &["--json"]),
            "apply" => (
                2,
                &[
                    "--target-size",
                    "--expected-sha256",
                    "--expected-target-id",
                    "--allow-gpt-cleanup",
                    "--hdmi-mode",
                    "--hdmi-config",
                    "--wipe-entire-drive",
                ],
            ),
            _ => return Err(Error::Arguments(format!("invalid command: {command}"))),
        };
        let mut result = Self {
            command: command.into(),
            positional: Vec::new(),
            values: BTreeMap::new(),
        };
        let mut iter = args[1..].iter();
        let mut literal = false;
        while let Some(value) = iter.next() {
            if !literal && value == "--" {
                literal = true;
                continue;
            }
            if !literal && (value == "--help" || value == "-h") {
                return Ok(None);
            }
            let text = value.to_str().unwrap_or_default();
            if !literal && text.starts_with('-') {
                let (name, inline) = text
                    .split_once('=')
                    .map_or((text, None), |(name, value)| (name, Some(value)));
                if !allowed.contains(&name) {
                    return Err(Error::Arguments(format!("unrecognized arguments: {text}")));
                }
                let flag = ["--json", "--allow-gpt-cleanup", "--wipe-entire-drive"].contains(&name);
                let value = if flag {
                    if inline.is_some() {
                        return Err(Error::Arguments(format!("{name} does not take a value")));
                    }
                    OsString::from("true")
                } else if let Some(value) = inline {
                    OsString::from(value)
                } else {
                    iter.next()
                        .filter(|value| !value.to_str().is_some_and(|text| text.starts_with("--")))
                        .cloned()
                        .ok_or_else(|| Error::Arguments(format!("{name} requires a value")))?
                };
                result.values.insert(name.into(), value);
            } else {
                result.positional.push(value.clone());
            }
        }
        if result.positional.len() != count {
            return Err(Error::Arguments(format!(
                "{command} requires {count} path argument(s)"
            )));
        }
        for required in match command {
            "profile" => &["--board"][..],
            "apply" => &["--target-size"][..],
            _ => &[][..],
        } {
            if !result.values.contains_key(*required) {
                return Err(Error::Arguments(format!("{required} is required")));
            }
        }
        Ok(Some(result))
    }

    fn text(&self, name: &str, default: &str) -> Result<String> {
        self.values.get(name).map_or(Ok(default.into()), |value| {
            value
                .to_str()
                .map(str::to_owned)
                .ok_or_else(|| Error::Arguments(format!("{name} must be UTF-8 text")))
        })
    }

    fn optional_text(&self, name: &str) -> Result<Option<String>> {
        self.values
            .get(name)
            .map(|_| self.text(name, ""))
            .transpose()
    }

    fn flag(&self, name: &str) -> bool {
        self.values.contains_key(name)
    }

    fn path(&self, index: usize) -> PathBuf {
        PathBuf::from(&self.positional[index])
    }
}

fn output(out: &mut dyn Write, text: impl std::fmt::Display) -> Result<()> {
    writeln!(out, "{text}").map_err(|error| Error::io("write helper output", error))
}

fn json(out: &mut dyn Write, value: impl Serialize) -> Result<()> {
    let value = serde_json::to_value(value)
        .map_err(|error| Error::Source(format!("encode helper JSON: {error}")))?;
    output(out, value)
}

fn target_size(value: &str) -> Result<u64> {
    let value = value.trim();
    let digits = value.strip_prefix(['-', '+']).unwrap_or(value);
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(Error::Arguments("invalid integer for --target-size".into()));
    }
    // Preserve the source ordering: negative/oversized geometry is checked only
    // after the complete source has been parsed and approved.
    Ok(if value.starts_with('-') {
        0
    } else {
        digits.parse().unwrap_or(0)
    })
}

fn execute(
    args: &[OsString],
    out: &mut dyn Write,
    err: &mut dyn Write,
    cancel: Cancel<'_>,
) -> Result<()> {
    let Some(args) = Arguments::parse(args)? else {
        return output(out, HELP);
    };
    cancel.check()?;
    let mode = args.text("--hdmi-mode", "official")?;
    let custom = args.text("--hdmi-config", "")?;
    match args.command.as_str() {
        "hdmi-options" => {
            let preferences = hdmi_options(&mode, &custom)?;
            if args.flag("--json") {
                json(out, preferences)
            } else {
                output(out, preferences.label)
            }
        }
        "profile" => {
            let profile = image_profile(&args.text("--board", "")?)?;
            if args.flag("--json") {
                json(out, profile)
            } else {
                output(
                    out,
                    format!(
                        "Expected Windows 10 IoT Core build: {}; image verification: Not Assessed",
                        profile.expected_build
                    ),
                )
            }
        }
        "identify-target" => json(out, identify_target(&args.path(0))?),
        "inspect" => {
            let result = trust::inspect(&args.path(0), &mode, &custom, cancel)?;
            if args.flag("--json") {
                json(out, result)
            } else {
                output(
                    out,
                    format!(
                        "Windows 10 IoT Core {} / ARM32 / legacy maker image",
                        result.build
                    ),
                )?;
                output(
                    out,
                    format!(
                        "Minimum disk: {} bytes; SHA256: {}",
                        result.minimum_disk_bytes, result.sha256
                    ),
                )?;
                output(
                    out,
                    format!(
                        "HDMI configuration checked: {}",
                        hdmi_options(&mode, &custom)?.label
                    ),
                )
            }
        }
        "apply" => {
            let options = ApplyOptions {
                target_size: target_size(&args.text("--target-size", "")?)?,
                expected_sha256: args.optional_text("--expected-sha256")?,
                expected_target_id: args.optional_text("--expected-target-id")?,
                allow_gpt_cleanup: args.flag("--allow-gpt-cleanup"),
                hdmi_mode: mode.clone(),
                hdmi_config: custom.clone(),
                wipe_entire_drive: args.flag("--wipe-entire-drive"),
            };
            let mut output_error = None;
            let mut progress = |phase: &str, percent: u8| {
                if output_error.is_none()
                    && let Err(error) =
                        writeln!(err, "{phase} IoT Core {percent}%").and_then(|_| err.flush())
                {
                    output_error = Some(error);
                    cancel.flag.store(true, Ordering::Relaxed);
                }
            };
            let result = writer::apply(
                &args.path(0),
                &args.path(1),
                &options,
                Some(&mut progress),
                cancel,
            );
            if let Some(error) = output_error {
                return Err(Error::io("write helper progress", error));
            }
            result?;
            if mode != "official" {
                output(
                    err,
                    format!(
                        "HDMI configuration written and readback-verified: {}",
                        hdmi_options(&mode, &custom)?.label
                    ),
                )?;
            }
            Ok(())
        }
        _ => unreachable!("validated command"),
    }
}

static CLI_SIGNAL: AtomicI32 = AtomicI32::new(0);

#[cfg(unix)]
mod signals {
    use super::*;
    use std::{
        mem::MaybeUninit,
        sync::{Mutex, MutexGuard},
    };

    static LOCK: Mutex<()> = Mutex::new(());

    extern "C" fn interrupted(signal: libc::c_int) {
        CLI_SIGNAL.store(signal, Ordering::Relaxed);
    }

    pub struct Guard {
        _lock: MutexGuard<'static, ()>,
        previous: Vec<(i32, libc::sigaction)>,
    }

    impl Guard {
        pub fn install() -> Result<Self> {
            let mut guard = Self {
                _lock: LOCK.lock().unwrap_or_else(|poison| poison.into_inner()),
                previous: Vec::new(),
            };
            CLI_SIGNAL.store(0, Ordering::Relaxed);
            for signal in [libc::SIGINT, libc::SIGTERM] {
                // SAFETY: sigaction is a C POD; an empty mask and a plain handler are valid.
                let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
                action.sa_sigaction = interrupted as *const () as usize;
                action.sa_flags = 0;
                // SAFETY: action owns the signal set being initialized.
                if unsafe { libc::sigemptyset(&mut action.sa_mask) } == -1 {
                    return Err(Error::io(
                        "initialize signal mask",
                        io::Error::last_os_error(),
                    ));
                }
                let mut previous = MaybeUninit::<libc::sigaction>::uninit();
                // SAFETY: handler and output pointers are valid; handler does only an atomic store.
                if unsafe { libc::sigaction(signal, &action, previous.as_mut_ptr()) } == -1 {
                    return Err(Error::io(
                        "install interruption handler",
                        io::Error::last_os_error(),
                    ));
                }
                // SAFETY: sigaction succeeded.
                guard
                    .previous
                    .push((signal, unsafe { previous.assume_init() }));
            }
            Ok(guard)
        }

        pub fn restore(&mut self) -> Result<()> {
            let mut error = None;
            while let Some((signal, previous)) = self.previous.pop() {
                // SAFETY: restore exactly the disposition returned by sigaction.
                if unsafe { libc::sigaction(signal, &previous, std::ptr::null_mut()) } == -1 {
                    error = Some(Error::io(
                        "restore interruption handler",
                        io::Error::last_os_error(),
                    ));
                }
            }
            error.map_or(Ok(()), Err)
        }
    }

    impl Drop for Guard {
        fn drop(&mut self) {
            let _ = self.restore();
        }
    }
}

/// Original helper command/JSON/status adapter. Errors are diagnosed on stderr
/// and returned as `Ok(2|3|4|130|143)`; `Err` means reporting/handler restoration
/// itself failed and callers should use `Error::exit_status()`.
///
/// Arguments exclude the executable name and any parent `iot-ffu` selector.
/// The synchronous writer stays in its caller's process; no child is detached.
pub fn run(args: &[OsString]) -> Result<i32> {
    run_with_io(args, &mut io::stdout().lock(), &mut io::stderr().lock())
}

/// Testable CLI adapter with caller-owned output sinks; the wire protocol is
/// identical to [`run`]. Library service APIs instead return typed failures.
pub fn run_with_io(args: &[OsString], out: &mut dyn Write, err: &mut dyn Write) -> Result<i32> {
    #[cfg(unix)]
    let mut signals = signals::Guard::install()?;
    let flag = AtomicBool::new(false);
    let cancel = Cancel {
        flag: &flag,
        signal: Some(&CLI_SIGNAL),
    };
    let mut result = execute(args, out, err, cancel);
    let signal = CLI_SIGNAL.load(Ordering::Relaxed);
    if signal != 0 {
        result = Err(Error::Interrupted { signal });
    } else if result.is_ok() {
        result = cancel.check();
    }
    #[cfg(unix)]
    signals.restore()?;
    match result {
        Ok(()) => Ok(0),
        Err(error) => {
            match &error {
                Error::Interrupted { .. } => output(
                    err,
                    "IoT Core FFU interrupted; target may be incomplete, do not boot it",
                )?,
                Error::Io { .. } => output(
                    err,
                    format!(
                        "IoT Core FFU I/O failure: {error}; if writing began, target may be incomplete"
                    ),
                )?,
                _ => output(err, format!("IoT Core FFU: {error}"))?,
            }
            Ok(error.exit_status())
        }
    }
}
