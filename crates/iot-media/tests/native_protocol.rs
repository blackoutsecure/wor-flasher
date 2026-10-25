//! Exercise the companion ABI and helper statuses using only native child processes.
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::{BufRead, BufReader, Read};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::AtomicBool;
use wor_iot_media::*;

fn main() {
    let args: Vec<OsString> = std::env::args_os().collect();
    if args.get(1).is_some_and(|arg| arg == "iot-ffu") {
        assert_eq!(args.len(), 5);
        assert_eq!(args[2], "inspect");
        assert_eq!(args[4], "--json");
        let path = Path::new(&args[3]);
        assert!(path.is_absolute());
        assert_eq!(
            std::fs::read(path).unwrap(),
            b"native companion fixture, not an approved FFU"
        );
        let info = Inspection {
            format: "ffu-v1".into(),
            architecture: "arm32".into(),
            profile: "iot-core".into(),
            sha256: "0".repeat(64),
            file_bytes: std::fs::metadata(path).unwrap().len(),
            trust: "pinned-official-sha256".into(),
            details: BTreeMap::new(),
        };
        println!("{}", serde_json::to_string(&info).unwrap());
        return;
    }
    if args.get(1).is_some_and(|arg| arg == "media-cli") {
        std::process::exit(run(&args[2..]).unwrap_or(4));
    }
    if args.get(1).is_some_and(|arg| arg == "trusted-metadata") {
        assert_eq!(
            OfficialImage::reviewed().unwrap().ffu_sha256(),
            "15e9451eb7b3e5645e89e25ffc846d4dfd4b4a8c9c63600e177cb16817da67f5"
        );
        return;
    }
    let root = tempfile::Builder::new()
        .prefix(".native-protocol-")
        .tempdir_in(env!("CARGO_MANIFEST_DIR"))
        .unwrap();
    let source = root.path().join("native fixture with spaces.ffu");
    std::fs::write(&source, b"native companion fixture, not an approved FFU").unwrap();
    let executable = std::env::current_exe().unwrap();
    let metadata = Command::new(&executable)
        .arg("trusted-metadata")
        .env("VERIFY_TLS", "0")
        .env("TRUST_CACHE", "1")
        .env("IOT_PROVENANCE_SHA256", "0".repeat(64))
        .output()
        .unwrap();
    assert!(metadata.status.success() && metadata.stdout.is_empty());
    let verifier = NativeFfuVerifier::new(&executable);
    let info = verifier.inspect(&source, &AtomicBool::new(false)).unwrap();
    assert_eq!(info.trust, "pinned-official-sha256");
    let options = MediaOptions {
        cache_dir: root.path().join("cache"),
        companion_executable: executable.clone(),
    };
    assert!(prepare(&source, &options, &mut |_| {}, &AtomicBool::new(false)).is_err());
    assert!(!options.cache_dir.exists());
    for (source, expected) in [
        (source.clone(), 2),
        (root.path().join("missing.ffu"), 4),
        (root.path().join("not-an-image.txt"), 2),
    ] {
        let output = Command::new(&executable)
            .arg("media-cli")
            .arg("prepare")
            .arg(source)
            .arg("--cache-dir")
            .arg(&options.cache_dir)
            .arg("--json")
            .env("VERIFY_TLS", "0")
            .env("TRUST_CACHE", "1")
            .env("IOT_PROVENANCE_SHA256", "0".repeat(64))
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(expected));
        assert!(output.stdout.is_empty());
    }
    let output = Command::new(&executable)
        .arg("media-cli")
        .arg("prepare")
        .arg(root.path().join("missing.ffu"))
        .arg("--cache-dir=ignored")
        .arg("--cache-dir")
        .arg(&options.cache_dir)
        .arg("--json")
        .arg("--json")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(4));
    assert!(output.stdout.is_empty());
    let sparse = root.path().join("cancellation fixture.ffu");
    let file = std::fs::File::create(&sparse).unwrap();
    file.set_len(1008861184).unwrap();
    drop(file);
    for signal in [libc::SIGINT, libc::SIGTERM] {
        let mut child = Command::new(&executable)
            .arg("media-cli")
            .arg("prepare")
            .arg(&sparse)
            .arg("--cache-dir")
            .arg(&options.cache_dir)
            .arg("--json")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut errors = BufReader::new(child.stderr.take().unwrap());
        let mut first = String::new();
        errors.read_line(&mut first).unwrap();
        assert!(first.contains("Validating complete approved FFU: 0%"));
        assert_eq!(unsafe { libc::kill(child.id() as i32, signal) }, 0);
        let mut rest = String::new();
        errors.read_to_string(&mut rest).unwrap();
        let output = child.wait_with_output().unwrap();
        assert_eq!(output.status.code(), Some(128 + signal));
        assert!(output.stdout.is_empty());
        assert!(!options.cache_dir.exists());
    }
    root.close().unwrap();
    println!(
        "native media protocol: fixed companion argv, production trust refusal, JSON/status and SIGINT/SIGTERM passed"
    );
}
