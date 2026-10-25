//! Native, opt-in IoT administrator/language setup through fingerprint-pinned SSH.
//! Credentials are stdin/private-file data, never subprocess arguments or diagnostics.

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
compile_error!("native IoT account setup supports Linux and macOS only");

mod network;
mod payload;
mod preferences;
mod process;
mod state;

pub use network::{EndpointResolver, ResolvedAddress, SystemResolver, local_address};
pub use payload::{
    ACCOUNT_PAYLOAD, AccountOperation, AccountResponse, AccountStage, AccountState, LOCALE_MARKER,
    LOCALE_PAYLOAD, LOCALE_USER_PAYLOAD, LanguageResponse, LanguageStage, LanguageState, MARKER,
    ManagementScript, locale_script, parse_locale_response, parse_response, remote_script,
};
pub use preferences::{
    AUTOMATIC_HOSTNAME, AccountPreferences, FACTORY_PASSWORD, FACTORY_USERNAME, FactoryDefaults,
    LoginPreferences, account_preferences, factory_login_defaults, login_preferences,
    normalize_language, validate_host_input, validate_password, validate_username,
};
pub use process::{NativeTools, SystemTools, ToolOutput, ToolRequest};
pub use state::read_askpass_file;

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;
use zeroize::Zeroize;

pub type Result<T> = std::result::Result<T, AccountError>;
const MAX_INPUT: usize = 32768;
const KEY_TYPES: [&str; 3] = ["ssh-ed25519", "ecdsa-sha2-nistp256", "ssh-rsa"];

#[derive(Debug, thiserror::Error)]
pub enum AccountError {
    #[error("{0}")]
    Invalid(String),
    #[error("IoT account setup encountered an I/O or encoding error.")]
    Io(#[from] std::io::Error),
    #[error(
        "IoT account setup interrupted; credentials may have changed. Verify the old/new login before retrying."
    )]
    Cancelled,
}
impl AccountError {
    pub fn exit_status(&self) -> i32 {
        if matches!(self, Self::Cancelled) {
            130
        } else {
            1
        }
    }
    pub(crate) fn invalid(message: impl Into<String>) -> Self {
        Self::Invalid(message.into())
    }
}

/// No Display/Debug implementation. Serialization is only for the existing private
/// `preferences` JSON protocol; callers must never forward that JSON to logs.
///
/// ```compile_fail
/// use wor_iot_account::Secret;
/// let private = Secret::new("fixture-only-password");
/// println!("{private:?}");
/// ```
#[derive(Clone, Default)]
pub struct Secret(String);
impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
    pub fn expose(&self) -> &str {
        &self.0
    }
}
impl Drop for Secret {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}
impl Serialize for Secret {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}
impl<'de> Deserialize<'de> for Secret {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        String::deserialize(deserializer).map(Self)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Pairing {
    pub address: String,
    pub key_type: String,
    pub key: String,
    pub fingerprint: String,
}

pub fn key_fingerprint(key_type: &str, key: &str) -> Result<String> {
    if !KEY_TYPES.contains(&key_type) || key.len() > 16384 {
        return Err(AccountError::invalid("Invalid SSH host key."));
    }
    let raw = STANDARD
        .decode(key)
        .map_err(|_| AccountError::invalid("Invalid SSH host key."))?;
    if raw.len() < 4 {
        return Err(AccountError::invalid("Invalid SSH host key."));
    }
    let length = u32::from_be_bytes(raw[..4].try_into().expect("bounded SSH key header")) as usize;
    if raw.get(4..4usize.saturating_add(length)) != Some(key_type.as_bytes()) {
        return Err(AccountError::invalid(
            "SSH host key type does not match its key data.",
        ));
    }
    Ok(format!(
        "SHA256:{}",
        STANDARD_NO_PAD.encode(Sha256::digest(&raw))
    ))
}

#[derive(Clone)]
pub struct ConfigureRequest {
    pub pairing: Pairing,
    pub confirm_identity: bool,
    pub current_username: String,
    pub current_password: Secret,
    pub account_setup: bool,
    pub account_username: String,
    pub account_password: Secret,
    pub language_setup: bool,
    pub language: String,
}

impl ConfigureRequest {
    pub fn from_json(value: &Value) -> Result<Self> {
        preferences::object(value)?;
        let current_username = preferences::text(value, "currentUsername", FACTORY_USERNAME)?;
        let pairing = serde_json::from_value(value.get("pairing").cloned().unwrap_or(Value::Null))
            .map_err(|_| {
                AccountError::invalid("Confirm the Pi's SSH identity before sending credentials.")
            })?;
        Ok(Self {
            pairing,
            confirm_identity: preferences::boolean(value, "confirmIdentity", false)?,
            current_password: Secret::new(preferences::text(value, "currentPassword", "")?),
            account_username: preferences::text(value, "accountUsername", &current_username)?,
            current_username,
            account_password: Secret::new(preferences::text(value, "accountPassword", "")?),
            account_setup: preferences::boolean(value, "accountSetup", true)?,
            language_setup: preferences::boolean(value, "languageSetup", false)?,
            language: normalize_language(&preferences::text(value, "language", "en-US")?)?,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SetupState {
    Verified,
    PendingReboot,
    NotAssessed,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfiguredAccount {
    pub state: SetupState,
    pub username: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sid: Option<String>,
    pub address: String,
    pub account_state: SetupState,
    pub language_state: SetupState,
    pub language: String,
}

#[derive(Clone, Debug)]
pub struct AccountOptions {
    /// Native CLI that recognizes the `wor-iot-askpass` argv[0] basename.
    pub companion_executable: PathBuf,
    /// Existing current-user directory. No default system temporary directory is used.
    pub private_state_root: PathBuf,
    pub ssh_executable: PathBuf,
    pub keyscan_executable: PathBuf,
}
impl AccountOptions {
    pub fn new() -> Result<Self> {
        Ok(Self {
            companion_executable: std::env::var_os("WOR_IOT_NATIVE_COMPANION")
                .map(PathBuf::from)
                .map(Ok)
                .unwrap_or_else(std::env::current_exe)?,
            private_state_root: std::env::var_os("HOME")
                .filter(|path| !path.is_empty())
                .map(PathBuf::from)
                .map(Ok)
                .unwrap_or_else(std::env::current_dir)?,
            ssh_executable: PathBuf::from("ssh"),
            keyscan_executable: PathBuf::from("ssh-keyscan"),
        })
    }
}

pub struct AccountService<'a> {
    pub resolver: &'a dyn EndpointResolver,
    pub tools: &'a dyn NativeTools,
}

fn check_cancel(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::SeqCst) {
        Err(AccountError::Cancelled)
    } else {
        Ok(())
    }
}
fn admin_sid(sid: &str) -> bool {
    let Some(rest) = sid.strip_prefix("S-1-") else {
        return false;
    };
    rest.ends_with("-500")
        && rest
            .split('-')
            .all(|part| !part.is_empty() && part.bytes().all(|c| c.is_ascii_digit()))
}

struct Login<'a> {
    address: &'a str,
    username: &'a str,
    password: &'a Secret,
}

impl AccountService<'_> {
    pub fn probe_device(
        &self,
        host: &str,
        automatic: bool,
        options: &AccountOptions,
        cancel: &AtomicBool,
    ) -> Result<Pairing> {
        check_cancel(cancel)?;
        let address = local_address(
            if automatic { AUTOMATIC_HOSTNAME } else { host },
            automatic,
            self.resolver,
        )?;
        check_cancel(cancel)?;
        let arguments =
            ["-T", "5", "-p", "22", "-t", "ed25519,ecdsa,rsa", &address].map(OsString::from);
        let response = self.tools.run(
            ToolRequest {
                executable: &options.keyscan_executable,
                arguments: &arguments,
                environment: &BTreeMap::new(),
                working_directory: None,
                stdin: &[],
                timeout: Duration::from_secs(15),
                stdout_limit: 65536,
            },
            cancel,
        )?;
        check_cancel(cancel)?;
        if response.status != 0 || response.stdout.len() > 65536 || !response.stdout.is_ascii() {
            return Err(AccountError::invalid(
                "The Pi's SSH service is unavailable or returned invalid identity data.",
            ));
        }
        let text = std::str::from_utf8(&response.stdout)
            .map_err(|_| AccountError::invalid("Invalid SSH host-key encoding."))?;
        let mut keys = BTreeMap::new();
        for line in text.lines() {
            if line.starts_with('#') {
                continue;
            }
            let fields: Vec<_> = line.split_whitespace().collect();
            if fields.len() == 3 && KEY_TYPES.contains(&fields[1]) {
                key_fingerprint(fields[1], fields[2])?;
                if keys
                    .insert(fields[1], fields[2])
                    .is_some_and(|old| old != fields[2])
                {
                    return Err(AccountError::invalid(
                        "The SSH scan returned conflicting device identities.",
                    ));
                }
            }
        }
        for key_type in KEY_TYPES {
            if let Some(key) = keys.get(key_type) {
                return Ok(Pairing {
                    address,
                    key_type: key_type.to_owned(),
                    key: (*key).to_owned(),
                    fingerprint: key_fingerprint(key_type, key)?,
                });
            }
        }
        Err(AccountError::invalid(
            "The Pi did not return a supported SSH host key.",
        ))
    }

    fn ssh(
        &self,
        state: &state::PrivateState,
        options: &AccountOptions,
        login: Login<'_>,
        script: &[u8],
        cancel: &AtomicBool,
    ) -> Result<Vec<u8>> {
        check_cancel(cancel)?;
        state.check()?;
        let secret = state.password(login.password)?;
        let mut arguments: Vec<OsString> = ["-F", "/dev/null", "-T"].map(OsString::from).to_vec();
        for option in [
            OsString::from("StrictHostKeyChecking=yes"),
            "HostKeyAlias=wor-iot-paired".into(),
            // SSH parses -o values again; a literal cwd-relative name avoids
            // interpreting spaces, percent tokens, or ${...} in a parent path.
            "UserKnownHostsFile=known_hosts".into(),
            "GlobalKnownHostsFile=/dev/null".into(),
            "PreferredAuthentications=password".into(),
            "PasswordAuthentication=yes".into(),
            "PubkeyAuthentication=no".into(),
            "KbdInteractiveAuthentication=no".into(),
            "NumberOfPasswordPrompts=1".into(),
            "ConnectTimeout=10".into(),
            "ConnectionAttempts=1".into(),
            "ControlMaster=no".into(),
            "ControlPath=none".into(),
            "ClearAllForwardings=yes".into(),
            "PermitLocalCommand=no".into(),
            "UpdateHostKeys=no".into(),
        ] {
            arguments.push("-o".into());
            arguments.push(option);
        }
        arguments.extend([
            "--".into(),
            format!("{}@{}", login.username, login.address).into(),
            "powershell.exe -NoLogo -NoProfile -NonInteractive -Command -".into(),
        ]);
        let environment = BTreeMap::from([
            (
                "SSH_ASKPASS".into(),
                state.path.join("wor-iot-askpass").into_os_string(),
            ),
            ("SSH_ASKPASS_REQUIRE".into(), "force".into()),
            ("WOR_IOT_ASKPASS_FILE".into(), secret.into_os_string()),
            (
                "DISPLAY".into(),
                std::env::var_os("DISPLAY").unwrap_or_else(|| "WoR-Flasher".into()),
            ),
        ]);
        let response = self.tools.run(
            ToolRequest {
                executable: &options.ssh_executable,
                arguments: &arguments,
                environment: &environment,
                working_directory: Some(&state.directory),
                stdin: script,
                timeout: Duration::from_secs(60),
                stdout_limit: 1024 * 1024,
            },
            cancel,
        );
        let cleanup = state.remove_password();
        cleanup?;
        let response = response?;
        check_cancel(cancel)?;
        if response.status != 0 {
            return Err(AccountError::invalid(
                "SSH login or account setup failed. Check the address, trusted key and current credentials; an update may already have occurred.",
            ));
        }
        Ok(response.stdout)
    }

    pub fn configure_account(
        &self,
        request: &ConfigureRequest,
        options: &AccountOptions,
        cancel: &AtomicBool,
    ) -> Result<ConfiguredAccount> {
        check_cancel(cancel)?;
        if !request.account_setup && !request.language_setup {
            return Err(AccountError::invalid(
                "Select an IoT account or language preference to apply.",
            ));
        }
        validate_username(&request.current_username)?;
        validate_username(&request.account_username)?;
        validate_password(request.current_password.expose(), false)?;
        if request.account_setup {
            validate_password(request.account_password.expose(), true)?;
        }
        let language = normalize_language(&request.language)?;
        if !request.confirm_identity {
            return Err(AccountError::invalid(
                "Confirm the Pi's SSH identity before sending credentials.",
            ));
        }
        if local_address(&request.pairing.address, false, self.resolver)? != request.pairing.address
        {
            return Err(AccountError::invalid("The paired device address changed."));
        }
        if key_fingerprint(&request.pairing.key_type, &request.pairing.key)?
            != request.pairing.fingerprint
        {
            return Err(AccountError::invalid(
                "The paired SSH identity is inconsistent.",
            ));
        }
        check_cancel(cancel)?;
        let state = state::PrivateState::new(
            &options.private_state_root,
            &options.companion_executable,
            &request.pairing,
        )?;
        let outcome = (|| {
            let mut language_state = SetupState::NotAssessed;
            if request.language_setup {
                let script = locale_script(&language)?;
                let result = parse_locale_response(&self.ssh(
                    &state,
                    options,
                    Login {
                        address: &request.pairing.address,
                        username: &request.current_username,
                        password: &request.current_password,
                    },
                    script.as_bytes(),
                    cancel,
                )?)?;
                if !result.language.eq_ignore_ascii_case(&language) {
                    return Err(AccountError::invalid(
                        "The Pi returned a different language than selected.",
                    ));
                }
                if result.state == LanguageState::Unsupported {
                    return Err(AccountError::invalid(format!(
                        "The selected IoT language {language} is not installed. No administrator credentials were changed; choose an installed language."
                    )));
                }
                if !matches!(
                    result.state,
                    LanguageState::Verified | LanguageState::PendingReboot
                ) || !matches!(
                    result.stage,
                    LanguageStage::LanguageSupport | LanguageStage::LanguageVerification
                ) || !result
                    .available
                    .iter()
                    .any(|available| available.eq_ignore_ascii_case(&language))
                {
                    return Err(AccountError::invalid(
                        "IoT language configuration failed or its cleanup could not be verified. Administrator credentials were not changed.",
                    ));
                }
                language_state = if result.state == LanguageState::PendingReboot {
                    SetupState::PendingReboot
                } else {
                    SetupState::Verified
                };
            }
            if !request.account_setup {
                return Ok(ConfiguredAccount {
                    state: language_state,
                    username: request.current_username.clone(),
                    sid: None,
                    address: request.pairing.address.clone(),
                    account_state: SetupState::NotAssessed,
                    language_state,
                    language,
                });
            }
            let script = remote_script(
                AccountOperation::Apply,
                &request.account_username,
                &request.account_password,
            )?;
            let result = parse_response(&self.ssh(
                &state,
                options,
                Login {
                    address: &request.pairing.address,
                    username: &request.current_username,
                    password: &request.current_password,
                },
                script.as_bytes(),
                cancel,
            )?)?;
            if result.state != AccountState::Applied {
                return Err(AccountError::invalid(format!(
                    "IoT account setup failed (Windows status {}; stage {}). {}",
                    result.code,
                    match result.stage {
                        AccountStage::DeviceGuard => "device-guard",
                        AccountStage::NativePreflight => "native-preflight",
                        AccountStage::PasswordUpdate => "password-update",
                        AccountStage::UsernameUpdate => "username-update",
                        AccountStage::LoginVerification => "login-verification",
                    },
                    if result.password_changed || result.username_changed {
                        "A partial change occurred; check the new password with the original/desired username."
                    } else {
                        "No completed account update was reported."
                    }
                )));
            }
            if !result
                .username
                .eq_ignore_ascii_case(&request.account_username)
                || !admin_sid(&result.sid)
            {
                return Err(AccountError::invalid(
                    "The Pi returned an unexpected administrator identity; account verification failed.",
                ));
            }
            let script = remote_script(
                AccountOperation::Verify,
                &request.account_username,
                &Secret::default(),
            )?;
            let verified = parse_response(&self.ssh(
                &state,
                options,
                Login {
                    address: &request.pairing.address,
                    username: &request.account_username,
                    password: &request.account_password,
                },
                script.as_bytes(),
                cancel,
            )?)?;
            if verified.state != AccountState::Verified
                || verified.sid != result.sid
                || !verified
                    .username
                    .eq_ignore_ascii_case(&request.account_username)
            {
                return Err(AccountError::invalid(
                    "The new IoT login could not be verified. Credentials may have changed; do not assume setup completed.",
                ));
            }
            Ok(ConfiguredAccount {
                state: if language_state == SetupState::PendingReboot {
                    SetupState::PendingReboot
                } else {
                    SetupState::Verified
                },
                username: request.account_username.clone(),
                sid: Some(result.sid),
                address: request.pairing.address.clone(),
                account_state: SetupState::Verified,
                language_state,
                language,
            })
        })();
        state.close()?;
        check_cancel(cancel)?;
        outcome
    }
}

pub fn probe_device(
    host: &str,
    automatic: bool,
    options: &AccountOptions,
    cancel: &AtomicBool,
) -> Result<Pairing> {
    AccountService {
        resolver: &SystemResolver,
        tools: &SystemTools,
    }
    .probe_device(host, automatic, options, cancel)
}
pub fn configure_account(
    request: &ConfigureRequest,
    options: &AccountOptions,
    cancel: &AtomicBool,
) -> Result<ConfiguredAccount> {
    AccountService {
        resolver: &SystemResolver,
        tools: &SystemTools,
    }
    .configure_account(request, options, cancel)
}

pub trait Interaction {
    fn line(&mut self, prompt: &str) -> Result<String>;
    fn password(&mut self, prompt: &str) -> Result<Secret>;
    fn show(&mut self, message: &str) -> Result<()>;
}

pub struct TerminalInteraction;
impl Interaction for TerminalInteraction {
    fn line(&mut self, prompt: &str) -> Result<String> {
        write!(std::io::stdout().lock(), "{prompt}")?;
        std::io::stdout().flush()?;
        let mut line = String::new();
        std::io::stdin().read_line(&mut line)?;
        Ok(line.trim().to_owned())
    }
    fn password(&mut self, prompt: &str) -> Result<Secret> {
        Ok(Secret::new(rpassword::prompt_password(prompt)?))
    }
    fn show(&mut self, message: &str) -> Result<()> {
        writeln!(std::io::stdout().lock(), "{message}")?;
        Ok(())
    }
}

pub fn interactive_request(
    service: &AccountService<'_>,
    options: &AccountOptions,
    interaction: &mut dyn Interaction,
    language: Option<&str>,
    language_only: bool,
    cancel: &AtomicBool,
) -> Result<ConfigureRequest> {
    let host = interaction.line("Pi local address [Enter for automatic minwinpc]: ")?;
    let pairing = service.probe_device(&host, host.is_empty(), options, cancel)?;
    interaction.show(&format!(
        "Resolved device: {}\nSSH identity: {}",
        pairing.address, pairing.fingerprint
    ))?;
    if interaction.line("Verify this is your Pi; type CONFIRM to continue: ")? != "CONFIRM" {
        return Err(AccountError::invalid(
            "Device identity was not confirmed; no credentials were sent.",
        ));
    }
    let current_username = interaction.line("Current IoT administrator [Administrator]: ")?;
    let current_username = if current_username.is_empty() {
        FACTORY_USERNAME.to_owned()
    } else {
        current_username
    };
    let factory = current_username.eq_ignore_ascii_case(FACTORY_USERNAME);
    let current_password = interaction.password(if factory {
        "Current IoT password [Enter for Microsoft factory default]: "
    } else {
        "Current IoT password: "
    })?;
    let current_password = if factory && current_password.expose().is_empty() {
        Secret::new(FACTORY_PASSWORD)
    } else {
        current_password
    };
    let account_username = if language_only {
        FACTORY_USERNAME.to_owned()
    } else {
        let name = interaction.line("New IoT administrator [Administrator]: ")?;
        if name.is_empty() {
            FACTORY_USERNAME.to_owned()
        } else {
            name
        }
    };
    let account_password = if language_only {
        Secret::default()
    } else {
        interaction.password("New IoT password (12-127 characters): ")?
    };
    Ok(ConfigureRequest {
        pairing,
        confirm_identity: true,
        current_username,
        current_password,
        account_setup: !language_only,
        account_username,
        account_password,
        language_setup: language.is_some(),
        language: normalize_language(language.unwrap_or("en-US"))?,
    })
}

fn read_json(input: &mut dyn Read) -> Result<Value> {
    let mut bytes = zeroize::Zeroizing::new(Vec::new());
    input.take(MAX_INPUT as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > MAX_INPUT {
        return Err(AccountError::invalid("IoT account request is too large."));
    }
    let value = serde_json::from_slice(&bytes)
        .map_err(|_| AccountError::invalid("Invalid IoT account request."))?;
    preferences::object(&value)?;
    Ok(value)
}

pub fn askpass(prompt: &OsStr) -> Result<i32> {
    if !prompt
        .to_string_lossy()
        .to_ascii_lowercase()
        .contains("password")
    {
        return Ok(1);
    }
    let Some(path) = std::env::var_os("WOR_IOT_ASKPASS_FILE") else {
        return Ok(1);
    };
    let password = match read_askpass_file(Path::new(&path)) {
        Ok(value) => value,
        Err(_) => return Ok(1),
    };
    writeln!(std::io::stdout().lock(), "{}", password.expose())?;
    Ok(0)
}

/// Existing helper commands and machine-readable status contract, without a Python runtime.
pub fn run(args: &[OsString]) -> Result<i32> {
    let usage = "usage: iot-account {login-defaults,preferences,probe,configure,askpass} [--interactive] [--metadata-only] [--language TAG] [--language-only]";
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        writeln!(std::io::stdout().lock(), "{usage}")?;
        return Ok(0);
    }
    let Some(command) = args.first().and_then(|arg| arg.to_str()) else {
        writeln!(std::io::stderr().lock(), "{usage}")?;
        return Ok(2);
    };
    if command == "askpass" {
        return if let Some(prompt) = args.get(1) {
            askpass(prompt)
        } else {
            Ok(1)
        };
    }
    if !["login-defaults", "preferences", "probe", "configure"].contains(&command) {
        writeln!(std::io::stderr().lock(), "{usage}")?;
        return Ok(2);
    }
    let mut interactive = false;
    let mut metadata_only = false;
    let mut language_only = false;
    let mut language = None;
    let mut prompt_seen = false;
    let mut index = 1;
    while index < args.len() {
        match args[index].to_str() {
            Some("--interactive") => interactive = true,
            Some("--metadata-only") => metadata_only = true,
            Some("--language-only") => language_only = true,
            Some("--language") => {
                index += 1;
                language = args
                    .get(index)
                    .and_then(|arg| arg.to_str())
                    .filter(|value| *value == "-" || !value.starts_with('-'));
                if language.is_none() {
                    writeln!(std::io::stderr().lock(), "{usage}")?;
                    return Ok(2);
                }
            }
            Some(value) if value.starts_with("--language=") => {
                language = Some(&value["--language=".len()..]);
            }
            Some(value) if !value.starts_with('-') && !prompt_seen => prompt_seen = true,
            _ => {
                writeln!(std::io::stderr().lock(), "{usage}")?;
                return Ok(2);
            }
        }
        index += 1;
    }
    if command == "login-defaults" {
        writeln!(
            std::io::stdout().lock(),
            "{}",
            serde_json::to_string(&factory_login_defaults()).expect("static defaults")
        )?;
        return Ok(0);
    }
    let cancel = Arc::new(AtomicBool::new(false));
    let signal = Arc::new(AtomicUsize::new(0));
    let mut registrations = Vec::new();
    for number in [signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM] {
        registrations.push(signal_hook::flag::register(number, cancel.clone())?);
        registrations.push(signal_hook::flag::register_usize(
            number,
            signal.clone(),
            number as usize,
        )?);
    }
    let result = (|| -> Result<Value> {
        let options = AccountOptions::new()?;
        let service = AccountService {
            resolver: &SystemResolver,
            tools: &SystemTools,
        };
        if interactive {
            if command != "configure" {
                return Err(AccountError::invalid(
                    "Interactive mode is only for post-boot account configuration.",
                ));
            }
            let request = interactive_request(
                &service,
                &options,
                &mut TerminalInteraction,
                language,
                language_only,
                &cancel,
            )?;
            return serde_json::to_value(service.configure_account(&request, &options, &cancel)?)
                .map_err(|_| AccountError::invalid("Cannot encode IoT configuration result."));
        }
        let value = read_json(&mut std::io::stdin().lock())?;
        match command {
            "preferences" => serde_json::to_value(account_preferences(&value, metadata_only)?)
                .map_err(|_| AccountError::invalid("Cannot encode private preferences.")),
            "probe" => {
                let host = preferences::text(&value, "host", "")?;
                let automatic = preferences::boolean(&value, "automaticAddress", host.is_empty())?;
                serde_json::to_value(service.probe_device(&host, automatic, &options, &cancel)?)
                    .map_err(|_| AccountError::invalid("Cannot encode SSH identity."))
            }
            _ => serde_json::to_value(service.configure_account(
                &ConfigureRequest::from_json(&value)?,
                &options,
                &cancel,
            )?)
            .map_err(|_| AccountError::invalid("Cannot encode IoT configuration result.")),
        }
    })();
    for registration in registrations {
        signal_hook::low_level::unregister(registration);
    }
    if signal.load(Ordering::SeqCst) != 0 {
        writeln!(
            std::io::stderr().lock(),
            "IoT account setup interrupted; credentials may have changed. Verify the old/new login before retrying."
        )?;
        return Ok(128 + signal.load(Ordering::SeqCst) as i32);
    }
    match result {
        Ok(value) => {
            writeln!(
                std::io::stdout().lock(),
                "{}",
                serde_json::to_string(&value)
                    .map_err(|_| AccountError::invalid("Cannot encode IoT result."))?
            )?;
            Ok(0)
        }
        Err(error) => {
            writeln!(std::io::stderr().lock(), "IoT account setup: {error}")?;
            Ok(error.exit_status())
        }
    }
}

#[cfg(test)]
mod tests;
