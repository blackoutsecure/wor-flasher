use super::*;
use std::cell::{Cell, RefCell};
use std::io::Cursor;
use std::os::unix::fs::{PermissionsExt, symlink};

fn temporary() -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix(".media-test-")
        .tempdir_in(env!("CARGO_MANIFEST_DIR"))
        .unwrap()
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn package(bytes: &[u8]) -> Package {
    Package {
        size: bytes.len() as u64,
        sha256: digest(bytes),
    }
}
fn put32(bytes: &mut [u8], at: usize, value: u32) {
    bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
}

fn cab_blob(name: &str, data: &[u8], compression: cab::CompressionType) -> Vec<u8> {
    let mut builder = cab::CabinetBuilder::new();
    builder.add_folder(compression).add_file(name);
    let mut writer = builder.build(Cursor::new(Vec::new())).unwrap();
    writer
        .next_file()
        .unwrap()
        .unwrap()
        .write_all(data)
        .unwrap();
    writer.finish().unwrap().into_inner()
}

struct Fixture {
    image: OfficialImage,
    ffu: Vec<u8>,
    msi: Vec<u8>,
    iso: Vec<u8>,
}

impl Fixture {
    fn new() -> Self {
        let ffu = b"synthetic fixture, never a production-approved Windows image".repeat(100);
        let cabinet = cab_blob("filReviewed", &ffu, cab::CompressionType::MsZip);
        let count = cabinet.len().div_ceil(4096);
        let mut msi = vec![0; (3 + count) * 4096];
        msi[..8].copy_from_slice(b"\xd0\xcf\x11\xe0\xa1\xb1\x1a\xe1");
        for (at, value) in [(26, 4u16), (28, 0xfffe), (30, 12), (32, 6)] {
            msi[at..at + 2].copy_from_slice(&value.to_le_bytes());
        }
        put32(&mut msi, 44, 1);
        put32(&mut msi, 68, 0xffff_fffe);
        put32(&mut msi, 72, 0);
        for at in (76..512).step_by(4) {
            put32(&mut msi, at, 0xffff_ffff);
        }
        put32(&mut msi, 76, 0);
        for at in (4096..8192).step_by(4) {
            put32(&mut msi, at, 0xffff_ffff);
        }
        put32(&mut msi, 4096, 0xffff_fffd);
        put32(&mut msi, 4100, 0xffff_fffe);
        for index in 0..count {
            put32(
                &mut msi,
                4096 + (index + 2) * 4,
                if index + 1 == count {
                    0xffff_fffe
                } else {
                    (index + 3) as u32
                },
            );
        }
        msi[12288..12288 + cabinet.len()].copy_from_slice(&cabinet);
        let mut iso = vec![0; 2048];
        iso.extend(&msi);
        iso.extend(b"end");
        let image = OfficialImage {
            iso: package(&iso),
            msi: package(&msi),
            cabinet: package(&cabinet),
            msi_offset: 2048,
            cabinet_sector: 2,
            member: "filReviewed".into(),
            url: "https://software-download.microsoft.com/fixture.iso".into(),
            ffu: package(&ffu),
        };
        Self {
            image,
            ffu,
            msi,
            iso,
        }
    }

    fn verifier(&self) -> FakeVerifier {
        FakeVerifier {
            expected: self.ffu.clone(),
            calls: RefCell::new(Vec::new()),
            corrupt_after: Cell::new(false),
        }
    }
}

struct FakeVerifier {
    expected: Vec<u8>,
    calls: RefCell<Vec<PathBuf>>,
    corrupt_after: Cell<bool>,
}
impl FfuVerifier for FakeVerifier {
    fn inspect(&self, path: &Path, _: &AtomicBool) -> Result<Inspection> {
        assert_eq!(std::fs::read(path).unwrap(), self.expected);
        self.calls.borrow_mut().push(path.to_owned());
        if self.corrupt_after.get() {
            std::fs::write(path, b"mutated during verifier").unwrap();
        }
        Ok(Inspection {
            format: "ffu-v1".into(),
            architecture: "arm32".into(),
            profile: "iot-core".into(),
            sha256: digest(&self.expected),
            file_bytes: self.expected.len() as u64,
            trust: "pinned-official-sha256".into(),
            details: BTreeMap::from([("hdmi".into(), serde_json::json!({"mode":"official"}))]),
        })
    }
}
struct NoNetwork;
impl DownloadProvider for NoNetwork {
    fn download(
        &self,
        _: &str,
        _: &Package,
        _: &mut File,
        _: &mut dyn FnMut(Progress),
        _: &AtomicBool,
    ) -> Result<()> {
        panic!("unexpected network call")
    }
}

fn write(path: &Path, data: &[u8]) {
    std::fs::write(path, data).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
}
fn options(root: &Path) -> MediaOptions {
    MediaOptions {
        cache_dir: root.join("cache with spaces"),
        companion_executable: PathBuf::from("/not-used/native"),
    }
}
fn no_cancel() -> AtomicBool {
    AtomicBool::new(false)
}

#[test]
fn manifest_commands_and_digest_are_fixed() {
    let image = OfficialImage::reviewed().unwrap();
    assert_eq!(image.ffu.size, 1008861184);
    assert_eq!(image.msi_offset, 634880);
    assert_eq!(image.cabinet_sector, 4);
    assert_eq!(image.member, "fil2dda957cfdb079d52c8f284117ff4a0b");
    assert_eq!(
        NativeFfuVerifier::arguments(Path::new("/private/file with spaces.ffu")),
        [
            "iot-ffu",
            "inspect",
            "/private/file with spaces.ffu",
            "--json"
        ]
        .map(OsString::from)
    );
    for flag in ["--url", "--sha256", "--trust-cache", "--skip-verification"] {
        assert_eq!(
            run(&["download", "--cache-dir", "unused", flag, "ignored"].map(OsString::from))
                .unwrap(),
            2
        );
    }
}

#[test]
fn local_iso_msi_ffu_and_atomic_cache_preserve_inspection() {
    for (suffix, mode) in [
        ("iso", Acquisition::LocalIso),
        ("msi", Acquisition::LocalMsi),
        ("FFU", Acquisition::LocalFfu),
    ] {
        let root = temporary();
        let fixture = Fixture::new();
        let path = root.path().join(format!("source with spaces.{suffix}"));
        write(
            &path,
            match suffix {
                "iso" => &fixture.iso,
                "msi" => &fixture.msi,
                _ => &fixture.ffu,
            },
        );
        let verifier = fixture.verifier();
        let service = MediaService {
            verifier: &verifier,
            download_provider: &NoNetwork,
        };
        let options = options(root.path());
        let mut events = Vec::new();
        let result = service
            .prepare_image(
                &path,
                &options,
                &fixture.image,
                &mut |event| events.push(event),
                &no_cancel(),
            )
            .unwrap();
        assert_eq!(result.acquisition, mode);
        assert_eq!(std::fs::read(&result.source_file).unwrap(), fixture.ffu);
        assert_eq!(result.inspection.details["hdmi"]["mode"], "official");
        assert!(events.iter().any(|event| event.percent == 0));
        assert!(events.iter().any(|event| event.percent == 100));
        if suffix == "FFU" {
            assert!(!options.cache_dir.exists());
            assert_eq!(verifier.calls.borrow().len(), 1);
        } else {
            let metadata = std::fs::metadata(&result.source_file).unwrap();
            use std::os::unix::fs::MetadataExt;
            assert_eq!(metadata.mode() & 0o777, 0o600);
            assert_eq!(metadata.nlink(), 1);
            assert_eq!(std::fs::read_dir(&options.cache_dir).unwrap().count(), 1);
            assert_eq!(verifier.calls.borrow().len(), 2);
            let cached = service
                .prepare_image(&path, &options, &fixture.image, &mut |_| {}, &no_cancel())
                .unwrap();
            assert_eq!(cached.acquisition, Acquisition::Cache);
        }
    }
}

#[test]
fn source_wrapper_must_verify_even_with_a_good_cache() {
    let root = temporary();
    let fixture = Fixture::new();
    let path = root.path().join("input.iso");
    write(&path, &fixture.iso);
    let verifier = fixture.verifier();
    let service = MediaService {
        verifier: &verifier,
        download_provider: &NoNetwork,
    };
    let options = options(root.path());
    let original = service
        .prepare_image(&path, &options, &fixture.image, &mut |_| {}, &no_cancel())
        .unwrap();
    let mut changed = fixture.iso.clone();
    changed[0] ^= 1;
    write(&path, &changed);
    let error = service
        .prepare_image(&path, &options, &fixture.image, &mut |_| {}, &no_cancel())
        .unwrap_err();
    assert!(error.to_string().contains("SHA256"));
    assert_eq!(std::fs::read(original.source_file).unwrap(), fixture.ffu);
}

#[test]
fn wrappers_cabinet_and_ffu_digests_all_precede_parsers_and_publication() {
    for which in ["iso", "msi", "cab", "ffu"] {
        let root = temporary();
        let mut fixture = Fixture::new();
        let path = root.path().join("input.iso");
        write(&path, &fixture.iso);
        match which {
            "iso" => fixture.image.iso.sha256 = "0".repeat(64),
            "msi" => fixture.image.msi.sha256 = "0".repeat(64),
            "cab" => fixture.image.cabinet.sha256 = "0".repeat(64),
            _ => fixture.image.ffu.sha256 = "0".repeat(64),
        }
        let verifier = fixture.verifier();
        let service = MediaService {
            verifier: &verifier,
            download_provider: &NoNetwork,
        };
        let options = options(root.path());
        assert!(
            service
                .prepare_image(&path, &options, &fixture.image, &mut |_| {}, &no_cancel())
                .is_err(),
            "{which}"
        );
        assert!(verifier.calls.borrow().is_empty());
        assert_eq!(std::fs::read_dir(&options.cache_dir).unwrap().count(), 0);
    }
}

#[test]
fn malformed_compound_storage_is_bounded_and_cycle_checked() {
    for (at, value) in [
        (44, 1000000),
        (68, 999999),
        (72, 5),
        (76, 999999),
        (4096, 0xffff_ffff),
        (4104, 999999),
        (4104, 2),
        (4104, 4),
    ] {
        let root = temporary();
        let mut fixture = Fixture::new();
        put32(&mut fixture.msi, at, value);
        fixture.image.msi = package(&fixture.msi);
        let source = root.path().join("malformed.msi");
        write(&source, &fixture.msi);
        let verifier = fixture.verifier();
        let service = MediaService {
            verifier: &verifier,
            download_provider: &NoNetwork,
        };
        let options = options(root.path());
        assert!(
            service
                .prepare_image(&source, &options, &fixture.image, &mut |_| {}, &no_cancel())
                .is_err(),
            "{at}:{value}"
        );
        assert!(verifier.calls.borrow().is_empty());
        assert_eq!(std::fs::read_dir(&options.cache_dir).unwrap().count(), 0);
    }
}

#[test]
fn cabinet_rust_extraction_never_materializes_traversal_or_duplicate_names() {
    for member in [
        "../image.ffu",
        "/absolute",
        "-option",
        "file\\path",
        "file*",
        "file?",
    ] {
        let root = temporary();
        let mut fixture = Fixture::new();
        let blob = cab_blob(member, &fixture.ffu, cab::CompressionType::None);
        fixture.image.cabinet = package(&blob);
        let source = root.path().join("source.cab");
        write(&source, &blob);
        let mut source = File::open(source).unwrap();
        let mut target = File::create(root.path().join("out.ffu")).unwrap();
        assert!(
            archive::extract(
                &mut source,
                &mut target,
                &fixture.image,
                &mut |_| {},
                &no_cancel()
            )
            .is_err(),
            "{member}"
        );
        assert_eq!(target.metadata().unwrap().len(), 0);
        assert!(!root.path().join("image.ffu").exists());
    }
}

#[test]
fn all_url_trust_failures_are_refused_without_url_echo() {
    for url in [
        "http://software-download.microsoft.com/image",
        "https://example.org/image?token=SECRET",
        "https://software-download.microsoft.com.evil.invalid/x",
        "https://software-download.microsoft.com@evil.invalid/x",
        "https://software-download.microsoft.com:8443/x",
        "https://software-download.microsoft.com/x#fragment",
        "https://user:SECRET@software-download.microsoft.com/x",
        "https://@software-download.microsoft.com/x",
        "https:\\\\software-download.microsoft.com/x",
        "https://software-download.microsoft.com/x\ninjection",
    ] {
        let error = network::check_url(url).unwrap_err();
        assert!(!error.to_string().contains("SECRET"));
    }
    for url in [
        "https://go.microsoft.com/fwlink/?LinkId=846058",
        "https://software-download.microsoft.com:443/image",
    ] {
        network::check_url(url).unwrap();
    }
}

#[test]
fn no_follow_private_owner_and_hardlink_cache_rules_hold() {
    let root = temporary();
    let fixture = Fixture::new();
    let verifier = fixture.verifier();
    let service = MediaService {
        verifier: &verifier,
        download_provider: &NoNetwork,
    };
    let options = options(root.path());
    let path = root.path().join("input.iso");
    write(&path, &fixture.iso);
    let linked = root.path().join("link.iso");
    symlink(&path, &linked).unwrap();
    assert!(
        service
            .prepare_image(&linked, &options, &fixture.image, &mut |_| {}, &no_cancel())
            .is_err()
    );
    assert!(!options.cache_dir.exists());
    let real = root.path().join("real");
    std::fs::create_dir(&real).unwrap();
    let alias = root.path().join("alias");
    symlink(&real, &alias).unwrap();
    assert!(fs::Directory::open(&alias.join("child"), true, true).is_err());
    assert_eq!(std::fs::read_dir(&real).unwrap().count(), 0);
    let good = service
        .prepare_image(&path, &options, &fixture.image, &mut |_| {}, &no_cancel())
        .unwrap();
    std::fs::set_permissions(&good.source_file, std::fs::Permissions::from_mode(0o644)).unwrap();
    let cache = fs::Directory::open(&options.cache_dir, false, true).unwrap();
    assert!(
        service
            .cached(&cache, &fixture.image, &mut |_| {}, &no_cancel())
            .is_err()
    );
    std::fs::set_permissions(&good.source_file, std::fs::Permissions::from_mode(0o600)).unwrap();
    std::fs::hard_link(&good.source_file, root.path().join("hardlink.ffu")).unwrap();
    assert!(
        service
            .cached(&cache, &fixture.image, &mut |_| {}, &no_cancel())
            .is_err()
    );
}

#[test]
fn corrupt_cache_is_never_replaced_or_downloaded() {
    let root = temporary();
    let fixture = Fixture::new();
    let options = options(root.path());
    let cache = fs::Directory::open(&options.cache_dir, true, true).unwrap();
    let path = options.cache_dir.join(fixture.image.cache_name());
    write(&path, b"invalid");
    let verifier = fixture.verifier();
    let service = MediaService {
        verifier: &verifier,
        download_provider: &NoNetwork,
    };
    assert!(
        service
            .acquire(
                &cache,
                &fixture.image,
                None,
                Acquisition::Download,
                &mut |_| {},
                &no_cancel()
            )
            .is_err()
    );
    assert_eq!(std::fs::read(path).unwrap(), b"invalid");
}

#[test]
fn source_and_snapshot_replacement_during_verification_are_refused_and_cleaned() {
    let root = temporary();
    let fixture = Fixture::new();
    let source = root.path().join("input.iso");
    write(&source, &fixture.iso);
    let verifier = fixture.verifier();
    verifier.corrupt_after.set(true);
    let service = MediaService {
        verifier: &verifier,
        download_provider: &NoNetwork,
    };
    let options = options(root.path());
    assert!(
        service
            .prepare_image(&source, &options, &fixture.image, &mut |_| {}, &no_cancel())
            .is_err()
    );
    assert_eq!(std::fs::read_dir(&options.cache_dir).unwrap().count(), 0);
    let verifier = fixture.verifier();
    let service = MediaService {
        verifier: &verifier,
        download_provider: &NoNetwork,
    };
    let moved = root.path().join("old.iso");
    let mut changed = false;
    let error = service
        .prepare_image(
            &source,
            &options,
            &fixture.image,
            &mut |event| {
                if event.label == "Copying and verifying official package"
                    && event.percent == 100
                    && !changed
                {
                    std::fs::rename(&source, &moved).unwrap();
                    write(&source, &fixture.iso);
                    changed = true;
                }
            },
            &no_cancel(),
        )
        .unwrap_err();
    assert!(error.to_string().contains("changed"));
    assert_eq!(std::fs::read_dir(&options.cache_dir).unwrap().count(), 0);
}

#[test]
fn cancellation_after_publication_rolls_back_only_owned_inode() {
    let root = temporary();
    let fixture = Fixture::new();
    let source = root.path().join("input.msi");
    write(&source, &fixture.msi);
    let verifier = fixture.verifier();
    let service = MediaService {
        verifier: &verifier,
        download_provider: &NoNetwork,
    };
    let options = options(root.path());
    let cancel = no_cancel();
    let mut count = 0;
    let error = service
        .prepare_image(
            &source,
            &options,
            &fixture.image,
            &mut |event| {
                if event.label == "Validating complete approved FFU" && event.percent == 100 {
                    count += 1;
                    if count == 2 {
                        cancel.store(true, Ordering::SeqCst);
                    }
                }
            },
            &cancel,
        )
        .unwrap_err();
    assert!(matches!(error, MediaError::Cancelled));
    assert_eq!(std::fs::read_dir(&options.cache_dir).unwrap().count(), 0);
}

#[test]
fn production_does_not_accept_synthetic_provenance_or_desktop_trust_bypasses() {
    let root = temporary();
    let fixture = Fixture::new();
    let source = root.path().join("synthetic.ffu");
    write(&source, &fixture.ffu);
    let verifier = fixture.verifier();
    let service = MediaService {
        verifier: &verifier,
        download_provider: &NoNetwork,
    };
    assert!(
        service
            .prepare(&source, &options(root.path()), &mut |_| {}, &no_cancel())
            .is_err()
    );
    assert!(verifier.calls.borrow().is_empty());
    assert!(fs::absolute(&root.path().join("../escape")).is_err());
}

#[test]
fn download_headers_require_exact_length_unencoded_http_200_and_final_host() {
    use reqwest::header::{
        CONTENT_ENCODING, CONTENT_LENGTH, HeaderMap, HeaderValue, TRANSFER_ENCODING,
    };
    let url = "https://software-download.microsoft.com/file.iso";
    let good = HeaderMap::from_iter([(CONTENT_LENGTH, HeaderValue::from_static("123"))]);
    network::check_response(200, url, &good, 123).unwrap();
    for length in [
        None,
        Some("122"),
        Some("124"),
        Some("NaN"),
        Some("1234567890123"),
    ] {
        let mut headers = HeaderMap::new();
        if let Some(value) = length {
            headers.insert(CONTENT_LENGTH, HeaderValue::from_str(value).unwrap());
        }
        assert!(network::check_response(200, url, &headers, 123).is_err());
    }
    let mut duplicate = good.clone();
    duplicate.append(CONTENT_LENGTH, HeaderValue::from_static("123"));
    assert!(network::check_response(200, url, &duplicate, 123).is_err());
    for status in [206, 301, 404, 500] {
        assert!(network::check_response(status, url, &good, 123).is_err());
    }
    for (name, value) in [(CONTENT_ENCODING, "gzip"), (TRANSFER_ENCODING, "chunked")] {
        let mut headers = good.clone();
        headers.insert(name, HeaderValue::from_static(value));
        assert!(network::check_response(200, url, &headers, 123).is_err());
    }
    let error = network::check_response(
        200,
        "https://evil.invalid/file?token=fixture-sensitive",
        &good,
        123,
    )
    .unwrap_err();
    assert!(!format!("{error:?}").contains("fixture-sensitive"));
}

#[test]
fn download_provider_cannot_bypass_the_native_digest_gate() {
    struct Download {
        bytes: Vec<u8>,
        calls: Cell<usize>,
    }
    impl DownloadProvider for Download {
        fn download(
            &self,
            _: &str,
            _: &Package,
            file: &mut File,
            _: &mut dyn FnMut(Progress),
            _: &AtomicBool,
        ) -> Result<()> {
            self.calls.set(self.calls.get() + 1);
            file.write_all(&self.bytes)?;
            file.seek(SeekFrom::Start(0))?;
            Ok(())
        }
    }
    for valid in [true, false] {
        let root = temporary();
        let fixture = Fixture::new();
        let mut bytes = fixture.iso.clone();
        if !valid {
            bytes[0] ^= 1;
        }
        let downloader = Download {
            bytes,
            calls: Cell::new(0),
        };
        let verifier = fixture.verifier();
        let service = MediaService {
            verifier: &verifier,
            download_provider: &downloader,
        };
        let options = options(root.path());
        let cache = fs::Directory::open(&options.cache_dir, true, true).unwrap();
        let result = service.acquire(
            &cache,
            &fixture.image,
            None,
            Acquisition::Download,
            &mut |_| {},
            &no_cancel(),
        );
        assert_eq!(downloader.calls.get(), 1);
        if valid {
            assert_eq!(result.unwrap().acquisition, Acquisition::Download);
            assert_eq!(
                service
                    .acquire(
                        &cache,
                        &fixture.image,
                        None,
                        Acquisition::Download,
                        &mut |_| {},
                        &no_cancel()
                    )
                    .unwrap()
                    .acquisition,
                Acquisition::Cache
            );
            assert_eq!(downloader.calls.get(), 1);
        } else {
            assert!(result.is_err());
            assert!(verifier.calls.borrow().is_empty());
            assert_eq!(std::fs::read_dir(&options.cache_dir).unwrap().count(), 0);
        }
    }
}

#[test]
fn wrapper_extent_size_member_and_provenance_shape_are_bounded() {
    let root = temporary();
    let fixture = Fixture::new();
    let source = root.path().join("input.iso");
    write(&source, &fixture.iso);
    let verifier = fixture.verifier();
    let service = MediaService {
        verifier: &verifier,
        download_provider: &NoNetwork,
    };
    for image in [
        OfficialImage {
            msi_offset: fixture.iso.len() as u64,
            ..fixture.image.clone()
        },
        OfficialImage {
            cabinet_sector: 999999,
            ..fixture.image.clone()
        },
        OfficialImage {
            cabinet: Package {
                size: fixture.msi.len() as u64 + 1,
                ..fixture.image.cabinet.clone()
            },
            ..fixture.image.clone()
        },
    ] {
        assert!(
            service
                .prepare_image(
                    &source,
                    &options(root.path()),
                    &image,
                    &mut |_| {},
                    &no_cancel()
                )
                .is_err()
        );
    }
    for (key, value) in [
        ("source_bytes", serde_json::json!(true)),
        ("source_msi_bytes", serde_json::json!(0)),
        ("ffu_bytes", serde_json::json!(MAX_FFU + 1)),
        ("source_sha256", serde_json::json!("A".repeat(64))),
        ("source_cab_member", serde_json::json!("../flash.ffu")),
        ("source_cab_member", serde_json::json!("*")),
        ("architecture", serde_json::json!("arm64")),
    ] {
        let mut manifest: Value = serde_json::from_slice(PROVENANCE).unwrap();
        manifest["images"][0][key] = value;
        assert!(
            OfficialImage::from_manifest(&serde_json::to_vec(&manifest).unwrap()).is_err(),
            "{key}"
        );
    }
    for bytes in [
        fixture.iso[..fixture.iso.len() - 1].to_vec(),
        [fixture.iso.clone(), vec![1]].concat(),
    ] {
        write(&source, &bytes);
        assert!(
            service
                .prepare_image(
                    &source,
                    &options(root.path()),
                    &fixture.image,
                    &mut |_| {},
                    &no_cancel()
                )
                .is_err()
        );
    }
}

#[test]
fn private_cache_leaf_and_nonsticky_writable_ancestors_are_refused() {
    let root = temporary();
    let cache = root.path().join("unsafe");
    std::fs::create_dir(&cache).unwrap();
    std::fs::set_permissions(&cache, std::fs::Permissions::from_mode(0o777)).unwrap();
    assert!(fs::Directory::open(&cache, false, true).is_err());
    assert!(fs::Directory::open(&cache.join("child"), true, true).is_err());
    assert_eq!(std::fs::read_dir(&cache).unwrap().count(), 0);
}

#[test]
fn deadline_overflow_and_pre_cancel_remove_partial_files() {
    let root = temporary();
    let fixture = Fixture::new();
    for data in [
        &fixture.ffu[..fixture.ffu.len() - 1],
        &[fixture.ffu.clone(), vec![0]].concat(),
        fixture.ffu.as_slice(),
    ] {
        let mut source = Cursor::new(data);
        let mut target = File::create(root.path().join("output")).unwrap();
        let timeout = if data == fixture.ffu {
            Duration::ZERO
        } else {
            Duration::from_secs(10)
        };
        assert!(
            copy_verified(
                &mut source,
                &mut target,
                &fixture.image.ffu,
                "fixture",
                &mut |_| {},
                &no_cancel(),
                timeout
            )
            .is_err()
        );
    }
    let source = root.path().join("source.iso");
    write(&source, &fixture.iso);
    let verifier = fixture.verifier();
    let service = MediaService {
        verifier: &verifier,
        download_provider: &NoNetwork,
    };
    let options = options(root.path());
    assert!(matches!(
        service.prepare_image(
            &source,
            &options,
            &fixture.image,
            &mut |_| {},
            &AtomicBool::new(true)
        ),
        Err(MediaError::Cancelled)
    ));
    assert!(verifier.calls.borrow().is_empty());
}

#[test]
fn native_companion_failures_timeout_and_output_limit_are_bounded() {
    let file = Path::new("/not-a-real-ffu");
    let verifier = NativeFfuVerifier::new("/usr/bin/false");
    assert!(verifier.inspect(file, &no_cancel()).is_err());
    let verifier = NativeFfuVerifier::new("/bin/echo");
    assert!(
        verifier
            .inspect(file, &no_cancel())
            .unwrap_err()
            .to_string()
            .contains("JSON")
    );
    let verifier = NativeFfuVerifier::new("/usr/bin/yes");
    assert!(
        verifier
            .inspect(file, &no_cancel())
            .unwrap_err()
            .to_string()
            .contains("limit")
    );
    let verifier = NativeFfuVerifier {
        executable: "/usr/bin/yes".into(),
        timeout: Duration::ZERO,
    };
    assert!(verifier.inspect(file, &no_cancel()).is_err());
    assert!(matches!(
        verifier.inspect(file, &AtomicBool::new(true)),
        Err(MediaError::Cancelled)
    ));
}

#[test]
fn invalid_cli_never_starts_acquisition() {
    for args in [
        vec!["download", "--cache-dir", "--json"],
        vec!["prepare", "--cache-dir", "unused"],
        vec!["download"],
        vec!["unknown", "--cache-dir", "unused"],
        vec![
            "download",
            "--cache-dir",
            "unused",
            "--url",
            "http://evil.invalid/",
        ],
    ] {
        assert_eq!(
            run(&args.into_iter().map(OsString::from).collect::<Vec<_>>()).unwrap(),
            2
        );
    }
}

#[test]
fn cached_verifier_io_failure_preserves_acquisition_status_not_validation_status() {
    struct FailedVerifier;
    impl FfuVerifier for FailedVerifier {
        fn inspect(&self, _: &Path, _: &AtomicBool) -> Result<Inspection> {
            Err(MediaError::acquisition(
                "native verifier fixture I/O failure",
            ))
        }
    }
    let root = temporary();
    let fixture = Fixture::new();
    let options = options(root.path());
    let cache = fs::Directory::open(&options.cache_dir, true, true).unwrap();
    write(
        &options.cache_dir.join(fixture.image.cache_name()),
        &fixture.ffu,
    );
    let service = MediaService {
        verifier: &FailedVerifier,
        download_provider: &NoNetwork,
    };
    assert_eq!(
        service
            .cached(&cache, &fixture.image, &mut |_| {}, &no_cancel())
            .unwrap_err()
            .exit_status(),
        4
    );
}
