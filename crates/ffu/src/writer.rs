use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io,
    path::Path,
    sync::atomic::AtomicBool,
};

use crate::{
    Cancel, Error, MAX_GPT_TABLE_BYTES, ParsedFfu, Result, SECTOR_SIZE, WIPE_CHUNK_BYTES,
    WritePlan, align,
    hdmi::{ConfiguredBlock, configured_blocks, hdmi_options},
    parser::{parse, resolve_plan},
    source::{Kind, Source, SourceStamp, TargetIdentity, open_path, valid_identity},
    trust::approve,
    u32_at, u64_at,
    unix::{self, Host, Metadata, NativeIoctl},
};

pub type Progress<'a> = dyn FnMut(&str, u8) + 'a;

#[derive(Clone, Debug)]
pub struct ApplyOptions {
    pub target_size: u64,
    pub expected_sha256: Option<String>,
    pub expected_target_id: Option<String>,
    pub allow_gpt_cleanup: bool,
    pub hdmi_mode: String,
    pub hdmi_config: String,
    pub wipe_entire_drive: bool,
}

impl Default for ApplyOptions {
    fn default() -> Self {
        Self {
            target_size: 0,
            expected_sha256: None,
            expected_target_id: None,
            allow_gpt_cleanup: false,
            hdmi_mode: "official".into(),
            hdmi_config: String::new(),
            wipe_entire_drive: false,
        }
    }
}

pub(crate) trait TargetIo {
    fn close(&self, file: File) -> Result<()> {
        unix::close(file)
    }
    fn metadata(&self, file: &File) -> Result<Metadata> {
        unix::metadata(file)
    }
    fn geometry(&self, file: &File) -> Result<(u64, u64)> {
        unix::geometry_with(file, unix::host(), &NativeIoctl)
    }
    fn read(&self, file: &File, buffer: &mut [u8], offset: u64) -> io::Result<usize> {
        unix::read_at(file, buffer, offset)
    }
    fn write(&self, file: &File, buffer: &[u8], offset: u64) -> io::Result<usize> {
        unix::write_at(file, buffer, offset)
    }
    fn sync(&self, file: &File, fixture: bool, before_readback: bool) -> Result<()> {
        file.sync_all()
            .map_err(|error| Error::io("fsync target", error))?;
        unix::cache_sync_with(file, unix::host(), fixture, before_readback, &NativeIoctl)
    }
}

pub(crate) struct NativeTargetIo;
impl TargetIo for NativeTargetIo {}

pub(crate) struct Target<'a> {
    pub file: File,
    pub size: u64,
    pub sector: u64,
    pub fixture: bool,
    pub identity: TargetIdentity,
    pub io: &'a dyn TargetIo,
    pub cancel: Cancel<'a>,
}

impl Target<'_> {
    pub fn check(&self) -> Result<()> {
        self.cancel.check()?;
        let metadata = self.io.metadata(&self.file)?;
        if TargetIdentity::from(&metadata) != self.identity {
            return Err(Error::Target(
                "target identity changed; stop using this target".into(),
            ));
        }
        let (actual, sector) = if self.fixture {
            if !metadata.regular() || metadata.links != 1 || metadata.size < 0 {
                return Err(Error::Target("fixture target identity changed".into()));
            }
            (metadata.size as u64, SECTOR_SIZE)
        } else {
            self.io.geometry(&self.file)?
        };
        if actual != self.size || sector != self.sector {
            return Err(Error::Target(
                "actual target size/sector size does not match validated geometry".into(),
            ));
        }
        Ok(())
    }

    pub fn stamp(&self) -> Result<SourceStamp> {
        Ok(SourceStamp::from(&self.io.metadata(&self.file)?))
    }

    fn bounds(&self, size: usize, offset: u64) -> Result<()> {
        if offset > self.size
            || size as u64 > self.size - offset
            || !offset.is_multiple_of(self.sector)
            || !(size as u64).is_multiple_of(self.sector)
        {
            return Err(Error::Apply("unaligned or out-of-bounds target I/O".into()));
        }
        Ok(())
    }

    pub fn read(&self, size: usize, offset: u64) -> Result<Vec<u8>> {
        self.bounds(size, offset)?;
        let mut data = vec![0; size];
        let mut done = 0;
        while done < size {
            self.cancel.check()?;
            let count = match self
                .io
                .read(&self.file, &mut data[done..], offset + done as u64)
            {
                Ok(count) => count,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {
                    self.cancel.check()?;
                    continue;
                }
                Err(error) => return Err(Error::io("read back target", error)),
            };
            if count == 0 || count > size - done || !(count as u64).is_multiple_of(self.sector) {
                return Err(Error::Apply(
                    "short or unaligned target readback; target may be incomplete".into(),
                ));
            }
            done += count;
        }
        self.cancel.check()?;
        Ok(data)
    }

    pub fn write(&self, data: &[u8], offset: u64) -> Result<()> {
        self.bounds(data.len(), offset)?;
        let mut done = 0;
        while done < data.len() {
            self.cancel.check()?;
            let count = match self
                .io
                .write(&self.file, &data[done..], offset + done as u64)
            {
                Ok(count) => count,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {
                    self.cancel.check()?;
                    continue;
                }
                Err(error) => return Err(Error::io("write target", error)),
            };
            if count == 0
                || count > data.len() - done
                || !(count as u64).is_multiple_of(self.sector)
            {
                return Err(Error::Apply(
                    "short or unaligned target write; target may be incomplete".into(),
                ));
            }
            done += count;
        }
        self.cancel.check()?;
        Ok(())
    }

    pub fn sync(&self, before_readback: bool) -> Result<()> {
        self.cancel.check()?;
        let result = self.io.sync(&self.file, self.fixture, before_readback);
        self.cancel.check()?;
        result
    }
}

fn report(
    progress: &mut Option<&mut Progress<'_>>,
    phase: &str,
    percent: u8,
    cancel: Cancel<'_>,
) -> Result<()> {
    cancel.check()?;
    if let Some(progress) = progress {
        progress(phase, percent);
    }
    cancel.check()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct GptHeader {
    current_lba: u64,
    other_lba: u64,
    first_usable: u64,
    last_usable: u64,
    disk_guid: [u8; 16],
    entries_lba: u64,
    entry_count: u32,
    entry_size: u32,
    table_crc: u32,
}

#[derive(Clone, Debug)]
pub(crate) struct MetadataRegion {
    pub offset: u64,
    pub original: Vec<u8>,
}

pub(crate) fn gpt_header(data: &[u8], lba: u64, last_lba: u64) -> Result<GptHeader> {
    if data.len() != SECTOR_SIZE as usize || &data[..8] != b"EFI PART" {
        return Err(Error::Target(
            "GPT cleanup requires both valid primary and backup headers; no metadata was erased"
                .into(),
        ));
    }
    let size = u32_at(data, 12) as usize;
    if u32_at(data, 8) != 0x10000 || !(92..=512).contains(&size) || u32_at(data, 20) != 0 {
        return Err(Error::Target(
            "unsupported GPT header geometry; no metadata was erased".into(),
        ));
    }
    let mut checked = data[..size].to_vec();
    checked[16..20].fill(0);
    if crc32fast::hash(&checked) != u32_at(data, 16) {
        return Err(Error::Target(
            "GPT header checksum is invalid; no metadata was erased".into(),
        ));
    }
    let (current, other, first, last) = (
        u64_at(data, 24),
        u64_at(data, 32),
        u64_at(data, 40),
        u64_at(data, 48),
    );
    let (entries, count, entry_size, table_crc) = (
        u64_at(data, 72),
        u32_at(data, 80),
        u32_at(data, 84),
        u32_at(data, 88),
    );
    let expected_other = if lba == 1 { last_lba } else { 1 };
    if current != lba || other != expected_other || !(2 < first && first <= last && last < last_lba)
    {
        return Err(Error::Target(
            "GPT header bounds do not match the confirmed disk; no metadata was erased".into(),
        ));
    }
    let table_bytes = count as u64 * entry_size as u64;
    if count == 0
        || entry_size < 128
        || !entry_size.is_multiple_of(128)
        || table_bytes > MAX_GPT_TABLE_BYTES
    {
        return Err(Error::Target(
            "GPT partition table exceeds supported cleanup bounds; no metadata was erased".into(),
        ));
    }
    let sectors = table_bytes.div_ceil(SECTOR_SIZE);
    let table_end = entries.checked_add(sectors);
    let valid = if lba == 1 {
        entries >= 2 && table_end.is_some_and(|end| end <= first)
    } else {
        last < entries && table_end.is_some_and(|end| end <= last_lba)
    };
    if !valid {
        return Err(Error::Target(
            "GPT table overlaps usable data or a header; no metadata was erased".into(),
        ));
    }
    Ok(GptHeader {
        current_lba: current,
        other_lba: other,
        first_usable: first,
        last_usable: last,
        disk_guid: data[56..72].try_into().expect("bounded GUID"),
        entries_lba: entries,
        entry_count: count,
        entry_size,
        table_crc,
    })
}

fn probe_positions(info: &ParsedFfu, size: u64) -> BTreeSet<u64> {
    [
        SECTOR_SIZE,
        info.minimum_disk_bytes - SECTOR_SIZE,
        size - SECTOR_SIZE,
    ]
    .into()
}

fn reject_existing_gpt(target: &Target<'_>, info: &ParsedFfu) -> Result<()> {
    for offset in probe_positions(info, target.size) {
        if &target.read(info.sector_size as usize, offset)?[..8] == b"EFI PART" {
            return Err(Error::Target(
                "existing GPT target is unsupported by this fixed MBR/EBR IoT image; use a deliberately prepared blank/MBR target, not automatic GPT conversion".into(),
            ));
        }
    }
    Ok(())
}

pub(crate) fn gpt_cleanup_plan(
    target: &Target<'_>,
    info: &ParsedFfu,
) -> Result<Vec<MetadataRegion>> {
    let last_lba = target.size / SECTOR_SIZE - 1;
    let mut probes = BTreeMap::new();
    for offset in probe_positions(info, target.size) {
        probes.insert(offset, target.read(SECTOR_SIZE as usize, offset)?);
    }
    let signatures: BTreeSet<_> = probes
        .iter()
        .filter(|(_, data)| &data[..8] == b"EFI PART")
        .map(|(offset, _)| *offset)
        .collect();
    if signatures.is_empty() {
        return Ok(Vec::new());
    }
    let expected = BTreeSet::from([SECTOR_SIZE, target.size - SECTOR_SIZE]);
    if signatures != expected {
        return Err(Error::Target(
            "orphaned or unexpectedly located GPT metadata; no metadata was erased".into(),
        ));
    }
    let primary = gpt_header(&probes[&SECTOR_SIZE], 1, last_lba)?;
    let backup = gpt_header(&probes[&(target.size - SECTOR_SIZE)], last_lba, last_lba)?;
    if (
        primary.first_usable,
        primary.last_usable,
        primary.disk_guid,
        primary.entry_count,
        primary.entry_size,
        primary.table_crc,
    ) != (
        backup.first_usable,
        backup.last_usable,
        backup.disk_guid,
        backup.entry_count,
        backup.entry_size,
        backup.table_crc,
    ) {
        return Err(Error::Target(
            "GPT copies disagree; no metadata was erased".into(),
        ));
    }
    let table_bytes = primary.entry_count as u64 * primary.entry_size as u64;
    let table_span = align(table_bytes, SECTOR_SIZE) as usize;
    let primary_table = target.read(table_span, primary.entries_lba * SECTOR_SIZE)?;
    let backup_table = target.read(table_span, backup.entries_lba * SECTOR_SIZE)?;
    let length = table_bytes as usize;
    if crc32fast::hash(&primary_table[..length]) != primary.table_crc
        || crc32fast::hash(&backup_table[..length]) != backup.table_crc
        || primary_table[..length] != backup_table[..length]
    {
        return Err(Error::Target(
            "GPT partition-table checksum or copy mismatch; no metadata was erased".into(),
        ));
    }
    let mut partitions = Vec::new();
    for entry in primary_table[..length].chunks_exact(primary.entry_size as usize) {
        target.cancel.check()?;
        if entry[..16].iter().all(|byte| *byte == 0) {
            continue;
        }
        let (start, end) = (u64_at(entry, 32), u64_at(entry, 40));
        if !(primary.first_usable <= start && start <= end && end <= primary.last_usable) {
            return Err(Error::Target(
                "GPT partition lies outside usable bounds; no metadata was erased".into(),
            ));
        }
        partitions.push((start, end));
    }
    partitions.sort_unstable();
    if partitions.windows(2).any(|pair| pair[1].0 <= pair[0].1) {
        return Err(Error::Target(
            "GPT partitions overlap; no metadata was erased".into(),
        ));
    }
    let mut regions = vec![
        MetadataRegion {
            offset: 0,
            original: target.read(SECTOR_SIZE as usize, 0)?,
        },
        MetadataRegion {
            offset: SECTOR_SIZE,
            original: probes[&SECTOR_SIZE].clone(),
        },
        MetadataRegion {
            offset: primary.entries_lba * SECTOR_SIZE,
            original: primary_table,
        },
        MetadataRegion {
            offset: backup.entries_lba * SECTOR_SIZE,
            original: backup_table,
        },
        MetadataRegion {
            offset: target.size - SECTOR_SIZE,
            original: probes[&(target.size - SECTOR_SIZE)].clone(),
        },
    ];
    regions.sort_by_key(|region| region.offset);
    let mut previous_end = 0;
    for region in &regions {
        if region.offset < previous_end
            || region.offset + region.original.len() as u64 > target.size
        {
            return Err(Error::Target(
                "GPT cleanup regions overlap or exceed the target; no metadata was erased".into(),
            ));
        }
        previous_end = region.offset + region.original.len() as u64;
    }
    Ok(regions)
}

pub(crate) fn clear_gpt_metadata(
    source: &Source,
    target: &Target<'_>,
    regions: &[MetadataRegion],
    progress: &mut Option<&mut Progress<'_>>,
) -> Result<()> {
    source.check(target.cancel)?;
    target.check()?;
    for region in regions {
        if target.read(region.original.len(), region.offset)? != region.original {
            return Err(Error::Target(
                "GPT metadata changed during preflight; no metadata was erased".into(),
            ));
        }
    }
    report(progress, "Preparing GPT metadata", 0, target.cancel)?;
    for (index, region) in regions.iter().enumerate() {
        source.check(target.cancel)?;
        target.check()?;
        target.write(&vec![0; region.original.len()], region.offset)?;
        report(
            progress,
            "Preparing GPT metadata",
            (((index + 1) * 100 / regions.len()).min(99)) as u8,
            target.cancel,
        )?;
    }
    target.sync(true)?;
    for region in regions {
        if target
            .read(region.original.len(), region.offset)?
            .iter()
            .any(|byte| *byte != 0)
        {
            return Err(Error::Apply(
                "GPT cleanup readback failed; target may be incomplete, do not boot it".into(),
            ));
        }
    }
    source.check(target.cancel)?;
    target.check()?;
    report(progress, "Preparing GPT metadata", 100, target.cancel)
}

fn verify_zero_range(target: &Target<'_>, offset: u64, end: u64) -> Result<()> {
    if end > offset
        && target
            .read((end - offset) as usize, offset)?
            .iter()
            .any(|byte| *byte != 0)
    {
        return Err(Error::Apply(
            "old GPT metadata reappeared during readback; do not boot this target".into(),
        ));
    }
    Ok(())
}

pub(crate) fn verify_gpt_cleanup(
    target: &Target<'_>,
    info: &ParsedFfu,
    plan: &WritePlan,
    regions: &[MetadataRegion],
) -> Result<()> {
    for region in regions {
        let mut position = region.offset;
        let end = position + region.original.len() as u64;
        for extent in &plan.final_extents {
            target.cancel.check()?;
            let extent_end = extent.offset + extent.block_count as u64 * info.chunk_size;
            if extent_end <= position {
                continue;
            }
            if extent.offset >= end {
                break;
            }
            verify_zero_range(target, position, end.min(extent.offset))?;
            position = position.max(end.min(extent_end));
        }
        verify_zero_range(target, position, end)?;
        if region.original.starts_with(b"EFI PART")
            && target
                .read(SECTOR_SIZE as usize, region.offset)?
                .starts_with(b"EFI PART")
        {
            return Err(Error::Apply(
                "GPT header remains after FFU application; do not boot this target".into(),
            ));
        }
    }
    Ok(())
}

pub(crate) fn wipe_target(
    source: &Source,
    target: &Target<'_>,
    progress: &mut Option<&mut Progress<'_>>,
) -> Result<()> {
    let mut target_stamp = target.stamp()?;
    let zeros = vec![0; WIPE_CHUNK_BYTES];
    for phase in ["Wiping entire drive", "Verifying blank drive"] {
        source.check(target.cancel)?;
        target.check()?;
        let mut last_percent = None;
        report(progress, phase, 0, target.cancel)?;
        for offset in (0..target.size).step_by(WIPE_CHUNK_BYTES) {
            source.check(target.cancel)?;
            target.check()?;
            let size = (WIPE_CHUNK_BYTES as u64).min(target.size - offset) as usize;
            if phase == "Wiping entire drive" {
                target.write(&zeros[..size], offset)?;
            } else if target.read(size, offset)? != zeros[..size] {
                return Err(Error::Apply(format!(
                    "full-wipe readback mismatch at byte {offset}; image was not applied"
                )));
            }
            let percent = ((offset + size as u64) * 100 / target.size).min(99) as u8;
            if last_percent != Some(percent) {
                report(progress, phase, percent, target.cancel)?;
            }
            last_percent = Some(percent);
        }
        if phase == "Wiping entire drive" {
            target.sync(true)?;
            target_stamp = target.stamp()?;
        } else if target.stamp()? != target_stamp {
            return Err(Error::Apply(
                "target changed during full-wipe verification; image was not applied".into(),
            ));
        }
        source.check(target.cancel)?;
        target.check()?;
        report(progress, phase, 100, target.cancel)?;
    }
    Ok(())
}

fn output_block(
    source: &mut Source,
    info: &ParsedFfu,
    configured: &BTreeMap<u64, &ConfiguredBlock>,
    offset: u64,
    payload: u64,
    cancel: Cancel<'_>,
) -> Result<Vec<u8>> {
    let data = source.payload(info, payload, cancel)?;
    Ok(match configured.get(&offset) {
        Some(block) if block.payload_block == payload => block.data.clone(),
        _ => data,
    })
}

pub(crate) fn apply_plan(
    source: &mut Source,
    info: &ParsedFfu,
    plan: &WritePlan,
    target: &Target<'_>,
    options: &ApplyOptions,
    blocks: &[ConfiguredBlock],
    progress: &mut Option<&mut Progress<'_>>,
) -> Result<()> {
    source.check(target.cancel)?;
    target.check()?;
    // A cache-capability failure must happen before destructive cleanup or writes.
    if !target.fixture {
        target.sync(true)?;
    }
    let mut cleanup = Vec::new();
    if options.wipe_entire_drive {
        wipe_target(source, target, progress)?;
    } else if options.allow_gpt_cleanup {
        cleanup = gpt_cleanup_plan(target, info)?;
        if !cleanup.is_empty() {
            clear_gpt_metadata(source, target, &cleanup, progress)?;
        }
    } else {
        reject_existing_gpt(target, info)?;
    }
    let configured = blocks.iter().map(|block| (block.offset, block)).collect();
    let total: u64 = plan
        .writes
        .iter()
        .map(|extent| extent.block_count as u64)
        .sum();
    let (mut done, mut previous_phase, mut last_percent) = (0_u64, None, Some(0));
    report(progress, "Writing", 0, target.cancel)?;
    for extent in &plan.writes {
        if previous_phase.is_some_and(|phase| phase != extent.phase) {
            target.sync(false)?;
        }
        previous_phase = Some(extent.phase);
        for index in 0..extent.block_count as u64 {
            target.check()?;
            let offset = extent.offset + index * info.chunk_size;
            let data = output_block(
                source,
                info,
                &configured,
                offset,
                extent.payload_block + index,
                target.cancel,
            )?;
            target.write(&data, offset)?;
            done += 1;
            let percent = (done * 100 / total).min(99) as u8;
            if last_percent != Some(percent) {
                report(progress, "Writing", percent, target.cancel)?;
            }
            last_percent = Some(percent);
        }
    }
    target.sync(true)?;
    source.check(target.cancel)?;
    let target_stamp = target.stamp()?;
    report(progress, "Writing", 100, target.cancel)?;
    let total: u64 = plan
        .final_extents
        .iter()
        .map(|extent| extent.block_count as u64)
        .sum();
    done = 0;
    last_percent = Some(0);
    report(progress, "Verifying", 0, target.cancel)?;
    for extent in &plan.final_extents {
        for index in 0..extent.block_count as u64 {
            target.check()?;
            let offset = extent.offset + index * info.chunk_size;
            let expected = output_block(
                source,
                info,
                &configured,
                offset,
                extent.payload_block + index,
                target.cancel,
            )?;
            let actual = target.read(info.chunk_size as usize, offset)?;
            if actual != expected {
                return Err(Error::Apply(format!(
                    "target readback mismatch at byte {offset}; do not boot this target"
                )));
            }
            done += 1;
            let percent = (done * 100 / total).min(99) as u8;
            if last_percent != Some(percent) {
                report(progress, "Verifying", percent, target.cancel)?;
            }
            last_percent = Some(percent);
        }
    }
    verify_gpt_cleanup(target, info, plan, &cleanup)?;
    source.check(target.cancel)?;
    target.check()?;
    if target.stamp()? != target_stamp {
        return Err(Error::Apply(
            "target changed during readback; do not boot this target".into(),
        ));
    }
    report(progress, "Verifying", 100, target.cancel)
}

pub(crate) fn validate_options(options: &ApplyOptions, fixture: bool) -> Result<()> {
    hdmi_options(&options.hdmi_mode, &options.hdmi_config)?;
    if options.wipe_entire_drive && options.allow_gpt_cleanup {
        return Err(Error::Target(
            "choose a full-drive wipe or bounded GPT cleanup, not both".into(),
        ));
    }
    if !fixture && (options.expected_sha256.is_none() || options.expected_target_id.is_none()) {
        if options.wipe_entire_drive {
            return Err(Error::Target(
                "full-drive wipe requires the verified image digest and confirmed target identity"
                    .into(),
            ));
        }
        if options.allow_gpt_cleanup {
            return Err(Error::Target(
                "GPT cleanup requires the verified image digest and confirmed target identity"
                    .into(),
            ));
        }
    }
    if options.expected_sha256.as_ref().is_some_and(|value| {
        value.len() != 64
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    }) {
        return Err(Error::Source(
            "expected SHA256 must be 64 lowercase hexadecimal characters".into(),
        ));
    }
    if options
        .expected_target_id
        .as_ref()
        .is_some_and(|value| !valid_identity(value))
    {
        return Err(Error::Target(
            "expected target identity is malformed".into(),
        ));
    }
    Ok(())
}

pub(crate) fn apply_with_io(
    path: &Path,
    target_path: &Path,
    options: &ApplyOptions,
    fixture: bool,
    mut progress: Option<&mut Progress<'_>>,
    cancel: Cancel<'_>,
    io: &dyn TargetIo,
) -> Result<ParsedFfu> {
    cancel.check()?;
    validate_options(options, fixture)?;
    let mut source = Source::open(path)?;
    let info = parse(&mut source.file, cancel)?;
    if !fixture {
        approve(&info)?;
    }
    if options
        .expected_sha256
        .as_ref()
        .is_some_and(|expected| info.sha256 != *expected)
    {
        return Err(Error::Source(
            "FFU SHA256 changed since inspection (expected hash mismatch)".into(),
        ));
    }
    let plan = resolve_plan(&info, options.target_size)?;
    let blocks = configured_blocks(
        &mut source,
        &info,
        &plan,
        &options.hdmi_mode,
        &options.hdmi_config,
        cancel,
    )?;
    source.check(cancel)?;
    let file = open_path(
        target_path,
        if fixture { Kind::Fixture } else { Kind::Device },
        Some(&source.metadata),
    )?;
    let identity = TargetIdentity::from(&unix::metadata(&file)?);
    if options
        .expected_target_id
        .as_ref()
        .is_some_and(|expected| identity.to_string() != *expected)
    {
        return Err(Error::Target(
            "target device changed since confirmation; no image blocks were written".into(),
        ));
    }
    if fixture {
        unix::lock(&file, true)?;
    }
    let target = Target {
        file,
        size: options.target_size,
        sector: info.sector_size,
        fixture,
        identity,
        io,
        cancel,
    };
    let result = apply_plan(
        &mut source,
        &info,
        &plan,
        &target,
        options,
        &blocks,
        &mut progress,
    );
    let target_close = target.io.close(target.file);
    let source_close = source.close();
    target_close?;
    source_close?;
    result?;
    Ok(info)
}

pub(crate) fn check_host(host: Host, release: &str) -> Result<()> {
    match host {
        Host::Linux
            if release.to_lowercase().contains("microsoft")
                || release.to_lowercase().contains("wsl") =>
        {
            Err(Error::Target(
                "WSL is not a safe native disk-flashing host".into(),
            ))
        }
        Host::Linux | Host::Macos => Ok(()),
        Host::Unsupported => Err(Error::Target(
            "only native Linux/macOS device targets are supported".into(),
        )),
    }
}

pub(crate) fn apply(
    path: &Path,
    target: &Path,
    options: &ApplyOptions,
    progress: Option<&mut Progress<'_>>,
    cancel: Cancel<'_>,
) -> Result<ParsedFfu> {
    let host = unix::host();
    check_host(
        host,
        &if host == Host::Linux {
            unix::release()?
        } else {
            String::new()
        },
    )?;
    apply_with_io(
        path,
        target,
        options,
        false,
        progress,
        cancel,
        &NativeTargetIo,
    )
}

/// Production entry point: exact approved source, native device-only target, mandatory readback.
pub fn apply_ffu(
    path: &Path,
    target: &Path,
    options: &ApplyOptions,
    progress: Option<&mut Progress<'_>>,
    cancellation: &AtomicBool,
) -> Result<ParsedFfu> {
    apply(path, target, options, progress, Cancel::new(cancellation))
}

/// **TEST ONLY.** Existing regular targets only, no symlinks/hard-link aliases or devices.
///
/// This shares the actual parser, HDMI derivation, cleanup, wipe and readback
/// implementation. It cannot authorize a production image/device and is never
/// reachable through a CLI argument, configuration value or environment variable.
pub fn apply_to_file_fixture(
    path: &Path,
    target: &Path,
    options: &ApplyOptions,
    progress: Option<&mut Progress<'_>>,
    cancellation: &AtomicBool,
) -> Result<ParsedFfu> {
    apply_with_io(
        path,
        target,
        options,
        true,
        progress,
        Cancel::new(cancellation),
        &NativeTargetIo,
    )
}
