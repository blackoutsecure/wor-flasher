#![forbid(unsafe_code)]

use std::process::ExitCode;

#[cfg(not(windows))]
use wor_pi3_boot_refresh::Error;
use wor_pi3_boot_refresh::{Result, emit_event};

#[cfg(windows)]
fn run() -> Result<&'static str> {
    wor_pi3_boot_refresh::windows::run()
}

#[cfg(not(windows))]
fn run() -> Result<&'static str> {
    Err(Error::UnsupportedPlatform)
}

fn main() -> ExitCode {
    match run() {
        Ok(message) => {
            emit_event("LogInfo", message);
            ExitCode::SUCCESS
        }
        Err(error) => {
            emit_event(
                "LogFatal",
                &format!("Pi 3 boot finalization failed: {error}"),
            );
            ExitCode::FAILURE
        }
    }
}
