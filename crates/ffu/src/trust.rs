use std::{path::Path, sync::atomic::AtomicBool};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    Cancel, Error, MAX_MANIFEST_BYTES, NEVER_CANCELLED, ParsedFfu, Result,
    hdmi::{configured_blocks, hdmi_options},
    parser::{parse, resolve_plan},
    source::Source,
};

pub const PROVENANCE_SHA256: &str =
    "74e9324c77e3e560a34742de1cd13e6c234789bf5089a75ba334d80c098ca41a";
pub(crate) const PROVENANCE: &[u8] = include_bytes!("../../../src/config/iot-core-images.json");

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ApprovedImage {
    pub sha256: String,
    pub format: String,
    pub profile: String,
    pub architecture: String,
    pub build: String,
    pub os_version: String,
    pub platform_ids: Vec<String>,
    pub recommended_boards: Vec<String>,
    #[serde(rename = "ffu_bytes")]
    pub file_bytes: u64,
    pub payload_bytes: u64,
    pub minimum_disk_bytes: u64,
    pub sector_size: u64,
    pub chunk_size: u64,
    pub source_url: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct ImageProfile {
    pub architecture: String,
    pub profile: String,
    pub format: String,
    pub expected_build: String,
    pub expected_sha256: String,
    pub minimum_disk_bytes: u64,
    pub recommended_boards: Vec<String>,
    pub source_url: String,
    pub source_verification: &'static str,
    pub trust: &'static str,
}

#[derive(Clone, Debug, Serialize)]
pub struct InspectionHdmi {
    pub mode: String,
    pub label: String,
    pub configured_blocks: usize,
}

#[derive(Clone, Debug, Serialize)]
pub struct Inspection {
    pub format: String,
    pub architecture: String,
    pub profile: String,
    pub build: String,
    pub os_version: String,
    pub platform_ids: Vec<String>,
    pub recommended_boards: Vec<String>,
    pub minimum_disk_bytes: u64,
    pub payload_bytes: u64,
    pub file_bytes: u64,
    pub chunk_size: u64,
    pub sector_size: u64,
    pub sha256: String,
    pub source_url: String,
    pub trust: &'static str,
    pub hardware_tested: bool,
    pub hdmi: InspectionHdmi,
}

/// The compiled manifest is authenticated on every use; there is no override path.
pub fn load_provenance() -> Result<serde_json::Value> {
    decode_provenance(PROVENANCE)
}

fn decode_provenance(raw: &[u8]) -> Result<serde_json::Value> {
    if raw.len() > MAX_MANIFEST_BYTES || format!("{:x}", Sha256::digest(raw)) != PROVENANCE_SHA256 {
        return Err(Error::Source(
            "bundled IoT provenance manifest is missing, modified, or untrusted".into(),
        ));
    }
    let manifest: serde_json::Value = serde_json::from_slice(raw)
        .map_err(|error| Error::Source(format!("invalid bundled provenance: {error}")))?;
    if manifest
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
        != Some(1)
    {
        return Err(Error::Source(
            "unsupported bundled provenance schema".into(),
        ));
    }
    Ok(manifest)
}

pub(crate) fn approved_images() -> Result<Vec<ApprovedImage>> {
    let manifest = load_provenance()?;
    serde_json::from_value(manifest.get("images").cloned().unwrap_or_default())
        .map_err(|error| Error::Source(format!("invalid bundled image profile: {error}")))
}

pub fn image_profile(board: &str) -> Result<ImageProfile> {
    let matches: Vec<_> = approved_images()?
        .into_iter()
        .filter(|image| image.recommended_boards.iter().any(|item| item == board))
        .collect();
    if matches.len() != 1 {
        return Err(Error::Source(
            "board must identify exactly one reviewed Raspberry Pi IoT Core image".into(),
        ));
    }
    let approved = &matches[0];
    if approved.architecture != "arm32"
        || approved.profile != "iot-core"
        || approved.format != "ffu-v1"
        || approved.sector_size != 512
        || approved.minimum_disk_bytes == 0
    {
        return Err(Error::Source(
            "the reviewed image profile is incompatible with this IoT Core workflow".into(),
        ));
    }
    Ok(ImageProfile {
        architecture: approved.architecture.clone(),
        profile: approved.profile.clone(),
        format: approved.format.clone(),
        expected_build: approved.build.clone(),
        expected_sha256: approved.sha256.clone(),
        minimum_disk_bytes: approved.minimum_disk_bytes,
        recommended_boards: approved.recommended_boards.clone(),
        source_url: approved.source_url.clone(),
        source_verification: "not-assessed",
        trust: "pinned-bundled-profile",
    })
}

pub(crate) fn check_approval(info: &ParsedFfu, approved: &ApprovedImage) -> Result<()> {
    if approved.architecture != "arm32"
        || approved.profile != "iot-core"
        || approved.format != "ffu-v1"
    {
        return Err(Error::Source(
            "only approved ARM32 Windows 10 IoT Core V1 images are supported".into(),
        ));
    }
    if info.sha256 != approved.sha256 {
        return Err(Error::Source("FFU SHA256 is not approved".into()));
    }
    if info.os_version != approved.os_version
        || info.os_version != format!("10.0.{}", approved.build)
        || info.platform_ids != approved.platform_ids
        || !info
            .platform_ids
            .iter()
            .any(|platform| platform == "Broadcom.RPi.2")
        || approved.recommended_boards.is_empty()
        || approved
            .recommended_boards
            .iter()
            .any(|board| !["pi2-v1.1", "pi2-v1.2", "pi3-b"].contains(&board.as_str()))
        || (
            info.file_bytes,
            info.payload_bytes,
            info.minimum_disk_bytes,
            info.sector_size,
            info.chunk_size,
        ) != (
            approved.file_bytes,
            approved.payload_bytes,
            approved.minimum_disk_bytes,
            approved.sector_size,
            approved.chunk_size,
        )
    {
        return Err(Error::Source(
            "FFU identity/geometry does not match the pinned Raspberry Pi image".into(),
        ));
    }
    Ok(())
}

pub(crate) fn approve(info: &ParsedFfu) -> Result<ApprovedImage> {
    for approved in approved_images()? {
        if approved.sha256 == info.sha256 {
            check_approval(info, &approved)?;
            return Ok(approved);
        }
    }
    Err(Error::Source(
        "FFU SHA256 is not allowlisted; custom, ARM64, non-IoT and other images are unsupported"
            .into(),
    ))
}

pub(crate) fn inspection(
    info: &ParsedFfu,
    approved: &ApprovedImage,
    hdmi: InspectionHdmi,
) -> Inspection {
    Inspection {
        format: approved.format.clone(),
        architecture: approved.architecture.clone(),
        profile: approved.profile.clone(),
        build: approved.build.clone(),
        os_version: info.os_version.clone(),
        platform_ids: info.platform_ids.clone(),
        recommended_boards: approved.recommended_boards.clone(),
        minimum_disk_bytes: info.minimum_disk_bytes,
        payload_bytes: info.payload_bytes,
        file_bytes: info.file_bytes,
        chunk_size: info.chunk_size,
        sector_size: info.sector_size,
        sha256: info.sha256.clone(),
        source_url: approved.source_url.clone(),
        trust: "pinned-official-sha256",
        hardware_tested: false,
        hdmi,
    }
}

pub fn inspect_ffu(path: &Path, hdmi_mode: &str, hdmi_config: &str) -> Result<Inspection> {
    inspect_ffu_with_cancel(path, hdmi_mode, hdmi_config, &NEVER_CANCELLED)
}

pub fn inspect_ffu_with_cancel(
    path: &Path,
    hdmi_mode: &str,
    hdmi_config: &str,
    cancellation: &AtomicBool,
) -> Result<Inspection> {
    inspect(path, hdmi_mode, hdmi_config, Cancel::new(cancellation))
}

pub(crate) fn inspect(
    path: &Path,
    mode: &str,
    custom: &str,
    cancel: Cancel<'_>,
) -> Result<Inspection> {
    cancel.check()?;
    let mut source = Source::open(path)?;
    let info = parse(&mut source.file, cancel)?;
    let approved = approve(&info)?;
    let preferences = hdmi_options(mode, custom)?;
    let blocks = configured_blocks(
        &mut source,
        &info,
        &resolve_plan(&info, info.minimum_disk_bytes)?,
        mode,
        custom,
        cancel,
    )?;
    source.check(cancel)?;
    let result = inspection(
        &info,
        &approved,
        InspectionHdmi {
            mode: mode.into(),
            label: preferences.label,
            configured_blocks: blocks.len(),
        },
    );
    source.close()?;
    Ok(result)
}

#[cfg(test)]
pub(crate) fn tampered_provenance(raw: &[u8]) -> Result<serde_json::Value> {
    decode_provenance(raw)
}
