use sha2::{Digest, Sha256};
use std::fs;
use std::io::Write;
use std::io::{Seek, SeekFrom};
use std::path::Path;
use std::sync::atomic::AtomicBool;
use wor_desktop_media::*;

fn workspace() -> tempfile::TempDir {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/test-work");
    fs::create_dir_all(&root).unwrap();
    tempfile::Builder::new()
        .prefix("contracts-")
        .tempdir_in(root)
        .unwrap()
}

fn defaults() -> SystemDefaults {
    SystemDefaults::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")).unwrap()
}

#[test]
fn language_and_locale_queries_preserve_all_published_choices() {
    assert_eq!(languages().len(), 38);
    assert_eq!(default_language("en_GB.UTF-8"), "en-gb");
    assert_eq!(default_language("sr_Latn_RS@latin"), "sr-latn-rs");
    assert_eq!(default_language("unavailable"), "en-us");
    assert_eq!(windows_locale("SR-latn-rs"), "sr-Latn-RS");
    assert_eq!(windows_locale("en-us"), "en-US");
    let _ = windows_locale("en-čaa");
}

#[test]
fn catalogue_uses_first_complete_file_for_exact_language_before_summary() {
    let xml = format!(
        r#"<Catalog><File><LanguageCode>en-gb</LanguageCode><FilePath>https://example.invalid/gb.esd</FilePath><Size>1</Size><Sha1>{}</Sha1></File><File><LanguageCode>en-us</LanguageCode><FilePath>https://example.invalid/one.esd?token=private&amp;x=1</FilePath><Size>2</Size><Sha1>{}</Sha1></File><File><LanguageCode>en-us</LanguageCode><FilePath>https://example.invalid/two.esd</FilePath><Size>3</Size><Sha1>{}</Sha1></File><Languages><File><LanguageCode>fr-fr</LanguageCode><FilePath>https://example.invalid/summary.esd</FilePath><Size>9</Size><Sha1>{}</Sha1></File></Languages></Catalog>"#,
        "a".repeat(40),
        "b".repeat(40),
        "c".repeat(40),
        "d".repeat(40),
    );
    let entry = get_esd_catalog_entry(xml.as_bytes(), "en-us").unwrap();
    assert_eq!(entry.size, 2);
    assert!(entry.url.ends_with("?token=private&x=1"));
    assert_eq!(entry.sha1, Some("b".repeat(40)));
    assert!(!format!("{entry:?}").contains("private"));
    assert!(get_esd_catalog_entry(xml.as_bytes(), "fr-fr").is_err());
    assert!(get_esd_catalog_entry(xml.as_bytes(), "en").is_err());
}

#[test]
fn malformed_catalogues_and_unsigned_or_non_https_sources_are_rejected() {
    for body in [
        "<Catalog><File/></Catalog>".into(),
        format!(r#"<Catalog><File><LanguageCode>en-us</LanguageCode><FilePath>http://example.invalid/a.esd</FilePath><Size>4</Size><Sha1>{}</Sha1></File></Catalog>"#, "a".repeat(40)),
        format!(r#"<Catalog><File><LanguageCode>en-us</LanguageCode><FilePath>https://secret:password@example.invalid/a.esd</FilePath><Size>4</Size><Sha1>{}</Sha1></File></Catalog>"#, "a".repeat(40)),
        r#"<Catalog><File><LanguageCode>en-us</LanguageCode><FilePath>https://example.invalid/a.esd</FilePath><Size>4</Size></File></Catalog>"#.into(),
        format!(r#"<Catalog><File><LanguageCode>en-us</LanguageCode><FilePath>https://example.invalid/a.esd</FilePath><Size>-1</Size><Sha1>{}</Sha1></File></Catalog>"#, "a".repeat(40)),
        r#"<!DOCTYPE Catalog [<!ENTITY hidden SYSTEM "file:///private">]><Catalog>&hidden;</Catalog>"#.into(),
        r#"<Catalog><File><LanguageCode>en-us</File></Catalog>"#.into(),
    ] {
        assert!(get_esd_catalog_entry(body.as_bytes(), "en-us").is_err(), "{body}");
    }
}

#[test]
fn build_catalogue_filters_armv80_and_stops_at_supported_windows10_floor() {
    let xml = br#"<releases><version number="11"><release build="26100.1"><date>20240901</date></release><release build="22631.2861"><date>20231101</date></release></version><version number="10"><release build="19045.3803"><date>20231201</date></release><release build="17134.112"><date>20180601</date></release><release build="16299.1"><date>20171001</date></release></version></releases>"#;
    let builds =
        parse_build_releases(xml, WindowsVersion::Windows11, Model::Pi4, &defaults()).unwrap();
    assert_eq!(builds.len(), 1);
    assert_eq!(builds[0].build.as_str(), "22631.2861");
    assert_eq!(builds[0].date, "2023-11-01");
    assert_eq!(
        parse_build_releases(xml, WindowsVersion::Windows11, Model::Pi5, &defaults())
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        parse_build_releases(xml, WindowsVersion::Windows10, Model::Pi3, &defaults())
            .unwrap()
            .len(),
        2
    );
}

#[test]
fn release_metadata_requires_compatible_uploaded_assets_and_published_safe_tags() {
    let page = serde_json::json!([
        {"tag_name":"v1.53.1","draft":false,"prerelease":false,"assets":[{"name":"RPi3_UEFI_Firmware_v1.53.1.zip","state":"uploaded"}]},
        {"tag_name":"v1.53.1","draft":false,"prerelease":false,"assets":[{"name":"RPi3_UEFI_Firmware_v1.53.1.zip","state":"uploaded"}]},
        {"tag_name":"v1.39","draft":true,"prerelease":false,"assets":[{"name":"RPi3_UEFI_Firmware_v1.39.zip","state":"uploaded"}]},
        {"tag_name":"../unsafe","draft":false,"prerelease":false,"assets":[]},
        {"tag_name":"v1.50","draft":false,"prerelease":false,"assets":[{"name":"RPi4_UEFI_Firmware_v1.50.zip","state":"uploaded"}]},
        {"tag_name":"v1.52","draft":false,"prerelease":true,"assets":[{"name":"RPi3_UEFI_Firmware_v1.52.zip","state":"uploaded"}]},
        {"tag_name":"v1.51","draft":false,"prerelease":false,"assets":[{"name":"RPi3_UEFI_Firmware_v1.51.zip","state":"new"}]}
    ]);
    let releases = parse_release_page(
        &serde_json::to_vec(&page).unwrap(),
        ReleaseKind::Uefi,
        Model::Pi3,
        &defaults(),
    )
    .unwrap();
    assert_eq!(releases.len(), 1);
    assert_eq!(releases[0].tag, "v1.53.1");
    assert_eq!(
        releases[0].download_url,
        "https://github.com/pftf/RPi3/releases/download/v1.53.1/RPi3_UEFI_Firmware_v1.53.1.zip"
    );
    for malformed in [
        b"{}".as_slice(),
        b"[null]",
        br#"[{"tag_name":"v1","assets":[]}]"#,
    ] {
        assert!(parse_release_page(malformed, ReleaseKind::Uefi, Model::Pi3, &defaults()).is_err());
    }
    assert!(parse_release_page(b"[]", ReleaseKind::Drivers, Model::Pi5, &defaults()).is_err());
}

#[test]
fn wim_resource_sums_unique_uncompressed_blobs_and_cannot_overflow() {
    let summary = parse_resource_summary("Uncompressed size = 10000 bytes\nFlags = WIM_RESHDR_FLAG_SOLID\nUncompressed size = 3 bytes\n").unwrap();
    assert!(summary.solid);
    assert_eq!(summary.uncompressed_bytes, 10003);
    for text in [
        "",
        "Uncompressed size = 0 bytes\n",
        "Uncompressed size = unavailable bytes\n",
        "Uncompressed size = 18446744073709551615 bytes\nUncompressed size = 1 bytes\n",
    ] {
        assert!(parse_resource_summary(text).is_err(), "{text}");
    }
}

#[test]
fn wim_info_rejects_split_missing_duplicate_or_mismatched_metadata() {
    let valid = "Image Count: 2\nCompression: LZX\nChunk Size: 32768 bytes\nPart Number: 1/1\nBoot Index: 0\nAvailable Images:\nIndex: 1\nName: First\nIndex: 2\nName: Second\n";
    let info = parse_wim_info(valid).unwrap();
    assert_eq!(info.image_count, 2);
    for malformed in [
        valid.replace("1/1", "1/2"),
        valid.replace("Image Count: 2", "Image Count: 0"),
        valid.replace("Index: 2", "Index: 1"),
        format!("Image Count: 2\n{valid}"),
        valid.replace("Chunk Size: 32768 bytes\n", ""),
        valid.replace("Boot Index: 0", "Boot Index: 3"),
    ] {
        assert!(parse_wim_info(&malformed).is_err(), "{malformed}");
    }
}

#[test]
fn utf16_wim_xml_is_parsed_structurally_not_as_nul_bearing_lines() {
    let xml = r#"<?xml version="1.0" encoding="UTF-16"?><WIM><IMAGE INDEX="1"><NAME/><WINDOWS><ARCH>12</ARCH><VERSION><BUILD>22621</BUILD></VERSION><LANGUAGES><LANGUAGE>en-US</LANGUAGE><LANGUAGE>sr-Latn-RS</LANGUAGE><DEFAULT>en-US</DEFAULT></LANGUAGES></WINDOWS></IMAGE></WIM>"#;
    let mut bytes = vec![0xff, 0xfe];
    for unit in xml.encode_utf16() {
        bytes.extend_from_slice(&unit.to_le_bytes());
    }
    let images = parse_image_metadata(&bytes).unwrap();
    assert_eq!(images[0].architecture, 12);
    assert_eq!(images[0].build, Some(22621));
    assert_eq!(images[0].languages, ["en-us", "sr-latn-rs"]);
    assert_eq!(locales_from_xml(&bytes).unwrap(), ["en-us", "sr-latn-rs"]);
    bytes.pop();
    assert!(parse_image_metadata(&bytes).is_err());
}

fn archive_fixture(directory: &Path, entries: &[(&str, &[u8])]) -> std::path::PathBuf {
    let path = directory.join("archive.zip");
    let mut zip = zip::ZipWriter::new(fs::File::create(&path).unwrap());
    for (name, bytes) in entries {
        zip.start_file(*name, zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(bytes).unwrap();
    }
    zip.finish().unwrap();
    path
}

#[test]
fn safe_zip_streaming_preserves_files_and_checks_crc() {
    let root = workspace();
    let path = archive_fixture(
        root.path(),
        &[
            ("winpe/2/settings.ini", b"HideEmptyDrives=1"),
            ("efi/boot/bootaa64.efi", b"EFI"),
        ],
    );
    let destination = root.path().join("output");
    extract_zip(
        &path,
        &destination,
        ArchiveLimits::default(),
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(
        fs::read(destination.join("winpe/2/settings.ini")).unwrap(),
        b"HideEmptyDrives=1"
    );
    let data = fs::read(path).unwrap();
    assert!(!data.is_empty());
}

#[test]
fn zip_paths_cannot_escape_or_create_case_collisions_or_special_files() {
    for entries in [
        vec![("../outside", &b"x"[..])],
        vec![("/absolute", &b"x"[..])],
        vec![("C:\\outside", &b"x"[..])],
        vec![("one/../../outside", &b"x"[..])],
        vec![("one", &b"x"[..]), ("ONE", &b"y"[..])],
        vec![("one", &b"x"[..]), ("one/two", &b"y"[..])],
        vec![("one/two", &b"x"[..]), ("one", &b"y"[..])],
        vec![("one/first", &b"x"[..]), ("ONE/second", &b"y"[..])],
        vec![("name:stream", &b"x"[..])],
        vec![("name.", &b"x"[..])],
        vec![("CON.txt", &b"x"[..])],
    ] {
        let root = workspace();
        let path = archive_fixture(root.path(), &entries);
        let output = root.path().join("output");
        assert!(
            extract_zip(
                &path,
                &output,
                ArchiveLimits::default(),
                &AtomicBool::new(false)
            )
            .is_err(),
            "{entries:?}"
        );
        assert!(!output.exists(), "validation must precede extraction");
    }
}

#[test]
fn zip_symlinks_expansion_limits_and_cancellation_are_fail_closed() {
    let root = workspace();
    let path = root.path().join("link.zip");
    let mut zip = zip::ZipWriter::new(fs::File::create(&path).unwrap());
    zip.add_symlink(
        "unsafe-link",
        "../outside",
        zip::write::SimpleFileOptions::default(),
    )
    .unwrap();
    zip.finish().unwrap();
    assert!(
        extract_zip(
            &path,
            &root.path().join("link-output"),
            ArchiveLimits::default(),
            &AtomicBool::new(false)
        )
        .is_err()
    );
    let path = archive_fixture(root.path(), &[("payload", b"four")]);
    assert!(
        extract_zip(
            &path,
            &root.path().join("large-output"),
            ArchiveLimits {
                entries: 1,
                total_bytes: 3,
                file_bytes: 4
            },
            &AtomicBool::new(false)
        )
        .is_err()
    );
    assert!(matches!(
        extract_zip(
            &path,
            &root.path().join("cancelled"),
            ArchiveLimits::default(),
            &AtomicBool::new(true)
        ),
        Err(MediaError::Cancelled)
    ));
}

#[test]
fn cache_manifest_is_stable_source_compatible_and_tampering_is_not_reused() {
    let root = workspace();
    fs::create_dir(root.path().join("payload")).unwrap();
    fs::write(root.path().join("payload/b"), b"second").unwrap();
    fs::write(root.path().join("payload/a"), b"first").unwrap();
    let cancel = AtomicBool::new(false);
    let expected = format!(
        "{:x}  ./a\n{:x}  ./b\n",
        Sha256::digest(b"first"),
        Sha256::digest(b"second")
    );
    assert_eq!(
        cache_manifest(&root.path().join("payload"), &cancel).unwrap(),
        expected
    );
    mark_cache(&root.path().join("payload"), "source-token", &cancel).unwrap();
    assert!(
        cache_is_current(
            &root.path().join("payload"),
            "source-token",
            CacheMode::Verify,
            &cancel
        )
        .unwrap()
    );
    assert!(
        !cache_is_current(
            &root.path().join("payload"),
            "different-token",
            CacheMode::Verify,
            &cancel
        )
        .unwrap()
    );
    fs::write(root.path().join("payload/a"), b"tamper").unwrap();
    assert!(
        !cache_is_current(
            &root.path().join("payload"),
            "source-token",
            CacheMode::Verify,
            &cancel
        )
        .unwrap()
    );
    assert!(
        cache_is_current(
            &root.path().join("payload"),
            "source-token",
            CacheMode::Trust,
            &cancel
        )
        .unwrap()
    );
    assert!(
        !cache_is_current(
            &root.path().join("payload"),
            "source-token",
            CacheMode::Refresh,
            &cancel
        )
        .unwrap()
    );
}

#[cfg(unix)]
#[test]
fn symlinked_cache_payload_is_never_hashed_or_copied_as_a_regular_file() {
    let root = workspace();
    fs::create_dir(root.path().join("payload")).unwrap();
    fs::write(root.path().join("outside"), "outside").unwrap();
    std::os::unix::fs::symlink(
        root.path().join("outside"),
        root.path().join("payload/link"),
    )
    .unwrap();
    assert!(cache_manifest(&root.path().join("payload"), &AtomicBool::new(false)).is_err());
}

#[test]
fn corrupt_zip_crc_is_an_integrity_error_not_a_successful_extraction() {
    let root = workspace();
    let path = archive_fixture(root.path(), &[("payload", b"fixture payload")]);
    let mut archive = zip::ZipArchive::new(fs::File::open(&path).unwrap()).unwrap();
    let offset = archive.by_index(0).unwrap().data_start();
    drop(archive);
    let mut file = fs::OpenOptions::new().write(true).open(&path).unwrap();
    file.seek(SeekFrom::Start(offset)).unwrap();
    file.write_all(&[0xff]).unwrap();
    drop(file);
    assert!(
        extract_zip(
            &path,
            &root.path().join("output"),
            ArchiveLimits::default(),
            &AtomicBool::new(false)
        )
        .is_err()
    );
}

#[test]
fn uncompressed_wim_information_without_a_chunk_size_is_valid_source_media() {
    let info = parse_wim_info("Image Count: 1\nCompression: None\nPart Number: 1/1\nBoot Index: 0\nAvailable Images:\nIndex: 1\nName: Uncompressed\n").unwrap();
    assert_eq!(info.compression, "None");
    assert_eq!(info.chunk_size, 0);
}
