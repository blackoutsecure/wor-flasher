//! Bash is used only as a frozen behavioral oracle in tests, never by the
//! production library or its native image providers.
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::process::{Command, Output};
use std::sync::atomic::AtomicBool;
use wor_desktop_media::*;

fn workspace() -> tempfile::TempDir {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/test-work");
    fs::create_dir_all(&root).unwrap();
    tempfile::Builder::new()
        .prefix("source-oracle-")
        .tempdir_in(root)
        .unwrap()
}

fn runtime() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn oracle(directory: &Path, body: &str, settings: &BTreeMap<String, String>) -> Output {
    let result = Command::new("bash")
        .arg("-c")
        .arg(format!(
            "source \"$DIRECTORY/install-wor.sh\" source >/dev/null || exit 90\n{body}"
        ))
        .env("DIRECTORY", runtime())
        .env("WOR_CACHE_DIR", directory.join("cache"))
        .env("NO_UPDATE", "1")
        .env("CHECK_FOR_UPDATES", "0")
        .env("TMPDIR", directory)
        .envs(settings)
        .current_dir(directory)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "source oracle failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    result
}

#[test]
fn source_differential_language_catalogue_locale_casing_and_cpu_guards() {
    let root = workspace();
    let result = oracle(root.path(), "list_langs", &BTreeMap::new());
    let expected = languages()
        .into_iter()
        .map(|language| format!("{}:{}\n", language.code, language.label))
        .collect::<String>();
    assert_eq!(String::from_utf8(result.stdout).unwrap(), expected);
    for code in ["en-us", "en-gb", "sr-latn-rs", "zh-cn"] {
        let result = oracle(
            root.path(),
            "windows_locale_from_language_code \"$ORACLE_CODE\"",
            &BTreeMap::from([("ORACLE_CODE".into(), code.into())]),
        );
        assert_eq!(
            String::from_utf8(result.stdout).unwrap().trim(),
            windows_locale(code)
        );
    }
    let defaults = SystemDefaults::load(&runtime()).unwrap();
    for model in [Model::Pi3, Model::Pi4, Model::Pi5] {
        for build in ["19045.3803", "22631.2861", "25163.1", "25164.1", "26100.1"] {
            let result = oracle(
                root.path(),
                "if cpu_supports_bid \"$ORACLE_BUILD\"; then printf yes; else printf no; fi",
                &BTreeMap::from([
                    ("RPI_MODEL".into(), model.number().to_string()),
                    ("ORACLE_BUILD".into(), build.into()),
                ]),
            );
            assert_eq!(
                String::from_utf8(result.stdout).unwrap(),
                if model.supports(&BuildId::parse(build).unwrap(), defaults.armv80_max_build) {
                    "yes"
                } else {
                    "no"
                }
            );
        }
    }
}

#[test]
fn source_differential_all_windows_setup_branches_and_recovery_scope() {
    let root = workspace();
    for model in ["3", "4", "5"] {
        for build in ["19045.3803", "22631.2861"] {
            for mode in ["0", "1"] {
                let settings = BTreeMap::from([
                    ("RPI_MODEL".into(), model.into()),
                    ("BID".into(), build.into()),
                    ("CAN_INSTALL_ON_SAME_DRIVE".into(), mode.into()),
                    ("OOBE_NETWORK_BYPASS".into(), "1".into()),
                    ("PI4_AUTO_DISABLE_3GB".into(), "1".into()),
                    ("WINDOWS_ACCOUNT_SETUP".into(), "1".into()),
                    (
                        "WINDOWS_ACCOUNT_USERNAME".into(),
                        "Fixture & <User> \"'".into(),
                    ),
                    (
                        "WINDOWS_ACCOUNT_PASSWORD".into(),
                        "fixture-only<&>\"'".into(),
                    ),
                    ("WINDOWS_LOCALE_SETUP".into(), "1".into()),
                    ("WINDOWS_LOCALE".into(), "sr-Latn-RS".into()),
                    ("UEFI_USE_LATEST".into(), "0".into()),
                    ("DRIVERS_USE_LATEST".into(), "0".into()),
                ]);
                let result = oracle(root.path(), "unattend_xml", &settings);
                let typed = DesktopOptions::from_compat(&PrepareOptions {
                    runtime_dir: runtime(),
                    download_dir: root.path().join("downloads"),
                    settings,
                })
                .unwrap();
                assert_eq!(
                    answer_file(&typed, &BuildId::parse(build).unwrap())
                        .unwrap()
                        .unwrap(),
                    String::from_utf8(result.stdout).unwrap(),
                    "model {model}, build {build}, mode {mode}"
                );
            }
        }
    }
}

#[test]
fn source_differential_cache_manifest_and_integrity_modes() {
    let root = workspace();
    let payload = root.path().join("payload");
    fs::create_dir(&payload).unwrap();
    fs::write(payload.join("first file"), b"first").unwrap();
    fs::write(payload.join("second"), b"second").unwrap();
    let settings = BTreeMap::from([("ORACLE_PAYLOAD".into(), payload.to_str().unwrap().into())]);
    let result = oracle(root.path(), "cache_manifest \"$ORACLE_PAYLOAD\"", &settings);
    assert_eq!(
        cache_manifest(&payload, &AtomicBool::new(false)).unwrap(),
        String::from_utf8(result.stdout).unwrap()
    );
    oracle(
        root.path(),
        "mark_cache \"$ORACLE_PAYLOAD\" source-version",
        &settings,
    );
    for mode in ["0", "1", "2"] {
        let mut settings = settings.clone();
        settings.insert("USE_CACHE".into(), mode.into());
        let result = oracle(
            root.path(),
            "if cache_is_current \"$ORACLE_PAYLOAD\" source-version; then printf yes; else printf no; fi",
            &settings,
        );
        let native_mode = match mode {
            "0" => CacheMode::Refresh,
            "1" => CacheMode::Verify,
            _ => CacheMode::Trust,
        };
        assert_eq!(
            String::from_utf8(result.stdout).unwrap(),
            if cache_is_current(
                &payload,
                "source-version",
                native_mode,
                &AtomicBool::new(false)
            )
            .unwrap()
            {
                "yes"
            } else {
                "no"
            }
        );
    }
}

#[test]
fn source_differential_release_filtering_and_compatible_asset_names() {
    let root = workspace();
    let page = serde_json::json!([
        {"tag_name":"v1.53.1","draft":false,"prerelease":false,"assets":[{"name":"RPi3_UEFI_Firmware_v1.53.1.zip","state":"uploaded"}]},
        {"tag_name":"v1.39","draft":false,"prerelease":false,"assets":[{"name":"RPi3_UEFI_Firmware_v1.39.zip","state":"uploaded"}]},
        {"tag_name":"v1.39","draft":false,"prerelease":false,"assets":[{"name":"RPi3_UEFI_Firmware_v1.39.zip","state":"uploaded"}]},
        {"tag_name":"v1.50","draft":false,"prerelease":false,"assets":[{"name":"RPi4_UEFI_Firmware_v1.50.zip","state":"uploaded"}]},
        {"tag_name":"v1.52","draft":false,"prerelease":true,"assets":[{"name":"RPi3_UEFI_Firmware_v1.52.zip","state":"uploaded"}]},
        {"tag_name":"v1.51","draft":true,"prerelease":false,"assets":[{"name":"RPi3_UEFI_Firmware_v1.51.zip","state":"uploaded"}]}
    ]);
    let bytes = serde_json::to_vec(&page).unwrap();
    let path = root.path().join("releases.json");
    fs::write(&path, &bytes).unwrap();
    let settings = BTreeMap::from([
        ("RPI_MODEL".into(), "3".into()),
        ("ORACLE_RELEASES".into(), path.to_str().unwrap().into()),
    ]);
    let result = oracle(
        root.path(),
        "wget() { cat \"$ORACLE_RELEASES\"; }\nlist_release_versions uefi",
        &settings,
    );
    let expected = parse_release_page(
        &bytes,
        ReleaseKind::Uefi,
        Model::Pi3,
        &SystemDefaults::load(&runtime()).unwrap(),
    )
    .unwrap()
    .into_iter()
    .map(|release| format!("{}\n", release.tag))
    .collect::<String>();
    assert_eq!(String::from_utf8(result.stdout).unwrap(), expected);
}

#[test]
fn source_differential_wim_resource_sum_and_solid_flags() {
    let root = workspace();
    let text = "Uncompressed size = 10000 bytes\nFlags = WIM_RESHDR_FLAG_SOLID\nUncompressed size = 524288 bytes\n";
    let path = root.path().join("blobs.txt");
    fs::write(&path, text).unwrap();
    let result = oracle(
        root.path(),
        "wiminfo() { cat \"$ORACLE_BLOBS\"; }\nwim_resource_summary unused.wim",
        &BTreeMap::from([("ORACLE_BLOBS".into(), path.to_str().unwrap().into())]),
    );
    let native = parse_resource_summary(text).unwrap();
    assert_eq!(
        String::from_utf8(result.stdout).unwrap(),
        format!(
            "{}\t{}\n",
            u8::from(native.solid),
            native.uncompressed_bytes
        )
    );
}

#[test]
fn source_differential_windows_build_families_dates_floor_and_cpu_filtering() {
    let root = workspace();
    let document = br#"<releases>
<version number="11">
<release build="26100.1"><date>20240901</date></release>
<release build="22631.2861"><date>20231101</date></release>
</version>
<version number="10">
<release build="19045.3803"><date>20231201</date></release>
<release build="17134.112"><date>20180601</date></release>
<release build="16299.1"><date>20171001</date></release>
</version>
</releases>"#;
    let path = root.path().join("versions.xml");
    fs::write(&path, document).unwrap();
    let defaults = SystemDefaults::load(&runtime()).unwrap();
    for model in [Model::Pi3, Model::Pi5] {
        for (number, family) in [
            ("10", WindowsVersion::Windows10),
            ("11", WindowsVersion::Windows11),
        ] {
            let result = oracle(
                root.path(),
                "versions=''\ncache_downloader() { cat \"$ORACLE_VERSIONS\"; }\nlist_bids_supported \"$ORACLE_VERSION\"",
                &BTreeMap::from([
                    ("RPI_MODEL".into(), model.number().to_string()),
                    ("ORACLE_VERSIONS".into(), path.to_str().unwrap().into()),
                    ("ORACLE_VERSION".into(), number.into()),
                ]),
            );
            let expected = parse_build_releases(document, family, model, &defaults)
                .unwrap()
                .into_iter()
                .map(|release| format!("{} ({})\n", release.build, release.date))
                .collect::<String>();
            assert_eq!(String::from_utf8(result.stdout).unwrap(), expected);
        }
    }
}

#[test]
fn source_differential_catalogue_selects_first_matching_file_before_summary() {
    let root = workspace();
    let document = format!(
        r#"<Catalog><File><LanguageCode>en-gb</LanguageCode><FilePath>https://example.invalid/gb.esd</FilePath><Size>1</Size><Sha1>{}</Sha1></File><File><LanguageCode>en-us</LanguageCode><FilePath>https://example.invalid/first.esd</FilePath><Size>2</Size><Sha1>{}</Sha1></File><File><LanguageCode>en-us</LanguageCode><FilePath>https://example.invalid/second.esd</FilePath><Size>3</Size><Sha1>{}</Sha1></File><Languages><LanguageCode>en-us</LanguageCode></Languages></Catalog>"#,
        "1".repeat(40),
        "2".repeat(40),
        "3".repeat(40)
    );
    let path = root.path().join("catalogue.xml");
    fs::write(&path, &document).unwrap();
    for language in ["en-us", "en-gb"] {
        let result = oracle(
            root.path(),
            "get_esd_catalog_entry \"$(sed 's/></>\\n</g' \"$ORACLE_CATALOGUE\")\" \"$ORACLE_LANGUAGE\" | grep -m 1 '^<FilePath>' | sed 's#^<FilePath>##;s#</FilePath>$##'",
            &BTreeMap::from([
                ("ORACLE_CATALOGUE".into(), path.to_str().unwrap().into()),
                ("ORACLE_LANGUAGE".into(), language.into()),
            ]),
        );
        assert_eq!(
            String::from_utf8(result.stdout).unwrap().trim(),
            get_esd_catalog_entry(document.as_bytes(), language)
                .unwrap()
                .url
        );
    }
}
