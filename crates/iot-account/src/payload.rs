use crate::{AccountError, Result, Secret, normalize_language, validate_username};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

pub const MARKER: &str = "WOR_IOT_ACCOUNT_JSON:";
pub const LOCALE_MARKER: &str = "WOR_IOT_LOCALE_JSON:";
pub const ACCOUNT_PAYLOAD: &str = include_str!("../../../src/lib/iot-account.ps1");
pub const LOCALE_PAYLOAD: &str = include_str!("../../../src/lib/iot-locale.ps1");
pub const LOCALE_USER_PAYLOAD: &[u8] = include_bytes!("../../../src/lib/iot-locale-user.ps1");

/// Encoded credentials can still be secrets, so scripts intentionally do not implement Debug.
///
/// ```compile_fail
/// use wor_iot_account::{AccountOperation, Secret, remote_script};
/// let script = remote_script(AccountOperation::Apply, "FixtureAdmin",
///     &Secret::new("fixture-only-password")).unwrap();
/// println!("{script:?}");
/// ```
pub struct ManagementScript(Zeroizing<String>);
impl std::ops::Deref for ManagementScript {
    type Target = str;
    fn deref(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug)]
pub enum AccountOperation {
    Apply,
    Verify,
}

pub fn remote_script(
    operation: AccountOperation,
    username: &str,
    password: &Secret,
) -> Result<ManagementScript> {
    validate_username(username)?;
    if ACCOUNT_PAYLOAD.matches("__WOR_REQUEST__").count() != 1 {
        return Err(AccountError::invalid(
            "IoT account management script is incomplete.",
        ));
    }
    #[derive(Serialize)]
    struct Request<'a> {
        operation: &'a str,
        username: &'a str,
        password: &'a str,
    }
    let value = Request {
        operation: match operation {
            AccountOperation::Apply => "apply",
            AccountOperation::Verify => "verify",
        },
        username,
        password: password.expose(),
    };
    let bytes = Zeroizing::new(
        serde_json::to_vec(&value)
            .map_err(|_| AccountError::invalid("Cannot encode private IoT request."))?,
    );
    let encoded = Zeroizing::new(STANDARD.encode(&bytes));
    Ok(ManagementScript(Zeroizing::new(
        ACCOUNT_PAYLOAD.replace("__WOR_REQUEST__", &encoded) + "\n\n",
    )))
}

pub fn locale_script(language: &str) -> Result<ManagementScript> {
    let language = normalize_language(language)?;
    if LOCALE_PAYLOAD.matches("__WOR_REQUEST__").count() != 1
        || LOCALE_PAYLOAD.matches("__WOR_USER_SCRIPT__").count() != 1
    {
        return Err(AccountError::invalid(
            "IoT language management script is incomplete.",
        ));
    }
    let payload = STANDARD.encode(
        serde_json::to_vec(&serde_json::json!({"language":language}))
            .map_err(|_| AccountError::invalid("Cannot encode IoT language request."))?,
    );
    Ok(ManagementScript(Zeroizing::new(
        LOCALE_PAYLOAD
            .replace("__WOR_REQUEST__", &payload)
            .replace("__WOR_USER_SCRIPT__", &STANDARD.encode(LOCALE_USER_PAYLOAD))
            + "\n\n",
    )))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AccountState {
    Applied,
    Verified,
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AccountStage {
    DeviceGuard,
    NativePreflight,
    PasswordUpdate,
    UsernameUpdate,
    LoginVerification,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountResponse {
    pub state: AccountState,
    pub username: String,
    pub sid: String,
    pub password_changed: bool,
    pub username_changed: bool,
    pub code: u32,
    pub stage: AccountStage,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LanguageState {
    Verified,
    PendingReboot,
    Unsupported,
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LanguageStage {
    DeviceGuard,
    DefaultAccountTask,
    DefaultAccountGuard,
    LanguageSupport,
    LanguageUpdate,
    LanguageVerification,
    TaskCleanup,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LanguageResponse {
    pub state: LanguageState,
    pub language: String,
    pub changed: bool,
    pub stage: LanguageStage,
    pub available: Vec<String>,
}

fn clean_ansi(output: &[u8]) -> Vec<u8> {
    let mut result = Vec::with_capacity(output.len());
    let mut at = 0;
    while at < output.len() {
        if output[at] == 0x1b && output.get(at + 1) == Some(&b'[') {
            let mut end = at + 2;
            while end < output.len() && (0x30..=0x3f).contains(&output[end]) {
                end += 1;
            }
            while end < output.len() && (0x20..=0x2f).contains(&output[end]) {
                end += 1;
            }
            if end < output.len() && (0x40..=0x7e).contains(&output[end]) {
                at = end + 1;
                continue;
            }
        }
        result.push(output[at]);
        at += 1;
    }
    result
}

fn record(output: &[u8], marker: &str) -> Result<String> {
    if output.len() > 1024 * 1024 {
        return Err(AccountError::invalid(
            "The Pi returned excessive setup output; its state is uncertain.",
        ));
    }
    let cleaned = clean_ansi(output);
    let text = String::from_utf8_lossy(&cleaned);
    let mut records = text.lines().filter_map(|line| line.strip_prefix(marker));
    let first = records.next().ok_or_else(|| {
        AccountError::invalid(
            "No unambiguous IoT result was received; credentials may already have changed.",
        )
    })?;
    if records.next().is_some() {
        return Err(AccountError::invalid(
            "No unambiguous IoT result was received; credentials may already have changed.",
        ));
    }
    Ok(first.to_owned())
}

pub fn parse_response(output: &[u8]) -> Result<AccountResponse> {
    serde_json::from_str(&record(output, MARKER)?).map_err(|_| {
        AccountError::invalid(
            "The Pi returned incomplete account state; credentials may already have changed.",
        )
    })
}

pub fn parse_locale_response(output: &[u8]) -> Result<LanguageResponse> {
    let response: LanguageResponse = serde_json::from_str(&record(output, LOCALE_MARKER)?)
        .map_err(|_| AccountError::invalid("The Pi returned incomplete IoT language state."))?;
    normalize_language(&response.language)?;
    Ok(response)
}
