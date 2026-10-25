use crate::files::{make_directory, read_required, write_private};
use crate::options::{valid_locale, validate_digest, validate_release_tag};
use crate::provider::safe_url;
use crate::{
    BuildId, CacheMode, DownloadRequest, MediaError, MediaProvider, Model, NativeProvider,
    SystemDefaults, WindowsVersion, cache_is_current, check_cancelled,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use wor_domain::progress::Progress;

const LANGUAGES: &[(&str, &str)] = &[
    ("ar-sa", "Arabic (Saudi Arabia)"),
    ("bg-bg", "Bulgarian (Bulgaria)"),
    ("cs-cz", "Czech (Czechia)"),
    ("da-dk", "Danish (Denmark)"),
    ("de-de", "German (Germany)"),
    ("el-gr", "Greek (Greece)"),
    ("en-gb", "English (United Kingdom)"),
    ("en-us", "English (United States)"),
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

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Language {
    pub code: String,
    pub label: String,
    pub locale: String,
}

pub fn languages() -> Vec<Language> {
    LANGUAGES
        .iter()
        .map(|(code, label)| Language {
            code: (*code).into(),
            label: (*label).into(),
            locale: windows_locale(code),
        })
        .collect()
}

pub(crate) fn is_known_language(code: &str) -> bool {
    LANGUAGES.iter().any(|(known, _)| *known == code)
}

pub fn default_language(host_locale: &str) -> String {
    let normalized = host_locale
        .split(['@', '.'])
        .next()
        .unwrap_or("")
        .replace('_', "-")
        .to_ascii_lowercase();
    if is_known_language(&normalized) {
        normalized
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
                let lower = part.to_ascii_lowercase();
                let mut characters = lower.chars();
                match characters.next() {
                    Some(first) => format!("{}{}", first.to_ascii_uppercase(), characters.as_str()),
                    None => lower,
                }
            } else {
                part.to_ascii_uppercase()
            }
        })
        .collect::<Vec<_>>()
        .join("-")
}

pub(crate) fn language_from_filename(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_str()?.to_ascii_lowercase();
    LANGUAGES
        .iter()
        .find(|(code, _)| name.contains(code))
        .map(|(code, _)| (*code).into())
}

#[derive(Clone, Debug)]
pub struct QueryOptions {
    pub runtime_dir: PathBuf,
    pub cache_dir: PathBuf,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct BuildRelease {
    pub build: BuildId,
    pub date: String,
    pub version: WindowsVersion,
}

pub fn parse_build_releases(
    bytes: &[u8],
    version: WindowsVersion,
    model: Model,
    defaults: &SystemDefaults,
) -> Result<Vec<BuildRelease>, MediaError> {
    let document = crate::xml::parse(bytes)?;
    let mut versions = Vec::new();
    document.descendants("version", &mut versions);
    if document.name == "version" {
        versions.insert(0, &document);
    }
    let number = if version == WindowsVersion::Windows11 {
        "11"
    } else {
        "10"
    };
    let selected = versions
        .into_iter()
        .find(|node| node.attributes.get("number").is_some_and(|n| n == number))
        .ok_or(MediaError::InvalidMetadata("Windows version catalogue"))?;
    let mut releases = Vec::new();
    let mut seen = BTreeSet::new();
    for release in &selected.children {
        if release.name != "release" {
            continue;
        }
        let build = BuildId::parse(
            release
                .attributes
                .get("build")
                .ok_or(MediaError::InvalidMetadata("Windows release build"))?,
        )?;
        let date = release
            .value("date")
            .ok_or(MediaError::InvalidMetadata("Windows release date"))?;
        let date = if date.len() == 8 && date.bytes().all(|b| b.is_ascii_digit()) {
            format!("{}-{}-{}", &date[..4], &date[4..6], &date[6..])
        } else if date.len() == 10
            && date.as_bytes()[4] == b'-'
            && date.as_bytes()[7] == b'-'
            && date
                .bytes()
                .enumerate()
                .all(|(i, b)| i == 4 || i == 7 || b.is_ascii_digit())
        {
            date.into()
        } else {
            return Err(MediaError::InvalidMetadata("Windows release date"));
        };
        if seen.insert(build.clone()) && model.supports(&build, defaults.armv80_max_build) {
            releases.push(BuildRelease {
                build: build.clone(),
                date,
                version,
            });
        }
        if version == WindowsVersion::Windows10 && build.as_str() == defaults.win10_oldest_build {
            break;
        }
    }
    Ok(releases)
}

/// Synchronous in-process query boundary for the guided worker. Build results
/// combine both Windows families, preserving each family's published order.
pub fn catalog(
    options: &crate::PrepareOptions,
    kind: &str,
    progress: &mut dyn FnMut(Progress),
    cancelled: &AtomicBool,
) -> Result<serde_json::Value, MediaError> {
    check_cancelled(cancelled)?;
    if !matches!(kind, "builds" | "uefi" | "drivers" | "languages") {
        return Err(crate::options::option(
            "catalog kind",
            "expected builds, uefi, drivers or languages",
        ));
    }
    if kind == "languages" {
        return serde_json::to_value(languages())
            .map_err(|_| MediaError::InvalidMetadata("language query JSON"));
    }
    catalog_with_provider(
        options,
        kind,
        progress,
        cancelled,
        &mut NativeProvider::new()?,
    )
}

pub fn catalog_with_provider(
    options: &crate::PrepareOptions,
    kind: &str,
    progress: &mut dyn FnMut(Progress),
    cancelled: &AtomicBool,
    provider: &mut dyn MediaProvider,
) -> Result<serde_json::Value, MediaError> {
    check_cancelled(cancelled)?;
    if !matches!(kind, "builds" | "uefi" | "drivers" | "languages") {
        return Err(crate::options::option(
            "catalog kind",
            "expected builds, uefi, drivers or languages",
        ));
    }
    if kind == "languages" {
        return serde_json::to_value(languages())
            .map_err(|_| MediaError::InvalidMetadata("language query JSON"));
    }
    let model = options
        .settings
        .get("RPI_MODEL")
        .filter(|s| !s.is_empty())
        .map(String::as_str)
        .unwrap_or("4")
        .parse::<Model>()?;
    let query = QueryOptions {
        runtime_dir: options.runtime_dir.clone(),
        cache_dir: match options
            .settings
            .get("WOR_CACHE_DIR")
            .filter(|s| !s.is_empty())
        {
            Some(path) => PathBuf::from(path),
            None => options.download_dir.join(".wor-catalogues"),
        },
    };
    progress(Progress::Status(
        match kind {
            "builds" => "Querying compatible Windows builds",
            "uefi" => "Querying compatible UEFI firmware releases",
            _ => "Querying compatible ARM64 driver releases",
        }
        .into(),
    ));
    let value = if kind == "builds" {
        let mut defaults = SystemDefaults::load(&options.runtime_dir)?;
        if let Some(limit) = options
            .settings
            .get("ARMV80_MAX_BUILD")
            .filter(|s| !s.is_empty())
        {
            let limit = limit.parse::<u32>().map_err(|_| {
                crate::options::option("ARMV80_MAX_BUILD", "expected a numeric build ceiling")
            })?;
            if limit < 10000 || limit > defaults.armv80_max_build {
                return Err(crate::options::option(
                    "ARMV80_MAX_BUILD",
                    "cannot widen the reviewed CPU compatibility ceiling",
                ));
            }
            defaults.armv80_max_build = limit;
        }
        if let Some(floor) = options
            .settings
            .get("WIN10_OLDEST_BUILD")
            .filter(|s| !s.is_empty())
        {
            defaults.win10_oldest_build = BuildId::parse(floor)?.to_string();
        }
        let (bytes, _) = cached_text(
            "https://worproject.com/dldserv/esd/getversions.php",
            "Windows build catalogue",
            &query.cache_dir,
            cancelled,
            provider,
        )?;
        let mut releases =
            parse_build_releases(&bytes, WindowsVersion::Windows11, model, &defaults)?;
        releases.extend(parse_build_releases(
            &bytes,
            WindowsVersion::Windows10,
            model,
            &defaults,
        )?);
        serde_json::to_value(releases)
    } else {
        serde_json::to_value(query_release_versions_with_provider(
            &query,
            if kind == "uefi" {
                ReleaseKind::Uefi
            } else {
                ReleaseKind::Drivers
            },
            model,
            ReleaseSelection::All,
            cancelled,
            provider,
        )?)
    };
    check_cancelled(cancelled)?;
    value.map_err(|_| MediaError::InvalidMetadata("desktop catalogue JSON"))
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CatalogueEntry {
    pub language: String,
    pub url: String,
    pub size: u64,
    pub sha1: Option<String>,
    pub sha256: Option<String>,
    pub cached_metadata: bool,
}

impl fmt::Debug for CatalogueEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CatalogueEntry")
            .field("language", &self.language)
            .field("url", &"[redacted]")
            .field("size", &self.size)
            .field("sha1", &self.sha1)
            .field("sha256", &self.sha256)
            .field("cached_metadata", &self.cached_metadata)
            .finish()
    }
}

/// Select the first complete matching File before the summary Languages block.
pub fn get_esd_catalog_entry(bytes: &[u8], language: &str) -> Result<CatalogueEntry, MediaError> {
    if !is_known_language(language) {
        return Err(MediaError::InvalidMetadata("catalogue language"));
    }
    let document = crate::xml::parse(bytes)?;
    fn select<'a>(
        node: &'a crate::xml::Node,
        language: &str,
        ended: &mut bool,
    ) -> Option<&'a crate::xml::Node> {
        if node.name == "Languages" {
            *ended = true;
            return None;
        }
        if *ended {
            return None;
        }
        if node.name == "File" && node.value("LanguageCode") == Some(language) {
            return Some(node);
        }
        for child in &node.children {
            if let Some(node) = select(child, language, ended) {
                return Some(node);
            }
            if *ended {
                break;
            }
        }
        None
    }
    let file = select(&document, language, &mut false).ok_or(MediaError::InvalidMetadata(
        "selected language has no ESD file",
    ))?;
    let url = file
        .value("FilePath")
        .ok_or(MediaError::InvalidMetadata("ESD URL"))?;
    safe_url(url)?;
    let size: u64 = file
        .value("Size")
        .and_then(|s| s.parse().ok())
        .filter(|s| *s > 0 && *s <= 32 * 1024 * 1024 * 1024)
        .ok_or(MediaError::InvalidMetadata("ESD size"))?;
    let hash = |name, length| -> Result<Option<String>, MediaError> {
        match file.value(name).filter(|s| !s.is_empty()) {
            Some(value) => {
                validate_digest(value, length, "ESD digest")?;
                Ok(Some(value.to_ascii_lowercase()))
            }
            None => Ok(None),
        }
    };
    let sha1 = hash("Sha1", 40)?;
    let sha256 = hash("Sha256", 64)?;
    if sha1.is_none() && sha256.is_none() {
        return Err(MediaError::InvalidMetadata("ESD requires SHA-1 or SHA-256"));
    }
    Ok(CatalogueEntry {
        language: language.into(),
        url: url.into(),
        size,
        sha1,
        sha256,
        cached_metadata: false,
    })
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub enum ReleaseKind {
    Uefi,
    Drivers,
}

impl ReleaseKind {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Uefi => "uefi",
            Self::Drivers => "drivers",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReleaseSelection {
    All,
    Latest,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ReleaseVersion {
    pub tag: String,
    pub asset_name: String,
    pub download_url: String,
}

pub(crate) fn release_source(
    kind: ReleaseKind,
    model: Model,
    defaults: &SystemDefaults,
) -> Result<(&str, String), MediaError> {
    match (kind, model) {
        (ReleaseKind::Uefi, model) => Ok((
            defaults.uefi_repository(model),
            format!(
                "RPi{}_UEFI_{}_",
                model.number(),
                if model == Model::Pi5 {
                    "Release"
                } else {
                    "Firmware"
                }
            ),
        )),
        (ReleaseKind::Drivers, Model::Pi3 | Model::Pi4) => Ok((
            &defaults.drivers_repo,
            format!("RPi{}_Windows_ARM64_Drivers_", model.number()),
        )),
        (ReleaseKind::Drivers, Model::Pi5) => Err(MediaError::InvalidMetadata(
            "Pi 5 has no separate ARM64 driver package",
        )),
    }
}

pub(crate) fn pinned_release(
    kind: ReleaseKind,
    model: Model,
    tag: &str,
    defaults: &SystemDefaults,
) -> Result<ReleaseVersion, MediaError> {
    validate_release_tag(tag)?;
    let (repository, prefix) = release_source(kind, model, defaults)?;
    let asset_name = format!("{prefix}{tag}.zip");
    Ok(ReleaseVersion {
        tag: tag.into(),
        download_url: format!(
            "https://github.com/{repository}/releases/download/{tag}/{asset_name}"
        ),
        asset_name,
    })
}

pub fn parse_release_page(
    bytes: &[u8],
    kind: ReleaseKind,
    model: Model,
    defaults: &SystemDefaults,
) -> Result<Vec<ReleaseVersion>, MediaError> {
    #[derive(Deserialize)]
    struct Release {
        tag_name: String,
        draft: bool,
        prerelease: bool,
        assets: Vec<serde_json::Value>,
    }
    let page: Vec<Release> =
        serde_json::from_slice(bytes).map_err(|_| MediaError::InvalidMetadata("GitHub release"))?;
    let (_, prefix) = release_source(kind, model, defaults)?;
    let mut tags = BTreeSet::new();
    let mut releases = Vec::new();
    for release in page {
        if release.draft || release.prerelease || validate_release_tag(&release.tag_name).is_err() {
            continue;
        }
        let expected = format!("{prefix}{}.zip", release.tag_name);
        if release
            .assets
            .iter()
            .any(|a| a["name"].as_str() == Some(&expected) && a["state"] == "uploaded")
            && tags.insert(release.tag_name.clone())
        {
            releases.push(pinned_release(kind, model, &release.tag_name, defaults)?);
        }
    }
    Ok(releases)
}

pub fn query_builds(
    options: &QueryOptions,
    version: WindowsVersion,
    model: Model,
    cancelled: &AtomicBool,
) -> Result<Vec<BuildRelease>, MediaError> {
    query_builds_with_provider(
        options,
        version,
        model,
        cancelled,
        &mut NativeProvider::new()?,
    )
}

pub fn query_builds_with_provider(
    options: &QueryOptions,
    version: WindowsVersion,
    model: Model,
    cancelled: &AtomicBool,
    provider: &mut dyn MediaProvider,
) -> Result<Vec<BuildRelease>, MediaError> {
    let defaults = SystemDefaults::load(&options.runtime_dir)?;
    let (bytes, _) = cached_text(
        "https://worproject.com/dldserv/esd/getversions.php",
        "Windows build catalogue",
        &options.cache_dir,
        cancelled,
        provider,
    )?;
    parse_build_releases(&bytes, version, model, &defaults)
}

pub fn query_catalogue(
    options: &QueryOptions,
    build: &BuildId,
    language: &str,
    cancelled: &AtomicBool,
) -> Result<CatalogueEntry, MediaError> {
    query_catalogue_with_provider(
        options,
        build,
        language,
        cancelled,
        &mut NativeProvider::new()?,
    )
}

pub fn query_catalogue_with_provider(
    options: &QueryOptions,
    build: &BuildId,
    language: &str,
    cancelled: &AtomicBool,
    provider: &mut dyn MediaProvider,
) -> Result<CatalogueEntry, MediaError> {
    let url = format!(
        "https://worproject.com/dldserv/esd/getcatalog.php?build={build}&arch=ARM64&edition=Professional"
    );
    let (bytes, cached_metadata) = cached_text(
        &url,
        "Windows ESD catalogue",
        &options.cache_dir,
        cancelled,
        provider,
    )?;
    let mut entry = get_esd_catalog_entry(&bytes, language)?;
    entry.cached_metadata = cached_metadata;
    Ok(entry)
}

pub fn query_release_versions(
    options: &QueryOptions,
    kind: ReleaseKind,
    model: Model,
    selection: ReleaseSelection,
    cancelled: &AtomicBool,
) -> Result<Vec<ReleaseVersion>, MediaError> {
    query_release_versions_with_provider(
        options,
        kind,
        model,
        selection,
        cancelled,
        &mut NativeProvider::new()?,
    )
}

pub fn query_release_versions_with_provider(
    options: &QueryOptions,
    kind: ReleaseKind,
    model: Model,
    selection: ReleaseSelection,
    cancelled: &AtomicBool,
    provider: &mut dyn MediaProvider,
) -> Result<Vec<ReleaseVersion>, MediaError> {
    check_cancelled(cancelled)?;
    let defaults = SystemDefaults::load(&options.runtime_dir)?;
    let (repository, _) = release_source(kind, model, &defaults)?;
    let cache_dir = options
        .cache_dir
        .join("github-releases")
        .join(repository)
        .join(format!("pi{}", model.number()));
    make_directory(&cache_dir)?;
    let cache = cache_dir.join(format!(
        "{}{}.json",
        kind.name(),
        if selection == ReleaseSelection::Latest {
            "-latest"
        } else {
            ""
        }
    ));
    #[derive(Serialize, Deserialize)]
    struct Cache {
        fetched: u64,
        versions: Vec<String>,
    }
    let now = provider.unix_time();
    if let Ok(bytes) = read_required(&cache, "release catalogue cache", 1024 * 1024)
        && let Ok(cached) = serde_json::from_slice::<Cache>(&bytes)
        && now.checked_sub(cached.fetched).is_some_and(|age| age < 300)
        && !cached.versions.is_empty()
        && (selection != ReleaseSelection::Latest || cached.versions.len() == 1)
        && cached
            .versions
            .iter()
            .all(|tag| validate_release_tag(tag).is_ok())
    {
        return cached
            .versions
            .iter()
            .map(|tag| pinned_release(kind, model, tag, &defaults))
            .collect();
    }
    let mut releases = Vec::new();
    let mut seen = BTreeSet::new();
    let mut complete = false;
    for page in 1..=10 {
        let url = if selection == ReleaseSelection::Latest {
            format!("https://api.github.com/repos/{repository}/releases/latest")
        } else {
            format!("https://api.github.com/repos/{repository}/releases?per_page=100&page={page}")
        };
        let bytes = fetch_metadata(&url, "GitHub releases", &cache_dir, cancelled, provider)?;
        let value: serde_json::Value = serde_json::from_slice(&bytes)
            .map_err(|_| MediaError::InvalidMetadata("GitHub release"))?;
        let value = if selection == ReleaseSelection::Latest {
            if !value.is_object() {
                return Err(MediaError::InvalidMetadata("latest GitHub release"));
            }
            serde_json::json!([value])
        } else {
            value
        };
        let count = value
            .as_array()
            .ok_or(MediaError::InvalidMetadata("GitHub release page"))?
            .len();
        let parsed = parse_release_page(
            &serde_json::to_vec(&value)
                .map_err(|_| MediaError::InvalidMetadata("GitHub release serialization"))?,
            kind,
            model,
            &defaults,
        )?;
        for release in parsed {
            if seen.insert(release.tag.clone()) {
                releases.push(release);
            }
        }
        if selection == ReleaseSelection::Latest || count < 100 {
            complete = true;
            break;
        }
    }
    if !complete {
        return Err(MediaError::InvalidMetadata(
            "release history exceeds ten pages; incomplete list refused",
        ));
    }
    if releases.is_empty() {
        return Err(MediaError::InvalidMetadata(
            "no published compatible release package",
        ));
    }
    let cache_data = Cache {
        fetched: now,
        versions: releases.iter().map(|v| v.tag.clone()).collect(),
    };
    write_private(
        &cache,
        &serde_json::to_vec(&cache_data)
            .map_err(|_| MediaError::InvalidMetadata("release cache"))?,
    )?;
    Ok(releases)
}

pub(crate) fn fetch_metadata(
    url: &str,
    component: &'static str,
    work: &Path,
    cancelled: &AtomicBool,
    provider: &mut dyn MediaProvider,
) -> Result<Vec<u8>, MediaError> {
    check_cancelled(cancelled)?;
    make_directory(work)?;
    let staging = crate::files::private_tempdir(work, ".metadata-")?;
    let path = staging.path().join("response");
    let result = provider
        .fetch(
            &DownloadRequest {
                url: safe_url(url)?,
                component,
                verify_tls: true,
                maximum_bytes: 32 * 1024 * 1024,
                timeout: std::time::Duration::from_secs(8),
            },
            &path,
            cancelled,
        )
        .and_then(|_| read_required(&path, component, 32 * 1024 * 1024));
    crate::files::finish_temp(staging, result)
}

fn cached_text(
    url: &str,
    component: &'static str,
    cache_dir: &Path,
    cancelled: &AtomicBool,
    provider: &mut dyn MediaProvider,
) -> Result<(Vec<u8>, bool), MediaError> {
    make_directory(cache_dir)?;
    let name = format!("{:x}", Sha256::digest(url.as_bytes()));
    let path = cache_dir.join(format!("{name}.xml"));
    let checksum = cache_dir.join(format!("{name}.sha256"));
    match fetch_metadata(url, component, cache_dir, cancelled, provider) {
        Ok(bytes) => {
            crate::xml::parse(&bytes)?;
            write_private(&path, &bytes)?;
            write_private(
                &checksum,
                format!("{:x}", Sha256::digest(&bytes)).as_bytes(),
            )?;
            Ok((bytes, false))
        }
        Err(MediaError::Cancelled) => Err(MediaError::Cancelled),
        Err(error) => {
            let bytes = read_required(&path, component, 32 * 1024 * 1024);
            let digest = read_required(&checksum, "metadata cache digest", 128);
            match (bytes, digest) {
                (Ok(bytes), Ok(digest))
                    if format!("{:x}", Sha256::digest(&bytes)).as_bytes() == digest =>
                {
                    Ok((bytes, true))
                }
                _ => Err(error),
            }
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct CachedWindowsMedia {
    pub directory: PathBuf,
    pub build: BuildId,
    pub language: String,
    pub from_iso: bool,
    pub integrity_verified: bool,
}

pub fn cached_windows_media(
    download_dir: &Path,
    cancelled: &AtomicBool,
) -> Result<Vec<CachedWindowsMedia>, MediaError> {
    check_cancelled(cancelled)?;
    if !download_dir.exists() {
        return Ok(Vec::new());
    }
    let mut result = Vec::new();
    for entry in
        fs::read_dir(download_dir).map_err(|e| MediaError::io("list cached Windows media", e))?
    {
        check_cancelled(cancelled)?;
        let entry = entry.map_err(|e| MediaError::io("read cached Windows entry", e))?;
        if !entry
            .file_type()
            .map_err(|e| MediaError::io("inspect cached Windows entry", e))?
            .is_dir()
        {
            continue;
        }
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let (suffix, from_iso) = if let Some(suffix) = name.strip_prefix("winfiles_from_iso_") {
            (suffix, true)
        } else if let Some(suffix) = name.strip_prefix("winfiles_") {
            (suffix, false)
        } else {
            continue;
        };
        let Some((build, language)) = suffix.split_once('_') else {
            continue;
        };
        let Ok(build) = BuildId::parse(build) else {
            continue;
        };
        if !is_known_language(language)
            || !entry.path().join("alldone").is_file()
            || !entry.path().join("install.wim").is_file()
            || !entry.path().join("bootpart/sources/boot.wim").is_file()
        {
            continue;
        }
        let token = read_required(
            &entry.path().join(".wor-flasher-version"),
            "cache version",
            16 * 1024,
        )
        .ok();
        let verified = match token.and_then(|bytes| String::from_utf8(bytes).ok()) {
            Some(token) => {
                cache_is_current(&entry.path(), token.trim(), CacheMode::Verify, cancelled)?
            }
            None => false,
        };
        result.push(CachedWindowsMedia {
            directory: entry.path(),
            build,
            language: language.into(),
            from_iso,
            integrity_verified: verified,
        });
    }
    result.sort_by(|a, b| b.directory.cmp(&a.directory));
    Ok(result)
}

pub fn locales_from_xml(bytes: &[u8]) -> Result<Vec<String>, MediaError> {
    let document = crate::xml::parse(bytes)?;
    let mut language_nodes = Vec::new();
    document.descendants("LANGUAGE", &mut language_nodes);
    document.descendants("DEFAULT", &mut language_nodes);
    let mut seen = BTreeSet::new();
    Ok(language_nodes
        .into_iter()
        .filter_map(|node| {
            let code = node.text.trim().to_ascii_lowercase();
            (valid_locale(&code) && seen.insert(code.clone())).then_some(code)
        })
        .collect())
}
