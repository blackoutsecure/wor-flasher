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
fn summary_is_the_real_shared_engine_contract_including_explicit_false_preferences() {
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
    let common = |command: &mut Command| {
        command
            .args(["--config"])
            .arg(&config)
            .arg("summary")
            .env("NO_UPDATE", "1")
            .env("CHECK_FOR_UPDATES", "0")
            .env("WOR_RUN_ID", "native-contract")
            .env("DL_DIR", temp.path().join("downloads"))
            .env("WOR_CACHE_DIR", temp.path().join("cache"));
    };
    let mut native = Command::new(env!("CARGO_BIN_EXE_wor-flasher"));
    native.arg("--runtime-dir").arg(root());
    common(&mut native);
    let native = native.output().expect("native");
    let mut classic = Command::new("/bin/bash");
    classic.arg(root().join("install-wor-hook.sh"));
    common(&mut classic);
    let classic = classic.output().expect("classic");
    assert!(
        native.status.success(),
        "{}",
        String::from_utf8_lossy(&native.stderr)
    );
    assert_eq!(native.status.code(), classic.status.code());
    assert_eq!(native.stdout, classic.stdout);
}

#[cfg(unix)]
#[test]
fn termination_waits_for_the_owned_engine_and_does_not_leave_a_worker_running() {
    use std::fs;
    use std::process::Stdio;
    use std::time::{Duration, Instant};

    fn copy(source: &Path, target: &Path) {
        if source.is_dir() {
            fs::create_dir_all(target).expect("fixture directory");
            for entry in fs::read_dir(source).expect("fixture entries") {
                let entry = entry.expect("fixture entry");
                copy(&entry.path(), &target.join(entry.file_name()));
            }
        } else {
            fs::create_dir_all(target.parent().expect("parent")).expect("directory");
            fs::copy(source, target).expect("fixture file");
        }
    }

    let temp = tempfile::tempdir().expect("fixture");
    let runtime = temp.path().join("runtime");
    let metadata: serde_json::Value = serde_json::from_slice(
        &fs::read(root().join("src/config/metadata.json")).expect("metadata"),
    )
    .expect("metadata JSON");
    for path in metadata["runtimePaths"].as_array().expect("runtime paths") {
        let path = path.as_str().expect("path");
        copy(&root().join(path), &runtime.join(path));
    }
    fs::write(
        runtime.join("install-wor-hook.sh"),
        r#"#!/bin/bash
sleep 60 &
worker=$!
trap 'kill "$worker" 2>/dev/null; wait "$worker" 2>/dev/null; exit 143' TERM INT HUP
printf '%s %s' "$$" "$worker" > "$WOR_TEST_READY"
wait
"#,
    )
    .expect("owned-worker fixture");
    for (signal_name, expected_code) in [("-TERM", 143), ("-INT", 130), ("-HUP", 129)] {
        let ready = temp.path().join(format!("ready{signal_name}"));
        let mut child = Command::new(env!("CARGO_BIN_EXE_wor-flasher"))
            .arg("--classic")
            .arg("--runtime-dir")
            .arg(&runtime)
            .arg("--set")
            .arg(format!("WOR_TEST_READY={}", ready.display()))
            .arg("run")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("native CLI");
        let deadline = Instant::now() + Duration::from_secs(5);
        let identities = loop {
            if let Ok(identities) = fs::read_to_string(&ready)
                && identities.split_whitespace().count() == 2
            {
                break identities;
            }
            if Instant::now() >= deadline {
                child.kill().expect("stop fixture CLI");
                child.wait().expect("reap fixture CLI");
                panic!("Fixture did not start its owned worker");
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        let pids: Vec<&str> = identities.split_whitespace().collect();
        let signal = Command::new("/bin/kill")
            .args([signal_name, &child.id().to_string()])
            .status()
            .expect("signal owned CLI");
        assert!(signal.success());
        let deadline = Instant::now() + Duration::from_secs(8);
        let status = loop {
            if let Some(status) = child.try_wait().expect("wait for CLI") {
                break Some(status);
            }
            if Instant::now() >= deadline {
                for pid in &pids {
                    let _ = Command::new("/bin/kill")
                        .args(["-KILL", pid])
                        .stderr(Stdio::null())
                        .status();
                }
                child.kill().expect("stop unresponsive fixture CLI");
                child.wait().expect("reap fixture CLI");
                break None;
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        assert!(
            status.is_some(),
            "Native cancellation must finish within its existing cleanup deadline"
        );
        assert_eq!(
            status.expect("status").code(),
            Some(expected_code),
            "{signal_name}"
        );
        for pid in pids {
            let alive = Command::new("/bin/kill")
                .args(["-0", pid])
                .stderr(Stdio::null())
                .status()
                .expect("probe owned process");
            assert!(
                !alive.success(),
                "Owned process {pid} survived native cancellation"
            );
        }
    }
}
