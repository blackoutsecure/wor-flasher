use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use thiserror::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ValueKind {
    Text,
    Boolean,
    Object,
}

#[derive(Clone, Copy)]
pub struct SettingSpec {
    pub name: &'static str,
    pub paths: &'static [&'static str],
    pub kind: ValueKind,
}

use ValueKind::{Boolean, Object, Text};

pub const SETTINGS: &[SettingSpec] = &[
    SettingSpec {
        name: "RPI_MODEL",
        paths: &["target.rpiModel", "rpiModel", "RPI_MODEL"],
        kind: Text,
    },
    SettingSpec {
        name: "WOR_TARGET_BOARD",
        paths: &["target.board", "WOR_TARGET_BOARD"],
        kind: Text,
    },
    SettingSpec {
        name: "WOR_IMAGE_FAMILY",
        paths: &["media.imageFamily", "WOR_IMAGE_FAMILY"],
        kind: Text,
    },
    SettingSpec {
        name: "WOR_IMAGE_ARCH",
        paths: &["media.architecture", "WOR_IMAGE_ARCH"],
        kind: Text,
    },
    SettingSpec {
        name: "DEVICE",
        paths: &["target.device", "device", "DEVICE"],
        kind: Text,
    },
    SettingSpec {
        name: "CAN_INSTALL_ON_SAME_DRIVE",
        paths: &[
            "target.canInstallOnSameDrive",
            "canInstallOnSameDrive",
            "CAN_INSTALL_ON_SAME_DRIVE",
        ],
        kind: Boolean,
    },
    SettingSpec {
        name: "WIN_LANG",
        paths: &["media.winLang", "winLang", "WIN_LANG"],
        kind: Text,
    },
    SettingSpec {
        name: "BID",
        paths: &["media.bid", "bid", "BID"],
        kind: Text,
    },
    SettingSpec {
        name: "SOURCE_FILE",
        paths: &["media.sourceFile", "sourceFile", "SOURCE_FILE"],
        kind: Text,
    },
    SettingSpec {
        name: "DL_DIR",
        paths: &["media.downloadDir", "downloadDir", "DL_DIR"],
        kind: Text,
    },
    SettingSpec {
        name: "USE_CACHE",
        paths: &["media.useCache", "useCache", "USE_CACHE"],
        kind: Text,
    },
    SettingSpec {
        name: "WOR_IOT_DOWNLOAD",
        paths: &["media.downloadIotImage", "WOR_IOT_DOWNLOAD"],
        kind: Boolean,
    },
    SettingSpec {
        name: "WOR_IOT_CONFIRM_ERASE",
        paths: &["execution.confirmIotErase", "WOR_IOT_CONFIRM_ERASE"],
        kind: Boolean,
    },
    SettingSpec {
        name: "IOT_CORE_WIPE_DRIVE",
        paths: &["execution.wipeIotDrive", "IOT_CORE_WIPE_DRIVE"],
        kind: Boolean,
    },
    SettingSpec {
        name: "WOR_IOT_CONFIRM_WIPE",
        paths: &["execution.confirmIotWipe", "WOR_IOT_CONFIRM_WIPE"],
        kind: Boolean,
    },
    SettingSpec {
        name: "APPLY_CUSTOM_CONFIG_TXT",
        paths: &[
            "customization.applyCustomConfigTxt",
            "applyCustomConfigTxt",
            "APPLY_CUSTOM_CONFIG_TXT",
        ],
        kind: Boolean,
    },
    SettingSpec {
        name: "CONFIG_TXT",
        paths: &["customization.configTxt", "configTxt", "CONFIG_TXT"],
        kind: Text,
    },
    SettingSpec {
        name: "HDMI_MODE",
        paths: &["customization.hdmiMode", "hdmiMode", "HDMI_MODE"],
        kind: Text,
    },
    SettingSpec {
        name: "BOOT_CONFIG_SETTINGS",
        paths: &["customization.bootSettings", "BOOT_CONFIG_SETTINGS"],
        kind: Object,
    },
    SettingSpec {
        name: "IOT_CORE_HDMI_MODE",
        paths: &["customization.iotHdmiMode", "IOT_CORE_HDMI_MODE"],
        kind: Text,
    },
    SettingSpec {
        name: "IOT_CORE_HDMI_CONFIG",
        paths: &["customization.iotHdmiConfig", "IOT_CORE_HDMI_CONFIG"],
        kind: Text,
    },
    SettingSpec {
        name: "OOBE_NETWORK_BYPASS",
        paths: &[
            "customization.oobeNetworkBypass",
            "oobeNetworkBypass",
            "OOBE_NETWORK_BYPASS",
        ],
        kind: Boolean,
    },
    SettingSpec {
        name: "PI4_AUTO_DISABLE_3GB",
        paths: &[
            "customization.pi4AutoDisable3Gb",
            "pi4AutoDisable3Gb",
            "PI4_AUTO_DISABLE_3GB",
        ],
        kind: Boolean,
    },
    SettingSpec {
        name: "PI4_UEFI_SHELL_UNLOCK",
        paths: &[
            "customization.pi4UefiShellUnlock",
            "pi4UefiShellUnlock",
            "PI4_UEFI_SHELL_UNLOCK",
        ],
        kind: Boolean,
    },
    SettingSpec {
        name: "UEFI_USE_LATEST",
        paths: &[
            "customization.uefiUseLatest",
            "uefiUseLatest",
            "UEFI_USE_LATEST",
        ],
        kind: Boolean,
    },
    SettingSpec {
        name: "DRIVERS_USE_LATEST",
        paths: &[
            "customization.driversUseLatest",
            "driversUseLatest",
            "DRIVERS_USE_LATEST",
        ],
        kind: Boolean,
    },
    SettingSpec {
        name: "HIDE_EMPTY_DRIVES",
        paths: &[
            "customization.hideEmptyDrives",
            "hideEmptyDrives",
            "HIDE_EMPTY_DRIVES",
        ],
        kind: Boolean,
    },
    SettingSpec {
        name: "WINDOWS_ACCOUNT_SETUP",
        paths: &[
            "userAccount.setupAccount",
            "windowsAccountSetup",
            "WINDOWS_ACCOUNT_SETUP",
        ],
        kind: Boolean,
    },
    SettingSpec {
        name: "WINDOWS_ACCOUNT_USERNAME",
        paths: &[
            "userAccount.username",
            "windowsAccountUsername",
            "WINDOWS_ACCOUNT_USERNAME",
        ],
        kind: Text,
    },
    SettingSpec {
        name: "WINDOWS_ACCOUNT_PASSWORD",
        paths: &[
            "userAccount.password",
            "windowsAccountPassword",
            "WINDOWS_ACCOUNT_PASSWORD",
        ],
        kind: Text,
    },
    SettingSpec {
        name: "WINDOWS_LOCALE_SETUP",
        paths: &[
            "userAccount.localeSetup",
            "windowsLocaleSetup",
            "WINDOWS_LOCALE_SETUP",
        ],
        kind: Boolean,
    },
    SettingSpec {
        name: "WINDOWS_LOCALE",
        paths: &["userAccount.locale", "windowsLocale", "WINDOWS_LOCALE"],
        kind: Text,
    },
    SettingSpec {
        name: "IOT_CORE_ACCOUNT_SETUP",
        paths: &["userAccount.iotCore.enabled", "IOT_CORE_ACCOUNT_SETUP"],
        kind: Boolean,
    },
    SettingSpec {
        name: "IOT_CORE_ACCOUNT_USERNAME",
        paths: &["userAccount.iotCore.username", "IOT_CORE_ACCOUNT_USERNAME"],
        kind: Text,
    },
    SettingSpec {
        name: "IOT_CORE_ACCOUNT_PASSWORD",
        paths: &["userAccount.iotCore.password", "IOT_CORE_ACCOUNT_PASSWORD"],
        kind: Text,
    },
    SettingSpec {
        name: "IOT_CORE_HOST",
        paths: &["userAccount.iotCore.host", "IOT_CORE_HOST"],
        kind: Text,
    },
    SettingSpec {
        name: "IOT_CORE_AUTOMATIC_ADDRESS",
        paths: &[
            "userAccount.iotCore.automaticAddress",
            "IOT_CORE_AUTOMATIC_ADDRESS",
        ],
        kind: Boolean,
    },
    SettingSpec {
        name: "IOT_CORE_CURRENT_USERNAME",
        paths: &[
            "userAccount.iotCore.currentUsername",
            "IOT_CORE_CURRENT_USERNAME",
        ],
        kind: Text,
    },
    SettingSpec {
        name: "IOT_CORE_CURRENT_PASSWORD",
        paths: &[
            "userAccount.iotCore.currentPassword",
            "IOT_CORE_CURRENT_PASSWORD",
        ],
        kind: Text,
    },
    SettingSpec {
        name: "IOT_CORE_LANGUAGE_SETUP",
        paths: &[
            "userAccount.iotCore.languageSetup",
            "IOT_CORE_LANGUAGE_SETUP",
        ],
        kind: Boolean,
    },
    SettingSpec {
        name: "IOT_CORE_LANGUAGE",
        paths: &["userAccount.iotCore.language", "IOT_CORE_LANGUAGE"],
        kind: Text,
    },
    SettingSpec {
        name: "DRY_RUN",
        paths: &["execution.dryRun", "dryRun", "DRY_RUN"],
        kind: Boolean,
    },
    SettingSpec {
        name: "SKIP_IMAGE_VERIFICATION",
        paths: &[
            "execution.skipImageVerification",
            "skipImageVerification",
            "SKIP_IMAGE_VERIFICATION",
        ],
        kind: Boolean,
    },
    SettingSpec {
        name: "VERIFY_TLS",
        paths: &["execution.verifyTls", "verifyTls", "VERIFY_TLS"],
        kind: Boolean,
    },
    SettingSpec {
        name: "NO_UPDATE",
        paths: &["execution.noUpdate", "noUpdate", "NO_UPDATE"],
        kind: Boolean,
    },
    SettingSpec {
        name: "CHECK_FOR_UPDATES",
        paths: &[
            "execution.checkForUpdates",
            "checkForUpdates",
            "CHECK_FOR_UPDATES",
        ],
        kind: Boolean,
    },
    SettingSpec {
        name: "WOR_LOG_FILE",
        paths: &["execution.logFile", "logFile", "WOR_LOG_FILE"],
        kind: Text,
    },
    SettingSpec {
        name: "PLAY_SOUND",
        paths: &["notifications.playSound", "playSound", "PLAY_SOUND"],
        kind: Boolean,
    },
    SettingSpec {
        name: "SHOW_NOTIFICATION",
        paths: &[
            "notifications.showNotification",
            "showNotification",
            "SHOW_NOTIFICATION",
        ],
        kind: Boolean,
    },
    SettingSpec {
        name: "PE_INSTALLER_URL",
        paths: &[
            "system.peInstallerUrl",
            "peInstallerUrl",
            "PE_INSTALLER_URL",
        ],
        kind: Text,
    },
    SettingSpec {
        name: "PE_INSTALLER_SHA256",
        paths: &[
            "system.peInstallerSha256",
            "peInstallerSha256",
            "PE_INSTALLER_SHA256",
        ],
        kind: Text,
    },
    SettingSpec {
        name: "UEFI_VER_PI3",
        paths: &["system.uefiVerPi3", "uefiVerPi3", "UEFI_VER_PI3"],
        kind: Text,
    },
    SettingSpec {
        name: "UEFI_VER_PI4",
        paths: &["system.uefiVerPi4", "uefiVerPi4", "UEFI_VER_PI4"],
        kind: Text,
    },
    SettingSpec {
        name: "UEFI_VER_PI5",
        paths: &["system.uefiVerPi5", "uefiVerPi5", "UEFI_VER_PI5"],
        kind: Text,
    },
    SettingSpec {
        name: "DRIVER_VER",
        paths: &["system.driverVer", "driverVer", "DRIVER_VER"],
        kind: Text,
    },
    SettingSpec {
        name: "ARMV80_MAX_BUILD",
        paths: &[
            "system.armv80MaxBuild",
            "armv80MaxBuild",
            "ARMV80_MAX_BUILD",
        ],
        kind: Text,
    },
    SettingSpec {
        name: "WIN11_MIN_BUILD",
        paths: &["system.win11MinBuild", "win11MinBuild", "WIN11_MIN_BUILD"],
        kind: Text,
    },
    SettingSpec {
        name: "WIN10_OLDEST_BUILD",
        paths: &[
            "system.win10OldestBuild",
            "win10OldestBuild",
            "WIN10_OLDEST_BUILD",
        ],
        kind: Text,
    },
    SettingSpec {
        name: "EXAMPLE_BID",
        paths: &["system.exampleBid", "exampleBid", "EXAMPLE_BID"],
        kind: Text,
    },
    SettingSpec {
        name: "ARMV80_SAFE_BID",
        paths: &["system.armv80SafeBid", "armv80SafeBid", "ARMV80_SAFE_BID"],
        kind: Text,
    },
    SettingSpec {
        name: "REPO_SLUG",
        paths: &["system.repoSlug", "repoSlug", "REPO_SLUG"],
        kind: Text,
    },
    SettingSpec {
        name: "COMPLETION_SOUND",
        paths: &["completionSound", "COMPLETION_SOUND"],
        kind: Text,
    },
    SettingSpec {
        name: "PE_USE_LATEST",
        paths: &["PE_USE_LATEST"],
        kind: Boolean,
    },
    SettingSpec {
        name: "WOR_RUN_ID",
        paths: &["WOR_RUN_ID"],
        kind: Text,
    },
    SettingSpec {
        name: "WOR_CACHE_DIR",
        paths: &["WOR_CACHE_DIR"],
        kind: Text,
    },
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ImageFamily {
    Desktop,
    IotCore,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DriveCapability {
    TooSmall,
    Recovery,
    Install,
}

impl DriveCapability {
    pub fn desktop(bytes: u64) -> Self {
        if bytes < 8 * 1024 * 1024 * 1024 {
            Self::TooSmall
        } else if bytes < 25 * 1024 * 1024 * 1024 {
            Self::Recovery
        } else {
            Self::Install
        }
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ConfigError {
    #[error("Configuration must be a JSON object")]
    InvalidDocument,
    #[error("{0} must be a boolean")]
    Boolean(&'static str),
    #[error("{0} must be an object")]
    Object(&'static str),
    #[error("Invalid or missing {0}")]
    Invalid(String),
    #[error("Required canonical metadata is missing: {0}")]
    Metadata(&'static str),
    #[error("The selected board does not support this image family or architecture")]
    Profile,
    #[error(
        "Build {build} requires ARMv8.1 atomics; this board supports builds at or below {maximum}"
    )]
    Build { build: String, maximum: u32 },
    #[error("The selected drive is too small for this installation mode")]
    Capacity,
    #[error(
        "IoT Core verification cannot be skipped, and recovery/desktop customization does not apply"
    )]
    IotOptions,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeConfig {
    values: BTreeMap<String, String>,
    product_name: String,
    product_version: String,
}

pub fn at_path<'a>(root: &'a Value, path: &str) -> Option<&'a Value> {
    path.split('.').try_fold(root, |value, key| value.get(key))
}

fn preserve_empty(name: &str) -> bool {
    matches!(
        name,
        "IOT_CORE_CURRENT_USERNAME" | "IOT_CORE_CURRENT_PASSWORD"
    )
}

fn boolean(value: &Value, name: &'static str) -> Result<String, ConfigError> {
    match value {
        Value::Bool(value) => Ok(u8::from(*value).to_string()),
        Value::Number(number) if number.as_i64() == Some(0) => Ok("0".into()),
        Value::Number(number) if number.as_i64() == Some(1) => Ok("1".into()),
        Value::String(value) if matches!(value.as_str(), "true" | "1") => Ok("1".into()),
        Value::String(value) if matches!(value.as_str(), "false" | "0") => Ok("0".into()),
        _ => Err(ConfigError::Boolean(name)),
    }
}

fn scalar(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

impl NativeConfig {
    pub fn resolve(
        document: &Value,
        environment: &BTreeMap<String, String>,
        metadata: &Value,
        home: &str,
        host_locale: &str,
        macos: bool,
    ) -> Result<Self, ConfigError> {
        if !document.is_object() {
            return Err(ConfigError::InvalidDocument);
        }
        let mut values = BTreeMap::new();
        for spec in SETTINGS {
            if let Some(value) = environment.get(spec.name)
                && (!value.is_empty() || preserve_empty(spec.name))
            {
                let value = if spec.name == "CHECK_FOR_UPDATES" {
                    boolean(&Value::String(value.clone()), "CHECK_FOR_UPDATES")?
                } else {
                    value.clone()
                };
                values.insert(spec.name.to_owned(), value);
                continue;
            }
            let value = spec
                .paths
                .iter()
                .filter_map(|path| at_path(document, path))
                .find(|value| {
                    !value.is_null() && (spec.kind == Boolean || **value != Value::Bool(false))
                });
            if let Some(value) = value {
                let text = match spec.kind {
                    Boolean => {
                        if value == "" {
                            continue;
                        }
                        boolean(value, spec.name)?
                    }
                    Object => {
                        if !value.is_object() {
                            return Err(ConfigError::Object(spec.name));
                        }
                        value.to_string()
                    }
                    Text => {
                        if preserve_empty(spec.name) && !value.is_string() {
                            return Err(ConfigError::Invalid(spec.name.into()));
                        }
                        scalar(value)
                    }
                };
                values.insert(spec.name.to_owned(), text);
            }
        }
        if environment.get("NO_UPDATE").is_none_or(String::is_empty)
            && let Some(check) = values
                .get("CHECK_FOR_UPDATES")
                .filter(|value| !value.is_empty())
        {
            let inverse = match check.as_str() {
                "1" => "0",
                "0" => "1",
                _ => return Err(ConfigError::Boolean("CHECK_FOR_UPDATES")),
            };
            values.insert("NO_UPDATE".into(), inverse.into());
        }
        if environment
            .get("COMPLETION_SOUND")
            .is_none_or(String::is_empty)
            && let Some(value) = at_path(
                document,
                if macos {
                    "notifications.sounds.macos"
                } else {
                    "notifications.sounds.linux"
                },
            )
            && !value.is_null()
            && *value != Value::Bool(false)
        {
            values.insert("COMPLETION_SOUND".into(), scalar(value));
        }
        for (key, path) in [
            ("PE_INSTALLER_URL", "peInstallerUrl"),
            ("PE_INSTALLER_SHA256", "peInstallerSha256"),
            ("UEFI_VER_PI3", "uefiVerPi3"),
            ("UEFI_VER_PI4", "uefiVerPi4"),
            ("UEFI_VER_PI5", "uefiVerPi5"),
            ("DRIVER_VER", "driverVer"),
            ("ARMV80_MAX_BUILD", "armv80MaxBuild"),
            ("WIN11_MIN_BUILD", "win11MinBuild"),
            ("WIN10_OLDEST_BUILD", "win10OldestBuild"),
            ("EXAMPLE_BID", "exampleBid"),
            ("ARMV80_SAFE_BID", "armv80SafeBid"),
            ("REPO_SLUG", "repoSlug"),
        ] {
            if values.get(key).is_none_or(String::is_empty) {
                let value = metadata
                    .get("systemDefaults")
                    .and_then(|v| v.get(path))
                    .filter(|value| !value.is_null())
                    .ok_or(ConfigError::Metadata(path))?;
                values.insert(key.into(), scalar(value));
            }
        }
        for (key, value) in [
            ("WOR_IMAGE_FAMILY", "desktop"),
            ("USE_CACHE", "1"),
            ("PE_USE_LATEST", "1"),
            ("PI4_AUTO_DISABLE_3GB", "1"),
            ("PI4_UEFI_SHELL_UNLOCK", "0"),
            ("OOBE_NETWORK_BYPASS", "1"),
            ("WINDOWS_ACCOUNT_SETUP", "0"),
            ("WINDOWS_LOCALE_SETUP", "1"),
            ("DRIVERS_USE_LATEST", "1"),
            ("DRY_RUN", "0"),
            ("SKIP_IMAGE_VERIFICATION", "0"),
            ("APPLY_CUSTOM_CONFIG_TXT", "1"),
            ("HIDE_EMPTY_DRIVES", "1"),
            ("VERIFY_TLS", "1"),
            ("PLAY_SOUND", "1"),
            ("SHOW_NOTIFICATION", "1"),
            ("NO_UPDATE", "0"),
            ("WOR_IOT_DOWNLOAD", "0"),
            ("WOR_IOT_CONFIRM_ERASE", "0"),
            ("IOT_CORE_WIPE_DRIVE", "0"),
            ("WOR_IOT_CONFIRM_WIPE", "0"),
            ("IOT_CORE_ACCOUNT_SETUP", "0"),
            ("IOT_CORE_LANGUAGE_SETUP", "1"),
            ("IOT_CORE_HDMI_MODE", "720p60"),
            ("IOT_CORE_ACCOUNT_USERNAME", "Administrator"),
        ] {
            if values.get(key).is_none_or(String::is_empty) {
                values.insert(key.into(), value.into());
            }
        }
        if values.get("DL_DIR").is_none_or(String::is_empty) {
            values.insert("DL_DIR".into(), format!("{home}/wor-flasher-files"));
        }
        let locale = windows_locale(&normalize_language(host_locale));
        if values.get("WINDOWS_LOCALE").is_none_or(String::is_empty) {
            values.insert("WINDOWS_LOCALE".into(), locale);
        }
        for key in ["SOURCE_FILE", "DL_DIR", "WOR_LOG_FILE"] {
            if let Some(value) = values.get_mut(key)
                && let Some(suffix) = value.strip_prefix('~')
            {
                *value = format!("{home}{suffix}");
            }
        }
        if values
            .get("WOR_IMAGE_FAMILY")
            .is_some_and(|v| v == "iot-core")
        {
            if let Some(model) =
                values
                    .get("WOR_TARGET_BOARD")
                    .and_then(|board| match board.as_str() {
                        "pi2-v1.1" => Some("2"),
                        "pi2-v1.2" | "pi3-b" => Some("3"),
                        _ => None,
                    })
            {
                values.insert("RPI_MODEL".into(), model.into());
            }
            values
                .entry("CAN_INSTALL_ON_SAME_DRIVE".into())
                .or_insert("1".into());
        }
        let product = metadata
            .get("product")
            .ok_or(ConfigError::Metadata("product"))?;
        let result = Self {
            values,
            product_name: product
                .get("name")
                .and_then(Value::as_str)
                .ok_or(ConfigError::Metadata("product.name"))?
                .into(),
            product_version: product
                .get("version")
                .and_then(Value::as_str)
                .ok_or(ConfigError::Metadata("product.version"))?
                .into(),
        };
        for spec in SETTINGS.iter().filter(|spec| spec.kind == Boolean) {
            if !result.get(spec.name).is_empty() {
                result.flag(spec.name)?;
            }
        }
        if !matches!(result.get("USE_CACHE"), "0" | "1" | "2") {
            return Err(ConfigError::Invalid("USE_CACHE".into()));
        }
        if !matches!(
            result.get("HDMI_MODE"),
            "" | "official" | "recommended" | "custom"
        ) {
            return Err(ConfigError::Invalid("HDMI_MODE".into()));
        }
        if result.flag("WINDOWS_ACCOUNT_SETUP")?
            && (result.get("WINDOWS_ACCOUNT_USERNAME").is_empty()
                || result.get("WINDOWS_ACCOUNT_PASSWORD").is_empty())
        {
            return Err(ConfigError::Invalid(
                "Windows account username/password".into(),
            ));
        }
        if result.flag("WINDOWS_LOCALE_SETUP")? && !valid_locale(result.get("WINDOWS_LOCALE")) {
            return Err(ConfigError::Invalid("WINDOWS_LOCALE".into()));
        }
        Ok(result)
    }

    pub fn get(&self, name: &str) -> &str {
        self.values.get(name).map_or("", String::as_str)
    }
    pub fn is_set(&self, name: &str) -> bool {
        self.values.contains_key(name)
    }
    pub fn values(&self) -> &BTreeMap<String, String> {
        &self.values
    }
    pub fn set_derived(&mut self, name: &str, value: impl Into<String>) {
        self.values.insert(name.into(), value.into());
    }
    pub fn flag(&self, name: &str) -> Result<bool, ConfigError> {
        match self.get(name) {
            "1" => Ok(true),
            "0" => Ok(false),
            _ => Err(ConfigError::Invalid(name.into())),
        }
    }
    pub fn family(&self) -> Result<ImageFamily, ConfigError> {
        match self.get("WOR_IMAGE_FAMILY") {
            "desktop" => Ok(ImageFamily::Desktop),
            "iot-core" => Ok(ImageFamily::IotCore),
            _ => Err(ConfigError::Profile),
        }
    }
    pub fn architecture(&self) -> Result<&'static str, ConfigError> {
        match (self.family()?, self.get("WOR_IMAGE_ARCH")) {
            (ImageFamily::Desktop, "" | "arm64") => Ok("arm64"),
            (ImageFamily::IotCore, "" | "arm32") => Ok("arm32"),
            _ => Err(ConfigError::Profile),
        }
    }
    pub fn model(&self) -> Result<u8, ConfigError> {
        match self.get("RPI_MODEL") {
            "2" if self.family()? == ImageFamily::IotCore => Ok(2),
            "3" => Ok(3),
            "4" => Ok(4),
            "5" => Ok(5),
            _ => Err(ConfigError::Invalid("RPI_MODEL".into())),
        }
    }
    pub fn build_major(&self) -> Result<Option<u32>, ConfigError> {
        if self.get("BID").is_empty() {
            return Ok(None);
        }
        let mut parts = self.get("BID").split('.');
        let major = parts
            .next()
            .filter(|part| !part.is_empty() && part.bytes().all(|v| v.is_ascii_digit()))
            .and_then(|part| part.parse().ok())
            .ok_or_else(|| ConfigError::Invalid("BID".into()))?;
        for part in parts {
            if part.is_empty() || !part.bytes().all(|v| v.is_ascii_digit()) {
                return Err(ConfigError::Invalid("BID".into()));
            }
        }
        Ok(Some(major))
    }
    pub fn validate_profile(&self) -> Result<(), ConfigError> {
        self.architecture()?;
        match self.family()? {
            ImageFamily::Desktop => {
                if self.get("WOR_TARGET_BOARD") == "pi2-v1.1" || self.flag("WOR_IOT_DOWNLOAD")? {
                    return Err(ConfigError::Profile);
                }
                let model = self.model()?;
                if !matches!(model, 3..=5) {
                    return Err(ConfigError::Profile);
                }
                if let Some(build) = self.build_major()? {
                    let maximum = self
                        .get("ARMV80_MAX_BUILD")
                        .parse::<u32>()
                        .map_err(|_| ConfigError::Invalid("ARMV80_MAX_BUILD".into()))?;
                    if model != 5 && build > maximum {
                        return Err(ConfigError::Build {
                            build: self.get("BID").into(),
                            maximum,
                        });
                    }
                }
                if self
                    .get("SOURCE_FILE")
                    .to_ascii_lowercase()
                    .ends_with(".ffu")
                {
                    return Err(ConfigError::Profile);
                }
            }
            ImageFamily::IotCore => {
                if !matches!(
                    self.get("WOR_TARGET_BOARD"),
                    "pi2-v1.1" | "pi2-v1.2" | "pi3-b"
                ) {
                    return Err(ConfigError::Profile);
                }
                if !self.flag("CAN_INSTALL_ON_SAME_DRIVE")?
                    || self.flag("SKIP_IMAGE_VERIFICATION")?
                    || self.flag("WINDOWS_ACCOUNT_SETUP")?
                {
                    return Err(ConfigError::IotOptions);
                }
                if self.flag("WOR_IOT_DOWNLOAD")? && !self.get("SOURCE_FILE").is_empty() {
                    return Err(ConfigError::Invalid(
                        "Choose a local IoT source or official download, not both".into(),
                    ));
                }
                if !self.get("SOURCE_FILE").is_empty()
                    && ![".ffu", ".iso", ".msi"]
                        .iter()
                        .any(|ext| self.get("SOURCE_FILE").to_ascii_lowercase().ends_with(ext))
                {
                    return Err(ConfigError::Profile);
                }
                let language = if self.get("IOT_CORE_LANGUAGE").is_empty() {
                    self.get("WINDOWS_LOCALE")
                } else {
                    self.get("IOT_CORE_LANGUAGE")
                };
                if !LANGUAGES
                    .iter()
                    .any(|(code, _)| code.eq_ignore_ascii_case(language))
                {
                    return Err(ConfigError::Invalid("IOT_CORE_LANGUAGE".into()));
                }
            }
        }
        Ok(())
    }
    pub fn validate_capacity(
        &self,
        bytes: u64,
        iot_minimum: Option<u64>,
    ) -> Result<DriveCapability, ConfigError> {
        if self.family()? == ImageFamily::IotCore {
            let minimum = iot_minimum
                .filter(|v| *v > 0)
                .ok_or_else(|| ConfigError::Invalid("verified IoT minimum capacity".into()))?;
            return if bytes >= minimum && bytes <= 2 * 1024_u64.pow(4) {
                Ok(DriveCapability::Install)
            } else {
                Err(ConfigError::Capacity)
            };
        }
        let capability = DriveCapability::desktop(bytes);
        if capability == DriveCapability::TooSmall
            || (self.flag("CAN_INSTALL_ON_SAME_DRIVE")? && capability != DriveCapability::Install)
        {
            Err(ConfigError::Capacity)
        } else {
            Ok(capability)
        }
    }
    pub fn effective_latest_uefi(&self) -> Result<bool, ConfigError> {
        if self.get("UEFI_USE_LATEST").is_empty() {
            Ok(self.model()? == 3)
        } else {
            self.flag("UEFI_USE_LATEST")
        }
    }
    pub fn board_label(&self) -> String {
        match (
            self.get("WOR_IMAGE_FAMILY"),
            self.get("WOR_TARGET_BOARD"),
            self.get("RPI_MODEL"),
        ) {
            ("iot-core", "pi2-v1.1", _) => "Raspberry Pi 2 v1.1".into(),
            (_, "pi2-v1.2", "3") => "Raspberry Pi 2 v1.2".into(),
            ("iot-core", "pi3-b", _) => "Raspberry Pi 3 Model B".into(),
            ("iot-core", _, _) => "Not selected (IoT Core requires an explicit board)".into(),
            (_, _, model) => format!("Raspberry Pi {model}"),
        }
    }
    pub fn version(&self) -> &str {
        &self.product_version
    }
    pub fn name(&self) -> &str {
        &self.product_name
    }
    pub fn windows_label(&self) -> Result<&'static str, ConfigError> {
        let minimum = self
            .get("WIN11_MIN_BUILD")
            .parse::<u32>()
            .map_err(|_| ConfigError::Invalid("WIN11_MIN_BUILD".into()))?;
        Ok(
            if self.build_major()?.is_some_and(|build| build >= minimum) {
                "Windows 11"
            } else {
                "Windows 10"
            },
        )
    }
    pub fn desktop_summary(&self, description: &str) -> Result<Vec<(String, String)>, ConfigError> {
        let mut rows = vec![
            (format!("{} version", self.name()), self.version().into()),
            ("Target drive".into(), description.into()),
            ("Target hardware".into(), self.board_label()),
            (
                "Operating system".into(),
                if self.get("BID").is_empty() {
                    "Not selected".into()
                } else {
                    format!(
                        "{} ({}) arm64 build {}",
                        self.windows_label()?,
                        windows_locale(self.get("WIN_LANG")),
                        self.get("BID")
                    )
                },
            ),
            (
                "Installation mode".into(),
                match self.get("CAN_INSTALL_ON_SAME_DRIVE") {
                    "1" => "Install Windows onto this drive",
                    "0" => "Recovery drive for another >16 GB drive",
                    _ => "Not selected",
                }
                .into(),
            ),
        ];
        if !self.get("SOURCE_FILE").is_empty() {
            rows.push(("Windows source".into(), self.get("SOURCE_FILE").into()));
        }
        if self.get("CAN_INSTALL_ON_SAME_DRIVE") == "0" {
            rows.push((
                "Windows setup scope".into(),
                "Windows Setup configuration is included on recovery media".into(),
            ));
        }
        if self.windows_label()? == "Windows 11" {
            rows.push((
                "Offline OOBE".into(),
                if self.flag("OOBE_NETWORK_BYPASS")? {
                    "Allowed"
                } else {
                    "Disabled"
                }
                .into(),
            ));
        }
        rows.push((
            "Windows local account".into(),
            if self.flag("WINDOWS_ACCOUNT_SETUP")? {
                format!("Administrator ({})", self.get("WINDOWS_ACCOUNT_USERNAME"))
            } else {
                "Windows setup will ask".into()
            },
        ));
        rows.push((
            "Windows keyboard and regional settings".into(),
            if self.flag("WINDOWS_LOCALE_SETUP")? {
                self.get("WINDOWS_LOCALE").into()
            } else {
                "Windows setup defaults".into()
            },
        ));
        if self.get("RPI_MODEL") == "4" {
            rows.push((
                "Pi 4 RAM unlock".into(),
                if self.flag("PI4_AUTO_DISABLE_3GB")? {
                    "Enabled"
                } else {
                    "Disabled"
                }
                .into(),
            ));
        }
        let firmware = self.get(match self.get("RPI_MODEL") {
            "3" => "UEFI_VER_PI3",
            "5" => "UEFI_VER_PI5",
            _ => "UEFI_VER_PI4",
        });
        rows.push((
            "UEFI firmware".into(),
            if self.effective_latest_uefi()? {
                "Latest".into()
            } else {
                format!("Pinned ({firmware})")
            },
        ));
        rows.push((
            "Windows ARM64 drivers".into(),
            if self.get("RPI_MODEL") == "5" {
                "No separate driver package for Pi 5".into()
            } else if self.flag("DRIVERS_USE_LATEST")? {
                "Latest".into()
            } else {
                format!("Pinned ({})", self.get("DRIVER_VER"))
            },
        ));
        let hdmi = match self.get("HDMI_MODE") {
            "custom" => "Custom (set in config.txt)",
            "official" => "Official image default",
            _ if self.get("RPI_MODEL") == "4" => "Recommended (1920 x 1200 / 60 Hz)",
            _ => "Official image default",
        };
        rows.push(("HDMI display".into(), hdmi.into()));
        rows.push((
            "Custom config.txt".into(),
            format!(
                "{}{}",
                if self.flag("APPLY_CUSTOM_CONFIG_TXT")? {
                    "Applied"
                } else {
                    "Using the firmware default"
                },
                if self.get("CAN_INSTALL_ON_SAME_DRIVE") == "0" {
                    " (recovery media only)"
                } else {
                    ""
                }
            ),
        ));
        for (label, key, yes, no) in [
            ("Hide empty drives", "HIDE_EMPTY_DRIVES", "Yes", "No"),
            (
                "Verify written image",
                "SKIP_IMAGE_VERIFICATION",
                "No (skipped)",
                "Yes",
            ),
        ] {
            rows.push((label.into(), if self.flag(key)? { yes } else { no }.into()));
        }
        rows.push((
            "Downloaded files".into(),
            match self.get("USE_CACHE") {
                "0" => "Re-download everything",
                "2" => "Trust the cache without checking",
                _ => "Reuse when they still match",
            }
            .into(),
        ));
        rows.push((
            "Dry run".into(),
            if self.flag("DRY_RUN")? {
                "Yes (no changes will be written)"
            } else {
                "No"
            }
            .into(),
        ));
        rows.push(("Download folder".into(), self.get("DL_DIR").into()));
        rows.push((
            "Log file".into(),
            if self.get("WOR_LOG_FILE").is_empty() {
                format!(
                    "{}/logs/wor-flasher-{}.log",
                    self.get("DL_DIR"),
                    self.get("WOR_RUN_ID")
                )
            } else {
                self.get("WOR_LOG_FILE").into()
            },
        ));
        Ok(rows)
    }
}

pub fn normalize_language(locale: &str) -> String {
    let code = locale
        .split(['@', '.'])
        .next()
        .unwrap_or("")
        .replace('_', "-")
        .to_ascii_lowercase();
    if LANGUAGES.iter().any(|(known, _)| *known == code) {
        code
    } else {
        "en-us".into()
    }
}

pub fn windows_locale(code: &str) -> String {
    code.split('-')
        .enumerate()
        .map(|(index, part)| {
            if index == 0 {
                part.to_ascii_lowercase()
            } else if part.len() == 4 {
                let mut value = part.to_ascii_lowercase();
                if let Some(first) = value.get_mut(..1) {
                    first.make_ascii_uppercase();
                }
                value
            } else {
                part.to_ascii_uppercase()
            }
        })
        .collect::<Vec<_>>()
        .join("-")
}

fn valid_locale(value: &str) -> bool {
    let parts: Vec<_> = value.split('-').collect();
    (1..=3).contains(&parts.len())
        && (2..=3).contains(&parts[0].len())
        && parts[0].bytes().all(|v| v.is_ascii_alphabetic())
        && parts.iter().skip(1).all(|part| {
            (2..=8).contains(&part.len()) && part.bytes().all(|v| v.is_ascii_alphanumeric())
        })
}

pub const LANGUAGES: &[(&str, &str)] = &[
    ("en-us", "English (United States)"),
    ("en-gb", "English (United Kingdom)"),
    ("ar-sa", "Arabic (Saudi Arabia)"),
    ("bg-bg", "Bulgarian (Bulgaria)"),
    ("cs-cz", "Czech (Czechia)"),
    ("da-dk", "Danish (Denmark)"),
    ("de-de", "German (Germany)"),
    ("el-gr", "Greek (Greece)"),
    ("es-es", "Spanish (Spain, International Sort)"),
    ("es-mx", "Spanish (Mexico)"),
    ("et-ee", "Estonian (Estonia)"),
    ("fi-fi", "Finnish (Finland)"),
    ("fr-ca", "French (Canada)"),
    ("fr-fr", "French (France)"),
    ("he-il", "Hebrew (Israel)"),
    ("hr-hr", "Croatian (Croatia)"),
    ("hu-hu", "Hungarian (Hungary)"),
    ("it-it", "Italian (Italy)"),
    ("ja-jp", "Japanese (Japan)"),
    ("ko-kr", "Korean (Korea)"),
    ("lt-lt", "Lithuanian (Lithuania)"),
    ("lv-lv", "Latvian (Latvia)"),
    ("nb-no", "Norwegian Bokmål (Norway)"),
    ("nl-nl", "Dutch (Netherlands)"),
    ("pl-pl", "Polish (Poland)"),
    ("pt-br", "Portuguese (Brazil)"),
    ("pt-pt", "Portuguese (Portugal)"),
    ("ro-ro", "Romanian (Romania)"),
    ("ru-ru", "Russian (Russia)"),
    ("sk-sk", "Slovak (Slovakia)"),
    ("sl-si", "Slovenian (Slovenia)"),
    ("sr-latn-rs", "Serbian (Latin, Serbia)"),
    ("sv-se", "Swedish (Sweden)"),
    ("th-th", "Thai (Thailand)"),
    ("tr-tr", "Turkish (Turkey)"),
    ("uk-ua", "Ukrainian (Ukraine)"),
    ("zh-cn", "Chinese (Simplified, China)"),
    ("zh-tw", "Chinese (Traditional, Taiwan)"),
];
