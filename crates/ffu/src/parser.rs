use std::{
    collections::{BTreeMap, BTreeSet},
    io::{Read, Seek, SeekFrom},
    sync::atomic::AtomicBool,
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    Cancel, Error, MAX_CATALOG_BYTES, MAX_DESCRIPTORS, MAX_EXTENTS, MAX_HASH_BYTES,
    MAX_IMAGE_BYTES, MAX_LOCATIONS, MAX_MANIFEST_BYTES, MAX_TARGET_BYTES, NEVER_CANCELLED, Result,
    SECTOR_SIZE, align, u16_at, u32_at,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    Initial,
    Flash,
    Data,
    Final,
}

impl Phase {
    fn table_rank(self) -> Option<u8> {
        match self {
            Self::Initial => Some(0),
            Self::Flash => Some(1),
            Self::Final => Some(2),
            Self::Data => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DiskLocation {
    pub method: u32,
    pub block_index: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct WriteDescriptor {
    pub payload_block: u64,
    pub block_count: u32,
    pub locations: Vec<DiskLocation>,
    pub phase: Phase,
}

/// Untrusted-format metadata, not an image approval or a write capability.
#[derive(Clone, Debug, Serialize)]
pub struct ParsedFfu {
    pub sha256: String,
    pub file_bytes: u64,
    pub chunk_size: u64,
    pub sector_size: u64,
    pub minimum_disk_bytes: u64,
    pub image_offset: u64,
    pub payload_offset: u64,
    pub payload_bytes: u64,
    pub os_version: String,
    pub platform_ids: Vec<String>,
    #[serde(skip)]
    pub(crate) chunk_hashes: Vec<u8>,
    pub descriptors: Vec<WriteDescriptor>,
}

impl ParsedFfu {
    pub fn chunk_hashes(&self) -> &[u8] {
        &self.chunk_hashes
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Extent {
    pub offset: u64,
    pub payload_block: u64,
    pub block_count: u32,
    pub phase: Phase,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct WritePlan {
    pub writes: Vec<Extent>,
    pub final_extents: Vec<Extent>,
}

pub(crate) fn read_exact<R: Read>(
    source: &mut R,
    size: usize,
    cancel: Cancel<'_>,
) -> Result<Vec<u8>> {
    let mut data = vec![0; size];
    let mut done = 0;
    while done < size {
        cancel.check()?;
        match source.read(&mut data[done..]) {
            Ok(0) => return Err(Error::Source("truncated FFU".into())),
            Ok(count) => done += count,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => cancel.check()?,
            Err(error) => return Err(Error::io("read source", error)),
        }
    }
    cancel.check()?;
    Ok(data)
}

fn seek<R: Seek>(source: &mut R, position: SeekFrom) -> Result<u64> {
    source
        .seek(position)
        .map_err(|error| Error::io("seek source", error))
}

fn zero_padding<R: Read + Seek>(source: &mut R, end: u64, cancel: Cancel<'_>) -> Result<()> {
    let position = seek(source, SeekFrom::Current(0))?;
    let length = end
        .checked_sub(position)
        .filter(|length| *length <= 1024 * 1024)
        .ok_or_else(|| Error::Source("invalid FFU alignment".into()))?;
    if read_exact(source, length as usize, cancel)?
        .iter()
        .any(|byte| *byte != 0)
    {
        return Err(Error::Source("nonzero FFU padding is unsupported".into()));
    }
    Ok(())
}

type Manifest = Vec<(String, BTreeMap<String, String>)>;

fn manifest(data: &[u8]) -> Result<Manifest> {
    if !data.is_ascii() {
        return Err(Error::Source("manifest must be ASCII".into()));
    }
    if data
        .iter()
        .any(|byte| (*byte < 32 && !b"\r\n\t".contains(byte)) || *byte == 127)
    {
        return Err(Error::Source("invalid manifest control character".into()));
    }
    let mut sections: Manifest = Vec::new();
    for raw in std::str::from_utf8(data)
        .expect("ASCII")
        .split(['\r', '\n'])
    {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if ["[FullFlash]", "[Store]", "[Partition]"].contains(&line) {
            if sections.len() >= 130 {
                return Err(Error::Source("too many manifest sections".into()));
            }
            sections.push((line[1..line.len() - 1].into(), BTreeMap::new()));
            continue;
        }
        let (key, value) = line
            .split_once('=')
            .filter(|_| !sections.is_empty())
            .ok_or_else(|| Error::Source("unsupported manifest section or syntax".into()))?;
        let (key, value) = (key.trim(), value.trim());
        if key.is_empty()
            || !key.as_bytes()[0].is_ascii_alphabetic()
            || !key
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        {
            return Err(Error::Source("invalid manifest key".into()));
        }
        let key = key.to_ascii_lowercase();
        let values = &mut sections.last_mut().expect("section checked").1;
        if value.is_empty() || values.insert(key, value.into()).is_some() {
            return Err(Error::Source("duplicate or empty manifest value".into()));
        }
    }
    if sections.len() < 3
        || sections[0].0 != "FullFlash"
        || sections[1].0 != "Store"
        || sections[2..].iter().any(|(name, _)| name != "Partition")
    {
        return Err(Error::Source(
            "only a single-store FullFlash manifest is supported".into(),
        ));
    }
    Ok(sections)
}

fn positive_decimal(value: &str, maximum: u64, label: &str) -> Result<u64> {
    if value.is_empty() || value.len() > 20 || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(Error::Source(format!("invalid {label}")));
    }
    let number = value.parse::<u64>().ok();
    number
        .filter(|value| *value > 0 && *value <= maximum)
        .ok_or_else(|| Error::Source(format!("{label} exceeds supported bounds")))
}

fn platform_ids(raw: &[u8]) -> Result<Vec<String>> {
    if raw.last() != Some(&0) {
        return Err(Error::Source("unterminated platform IDs".into()));
    }
    let pieces: Vec<_> = raw.split(|byte| *byte == 0).collect();
    let end = pieces
        .iter()
        .position(|piece| piece.is_empty())
        .expect("terminated");
    if end == 0 || pieces[end..].iter().any(|piece| !piece.is_empty()) {
        return Err(Error::Source("invalid platform ID padding".into()));
    }
    if end > 16 || pieces[..end].iter().collect::<BTreeSet<_>>().len() != end {
        return Err(Error::Source("invalid platform ID count".into()));
    }
    if pieces[..end]
        .iter()
        .any(|piece| piece.iter().any(|byte| !(33..=126).contains(byte)))
    {
        return Err(Error::Source("invalid platform ID characters".into()));
    }
    Ok(pieces[..end]
        .iter()
        .map(|piece| std::str::from_utf8(piece).expect("ASCII").into())
        .collect())
}

fn catalog_envelope(catalog: &[u8]) -> Result<()> {
    if catalog.len() < 2 || catalog[0] != 0x30 {
        return Err(Error::Source("invalid catalog DER envelope".into()));
    }
    let mut size = catalog[1] as usize;
    let mut start = 2_usize;
    if size & 0x80 != 0 {
        let width = size & 0x7f;
        if !(1..=4).contains(&width) || catalog.len() < 2 + width {
            return Err(Error::Source("unsupported catalog DER length".into()));
        }
        size = catalog[2..2 + width]
            .iter()
            .fold(0, |value, byte| (value << 8) | *byte as usize);
        if size < 128 || catalog[2] == 0 {
            return Err(Error::Source("non-canonical catalog DER length".into()));
        }
        start += width;
    }
    if size == 0 || start.checked_add(size) != Some(catalog.len()) {
        return Err(Error::Source(
            "catalog size does not match DER envelope".into(),
        ));
    }
    Ok(())
}

/// Parse and hash a full, local, uncompressed single-store V1 FFU, read-only.
pub fn parse_ffu<R: Read + Seek>(source: &mut R) -> Result<ParsedFfu> {
    parse_ffu_with_cancel(source, &NEVER_CANCELLED)
}

pub fn parse_ffu_with_cancel<R: Read + Seek>(
    source: &mut R,
    cancellation: &AtomicBool,
) -> Result<ParsedFfu> {
    parse(source, Cancel::new(cancellation))
}

pub(crate) fn parse<R: Read + Seek>(source: &mut R, cancel: Cancel<'_>) -> Result<ParsedFfu> {
    cancel.check()?;
    let file_bytes = seek(source, SeekFrom::End(0))?;
    if !(32..=MAX_IMAGE_BYTES).contains(&file_bytes) {
        return Err(Error::Source("FFU size exceeds supported bounds".into()));
    }
    seek(source, SeekFrom::Start(0))?;
    let security = read_exact(source, 32, cancel)?;
    let size = u32_at(&security, 0);
    let kb = u32_at(&security, 16);
    let algorithm = u32_at(&security, 20);
    let catalog_size = u32_at(&security, 24) as usize;
    let hash_size = u32_at(&security, 28) as usize;
    if size != 32 || &security[4..16] != b"SignedImage " {
        return Err(Error::Source(
            "invalid security header (expected FFU SignedImage)".into(),
        ));
    }
    if !(1..=1024).contains(&kb) || !kb.is_power_of_two() {
        return Err(Error::Source("unsupported hash chunk size".into()));
    }
    let chunk = kb as u64 * 1024;
    if algorithm != 0x800c {
        return Err(Error::Source(
            "only SHA256 FFU chunk hashes are supported".into(),
        ));
    }
    if !(2..=MAX_CATALOG_BYTES).contains(&catalog_size) {
        return Err(Error::Source("invalid catalog size".into()));
    }
    if !(32..=MAX_HASH_BYTES).contains(&hash_size) || !hash_size.is_multiple_of(32) {
        return Err(Error::Source("invalid hash table size".into()));
    }
    let image_offset = align(size as u64 + catalog_size as u64 + hash_size as u64, chunk);
    if image_offset >= file_bytes || !(file_bytes - image_offset).is_multiple_of(chunk) {
        return Err(Error::Source("truncated or misaligned FFU image".into()));
    }
    if (file_bytes - image_offset) / chunk != (hash_size / 32) as u64 {
        return Err(Error::Source(
            "hash table does not cover the complete FFU image".into(),
        ));
    }
    catalog_envelope(&read_exact(source, catalog_size, cancel)?)?;
    let hashes = read_exact(source, hash_size, cancel)?;
    zero_padding(source, image_offset, cancel)?;
    let image = read_exact(source, 24, cancel)?;
    if u32_at(&image, 0) != 24 || &image[4..16] != b"ImageFlash  " {
        return Err(Error::Source(
            "unsupported image header or FFU variant".into(),
        ));
    }
    let manifest_size = u32_at(&image, 16) as usize;
    if u32_at(&image, 20) != kb || !(1..=MAX_MANIFEST_BYTES).contains(&manifest_size) {
        return Err(Error::Source(
            "invalid image chunk size or manifest length".into(),
        ));
    }
    let sections = manifest(&read_exact(source, manifest_size, cancel)?)?;
    let (full, store) = (&sections[0].1, &sections[1].1);
    let get =
        |values: &BTreeMap<String, String>, key: &str| values.get(key).cloned().unwrap_or_default();
    if get(full, "version") != "2.0" || get(full, "uefi") != "True" {
        return Err(Error::Source(
            "unsupported FullFlash manifest version".into(),
        ));
    }
    let version = get(full, "osversion");
    let version_parts: Vec<_> = version.split('.').collect();
    if version_parts.len() != 4
        || version_parts.iter().any(|part| {
            part.is_empty() || part.len() > 5 || !part.bytes().all(|byte| byte.is_ascii_digit())
        })
    {
        return Err(Error::Source("invalid manifest OSVersion".into()));
    }
    if get(store, "storetype") != "Default"
        || get(store, "ismainosstore") != "True"
        || get(store, "onlyallocatedefinedgptentries") != "False"
        || get(store, "sectorsize") != "512"
    {
        return Err(Error::Source(
            "unsupported store type, operation, or sector size".into(),
        ));
    }
    let sectors = positive_decimal(
        &get(store, "minsectorcount"),
        MAX_TARGET_BYTES / SECTOR_SIZE,
        "minimum sector count",
    )?;
    let names: Vec<_> = sections[2..]
        .iter()
        .map(|(_, values)| get(values, "name"))
        .collect();
    if names.iter().any(String::is_empty)
        || names.iter().collect::<BTreeSet<_>>().len() != names.len()
    {
        return Err(Error::Source("missing or duplicate partition name".into()));
    }
    let next = align(seek(source, SeekFrom::Current(0))?, chunk);
    zero_padding(source, next, cancel)?;
    let header = read_exact(source, 248, cancel)?;
    if u32_at(&header, 0) != 0 {
        return Err(Error::Source("delta/partial FFUs are unsupported".into()));
    }
    if [4, 6, 8, 10].map(|offset| u16_at(&header, offset)) != [1, 0, 2, 0] {
        return Err(Error::Source(
            "compressed, multistore, or other store versions are unsupported".into(),
        ));
    }
    let platforms = platform_ids(&header[12..204])?;
    let expected: BTreeMap<_, _> = platforms
        .iter()
        .enumerate()
        .map(|(index, value)| (format!("deviceplatformid{index}"), value.clone()))
        .collect();
    let actual: BTreeMap<_, _> = full
        .iter()
        .filter(|(key, _)| key.starts_with("deviceplatformid"))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    if expected != actual {
        return Err(Error::Source(
            "manifest and store platform IDs disagree".into(),
        ));
    }
    let descriptor_count = u32_at(&header, 208);
    let descriptor_bytes = u32_at(&header, 212) as u64;
    if u32_at(&header, 204) as u64 != chunk {
        return Err(Error::Source(
            "store block size must equal the hash chunk size".into(),
        ));
    }
    if u32_at(&header, 216) != 0 || u32_at(&header, 220) != 0 {
        return Err(Error::Source(
            "conditional validation/delta operations are unsupported".into(),
        ));
    }
    if !(1..=MAX_DESCRIPTORS).contains(&descriptor_count) {
        return Err(Error::Source(
            "write descriptor count exceeds supported bounds".into(),
        ));
    }
    if descriptor_bytes < descriptor_count as u64 * 16
        || descriptor_bytes > descriptor_count as u64 * (8 + 8 * MAX_LOCATIONS as u64)
    {
        return Err(Error::Source(
            "write descriptor length exceeds supported bounds".into(),
        ));
    }
    let descriptor_end = seek(source, SeekFrom::Current(0))? + descriptor_bytes;
    let payload_offset = align(descriptor_end, chunk);
    if payload_offset >= file_bytes {
        return Err(Error::Source("missing or truncated FFU payload".into()));
    }
    let payload_blocks = (file_bytes - payload_offset) / chunk;
    let ranges = [
        (
            u32_at(&header, 224) as u64,
            u32_at(&header, 228) as u64,
            Phase::Initial,
        ),
        (
            u32_at(&header, 232) as u64,
            u32_at(&header, 236) as u64,
            Phase::Flash,
        ),
        (
            u32_at(&header, 240) as u64,
            u32_at(&header, 244) as u64,
            Phase::Final,
        ),
    ];
    let mut previous_end = 0;
    let mut boundaries = BTreeSet::new();
    for (index, count, _) in ranges {
        if count == 0 {
            if index != 0 {
                return Err(Error::Source(
                    "empty table range has a nonzero index".into(),
                ));
            }
            continue;
        }
        if index < previous_end || index + count > payload_blocks {
            return Err(Error::Source("invalid partition-table phase bounds".into()));
        }
        previous_end = index + count;
        boundaries.extend([index, index + count]);
    }
    if ranges[0].1 != 0 && ranges[0].0 != 0 {
        return Err(Error::Source("initial table must begin the payload".into()));
    }
    if ranges[2].1 != 0 && ranges[2].0 + ranges[2].1 != payload_blocks {
        return Err(Error::Source("final table must end the payload".into()));
    }
    if (ranges[0].1 != 0 || ranges[1].1 != 0) && ranges[2].1 == 0 {
        return Err(Error::Source(
            "temporary tables require a final table".into(),
        ));
    }
    let mut descriptors = Vec::with_capacity(descriptor_count as usize);
    let (mut block, mut location_total) = (0_u64, 0_usize);
    for _ in 0..descriptor_count {
        cancel.check()?;
        if seek(source, SeekFrom::Current(0))? + 8 > descriptor_end {
            return Err(Error::Source("truncated write descriptors".into()));
        }
        let descriptor = read_exact(source, 8, cancel)?;
        let location_count = u32_at(&descriptor, 0);
        let block_count = u32_at(&descriptor, 4);
        if !(1..=MAX_LOCATIONS).contains(&location_count) {
            return Err(Error::Source(
                "unsupported descriptor location count".into(),
            ));
        }
        location_total += location_count as usize;
        if location_total > MAX_EXTENTS {
            return Err(Error::Source("too many target extents".into()));
        }
        if block_count == 0 || block_count as u64 > payload_blocks - block {
            return Err(Error::Source(
                "descriptor block count exceeds payload bounds".into(),
            ));
        }
        if seek(source, SeekFrom::Current(0))? + location_count as u64 * 8 > descriptor_end {
            return Err(Error::Source("truncated disk locations".into()));
        }
        if boundaries
            .iter()
            .any(|boundary| block < *boundary && *boundary < block + block_count as u64)
        {
            return Err(Error::Source(
                "descriptor crosses a partition-table phase boundary".into(),
            ));
        }
        let phase = ranges
            .iter()
            .find(|(index, count, _)| *index <= block && block < index + count)
            .map_or(Phase::Data, |(_, _, phase)| *phase);
        let mut locations = Vec::with_capacity(location_count as usize);
        for _ in 0..location_count {
            let location = read_exact(source, 8, cancel)?;
            let method = u32_at(&location, 0);
            if method != 0 && method != 2 {
                return Err(Error::Source("unsupported disk access method".into()));
            }
            locations.push(DiskLocation {
                method,
                block_index: u32_at(&location, 4),
            });
        }
        descriptors.push(WriteDescriptor {
            payload_block: block,
            block_count,
            locations,
            phase,
        });
        block += block_count as u64;
    }
    if seek(source, SeekFrom::Current(0))? != descriptor_end || block != payload_blocks {
        return Err(Error::Source(
            "descriptor lengths/counts do not match the complete payload".into(),
        ));
    }
    zero_padding(source, payload_offset, cancel)?;
    seek(source, SeekFrom::Start(0))?;
    let mut digest = Sha256::new();
    for offset in (0..file_bytes).step_by(chunk as usize) {
        let data = read_exact(source, chunk as usize, cancel)?;
        digest.update(&data);
        if offset >= image_offset {
            let index = ((offset - image_offset) / chunk) as usize;
            if Sha256::digest(&data)[..] != hashes[index * 32..(index + 1) * 32] {
                return Err(Error::Source(format!(
                    "FFU chunk hash mismatch at chunk {index}"
                )));
            }
        }
    }
    let mut trailing = [0];
    cancel.check()?;
    match source.read(&mut trailing) {
        Ok(0) => (),
        Ok(_) => return Err(Error::Source("FFU changed size during validation".into())),
        Err(error) => return Err(Error::io("check source length", error)),
    }
    let info = ParsedFfu {
        sha256: format!("{:x}", digest.finalize()),
        file_bytes,
        chunk_size: chunk,
        sector_size: SECTOR_SIZE,
        minimum_disk_bytes: sectors * SECTOR_SIZE,
        image_offset,
        payload_offset,
        payload_bytes: file_bytes - payload_offset,
        os_version: version,
        platform_ids: platforms,
        chunk_hashes: hashes,
        descriptors,
    };
    match resolve_plan(&info, info.minimum_disk_bytes) {
        Err(Error::Target(message)) => {
            return Err(Error::Source(format!(
                "invalid FFU minimum geometry: {message}"
            )));
        }
        Err(error) => return Err(error),
        Ok(_) => (),
    }
    cancel.check()?;
    Ok(info)
}

/// Resolve start/end-relative locations; this never opens or authorizes a target.
pub fn resolve_plan(info: &ParsedFfu, target_size: u64) -> Result<WritePlan> {
    if !(info.minimum_disk_bytes..=MAX_TARGET_BYTES).contains(&target_size)
        || info.sector_size != SECTOR_SIZE
        || info.chunk_size == 0
        || info.chunk_size > 1024 * 1024
        || !target_size.is_multiple_of(info.sector_size)
    {
        return Err(Error::Target(
            "target size is too small, too large, or not sector-aligned".into(),
        ));
    }
    let mut writes = Vec::new();
    for descriptor in &info.descriptors {
        let length = descriptor.block_count as u64 * info.chunk_size;
        for location in &descriptor.locations {
            let distance = (location.block_index as u64 + 1) * info.chunk_size;
            let offset = match location.method {
                0 => Some(location.block_index as u64 * info.chunk_size),
                2 => target_size.checked_sub(distance),
                _ => None,
            }
            .filter(|offset| {
                *offset <= target_size
                    && length <= target_size - offset
                    && offset.is_multiple_of(info.sector_size)
            })
            .ok_or_else(|| {
                Error::Target("FFU extent exceeds target bounds or sector alignment".into())
            })?;
            writes.push(Extent {
                offset,
                payload_block: descriptor.payload_block,
                block_count: descriptor.block_count,
                phase: descriptor.phase,
            });
            if writes.len() > MAX_EXTENTS {
                return Err(Error::Source("too many target extents".into()));
            }
        }
    }
    let mut ordered = writes.clone();
    // Stable ordering is essential: phase order within identical locations is write order.
    ordered.sort_by_key(|extent| (extent.offset, extent.block_count));
    let mut final_extents = Vec::new();
    let mut position = 0;
    let mut previous_end = 0;
    while position < ordered.len() {
        let first = &ordered[position];
        let mut end = position + 1;
        while end < ordered.len()
            && (ordered[end].offset, ordered[end].block_count) == (first.offset, first.block_count)
        {
            end += 1;
        }
        let group = &ordered[position..end];
        if first.offset < previous_end {
            return Err(Error::Source(
                "partially overlapping target extents are unsupported".into(),
            ));
        }
        previous_end = first.offset + first.block_count as u64 * info.chunk_size;
        if group.len() > 1 {
            let phases: Option<Vec<_>> = group
                .iter()
                .map(|extent| extent.phase.table_rank())
                .collect();
            let valid = phases.as_ref().is_some_and(|ranks| {
                ranks.last() == Some(&2) && ranks.windows(2).all(|pair| pair[0] < pair[1])
            });
            if !valid {
                return Err(Error::Source(
                    "overlapping data or duplicate table locations are unsafe".into(),
                ));
            }
        }
        let last = group.last().expect("nonempty group");
        if matches!(last.phase, Phase::Initial | Phase::Flash) {
            return Err(Error::Source(
                "temporary table extent is not replaced by an exact final extent".into(),
            ));
        }
        final_extents.push(last.clone());
        position = end;
    }
    Ok(WritePlan {
        writes,
        final_extents,
    })
}
