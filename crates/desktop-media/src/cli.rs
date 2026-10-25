use crate::files::{make_directory, read_required};
use crate::options::option;
use crate::{
    BuildId, MediaError, MediaProvider, Model, NativeProvider, PrepareOptions, QueryOptions,
    ReleaseKind, ReleaseSelection, StagedMedia, WindowsVersion, cached_windows_media, languages,
    prepare_with_provider, query_builds_with_provider, query_catalogue_with_provider,
    query_release_versions_with_provider,
};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::io::Write;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::atomic::AtomicBool;
use wor_domain::progress::Progress;

/// Fixed native helper boundary for the root `desktop-media` dispatcher.
/// stdout is one JSON document; progress uses the existing stderr tab protocol.
/// The host still owns signal handling and the worker process group.
pub fn run(args: &[OsString]) -> Result<i32, MediaError> {
    let mut output = std::io::stdout().lock();
    let mut progress = |event: Progress| match event {
        Progress::Status(message) => eprintln!("STATUS\t{message}"),
        Progress::Step {
            current,
            total,
            message,
        } => eprintln!("STEP\t{current}\t{total}\t{message}"),
        Progress::Substep(percent) => eprintln!("SUBSTEP\t{percent}"),
        Progress::Task { percent, label } => eprintln!("TASK\t{percent}\t{label}"),
        Progress::DiskWrite { active, device } => {
            eprintln!("DISK_WRITE\t{}\t{}", u8::from(active), device)
        }
    };
    run_with_io(
        args,
        &mut output,
        &mut progress,
        &AtomicBool::new(false),
        &mut NativeProvider::new()?,
    )
}

pub fn run_with_io(
    args: &[OsString],
    output: &mut dyn Write,
    progress: &mut dyn FnMut(Progress),
    cancelled: &AtomicBool,
    provider: &mut dyn MediaProvider,
) -> Result<i32, MediaError> {
    crate::check_cancelled(cancelled)?;
    let command = args.first().and_then(|a| a.to_str()).ok_or(option(
        "desktop-media arguments",
        "a native helper command is required",
    ))?;
    let allowed: &[&str] = match command {
        "help" | "--help" | "languages" => &[],
        "builds" => &["runtime-dir", "cache-dir", "model", "version"],
        "releases" => &["runtime-dir", "cache-dir", "model", "kind", "latest"],
        "catalogue" => &["runtime-dir", "cache-dir", "build", "language"],
        "cached" => &["download-dir"],
        "prepare" => &["runtime-dir", "download-dir", "settings", "output-dir"],
        "cleanup" => &["staging-dir"],
        _ => {
            return Err(option(
                "desktop-media arguments",
                "unknown native helper command",
            ));
        }
    };
    let mut flags = BTreeMap::new();
    let mut seen = BTreeSet::new();
    let mut index = 1;
    while index < args.len() {
        let flag = args[index]
            .to_str()
            .and_then(|s| s.strip_prefix("--"))
            .ok_or(option("desktop-media arguments", "expected a named option"))?;
        if !seen.insert(flag) || flag != "json" && !allowed.contains(&flag) {
            return Err(option(
                "desktop-media arguments",
                "unknown or repeated option",
            ));
        }
        index += 1;
        if matches!(flag, "latest" | "json") {
            flags.insert(flag, OsString::from("1"));
        } else {
            let value = args
                .get(index)
                .filter(|s| !s.is_empty())
                .filter(|s| !s.to_string_lossy().starts_with("--"))
                .ok_or(option(
                    "desktop-media arguments",
                    "an option value is missing",
                ))?;
            flags.insert(flag, value.clone());
            index += 1;
        }
    }
    let get = |key: &str| -> Result<&OsString, MediaError> {
        flags.get(key).ok_or(option(
            "desktop-media arguments",
            "a required option is missing",
        ))
    };
    let text = |key: &str| -> Result<&str, MediaError> {
        get(key)?.to_str().ok_or(option(
            "desktop-media arguments",
            "expected UTF-8 option text",
        ))
    };
    let query = || -> Result<QueryOptions, MediaError> {
        Ok(QueryOptions {
            runtime_dir: PathBuf::from(get("runtime-dir")?),
            cache_dir: PathBuf::from(get("cache-dir")?),
        })
    };
    let result = match command {
        "help" | "--help" => serde_json::json!({
            "commands":["languages","builds","releases","catalogue","cached","prepare","cleanup"],
            "writes_devices":false,
            "settings_format":"flat normalized string map",
            "prepare_output":"explicit new private directory on the download filesystem; cleanup requires its ownership receipt",
        }),
        "languages" => json(languages())?,
        "builds" => {
            let version = match text("version")? {
                "10" => WindowsVersion::Windows10,
                "11" => WindowsVersion::Windows11,
                _ => return Err(option("version", "expected Windows 10 or 11")),
            };
            json(query_builds_with_provider(
                &query()?,
                version,
                Model::from_str(text("model")?)?,
                cancelled,
                provider,
            )?)?
        }
        "releases" => {
            let kind = match text("kind")? {
                "uefi" => ReleaseKind::Uefi,
                "drivers" => ReleaseKind::Drivers,
                _ => return Err(option("kind", "expected uefi or drivers")),
            };
            let selection = if flags.contains_key("latest") {
                ReleaseSelection::Latest
            } else {
                ReleaseSelection::All
            };
            json(query_release_versions_with_provider(
                &query()?,
                kind,
                Model::from_str(text("model")?)?,
                selection,
                cancelled,
                provider,
            )?)?
        }
        "catalogue" => json(query_catalogue_with_provider(
            &query()?,
            &BuildId::parse(text("build")?)?,
            text("language")?,
            cancelled,
            provider,
        )?)?,
        "cached" => json(cached_windows_media(
            &PathBuf::from(get("download-dir")?),
            cancelled,
        )?)?,
        "prepare" => {
            let settings = read_required(
                &PathBuf::from(get("settings")?),
                "normalized desktop settings",
                1024 * 1024,
            )?;
            let settings: BTreeMap<String, String> = serde_json::from_slice(&settings)
                .map_err(|_| MediaError::InvalidMetadata("normalized desktop settings JSON"))?;
            let download_dir = PathBuf::from(get("download-dir")?);
            let output_dir = PathBuf::from(get("output-dir")?);
            if output_dir.exists() {
                return Err(option(
                    "output-dir",
                    "the private preparation output must not already exist",
                ));
            }
            make_directory(&download_dir)?;
            let prepared = prepare_with_provider(
                &PrepareOptions {
                    runtime_dir: PathBuf::from(get("runtime-dir")?),
                    download_dir,
                    settings,
                },
                progress,
                cancelled,
                provider,
            )?;
            json(prepared.persist_to(&output_dir)?)?
        }
        "cleanup" => {
            StagedMedia::load(&PathBuf::from(get("staging-dir")?))?.cleanup()?;
            serde_json::json!({"cleaned":true})
        }
        _ => unreachable!(),
    };
    serde_json::to_writer(&mut *output, &result).map_err(|_| {
        MediaError::io(
            "write desktop helper JSON",
            std::io::Error::other("JSON output failed"),
        )
    })?;
    output
        .write_all(b"\n")
        .map_err(|e| MediaError::io("finish desktop helper JSON", e))?;
    Ok(0)
}

fn json(value: impl serde::Serialize) -> Result<serde_json::Value, MediaError> {
    serde_json::to_value(value).map_err(|_| MediaError::InvalidMetadata("desktop helper JSON"))
}
