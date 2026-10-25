//! Opt-in guard-removal checks. Every mutation is compiled in a disposable copy.
use std::fs;
use std::path::Path;
use std::process::{Command, Output};

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name();
        if name.to_string_lossy().starts_with('.') || name == "target" {
            continue;
        }
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &to.join(name));
        } else {
            fs::copy(entry.path(), to.join(name)).unwrap();
        }
    }
}

fn test(root: &Path, package: &str, name: &str) -> Output {
    Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
        .current_dir(root)
        .args([
            "test",
            "--offline",
            "--quiet",
            "--lib",
            "-p",
            package,
            name,
            "--",
            "--exact",
        ])
        .env("CARGO_TARGET_DIR", root.join("target"))
        .env("CARGO_INCREMENTAL", "0")
        .output()
        .unwrap()
}

#[test]
#[ignore = "compiles disposable workspaces; run explicitly after the focused service tests"]
fn disposable_guard_mutations() {
    let owned = Path::new(env!("CARGO_MANIFEST_DIR"));
    let original_root = owned.parent().unwrap().parent().unwrap();
    let directory = tempfile::Builder::new()
        .prefix(".iot-mutations-")
        .tempdir_in(owned)
        .unwrap();
    let root = directory.path();
    for name in ["iot-media", "iot-account"] {
        copy_tree(
            &original_root.join("crates").join(name),
            &root.join("crates").join(name),
        );
    }
    fs::create_dir_all(root.join("src/lib")).unwrap();
    fs::create_dir_all(root.join("src/config")).unwrap();
    for name in [
        "iot-media.py",
        "iot-account.py",
        "iot-ffu.py",
        "iot-account.ps1",
        "iot-locale.ps1",
        "iot-locale-user.ps1",
    ] {
        fs::copy(
            original_root.join("src/lib").join(name),
            root.join("src/lib").join(name),
        )
        .unwrap();
    }
    fs::copy(
        original_root.join("src/config/iot-core-images.json"),
        root.join("src/config/iot-core-images.json"),
    )
    .unwrap();
    fs::write(root.join("Cargo.toml"), r#"
[workspace]
members = ["crates/iot-media", "crates/iot-account"]
resolver = "2"
[workspace.package]
version = "0.1.0"
edition = "2024"
rust-version = "1.99"
license = "GPL-3.0-or-later"
publish = false
[workspace.dependencies]
libc = "0.2"
signal-hook = "0.3"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
sha2 = "0.10"
reqwest = { version = "0.12", default-features = false, features = ["blocking", "json", "rustls-tls"] }
tempfile = "3"
thiserror = "2"
[workspace.lints.rust]
unsafe_op_in_unsafe_fn = "deny"
"#).unwrap();
    for arguments in [
        vec![
            "test",
            "--offline",
            "--quiet",
            "-p",
            "wor-iot-media",
            "-p",
            "wor-iot-account",
        ],
        vec![
            "clippy",
            "--offline",
            "--quiet",
            "-p",
            "wor-iot-media",
            "-p",
            "wor-iot-account",
            "--all-targets",
            "--",
            "-D",
            "warnings",
        ],
    ] {
        let baseline = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
            .current_dir(root)
            .args(arguments)
            .env("CARGO_TARGET_DIR", root.join("target"))
            .env("CARGO_INCREMENTAL", "0")
            .output()
            .unwrap();
        assert!(
            baseline.status.success(),
            "standalone service baseline failed:\n{}\n{}",
            String::from_utf8_lossy(&baseline.stdout),
            String::from_utf8_lossy(&baseline.stderr)
        );
    }
    println!(
        "standalone service baseline: all unit/doc/native-protocol tests and strict Clippy passed"
    );
    if let Ok(target) = std::env::var("WOR_IOT_CROSS_CHECK_TARGET") {
        let packages = std::env::var("WOR_IOT_CROSS_CHECK_PACKAGES")
            .unwrap_or_else(|_| "wor-iot-media,wor-iot-account".into());
        let mut command = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
        command
            .current_dir(root)
            .args(["clippy", "--offline", "--quiet", "--target", &target]);
        for package in packages.split(',') {
            assert!(["wor-iot-media", "wor-iot-account"].contains(&package));
            command.args(["-p", package]);
        }
        let cross = command
            .args(["--all-targets", "--", "-D", "warnings"])
            .env("CARGO_TARGET_DIR", root.join("target"))
            .env("CARGO_INCREMENTAL", "0")
            .output()
            .unwrap();
        assert!(
            cross.status.success(),
            "standalone cross-check failed:\n{}\n{}",
            String::from_utf8_lossy(&cross.stdout),
            String::from_utf8_lossy(&cross.stderr)
        );
        println!("standalone service cross-check: {target} {packages} strict Clippy passed");
    }
    let cases = [
        (
            "crates/iot-media/src/lib.rs",
            "if format!(\"{:x}\", hash.finalize()) != package.sha256 {",
            "if false {",
            "wor-iot-media",
            "tests::source_wrapper_must_verify_even_with_a_good_cache",
        ),
        (
            "crates/iot-media/src/network.rs",
            "Some(\"go.microsoft.com\" | \"software-download.microsoft.com\")",
            "Some(_)",
            "wor-iot-media",
            "tests::all_url_trust_failures_are_refused_without_url_echo",
        ),
        (
            "crates/iot-media/src/fs.rs",
            "if private",
            "if false && private",
            "wor-iot-media",
            "tests::no_follow_private_owner_and_hardlink_cache_rules_hold",
        ),
        (
            "crates/iot-account/src/lib.rs",
            "if !request.confirm_identity",
            "if false",
            "wor-iot-account",
            "tests::inconsistent_fingerprint_no_consent_and_invalid_keys_never_send_credentials",
        ),
        (
            "crates/iot-account/src/lib.rs",
            "\"StrictHostKeyChecking=yes\"",
            "\"StrictHostKeyChecking=no\"",
            "wor-iot-account",
            "tests::two_pinned_logins_verify_same_sid_using_new_credentials_and_clean_state",
        ),
        (
            "crates/iot-account/src/lib.rs",
            "verified.sid != result.sid",
            "false",
            "wor-iot-account",
            "tests::failed_reconnect_or_changed_sid_stops_without_retry_under_old_credentials",
        ),
        (
            "src/lib/iot-locale.ps1",
            "RunLevel = 0",
            "RunLevel = 1",
            "wor-iot-account",
            "tests::source_oracle_payloads_preserve_admin_defaultaccount_and_cleanup_guards",
        ),
    ];
    for (file, before, after, package, name) in cases {
        let baseline = test(root, package, name);
        assert!(
            baseline.status.success(),
            "baseline {name}: {}",
            String::from_utf8_lossy(&baseline.stderr)
        );
        let path = root.join(file);
        let original = fs::read_to_string(&path).unwrap();
        assert_eq!(
            original.matches(before).count(),
            1,
            "mutation anchor {file}: {before}"
        );
        fs::write(&path, original.replacen(before, after, 1)).unwrap();
        let mutant = test(root, package, name);
        fs::write(&path, original).unwrap();
        let output = String::from_utf8_lossy(&mutant.stdout);
        assert!(
            !mutant.status.success()
                && output.contains("test result: FAILED.")
                && output.contains("1 failed"),
            "mutant escaped or failed compilation: {name}\n{output}\n{}",
            String::from_utf8_lossy(&mutant.stderr)
        );
        println!("killed {package} {name}");
    }
    directory.close().unwrap();
}
