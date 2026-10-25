use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::collections::VecDeque;
use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use wor_desktop_media::*;

fn workspace() -> tempfile::TempDir {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/test-work");
    fs::create_dir_all(&root).unwrap();
    tempfile::Builder::new()
        .prefix("preparation-")
        .tempdir_in(root)
        .unwrap()
}

fn write(root: &Path, name: &str, data: impl AsRef<[u8]>) {
    let path = root.join(name);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, data).unwrap();
}

fn zip(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut archive = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (name, contents) in files {
        archive
            .start_file(*name, zip::write::SimpleFileOptions::default())
            .unwrap();
        archive.write_all(contents).unwrap();
    }
    archive.finish().unwrap().into_inner()
}

struct Fixture {
    root: tempfile::TempDir,
    options: PrepareOptions,
    provider: FakeProvider,
}

impl Fixture {
    fn new(model: &str) -> Self {
        let root = workspace();
        let runtime = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let pe_zip = zip(&[
            ("winpe/2/settings.ini", b"[General]\nHideEmptyDrives=1\n"),
            ("winpe/2/WoR.exe", b"fixture PE app"),
            ("efi/boot/bootaa64.efi", b"fixture WoR loader"),
        ]);
        let shell_zip = zip(&[("ShellBinPkg/UefiShell/AArch64/Shell.efi", b"fixture shell")]);
        let mut metadata: serde_json::Value =
            serde_json::from_slice(&fs::read(runtime.join("src/config/metadata.json")).unwrap())
                .unwrap();
        metadata["systemDefaults"]["peInstallerUrl"] =
            serde_json::json!("https://fixtures.invalid/pe.zip");
        metadata["systemDefaults"]["peInstallerSha256"] =
            serde_json::json!(format!("{:X}", Sha256::digest(&pe_zip)));
        metadata["systemDefaults"]["uefiShellPackageUrl"] =
            serde_json::json!("https://fixtures.invalid/shell.zip");
        metadata["systemDefaults"]["uefiShellPackageSha256"] =
            serde_json::json!(format!("{:x}", Sha256::digest(&shell_zip)));
        write(
            root.path(),
            "runtime/src/config/metadata.json",
            serde_json::to_vec(&metadata).unwrap(),
        );
        for entry in fs::read_dir(runtime.join("config-templates")).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_file() {
                write(
                    root.path(),
                    &format!(
                        "runtime/config-templates/{}",
                        entry.file_name().to_str().unwrap()
                    ),
                    fs::read(entry.path()).unwrap(),
                );
            }
        }
        for name in ["Pi3BootRefresh.exe", "GO-LICENSE.txt", "manifest.json"] {
            write(
                root.path(),
                &format!("runtime/src/lib/pi3-boot-refresh/{name}"),
                fs::read(runtime.join("src/lib/pi3-boot-refresh").join(name)).unwrap(),
            );
        }
        let settings = BTreeMap::from([
            ("BID".into(), "22621.525".into()),
            ("WIN_LANG".into(), "en-us".into()),
            ("RPI_MODEL".into(), model.into()),
            ("CAN_INSTALL_ON_SAME_DRIVE".into(), "1".into()),
            ("PE_USE_LATEST".into(), "0".into()),
            ("DRIVERS_USE_LATEST".into(), "0".into()),
            ("UEFI_USE_LATEST".into(), "0".into()),
            ("WINDOWS_LOCALE_SETUP".into(), "0".into()),
            ("OOBE_NETWORK_BYPASS".into(), "0".into()),
            ("PI4_AUTO_DISABLE_3GB".into(), "0".into()),
            ("USE_CACHE".into(), "2".into()),
        ]);
        let download = root.path().join("downloads");
        let windows = download.join("winfiles_22621.525_en-us");
        for (name, data) in [
            ("bootpart/boot/bcd", &b"fixture bcd"[..]),
            ("bootpart/efi/boot/bootaa64.efi", &b"Windows loader"[..]),
            ("bootpart/efi/microsoft/boot/bcd", &b"Windows EFI bcd"[..]),
            ("bootpart/sources/boot.wim", &b"boot fixture"[..]),
            ("install.wim", &b"install fixture"[..]),
            ("alldone", &b"done"[..]),
        ] {
            write(&windows, name, data);
        }
        write(
            &download,
            "peinstaller/winpe/2/settings.ini",
            "HideEmptyDrives=1\n",
        );
        write(&download, "peinstaller/winpe/2/WoR.exe", "PE fixture");
        write(&download, "peinstaller/efi/boot/bootaa64.efi", "PE loader");
        for name in PI4_REQUIRED_DRIVERS {
            write(&download.join("driverpackage"), name, "driver fixture");
        }
        if model == "3" {
            write(&download, "driverpackage/rpi.inf", "fixture Pi3 INF");
        }
        write(
            &download,
            &format!("pi{model}-uefipackage/RPI_EFI.fd"),
            "firmware fixture",
        );
        write(
            &download,
            &format!("pi{model}-uefipackage/config.txt"),
            "firmware default\n",
        );
        write(
            &download,
            &format!("pi{model}-uefipackage/start.elf"),
            "firmware startup",
        );
        for name in [
            "bootcode.bin",
            "start4.elf",
            "fixup4.dat",
            "bcm2712-rpi-5-b.dtb",
        ] {
            write(
                &download,
                &format!("pi{model}-uefipackage/{name}"),
                "firmware fixture",
            );
        }
        let mut provider = FakeProvider::default();
        provider
            .downloads
            .insert("https://fixtures.invalid/pe.zip".into(), pe_zip);
        provider
            .downloads
            .insert("https://fixtures.invalid/shell.zip".into(), shell_zip);
        Self {
            options: PrepareOptions {
                runtime_dir: root.path().join("runtime"),
                download_dir: download,
                settings,
            },
            root,
            provider,
        }
    }

    fn set(&mut self, key: &str, value: &str) {
        self.options.settings.insert(key.into(), value.into());
    }

    fn prepare(&mut self) -> Result<PreparedMedia, MediaError> {
        prepare_with_provider(
            &self.options,
            &mut |_| {},
            &AtomicBool::new(false),
            &mut self.provider,
        )
    }
}

struct FakeProvider {
    calls: Vec<(WimOperation, Vec<OsString>)>,
    downloads: BTreeMap<String, Vec<u8>>,
    requests: Vec<(String, bool)>,
    free: u64,
    solid: bool,
    chunk: u64,
    compression: String,
    fail: Option<WimOperation>,
    changed_editions: bool,
    converted_solid: bool,
    now: u64,
    spaces: VecDeque<u64>,
    actual_build: u32,
    architecture: u32,
    export_bytes: Option<u64>,
    file_limit: Option<u64>,
    extra_locale: Option<String>,
}

impl Default for FakeProvider {
    fn default() -> Self {
        Self {
            calls: Vec::new(),
            downloads: BTreeMap::new(),
            requests: Vec::new(),
            free: 100 * 1024 * 1024 * 1024,
            solid: false,
            chunk: 32768,
            compression: "LZX".into(),
            fail: None,
            changed_editions: false,
            converted_solid: false,
            now: 1000,
            spaces: VecDeque::new(),
            actual_build: 22621,
            architecture: 12,
            export_bytes: None,
            file_limit: None,
            extra_locale: None,
        }
    }
}

impl MediaProvider for FakeProvider {
    fn fetch(
        &mut self,
        request: &DownloadRequest,
        destination: &Path,
        cancelled: &AtomicBool,
    ) -> Result<(), MediaError> {
        if cancelled.load(Ordering::Relaxed) {
            return Err(MediaError::Cancelled);
        }
        self.requests
            .push((request.url.as_str().into(), request.verify_tls));
        let bytes = self
            .downloads
            .get(request.url.as_str())
            .ok_or(MediaError::Download {
                component: request.component,
                reason: "fixture unavailable",
            })?;
        fs::write(destination, bytes).unwrap();
        Ok(())
    }

    fn wim(
        &mut self,
        operation: WimOperation,
        args: &[OsString],
        _working_dir: &Path,
        _cancelled: &AtomicBool,
    ) -> Result<Vec<u8>, MediaError> {
        self.calls.push((operation, args.into()));
        if self.fail == Some(operation) {
            return Err(MediaError::Tool {
                tool: "fixture wimlib",
                operation: operation.name(),
                status: Some(31),
            });
        }
        let converted = args
            .first()
            .is_some_and(|p| p.to_string_lossy().contains("pi3-normalized"));
        match operation {
            WimOperation::Info => {
                if args.iter().any(|v| v == "--blobs") {
                    let solid = if converted {
                        self.converted_solid
                    } else {
                        self.solid
                    };
                    return Ok(format!(
                        "Uncompressed size = 10000 bytes\nFlags = {}\n",
                        if solid {
                            "WIM_RESHDR_FLAG_SOLID"
                        } else {
                            "WIM_RESHDR_FLAG_COMPRESSED"
                        }
                    )
                    .into_bytes());
                }
                if args.iter().any(|v| v == "--xml") {
                    let architecture = if Path::new(&args[0])
                        .file_name()
                        .is_some_and(|name| name == "boot.wim")
                    {
                        12
                    } else {
                        self.architecture
                    };
                    let extra = self
                        .extra_locale
                        .as_ref()
                        .map(|locale| format!("<LANGUAGE>{locale}</LANGUAGE>"))
                        .unwrap_or_default();
                    return Ok(format!(r#"<WIM><IMAGE INDEX="1"><WINDOWS><ARCH>{}</ARCH><VERSION><BUILD>{}</BUILD></VERSION><LANGUAGES><LANGUAGE>en-US</LANGUAGE>{extra}<DEFAULT>en-US</DEFAULT></LANGUAGES></WINDOWS></IMAGE><IMAGE INDEX="2"><WINDOWS><ARCH>{}</ARCH><VERSION><BUILD>{}</BUILD></VERSION></WINDOWS></IMAGE></WIM>"#, architecture, self.actual_build, architecture, self.actual_build).into_bytes());
                }
                let (compression, chunk) = if converted {
                    ("LZX", 32768)
                } else {
                    (self.compression.as_str(), self.chunk)
                };
                let esd = Path::new(&args[0]).extension().is_some_and(|e| e == "esd");
                let count = if esd { 5 } else { 2 };
                let mut info = format!(
                    "Image Count: {count}\nCompression: {compression}\nChunk Size: {chunk} bytes\nPart Number: 1/1\nBoot Index: 0\nAvailable Images:\nIndex: 1\nName: {}\nArchitecture: ARM64\nIndex: 2\nName: Second\nArchitecture: ARM64\n",
                    if converted && self.changed_editions {
                        "Changed"
                    } else {
                        "First"
                    }
                );
                if esd {
                    for index in 3..=5 {
                        info.push_str(&format!(
                            "Index: {index}\nName: Edition {index}\nArchitecture: ARM64\n"
                        ));
                    }
                }
                Ok(info.into_bytes())
            }
            WimOperation::Export => {
                let destination = Path::new(&args[2]);
                fs::create_dir_all(destination.parent().unwrap()).unwrap();
                fs::write(destination, b"converted installation fixture").unwrap();
                if let Some(size) = self.export_bytes
                    && args.iter().any(|a| a == "--recompress")
                {
                    fs::OpenOptions::new()
                        .write(true)
                        .open(destination)
                        .unwrap()
                        .set_len(size)
                        .unwrap();
                }
                Ok(Vec::new())
            }
            WimOperation::Extract => {
                let destination = args
                    .iter()
                    .find_map(|a| a.to_str().and_then(|a| a.strip_prefix("--dest-dir=")))
                    .unwrap();
                write(Path::new(destination), "boot/bcd", "ESD bcd");
                write(
                    Path::new(destination),
                    "efi/boot/bootaa64.efi",
                    "ESD loader",
                );
                write(
                    Path::new(destination),
                    "efi/microsoft/boot/bcd",
                    "ESD EFI bcd",
                );
                Ok(Vec::new())
            }
            WimOperation::Delete | WimOperation::Update | WimOperation::Verify => Ok(Vec::new()),
        }
    }

    fn extract_iso(
        &mut self,
        _image: &Path,
        destination: &Path,
        _cancelled: &AtomicBool,
    ) -> Result<(), MediaError> {
        write(destination, "boot/bcd", "ISO bcd");
        write(destination, "efi/boot/bootaa64.efi", "ISO loader");
        write(destination, "efi/microsoft/boot/bcd", "ISO EFI bcd");
        write(destination, "sources/boot.wim", "ISO boot");
        write(destination, "sources/install.esd", "ISO install");
        Ok(())
    }

    fn available_space(&mut self, _path: &Path) -> Result<u64, MediaError> {
        Ok(self.spaces.pop_front().unwrap_or(self.free))
    }

    fn file_size_limit(
        &mut self,
        _path: &Path,
        _cancelled: &AtomicBool,
    ) -> Result<Option<u64>, MediaError> {
        Ok(self.file_limit)
    }

    fn unix_time(&self) -> u64 {
        self.now
    }
}

#[test]
fn complete_private_tree_is_ready_for_a_copy_only_writer() {
    let mut fixture = Fixture::new("4");
    fixture.set("HIDE_EMPTY_DRIVES", "0");
    let prepared = fixture.prepare().unwrap();
    for path in [
        "boot/bcd",
        "efi/boot/bootaa64.efi",
        "sources/boot.wim",
        "RPI_EFI.fd",
        "config.txt",
        "start.elf",
    ] {
        assert!(prepared.boot_dir.join(path).is_file(), "{path}");
    }
    assert_eq!(fs::read(&prepared.install_wim).unwrap(), b"install fixture");
    assert_eq!(prepared.build, "22621.525");
    assert_eq!(prepared.language, "en-us");
    assert!(prepared.install_extras.is_dir());
    assert!(
        fs::read_dir(&prepared.install_extras)
            .unwrap()
            .next()
            .is_none()
    );
    assert_eq!(
        fs::read_to_string(
            fixture
                .options
                .download_dir
                .join("peinstaller/winpe/2/settings.ini")
        )
        .unwrap(),
        "HideEmptyDrives=1\n"
    );
    let updates: Vec<_> = fixture
        .provider
        .calls
        .iter()
        .filter(|(kind, _)| *kind == WimOperation::Update)
        .collect();
    assert_eq!(updates.len(), 2);
    assert!(
        updates[0]
            .1
            .iter()
            .any(|a| a.to_string_lossy().contains("add "))
    );
    assert!(
        updates[1]
            .1
            .iter()
            .any(|a| a.to_string_lossy().contains("/drivers"))
    );
    let private = prepared.boot_dir.parent().unwrap().to_path_buf();
    assert!(private.starts_with(&fixture.options.download_dir));
    prepared.cleanup().unwrap();
    assert!(!private.exists());
}

#[test]
fn setup_answer_and_ram_script_are_staged_in_both_media_roots_and_pe_hook() {
    let mut fixture = Fixture::new("4");
    fixture.set("OOBE_NETWORK_BYPASS", "1");
    fixture.set("PI4_AUTO_DISABLE_3GB", "1");
    fixture.set("WINDOWS_ACCOUNT_SETUP", "1");
    fixture.set("WINDOWS_ACCOUNT_USERNAME", "A&B<>\"'");
    fixture.set("WINDOWS_ACCOUNT_PASSWORD", "private<&>\"'password");
    fixture.set("WINDOWS_LOCALE_SETUP", "1");
    fixture.set("WINDOWS_LOCALE", "sr-Latn-RS");
    let prepared = fixture.prepare().unwrap();
    let answer = fs::read_to_string(prepared.boot_dir.join("Autounattend.xml")).unwrap();
    assert!(answer.contains("A&amp;B&lt;&gt;&quot;&apos;"));
    assert!(answer.contains("private&lt;&amp;&gt;&quot;&apos;password"));
    assert!(answer.contains("<HideOnlineAccountScreens>true"));
    assert!(answer.contains("<WillReboot>Always"));
    assert!(answer.contains("<InputLocale>sr-Latn-RS</InputLocale>"));
    assert_eq!(
        answer,
        fs::read_to_string(prepared.install_extras.join("Autounattend.xml")).unwrap()
    );
    assert!(prepared.install_extras.join("Pi4Disable3GB.ps1").is_file());
    let hooks = prepared.staging_dir().join("pe/winpe/2/scripts");
    let batch = fs::read(hooks.join("prefinalize.cmd")).unwrap();
    assert!(!batch.windows(2).any(|w| w[1] == b'\n' && w[0] != b'\r'));
    assert_eq!(
        fs::read_to_string(hooks.join("unattend.xml")).unwrap(),
        answer
    );
    assert!(!format!("{:?}", prepared.report).contains("private"));
}

#[test]
fn pi3_normalization_checks_resources_editions_and_keeps_the_verified_base_unchanged() {
    let mut fixture = Fixture::new("3");
    fixture.provider.solid = true;
    fixture.provider.compression = "LZMS".into();
    let base = fixture
        .options
        .download_dir
        .join("winfiles_22621.525_en-us/install.wim");
    let original = fs::read(&base).unwrap();
    let prepared = fixture.prepare().unwrap();
    assert_eq!(fs::read(base).unwrap(), original);
    assert_ne!(fs::read(&prepared.install_wim).unwrap(), original);
    let export = fixture
        .provider
        .calls
        .iter()
        .find(|(op, args)| *op == WimOperation::Export && args.iter().any(|a| a == "--recompress"))
        .unwrap();
    for flag in [
        "all",
        "--compress=LZX",
        "--chunk-size=32768",
        "--recompress",
        "--check",
        "--threads=2",
    ] {
        assert!(export.1.iter().any(|a| a == flag), "{flag}");
    }
    let hooks = prepared.staging_dir().join("pe/winpe/2/scripts");
    assert!(hooks.join("Pi3BootRefresh.exe").is_file());
    assert!(hooks.join("GO-LICENSE.txt").is_file());
    assert!(hooks.join("prefinalize.cmd").is_file());
}

#[test]
fn pi3_reuses_compatible_lzx_without_recompressing() {
    let mut fixture = Fixture::new("3");
    let prepared = fixture.prepare().unwrap();
    assert!(
        !fixture
            .provider
            .calls
            .iter()
            .any(|(op, _)| *op == WimOperation::Export)
    );
    assert!(prepared.install_wim.is_file());
}

#[test]
fn every_conversion_failure_preserves_base_and_cleans_private_staging() {
    for fault in ["export", "verify", "editions", "solid"] {
        let mut fixture = Fixture::new("3");
        fixture.provider.solid = true;
        match fault {
            "export" => fixture.provider.fail = Some(WimOperation::Export),
            "verify" => fixture.provider.fail = Some(WimOperation::Verify),
            "editions" => fixture.provider.changed_editions = true,
            "solid" => fixture.provider.converted_solid = true,
            _ => unreachable!(),
        }
        let base = fixture
            .options
            .download_dir
            .join("winfiles_22621.525_en-us/install.wim");
        assert!(fixture.prepare().is_err(), "{fault}");
        assert_eq!(fs::read(base).unwrap(), b"install fixture");
        assert!(
            !fs::read_dir(&fixture.options.download_dir)
                .unwrap()
                .any(|e| e
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".wor-prepare-")),
            "{fault}"
        );
    }
}

#[test]
fn all_models_and_trust_mode_still_fail_on_missing_required_files() {
    for (model, missing) in [
        ("4", "driverpackage/bcmgenet/bcmgenet.cat"),
        ("4", "peinstaller/winpe/2/settings.ini"),
        ("4", "pi4-uefipackage/RPI_EFI.fd"),
        ("3", "winfiles_22621.525_en-us/bootpart/sources/boot.wim"),
        ("5", "peinstaller/efi/boot/bootaa64.efi"),
    ] {
        let mut fixture = Fixture::new(model);
        fs::remove_file(fixture.options.download_dir.join(missing)).unwrap();
        assert!(fixture.prepare().is_err(), "{model} {missing}");
    }
}

#[test]
fn safe_build_and_model_guards_run_before_downloads_or_native_tools() {
    for (key, value) in [
        ("BID", "26100.1"),
        ("RPI_MODEL", "2"),
        ("WIN_LANG", "../en-us"),
        ("BID", "../22621"),
        ("CAN_INSTALL_ON_SAME_DRIVE", "2"),
        ("WOR_IMAGE_ARCH", "x64"),
        ("WOR_IMAGE_FAMILY", "iot-core"),
        ("WINDOWS_LOCALE", "en-US</UILanguage>"),
        ("DRIVER_VER", "../v0.17"),
    ] {
        let mut fixture = Fixture::new("4");
        fixture.set("WINDOWS_LOCALE_SETUP", "1");
        fixture.set(key, value);
        assert!(fixture.prepare().is_err(), "{key}");
        assert!(fixture.provider.calls.is_empty(), "{key}");
        assert!(fixture.provider.requests.is_empty(), "{key}");
    }
}

#[test]
fn cancelled_preparation_never_runs_providers() {
    let mut fixture = Fixture::new("4");
    let result = prepare_with_provider(
        &fixture.options,
        &mut |_| {},
        &AtomicBool::new(true),
        &mut fixture.provider,
    );
    assert!(matches!(result, Err(MediaError::Cancelled)));
    assert!(fixture.provider.calls.is_empty());
    assert!(fixture.provider.requests.is_empty());
}

#[test]
fn pi4_shell_only_customization_does_not_need_an_answer_file() {
    let mut fixture = Fixture::new("4");
    fixture.set("PI4_UEFI_SHELL_UNLOCK", "1");
    let prepared = fixture.prepare().unwrap();
    let scripts = prepared.staging_dir().join("pe/winpe/2/scripts");
    assert!(scripts.join("Shell.efi").is_file());
    assert!(scripts.join("prefinalize.cmd").is_file());
    assert!(!scripts.join("unattend.xml").exists());
    assert!(!prepared.boot_dir.join("Autounattend.xml").exists());
}

#[test]
fn exact_pi3_partition_fit_is_accepted_but_one_more_byte_is_rejected() {
    for (mode, megabytes) in [("0", 6000u64), ("1", 18000u64)] {
        let unit = if cfg!(target_os = "macos") {
            1024 * 1024
        } else {
            1_000_000
        };
        let maximum = megabytes * unit - 64 * 1024 * 1024;
        for overflow in [false, true] {
            let mut fixture = Fixture::new("3");
            fixture.set("CAN_INSTALL_ON_SAME_DRIVE", mode);
            fixture.provider.solid = true;
            fixture.provider.export_bytes = Some(maximum + u64::from(overflow));
            let result = fixture.prepare();
            assert_eq!(
                result.is_err(),
                overflow,
                "mode {mode}, overflow {overflow}"
            );
        }
    }
}

#[test]
fn pi5_does_not_acquire_or_inject_pi3_pi4_drivers() {
    let mut fixture = Fixture::new("5");
    fs::remove_dir_all(fixture.options.download_dir.join("driverpackage")).unwrap();
    let prepared = fixture.prepare().unwrap();
    let updates: Vec<_> = fixture
        .provider
        .calls
        .iter()
        .filter(|(op, _)| *op == WimOperation::Update)
        .collect();
    assert_eq!(updates.len(), 2);
    assert!(
        updates[1]
            .1
            .iter()
            .any(|a| a.to_string_lossy().contains("/drivers/critical"))
    );
    assert!(prepared.install_extras.is_dir());
}

#[test]
fn pe_pinned_hash_is_enforced_and_errors_do_not_disclose_a_secret_url() {
    let mut fixture = Fixture::new("4");
    fixture.set("USE_CACHE", "1");
    fixture.set(
        "PE_INSTALLER_URL",
        "https://fixtures.invalid/pe.zip?token=never-log",
    );
    fixture.set("PE_INSTALLER_SHA256", &"0".repeat(64));
    fixture.provider.downloads.insert(
        "https://fixtures.invalid/pe.zip?token=never-log".into(),
        b"untrusted data".to_vec(),
    );
    let error = fixture.prepare().unwrap_err();
    assert!(matches!(error, MediaError::Integrity { .. }));
    assert!(!error.to_string().contains("never-log"));
    assert!(!format!("{error:?}").contains("never-log"));
}

#[test]
fn insufficient_measured_space_stops_before_native_conversion() {
    let mut fixture = Fixture::new("3");
    fixture.provider.solid = true;
    fixture.provider.free = 1;
    assert!(matches!(fixture.prepare(), Err(MediaError::Space { .. })));
    assert!(
        !fixture
            .provider
            .calls
            .iter()
            .any(|(op, _)| *op == WimOperation::Export)
    );
}

#[test]
fn customizations_do_not_leak_between_runs_or_into_download_manifests() {
    let mut fixture = Fixture::new("4");
    fixture.set("WINDOWS_ACCOUNT_SETUP", "1");
    fixture.set("WINDOWS_ACCOUNT_USERNAME", "fixture");
    fixture.set("WINDOWS_ACCOUNT_PASSWORD", "secret-first-run");
    let first = fixture.prepare().unwrap();
    assert!(first.boot_dir.join("Autounattend.xml").exists());
    fixture.set("WINDOWS_ACCOUNT_SETUP", "0");
    let second = fixture.prepare().unwrap();
    assert!(!second.boot_dir.join("Autounattend.xml").exists());
    assert!(!second.staging_dir().join("pe/winpe/2/scripts").exists());
    assert!(
        !fixture
            .options
            .download_dir
            .join("peinstaller/winpe/2/scripts")
            .exists()
    );
    assert!(
        !format!(
            "{:?}",
            DesktopOptions::from_compat(&fixture.options).unwrap()
        )
        .contains("secret-first-run")
    );
    assert!(fixture.root.path().exists());
}

impl Fixture {
    fn seal_caches(&mut self) {
        let typed = DesktopOptions::from_compat(&self.options).unwrap();
        let cancel = AtomicBool::new(false);
        mark_cache(
            &self.options.download_dir.join("peinstaller"),
            &typed.packages.pe_sha256,
            &cancel,
        )
        .unwrap();
        for (folder, token) in [
            (
                "driverpackage".into(),
                format!(
                    "https://github.com/worproject/RPi-Windows-Drivers/releases/download/v0.17/RPi{}_Windows_ARM64_Drivers_v0.17.zip",
                    typed.model.number()
                ),
            ),
            (
                format!("pi{}-uefipackage", typed.model.number()),
                format!(
                    "https://github.com/{}/releases/download/{}/RPi{}_UEFI_{}_{}.zip",
                    typed.defaults.uefi_repository(typed.model),
                    typed.packages.uefi_version,
                    typed.model.number(),
                    if typed.model == Model::Pi5 {
                        "Release"
                    } else {
                        "Firmware"
                    },
                    typed.packages.uefi_version
                ),
            ),
            (
                "winfiles_22621.525_en-us".into(),
                "windows:esd:22621.525:en-us".into(),
            ),
        ] {
            if typed.model == Model::Pi5 && folder == "driverpackage" {
                continue;
            }
            mark_cache(&self.options.download_dir.join(folder), &token, &cancel).unwrap();
        }
    }

    fn register_packages(&mut self) {
        let typed = DesktopOptions::from_compat(&self.options).unwrap();
        let driver_files: Vec<_> = PI4_REQUIRED_DRIVERS
            .iter()
            .map(|name| (*name, &b"driver fixture"[..]))
            .collect();
        self.provider.downloads.insert(
            format!("https://github.com/worproject/RPi-Windows-Drivers/releases/download/v0.17/RPi{}_Windows_ARM64_Drivers_v0.17.zip", typed.model.number()),
            zip(&driver_files),
        );
        let firmware = zip(&[
            ("RPI_EFI.fd", b"firmware fixture"),
            ("config.txt", b"firmware default"),
            ("bootcode.bin", b"firmware fixture"),
            ("start.elf", b"firmware fixture"),
            ("start4.elf", b"firmware fixture"),
            ("fixup4.dat", b"firmware fixture"),
            ("bcm2712-rpi-5-b.dtb", b"firmware fixture"),
        ]);
        self.provider.downloads.insert(
            format!(
                "https://github.com/{}/releases/download/{}/RPi{}_UEFI_{}_{}.zip",
                typed.defaults.uefi_repository(typed.model),
                typed.packages.uefi_version,
                typed.model.number(),
                if typed.model == Model::Pi5 {
                    "Release"
                } else {
                    "Firmware"
                },
                typed.packages.uefi_version
            ),
            firmware,
        );
    }
}

#[test]
fn verified_cache_is_reused_without_network_and_is_not_changed_by_personalization() {
    let mut fixture = Fixture::new("4");
    fixture.seal_caches();
    fixture.set("USE_CACHE", "1");
    let before = cache_manifest(
        &fixture.options.download_dir.join("peinstaller"),
        &AtomicBool::new(false),
    )
    .unwrap();
    fixture.set("WINDOWS_ACCOUNT_SETUP", "1");
    fixture.set("WINDOWS_ACCOUNT_USERNAME", "fixture");
    fixture.set("WINDOWS_ACCOUNT_PASSWORD", "private");
    fixture.set("HIDE_EMPTY_DRIVES", "0");
    let prepared = fixture.prepare().unwrap();
    assert!(fixture.provider.requests.is_empty());
    assert!(
        prepared
            .report
            .packages
            .iter()
            .all(|package| package.cache_reused)
    );
    assert_eq!(
        before,
        cache_manifest(
            &fixture.options.download_dir.join("peinstaller"),
            &AtomicBool::new(false)
        )
        .unwrap()
    );
    assert!(
        fs::read_to_string(prepared.staging_dir().join("pe/winpe/2/settings.ini"))
            .unwrap()
            .contains("HideEmptyDrives=0")
    );
}

#[test]
fn permitted_latest_lookup_failures_warn_and_use_only_the_configured_pins() {
    let mut fixture = Fixture::new("4");
    fixture.seal_caches();
    for key in ["PE_USE_LATEST", "DRIVERS_USE_LATEST", "UEFI_USE_LATEST"] {
        fixture.set(key, "1");
    }
    fixture.set("USE_CACHE", "1");
    let prepared = fixture.prepare().unwrap();
    for code in [
        "pe-pinned-fallback",
        "drivers-pinned-fallback",
        "uefi-pinned-fallback",
    ] {
        assert!(
            prepared
                .report
                .warnings
                .iter()
                .any(|warning| warning.code == code),
            "{code}"
        );
    }
    assert_eq!(fixture.provider.requests.len(), 3);
    assert!(
        prepared
            .report
            .packages
            .iter()
            .all(|p| p.cache_reused && p.origin == "pinned")
    );
}

#[test]
fn pi3_measured_conversion_space_uses_blob_bytes_plus_exact_512_mib_headroom() {
    for enough in [false, true] {
        let mut fixture = Fixture::new("3");
        fixture.provider.solid = true;
        fixture.provider.spaces = VecDeque::from([
            100 * 1024 * 1024 * 1024,
            100 * 1024 * 1024 * 1024,
            10000 + 512 * 1024 * 1024 - u64::from(!enough),
        ]);
        let result = fixture.prepare();
        if enough {
            assert!(result.is_ok(), "{result:?}");
        } else {
            assert!(
                matches!(result, Err(MediaError::Space { .. })),
                "{result:?}"
            );
            assert!(
                !fixture
                    .provider
                    .calls
                    .iter()
                    .any(|(op, _)| *op == WimOperation::Export)
            );
        }
    }
}

#[test]
fn pi3_normalized_image_must_fit_selected_partition_even_in_trust_mode() {
    let mut fixture = Fixture::new("3");
    fixture.set("CAN_INSTALL_ON_SAME_DRIVE", "0");
    fixture.provider.solid = true;
    fixture.provider.export_bytes = Some(9_000_000_000);
    assert!(fixture.prepare().is_err());
    assert_eq!(
        fs::read(
            fixture
                .options
                .download_dir
                .join("winfiles_22621.525_en-us/install.wim")
        )
        .unwrap(),
        b"install fixture"
    );
}

#[test]
fn actual_image_architecture_and_build_cannot_bypass_selected_model_guards() {
    for (architecture, build) in [(9, 22621), (12, 26100), (12, 19045)] {
        let mut fixture = Fixture::new("4");
        fixture.provider.architecture = architecture;
        fixture.provider.actual_build = build;
        assert!(fixture.prepare().is_err());
        assert!(
            !fixture
                .provider
                .calls
                .iter()
                .any(|(op, _)| *op == WimOperation::Update)
        );
    }
}

#[test]
fn pi3_qualified_go_helper_digest_is_required_even_when_trusting_cache() {
    for name in ["Pi3BootRefresh.exe", "GO-LICENSE.txt"] {
        let mut fixture = Fixture::new("3");
        let path = fixture
            .options
            .runtime_dir
            .join("src/lib/pi3-boot-refresh")
            .join(name);
        let mut data = fs::read(&path).unwrap();
        data.push(0xff);
        fs::write(&path, data).unwrap();
        assert!(
            matches!(fixture.prepare(), Err(MediaError::Integrity { .. })),
            "{name}"
        );
        assert!(fixture.provider.calls.is_empty());
        assert!(fixture.provider.requests.is_empty());
    }
}

#[test]
fn cleanup_reports_failure_instead_of_success_if_private_root_is_replaced() {
    let mut fixture = Fixture::new("4");
    let prepared = fixture.prepare().unwrap();
    let original = prepared.staging_dir().to_path_buf();
    let retained = original.with_extension("retained");
    fs::rename(&original, &retained).unwrap();
    fs::write(&original, "replacement fixture file").unwrap();
    let result = prepared.cleanup();
    assert!(
        matches!(result, Err(MediaError::Cleanup { .. })),
        "{result:?}"
    );
    if original.exists() {
        fs::remove_file(original).unwrap();
    }
    fs::remove_dir_all(retained).unwrap();
}

#[test]
fn full_download_pipeline_verifies_esd_then_extracts_boot_and_preserves_source() {
    let mut fixture = Fixture::new("4");
    fixture.set("USE_CACHE", "0");
    fixture.set("VERIFY_TLS", "0");
    fixture.register_packages();
    let source = b"fixture Microsoft ESD";
    let sha1 = format!("{:x}", sha1::Sha1::digest(source));
    let sha256 = format!("{:x}", Sha256::digest(source));
    let catalogue = format!(
        r#"<Catalog><File><LanguageCode>en-us</LanguageCode><FilePath>https://fixtures.invalid/windows.esd?token=private</FilePath><Size>{}</Size><Sha1>{sha1}</Sha1><Sha256>{sha256}</Sha256></File></Catalog>"#,
        source.len()
    );
    fixture.provider.downloads.insert("https://worproject.com/dldserv/esd/getcatalog.php?build=22621.525&arch=ARM64&edition=Professional".into(), catalogue.into_bytes());
    fixture.provider.downloads.insert(
        "https://fixtures.invalid/windows.esd?token=private".into(),
        source.to_vec(),
    );
    let prepared = fixture.prepare().unwrap();
    assert!(prepared.boot_dir.join("sources/boot.wim").is_file());
    assert_eq!(prepared.report.source, "esd");
    let raw = fixture
        .options
        .download_dir
        .join(format!(".wor-sources/{sha256}.esd"));
    assert_eq!(fs::read(&raw).unwrap(), source);
    let deletes: Vec<_> = fixture
        .provider
        .calls
        .iter()
        .filter(|(op, _)| *op == WimOperation::Delete)
        .collect();
    assert_eq!(deletes.len(), 3);
    assert!(deletes[0].1.iter().any(|a| a == "--soft"));
    assert!(deletes[1].1.iter().any(|a| a == "--soft"));
    assert!(!deletes[2].1.iter().any(|a| a == "--soft"));
    assert!(deletes.iter().all(|(_, args)| Path::new(&args[0]) != raw));
    assert!(!format!("{:?}", prepared.report).contains("token"));
    assert!(
        fixture
            .provider
            .requests
            .iter()
            .filter(|(url, _)| url.contains("worproject.com/dldserv/esd/getcatalog"))
            .all(|(_, tls)| *tls)
    );
    assert!(
        fixture
            .provider
            .requests
            .iter()
            .filter(|(url, _)| url.contains("fixtures.invalid/"))
            .all(|(_, tls)| !tls)
    );
    assert!(
        prepared
            .report
            .warnings
            .iter()
            .any(|warning| warning.code == "tls-disabled")
    );
    let second = fixture.prepare().unwrap();
    assert!(second.install_wim.is_file());
    assert_eq!(
        fixture
            .provider
            .requests
            .iter()
            .filter(|(url, _)| url == "https://fixtures.invalid/windows.esd?token=private")
            .count(),
        2
    );
}

#[test]
fn tampered_downloaded_esd_is_not_cached_or_extracted() {
    for hash_kind in ["Sha1", "Sha256"] {
        let mut fixture = Fixture::new("4");
        fixture.set("USE_CACHE", "0");
        fixture.register_packages();
        let catalogue = format!(
            r#"<Catalog><File><LanguageCode>en-us</LanguageCode><FilePath>https://fixtures.invalid/corrupt.esd</FilePath><Size>4</Size><{hash_kind}>{}</{hash_kind}></File></Catalog>"#,
            "0".repeat(if hash_kind == "Sha1" { 40 } else { 64 })
        );
        fixture.provider.downloads.insert("https://worproject.com/dldserv/esd/getcatalog.php?build=22621.525&arch=ARM64&edition=Professional".into(), catalogue.into_bytes());
        fixture.provider.downloads.insert(
            "https://fixtures.invalid/corrupt.esd".into(),
            b"bad!".to_vec(),
        );
        assert!(matches!(
            fixture.prepare(),
            Err(MediaError::Integrity { .. })
        ));
        assert!(
            !fixture
                .provider
                .calls
                .iter()
                .any(|(op, _)| *op == WimOperation::Extract)
        );
        assert!(
            fs::read_dir(fixture.options.download_dir.join(".wor-sources"))
                .unwrap()
                .next()
                .is_none()
        );
    }
}

#[test]
fn fresh_iso_import_copies_install_esd_and_makes_a_verified_reusable_base() {
    let mut fixture = Fixture::new("4");
    let source = fixture.root.path().join("22621.525_en-us.iso");
    fs::File::create(&source)
        .unwrap()
        .set_len(3 * 1024 * 1024 * 1024)
        .unwrap();
    fixture.set("SOURCE_FILE", source.to_str().unwrap());
    let prepared = fixture.prepare().unwrap();
    assert_eq!(prepared.report.source, "iso");
    assert_eq!(fs::read(&prepared.install_wim).unwrap(), b"ISO install");
    let base = fixture
        .options
        .download_dir
        .join("winfiles_from_iso_22621.525_en-us");
    assert!(
        cache_is_current(
            &base,
            "windows:iso:22621.525:en-us",
            CacheMode::Verify,
            &AtomicBool::new(false)
        )
        .unwrap()
    );
    let choices =
        cached_windows_media(&fixture.options.download_dir, &AtomicBool::new(false)).unwrap();
    assert!(choices.iter().any(|choice| choice.from_iso
        && choice.integrity_verified
        && choice.build.as_str() == "22621.525"));
    fixture.set("SOURCE_FILE", "");
    let next = fixture.prepare().unwrap();
    assert_eq!(next.report.source, "cached-iso");
}

fn native(tool: &str, args: &[&std::ffi::OsStr]) -> Vec<u8> {
    let result = Command::new(tool)
        .args(args)
        .env("LC_ALL", "C")
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{tool}: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    result.stdout
}

#[test]
fn real_small_esd_pipeline_preserves_editions_and_configures_the_actual_boot_wim() {
    let mut fixture = Fixture::new("3");
    let quoted_downloads = fixture.root.path().join("downloads with ' spaces");
    fs::rename(&fixture.options.download_dir, &quoted_downloads).unwrap();
    fixture.options.download_dir = quoted_downloads;
    let source = fixture.root.path().join("22621.525_en-us.esd");
    for (index, name) in [
        "Boot files",
        "Windows PE",
        "Windows Setup",
        "First",
        "Second",
    ]
    .iter()
    .enumerate()
    {
        let directory = fixture.root.path().join(format!("image-{index}"));
        write(&directory, "edition.txt", *name);
        write(&directory, "shared.bin", vec![23u8; 100_000]);
        if index == 0 {
            write(&directory, "boot/bcd", "fixture bcd");
            write(&directory, "efi/boot/bootaa64.efi", "fixture loader");
            write(&directory, "efi/microsoft/boot/bcd", "fixture EFI bcd");
        }
        let verb = if index == 0 { "capture" } else { "append" };
        native(
            "wimlib-imagex",
            &[
                verb.as_ref(),
                directory.as_os_str(),
                source.as_os_str(),
                (*name).as_ref(),
                "--solid".as_ref(),
                "--solid-compress=LZMS".as_ref(),
                "--solid-chunk-size=1048576".as_ref(),
                "--threads=1".as_ref(),
                "--check".as_ref(),
            ],
        );
        native(
            "wimlib-imagex",
            &[
                "info".as_ref(),
                source.as_os_str(),
                (index + 1).to_string().as_ref(),
                "--image-property=WINDOWS/ARCH=12".as_ref(),
                "--image-property=WINDOWS/VERSION/BUILD=22621".as_ref(),
                "--image-property=WINDOWS/LANGUAGES/LANGUAGE=en-US".as_ref(),
                "--image-property=WINDOWS/LANGUAGES/DEFAULT=en-US".as_ref(),
            ],
        );
    }
    let before = Sha256::digest(fs::read(&source).unwrap());
    fixture.set("SOURCE_FILE", source.to_str().unwrap());
    fixture.set("WINDOWS_LOCALE_SETUP", "1");
    fixture.set("WINDOWS_LOCALE", "en-GB");
    fixture.set("HIDE_EMPTY_DRIVES", "0");
    let mut provider = NativeProvider::new().unwrap();
    let prepared = prepare_with_provider(
        &fixture.options,
        &mut |_| {},
        &AtomicBool::new(false),
        &mut provider,
    )
    .unwrap();
    assert_eq!(Sha256::digest(fs::read(&source).unwrap()), before);
    let info = native(
        "wimlib-imagex",
        &["info".as_ref(), prepared.install_wim.as_os_str()],
    );
    let info = parse_wim_info(std::str::from_utf8(&info).unwrap()).unwrap();
    assert_eq!(info.image_count, 2);
    assert_eq!(info.compression, "LZX");
    assert_eq!(info.chunk_size, 32768);
    let blobs = native(
        "wimlib-imagex",
        &[
            "info".as_ref(),
            prepared.install_wim.as_os_str(),
            "--blobs".as_ref(),
        ],
    );
    assert!(
        !parse_resource_summary(std::str::from_utf8(&blobs).unwrap())
            .unwrap()
            .solid
    );
    for (index, name) in [("1", "First"), ("2", "Second")] {
        let output = fixture.root.path().join(format!("edition-{index}"));
        native(
            "wimlib-imagex",
            &[
                "extract".as_ref(),
                prepared.install_wim.as_os_str(),
                index.as_ref(),
                "edition.txt".as_ref(),
                format!("--dest-dir={}", output.display()).as_ref(),
            ],
        );
        assert_eq!(
            fs::read_to_string(output.join("edition.txt")).unwrap(),
            name
        );
    }
    let boot_output = fixture.root.path().join("boot-proof");
    native(
        "wimlib-imagex",
        &[
            "extract".as_ref(),
            prepared.boot_dir.join("sources/boot.wim").as_os_str(),
            "2".as_ref(),
            "settings.ini".as_ref(),
            "scripts".as_ref(),
            "drivers".as_ref(),
            format!("--dest-dir={}", boot_output.display()).as_ref(),
        ],
    );
    assert!(
        fs::read_to_string(boot_output.join("settings.ini"))
            .unwrap()
            .contains("HideEmptyDrives=0")
    );
    assert!(boot_output.join("scripts/Pi3BootRefresh.exe").is_file());
    assert!(boot_output.join("drivers/rpi.inf").is_file());
    assert!(
        fs::read_to_string(boot_output.join("scripts/unattend.xml"))
            .unwrap()
            .contains("<InputLocale>en-GB</InputLocale>")
    );
    assert_eq!(
        fs::read_to_string(
            fixture
                .options
                .download_dir
                .join("peinstaller/winpe/2/settings.ini")
        )
        .unwrap(),
        "HideEmptyDrives=1\n"
    );
}

fn release(tag: &str, prefix: &str) -> serde_json::Value {
    serde_json::json!({
        "tag_name":tag, "draft":false, "prerelease":false,
        "assets":[{"name":format!("{prefix}{tag}.zip"),"state":"uploaded"}],
    })
}

#[test]
fn release_queries_use_separate_latest_and_all_five_minute_caches() {
    let mut fixture = Fixture::new("3");
    let options = QueryOptions {
        runtime_dir: fixture.options.runtime_dir.clone(),
        cache_dir: fixture.root.path().join("queries"),
    };
    let all_url = "https://api.github.com/repos/pftf/RPi3/releases?per_page=100&page=1";
    let latest_url = "https://api.github.com/repos/pftf/RPi3/releases/latest";
    fixture.provider.downloads.insert(
        all_url.into(),
        serde_json::to_vec(&vec![release("v1.54", "RPi3_UEFI_Firmware_")]).unwrap(),
    );
    fixture.provider.downloads.insert(
        latest_url.into(),
        serde_json::to_vec(&release("v1.53.1", "RPi3_UEFI_Firmware_")).unwrap(),
    );
    let cancel = AtomicBool::new(false);
    let all = query_release_versions_with_provider(
        &options,
        ReleaseKind::Uefi,
        Model::Pi3,
        ReleaseSelection::All,
        &cancel,
        &mut fixture.provider,
    )
    .unwrap();
    let latest = query_release_versions_with_provider(
        &options,
        ReleaseKind::Uefi,
        Model::Pi3,
        ReleaseSelection::Latest,
        &cancel,
        &mut fixture.provider,
    )
    .unwrap();
    assert_eq!(all[0].tag, "v1.54");
    assert_eq!(latest[0].tag, "v1.53.1");
    fixture.provider.downloads.clear();
    assert!(
        query_release_versions_with_provider(
            &options,
            ReleaseKind::Uefi,
            Model::Pi3,
            ReleaseSelection::Latest,
            &cancel,
            &mut fixture.provider
        )
        .is_ok()
    );
    assert_eq!(fixture.provider.requests.len(), 2);
    fixture.provider.now = 1300;
    assert!(
        query_release_versions_with_provider(
            &options,
            ReleaseKind::Uefi,
            Model::Pi3,
            ReleaseSelection::Latest,
            &cancel,
            &mut fixture.provider
        )
        .is_err()
    );
    assert_eq!(fixture.provider.requests.len(), 3);
    assert!(fixture.provider.requests.iter().all(|(_, tls)| *tls));
}

#[test]
fn release_queries_deduplicate_pages_and_refuse_partial_malformed_or_truncated_lists() {
    for malformed in [false, true] {
        let mut fixture = Fixture::new("3");
        let options = QueryOptions {
            runtime_dir: fixture.options.runtime_dir.clone(),
            cache_dir: fixture.root.path().join("queries"),
        };
        let first: Vec<_> = (0..100)
            .map(|_| release("v1.53.1", "RPi3_UEFI_Firmware_"))
            .collect();
        fixture.provider.downloads.insert(
            "https://api.github.com/repos/pftf/RPi3/releases?per_page=100&page=1".into(),
            serde_json::to_vec(&first).unwrap(),
        );
        fixture.provider.downloads.insert(
            "https://api.github.com/repos/pftf/RPi3/releases?per_page=100&page=2".into(),
            if malformed {
                br#"{"message":"rate limit"}"#.to_vec()
            } else {
                serde_json::to_vec(&vec![
                    release("v1.53.1", "RPi3_UEFI_Firmware_"),
                    release("v1.39", "RPi3_UEFI_Firmware_"),
                ])
                .unwrap()
            },
        );
        let result = query_release_versions_with_provider(
            &options,
            ReleaseKind::Uefi,
            Model::Pi3,
            ReleaseSelection::All,
            &AtomicBool::new(false),
            &mut fixture.provider,
        );
        if malformed {
            assert!(result.is_err());
            assert!(
                !options
                    .cache_dir
                    .join("github-releases/pftf/RPi3/pi3/uefi.json")
                    .exists()
            );
        } else {
            assert_eq!(
                result
                    .unwrap()
                    .iter()
                    .map(|r| r.tag.as_str())
                    .collect::<Vec<_>>(),
                ["v1.53.1", "v1.39"]
            );
        }
    }
    let mut fixture = Fixture::new("3");
    let options = QueryOptions {
        runtime_dir: fixture.options.runtime_dir.clone(),
        cache_dir: fixture.root.path().join("queries"),
    };
    for page in 1..=10 {
        fixture.provider.downloads.insert(
            format!("https://api.github.com/repos/pftf/RPi3/releases?per_page=100&page={page}"),
            serde_json::to_vec(
                &(0..100)
                    .map(|_| release("v1.39", "RPi3_UEFI_Firmware_"))
                    .collect::<Vec<_>>(),
            )
            .unwrap(),
        );
    }
    assert!(
        query_release_versions_with_provider(
            &options,
            ReleaseKind::Uefi,
            Model::Pi3,
            ReleaseSelection::All,
            &AtomicBool::new(false),
            &mut fixture.provider
        )
        .is_err()
    );
    assert_eq!(fixture.provider.requests.len(), 10);
    assert!(
        query_release_versions_with_provider(
            &options,
            ReleaseKind::Drivers,
            Model::Pi5,
            ReleaseSelection::Latest,
            &AtomicBool::new(false),
            &mut fixture.provider
        )
        .is_err()
    );
    assert_eq!(fixture.provider.requests.len(), 10);
}

#[test]
fn catalogues_bind_the_complete_url_and_verify_cached_metadata_before_offline_fallback() {
    let mut fixture = Fixture::new("4");
    let options = QueryOptions {
        runtime_dir: fixture.options.runtime_dir.clone(),
        cache_dir: fixture.root.path().join("queries"),
    };
    let url = "https://worproject.com/dldserv/esd/getcatalog.php?build=22621.525&arch=ARM64&edition=Professional";
    let xml = format!(
        r#"<Catalog><File><LanguageCode>en-us</LanguageCode><FilePath>https://fixtures.invalid/windows.esd</FilePath><Size>1</Size><Sha256>{}</Sha256></File></Catalog>"#,
        "1".repeat(64)
    );
    fixture
        .provider
        .downloads
        .insert(url.into(), xml.into_bytes());
    let build = BuildId::parse("22621.525").unwrap();
    let cancel = AtomicBool::new(false);
    assert!(
        !query_catalogue_with_provider(&options, &build, "en-us", &cancel, &mut fixture.provider)
            .unwrap()
            .cached_metadata
    );
    fixture.provider.downloads.clear();
    assert!(
        query_catalogue_with_provider(&options, &build, "en-us", &cancel, &mut fixture.provider)
            .unwrap()
            .cached_metadata
    );
    assert!(
        query_catalogue_with_provider(
            &options,
            &BuildId::parse("19045.3803").unwrap(),
            "en-us",
            &cancel,
            &mut fixture.provider
        )
        .is_err()
    );
    let file = options
        .cache_dir
        .join(format!("{:x}.xml", Sha256::digest(url.as_bytes())));
    fs::write(file, b"<Catalog>tampered</Catalog>").unwrap();
    assert!(
        query_catalogue_with_provider(&options, &build, "en-us", &cancel, &mut fixture.provider)
            .is_err()
    );
}

#[test]
fn fixed_native_cli_prepares_private_output_and_binds_explicit_cleanup_to_its_receipt() {
    let mut fixture = Fixture::new("4");
    let settings = fixture.root.path().join("private-settings.json");
    fs::write(
        &settings,
        serde_json::to_vec(&fixture.options.settings).unwrap(),
    )
    .unwrap();
    let output = fixture.options.download_dir.join("explicit-private-output");
    let args: Vec<OsString> = vec![
        "prepare".into(),
        "--runtime-dir".into(),
        fixture.options.runtime_dir.as_os_str().into(),
        "--download-dir".into(),
        fixture.options.download_dir.as_os_str().into(),
        "--settings".into(),
        settings.as_os_str().into(),
        "--output-dir".into(),
        output.as_os_str().into(),
    ];
    let mut json = Vec::new();
    assert_eq!(
        run_with_io(
            &args,
            &mut json,
            &mut |_| {},
            &AtomicBool::new(false),
            &mut fixture.provider
        )
        .unwrap(),
        0
    );
    let staged: StagedMedia = serde_json::from_slice(&json).unwrap();
    assert!(staged.boot_dir.join("sources/boot.wim").is_file());
    assert!(staged.install_wim.is_file());
    assert!(staged.install_extras.is_dir());
    assert_eq!(staged.build, "22621.525");
    assert!(
        !json
            .windows(b"WINDOWS_ACCOUNT_PASSWORD".len())
            .any(|s| s == b"WINDOWS_ACCOUNT_PASSWORD")
    );
    let mut json = Vec::new();
    assert_eq!(
        run_with_io(
            &[
                "cleanup".into(),
                "--staging-dir".into(),
                output.as_os_str().into()
            ],
            &mut json,
            &mut |_| {},
            &AtomicBool::new(false),
            &mut fixture.provider
        )
        .unwrap(),
        0
    );
    assert!(!output.exists());
    assert!(fixture.options.download_dir.join("peinstaller").is_dir());
}

#[test]
fn native_cli_rejects_ambiguous_arguments_secret_values_and_existing_output() {
    let mut fixture = Fixture::new("4");
    let mut json = Vec::new();
    for args in [
        vec!["unknown".into()],
        vec!["languages".into(), "--secret-token=never-log".into()],
        vec!["builds".into(), "--runtime-dir".into()],
        vec!["releases".into(), "--latest".into(), "--latest".into()],
        vec![
            "cleanup".into(),
            "--staging-dir".into(),
            fixture.options.download_dir.as_os_str().into(),
        ],
    ] {
        let error = run_with_io(
            &args,
            &mut json,
            &mut |_| {},
            &AtomicBool::new(false),
            &mut fixture.provider,
        )
        .unwrap_err();
        assert!(!format!("{error:?}").contains("never-log"));
        assert!(json.is_empty());
    }
    let prepared = fixture.prepare().unwrap();
    let before = prepared.boot_dir.clone();
    fs::create_dir(fixture.root.path().join("already-exists")).unwrap();
    assert!(
        prepared
            .persist_to(&fixture.root.path().join("already-exists"))
            .is_err()
    );
    assert!(!before.exists());
}

#[cfg(target_os = "macos")]
#[test]
fn real_small_iso_uses_native_archive_inspection_without_mounting_or_devices() {
    let mut fixture = Fixture::new("4");
    let media = fixture.root.path().join("iso-root");
    write(&media, "boot/bcd", "fixture bcd");
    write(&media, "efi/boot/bootaa64.efi", "fixture loader");
    write(&media, "efi/microsoft/boot/bcd", "fixture EFI bcd");
    for (image_name, name) in [("boot.wim", "PE"), ("install.esd", "Windows")] {
        let image = media.join("sources").join(image_name);
        fs::create_dir_all(image.parent().unwrap()).unwrap();
        for index in 1..=2 {
            let input = fixture.root.path().join(format!("{image_name}-{index}"));
            write(&input, "edition.txt", format!("{name} {index}"));
            native(
                "wimlib-imagex",
                &[
                    if index == 1 {
                        "capture".as_ref()
                    } else {
                        "append".as_ref()
                    },
                    input.as_os_str(),
                    image.as_os_str(),
                    format!("{name} {index}").as_ref(),
                    "--compress=LZX".as_ref(),
                    "--threads=1".as_ref(),
                    "--check".as_ref(),
                ],
            );
            native(
                "wimlib-imagex",
                &[
                    "info".as_ref(),
                    image.as_os_str(),
                    index.to_string().as_ref(),
                    "--image-property=WINDOWS/ARCH=12".as_ref(),
                    "--image-property=WINDOWS/VERSION/BUILD=22621".as_ref(),
                ],
            );
        }
    }
    let iso = fixture.root.path().join("22621.525_en-us.iso");
    native(
        "hdiutil",
        &[
            "makehybrid".as_ref(),
            "-iso".as_ref(),
            "-joliet".as_ref(),
            "-o".as_ref(),
            iso.as_os_str(),
            media.as_os_str(),
        ],
    );
    fs::OpenOptions::new()
        .write(true)
        .open(&iso)
        .unwrap()
        .set_len(3 * 1024 * 1024 * 1024)
        .unwrap();
    fixture.set("SOURCE_FILE", iso.to_str().unwrap());
    let prepared = prepare(&fixture.options, &mut |_| {}, &AtomicBool::new(false)).unwrap();
    assert_eq!(prepared.report.source, "iso");
    assert!(prepared.boot_dir.join("RPI_EFI.fd").is_file());
    let install_info = native(
        "wimlib-imagex",
        &["info".as_ref(), prepared.install_wim.as_os_str()],
    );
    assert_eq!(
        parse_wim_info(std::str::from_utf8(&install_info).unwrap())
            .unwrap()
            .image_count,
        2
    );
    assert!(!fs::read_dir(prepared.staging_dir()).unwrap().any(|e| {
        e.unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".iso-mount-")
    }));
    fixture.set("RPI_MODEL", "5");
    for name in ["RPI_EFI.fd", "config.txt", "bcm2712-rpi-5-b.dtb"] {
        write(
            &fixture.options.download_dir,
            &format!("pi5-uefipackage/{name}"),
            "Pi5 firmware fixture",
        );
    }
    let pi5 = prepare(&fixture.options, &mut |_| {}, &AtomicBool::new(false)).unwrap();
    let proof = fixture.root.path().join("pi5-proof");
    native(
        "wimlib-imagex",
        &[
            "extract".as_ref(),
            pi5.boot_dir.join("sources/boot.wim").as_os_str(),
            "2".as_ref(),
            "drivers".as_ref(),
            format!("--dest-dir={}", proof.display()).as_ref(),
        ],
    );
    assert_eq!(
        fs::metadata(proof.join("drivers/critical")).unwrap().len(),
        0
    );
}

#[test]
fn standardized_catalog_api_is_device_independent_and_returns_both_windows_families() {
    let mut fixture = Fixture::new("4");
    fixture.options.settings.remove("CAN_INSTALL_ON_SAME_DRIVE");
    fixture.options.settings.remove("BID");
    fixture.provider.downloads.insert(
        "https://worproject.com/dldserv/esd/getversions.php".into(),
        br#"<releases><version number="11"><release build="26100.1"><date>20240901</date></release><release build="22631.2861"><date>20231101</date></release></version><version number="10"><release build="19045.3803"><date>20231201</date></release></version></releases>"#.to_vec(),
    );
    let cancel = AtomicBool::new(false);
    let builds = catalog_with_provider(
        &fixture.options,
        "builds",
        &mut |_| {},
        &cancel,
        &mut fixture.provider,
    )
    .unwrap();
    assert_eq!(builds.as_array().unwrap().len(), 2);
    assert_eq!(builds[0]["build"], "22631.2861");
    assert_eq!(builds[0]["version"], "Windows11");
    assert_eq!(builds[1]["build"], "19045.3803");
    assert_eq!(fixture.provider.requests.len(), 1);
    assert!(fixture.provider.calls.is_empty());
    let language_choices = catalog_with_provider(
        &fixture.options,
        "languages",
        &mut |_| {},
        &cancel,
        &mut fixture.provider,
    )
    .unwrap();
    assert_eq!(
        language_choices.as_array().unwrap().len(),
        languages().len()
    );
    assert_eq!(fixture.provider.requests.len(), 1);
    assert!(
        catalog_with_provider(
            &fixture.options,
            "unsupported",
            &mut |_| {},
            &cancel,
            &mut fixture.provider
        )
        .is_err()
    );
    assert!(matches!(
        catalog_with_provider(
            &fixture.options,
            "languages",
            &mut |_| {},
            &AtomicBool::new(true),
            &mut fixture.provider
        ),
        Err(MediaError::Cancelled)
    ));
}

#[test]
fn fat_download_filesystem_rejects_an_oversized_esd_before_payload_download() {
    let mut fixture = Fixture::new("4");
    fixture.seal_caches();
    fixture.set("USE_CACHE", "1");
    fs::remove_dir_all(
        fixture
            .options
            .download_dir
            .join("winfiles_22621.525_en-us"),
    )
    .unwrap();
    fixture.provider.file_limit = Some(4 * 1024 * 1024 * 1024 - 1);
    let catalogue = format!(
        r#"<Catalog><File><LanguageCode>en-us</LanguageCode><FilePath>https://fixtures.invalid/too-large.esd</FilePath><Size>4294967296</Size><Sha1>{}</Sha1></File></Catalog>"#,
        "0".repeat(40)
    );
    fixture.provider.downloads.insert("https://worproject.com/dldserv/esd/getcatalog.php?build=22621.525&arch=ARM64&edition=Professional".into(), catalogue.into_bytes());
    assert!(matches!(
        fixture.prepare(),
        Err(MediaError::InvalidOption { key: "DL_DIR", .. })
    ));
    assert_eq!(fixture.provider.requests.len(), 1);
    assert!(
        !fixture
            .provider
            .calls
            .iter()
            .any(|(operation, _)| *operation == WimOperation::Extract)
    );
}

#[test]
fn regional_locale_query_includes_cached_image_locales_before_published_fallbacks() {
    let mut fixture = Fixture::new("4");
    fixture.provider.extra_locale = Some("en-CA".into());
    let choices = query_locale_options_with_provider(
        &fixture.options,
        &mut |_| {},
        &AtomicBool::new(false),
        &mut fixture.provider,
    )
    .unwrap();
    assert_eq!(choices[0].locale, "en-US");
    assert_eq!(choices[1].locale, "en-CA");
    assert_eq!(choices[2].locale, "en-GB");
    assert_eq!(choices[1].label, "en-CA (en-CA)");
    assert_eq!(choices.len(), languages().len() + 1);
    assert!(fixture.provider.requests.is_empty());
    fixture.provider.fail = Some(WimOperation::Info);
    let fallback = query_locale_options_with_provider(
        &fixture.options,
        &mut |_| {},
        &AtomicBool::new(false),
        &mut fixture.provider,
    )
    .unwrap();
    assert_eq!(fallback.len(), languages().len());
    assert_eq!(fallback[0].locale, "en-US");
}

#[test]
fn resolved_system_thresholds_are_respected_without_duplicating_configuration_resolution() {
    let mut fixture = Fixture::new("4");
    fixture.set("WIN11_MIN_BUILD", "30000");
    fixture.set("WIN10_OLDEST_BUILD", "19045.3803");
    fixture.set("OOBE_NETWORK_BYPASS", "1");
    let typed = DesktopOptions::from_compat(&fixture.options).unwrap();
    assert_eq!(typed.defaults.win11_min_build, 30000);
    assert_eq!(typed.defaults.win10_oldest_build, "19045.3803");
    let prepared = fixture.prepare().unwrap();
    assert!(!prepared.boot_dir.join("Autounattend.xml").exists());
}
