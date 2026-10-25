use crate::{AccountError, Result};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use zeroize::Zeroizing;

/// Deliberately has no Debug implementation: input can contain an encoded password.
pub struct ToolRequest<'a> {
    pub executable: &'a Path,
    pub arguments: &'a [OsString],
    pub environment: &'a BTreeMap<OsString, OsString>,
    pub working_directory: Option<&'a File>,
    pub stdin: &'a [u8],
    pub timeout: Duration,
    pub stdout_limit: usize,
}

pub struct ToolOutput {
    pub status: i32,
    pub stdout: Vec<u8>,
}

pub trait NativeTools {
    fn run(&self, request: ToolRequest<'_>, cancel: &AtomicBool) -> Result<ToolOutput>;
}

#[derive(Default)]
pub struct SystemTools;

impl NativeTools for SystemTools {
    fn run(&self, request: ToolRequest<'_>, cancel: &AtomicBool) -> Result<ToolOutput> {
        if cancel.load(Ordering::SeqCst) {
            return Err(AccountError::Cancelled);
        }
        let mut command = Command::new(request.executable);
        command
            .args(request.arguments)
            .envs(request.environment)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(directory) = request.working_directory {
            let fd = directory.as_raw_fd();
            // Only the async-signal-safe fchdir syscall runs between fork and exec.
            unsafe {
                command.pre_exec(move || {
                    if libc::fchdir(fd) == 0 {
                        Ok(())
                    } else {
                        Err(std::io::Error::last_os_error())
                    }
                });
            }
        }
        let mut child = command.spawn().map_err(|_| {
            AccountError::invalid(
                "Cannot start OpenSSH. OpenSSH client tools are required for post-boot IoT setup.",
            )
        })?;
        let mut stdin = child.stdin.take().expect("piped stdin");
        let input = Zeroizing::new(request.stdin.to_owned());
        let input_thread = std::thread::spawn(move || stdin.write_all(&input));
        let mut stdout = child.stdout.take().expect("piped stdout");
        let mut stderr = child.stderr.take().expect("piped stderr");
        let limit = request.stdout_limit;
        let excessive = Arc::new(AtomicBool::new(false));
        let out_flag = excessive.clone();
        let output_thread = std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let status = (&mut stdout).take(limit as u64 + 1).read_to_end(&mut bytes);
            if bytes.len() > limit {
                out_flag.store(true, Ordering::SeqCst);
            }
            (status, bytes)
        });
        let err_flag = excessive.clone();
        let error_thread = std::thread::spawn(move || {
            let mut bytes = Zeroizing::new(Vec::new());
            let status = (&mut stderr).take(65537).read_to_end(&mut bytes);
            if bytes.len() > 65536 {
                err_flag.store(true, Ordering::SeqCst);
            }
            status
        });
        let start = Instant::now();
        let status = loop {
            let error = if cancel.load(Ordering::SeqCst) {
                Some(AccountError::Cancelled)
            } else if excessive.load(Ordering::SeqCst) {
                Some(AccountError::invalid(
                    "OpenSSH returned excessive output; device state is unverified.",
                ))
            } else if start.elapsed() >= request.timeout {
                Some(AccountError::invalid(
                    "SSH account setup failed or timed out; verify the old/new login before retrying.",
                ))
            } else {
                None
            };
            if let Some(error) = error {
                let _ = child.kill();
                let _ = child.wait();
                let _ = input_thread.join();
                let _ = output_thread.join();
                let _ = error_thread.join();
                return Err(error);
            }
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => std::thread::sleep(Duration::from_millis(10)),
                Err(_) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = input_thread.join();
                    let _ = output_thread.join();
                    let _ = error_thread.join();
                    return Err(AccountError::invalid(
                        "Cannot wait for OpenSSH; device state is unverified.",
                    ));
                }
            }
        };
        let input_result = input_thread
            .join()
            .map_err(|_| AccountError::invalid("OpenSSH private input failed."))?;
        let (read, stdout) = output_thread
            .join()
            .map_err(|_| AccountError::invalid("OpenSSH output reader failed."))?;
        let stderr_result = error_thread
            .join()
            .map_err(|_| AccountError::invalid("OpenSSH diagnostic reader failed."))?;
        if cancel.load(Ordering::SeqCst) {
            return Err(AccountError::Cancelled);
        }
        if excessive.load(Ordering::SeqCst) || stdout.len() > limit {
            return Err(AccountError::invalid(
                "OpenSSH returned excessive output; device state is unverified.",
            ));
        }
        if status.success() && (input_result.is_err() || read.is_err() || stderr_result.is_err()) {
            return Err(AccountError::invalid(
                "OpenSSH private input/output failed; device state is unverified.",
            ));
        }
        Ok(ToolOutput {
            status: status.code().unwrap_or(1),
            stdout,
        })
    }
}
