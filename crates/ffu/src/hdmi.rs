use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use crate::{
    Cancel, Error, MAX_BOOT_CONFIG_BYTES, MAX_HDMI_CONFIG_BYTES, ParsedFfu, Result, SECTOR_SIZE,
    WritePlan, source::Source, u16_at, u32_at,
};

const PRESETS: [(&str, &str, &str); 4] = [
    ("official", "Official image default", ""),
    (
        "720p60",
        "1280 x 720 / 60 Hz (compatibility)",
        "hdmi_force_hotplug=1\nhdmi_group=1\nhdmi_mode=4\n",
    ),
    (
        "1080p60",
        "1920 x 1080 / 60 Hz",
        "hdmi_force_hotplug=1\nhdmi_group=1\nhdmi_mode=16\n",
    ),
    ("custom", "Custom video settings", ""),
];

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct HdmiChoice {
    pub value: String,
    pub label: String,
    pub settings: String,
    pub recommended: bool,
    pub menu_label: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct HdmiOptions {
    pub mode: String,
    pub label: String,
    pub settings: String,
    pub recommended: bool,
    pub custom: String,
    pub choices: Vec<HdmiChoice>,
}

/// Validate display-only choices without opening either an image or a target.
pub fn hdmi_options(mode: &str, custom: &str) -> Result<HdmiOptions> {
    let (_, label, settings) = PRESETS
        .iter()
        .find(|(value, _, _)| *value == mode)
        .ok_or_else(|| {
            Error::Source("unknown HDMI mode; choose official, 720p60, 1080p60 or custom".into())
        })?;
    if custom.len() > MAX_HDMI_CONFIG_BYTES
        || !custom.is_ascii()
        || custom
            .bytes()
            .any(|byte| byte < 32 && !b"\t\r\n".contains(&byte))
    {
        return Err(Error::Source(
            "custom HDMI settings must be at most 512 ASCII characters".into(),
        ));
    }
    let (mut label, mut settings) = (label.to_string(), settings.to_string());
    if mode == "custom" {
        let mut values: Vec<(&str, u64)> = Vec::new();
        for line in custom.split(['\r', '\n']) {
            let line = line.split('#').next().unwrap_or_default().trim();
            if line.is_empty() {
                continue;
            }
            let bad = || {
                Error::Source(
                "custom HDMI settings accept only hdmi_force_hotplug, hdmi_group, hdmi_mode and hdmi_drive".into(),
            )
            };
            let (key, value) = line.split_once('=').ok_or_else(bad)?;
            let (key, value) = (key.trim_end(), value.trim_start());
            if ![
                "hdmi_force_hotplug",
                "hdmi_group",
                "hdmi_mode",
                "hdmi_drive",
            ]
            .contains(&key)
                || value.is_empty()
                || !value.bytes().all(|byte| byte.is_ascii_digit())
            {
                return Err(bad());
            }
            if values.iter().any(|(existing, _)| *existing == key) {
                return Err(Error::Source(format!("duplicate HDMI setting: {key}")));
            }
            // Arbitrarily long decimal strings are legal syntax, but never in-range.
            let value = value.parse().unwrap_or(u64::MAX);
            values.push((key, value));
        }
        let value = |name: &str| {
            values
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| *value)
        };
        let group = value("hdmi_group");
        let timing = value("hdmi_mode");
        if !matches!(group, Some(1 | 2)) || timing.is_none() {
            return Err(Error::Source(
                "custom HDMI settings require hdmi_group=1 or 2 and hdmi_mode".into(),
            ));
        }
        let maximum = if group == Some(1) { 64 } else { 86 };
        if !(1..=maximum).contains(&timing.expect("checked")) {
            return Err(Error::Source("HDMI mode is outside the legacy Pi 2/3 CEA/DMT range; custom modelines are not supported".into()));
        }
        if value("hdmi_force_hotplug").is_some_and(|value| value > 1) {
            return Err(Error::Source("hdmi_force_hotplug must be 0 or 1".into()));
        }
        if value("hdmi_drive").is_some_and(|value| !(1..=2).contains(&value)) {
            return Err(Error::Source(
                "hdmi_drive must be 1 (DVI) or 2 (HDMI)".into(),
            ));
        }
        settings = values
            .iter()
            .map(|(key, value)| format!("{key}={value}\n"))
            .collect();
        label = format!(
            "Custom HDMI (group {}, mode {})",
            group.expect("checked"),
            timing.expect("checked")
        );
    }
    Ok(HdmiOptions {
        mode: mode.into(),
        label,
        settings,
        recommended: mode == "720p60",
        custom: if custom.is_empty() {
            PRESETS[1].2.into()
        } else {
            custom.into()
        },
        choices: PRESETS
            .iter()
            .map(|(value, label, settings)| HdmiChoice {
                value: (*value).into(),
                label: (*label).into(),
                settings: (*settings).into(),
                recommended: *value == "720p60",
                menu_label: format!(
                    "{label}{}",
                    if *value == "720p60" {
                        " (Recommended)"
                    } else {
                        ""
                    }
                ),
            })
            .collect(),
    })
}

#[derive(Clone, Debug)]
pub(crate) struct ConfiguredBlock {
    pub offset: u64,
    pub payload_block: u64,
    pub data: Vec<u8>,
}

fn image_bytes(
    source: &mut Source,
    info: &ParsedFfu,
    plan: &WritePlan,
    mut offset: u64,
    mut size: usize,
    cancel: Cancel<'_>,
) -> Result<Vec<u8>> {
    if size > 512 * 1024 {
        return Err(Error::Source(
            "boot configuration read exceeds its bounded range".into(),
        ));
    }
    let mut result = Vec::with_capacity(size);
    while size > 0 {
        cancel.check()?;
        let extent = plan
            .final_extents
            .iter()
            .find(|item| {
                item.offset <= offset
                    && offset < item.offset + item.block_count as u64 * info.chunk_size
            })
            .ok_or_else(|| {
                Error::Source(
                    "boot configuration references bytes not supplied by the verified FFU".into(),
                )
            })?;
        let index = (offset - extent.offset) / info.chunk_size;
        let within = ((offset - extent.offset) % info.chunk_size) as usize;
        let take = size.min(info.chunk_size as usize - within);
        let data = source.payload(info, extent.payload_block + index, cancel)?;
        result.extend_from_slice(&data[within..within + take]);
        offset += take as u64;
        size -= take;
    }
    Ok(result)
}

fn lines_with_ends(text: &str) -> Vec<&str> {
    let bytes = text.as_bytes();
    let (mut start, mut offset) = (0, 0);
    let mut result = Vec::new();
    while offset < bytes.len() {
        if b"\r\n\x0b\x0c\x1c\x1d\x1e".contains(&bytes[offset]) {
            offset += if bytes[offset] == b'\r' && bytes.get(offset + 1) == Some(&b'\n') {
                2
            } else {
                1
            };
            result.push(&text[start..offset]);
            start = offset;
        } else {
            offset += 1;
        }
    }
    if start < text.len() {
        result.push(&text[start..]);
    }
    result
}

fn assignment(line: &str) -> Option<&str> {
    let line = line.trim_start();
    let (key, _) = line.split_once('=')?;
    let key = key.trim_end();
    if key.is_empty()
        || !key.as_bytes()[0].is_ascii_lowercase()
        || !key
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    {
        None
    } else {
        Some(key)
    }
}

pub(crate) fn configured_blocks(
    source: &mut Source,
    info: &ParsedFfu,
    plan: &WritePlan,
    mode: &str,
    custom: &str,
    cancel: Cancel<'_>,
) -> Result<Vec<ConfiguredBlock>> {
    let preferences = hdmi_options(mode, custom)?;
    if mode == "official" {
        return Ok(Vec::new());
    }
    let settings = preferences.settings;
    let mbr = image_bytes(source, info, plan, 0, SECTOR_SIZE as usize, cancel)?;
    let start_sector = u32_at(&mbr, 454) as u64;
    let partition_sectors = u32_at(&mbr, 458) as u64;
    if &mbr[510..512] != b"\x55\xaa"
        || ![0x06, 0x0e, 0x0c].contains(&mbr[450])
        || start_sector == 0
        || partition_sectors == 0
        || (start_sector + partition_sectors) * SECTOR_SIZE > info.minimum_disk_bytes
    {
        return Err(Error::Source(
            "HDMI customization requires the image's existing first FAT boot partition".into(),
        ));
    }
    let start = start_sector * SECTOR_SIZE;
    let boot = image_bytes(source, info, plan, start, SECTOR_SIZE as usize, cancel)?;
    let sector = u16_at(&boot, 11) as u64;
    let sectors_per_cluster = boot[13] as u64;
    let reserved = u16_at(&boot, 14) as u64;
    let roots = u16_at(&boot, 17) as u64;
    let fat_sectors = u16_at(&boot, 22) as u64;
    let total_sectors = match u16_at(&boot, 19) {
        0 => u32_at(&boot, 32) as u64,
        value => value as u64,
    };
    if &boot[510..512] != b"\x55\xaa"
        || &boot[54..62] != b"FAT16   "
        || sector != SECTOR_SIZE
        || ![1, 2, 4, 8, 16, 32, 64, 128].contains(&sectors_per_cluster)
        || boot[16] != 2
        || !(1..=128).contains(&reserved)
        || !(1..=4096).contains(&roots)
        || !(1..=1024).contains(&fat_sectors)
        || total_sectors != partition_sectors
    {
        return Err(Error::Source(
            "HDMI customization requires a consistent bounded FAT16 boot filesystem".into(),
        ));
    }
    let root_sectors = (roots * 32).div_ceil(sector);
    let data_sector = reserved + 2 * fat_sectors + root_sectors;
    let clusters = total_sectors
        .checked_sub(data_sector)
        .map(|value| value / sectors_per_cluster);
    let clusters = clusters
        .filter(|value| (4085..65525).contains(value) && (value + 2) * 2 <= fat_sectors * sector)
        .ok_or_else(|| {
            Error::Source("invalid FAT16 allocation bounds for HDMI customization".into())
        })?;
    let fat_start = start + reserved * sector;
    let fat = image_bytes(
        source,
        info,
        plan,
        fat_start,
        (fat_sectors * sector) as usize,
        cancel,
    )?;
    if fat
        != image_bytes(
            source,
            info,
            plan,
            fat_start + fat_sectors * sector,
            (fat_sectors * sector) as usize,
            cancel,
        )?
    {
        return Err(Error::Source(
            "boot FAT copies disagree; HDMI customization was not applied".into(),
        ));
    }
    let root_start = start + (reserved + 2 * fat_sectors) * sector;
    let directory = image_bytes(
        source,
        info,
        plan,
        root_start,
        (roots * 32) as usize,
        cancel,
    )?;
    let mut entries = Vec::new();
    for (index, entry) in directory.as_chunks::<32>().0.iter().enumerate() {
        if entry[0] == 0 {
            break;
        }
        if entry[0] == 0xe5 || entry[11] == 0x0f {
            continue;
        }
        if entry[..11].eq_ignore_ascii_case(b"CONFIG  TXT") {
            entries.push((index * 32, entry));
        }
    }
    if entries.len() != 1 || entries[0].1[11] & 0x18 != 0 {
        return Err(Error::Source(
            "boot config.txt must be one regular file in the FAT16 root".into(),
        ));
    }
    let (entry_position, entry) = entries[0];
    let file_size = u32_at(entry, 28) as usize;
    let mut cluster = u16_at(entry, 26) as u64;
    if file_size == 0 || file_size > MAX_BOOT_CONFIG_BYTES || u16_at(entry, 20) != 0 {
        return Err(Error::Source(
            "boot config.txt size or first cluster is invalid".into(),
        ));
    }
    let mut chain = Vec::new();
    while cluster < 0xfff8 {
        if !(2..clusters + 2).contains(&cluster) || chain.contains(&cluster) || chain.len() >= 16 {
            return Err(Error::Source(
                "boot config.txt has an invalid or excessive FAT chain".into(),
            ));
        }
        chain.push(cluster);
        cluster = u16_at(&fat, (cluster * 2) as usize) as u64;
    }
    let cluster_size = (sectors_per_cluster * sector) as usize;
    let capacity = chain.len() * cluster_size;
    if chain.is_empty() || (chain.len() - 1) * cluster_size >= file_size || file_size > capacity {
        return Err(Error::Source(
            "boot config.txt allocation does not match its size".into(),
        ));
    }
    let offsets: Vec<_> = chain
        .iter()
        .map(|item| start + data_sector * sector + (item - 2) * cluster_size as u64)
        .collect();
    let mut original = Vec::with_capacity(capacity);
    for offset in &offsets {
        original.extend(image_bytes(
            source,
            info,
            plan,
            *offset,
            cluster_size,
            cancel,
        )?);
    }
    original.truncate(file_size);
    if !original.is_ascii() {
        return Err(Error::Source(
            "boot config.txt is not an ASCII firmware configuration".into(),
        ));
    }
    let text = std::str::from_utf8(&original).expect("ASCII");
    let lines = lines_with_ends(text);
    if text.contains('\0') || lines.iter().any(|line| line.trim().starts_with('[')) {
        return Err(Error::Source(
            "filtered or binary boot configurations are not supported for HDMI customization"
                .into(),
        ));
    }
    let keys: BTreeSet<_> = settings
        .lines()
        .filter_map(|line| line.split_once('=').map(|(key, _)| key))
        .collect();
    let retained: String = lines
        .into_iter()
        .filter(|line| assignment(line).is_none_or(|key| !keys.contains(key)))
        .collect();
    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let prefix = retained.trim_end_matches(['\r', '\n']);
    let replacement = format!(
        "{prefix}{newline}# WoR-Flasher HDMI timing override{newline}{}",
        settings.replace('\n', newline)
    );
    if replacement.len() > capacity || replacement.len() > MAX_BOOT_CONFIG_BYTES {
        return Err(Error::Source("HDMI settings exceed the existing config.txt allocation; no filesystem resize is attempted".into()));
    }
    let mut padded = replacement.into_bytes();
    let replacement_size = padded.len() as u32;
    padded.resize(capacity, 0);
    let mut changes: Vec<_> = offsets
        .into_iter()
        .enumerate()
        .map(|(index, offset)| {
            (
                offset,
                padded[index * cluster_size..(index + 1) * cluster_size].to_vec(),
            )
        })
        .collect();
    changes.push((
        root_start + entry_position as u64 + 28,
        replacement_size.to_le_bytes().to_vec(),
    ));
    let mut changed: BTreeMap<u64, ConfiguredBlock> = BTreeMap::new();
    for (mut offset, data) in changes {
        let mut position = 0;
        while position < data.len() {
            cancel.check()?;
            let block_offset = offset / info.chunk_size * info.chunk_size;
            let within = (offset - block_offset) as usize;
            let take = (data.len() - position).min(info.chunk_size as usize - within);
            if let std::collections::btree_map::Entry::Vacant(entry) = changed.entry(block_offset) {
                let extent = plan
                    .final_extents
                    .iter()
                    .find(|item| {
                        item.offset <= block_offset
                            && block_offset
                                < item.offset + item.block_count as u64 * info.chunk_size
                    })
                    .ok_or_else(|| {
                        Error::Source(
                            "configured boot block is not supplied by the verified FFU".into(),
                        )
                    })?;
                let payload =
                    extent.payload_block + (block_offset - extent.offset) / info.chunk_size;
                entry.insert(ConfiguredBlock {
                    offset: block_offset,
                    payload_block: payload,
                    data: source.payload(info, payload, cancel)?,
                });
            }
            changed.get_mut(&block_offset).expect("inserted block").data[within..within + take]
                .copy_from_slice(&data[position..position + take]);
            offset += take as u64;
            position += take;
        }
    }
    Ok(changed.into_values().collect())
}
