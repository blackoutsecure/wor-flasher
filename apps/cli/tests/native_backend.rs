use serde_json::{Value, json};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}
fn copy(source: &Path, target: &Path) {
    if source.is_dir() {
        fs::create_dir_all(target).expect("directory");
        for entry in fs::read_dir(source).expect("source entries") {
            let entry = entry.expect("entry");
            copy(&entry.path(), &target.join(entry.file_name()));
        }
    } else {
        fs::create_dir_all(target.parent().expect("parent")).expect("parent");
        fs::copy(source, target).expect("file");
    }
}
fn sparse() -> (tempfile::TempDir, PathBuf) {
    let temp = tempfile::tempdir().expect("fixture");
    let runtime = temp.path().join("runtime");
    let metadata: Value = serde_json::from_slice(
        &fs::read(root().join("src/config/metadata.json")).expect("metadata"),
    )
    .expect("JSON");
    for path in metadata["nativeRuntimePaths"]
        .as_array()
        .expect("native paths")
    {
        let path = path.as_str().expect("path");
        copy(&root().join(path), &runtime.join(path));
    }
    (temp, runtime)
}
fn run(runtime: &Path, arguments: &[&str], input: Option<&[u8]>) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_wor-flasher"));
    command
        .arg("--runtime-dir")
        .arg(runtime)
        .args(arguments)
        .env("NO_UPDATE", "1")
        .env("CHECK_FOR_UPDATES", "0");
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    if input.is_some() {
        command.stdin(Stdio::piped());
    } else {
        command.stdin(Stdio::null());
    }
    let mut child = command.spawn().expect("native CLI");
    if let Some(input) = input {
        child
            .stdin
            .take()
            .expect("private input")
            .write_all(input)
            .expect("stdin");
    }
    child.wait_with_output().expect("result")
}

#[test]
fn native_data_runtime_is_complete_without_any_host_language_bridge() {
    let (_temp, runtime) = sparse();
    for legacy in [
        "install-wor.sh",
        "install-wor-hook.sh",
        "src/lib/boot-config.py",
        "src/lib/iot-ffu.py",
        "src/updater.mjs",
    ] {
        assert!(!runtime.join(legacy).exists(), "{legacy}");
    }
    let result = run(&runtime, &["check-runtime"], None);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let result = run(&runtime, &["run", "--version"], None);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(String::from_utf8_lossy(&result.stdout).starts_with("WoR-Flasher "));
}

#[test]
fn summary_uses_native_resolution_without_bash_python_or_node_execution() {
    let (temp, runtime) = sparse();
    let config = temp.path().join("config.json");
    fs::write(&config,json!({
        "target":{"rpiModel":4,"device":"/dev/does-not-exist","canInstallOnSameDrive":true},
        "media":{"winLang":"en-us","bid":"22631.2861"},"execution":{"checkForUpdates":false,"dryRun":true}
    }).to_string()).expect("config");
    let forbidden = temp.path().join("forbidden");
    fs::create_dir(&forbidden).expect("bin");
    for name in ["bash", "python", "python3", "node"] {
        let path = forbidden.join(name);
        fs::write(&path, b"#!/bin/sh\nexit 91\n").expect("forbidden provider");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("mode");
        }
    }
    let output = Command::new(env!("CARGO_BIN_EXE_wor-flasher"))
        .arg("--runtime-dir")
        .arg(&runtime)
        .arg("--config")
        .arg(&config)
        .arg("summary")
        .env("PATH", &forbidden)
        .env("NO_UPDATE", "1")
        .env("WOR_RUN_ID", "native-no-bridge")
        .output()
        .expect("native summary");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).expect("UTF-8");
    assert!(text.contains("Target hardware\tRaspberry Pi 4"));
    assert!(text.contains("Operating system\tWindows 11 (en-US) arm64 build 22631.2861"));
}

#[test]
fn real_native_writes_require_an_independent_explicit_qualification_opt_in() {
    let (temp, runtime) = sparse();
    let config = temp.path().join("config.json");
    fs::write(&config,json!({
        "target":{"rpiModel":4,"device":"/dev/does-not-exist","canInstallOnSameDrive":true},
        "media":{"winLang":"en-us","bid":"22631.2861","downloadDir":temp.path().join("downloads")},"execution":{"dryRun":false,"checkForUpdates":false}
    }).to_string()).expect("config");
    let output = run(
        &runtime,
        &["--config", config.to_str().expect("path"), "run"],
        None,
    );
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("--allow-native-write"));
    assert!(!temp.path().join("downloaded").exists());
}

#[test]
fn native_ffu_helpers_keep_approval_and_exit_status_without_a_source_runtime() {
    let temp = tempfile::tempdir().expect("fixture");
    let file = temp.path().join("unapproved.ffu");
    fs::write(&file, b"not a Windows image").expect("synthetic input");
    let file = file.canonicalize().expect("canonical owned fixture");
    let output = Command::new(env!("CARGO_BIN_EXE_wor-flasher"))
        .args(["iot-ffu", "inspect"])
        .arg(&file)
        .arg("--json")
        .output()
        .expect("native helper");
    assert_eq!(
        output.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.is_empty());
    let output = Command::new(env!("CARGO_BIN_EXE_wor-flasher"))
        .args(["iot-ffu", "profile", "--board", "pi3-b", "--json"])
        .output()
        .expect("profile");
    assert!(output.status.success());
    let profile: Value = serde_json::from_slice(&output.stdout).expect("profile JSON");
    assert_eq!(profile["trust"], "pinned-bundled-profile");
    assert_eq!(profile["source_verification"], "not-assessed");
}

#[test]
fn native_account_validation_does_not_echo_private_passwords() {
    let (_temp, runtime) = sparse();
    let input = json!({"accountSetup":true,"accountUsername":"FixtureAdmin","accountPassword":"fixture-private-password-123",
        "currentUsername":"Administrator","currentPassword":"fixture-current-password-456"});
    let output = run(
        &runtime,
        &["iot-account", "configure"],
        Some(input.to_string().as_bytes()),
    );
    assert!(
        !output.status.success(),
        "An unpaired endpoint must never accept credentials"
    );
    assert!(output.stdout.is_empty());
    assert!(!String::from_utf8_lossy(&output.stdout).contains("fixture-private-password"));
    assert!(!String::from_utf8_lossy(&output.stderr).contains("fixture-current-password"));
}
