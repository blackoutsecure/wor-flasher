use crate::{AccountError, Result, Secret};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const FACTORY_USERNAME: &str = "Administrator";
pub const FACTORY_PASSWORD: &str = "p@ssw0rd";
pub const AUTOMATIC_HOSTNAME: &str = "minwinpc";

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FactoryDefaults {
    pub current_username: &'static str,
    pub current_password: &'static str,
    pub automatic_hostname: &'static str,
}

pub fn factory_login_defaults() -> FactoryDefaults {
    FactoryDefaults {
        current_username: FACTORY_USERNAME,
        current_password: FACTORY_PASSWORD,
        automatic_hostname: AUTOMATIC_HOSTNAME,
    }
}

pub fn validate_host_input(host: &str, allow_empty: bool) -> Result<()> {
    if (host.is_empty() && !allow_empty)
        || host.len() > 253
        || !host
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_.:%-".contains(&c))
    {
        return Err(AccountError::invalid(
            "Enter the Pi's local IP address or hostname, not a URL.",
        ));
    }
    Ok(())
}

pub fn validate_username(username: &str) -> Result<()> {
    if username.trim().is_empty() {
        return Err(AccountError::invalid(
            "An IoT username is required and cannot be blank.",
        ));
    }
    if username.len() > 20
        || !username.as_bytes()[0].is_ascii_alphanumeric()
        || !username
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_.-".contains(&c))
    {
        return Err(AccountError::invalid(
            "Use a 1-20 character IoT username containing letters, digits, dot, underscore or hyphen.",
        ));
    }
    if ["defaultaccount", "guest", "system", "wdagutilityaccount"]
        .contains(&username.to_ascii_lowercase().as_str())
    {
        return Err(AccountError::invalid(
            "Choose an administrator username, not a reserved system account.",
        ));
    }
    Ok(())
}

pub fn validate_password(password: &str, new: bool) -> Result<()> {
    if !new && password.is_empty() {
        return Err(AccountError::invalid(
            "The current IoT password cannot be blank; this password-based SSH login requires a nonempty password.",
        ));
    }
    let length = password.encode_utf16().count();
    if !(if new { 12 } else { 1 }..=127).contains(&length)
        || password.chars().any(|c| (c as u32) < 32)
    {
        return Err(AccountError::invalid(if new {
            "The new IoT password must contain 12-127 characters and no control characters."
        } else {
            "Enter the current IoT password without control characters."
        }));
    }
    Ok(())
}

pub fn normalize_language(value: &str) -> Result<String> {
    let invalid =
        || AccountError::invalid("Choose a Windows IoT language tag, for example en-US or fr-FR.");
    let parts: Vec<_> = value.split('-').collect();
    if !(2..=3).contains(&parts.len())
        || !(2..=3).contains(&parts[0].len())
        || !parts[0].bytes().all(|c| c.is_ascii_alphabetic())
        || parts[1..].iter().any(|part| {
            !(2..=8).contains(&part.len()) || !part.bytes().all(|c| c.is_ascii_alphanumeric())
        })
    {
        return Err(invalid());
    }
    let mut normalized = vec![parts[0].to_ascii_lowercase()];
    for part in &parts[1..] {
        if part.len() == 4 {
            normalized.push(format!(
                "{}{}",
                part[..1].to_ascii_uppercase(),
                part[1..].to_ascii_lowercase()
            ));
        } else {
            normalized.push(part.to_ascii_uppercase());
        }
    }
    Ok(normalized.join("-"))
}

pub(crate) fn object(value: &Value) -> Result<()> {
    if !value.is_object() {
        return Err(AccountError::invalid(
            "IoT account request must be an object.",
        ));
    }
    Ok(())
}
pub(crate) fn text(value: &Value, name: &str, default: &str) -> Result<String> {
    match value.get(name) {
        None => Ok(default.to_owned()),
        Some(Value::String(text)) => Ok(text.clone()),
        _ => Err(AccountError::invalid(
            "Invalid IoT connection or account preferences.",
        )),
    }
}
pub(crate) fn boolean(value: &Value, name: &str, default: bool) -> Result<bool> {
    match value.get(name) {
        None => Ok(default),
        Some(Value::Bool(value)) => Ok(*value),
        _ => Err(AccountError::invalid(
            "IoT preference flags must be enabled or disabled.",
        )),
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginPreferences {
    pub host: String,
    pub automatic_address: bool,
    pub current_username: String,
    pub current_password: Secret,
}

pub fn login_preferences(value: &Value) -> Result<LoginPreferences> {
    object(value)?;
    let host = text(value, "host", "")?;
    let current_username = text(value, "currentUsername", FACTORY_USERNAME)?;
    let current_password = Secret::new(text(value, "currentPassword", FACTORY_PASSWORD)?);
    let automatic_address = boolean(value, "automaticAddress", host.is_empty())?;
    validate_host_input(&host, automatic_address)?;
    validate_username(&current_username)?;
    validate_password(current_password.expose(), false)?;
    Ok(LoginPreferences {
        host,
        automatic_address,
        current_username,
        current_password,
    })
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountPreferences {
    pub account_setup: bool,
    pub account_username: String,
    pub account_password: Secret,
    pub language_setup: bool,
    pub language: String,
    #[serde(flatten)]
    pub connection: LoginPreferences,
}

pub fn account_preferences(value: &Value, metadata_only: bool) -> Result<AccountPreferences> {
    object(value)?;
    let account_setup = boolean(value, "accountSetup", false)?;
    let account_username = text(value, "accountUsername", FACTORY_USERNAME)?;
    let account_password = Secret::new(text(value, "accountPassword", "")?);
    if account_setup {
        validate_username(&account_username)?;
        if !metadata_only {
            validate_password(account_password.expose(), true)?;
        }
    } else if !account_password.expose().is_empty() {
        validate_password(account_password.expose(), false)?;
    }
    let language_setup = boolean(value, "languageSetup", false)?;
    let language = normalize_language(&text(value, "language", "en-US")?)?;
    let connection = login_preferences(value)?;
    Ok(AccountPreferences {
        account_setup,
        account_username,
        account_password,
        language_setup,
        language,
        connection,
    })
}
