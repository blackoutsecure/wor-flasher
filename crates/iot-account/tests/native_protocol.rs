//! A native fake SSH executable and the real native askpass in one test binary.
//! The process never opens a network socket, device, or real credential.
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use std::ffi::OsString;
use std::io::{Read, Write};
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::sync::atomic::AtomicBool;
use wor_iot_account::*;

const ADDRESS: &str = "192.168.50.23";
const CURRENT: &str = "Fixture-current-123";
const DESIRED: &str = "Fixture-new-\"%&-456";
const SID: &str = "S-1-5-21-1-2-3-500";

fn key(suffix: u8) -> String {
    let mut bytes = 11u32.to_be_bytes().to_vec();
    bytes.extend(b"ssh-ed25519");
    bytes.extend([suffix; 32]);
    STANDARD.encode(bytes)
}
fn pairing() -> Pairing {
    Pairing {
        address: ADDRESS.into(),
        key_type: "ssh-ed25519".into(),
        fingerprint: key_fingerprint("ssh-ed25519", &key(0)).unwrap(),
        key: key(0),
    }
}
struct Resolver;
impl EndpointResolver for Resolver {
    fn resolve(&self, host: &str) -> Result<Vec<ResolvedAddress>> {
        assert!([ADDRESS, "minwinpc"].contains(&host));
        Ok(vec![ResolvedAddress {
            address: ADDRESS.parse().unwrap(),
            scope_id: 0,
        }])
    }
}
fn native_ssh() -> i32 {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    for option in [
        "StrictHostKeyChecking=yes",
        "HostKeyAlias=wor-iot-paired",
        "UserKnownHostsFile=known_hosts",
        "GlobalKnownHostsFile=/dev/null",
        "PreferredAuthentications=password",
        "PubkeyAuthentication=no",
    ] {
        assert!(args.contains(&OsString::from(option)));
    }
    assert_eq!(&args[..3], &["-F", "/dev/null", "-T"].map(OsString::from));
    let known = std::fs::read_to_string("known_hosts").unwrap();
    if known != format!("wor-iot-paired ssh-ed25519 {}\n", key(0)) {
        // A host-key mismatch is refused before the fake SSH even invokes askpass.
        return 255;
    }
    if let Ok(signal) = std::env::var("WOR_IOT_FIXTURE_SIGNAL") {
        let number: i32 = signal.parse().unwrap();
        assert!([libc::SIGINT, libc::SIGTERM].contains(&number));
        assert_eq!(unsafe { libc::kill(libc::getppid(), number) }, 0);
        std::thread::sleep(std::time::Duration::from_secs(5));
        return 255;
    }
    assert_eq!(std::env::var("SSH_ASKPASS_REQUIRE").unwrap(), "force");
    let response = Command::new(std::env::var_os("SSH_ASKPASS").unwrap())
        .arg("Password:")
        .output()
        .unwrap();
    assert!(response.status.success());
    let password = String::from_utf8(response.stdout).unwrap();
    let mut script = String::new();
    std::io::stdin()
        .take(65537)
        .read_to_string(&mut script)
        .unwrap();
    assert!(script.len() <= 65536);
    assert!(!script.contains(CURRENT) && !script.contains(DESIRED));
    assert!(
        args.iter()
            .all(|arg| !arg.to_string_lossy().contains(CURRENT)
                && !arg.to_string_lossy().contains(DESIRED))
    );
    let encoded = script
        .split("FromBase64String('")
        .nth(1)
        .unwrap()
        .split("')")
        .next()
        .unwrap();
    let request: serde_json::Value =
        serde_json::from_slice(&STANDARD.decode(encoded).unwrap()).unwrap();
    if request.get("language").is_some() {
        assert_eq!(password.trim_end(), CURRENT);
        assert!(
            script.contains("LogonType = 3")
                && script.contains("RunLevel = 0")
                && script.contains("DeleteTask")
        );
        println!(
            "{LOCALE_MARKER}{}",
            serde_json::json!({"state":"pending-reboot","language":"fr-FR",
            "changed":true,"stage":"language-verification","available":["en-US","fr-FR"]})
        );
    } else {
        let apply = request["operation"] == "apply";
        assert_eq!(password.trim_end(), if apply { CURRENT } else { DESIRED });
        assert_eq!(request["password"], if apply { DESIRED } else { "" });
        assert!(
            args.contains(
                &format!(
                    "{}@{ADDRESS}",
                    if apply {
                        "Administrator"
                    } else {
                        "FixtureAdmin"
                    }
                )
                .into()
            )
        );
        println!(
            "{MARKER}{}",
            serde_json::json!({"state":if apply {"applied"} else {"verified"},
            "username":"FixtureAdmin","sid":SID,"passwordChanged":apply,"usernameChanged":apply,
            "code":0,"stage":"login-verification"})
        );
    }
    0
}

fn cli(root: &Path, state: &Path, args: &[&str], input: &[u8], signal: Option<i32>) -> Output {
    let mut paths = vec![root.to_owned()];
    paths.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap()));
    let mut command = Command::new(root.join("fixture-cli"));
    command
        .args(args)
        .current_dir(state)
        .env("HOME", state)
        .env("PATH", std::env::join_paths(paths).unwrap())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(number) = signal {
        command.env("WOR_IOT_FIXTURE_SIGNAL", number.to_string());
    }
    let mut child = command.spawn().unwrap();
    child.stdin.take().unwrap().write_all(input).unwrap();
    child.wait_with_output().unwrap()
}

fn main() {
    let args: Vec<_> = std::env::args_os().collect();
    match Path::new(&args[0]).file_name().unwrap().to_str().unwrap() {
        "wor-iot-askpass" => {
            let mut command = vec![OsString::from("askpass")];
            command.extend(args.into_iter().skip(1));
            std::process::exit(run(&command).unwrap_or(1));
        }
        "fixture-cli" => std::process::exit(run(&args[1..]).unwrap_or(1)),
        "fixture-keyscan" | "ssh-keyscan" => {
            assert_eq!(
                args[1..],
                ["-T", "5", "-p", "22", "-t", "ed25519,ecdsa,rsa", ADDRESS].map(OsString::from)
            );
            println!("{ADDRESS} ssh-ed25519 {}", key(0));
            return;
        }
        "fixture-ssh" | "ssh" => std::process::exit(native_ssh()),
        _ => {}
    }
    let root = tempfile::Builder::new()
        .prefix(".native-protocol-")
        .tempdir_in(env!("CARGO_MANIFEST_DIR"))
        .unwrap();
    let executable = std::env::current_exe().unwrap();
    for name in [
        "fixture-ssh",
        "fixture-keyscan",
        "wor-iot-askpass",
        "fixture-cli",
        "ssh",
        "ssh-keyscan",
    ] {
        symlink(&executable, root.path().join(name)).unwrap();
    }
    let state_root = root.path().join("private state %h ${HOME} \"quoted\"");
    std::fs::create_dir(&state_root).unwrap();
    std::fs::set_permissions(&state_root, std::fs::Permissions::from_mode(0o700)).unwrap();
    let options = AccountOptions {
        companion_executable: executable,
        private_state_root: state_root.clone(),
        ssh_executable: root.path().join("fixture-ssh"),
        keyscan_executable: root.path().join("fixture-keyscan"),
    };
    let service = AccountService {
        resolver: &Resolver,
        tools: &SystemTools,
    };
    let cancel = AtomicBool::new(false);
    let scanned = service
        .probe_device("remembered.local", true, &options, &cancel)
        .unwrap();
    assert_eq!(scanned, pairing());
    let request = ConfigureRequest {
        pairing: scanned,
        confirm_identity: true,
        current_username: "Administrator".into(),
        current_password: Secret::new(CURRENT),
        account_setup: true,
        account_username: "FixtureAdmin".into(),
        account_password: Secret::new(DESIRED),
        language_setup: true,
        language: "fr-FR".into(),
    };
    let result = service
        .configure_account(&request, &options, &cancel)
        .unwrap();
    assert_eq!(result.state, SetupState::PendingReboot);
    assert_eq!(result.account_state, SetupState::Verified);
    assert_eq!(result.language_state, SetupState::PendingReboot);
    assert_eq!(std::fs::read_dir(&state_root).unwrap().count(), 0);
    let input = serde_json::to_vec(&serde_json::json!({
        "pairing":pairing(),"confirmIdentity":true,"currentUsername":"Administrator","currentPassword":CURRENT,
        "accountSetup":true,"accountUsername":"FixtureAdmin","accountPassword":DESIRED,"languageSetup":true,"language":"fr-FR"
    })).unwrap();
    let output = cli(root.path(), &state_root, &["configure"], &input, None);
    assert!(output.status.success(), "native CLI setup failed");
    let configured: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(configured["state"], "pending-reboot");
    assert_eq!(configured["accountState"], "verified");
    assert!(!String::from_utf8_lossy(&output.stdout).contains(CURRENT));
    assert!(!String::from_utf8_lossy(&output.stderr).contains(DESIRED));
    for signal in [libc::SIGINT, libc::SIGTERM] {
        let output = cli(
            root.path(),
            &state_root,
            &["configure"],
            &input,
            Some(signal),
        );
        assert_eq!(output.status.code(), Some(128 + signal));
        assert!(output.stdout.is_empty());
        assert!(!String::from_utf8_lossy(&output.stderr).contains(CURRENT));
        assert!(!String::from_utf8_lossy(&output.stderr).contains(DESIRED));
        assert_eq!(std::fs::read_dir(&state_root).unwrap().count(), 0);
    }
    let output = cli(
        root.path(),
        &state_root,
        &[
            "preferences",
            "--metadata-only",
            "--metadata-only",
            "--language",
            "fr-FR",
            "--language=en-US",
        ],
        br#"{"currentPassword":"Fixture-current-123"}"#,
        None,
    );
    assert!(output.status.success());
    let preferences: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(preferences["currentPassword"], CURRENT);
    assert_eq!(preferences["accountSetup"], false);
    assert!(output.stderr.is_empty());
    let output = cli(
        root.path(),
        &state_root,
        &["probe"],
        format!("{{\"host\":\"{ADDRESS}\",\"automaticAddress\":false}}").as_bytes(),
        None,
    );
    assert!(output.status.success());
    assert_eq!(
        serde_json::from_slice::<Pairing>(&output.stdout).unwrap(),
        pairing()
    );
    let output = cli(
        root.path(),
        &state_root,
        &["preferences"],
        b"{malformed fixture request",
        None,
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let output = cli(root.path(), &state_root, &["login-defaults"], &[], None);
    assert!(output.status.success());
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap()["automaticHostname"],
        "minwinpc"
    );
    let mut bad_key = request.clone();
    bad_key.pairing.key = key(1);
    bad_key.pairing.fingerprint = key_fingerprint("ssh-ed25519", &bad_key.pairing.key).unwrap();
    assert!(
        service
            .configure_account(&bad_key, &options, &cancel)
            .is_err()
    );
    assert_eq!(std::fs::read_dir(&state_root).unwrap().count(), 0);

    let secret = root.path().join("private-password");
    std::fs::write(&secret, CURRENT).unwrap();
    let askpass = root.path().join("wor-iot-askpass");
    for (permissions, prompt, expected) in [
        (0o600, "Password:", true),
        (0o644, "Password:", false),
        (0o600, "Accept host key?", false),
    ] {
        std::fs::set_permissions(&secret, std::fs::Permissions::from_mode(permissions)).unwrap();
        let output = Command::new(&askpass)
            .arg(prompt)
            .env("WOR_IOT_ASKPASS_FILE", &secret)
            .output()
            .unwrap();
        assert_eq!(output.status.success(), expected);
        if expected {
            assert_eq!(output.stdout, format!("{CURRENT}\n").as_bytes());
        } else {
            assert!(output.stdout.is_empty());
        }
    }
    let link = root.path().join("password-symlink");
    symlink(&secret, &link).unwrap();
    let output = Command::new(&askpass)
        .arg("Password:")
        .env("WOR_IOT_ASKPASS_FILE", &link)
        .output()
        .unwrap();
    assert!(!output.status.success() && output.stdout.is_empty());
    assert!(!serde_json::to_string(&result).unwrap().contains(DESIRED));
    root.close().unwrap();
    println!(
        "native IoT protocol: helper JSON/status, SIGINT/SIGTERM, probe, pinned SSH/askpass, mismatched key, special-character cwd and cleanup passed"
    );
}
