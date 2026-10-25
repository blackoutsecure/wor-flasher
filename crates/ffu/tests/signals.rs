#![cfg(unix)]

use std::{
    fs::{self, File},
    io::Write,
    os::fd::AsRawFd,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use sha2::{Digest, Sha256};

struct State(PathBuf);
impl State {
    fn new() -> Self {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join(".test-state");
        fs::create_dir_all(&root).unwrap();
        let path = root.join(format!("signals-{}", std::process::id()));
        fs::create_dir(&path).unwrap();
        Self(path.canonicalize().unwrap())
    }
}
impl Drop for State {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
        let _ = fs::remove_dir(self.0.parent().unwrap());
    }
}

fn put16(bytes: &mut [u8], offset: usize, value: u16) {
    bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}
fn put32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}
fn pad(bytes: &mut Vec<u8>) {
    bytes.resize(bytes.len().div_ceil(1024) * 1024, 0);
}

fn large_synthetic_source(path: &Path) {
    let manifest = b"[FullFlash]\nVersion=2.0\nUEFI=True\nOSVersion=10.0.17763.107\nDevicePlatformId0=Broadcom.RPi.2\n\
[Store]\nStoreType=Default\nIsMainOSStore=True\nOnlyAllocateDefinedGptEntries=False\nSectorSize=512\nMinSectorCount=131072\n\
[Partition]\nName=SyntheticNonWindows\n";
    let blocks = 65536;
    let mut image = vec![0; 24];
    put32(&mut image, 0, 24);
    image[4..16].copy_from_slice(b"ImageFlash  ");
    put32(&mut image, 16, manifest.len() as u32);
    put32(&mut image, 20, 1);
    image.extend_from_slice(manifest);
    pad(&mut image);
    let mut store = vec![0; 248];
    put16(&mut store, 4, 1);
    put16(&mut store, 8, 2);
    store[12..27].copy_from_slice(b"Broadcom.RPi.2\0");
    put32(&mut store, 204, 1024);
    put32(&mut store, 208, 1);
    put32(&mut store, 212, 16);
    store.extend_from_slice(&1_u32.to_le_bytes());
    store.extend_from_slice(&(blocks as u32).to_le_bytes());
    store.extend_from_slice(&0_u64.to_le_bytes());
    pad(&mut store);
    let block = [0x33; 1024];
    let mut hashes = Vec::with_capacity((blocks + 2) * 32);
    hashes.extend_from_slice(&Sha256::digest(&image));
    hashes.extend_from_slice(&Sha256::digest(&store));
    let hash = Sha256::digest(block);
    for _ in 0..blocks {
        hashes.extend_from_slice(&hash);
    }
    let catalog = b"\x30\x03\x02\x01\x01";
    let mut security = vec![0; 32];
    put32(&mut security, 0, 32);
    security[4..16].copy_from_slice(b"SignedImage ");
    put32(&mut security, 16, 1);
    put32(&mut security, 20, 0x800c);
    put32(&mut security, 24, catalog.len() as u32);
    put32(&mut security, 28, hashes.len() as u32);
    security.extend_from_slice(catalog);
    security.extend_from_slice(&hashes);
    pad(&mut security);
    let mut file = File::create(path).unwrap();
    file.write_all(&security).unwrap();
    file.write_all(&image).unwrap();
    file.write_all(&store).unwrap();
    for _ in 0..blocks {
        file.write_all(&block).unwrap();
    }
    file.sync_all().unwrap();
}

#[test]
fn cli_sigint_and_sigterm_stop_owned_native_parser() {
    let state = State::new();
    let source = state.0.join("large-non-Windows.ffu");
    large_synthetic_source(&source);
    let original_bytes = fs::metadata(&source).unwrap().len();
    for signal in [libc::SIGINT, libc::SIGTERM] {
        let probe = File::open(&source).unwrap();
        let mut child = Command::new(env!("CARGO_BIN_EXE_iot-ffu"))
            .args(["inspect"])
            .arg(&source)
            .args(["--json"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut observed = false;
        while Instant::now() < deadline {
            // SAFETY: probe is a regular, exclusively test-owned file descriptor.
            let result = unsafe { libc::flock(probe.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
            if result == -1 {
                assert_eq!(
                    std::io::Error::last_os_error().kind(),
                    std::io::ErrorKind::WouldBlock
                );
                observed = true;
                break;
            }
            // SAFETY: release only this probe's successful test-file lock.
            assert_eq!(unsafe { libc::flock(probe.as_raw_fd(), libc::LOCK_UN) }, 0);
            if let Some(status) = child.try_wait().unwrap() {
                panic!("helper exited before source-lock readiness: {status}");
            }
            thread::sleep(Duration::from_millis(1));
        }
        if !observed {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("native helper never acquired the source lock");
        }
        // SAFETY: the specific PID is the live child started immediately above.
        assert_eq!(unsafe { libc::kill(child.id() as libc::pid_t, signal) }, 0);
        let output = child.wait_with_output().unwrap();
        assert_eq!(
            output.status.code(),
            Some(128 + signal),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stdout.is_empty());
        assert_eq!(
            String::from_utf8(output.stderr).unwrap(),
            "IoT Core FFU interrupted; target may be incomplete, do not boot it\n"
        );
        assert_eq!(fs::metadata(&source).unwrap().len(), original_bytes);
        // A terminated operation released its shared lock; no detached worker remains.
        assert_eq!(
            unsafe { libc::flock(probe.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
            0
        );
        assert_eq!(unsafe { libc::flock(probe.as_raw_fd(), libc::LOCK_UN) }, 0);
    }
}
