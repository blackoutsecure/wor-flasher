use crate::files::{finish_temp, make_directory, private_tempdir, required_file};
use crate::{
    BuildId, Language, MediaError, MediaProvider, NativeProvider, PrepareOptions, WimOperation,
    check_cancelled, languages, locales_from_xml, windows_locale,
};
use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use wor_domain::progress::Progress;

/// Regional/keyboard choices differ from the fixed ESD language list: cached
/// Windows media can declare additional installed locales. Preserve the source's
/// ISO-before-ESD, image-before-published ordering without downloading an image.
pub fn query_locale_options(
    options: &PrepareOptions,
    progress: &mut dyn FnMut(Progress),
    cancelled: &AtomicBool,
) -> Result<Vec<Language>, MediaError> {
    query_locale_options_with_provider(options, progress, cancelled, &mut NativeProvider::new()?)
}

pub fn query_locale_options_with_provider(
    options: &PrepareOptions,
    progress: &mut dyn FnMut(Progress),
    cancelled: &AtomicBool,
    provider: &mut dyn MediaProvider,
) -> Result<Vec<Language>, MediaError> {
    check_cancelled(cancelled)?;
    let known = languages();
    let mut codes = Vec::new();
    let mut seen = BTreeSet::new();
    if let Some(build) = options
        .settings
        .get("BID")
        .and_then(|s| BuildId::parse(s).ok())
    {
        let language = options
            .settings
            .get("WIN_LANG")
            .map(String::as_str)
            .unwrap_or("en-us");
        if known.iter().any(|item| item.code == language) {
            let images: Vec<_> = ["winfiles_from_iso_", "winfiles_"]
                .into_iter()
                .map(|prefix| {
                    options
                        .download_dir
                        .join(format!("{prefix}{build}_{language}"))
                })
                .filter(|directory| {
                    fs::symlink_metadata(directory)
                        .is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink())
                })
                .map(|directory| directory.join("install.wim"))
                .filter(|image| required_file(image, "cached locale image").is_ok())
                .collect();
            if !images.is_empty() {
                let cache = options
                    .settings
                    .get("WOR_CACHE_DIR")
                    .filter(|s| !s.is_empty())
                    .map(PathBuf::from)
                    .unwrap_or_else(|| options.download_dir.join(".wor-catalogues"));
                make_directory(&cache)?;
                let work = private_tempdir(&cache, ".locale-query-")?;
                progress(Progress::Status(
                    "Discovering Windows locales from cached images".into(),
                ));
                let result = (|| {
                    for image in images {
                        check_cancelled(cancelled)?;
                        let parsed = provider
                            .wim(
                                WimOperation::Info,
                                &[image.as_os_str().into(), "--xml".into()],
                                work.path(),
                                cancelled,
                            )
                            .and_then(|bytes| locales_from_xml(&bytes));
                        match parsed {
                            Ok(languages) => {
                                for code in languages {
                                    if seen.insert(code.clone()) { codes.push(code); }
                                }
                            }
                            Err(MediaError::Cancelled) => return Err(MediaError::Cancelled),
                            Err(error @ MediaError::Cleanup { .. }) => return Err(error),
                            Err(_) => progress(Progress::Status("Cached locale information is unavailable; retaining published language choices".into())),
                        }
                    }
                    Ok(())
                })();
                finish_temp(work, result)?;
            }
        }
    }
    for item in known
        .iter()
        .filter(|item| item.code == "en-us")
        .chain(
            known
                .iter()
                .filter(|item| item.code.starts_with("en-") && item.code != "en-us"),
        )
        .chain(known.iter().filter(|item| !item.code.starts_with("en-")))
    {
        if seen.insert(item.code.clone()) {
            codes.push(item.code.clone());
        }
    }
    check_cancelled(cancelled)?;
    Ok(codes
        .into_iter()
        .map(|code| {
            let locale = windows_locale(&code);
            let label = known
                .iter()
                .find(|item| item.code == code)
                .map(|item| item.label.as_str())
                .unwrap_or(&locale);
            Language {
                code,
                label: format!("{label} ({locale})"),
                locale,
            }
        })
        .collect())
}
