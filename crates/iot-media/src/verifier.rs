use crate::{Inspection, MediaError, Result, check_cancel};
use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

pub trait FfuVerifier {
    fn inspect(&self, path: &Path, cancel: &AtomicBool) -> Result<Inspection>;
}

/// The native CLI companion, invoked with `[iot-ffu, inspect, FILE, --json]`.
/// No sibling-crate dependency is needed to test this boundary.
#[derive(Clone, Debug)]
pub struct NativeFfuVerifier {
    pub executable: PathBuf,
    pub timeout: Duration,
}

impl NativeFfuVerifier {
    pub fn new(executable: impl Into<PathBuf>) -> Self {
        Self {
            executable: executable.into(),
            timeout: Duration::from_secs(600),
        }
    }

    pub fn arguments(path: &Path) -> Vec<OsString> {
        vec![
            "iot-ffu".into(),
            "inspect".into(),
            path.as_os_str().to_owned(),
            "--json".into(),
        ]
    }
}

impl FfuVerifier for NativeFfuVerifier {
    fn inspect(&self, path: &Path, cancel: &AtomicBool) -> Result<Inspection> {
        check_cancel(cancel)?;
        let mut child = Command::new(&self.executable)
            .args(Self::arguments(path))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|_| {
                MediaError::acquisition("cannot start the native IoT FFU verifier companion")
            })?;
        let excessive = Arc::new(AtomicBool::new(false));
        let mut stdout = child.stdout.take().expect("piped stdout");
        let mut stderr = child.stderr.take().expect("piped stderr");
        let out_flag = excessive.clone();
        let output = std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let result = (&mut stdout).take(1024 * 1024 + 1).read_to_end(&mut bytes);
            if bytes.len() > 1024 * 1024 {
                out_flag.store(true, Ordering::SeqCst);
            }
            (result, bytes)
        });
        let err_flag = excessive.clone();
        let diagnostic = std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let result = (&mut stderr).take(65537).read_to_end(&mut bytes);
            if bytes.len() > 65536 {
                err_flag.store(true, Ordering::SeqCst);
            }
            result
        });
        let start = Instant::now();
        let status = loop {
            let error = if cancel.load(Ordering::SeqCst) {
                Some(MediaError::Cancelled)
            } else if excessive.load(Ordering::SeqCst) {
                Some(MediaError::acquisition(
                    "native FFU verifier output exceeds its limit",
                ))
            } else if start.elapsed() >= self.timeout {
                Some(MediaError::acquisition("native FFU verifier timed out"))
            } else {
                None
            };
            if let Some(error) = error {
                let _ = child.kill();
                let _ = child.wait();
                let _ = output.join();
                let _ = diagnostic.join();
                return Err(error);
            }
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => std::thread::sleep(Duration::from_millis(10)),
                Err(_) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = output.join();
                    let _ = diagnostic.join();
                    return Err(MediaError::acquisition(
                        "cannot wait for the native FFU verifier",
                    ));
                }
            }
        };
        let (read, bytes) = output
            .join()
            .map_err(|_| MediaError::acquisition("native FFU verifier output reader failed"))?;
        read?;
        diagnostic.join().map_err(|_| {
            MediaError::acquisition("native FFU verifier diagnostic reader failed")
        })??;
        check_cancel(cancel)?;
        if excessive.load(Ordering::SeqCst) {
            return Err(MediaError::acquisition(
                "native FFU verifier output exceeds its limit",
            ));
        }
        if !status.success() {
            return Err(if status.code() == Some(2) {
                MediaError::invalid("native FFU verifier rejected the image")
            } else {
                MediaError::acquisition("native FFU verification failed; image is not verified")
            });
        }
        serde_json::from_slice(&bytes).map_err(|_| {
            MediaError::invalid("native FFU verifier returned invalid inspection JSON")
        })
    }
}
