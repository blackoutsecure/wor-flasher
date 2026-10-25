use crate::archive::{ArchiveLimits, extract_zip};
use crate::cache::CacheLock;
use crate::catalog::{fetch_metadata, pinned_release};
use crate::files::{
    copy_file, copy_tree, copy_tree_merge, finish_temp, make_directory, require_directory,
    require_space, required_file, sha256_file, tree_bytes, tree_files, write_private,
};
use crate::provider::{find_case_insensitive, safe_url};
use crate::setup::{boot_config, configure_pe, stage_answers, validate_pi3_helper};
use crate::wim::{add_tree_command, inspect, normalize_pi3, validate_install_metadata, verify};
use crate::{
    BuildId, CacheMode, CatalogueEntry, DesktopOptions, DownloadRequest, MediaError, MediaProvider,
    Model, NativeProvider, PackageReport, PreparationReport, PreparationWarning, PrepareOptions,
    PreparedMedia, QueryOptions, ReleaseKind, ReleaseSelection, WimOperation, WindowsSource,
    answer_file, cache_is_current, check_cancelled, mark_cache, query_builds_with_provider,
    query_catalogue_with_provider, query_release_versions_with_provider,
};
use sha1::Sha1;
use sha2::{Digest, Sha256};
use std::ffi::OsString;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use wor_domain::progress::Progress;

pub const PI4_REQUIRED_DRIVERS: &[&str] = &[
    "bcmgenet/bcmgenet.inf",
    "bcmgenet/bcmgenet.cat",
    "mcci_dwchsotg/mcci_dwchsotg_hcd.inf",
    "mcci_dwchsotg/mcci_dwchsotg_hcd.cat",
    "mcci_dwchsotg/mcci_dwchsotg_hub.inf",
    "mcci_dwchsotg/mcci_dwchsotg_hub.cat",
    "mcci_dwchsotg/arm64/mcci_dwchsotg_hcd.sys",
    "mcci_dwchsotg/arm64/mcci_dwchsotg_hub.sys",
    "rpiuxflt/rpiuxflt.inf",
    "rpiuxflt/rpiuxflt.cat",
    "rpiuxflt/rpiuxflt.sys",
];

pub fn prepare(
    options: &PrepareOptions,
    progress: &mut dyn FnMut(Progress),
    cancelled: &AtomicBool,
) -> Result<PreparedMedia, MediaError> {
    check_cancelled(cancelled)?;
    prepare_typed(&DesktopOptions::from_compat(options)?, progress, cancelled)
}

pub fn prepare_typed(
    options: &DesktopOptions,
    progress: &mut dyn FnMut(Progress),
    cancelled: &AtomicBool,
) -> Result<PreparedMedia, MediaError> {
    prepare_inner(options, progress, cancelled, &mut NativeProvider::new()?)
}

pub fn prepare_with_provider(
    options: &PrepareOptions,
    progress: &mut dyn FnMut(Progress),
    cancelled: &AtomicBool,
    provider: &mut dyn MediaProvider,
) -> Result<PreparedMedia, MediaError> {
    check_cancelled(cancelled)?;
    prepare_inner(
        &DesktopOptions::from_compat(options)?,
        progress,
        cancelled,
        provider,
    )
}

fn prepare_inner(
    options: &DesktopOptions,
    progress: &mut dyn FnMut(Progress),
    cancelled: &AtomicBool,
    provider: &mut dyn MediaProvider,
) -> Result<PreparedMedia, MediaError> {
    check_cancelled(cancelled)?;
    options.validate()?;
    let (config, config_warnings) = boot_config(options)?;
    validate_pi3_helper(options, cancelled)?;
    make_directory(&options.download_dir)?;
    make_directory(&options.cache_dir)?;
    let _lock = CacheLock::acquire(&options.download_dir)?;
    let staging = crate::files::private_tempdir(&options.download_dir, ".wor-prepare-")?;
    let mut report = PreparationReport {
        model: options.model,
        build: options
            .build
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_default(),
        language: options.language.clone(),
        source: String::new(),
        cache_mode: options.cache_mode,
        packages: Vec::new(),
        warnings: Vec::new(),
    };
    if options.cache_mode == CacheMode::Trust {
        warn(
            &mut report,
            progress,
            "unchecked-cache",
            "USE_CACHE=2: source-version and cache-manifest checks are explicitly disabled; required files, WIM integrity, architecture and Pi 3 finalizer checks still apply.",
        );
    }
    if !options.verify_tls {
        warn(
            &mut report,
            progress,
            "tls-disabled",
            "VERIFY_TLS=0: payload certificate verification is explicitly disabled. Metadata requests still require verified HTTPS; payload hashes are not disabled.",
        );
    }
    for message in config_warnings {
        warn(&mut report, progress, "boot-configuration", &message);
    }
    let result = stage(
        options,
        staging.path(),
        config.as_deref(),
        &mut report,
        progress,
        cancelled,
        provider,
    );
    match result {
        Ok((boot_dir, install_wim, install_extras, build)) => {
            report.build = build.to_string();
            Ok(PreparedMedia {
                boot_dir,
                install_wim,
                install_extras,
                build: build.to_string(),
                language: options.language.clone(),
                report,
                staging: Some(staging),
            })
        }
        Err(error) => finish_temp(staging, Err(error)),
    }
}

fn warn(
    report: &mut PreparationReport,
    progress: &mut dyn FnMut(Progress),
    code: &str,
    message: &str,
) {
    report.warnings.push(PreparationWarning {
        code: code.into(),
        message: message.into(),
    });
    progress(Progress::Status(format!("Warning: {message}")));
}

#[allow(clippy::too_many_arguments)]
fn stage(
    options: &DesktopOptions,
    stage: &Path,
    config: Option<&str>,
    report: &mut PreparationReport,
    progress: &mut dyn FnMut(Progress),
    cancelled: &AtomicBool,
    provider: &mut dyn MediaProvider,
) -> Result<(PathBuf, PathBuf, PathBuf, BuildId), MediaError> {
    let mut required = 2 * 1024 * 1024 * 1024u64;
    for (directory, extra) in [
        ("peinstaller".to_owned(), 256 * 1024 * 1024),
        ("driverpackage".to_owned(), 256 * 1024 * 1024),
        (
            format!("pi{}-uefipackage", options.model.number()),
            192 * 1024 * 1024,
        ),
    ] {
        if options.cache_mode != CacheMode::Trust || !options.download_dir.join(directory).is_dir()
        {
            required += extra;
        }
    }
    require_space(required, provider.available_space(&options.download_dir)?)?;
    let query = QueryOptions {
        runtime_dir: options.runtime_dir.clone(),
        cache_dir: options.cache_dir.clone(),
    };
    let requested = if options.build.is_none() && matches!(options.source, WindowsSource::Download)
    {
        Some(
            query_builds_with_provider(
                &query,
                options.windows_version,
                options.model,
                cancelled,
                provider,
            )?
            .first()
            .ok_or(MediaError::InvalidMetadata("no compatible Windows build"))?
            .build
            .clone(),
        )
    } else {
        options.build.clone()
    };
    progress(Progress::Step {
        current: 1,
        total: 6,
        message: "Preparing the WoR-PE installer".into(),
    });
    let pe = acquire_pe(
        options, stage, &query, report, progress, cancelled, provider,
    )?;
    validate_pe(&pe)?;
    progress(Progress::Step {
        current: 2,
        total: 6,
        message: "Preparing ARM64 drivers and UEFI firmware".into(),
    });
    let drivers = if options.model != Model::Pi5 {
        let directory = acquire_release(
            options,
            stage,
            &query,
            ReleaseKind::Drivers,
            report,
            progress,
            cancelled,
            provider,
        )?;
        validate_drivers(&directory, options.model)?;
        Some(directory)
    } else {
        None
    };
    let firmware = acquire_release(
        options,
        stage,
        &query,
        ReleaseKind::Uefi,
        report,
        progress,
        cancelled,
        provider,
    )?;
    validate_firmware(&firmware, options.model)?;
    progress(Progress::Step {
        current: 3,
        total: 6,
        message: "Preparing and verifying the Windows image".into(),
    });
    let (windows, build, source) = acquire_windows(
        options,
        stage,
        requested.as_ref(),
        &query,
        report,
        progress,
        cancelled,
        provider,
    )?;
    report.source = source;
    let copy_bytes = tree_bytes(&windows)?
        .checked_add(tree_bytes(&pe)?)
        .and_then(|n| n.checked_add(tree_bytes(&firmware).ok()?))
        .and_then(|n| n.checked_add(512 * 1024 * 1024))
        .ok_or(MediaError::InvalidMetadata(
            "private media space requirement",
        ))?;
    require_space(copy_bytes, provider.available_space(stage)?)?;
    progress(Progress::Step {
        current: 4,
        total: 6,
        message: "Building a private startup environment".into(),
    });
    let boot = stage.join("boot");
    let extras = stage.join("install-extras");
    make_directory(&extras)?;
    copy_tree(&windows.join("bootpart"), &boot, cancelled)?;
    let install = stage.join("install.wim");
    copy_file(&windows.join("install.wim"), &install, cancelled)?;
    let install = normalize_pi3(options, &install, stage, progress, cancelled, provider)?;
    let private_pe = stage.join("pe");
    copy_tree(&pe, &private_pe, cancelled)?;
    let answer = answer_file(options, &build)?;
    let shell = if options.model == Model::Pi4 && options.setup.pi4_uefi_shell_unlock {
        Some(acquire_shell(options, stage, cancelled, provider)?)
    } else {
        None
    };
    let app = configure_pe(
        options,
        &private_pe,
        answer.as_deref(),
        shell.as_deref(),
        cancelled,
    )?;
    copy_tree_merge(
        &find_case_insensitive(&private_pe, "efi")?,
        &find_case_insensitive(&boot, "efi")?,
        cancelled,
    )?;
    let boot_wim = boot.join("sources/boot.wim");
    verify(&boot_wim, stage, cancelled, provider)?;
    let (boot_info, boot_images) = inspect(&boot_wim, stage, cancelled, provider)?;
    if boot_info.image_count < 2 || boot_images.iter().any(|image| image.architecture != 12) {
        return Err(MediaError::InvalidMetadata(
            "Windows ARM64 Setup/PE image 2 is required",
        ));
    }
    progress(Progress::Step {
        current: 5,
        total: 6,
        message: "Configuring WoR-PE and Windows setup".into(),
    });
    provider.wim(
        WimOperation::Update,
        &[
            boot_wim.as_os_str().into(),
            "2".into(),
            add_tree_command(&app, "/")?,
            "--check".into(),
        ],
        stage,
        cancelled,
    )?;
    let driver_command = if let Some(drivers) = drivers {
        let private_drivers = stage.join("drivers");
        copy_tree(&drivers, &private_drivers, cancelled)?;
        add_tree_command(&private_drivers, "/drivers")?
    } else {
        let critical = stage.join("critical");
        drop(crate::files::private_file(&critical)?);
        add_tree_command(&critical, "/drivers/critical")?
    };
    provider.wim(
        WimOperation::Update,
        &[
            boot_wim.as_os_str().into(),
            "2".into(),
            driver_command,
            "--check".into(),
        ],
        stage,
        cancelled,
    )?;
    stage_answers(options, answer.as_deref(), &boot, &extras)?;
    copy_tree_merge(&firmware, &boot, cancelled)?;
    if let Some(config) = config {
        write_private(&boot.join("config.txt"), config.as_bytes())?;
    }
    progress(Progress::Step {
        current: 6,
        total: 6,
        message: "Verifying prepared media before any device access".into(),
    });
    verify(&boot_wim, stage, cancelled, provider)?;
    verify(&install, stage, cancelled, provider)?;
    validate_boot_tree(&boot, options.model)?;
    validate_pi3_helper(options, cancelled)?;
    check_cancelled(cancelled)?;
    Ok((boot, install, extras, build))
}

#[allow(clippy::too_many_arguments)]
fn acquire_pe(
    options: &DesktopOptions,
    stage: &Path,
    _query: &QueryOptions,
    report: &mut PreparationReport,
    progress: &mut dyn FnMut(Progress),
    cancelled: &AtomicBool,
    provider: &mut dyn MediaProvider,
) -> Result<PathBuf, MediaError> {
    let folder = options.download_dir.join("peinstaller");
    if options.cache_mode == CacheMode::Trust
        && cache_is_current(&folder, "", options.cache_mode, cancelled)?
    {
        report.packages.push(PackageReport {
            kind: "pe".into(),
            version: "unchecked-cache".into(),
            origin: "unchecked-cache".into(),
            cache_reused: true,
        });
        return Ok(folder);
    }
    let mut url = options.packages.pe_url.clone();
    let mut hash = options.packages.pe_sha256.clone();
    let mut origin = "pinned";
    if options.packages.pe_latest {
        let latest = fetch_metadata(
            "https://worproject.com/dldserv/worpe/gethashlatest.php",
            "latest WoR-PE digest",
            &options.cache_dir,
            cancelled,
            provider,
        );
        let latest = match latest {
            Ok(bytes) => String::from_utf8(bytes)
                .ok()
                .map(|text| text.split(':').nth(1).unwrap_or(&text).trim().to_owned())
                .filter(|hash: &String| {
                    crate::options::validate_digest(hash, 64, "latest PE digest").is_ok()
                }),
            Err(MediaError::Cancelled) => return Err(MediaError::Cancelled),
            Err(error @ MediaError::Cleanup { .. }) => return Err(error),
            Err(_) => None,
        };
        if let Some(latest) = latest {
            hash = latest;
            url = "https://worproject.com/dldserv/worpe/downloadlatest.php".into();
            origin = "latest";
        } else {
            warn(
                report,
                progress,
                "pe-pinned-fallback",
                "Could not retrieve a valid latest WoR-PE digest; using the configured digest-pinned package.",
            );
        }
    }
    let reused = cache_is_current(&folder, &hash, options.cache_mode, cancelled)?;
    if !reused {
        let extracted = stage.join("new-pe");
        download_zip(
            &url,
            Some(&hash),
            "WoR-PE installer",
            &extracted,
            stage,
            options.verify_tls,
            cancelled,
            provider,
        )?;
        validate_pe(&extracted)?;
        mark_cache(&extracted, &hash, cancelled)?;
        replace_cache(&extracted, &folder, stage)?;
    }
    report.packages.push(PackageReport {
        kind: "pe".into(),
        version: hash,
        origin: origin.into(),
        cache_reused: reused,
    });
    Ok(folder)
}

#[allow(clippy::too_many_arguments)]
fn acquire_release(
    options: &DesktopOptions,
    stage: &Path,
    query: &QueryOptions,
    kind: ReleaseKind,
    report: &mut PreparationReport,
    progress: &mut dyn FnMut(Progress),
    cancelled: &AtomicBool,
    provider: &mut dyn MediaProvider,
) -> Result<PathBuf, MediaError> {
    let folder = options.download_dir.join(if kind == ReleaseKind::Uefi {
        format!("pi{}-uefipackage", options.model.number())
    } else {
        "driverpackage".into()
    });
    if options.cache_mode == CacheMode::Trust
        && cache_is_current(&folder, "", options.cache_mode, cancelled)?
    {
        report.packages.push(PackageReport {
            kind: kind.name().into(),
            version: "unchecked-cache".into(),
            origin: "unchecked-cache".into(),
            cache_reused: true,
        });
        return Ok(folder);
    }
    let (latest, pinned) = if kind == ReleaseKind::Uefi {
        (
            options.packages.uefi_latest,
            options.packages.uefi_version.as_str(),
        )
    } else {
        (
            options.packages.drivers_latest,
            options.packages.driver_version.as_str(),
        )
    };
    let mut selected = pinned_release(kind, options.model, pinned, &options.defaults)?;
    let mut origin = "pinned";
    if latest {
        match query_release_versions_with_provider(
            query,
            kind,
            options.model,
            ReleaseSelection::Latest,
            cancelled,
            provider,
        ) {
            Ok(releases) => {
                selected = releases
                    .into_iter()
                    .next()
                    .ok_or(MediaError::InvalidMetadata("latest compatible release"))?;
                origin = "latest";
            }
            Err(MediaError::Cancelled) => return Err(MediaError::Cancelled),
            Err(error @ MediaError::Cleanup { .. }) => return Err(error),
            Err(_) => warn(
                report,
                progress,
                &format!("{}-pinned-fallback", kind.name()),
                &format!(
                    "Could not query a compatible latest {} release; using pinned version {pinned}.",
                    kind.name()
                ),
            ),
        }
    }
    let reused = cache_is_current(
        &folder,
        &selected.download_url,
        options.cache_mode,
        cancelled,
    )?;
    if !reused {
        let extracted = stage.join(format!("new-{}", kind.name()));
        download_zip(
            &selected.download_url,
            None,
            kind.name(),
            &extracted,
            stage,
            options.verify_tls,
            cancelled,
            provider,
        )?;
        if kind == ReleaseKind::Uefi {
            validate_firmware(&extracted, options.model)?;
        } else {
            validate_drivers(&extracted, options.model)?;
        }
        mark_cache(&extracted, &selected.download_url, cancelled)?;
        replace_cache(&extracted, &folder, stage)?;
    }
    report.packages.push(PackageReport {
        kind: kind.name().into(),
        version: selected.tag,
        origin: origin.into(),
        cache_reused: reused,
    });
    Ok(folder)
}

#[allow(clippy::too_many_arguments)]
fn download_zip(
    url: &str,
    expected: Option<&str>,
    component: &'static str,
    extracted: &Path,
    stage: &Path,
    verify_tls: bool,
    cancelled: &AtomicBool,
    provider: &mut dyn MediaProvider,
) -> Result<(), MediaError> {
    check_cancelled(cancelled)?;
    let download = stage.join(format!(
        "{}.zip",
        extracted.file_name().unwrap().to_string_lossy()
    ));
    provider.fetch(
        &DownloadRequest {
            url: safe_url(url)?,
            component,
            verify_tls,
            maximum_bytes: 2 * 1024 * 1024 * 1024,
            timeout: std::time::Duration::from_secs(6 * 60 * 60),
        },
        &download,
        cancelled,
    )?;
    required_file(&download, component)?;
    if let Some(expected) = expected
        && !sha256_file(&download, cancelled)?.eq_ignore_ascii_case(expected)
    {
        return Err(MediaError::Integrity { component });
    }
    extract_zip(
        &download,
        extracted,
        ArchiveLimits {
            entries: 100_000,
            total_bytes: 4 * 1024 * 1024 * 1024,
            file_bytes: 2 * 1024 * 1024 * 1024,
        },
        cancelled,
    )?;
    fs::remove_file(download).map_err(|e| MediaError::io("remove verified package archive", e))
}

fn replace_cache(extracted: &Path, destination: &Path, stage: &Path) -> Result<(), MediaError> {
    let backup = stage.join(format!(
        "old-{}",
        destination.file_name().unwrap().to_string_lossy()
    ));
    if fs::symlink_metadata(destination).is_ok() {
        require_directory(destination, "existing cache directory")?;
        fs::rename(destination, &backup)
            .map_err(|e| MediaError::io("retain previous base cache", e))?;
    }
    if let Err(error) = fs::rename(extracted, destination) {
        if backup.exists()
            && let Err(source) = fs::rename(&backup, destination)
        {
            return Err(MediaError::Cleanup {
                primary: Some(Box::new(MediaError::io(
                    "promote verified base cache",
                    error,
                ))),
                source,
            });
        }
        return Err(MediaError::io("promote verified base cache", error));
    }
    if backup.exists() {
        fs::remove_dir_all(&backup).map_err(|e| MediaError::io("remove replaced base cache", e))?;
    }
    Ok(())
}

fn validate_pe(directory: &Path) -> Result<(), MediaError> {
    require_directory(&directory.join("winpe/2"), "WoR-PE application payload")?;
    required_file(
        &directory.join("winpe/2/settings.ini"),
        "WoR-PE settings.ini",
    )?;
    required_file(
        &find_case_insensitive(directory, "efi/boot/bootaa64.efi")?,
        "WoR-PE EFI loader",
    )?;
    if !tree_files(&directory.join("winpe/2"))?
        .iter()
        .any(|(_, file)| {
            file.extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("exe"))
                && file.metadata().is_ok_and(|m| m.len() > 0)
        })
    {
        return Err(MediaError::InvalidMetadata(
            "WoR-PE application executable is missing",
        ));
    }
    Ok(())
}

fn validate_drivers(directory: &Path, model: Model) -> Result<(), MediaError> {
    if model == Model::Pi4 {
        for name in PI4_REQUIRED_DRIVERS {
            required_file(
                &find_case_insensitive(directory, name)?,
                "Pi 4 ARM64 driver",
            )?;
        }
    } else if !tree_files(directory)?.iter().any(|(_, file)| {
        file.extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("inf"))
            && file.metadata().is_ok_and(|m| m.len() > 0)
    }) {
        return Err(MediaError::InvalidMetadata(
            "Pi 3 ARM64 driver package has no INF",
        ));
    }
    Ok(())
}

fn firmware_files(model: Model) -> &'static [&'static str] {
    match model {
        Model::Pi3 => &["RPI_EFI.fd", "config.txt", "bootcode.bin", "start.elf"],
        Model::Pi4 => &["RPI_EFI.fd", "config.txt", "start4.elf", "fixup4.dat"],
        Model::Pi5 => &["RPI_EFI.fd", "config.txt", "bcm2712-rpi-5-b.dtb"],
    }
}

fn validate_firmware(directory: &Path, model: Model) -> Result<(), MediaError> {
    for name in firmware_files(model) {
        required_file(
            &find_case_insensitive(directory, name)?,
            "UEFI firmware boot artifact",
        )?;
    }
    Ok(())
}

fn validate_boot_tree(directory: &Path, model: Model) -> Result<(), MediaError> {
    validate_firmware(directory, model)?;
    for name in [
        "boot/bcd",
        "efi/boot/bootaa64.efi",
        "efi/microsoft/boot/bcd",
        "sources/boot.wim",
    ] {
        required_file(
            &find_case_insensitive(directory, name)?,
            "complete Windows startup tree",
        )?;
    }
    Ok(())
}

fn windows_token(build: &BuildId, language: &str, from_iso: bool) -> String {
    format!(
        "windows:{}:{build}:{language}",
        if from_iso { "iso" } else { "esd" }
    )
}

#[allow(clippy::too_many_arguments)]
fn acquire_windows(
    options: &DesktopOptions,
    stage: &Path,
    requested: Option<&BuildId>,
    query: &QueryOptions,
    report: &mut PreparationReport,
    progress: &mut dyn FnMut(Progress),
    cancelled: &AtomicBool,
    provider: &mut dyn MediaProvider,
) -> Result<(PathBuf, BuildId, String), MediaError> {
    if matches!(options.source, WindowsSource::Download)
        && let Some(build) = requested
    {
        for from_iso in [true, false] {
            let directory = options.download_dir.join(format!(
                "winfiles_{}{build}_{}",
                if from_iso { "from_iso_" } else { "" },
                options.language
            ));
            if directory.join("alldone").is_file()
                && cache_is_current(
                    &directory,
                    &windows_token(build, &options.language, from_iso),
                    options.cache_mode,
                    cancelled,
                )?
            {
                validate_windows_base(&directory)?;
                verify(&directory.join("install.wim"), stage, cancelled, provider)?;
                let (_, metadata) =
                    inspect(&directory.join("install.wim"), stage, cancelled, provider)?;
                let build = validate_install_metadata(options, requested, &metadata)?;
                return Ok((
                    directory,
                    build,
                    if from_iso {
                        "cached-iso".into()
                    } else {
                        "cached-esd".into()
                    },
                ));
            }
        }
    }
    let extracted = stage.join("new-windows");
    make_directory(&extracted)?;
    let (source, from_iso) = match &options.source {
        WindowsSource::Iso(path) => {
            let size = required_file(path, "selected Windows ISO")?;
            if size < 3 * 1024 * 1024 * 1024 {
                return Err(crate::options::option(
                    "SOURCE_FILE",
                    "the Windows ISO is smaller than 3 GiB and probably incomplete",
                ));
            }
            (path.clone(), true)
        }
        WindowsSource::Esd(path) => {
            required_file(path, "selected Windows ESD")?;
            (path.clone(), false)
        }
        WindowsSource::Download => {
            let build = requested.ok_or(MediaError::InvalidMetadata("selected Windows build"))?;
            let entry = query_catalogue_with_provider(
                query,
                build,
                &options.language,
                cancelled,
                provider,
            )?;
            if entry.cached_metadata {
                warn(
                    report,
                    progress,
                    "cached-windows-catalogue",
                    "The Windows ESD catalogue request failed; using its integrity-checked local metadata cache.",
                );
            }
            if entry.size >= 4 * 1024 * 1024 * 1024
                && provider
                    .file_size_limit(&options.download_dir, cancelled)?
                    .is_some_and(|maximum| entry.size > maximum)
            {
                return Err(crate::options::option(
                    "DL_DIR",
                    "the selected FAT download filesystem cannot store this ESD; choose another download directory",
                ));
            }
            let required = entry
                .size
                .checked_add(768 * 1024 * 1024)
                .ok_or(MediaError::InvalidMetadata("ESD free-space requirement"))?;
            require_space(required, provider.available_space(&options.download_dir)?)?;
            (
                acquire_esd(options, stage, &entry, cancelled, provider)?,
                false,
            )
        }
    };
    check_cancelled(cancelled)?;
    if from_iso {
        let iso = stage.join("iso-files");
        provider.extract_iso(&source, &iso, cancelled)?;
        for name in ["boot", "efi"] {
            copy_tree(
                &find_case_insensitive(&iso, name)?,
                &extracted.join("bootpart").join(name),
                cancelled,
            )?;
        }
        make_directory(&extracted.join("bootpart/sources"))?;
        copy_file(
            &find_case_insensitive(&iso, "sources/boot.wim")?,
            &extracted.join("bootpart/sources/boot.wim"),
            cancelled,
        )?;
        let image = find_case_insensitive(&iso, "sources/install.wim")
            .or_else(|_| find_case_insensitive(&iso, "sources/install.esd"))?;
        copy_file(&image, &extracted.join("install.wim"), cancelled)?;
    } else {
        verify(&source, stage, cancelled, provider)?;
        let info = provider.wim(
            WimOperation::Info,
            &[source.as_os_str().into()],
            stage,
            cancelled,
        )?;
        let info = crate::parse_wim_info(
            std::str::from_utf8(&info)
                .map_err(|_| MediaError::InvalidMetadata("ESD image information"))?,
        )?;
        if info.image_count < 4 {
            return Err(MediaError::InvalidMetadata(
                "ESD must contain boot files, PE, Setup and Windows installation editions",
            ));
        }
        provider.wim(
            WimOperation::Extract,
            &[
                source.as_os_str().into(),
                "1".into(),
                "boot".into(),
                "efi".into(),
                format!(
                    "--dest-dir={}",
                    extracted
                        .join("bootpart")
                        .to_str()
                        .ok_or(MediaError::InvalidMetadata("ESD staging path"))?
                )
                .into(),
            ],
            stage,
            cancelled,
        )?;
        make_directory(&extracted.join("bootpart/sources"))?;
        let boot = extracted.join("bootpart/sources/boot.wim");
        for (index, bootable) in [("2", false), ("3", true)] {
            let mut args: Vec<OsString> = vec![
                source.as_os_str().into(),
                index.into(),
                boot.as_os_str().into(),
                "--compress=LZX".into(),
            ];
            if bootable {
                args.push("--boot".into());
            }
            provider.wim(WimOperation::Export, &args, stage, cancelled)?;
        }
        let working_esd = extracted.join("working.esd");
        copy_file(&source, &working_esd, cancelled)?;
        for soft in [true, true, false] {
            let mut args: Vec<OsString> = vec![working_esd.as_os_str().into(), "1".into()];
            if soft {
                args.push("--soft".into());
            }
            provider.wim(WimOperation::Delete, &args, stage, cancelled)?;
        }
        fs::rename(&working_esd, extracted.join("install.wim"))
            .map_err(|e| MediaError::io("stage Windows installation editions", e))?;
    }
    validate_windows_base(&extracted)?;
    verify(&extracted.join("install.wim"), stage, cancelled, provider)?;
    let (_, metadata) = inspect(&extracted.join("install.wim"), stage, cancelled, provider)?;
    let build = validate_install_metadata(options, requested, &metadata)?;
    write_private(&extracted.join("alldone"), b"complete\n")?;
    mark_cache(
        &extracted,
        &windows_token(&build, &options.language, from_iso),
        cancelled,
    )?;
    let folder = options.download_dir.join(format!(
        "winfiles_{}{build}_{}",
        if from_iso { "from_iso_" } else { "" },
        options.language
    ));
    replace_cache(&extracted, &folder, stage)?;
    Ok((
        folder,
        build,
        if from_iso { "iso".into() } else { "esd".into() },
    ))
}

fn validate_windows_base(directory: &Path) -> Result<(), MediaError> {
    for name in [
        "install.wim",
        "bootpart/sources/boot.wim",
        "bootpart/boot/bcd",
        "bootpart/efi/boot/bootaa64.efi",
        "bootpart/efi/microsoft/boot/bcd",
    ] {
        required_file(
            &find_case_insensitive(directory, name)?,
            "Windows media base",
        )?;
    }
    Ok(())
}

fn acquire_esd(
    options: &DesktopOptions,
    stage: &Path,
    entry: &CatalogueEntry,
    cancelled: &AtomicBool,
    provider: &mut dyn MediaProvider,
) -> Result<PathBuf, MediaError> {
    let sources = options.download_dir.join(".wor-sources");
    make_directory(&sources)?;
    let name = entry
        .sha256
        .as_ref()
        .or(entry.sha1.as_ref())
        .ok_or(MediaError::InvalidMetadata("ESD digest"))?;
    let source = sources.join(format!("{name}.esd"));
    if options.cache_mode != CacheMode::Refresh
        && source.exists()
        && verify_esd(&source, entry, cancelled).is_ok()
    {
        return Ok(source);
    }
    let downloaded = stage.join("downloaded.esd");
    provider.fetch(
        &DownloadRequest {
            url: safe_url(&entry.url)?,
            component: "Windows ESD",
            verify_tls: options.verify_tls,
            maximum_bytes: entry.size,
            timeout: std::time::Duration::from_secs(6 * 60 * 60),
        },
        &downloaded,
        cancelled,
    )?;
    verify_esd(&downloaded, entry, cancelled)?;
    if source.exists() {
        required_file(&source, "cached ESD source")?;
        fs::remove_file(&source).map_err(|e| MediaError::io("replace corrupt ESD cache", e))?;
    }
    fs::rename(downloaded, &source)
        .map_err(|e| MediaError::io("promote verified ESD source", e))?;
    Ok(source)
}

fn verify_esd(
    path: &Path,
    entry: &CatalogueEntry,
    cancelled: &AtomicBool,
) -> Result<(), MediaError> {
    if required_file(path, "ESD source")? != entry.size {
        return Err(MediaError::Integrity {
            component: "ESD source size",
        });
    }
    let mut input = File::open(path).map_err(|e| MediaError::io("open ESD source", e))?;
    let mut sha1 = Sha1::new();
    let mut sha256 = Sha256::new();
    let mut buffer = vec![0u8; 1024 * 1024];
    loop {
        check_cancelled(cancelled)?;
        let count = input
            .read(&mut buffer)
            .map_err(|e| MediaError::io("hash ESD source", e))?;
        if count == 0 {
            break;
        }
        sha1.update(&buffer[..count]);
        sha256.update(&buffer[..count]);
    }
    if entry
        .sha1
        .as_ref()
        .is_some_and(|expected| !format!("{:x}", sha1.finalize()).eq_ignore_ascii_case(expected))
        || entry.sha256.as_ref().is_some_and(|expected| {
            !format!("{:x}", sha256.finalize()).eq_ignore_ascii_case(expected)
        })
    {
        return Err(MediaError::Integrity {
            component: "Microsoft ESD digest",
        });
    }
    Ok(())
}

fn acquire_shell(
    options: &DesktopOptions,
    stage: &Path,
    cancelled: &AtomicBool,
    provider: &mut dyn MediaProvider,
) -> Result<PathBuf, MediaError> {
    let folder = options.download_dir.join("uefi-shell");
    let expected = &options.defaults.uefi_shell_package_sha256;
    if cache_is_current(&folder, expected, CacheMode::Verify, cancelled)?
        && required_file(&folder.join("Shell.efi"), "ARM64 UEFI Shell").is_ok()
    {
        return Ok(folder.join("Shell.efi"));
    }
    let extracted = stage.join("new-shell-package");
    download_zip(
        &options.defaults.uefi_shell_package_url,
        Some(expected),
        "ARM64 UEFI Shell",
        &extracted,
        stage,
        options.verify_tls,
        cancelled,
        provider,
    )?;
    let shell = find_case_insensitive(&extracted, "ShellBinPkg/UefiShell/AArch64/Shell.efi")?;
    required_file(&shell, "ARM64 UEFI Shell")?;
    let normalized = stage.join("new-shell");
    make_directory(&normalized)?;
    copy_file(&shell, &normalized.join("Shell.efi"), cancelled)?;
    mark_cache(&normalized, expected, cancelled)?;
    replace_cache(&normalized, &folder, stage)?;
    Ok(folder.join("Shell.efi"))
}
