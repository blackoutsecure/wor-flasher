use crate::files::{make_directory, require_space, required_file};
use crate::{
    BuildId, DesktopOptions, MediaError, MediaProvider, Model, WimOperation, check_cancelled,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use wor_domain::progress::Progress;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WimInfo {
    pub image_count: u32,
    pub compression: String,
    pub chunk_size: u64,
    pub part_number: u32,
    pub total_parts: u32,
    pub boot_index: u32,
    pub editions: String,
}

pub fn parse_wim_info(text: &str) -> Result<WimInfo, MediaError> {
    fn one<'a>(text: &'a str, prefix: &str) -> Result<&'a str, MediaError> {
        let mut matches = text.lines().filter_map(|line| line.strip_prefix(prefix));
        let value = matches
            .next()
            .map(str::trim)
            .ok_or(MediaError::InvalidMetadata("WIM information"))?;
        if matches.next().is_some() {
            return Err(MediaError::InvalidMetadata("duplicate WIM information"));
        }
        Ok(value)
    }
    let image_count = one(text, "Image Count:")?
        .parse::<u32>()
        .ok()
        .filter(|n| *n > 0 && *n <= 1024)
        .ok_or(MediaError::InvalidMetadata("WIM image count"))?;
    let compression = one(text, "Compression:")?.to_owned();
    let chunk_size =
        if compression == "None" && !text.lines().any(|line| line.starts_with("Chunk Size:")) {
            0
        } else {
            one(text, "Chunk Size:")?
                .strip_suffix(" bytes")
                .and_then(|s| s.parse::<u64>().ok())
                .ok_or(MediaError::InvalidMetadata("WIM chunk size"))?
        };
    let (part, total) = one(text, "Part Number:")?
        .split_once('/')
        .ok_or(MediaError::InvalidMetadata("WIM part number"))?;
    let part_number = part
        .parse::<u32>()
        .map_err(|_| MediaError::InvalidMetadata("WIM part number"))?;
    let total_parts = total
        .parse::<u32>()
        .map_err(|_| MediaError::InvalidMetadata("WIM part count"))?;
    if part_number != 1 || total_parts != 1 {
        return Err(MediaError::InvalidMetadata(
            "standalone WIM required; split media is unsupported",
        ));
    }
    let boot_index = one(text, "Boot Index:")?
        .parse::<u32>()
        .map_err(|_| MediaError::InvalidMetadata("WIM boot index"))?;
    if boot_index > image_count {
        return Err(MediaError::InvalidMetadata("WIM boot index"));
    }
    let start = text
        .find("Available Images:")
        .ok_or(MediaError::InvalidMetadata("WIM edition list"))?;
    let editions = text[start..].replace("\r\n", "\n").trim_end().to_owned();
    let indexes: Vec<_> = editions
        .lines()
        .filter_map(|l| l.strip_prefix("Index:"))
        .map(|v| v.trim().parse::<u32>())
        .collect::<Result<_, _>>()
        .map_err(|_| MediaError::InvalidMetadata("WIM edition index"))?;
    if indexes != (1..=image_count).collect::<Vec<_>>() {
        return Err(MediaError::InvalidMetadata(
            "WIM edition count/index mismatch",
        ));
    }
    Ok(WimInfo {
        image_count,
        compression,
        chunk_size,
        part_number,
        total_parts,
        boot_index,
        editions,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResourceSummary {
    pub solid: bool,
    pub uncompressed_bytes: u64,
}

pub fn parse_resource_summary(text: &str) -> Result<ResourceSummary, MediaError> {
    let mut solid = false;
    let mut bytes = 0u64;
    let mut found = false;
    for line in text.lines() {
        if let Some(size) = line.strip_prefix("Uncompressed size = ") {
            let size = size
                .strip_suffix(" bytes")
                .and_then(|size| size.parse::<u64>().ok())
                .ok_or(MediaError::InvalidMetadata("WIM resource size"))?;
            bytes = bytes
                .checked_add(size)
                .ok_or(MediaError::InvalidMetadata("WIM resource size overflow"))?;
            found = true;
        }
        solid |= line.contains("WIM_RESHDR_FLAG_SOLID");
    }
    if !found || bytes == 0 {
        return Err(MediaError::InvalidMetadata(
            "WIM uncompressed resource size",
        ));
    }
    Ok(ResourceSummary {
        solid,
        uncompressed_bytes: bytes,
    })
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ImageMetadata {
    pub index: u32,
    pub architecture: u32,
    pub build: Option<u32>,
    pub languages: Vec<String>,
}

pub fn parse_image_metadata(bytes: &[u8]) -> Result<Vec<ImageMetadata>, MediaError> {
    let root = crate::xml::parse(bytes)?;
    if root.name != "WIM" {
        return Err(MediaError::InvalidMetadata("WIM XML root"));
    }
    let mut result = Vec::new();
    let mut seen = BTreeSet::new();
    for image in root.children.iter().filter(|node| node.name == "IMAGE") {
        let index = image
            .attributes
            .get("INDEX")
            .and_then(|v| v.parse::<u32>().ok())
            .filter(|n| *n > 0)
            .ok_or(MediaError::InvalidMetadata("WIM XML image index"))?;
        if !seen.insert(index) {
            return Err(MediaError::InvalidMetadata("duplicate WIM XML index"));
        }
        let windows = image
            .child("WINDOWS")
            .ok_or(MediaError::InvalidMetadata("Windows image metadata"))?;
        let architecture = windows
            .value("ARCH")
            .and_then(|v| v.parse::<u32>().ok())
            .ok_or(MediaError::InvalidMetadata("Windows image architecture"))?;
        let build = windows
            .child("VERSION")
            .and_then(|v| v.value("BUILD"))
            .map(|b| {
                b.parse::<u32>()
                    .map_err(|_| MediaError::InvalidMetadata("Windows image build"))
            })
            .transpose()?;
        let mut languages = Vec::new();
        let mut seen_languages = BTreeSet::new();
        if let Some(nodes) = windows.child("LANGUAGES") {
            for language in &nodes.children {
                if matches!(language.name.as_str(), "LANGUAGE" | "DEFAULT") {
                    let code = language.text.trim().to_ascii_lowercase();
                    if crate::options::valid_locale(&code) && seen_languages.insert(code.clone()) {
                        languages.push(code);
                    }
                }
            }
        }
        result.push(ImageMetadata {
            index,
            architecture,
            build,
            languages,
        });
    }
    if result.is_empty() {
        return Err(MediaError::InvalidMetadata("empty Windows image metadata"));
    }
    Ok(result)
}

pub(crate) fn inspect(
    image: &Path,
    work: &Path,
    cancelled: &AtomicBool,
    provider: &mut dyn MediaProvider,
) -> Result<(WimInfo, Vec<ImageMetadata>), MediaError> {
    check_cancelled(cancelled)?;
    required_file(image, "Windows image")?;
    let bytes = provider.wim(
        WimOperation::Info,
        &[image.as_os_str().into()],
        work,
        cancelled,
    )?;
    let info = parse_wim_info(
        std::str::from_utf8(&bytes)
            .map_err(|_| MediaError::InvalidMetadata("WIM information encoding"))?,
    )?;
    let bytes = provider.wim(
        WimOperation::Info,
        &[image.as_os_str().into(), "--xml".into()],
        work,
        cancelled,
    )?;
    let metadata = parse_image_metadata(&bytes)?;
    if metadata.len() != info.image_count as usize
        || metadata
            .iter()
            .enumerate()
            .any(|(i, image)| image.index != i as u32 + 1)
    {
        return Err(MediaError::InvalidMetadata(
            "WIM XML edition count/index mismatch",
        ));
    }
    Ok((info, metadata))
}

pub(crate) fn validate_install_metadata(
    options: &DesktopOptions,
    requested: Option<&BuildId>,
    metadata: &[ImageMetadata],
) -> Result<BuildId, MediaError> {
    let first = metadata
        .first()
        .ok_or(MediaError::InvalidMetadata("Windows installation editions"))?;
    let major = first
        .build
        .ok_or(MediaError::InvalidMetadata("Windows installation build"))?;
    let actual = BuildId::parse(&major.to_string())?;
    if metadata
        .iter()
        .any(|image| image.architecture != 12 || image.build != Some(major))
    {
        return Err(MediaError::InvalidMetadata(
            "all installation editions must be ARM64 and the same Windows build",
        ));
    }
    if !options
        .model
        .supports(&actual, options.defaults.armv80_max_build)
    {
        return Err(crate::options::option(
            "BID",
            "installation image requires ARMv8.1; Pi 3 and Pi 4 are ARMv8.0",
        ));
    }
    if let Some(requested) = requested {
        if requested.major() != major {
            return Err(MediaError::InvalidMetadata(
                "selected and actual Windows build differ",
            ));
        }
        Ok(requested.clone())
    } else {
        Ok(actual)
    }
}

pub(crate) fn verify(
    image: &Path,
    work: &Path,
    cancelled: &AtomicBool,
    provider: &mut dyn MediaProvider,
) -> Result<(), MediaError> {
    required_file(image, "Windows image")?;
    check_cancelled(cancelled)?;
    provider.wim(
        WimOperation::Verify,
        &[image.as_os_str().into()],
        work,
        cancelled,
    )?;
    check_cancelled(cancelled)
}

pub(crate) fn normalize_pi3(
    options: &DesktopOptions,
    image: &Path,
    stage: &Path,
    progress: &mut dyn FnMut(Progress),
    cancelled: &AtomicBool,
    provider: &mut dyn MediaProvider,
) -> Result<PathBuf, MediaError> {
    if options.model != Model::Pi3 {
        return Ok(image.to_path_buf());
    }
    progress(Progress::Status(
        "Checking the Pi 3 installation-image layout".into(),
    ));
    let source = provider.wim(
        WimOperation::Info,
        &[image.as_os_str().into()],
        stage,
        cancelled,
    )?;
    let source = parse_wim_info(
        std::str::from_utf8(&source)
            .map_err(|_| MediaError::InvalidMetadata("WIM information encoding"))?,
    )?;
    let resources = provider.wim(
        WimOperation::Info,
        &[image.as_os_str().into(), "--blobs".into()],
        stage,
        cancelled,
    )?;
    let resources = parse_resource_summary(
        std::str::from_utf8(&resources)
            .map_err(|_| MediaError::InvalidMetadata("WIM resource encoding"))?,
    )?;
    let mut candidate = image.to_path_buf();
    if source.compression != "LZX" || source.chunk_size != 32768 || resources.solid {
        let required = resources
            .uncompressed_bytes
            .checked_add(512 * 1024 * 1024)
            .ok_or(MediaError::InvalidMetadata(
                "Pi 3 conversion space overflow",
            ))?;
        require_space(required, provider.available_space(stage)?)?;
        let directory = stage.join("pi3-normalized");
        make_directory(&directory)?;
        candidate = directory.join("install.wim");
        if candidate.exists() {
            return Err(MediaError::InvalidMetadata(
                "Pi 3 conversion output already exists",
            ));
        }
        progress(Progress::Status(
            "Converting the Pi 3 image to non-solid LZX, 32 KiB chunks".into(),
        ));
        provider.wim(
            WimOperation::Export,
            &[
                image.as_os_str().into(),
                "all".into(),
                candidate.as_os_str().into(),
                "--compress=LZX".into(),
                "--chunk-size=32768".into(),
                "--recompress".into(),
                "--check".into(),
                "--threads=2".into(),
            ],
            stage,
            cancelled,
        )?;
        verify(&candidate, stage, cancelled, provider)?;
        let info = provider.wim(
            WimOperation::Info,
            &[candidate.as_os_str().into()],
            stage,
            cancelled,
        )?;
        let info = parse_wim_info(
            std::str::from_utf8(&info)
                .map_err(|_| MediaError::InvalidMetadata("converted WIM information"))?,
        )?;
        let blobs = provider.wim(
            WimOperation::Info,
            &[candidate.as_os_str().into(), "--blobs".into()],
            stage,
            cancelled,
        )?;
        let blobs = parse_resource_summary(
            std::str::from_utf8(&blobs)
                .map_err(|_| MediaError::InvalidMetadata("converted WIM resource information"))?,
        )?;
        if info.compression != "LZX" || info.chunk_size != 32768 || blobs.solid {
            return Err(MediaError::InvalidMetadata(
                "converted Pi 3 image is not non-solid LZX with 32 KiB chunks",
            ));
        }
        if info.editions != source.editions {
            return Err(MediaError::Integrity {
                component: "Pi 3 edition metadata",
            });
        }
    }
    let size = required_file(&candidate, "prepared Pi 3 installation image")?;
    if size > options.installation_mode.maximum_install_bytes() {
        return Err(crate::options::option(
            "CAN_INSTALL_ON_SAME_DRIVE",
            "the normalized Pi 3 image does not fit the installation partition",
        ));
    }
    Ok(candidate)
}

pub(crate) fn add_tree_command(source: &Path, destination: &str) -> Result<OsString, MediaError> {
    let source = source.to_str().ok_or(MediaError::InvalidMetadata(
        "WIM update source path encoding",
    ))?;
    // wimlib's update language supports either quote delimiter but has no shell
    // expansion and no backslash-escape syntax. Select a delimiter not present
    // in the token; never insert an unquotable or multiline token.
    fn quote(text: &str) -> Result<String, MediaError> {
        if text.contains(['\0', '\r', '\n']) {
            return Err(MediaError::InvalidMetadata("WIM update token"));
        }
        if !text.contains('"') {
            Ok(format!("\"{text}\""))
        } else if !text.contains('\'') {
            Ok(format!("'{text}'"))
        } else {
            Err(MediaError::InvalidMetadata(
                "WIM update path contains both quote delimiters",
            ))
        }
    }
    Ok(format!("--command=add {} {}", quote(source)?, quote(destination)?).into())
}
