use crate::catalog::{is_known_language, language_from_filename};
use crate::files::read_required;
use crate::{MediaError, windows_locale};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;

#[derive(Clone)]
pub struct PrepareOptions {
    pub runtime_dir: PathBuf,
    pub download_dir: PathBuf,
    pub settings: BTreeMap<String, String>,
}

impl fmt::Debug for PrepareOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PrepareOptions")
            .field("runtime_dir", &self.runtime_dir)
            .field("download_dir", &self.download_dir)
            .field("setting_keys", &self.settings.keys().collect::<Vec<_>>())
            .finish()
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum Model {
    Pi3,
    Pi4,
    Pi5,
}

impl Model {
    pub const fn number(self) -> u8 {
        match self {
            Self::Pi3 => 3,
            Self::Pi4 => 4,
            Self::Pi5 => 5,
        }
    }

    pub fn supports(self, build: &BuildId, armv80_ceiling: u32) -> bool {
        self == Self::Pi5 || build.major() <= armv80_ceiling
    }
}

impl FromStr for Model {
    type Err = MediaError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "3" => Ok(Self::Pi3),
            "4" => Ok(Self::Pi4),
            "5" => Ok(Self::Pi5),
            _ => Err(option("RPI_MODEL", "expected desktop model 3, 4 or 5")),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(try_from = "String", into = "String")]
pub struct BuildId(String);

impl BuildId {
    pub fn parse(value: &str) -> Result<Self, MediaError> {
        let components: Vec<_> = value.split('.').collect();
        if value.len() > 64
            || components.is_empty()
            || components.len() > 6
            || components.iter().any(|s| {
                s.is_empty()
                    || !s.bytes().all(|ch| ch.is_ascii_digit())
                    || s.parse::<u32>().is_err()
            })
            || components[0].parse::<u32>().unwrap_or(0) < 10000
        {
            return Err(option("BID", "expected a numeric Windows build identifier"));
        }
        Ok(Self(value.into()))
    }

    pub fn major(&self) -> u32 {
        self.0.split('.').next().unwrap().parse().unwrap()
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn from_filename(path: &Path) -> Option<Self> {
        path.file_name()?
            .to_str()?
            .split(['_', ' ', '-'])
            .find_map(|part| {
                let number: String = part
                    .chars()
                    .take_while(|ch| ch.is_ascii_digit() || *ch == '.')
                    .collect();
                Self::parse(number.trim_end_matches('.')).ok()
            })
    }
}

impl fmt::Display for BuildId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl TryFrom<String> for BuildId {
    type Error = MediaError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<BuildId> for String {
    fn from(value: BuildId) -> Self {
        value.0
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum CacheMode {
    Refresh,
    Verify,
    Trust,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum WindowsVersion {
    Windows10,
    Windows11,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum InstallationMode {
    SelfInstall,
    Recovery,
}

impl InstallationMode {
    pub(crate) fn maximum_install_bytes(self) -> u64 {
        let unit = if cfg!(target_os = "macos") {
            1024 * 1024
        } else {
            1_000_000
        };
        match self {
            Self::SelfInstall => 18000 * unit - 64 * 1024 * 1024,
            Self::Recovery => 6000 * unit - 64 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Debug)]
pub enum WindowsSource {
    Download,
    Esd(PathBuf),
    Iso(PathBuf),
}

#[derive(Clone, Default)]
pub struct Secret(pub(crate) String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[redacted]")
    }
}

#[derive(Clone, Debug)]
pub struct SetupOptions {
    pub oobe_network_bypass: bool,
    pub account: Option<(String, Secret)>,
    pub locale: Option<String>,
    pub pi4_auto_disable_3gb: bool,
    pub pi4_uefi_shell_unlock: bool,
}

#[derive(Clone, Debug)]
pub struct BootOptions {
    pub apply_custom_config: bool,
    pub config_txt: Option<String>,
    pub hdmi_mode: String,
    pub settings: serde_json::Value,
    pub hide_empty_drives: bool,
}

#[derive(Clone)]
pub struct PackagePolicy {
    pub pe_latest: bool,
    pub pe_url: String,
    pub pe_sha256: String,
    pub uefi_latest: bool,
    pub uefi_version: String,
    pub drivers_latest: bool,
    pub driver_version: String,
}

impl fmt::Debug for PackagePolicy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PackagePolicy")
            .field("pe_latest", &self.pe_latest)
            .field("pe_url", &"[redacted]")
            .field("pe_sha256", &self.pe_sha256)
            .field("uefi_latest", &self.uefi_latest)
            .field("uefi_version", &self.uefi_version)
            .field("drivers_latest", &self.drivers_latest)
            .field("driver_version", &self.driver_version)
            .finish()
    }
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemDefaults {
    pub pe_installer_url: String,
    pub pe_installer_sha256: String,
    pub uefi_shell_package_url: String,
    pub uefi_shell_package_sha256: String,
    pub uefi_ver_pi3: String,
    pub uefi_ver_pi4: String,
    pub uefi_ver_pi5: String,
    pub uefi_repo_pi3: String,
    pub uefi_repo_pi4: String,
    pub uefi_repo_pi5: String,
    pub driver_ver: String,
    pub drivers_repo: String,
    pub armv80_max_build: u32,
    pub win11_min_build: u32,
    pub win10_oldest_build: String,
}

impl fmt::Debug for SystemDefaults {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SystemDefaults")
            .field("armv80_max_build", &self.armv80_max_build)
            .field("win11_min_build", &self.win11_min_build)
            .finish_non_exhaustive()
    }
}

impl SystemDefaults {
    pub fn load(runtime_dir: &Path) -> Result<Self, MediaError> {
        let runtime_dir = absolute_path(runtime_dir)?;
        #[derive(Deserialize)]
        struct Metadata {
            #[serde(rename = "systemDefaults")]
            defaults: SystemDefaults,
        }
        let bytes = read_required(
            &runtime_dir.join("src/config/metadata.json"),
            "runtime metadata",
            1024 * 1024,
        )?;
        let data: Metadata = serde_json::from_slice(&bytes)
            .map_err(|_| MediaError::InvalidMetadata("runtime defaults"))?;
        let defaults = data.defaults;
        for version in [
            &defaults.uefi_ver_pi3,
            &defaults.uefi_ver_pi4,
            &defaults.uefi_ver_pi5,
            &defaults.driver_ver,
        ] {
            validate_release_tag(version)?;
        }
        for repository in [
            &defaults.uefi_repo_pi3,
            &defaults.uefi_repo_pi4,
            &defaults.uefi_repo_pi5,
            &defaults.drivers_repo,
        ] {
            validate_repository(repository)?;
        }
        validate_digest(&defaults.pe_installer_sha256, 64, "PE installer hash")?;
        validate_digest(
            &defaults.uefi_shell_package_sha256,
            64,
            "UEFI shell package hash",
        )?;
        crate::provider::safe_url(&defaults.pe_installer_url)?;
        crate::provider::safe_url(&defaults.uefi_shell_package_url)?;
        BuildId::parse(&defaults.win10_oldest_build)?;
        if defaults.armv80_max_build < 10000 || defaults.win11_min_build < 10000 {
            return Err(MediaError::InvalidMetadata("runtime build limits"));
        }
        Ok(defaults)
    }

    pub fn uefi_version(&self, model: Model) -> &str {
        match model {
            Model::Pi3 => &self.uefi_ver_pi3,
            Model::Pi4 => &self.uefi_ver_pi4,
            Model::Pi5 => &self.uefi_ver_pi5,
        }
    }

    pub fn uefi_repository(&self, model: Model) -> &str {
        match model {
            Model::Pi3 => &self.uefi_repo_pi3,
            Model::Pi4 => &self.uefi_repo_pi4,
            Model::Pi5 => &self.uefi_repo_pi5,
        }
    }
}

#[derive(Clone, Debug)]
pub struct DesktopOptions {
    pub runtime_dir: PathBuf,
    pub download_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub model: Model,
    pub build: Option<BuildId>,
    pub windows_version: WindowsVersion,
    pub language: String,
    pub source: WindowsSource,
    pub cache_mode: CacheMode,
    pub installation_mode: InstallationMode,
    pub verify_tls: bool,
    pub packages: PackagePolicy,
    pub setup: SetupOptions,
    pub boot: BootOptions,
    pub defaults: SystemDefaults,
}

impl DesktopOptions {
    pub fn validate(&self) -> Result<(), MediaError> {
        let canonical = SystemDefaults::load(&self.runtime_dir)?;
        if self.defaults.armv80_max_build > canonical.armv80_max_build {
            return Err(option(
                "ARMV80_MAX_BUILD",
                "cannot widen the reviewed CPU compatibility ceiling",
            ));
        }
        if !is_known_language(&self.language) {
            return Err(option(
                "WIN_LANG",
                "expected a published Windows language code",
            ));
        }
        if let Some(build) = &self.build
            && !self.model.supports(build, self.defaults.armv80_max_build)
        {
            return Err(option(
                "BID",
                "this build requires ARMv8.1; Pi 3 and Pi 4 are ARMv8.0",
            ));
        }
        if self.setup.locale.as_ref().is_some_and(|s| !valid_locale(s)) {
            return Err(option("WINDOWS_LOCALE", "invalid Windows locale"));
        }
        if self.setup.account.as_ref().is_some_and(|(user, password)| {
            user.is_empty()
                || password.0.is_empty()
                || user.chars().any(invalid_xml_character)
                || password.0.chars().any(invalid_xml_character)
        }) {
            return Err(option(
                "WINDOWS_ACCOUNT_SETUP",
                "a valid username and password are required",
            ));
        }
        validate_release_tag(&self.packages.uefi_version)?;
        validate_release_tag(&self.packages.driver_version)?;
        validate_digest(&self.packages.pe_sha256, 64, "PE installer hash")?;
        crate::provider::safe_url(&self.packages.pe_url)?;
        if !self.boot.settings.is_object() {
            return Err(option(
                "BOOT_CONFIG_SETTINGS",
                "expected a boot settings object",
            ));
        }
        if !matches!(
            self.boot.hdmi_mode.as_str(),
            "official" | "recommended" | "custom"
        ) {
            return Err(option(
                "HDMI_MODE",
                "expected official, recommended or custom",
            ));
        }
        Ok(())
    }

    /// Adapt already-resolved legacy settings. This does not resolve JSON, host
    /// safety, target selection, or device-write policy.
    pub fn from_compat(options: &PrepareOptions) -> Result<Self, MediaError> {
        let runtime_dir = absolute_path(&options.runtime_dir)?;
        let defaults = SystemDefaults::load(&runtime_dir)?;
        let get = |key: &str| options.settings.get(key).map(String::as_str).unwrap_or("");
        let value = |key: &str, default: &str| {
            let value = get(key);
            if value.is_empty() {
                default.to_owned()
            } else {
                value.to_owned()
            }
        };
        let boolean = |key: &'static str, default: bool| -> Result<bool, MediaError> {
            match get(key) {
                "" => Ok(default),
                "1" => Ok(true),
                "0" => Ok(false),
                _ => Err(option(key, "expected 0 or 1")),
            }
        };
        if !matches!(get("WOR_IMAGE_FAMILY"), "" | "desktop")
            || !matches!(get("WOR_IMAGE_ARCH"), "" | "arm64")
        {
            return Err(option(
                "WOR_IMAGE_FAMILY",
                "desktop preparation requires ARM64 Windows",
            ));
        }
        let model = Model::from_str(&value("RPI_MODEL", "4"))?;
        if !matches!(
            get("WOR_TARGET_BOARD"),
            "" | "pi2-v1.2" | "pi3-b" | "pi3-b-plus" | "pi4" | "pi400" | "pi5"
        ) || get("WOR_TARGET_BOARD") == "pi2-v1.2" && model != Model::Pi3
        {
            return Err(option(
                "WOR_TARGET_BOARD",
                "unsupported desktop board route",
            ));
        }
        let source = match get("SOURCE_FILE") {
            "" => WindowsSource::Download,
            name => {
                let path = absolute_path(Path::new(name))?;
                match path
                    .extension()
                    .and_then(|v| v.to_str())
                    .map(str::to_ascii_lowercase)
                    .as_deref()
                {
                    Some("iso") => WindowsSource::Iso(path),
                    Some("esd") => WindowsSource::Esd(path),
                    _ => return Err(option("SOURCE_FILE", "expected an ISO or ESD file")),
                }
            }
        };
        let windows_version = match get("BID") {
            "10" => WindowsVersion::Windows10,
            _ => WindowsVersion::Windows11,
        };
        let build = match get("BID") {
            "" | "10" | "11" => match &source {
                WindowsSource::Esd(path) | WindowsSource::Iso(path) => BuildId::from_filename(path),
                WindowsSource::Download => None,
            },
            value => Some(BuildId::parse(value)?),
        };
        let mut defaults = defaults;
        if !get("ARMV80_MAX_BUILD").is_empty() {
            let limit = get("ARMV80_MAX_BUILD")
                .parse::<u32>()
                .map_err(|_| option("ARMV80_MAX_BUILD", "expected a numeric build ceiling"))?;
            if limit < 10000 || limit > defaults.armv80_max_build {
                return Err(option(
                    "ARMV80_MAX_BUILD",
                    "cannot widen the reviewed CPU compatibility ceiling",
                ));
            }
            defaults.armv80_max_build = limit;
        }
        if !get("WIN11_MIN_BUILD").is_empty() {
            defaults.win11_min_build = get("WIN11_MIN_BUILD")
                .parse::<u32>()
                .ok()
                .filter(|value| *value >= 10000)
                .ok_or(option(
                    "WIN11_MIN_BUILD",
                    "expected a numeric Windows family threshold",
                ))?;
        }
        if !get("WIN10_OLDEST_BUILD").is_empty() {
            defaults.win10_oldest_build = BuildId::parse(get("WIN10_OLDEST_BUILD"))?.to_string();
        }
        if let Some(build) = &build
            && !model.supports(build, defaults.armv80_max_build)
        {
            return Err(option(
                "BID",
                "this build requires ARMv8.1; Pi 3 and Pi 4 are ARMv8.0",
            ));
        }
        let inferred_language = match &source {
            WindowsSource::Esd(path) | WindowsSource::Iso(path) => language_from_filename(path),
            WindowsSource::Download => None,
        };
        let language = value("WIN_LANG", inferred_language.as_deref().unwrap_or("en-us"));
        if !is_known_language(&language) {
            return Err(option(
                "WIN_LANG",
                "expected a published Windows language code",
            ));
        }
        let installation_mode = match get("CAN_INSTALL_ON_SAME_DRIVE") {
            "1" => InstallationMode::SelfInstall,
            "0" => InstallationMode::Recovery,
            _ => {
                return Err(option(
                    "CAN_INSTALL_ON_SAME_DRIVE",
                    "choose installation mode 0 or 1",
                ));
            }
        };
        let cache_mode = match get("USE_CACHE") {
            "" | "1" => CacheMode::Verify,
            "0" => CacheMode::Refresh,
            "2" => CacheMode::Trust,
            _ => return Err(option("USE_CACHE", "expected 0, 1 or 2")),
        };
        let account = if boolean("WINDOWS_ACCOUNT_SETUP", false)? {
            if get("WINDOWS_ACCOUNT_USERNAME").is_empty()
                || get("WINDOWS_ACCOUNT_PASSWORD").is_empty()
            {
                return Err(option(
                    "WINDOWS_ACCOUNT_SETUP",
                    "a username and password are required",
                ));
            }
            if get("WINDOWS_ACCOUNT_USERNAME")
                .chars()
                .any(invalid_xml_character)
                || get("WINDOWS_ACCOUNT_PASSWORD")
                    .chars()
                    .any(invalid_xml_character)
            {
                return Err(option(
                    "WINDOWS_ACCOUNT_SETUP",
                    "credentials contain an invalid XML character",
                ));
            }
            Some((
                get("WINDOWS_ACCOUNT_USERNAME").into(),
                Secret::new(get("WINDOWS_ACCOUNT_PASSWORD")),
            ))
        } else {
            None
        };
        let locale = if boolean("WINDOWS_LOCALE_SETUP", true)? {
            let host = ["LC_ALL", "LC_MESSAGES", "LANG"]
                .iter()
                .find_map(|name| std::env::var(name).ok().filter(|s| !s.is_empty()))
                .unwrap_or_default();
            let locale = value(
                "WINDOWS_LOCALE",
                &windows_locale(&crate::default_language(&host)),
            );
            if !valid_locale(&locale) {
                return Err(option(
                    "WINDOWS_LOCALE",
                    "expected a Windows locale such as en-US or sr-Latn-RS",
                ));
            }
            Some(locale)
        } else {
            None
        };
        let uefi_key = match model {
            Model::Pi3 => "UEFI_VER_PI3",
            Model::Pi4 => "UEFI_VER_PI4",
            Model::Pi5 => "UEFI_VER_PI5",
        };
        let packages = PackagePolicy {
            pe_latest: boolean("PE_USE_LATEST", true)?,
            pe_url: value("PE_INSTALLER_URL", &defaults.pe_installer_url),
            pe_sha256: value("PE_INSTALLER_SHA256", &defaults.pe_installer_sha256),
            uefi_latest: boolean("UEFI_USE_LATEST", model == Model::Pi3)?,
            uefi_version: value(uefi_key, defaults.uefi_version(model)),
            drivers_latest: boolean("DRIVERS_USE_LATEST", true)?,
            driver_version: value("DRIVER_VER", &defaults.driver_ver),
        };
        validate_release_tag(&packages.uefi_version)?;
        validate_release_tag(&packages.driver_version)?;
        validate_digest(&packages.pe_sha256, 64, "PE installer hash")?;
        crate::provider::safe_url(&packages.pe_url)?;
        let hdmi_mode = value(
            "HDMI_MODE",
            if model == Model::Pi4 {
                "recommended"
            } else {
                "official"
            },
        );
        if !matches!(hdmi_mode.as_str(), "official" | "recommended" | "custom") {
            return Err(option(
                "HDMI_MODE",
                "expected official, recommended or custom",
            ));
        }
        let boot_settings = match get("BOOT_CONFIG_SETTINGS") {
            "" => serde_json::json!({}),
            json => serde_json::from_str::<serde_json::Value>(json)
                .map_err(|_| option("BOOT_CONFIG_SETTINGS", "expected a boot settings object"))?,
        };
        if !boot_settings.is_object() {
            return Err(option(
                "BOOT_CONFIG_SETTINGS",
                "expected a boot settings object",
            ));
        }
        let download_dir = absolute_path(&options.download_dir)?;
        let cache_dir = if get("WOR_CACHE_DIR").is_empty() {
            download_dir.join(".wor-catalogues")
        } else {
            absolute_path(Path::new(get("WOR_CACHE_DIR")))?
        };
        Ok(Self {
            runtime_dir,
            download_dir,
            cache_dir,
            model,
            build,
            windows_version,
            language,
            source,
            cache_mode,
            installation_mode,
            verify_tls: boolean("VERIFY_TLS", true)?,
            packages,
            setup: SetupOptions {
                oobe_network_bypass: boolean("OOBE_NETWORK_BYPASS", true)?,
                account,
                locale,
                pi4_auto_disable_3gb: boolean("PI4_AUTO_DISABLE_3GB", true)?,
                pi4_uefi_shell_unlock: boolean("PI4_UEFI_SHELL_UNLOCK", false)?,
            },
            boot: BootOptions {
                apply_custom_config: boolean("APPLY_CUSTOM_CONFIG_TXT", true)?,
                config_txt: (!get("CONFIG_TXT").is_empty()).then(|| get("CONFIG_TXT").into()),
                hdmi_mode,
                settings: boot_settings,
                hide_empty_drives: boolean("HIDE_EMPTY_DRIVES", true)?,
            },
            defaults,
        })
    }
}

pub(crate) fn absolute_path(path: &Path) -> Result<PathBuf, MediaError> {
    if path.as_os_str().is_empty() || path.to_string_lossy().contains("://") {
        return Err(option(
            "path",
            "a nonempty local filesystem path is required",
        ));
    }
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .map_err(|e| MediaError::io("resolve preparation path", e))
    }
}

pub(crate) fn option(key: &'static str, reason: &'static str) -> MediaError {
    MediaError::InvalidOption { key, reason }
}

pub(crate) fn validate_release_tag(tag: &str) -> Result<(), MediaError> {
    let mut bytes = tag.bytes();
    if !bytes.next().is_some_and(|b| b.is_ascii_alphanumeric())
        || tag.len() > 200
        || !bytes.all(|b| b.is_ascii_alphanumeric() || b"._+-".contains(&b))
    {
        return Err(option(
            "release version",
            "expected one safe tag/asset-name component",
        ));
    }
    Ok(())
}

pub(crate) fn validate_repository(repository: &str) -> Result<(), MediaError> {
    let parts: Vec<_> = repository.split('/').collect();
    if parts.len() != 2
        || parts.iter().any(|p| {
            let mut bytes = p.bytes();
            !bytes.next().is_some_and(|b| b.is_ascii_alphanumeric())
                || !bytes.all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
        })
    {
        return Err(MediaError::InvalidMetadata("release repository"));
    }
    Ok(())
}

pub(crate) fn validate_digest(
    value: &str,
    length: usize,
    label: &'static str,
) -> Result<(), MediaError> {
    if value.len() != length || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(MediaError::InvalidMetadata(label));
    }
    Ok(())
}

pub(crate) fn valid_locale(value: &str) -> bool {
    let parts: Vec<_> = value.split('-').collect();
    (1..=3).contains(&parts.len())
        && (2..=3).contains(&parts[0].len())
        && parts[0].bytes().all(|b| b.is_ascii_alphabetic())
        && parts[1..]
            .iter()
            .all(|p| (2..=8).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_alphanumeric()))
}

fn invalid_xml_character(ch: char) -> bool {
    !matches!(ch, '\t' | '\n' | '\r' | '\u{20}'..='\u{d7ff}' | '\u{e000}'..='\u{fffd}' | '\u{10000}'..='\u{10ffff}')
}
