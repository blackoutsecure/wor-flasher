use super::*;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};

const CURRENT: &str = "Fixture-current-123";
const DESIRED: &str = "Fixture-new-\"%&-456";
const ADDRESS: &str = "192.168.50.23";
const SID: &str = "S-1-5-21-1-2-3-500";

fn temporary() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix(".account-test-")
        .tempdir_in(env!("CARGO_MANIFEST_DIR"))
        .unwrap()
}
fn key(kind: &str, suffix: u8) -> String {
    let mut raw = (kind.len() as u32).to_be_bytes().to_vec();
    raw.extend(kind.as_bytes());
    raw.extend([suffix; 32]);
    STANDARD.encode(raw)
}
fn pairing() -> Pairing {
    let key = key("ssh-ed25519", 0);
    Pairing {
        address: ADDRESS.into(),
        key_type: "ssh-ed25519".into(),
        fingerprint: key_fingerprint("ssh-ed25519", &key).unwrap(),
        key,
    }
}
fn request() -> ConfigureRequest {
    ConfigureRequest {
        pairing: pairing(),
        confirm_identity: true,
        current_username: FACTORY_USERNAME.into(),
        current_password: Secret::new(CURRENT),
        account_setup: true,
        account_username: "FixtureAdmin".into(),
        account_password: Secret::new(DESIRED),
        language_setup: false,
        language: "en-US".into(),
    }
}
fn options(root: &Path) -> AccountOptions {
    AccountOptions {
        companion_executable: std::env::current_exe().unwrap(),
        private_state_root: root.to_owned(),
        ssh_executable: PathBuf::from("/fixture/ssh"),
        keyscan_executable: PathBuf::from("/fixture/ssh-keyscan"),
    }
}
fn account_result(state: &str, sid: &str, username: &str, changed: bool) -> Vec<u8> {
    format!(
        "{MARKER}{}\n",
        serde_json::json!({"state":state,"sid":sid,"username":username,"passwordChanged":changed,
        "usernameChanged":changed,"code":0,"stage":"login-verification"})
    )
    .into_bytes()
}
fn language_result(state: &str, stage: &str, language: &str) -> Vec<u8> {
    format!(
        "{LOCALE_MARKER}{}\n",
        serde_json::json!({"state":state,"language":language,"changed":true,
        "stage":stage,"available":["en-US","fr-FR"]})
    )
    .into_bytes()
}
struct Resolver {
    answers: Vec<ResolvedAddress>,
    hosts: RefCell<Vec<String>>,
}
impl Resolver {
    fn local() -> Self {
        Self {
            answers: vec![ResolvedAddress {
                address: ADDRESS.parse().unwrap(),
                scope_id: 0,
            }],
            hosts: RefCell::new(Vec::new()),
        }
    }
}
impl EndpointResolver for Resolver {
    fn resolve(&self, host: &str) -> Result<Vec<ResolvedAddress>> {
        self.hosts.borrow_mut().push(host.into());
        Ok(self.answers.clone())
    }
}
struct Call {
    args: Vec<OsString>,
    password: Secret,
    script: Vec<u8>,
    directory: PathBuf,
}
struct Tools {
    replies: RefCell<VecDeque<ToolOutput>>,
    calls: RefCell<Vec<Call>>,
    damage_state: bool,
}
impl Tools {
    fn new(outputs: Vec<Vec<u8>>) -> Self {
        Self {
            replies: RefCell::new(
                outputs
                    .into_iter()
                    .map(|stdout| ToolOutput { stdout, status: 0 })
                    .collect(),
            ),
            calls: RefCell::new(Vec::new()),
            damage_state: false,
        }
    }
}
impl NativeTools for Tools {
    fn run(&self, tool: ToolRequest<'_>, _: &AtomicBool) -> Result<ToolOutput> {
        if tool.executable.file_name() == Some(OsStr::new("ssh-keyscan")) {
            assert!(tool.stdin.is_empty());
            assert!(tool.environment.is_empty());
            self.calls.borrow_mut().push(Call {
                args: tool.arguments.to_owned(),
                password: Secret::default(),
                script: Vec::new(),
                directory: PathBuf::new(),
            });
        } else {
            let path = PathBuf::from(
                tool.environment
                    .get(OsStr::new("WOR_IOT_ASKPASS_FILE"))
                    .unwrap(),
            );
            let metadata = std::fs::metadata(&path).unwrap();
            assert_eq!(metadata.mode() & 0o777, 0o600);
            assert_eq!(metadata.nlink(), 1);
            let password = read_askpass_file(&path).unwrap();
            assert!(
                !tool
                    .arguments
                    .iter()
                    .any(|arg| arg.to_string_lossy().contains(password.expose()))
            );
            assert!(tool.arguments.contains(&"StrictHostKeyChecking=yes".into()));
            assert!(
                tool.arguments
                    .contains(&"HostKeyAlias=wor-iot-paired".into())
            );
            assert!(
                tool.arguments
                    .contains(&"GlobalKnownHostsFile=/dev/null".into())
            );
            assert!(tool.arguments.contains(&"PubkeyAuthentication=no".into()));
            assert_eq!(
                tool.environment
                    .get(OsStr::new("SSH_ASKPASS_REQUIRE"))
                    .unwrap(),
                "force"
            );
            let directory = path.parent().unwrap().to_owned();
            assert_eq!(
                tool.working_directory.unwrap().metadata().unwrap().ino(),
                std::fs::metadata(&directory).unwrap().ino()
            );
            assert!(
                tool.arguments
                    .contains(&"UserKnownHostsFile=known_hosts".into())
            );
            assert_eq!(std::fs::metadata(&directory).unwrap().mode() & 0o777, 0o700);
            let known = std::fs::read_to_string(directory.join("known_hosts")).unwrap();
            assert_eq!(
                known,
                format!("wor-iot-paired ssh-ed25519 {}\n", pairing().key)
            );
            assert_eq!(
                std::fs::metadata(directory.join("known_hosts"))
                    .unwrap()
                    .mode()
                    & 0o777,
                0o600
            );
            let askpass = PathBuf::from(tool.environment.get(OsStr::new("SSH_ASKPASS")).unwrap());
            assert_eq!(askpass.file_name().unwrap(), "wor-iot-askpass");
            assert!(
                std::fs::symlink_metadata(askpass)
                    .unwrap()
                    .file_type()
                    .is_symlink()
            );
            assert!(!String::from_utf8_lossy(tool.stdin).contains(DESIRED));
            self.calls.borrow_mut().push(Call {
                args: tool.arguments.to_owned(),
                password,
                script: tool.stdin.to_owned(),
                directory: directory.clone(),
            });
            if self.damage_state {
                std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o755))
                    .unwrap();
            }
        }
        self.replies
            .borrow_mut()
            .pop_front()
            .ok_or_else(|| AccountError::invalid("Unexpected extra SSH call"))
    }
}

#[test]
fn source_oracle_payloads_preserve_admin_defaultaccount_and_cleanup_guards() {
    let source = include_str!("../../../src/lib/iot-account.py");
    assert!(source.contains("MAX_INPUT = 32768"));
    assert!(source.contains("AUTOMATIC_HOSTNAME = \"minwinpc\""));
    assert!(ACCOUNT_PAYLOAD.contains("NetUserGetInfo"));
    assert!(ACCOUNT_PAYLOAD.contains("$localSid.Value -ne $sid"));
    assert!(ACCOUNT_PAYLOAD.contains("-500$"));
    assert!(ACCOUNT_PAYLOAD.contains("ZeroFreeGlobalAllocUnicode"));
    for marker in [
        "LogonType = 3",
        "RunLevel = 0",
        "$defaultSid",
        "$null, 3, $null",
        "DeleteTask",
        "task-cleanup",
    ] {
        assert!(LOCALE_PAYLOAD.contains(marker), "{marker}");
    }
    let locale = std::str::from_utf8(LOCALE_USER_PAYLOAD).unwrap();
    for marker in [
        "-503$",
        "IoTSettings.exe",
        "list uilanguage",
        "get uilanguage",
        "set uilanguage",
        "pending-reboot",
    ] {
        assert!(locale.contains(marker));
    }
    assert_eq!(factory_login_defaults().current_password, "p@ssw0rd");
}

#[test]
fn preferences_defaults_explicit_empty_metadata_and_utf16_passwords() {
    let value = account_preferences(&serde_json::json!({}), false).unwrap();
    assert!(!value.account_setup);
    assert!(!value.language_setup);
    assert!(value.connection.automatic_address);
    assert_eq!(value.account_password.expose(), "");
    assert_eq!(value.connection.current_password.expose(), FACTORY_PASSWORD);
    assert_eq!(value.connection.current_username, FACTORY_USERNAME);
    assert!(
        account_preferences(
            &serde_json::json!({"accountSetup":true,"accountUsername":"FixtureAdmin"}),
            true
        )
        .is_ok()
    );
    assert!(
        account_preferences(
            &serde_json::json!({"accountSetup":true,"accountUsername":"FixtureAdmin"}),
            false
        )
        .is_err()
    );
    for value in [
        serde_json::json!({"currentPassword":""}),
        serde_json::json!({"currentPassword":null}),
        serde_json::json!({"currentUsername":""}),
        serde_json::json!({"currentUsername":"DefaultAccount"}),
        serde_json::json!({"automaticAddress":"true"}),
        serde_json::json!({"automaticAddress":1}),
        serde_json::json!({"host":"","automaticAddress":false}),
        serde_json::json!({"host":"https://192.168.1.1"}),
        serde_json::json!({"host":"a\ninjected"}),
        serde_json::json!({"language":"fr-FR;evil"}),
    ] {
        assert!(account_preferences(&value, false).is_err());
    }
    assert!(validate_password(&"😀".repeat(6), true).is_ok());
    assert!(validate_password(&"😀".repeat(64), true).is_err());
    assert!(validate_password("Fixture\npassword123", true).is_err());
    assert_eq!(normalize_language("zh-hans-cn").unwrap(), "zh-Hans-CN");
    for username in [
        "DefaultAccount",
        "Guest",
        "SYSTEM",
        "WDAGUtilityAccount",
        "../Admin",
        "bad&name",
        "                     ",
    ] {
        assert!(validate_username(username).is_err());
    }
}

#[test]
fn configure_does_not_silently_use_factory_password() {
    let root = temporary();
    let tools = Tools::new(Vec::new());
    let resolver = Resolver::local();
    let service = AccountService {
        resolver: &resolver,
        tools: &tools,
    };
    let value = serde_json::json!({"pairing":pairing(),"confirmIdentity":true,"accountUsername":"FixtureAdmin","accountPassword":DESIRED});
    let request = ConfigureRequest::from_json(&value).unwrap();
    assert_eq!(request.current_password.expose(), "");
    assert!(
        service
            .configure_account(&request, &options(root.path()), &AtomicBool::new(false))
            .is_err()
    );
    assert!(resolver.hosts.borrow().is_empty());
    assert!(tools.calls.borrow().is_empty());
}

#[test]
fn automatic_lookup_is_exact_deduplicated_local_and_never_a_scan() {
    let root = temporary();
    let mut resolver = Resolver::local();
    resolver.answers.push(resolver.answers[0].clone());
    let tools = Tools::new(vec![
        format!("{ADDRESS} ssh-ed25519 {}\n", pairing().key).into_bytes(),
    ]);
    let service = AccountService {
        resolver: &resolver,
        tools: &tools,
    };
    assert_eq!(
        service
            .probe_device(
                "remembered-manual.local",
                true,
                &options(root.path()),
                &AtomicBool::new(false)
            )
            .unwrap(),
        pairing()
    );
    assert_eq!(*resolver.hosts.borrow(), ["minwinpc"]);
    assert_eq!(tools.calls.borrow()[0].args.last().unwrap(), ADDRESS);
}

#[test]
fn ambiguous_public_and_scoped_ipv6_addresses_fail_before_credentials() {
    let root = temporary();
    for addresses in [
        vec!["8.8.8.8", ADDRESS],
        vec!["127.0.0.1"],
        vec!["0.0.0.0"],
        vec!["224.1.2.3"],
        vec!["2001:4860:4860::8888"],
        vec!["::1"],
        vec!["::"],
        vec!["ff02::1"],
        vec!["100.64.0.1"],
        vec!["192.0.0.9"],
        vec!["192.0.0.10"],
        vec!["2001:1::1"],
        vec!["2001:1::2"],
        vec!["2001:3::1"],
        vec!["2001:4:112::1"],
        vec!["2001:20::1"],
        vec!["::ffff:8.8.8.8"],
        vec![ADDRESS, "192.168.50.24"],
    ] {
        let resolver = Resolver {
            answers: addresses
                .iter()
                .map(|address| ResolvedAddress {
                    address: address.parse().unwrap(),
                    scope_id: 0,
                })
                .collect(),
            hosts: RefCell::new(Vec::new()),
        };
        let tools = Tools::new(Vec::new());
        let service = AccountService {
            resolver: &resolver,
            tools: &tools,
        };
        assert!(
            service
                .probe_device("", true, &options(root.path()), &AtomicBool::new(false))
                .is_err()
        );
        assert!(tools.calls.borrow().is_empty());
    }
    let mut resolver = Resolver {
        answers: vec![ResolvedAddress {
            address: IpAddr::V6("fe80::23".parse::<Ipv6Addr>().unwrap()),
            scope_id: 7,
        }],
        hosts: RefCell::new(Vec::new()),
    };
    resolver.answers.push(resolver.answers[0].clone());
    assert_eq!(
        local_address("fe80::23%fixture0", true, &resolver).unwrap(),
        "fe80::23%7"
    );
    resolver.answers[1].scope_id = 8;
    assert!(local_address("minwinpc", true, &resolver).is_err());
    let mut request = request();
    request.pairing.address = "fe80::23%8".into();
    resolver.answers.truncate(1);
    let tools = Tools::new(Vec::new());
    assert!(
        AccountService {
            resolver: &resolver,
            tools: &tools
        }
        .configure_account(&request, &options(root.path()), &AtomicBool::new(false))
        .is_err()
    );
    assert!(tools.calls.borrow().is_empty());
}

#[test]
fn inconsistent_fingerprint_no_consent_and_invalid_keys_never_send_credentials() {
    let root = temporary();
    let resolver = Resolver::local();
    let tools = Tools::new(Vec::new());
    let service = AccountService {
        resolver: &resolver,
        tools: &tools,
    };
    let mut request = request();
    request.confirm_identity = false;
    assert!(
        service
            .configure_account(&request, &options(root.path()), &AtomicBool::new(false))
            .is_err()
    );
    request.confirm_identity = true;
    request.pairing.fingerprint = "SHA256:wrong".into();
    assert!(
        service
            .configure_account(&request, &options(root.path()), &AtomicBool::new(false))
            .is_err()
    );
    assert!(tools.calls.borrow().is_empty());
    for (kind, key) in [
        ("unknown", "aaaa".into()),
        ("ssh-ed25519", "not base64".into()),
        ("ssh-rsa", pairing().key),
        ("ssh-ed25519", STANDARD.encode([0, 0, 255, 255])),
    ] {
        assert!(key_fingerprint(kind, &key).is_err());
    }
}

#[test]
fn keyscan_conflicts_are_refused_and_preference_is_ed25519() {
    let root = temporary();
    let resolver = Resolver::local();
    let conflict = format!(
        "{ADDRESS} ssh-ed25519 {}\n{ADDRESS} ssh-ed25519 {}\n",
        key("ssh-ed25519", 0),
        key("ssh-ed25519", 1)
    );
    let tools = Tools::new(vec![conflict.into_bytes()]);
    assert!(
        AccountService {
            resolver: &resolver,
            tools: &tools
        }
        .probe_device(
            ADDRESS,
            false,
            &options(root.path()),
            &AtomicBool::new(false)
        )
        .is_err()
    );
    let multiple = format!(
        "{ADDRESS} ssh-rsa {}\n{ADDRESS} ssh-ed25519 {}\n",
        key("ssh-rsa", 0),
        pairing().key
    );
    let tools = Tools::new(vec![multiple.into_bytes()]);
    assert_eq!(
        AccountService {
            resolver: &resolver,
            tools: &tools
        }
        .probe_device(
            ADDRESS,
            false,
            &options(root.path()),
            &AtomicBool::new(false)
        )
        .unwrap()
        .key_type,
        "ssh-ed25519"
    );
}

#[test]
fn two_pinned_logins_verify_same_sid_using_new_credentials_and_clean_state() {
    let root = temporary();
    let resolver = Resolver::local();
    let tools = Tools::new(vec![
        account_result("applied", SID, "FixtureAdmin", true),
        account_result("verified", SID, "FixtureAdmin", false),
    ]);
    let service = AccountService {
        resolver: &resolver,
        tools: &tools,
    };
    let result = service
        .configure_account(&request(), &options(root.path()), &AtomicBool::new(false))
        .unwrap();
    assert_eq!(result.state, SetupState::Verified);
    assert_eq!(result.account_state, SetupState::Verified);
    assert_eq!(result.language_state, SetupState::NotAssessed);
    assert_eq!(result.sid.as_deref(), Some(SID));
    let calls = tools.calls.borrow();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].password.expose(), CURRENT);
    assert_eq!(calls[1].password.expose(), DESIRED);
    assert!(
        calls[0]
            .args
            .contains(&format!("Administrator@{ADDRESS}").into())
    );
    assert!(
        calls[1]
            .args
            .contains(&format!("FixtureAdmin@{ADDRESS}").into())
    );
    for call in calls.iter() {
        assert!(!call.directory.exists());
        assert!(String::from_utf8_lossy(&call.script).contains("NetUserSetInfo"));
    }
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
    let json = serde_json::to_string(&result).unwrap();
    assert!(!json.contains(DESIRED) && !json.contains(CURRENT));
}

#[test]
fn failed_partial_renamed_or_non_admin_identity_is_never_verified() {
    for response in [
        account_result("failed", SID, "Administrator", false),
        account_result("failed", SID, "Administrator", true),
        account_result("applied", SID, "AnotherAccount", true),
        account_result("applied", "S-1-5-21-1-2-3-501", "FixtureAdmin", true),
        account_result("applied", "S-1--500", "FixtureAdmin", true),
    ] {
        let root = temporary();
        let resolver = Resolver::local();
        let tools = Tools::new(vec![response]);
        let error = AccountService {
            resolver: &resolver,
            tools: &tools,
        }
        .configure_account(&request(), &options(root.path()), &AtomicBool::new(false))
        .unwrap_err();
        assert!(!error.to_string().contains(DESIRED));
        assert_eq!(tools.calls.borrow().len(), 1);
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
    }
}

#[test]
fn failed_reconnect_or_changed_sid_stops_without_retry_under_old_credentials() {
    for response in [
        account_result("verified", "S-1-5-21-4-5-6-500", "FixtureAdmin", false),
        b"unstructured output".to_vec(),
    ] {
        let root = temporary();
        let resolver = Resolver::local();
        let tools = Tools::new(vec![
            account_result("applied", SID, "FixtureAdmin", true),
            response,
        ]);
        assert!(
            AccountService {
                resolver: &resolver,
                tools: &tools
            }
            .configure_account(&request(), &options(root.path()), &AtomicBool::new(false))
            .is_err()
        );
        assert_eq!(tools.calls.borrow().len(), 2);
        assert_eq!(tools.calls.borrow()[1].password.expose(), DESIRED);
    }
}

#[test]
fn language_only_and_pending_reboot_do_not_mutate_administrator() {
    for (state, expected) in [
        ("verified", SetupState::Verified),
        ("pending-reboot", SetupState::PendingReboot),
    ] {
        let root = temporary();
        let resolver = Resolver::local();
        let tools = Tools::new(vec![language_result(
            state,
            "language-verification",
            "fr-FR",
        )]);
        let mut request = request();
        request.account_setup = false;
        request.account_password = Secret::default();
        request.language_setup = true;
        request.language = "fr-fr".into();
        let result = AccountService {
            resolver: &resolver,
            tools: &tools,
        }
        .configure_account(&request, &options(root.path()), &AtomicBool::new(false))
        .unwrap();
        assert_eq!(result.state, expected);
        assert_eq!(result.account_state, SetupState::NotAssessed);
        assert_eq!(result.language_state, expected);
        assert_eq!(result.language, "fr-FR");
        assert_eq!(tools.calls.borrow().len(), 1);
        assert!(
            !String::from_utf8_lossy(&tools.calls.borrow()[0].script).contains("NetUserSetInfo")
        );
    }
}

#[test]
fn language_must_be_installed_correct_and_cleaned_before_account_mutation() {
    for result in [
        language_result("unsupported", "language-support", "fr-FR"),
        language_result("failed", "task-cleanup", "fr-FR"),
        language_result("verified", "task-cleanup", "fr-FR"),
        language_result("verified", "language-verification", "de-DE"),
        language_result("verified", "default-account-guard", "fr-FR"),
        language_result("verified", "language-verification", "fr-FR"),
    ]
    .into_iter()
    .enumerate()
    {
        let root = temporary();
        let resolver = Resolver::local();
        let mut output = result.1;
        if result.0 == 5 {
            let text = String::from_utf8(output)
                .unwrap()
                .replace("[\"en-US\",\"fr-FR\"]", "[]");
            output = text.into_bytes();
        }
        let tools = Tools::new(vec![output]);
        let mut request = request();
        request.language_setup = true;
        request.language = "fr-FR".into();
        assert!(
            AccountService {
                resolver: &resolver,
                tools: &tools
            }
            .configure_account(&request, &options(root.path()), &AtomicBool::new(false))
            .is_err()
        );
        assert_eq!(tools.calls.borrow().len(), 1);
    }
}

#[test]
fn language_precedes_apply_and_reconnect_and_pending_state_survives_account_success() {
    let root = temporary();
    let resolver = Resolver::local();
    let tools = Tools::new(vec![
        language_result("pending-reboot", "language-verification", "fr-FR"),
        account_result("applied", SID, "FixtureAdmin", true),
        account_result("verified", SID, "FixtureAdmin", false),
    ]);
    let mut request = request();
    request.language_setup = true;
    request.language = "fr-FR".into();
    let result = AccountService {
        resolver: &resolver,
        tools: &tools,
    }
    .configure_account(&request, &options(root.path()), &AtomicBool::new(false))
    .unwrap();
    assert_eq!(result.state, SetupState::PendingReboot);
    assert_eq!(result.account_state, SetupState::Verified);
    assert_eq!(result.language_state, SetupState::PendingReboot);
    assert!(String::from_utf8_lossy(&tools.calls.borrow()[0].script).contains(LOCALE_MARKER));
    assert!(String::from_utf8_lossy(&tools.calls.borrow()[1].script).contains("NetUserSetInfo"));
}

#[test]
fn output_schema_ansi_ambiguity_boolean_and_code_bounds_are_strict() {
    let mut good = b"\x1b[?1h\x1b[?1l".to_vec();
    good.extend(account_result("verified", SID, "FixtureAdmin", false));
    assert_eq!(parse_response(&good).unwrap().state, AccountState::Verified);
    let duplicate = account_result("verified", SID, "FixtureAdmin", false).repeat(2);
    for bytes in [
        Vec::new(),
        b"anything".to_vec(),
        duplicate,
        b"WOR_IOT_ACCOUNT_JSON:{}\n".to_vec(),
        vec![b'X'; 1024 * 1024 + 1],
    ] {
        assert!(parse_response(&bytes).is_err());
    }
    for changed in ["-1", "4294967296", "true"] {
        let value = String::from_utf8(account_result("verified", SID, "FixtureAdmin", false))
            .unwrap()
            .replace("\"code\":0", &format!("\"code\":{changed}"));
        assert!(parse_response(value.as_bytes()).is_err());
    }
    assert!(parse_locale_response(b"WOR_IOT_LOCALE_JSON:{}\n").is_err());
}

#[test]
fn passwords_are_encoded_data_and_windows_payloads_are_not_host_logic() {
    let secret = Secret::new(DESIRED);
    let script = remote_script(AccountOperation::Apply, "FixtureAdmin", &secret).unwrap();
    assert!(!script.contains(DESIRED));
    let encoded = script
        .split("FromBase64String('")
        .nth(1)
        .unwrap()
        .split("')")
        .next()
        .unwrap();
    let data: Value = serde_json::from_slice(&STANDARD.decode(encoded).unwrap()).unwrap();
    assert_eq!(data["password"], DESIRED);
    assert_eq!(data["operation"], "apply");
    assert!(script.ends_with("\n\n"));
    let language = locale_script("fr-FR").unwrap();
    let encoded = language
        .split("FromBase64String('")
        .nth(2)
        .unwrap()
        .split("')")
        .next()
        .unwrap();
    assert_eq!(STANDARD.decode(encoded).unwrap(), LOCALE_USER_PAYLOAD);
}

#[test]
fn private_askpass_refuses_symlinks_public_hardlinks_and_bad_input() {
    let root = temporary();
    let path = root.path().join("password");
    std::fs::write(&path, CURRENT).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(read_askpass_file(&path).unwrap().expose(), CURRENT);
    let link = root.path().join("symlink");
    symlink(&path, &link).unwrap();
    assert!(read_askpass_file(&link).is_err());
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(read_askpass_file(&path).is_err());
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    std::fs::hard_link(&path, root.path().join("hardlink")).unwrap();
    assert!(read_askpass_file(&path).is_err());
    assert_eq!(askpass(OsStr::new("Accept host key?")).unwrap(), 1);
}

#[test]
fn temporary_state_mutation_cleanup_failure_and_cancellation_cannot_verify() {
    let root = temporary();
    let resolver = Resolver::local();
    let mut tools = Tools::new(vec![account_result("applied", SID, "FixtureAdmin", true)]);
    tools.damage_state = true;
    assert!(
        AccountService {
            resolver: &resolver,
            tools: &tools
        }
        .configure_account(&request(), &options(root.path()), &AtomicBool::new(false))
        .is_err()
    );
    assert_eq!(tools.calls.borrow().len(), 1);
    let tools = Tools::new(Vec::new());
    assert!(matches!(
        AccountService {
            resolver: &resolver,
            tools: &tools
        }
        .configure_account(&request(), &options(root.path()), &AtomicBool::new(true)),
        Err(AccountError::Cancelled)
    ));
    assert!(tools.calls.borrow().is_empty());
}

struct InteractionFixture {
    lines: VecDeque<String>,
    passwords: VecDeque<Secret>,
    shows: Vec<String>,
    password_prompts: Vec<String>,
}
impl Interaction for InteractionFixture {
    fn line(&mut self, _: &str) -> Result<String> {
        Ok(self.lines.pop_front().unwrap())
    }
    fn password(&mut self, prompt: &str) -> Result<Secret> {
        self.password_prompts.push(prompt.into());
        Ok(self.passwords.pop_front().unwrap())
    }
    fn show(&mut self, message: &str) -> Result<()> {
        self.shows.push(message.into());
        Ok(())
    }
}

#[test]
fn interactive_factory_default_is_only_after_explicit_fingerprint_confirmation() {
    for (name, password, expected_name, expected_password) in [
        ("", "", "Administrator", FACTORY_PASSWORD),
        ("Administrator", CURRENT, "Administrator", CURRENT),
        ("RenamedAdmin", CURRENT, "RenamedAdmin", CURRENT),
    ] {
        let root = temporary();
        let resolver = Resolver::local();
        let tools = Tools::new(vec![
            format!("{ADDRESS} ssh-ed25519 {}\n", pairing().key).into_bytes(),
        ]);
        let mut interaction = InteractionFixture {
            lines: [ADDRESS, "CONFIRM", name].map(str::to_owned).into(),
            passwords: [Secret::new(password)].into(),
            shows: Vec::new(),
            password_prompts: Vec::new(),
        };
        let selected = interactive_request(
            &AccountService {
                resolver: &resolver,
                tools: &tools,
            },
            &options(root.path()),
            &mut interaction,
            Some("en-US"),
            true,
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(selected.current_username, expected_name);
        assert_eq!(selected.current_password.expose(), expected_password);
        assert!(!selected.account_setup);
        assert_eq!(selected.account_password.expose(), "");
        assert!(
            interaction
                .shows
                .iter()
                .all(|show| !show.contains(CURRENT) && !show.contains(DESIRED))
        );
        assert_eq!(
            interaction.password_prompts[0].contains("factory default"),
            expected_name == FACTORY_USERNAME
        );
    }
    let root = temporary();
    let resolver = Resolver::local();
    let tools = Tools::new(vec![
        format!("{ADDRESS} ssh-ed25519 {}\n", pairing().key).into_bytes(),
    ]);
    let mut interaction = InteractionFixture {
        lines: [ADDRESS, "NO"].map(str::to_owned).into(),
        passwords: VecDeque::new(),
        shows: Vec::new(),
        password_prompts: Vec::new(),
    };
    assert!(
        interactive_request(
            &AccountService {
                resolver: &resolver,
                tools: &tools
            },
            &options(root.path()),
            &mut interaction,
            None,
            false,
            &AtomicBool::new(false)
        )
        .is_err()
    );
    assert!(interaction.password_prompts.is_empty());
}

#[test]
fn json_input_is_bounded_and_reports_no_secret_on_schema_errors() {
    for bytes in [
        b"{not valid".to_vec(),
        b"[]".to_vec(),
        vec![b' '; MAX_INPUT + 1],
    ] {
        assert!(read_json(&mut bytes.as_slice()).is_err());
    }
    let malformed = serde_json::json!({"pairing": {"fingerprint":DESIRED}});
    assert!(
        ConfigureRequest::from_json(&malformed)
            .err()
            .unwrap()
            .to_string()
            .find(DESIRED)
            .is_none()
    );
    assert!(matches!(
        local_address(
            "bad-host",
            false,
            &Resolver {
                answers: Vec::new(),
                hosts: RefCell::new(Vec::new())
            }
        ),
        Err(AccountError::Invalid(_))
    ));
    assert_eq!(
        ResolvedAddress {
            address: IpAddr::V4(Ipv4Addr::new(192, 168, 1, 2)),
            scope_id: 0
        }
        .numeric(),
        "192.168.1.2"
    );
}

#[test]
fn fixed_native_tool_provider_bounds_output_deadlines_and_private_stdin() {
    let tools = SystemTools;
    let environment = BTreeMap::new();
    let empty = Vec::new();
    let result = tools
        .run(
            ToolRequest {
                executable: Path::new("/bin/cat"),
                arguments: &empty,
                environment: &environment,
                working_directory: None,
                stdin: CURRENT.as_bytes(),
                timeout: Duration::from_secs(10),
                stdout_limit: 1024,
            },
            &AtomicBool::new(false),
        )
        .unwrap();
    assert_eq!(result.status, 0);
    assert_eq!(result.stdout, CURRENT.as_bytes());
    let error = tools
        .run(
            ToolRequest {
                executable: Path::new("/usr/bin/yes"),
                arguments: &empty,
                environment: &environment,
                working_directory: None,
                stdin: &[],
                timeout: Duration::from_secs(10),
                stdout_limit: 1024,
            },
            &AtomicBool::new(false),
        )
        .err()
        .unwrap();
    assert!(error.to_string().contains("excessive"));
    assert!(!format!("{error:?}").contains(CURRENT));
    let arguments = vec![OsString::from("5")];
    let error = tools
        .run(
            ToolRequest {
                executable: Path::new("/bin/sleep"),
                arguments: &arguments,
                environment: &environment,
                working_directory: None,
                stdin: &[],
                timeout: Duration::from_millis(20),
                stdout_limit: 1024,
            },
            &AtomicBool::new(false),
        )
        .err()
        .unwrap();
    assert!(error.to_string().contains("timed out"));
    let cancel = Arc::new(AtomicBool::new(false));
    let interrupt = cancel.clone();
    let worker = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(20));
        interrupt.store(true, Ordering::SeqCst);
    });
    let result = tools.run(
        ToolRequest {
            executable: Path::new("/bin/sleep"),
            arguments: &arguments,
            environment: &environment,
            working_directory: None,
            stdin: &[],
            timeout: Duration::from_secs(5),
            stdout_limit: 1024,
        },
        &cancel,
    );
    worker.join().unwrap();
    assert!(matches!(result, Err(AccountError::Cancelled)));
}

#[test]
fn strict_known_hosts_and_native_askpass_cannot_change_mid_operation() {
    let root = temporary();
    let state =
        state::PrivateState::new(root.path(), &std::env::current_exe().unwrap(), &pairing())
            .unwrap();
    let known = state.path.join("known_hosts");
    std::fs::write(&known, "wor-iot-paired ssh-ed25519 substituted\n").unwrap();
    assert!(state.check().is_err());
    assert!(state.close().is_err());
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
    let state =
        state::PrivateState::new(root.path(), &std::env::current_exe().unwrap(), &pairing())
            .unwrap();
    let askpass = state.path.join("wor-iot-askpass");
    std::fs::remove_file(&askpass).unwrap();
    symlink("/usr/bin/false", &askpass).unwrap();
    assert!(state.check().is_err());
    assert!(state.close().is_err());
}

#[test]
fn native_error_and_exit_contracts_do_not_reflect_untrusted_output() {
    let root = temporary();
    let resolver = Resolver::local();
    let tools = Tools {
        replies: RefCell::new(
            [ToolOutput {
                status: 255,
                stdout: DESIRED.as_bytes().to_vec(),
            }]
            .into(),
        ),
        calls: RefCell::new(Vec::new()),
        damage_state: false,
    };
    let error = AccountService {
        resolver: &resolver,
        tools: &tools,
    }
    .configure_account(&request(), &options(root.path()), &AtomicBool::new(false))
    .unwrap_err();
    assert!(!format!("{error:?}").contains(DESIRED));
    assert!(error.to_string().contains("may already"));
    assert_eq!(error.exit_status(), 1);
    assert_eq!(AccountError::Cancelled.exit_status(), 130);
    for args in [
        vec!["unknown"],
        vec!["probe", "--unsupported"],
        vec!["configure", "--language"],
    ] {
        assert_eq!(
            run(&args.into_iter().map(OsString::from).collect::<Vec<_>>()).unwrap(),
            2
        );
    }
}
