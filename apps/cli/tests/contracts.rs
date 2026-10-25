use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn cli(arguments: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_wor-flasher"))
        .args(arguments)
        .env("NO_UPDATE", "1")
        .env("CHECK_FOR_UPDATES", "0")
        .output()
        .expect("native CLI")
}

#[test]
fn product_version_comes_from_canonical_metadata_not_the_internal_crate_version() {
    let output = cli(&["--version"]);
    let product = wor_infrastructure::bundled_product().expect("metadata");
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).expect("version"),
        format!("WoR-Flasher {}\n", product.version)
    );
}

#[test]
fn usage_errors_keep_status_two_and_do_not_echo_secret_assignments() {
    for args in [
        vec!["unknown-command"],
        vec!["describe-device"],
        vec!["--set", "INVALID-NAME=fixture-sensitive-value", "summary"],
    ] {
        let output = cli(&args);
        assert_eq!(output.status.code(), Some(2));
        assert!(!String::from_utf8_lossy(&output.stderr).contains("fixture-sensitive-value"));
    }
}

#[test]
fn invalid_diagnostic_preferences_fail_explicitly_without_echoing_the_value() {
    let output = Command::new(env!("CARGO_BIN_EXE_wor-flasher"))
        .arg("--version")
        .env("RUST_LOG", "fixture-sensitive-value=not-a-log-level")
        .output()
        .expect("native CLI");
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("invalid RUST_LOG"));
    assert!(!String::from_utf8_lossy(&output.stderr).contains("fixture-sensitive-value"));
}

#[test]
fn missing_bundled_runtime_is_an_error_not_a_network_bootstrap() {
    let output = cli(&["check-runtime"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("--runtime-dir"));
}

#[test]
fn explicit_source_checkout_forwards_the_engine_version_without_media_access() {
    let output = Command::new(env!("CARGO_BIN_EXE_wor-flasher"))
        .arg("--runtime-dir")
        .arg(root())
        .args(["run", "--version"])
        .env("NO_UPDATE", "1")
        .env("CHECK_FOR_UPDATES", "0")
        .output()
        .expect("native CLI");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let product = wor_infrastructure::bundled_product().expect("metadata");
    assert_eq!(
        String::from_utf8(output.stdout).expect("version"),
        format!("WoR-Flasher {}\n", product.version)
    );
}

#[test]
fn classic_shell_execution_is_not_a_supported_cli_mode() {
    let output = cli(&["--classic", "--version"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("--classic"));
}

#[test]
fn native_summary_honors_explicit_false_preferences_without_media_access() {
    let temp = tempfile::tempdir().expect("fixture");
    let config = temp.path().join("config with spaces.json");
    std::fs::write(
        &config,
        r#"{
        "target":{"rpiModel":4,"device":"/dev/does-not-exist","canInstallOnSameDrive":true},
        "media":{"winLang":"en-us","bid":"22631.2861"},
        "notifications":{"playSound":false},
        "execution":{"dryRun":true,"checkForUpdates":false}
    }"#,
    )
    .expect("config");
    let output = Command::new(env!("CARGO_BIN_EXE_wor-flasher"))
        .arg("--runtime-dir")
        .arg(root())
        .args(["--config"])
        .arg(config)
        .arg("summary")
        .env("NO_UPDATE", "1")
        .env("CHECK_FOR_UPDATES", "0")
        .env("WOR_RUN_ID", "native-contract")
        .env("DL_DIR", temp.path().join("downloads"))
        .env("WOR_CACHE_DIR", temp.path().join("cache"))
        .output()
        .expect("native CLI");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let summary = String::from_utf8(output.stdout).expect("summary");
    assert!(summary.contains("Target hardware\tRaspberry Pi 4"));
    assert!(summary.contains("Windows 11 (en-US) arm64 build 22631.2861"));
}
