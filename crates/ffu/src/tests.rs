use std::{
    cell::{Cell, RefCell},
    ffi::OsString,
    fs::{self, File, OpenOptions},
    io::{self, Cursor, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
};

use sha2::{Digest, Sha256};

use super::*;
use crate::{
    source::{Kind, Source, SourceStamp, open_path},
    unix::{Host, Ioctl, Metadata},
    writer::{NativeTargetIo, Target, TargetIo, apply_plan, apply_with_io},
};

const CHUNK: usize = 1024;
const SIZE: u64 = 32768;
const DEFAULT_RANGES: [u32; 6] = [0, 1, 3, 1, 5, 1];
const MANIFEST: &str = "\
[FullFlash]
AntiTheftVersion = 1.1
Description = Synthetic non-Windows test fixture
StateSeparationLevel = 0
OSVersion = 10.0.17763.107
Version = 2.0
UEFI = True
DevicePlatformId0 = Broadcom.RPi.2
DevicePlatformId1 = *
[Store]
OnlyAllocateDefinedGptEntries = False
StoreType = Default
MinSectorCount = 64
IsMainOSStore = True
SectorSize = 512
[Partition]
Name = NonWindowsFixture
TotalSectors = 16
";
type Descriptors = Vec<(u32, Vec<(u32, u32)>)>;

fn default_descriptors() -> Descriptors {
    vec![
        (1, vec![(0, 0)]),
        (2, vec![(0, 2), (0, 6)]),
        (1, vec![(0, 0)]),
        (1, vec![(2, 0)]),
        (1, vec![(0, 0)]),
    ]
}

fn put16(data: &mut [u8], offset: usize, value: u16) {
    data[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}
fn put32(data: &mut [u8], offset: usize, value: u32) {
    data[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}
fn put64(data: &mut [u8], offset: usize, value: u64) {
    data[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}
fn pad(data: &mut Vec<u8>) {
    data.resize(data.len().div_ceil(CHUNK) * CHUNK, 0);
}

struct Fixture {
    blob: Vec<u8>,
    image_offset: usize,
    store_offset: usize,
    descriptor_offset: usize,
    payload_offset: usize,
    hash_offset: usize,
}

impl Fixture {
    fn new() -> Self {
        Self::build(
            default_descriptors(),
            DEFAULT_RANGES,
            MANIFEST,
            &["Broadcom.RPi.2", "*"],
        )
    }

    fn build(
        descriptors: Descriptors,
        ranges: [u32; 6],
        manifest: &str,
        platforms: &[&str],
    ) -> Self {
        let mut image = vec![0; 24];
        put32(&mut image, 0, 24);
        image[4..16].copy_from_slice(b"ImageFlash  ");
        put32(&mut image, 16, manifest.len() as u32);
        put32(&mut image, 20, 1);
        image.extend_from_slice(manifest.as_bytes());
        pad(&mut image);
        let mut store = vec![0; 248];
        for (offset, value) in [(4, 1), (6, 0), (8, 2), (10, 0)] {
            put16(&mut store, offset, value);
        }
        let mut platform_bytes = platforms.join("\0").into_bytes();
        platform_bytes.push(0);
        let length = platform_bytes.len().min(192);
        store[12..12 + length].copy_from_slice(&platform_bytes[..length]);
        put32(&mut store, 204, CHUNK as u32);
        put32(&mut store, 208, descriptors.len() as u32);
        for (index, value) in ranges.iter().enumerate() {
            put32(&mut store, 224 + 4 * index, *value);
        }
        let mut count = 0;
        for (blocks, locations) in descriptors {
            store.extend_from_slice(&(locations.len() as u32).to_le_bytes());
            store.extend_from_slice(&blocks.to_le_bytes());
            count += blocks;
            for (method, index) in locations {
                store.extend_from_slice(&method.to_le_bytes());
                store.extend_from_slice(&index.to_le_bytes());
            }
        }
        let descriptor_bytes = store.len() - 248;
        put32(&mut store, 212, descriptor_bytes as u32);
        pad(&mut store);
        let mut body = image.clone();
        body.extend_from_slice(&store);
        for index in 0..count {
            body.extend(vec![48 + (index % 40) as u8; CHUNK]);
        }
        let mut hashes = Vec::new();
        for block in body.as_chunks::<CHUNK>().0 {
            hashes.extend_from_slice(&Sha256::digest(block));
        }
        let catalog = b"\x30\x03\x02\x01\x01";
        let mut blob = vec![0; 32];
        put32(&mut blob, 0, 32);
        blob[4..16].copy_from_slice(b"SignedImage ");
        put32(&mut blob, 16, 1);
        put32(&mut blob, 20, 0x800c);
        put32(&mut blob, 24, catalog.len() as u32);
        put32(&mut blob, 28, hashes.len() as u32);
        blob.extend_from_slice(catalog);
        blob.extend_from_slice(&hashes);
        pad(&mut blob);
        let image_offset = blob.len();
        let store_offset = image_offset + image.len();
        let payload_offset = store_offset + store.len();
        blob.extend_from_slice(&body);
        Self {
            blob,
            image_offset,
            store_offset,
            descriptor_offset: store_offset + 248,
            payload_offset,
            hash_offset: 32 + catalog.len(),
        }
    }

    fn rehash(&self, mut blob: Vec<u8>) -> Vec<u8> {
        for (index, offset) in (self.image_offset..blob.len()).step_by(CHUNK).enumerate() {
            let digest = Sha256::digest(&blob[offset..offset + CHUNK]);
            let start = self.hash_offset + index * 32;
            blob[start..start + 32].copy_from_slice(&digest);
        }
        blob
    }

    fn mutate(&self, offset: usize, width: usize, value: u64, rehash: bool) -> Vec<u8> {
        let mut blob = self.blob.clone();
        blob[offset..offset + width].copy_from_slice(&value.to_le_bytes()[..width]);
        if rehash { self.rehash(blob) } else { blob }
    }

    fn parse(&self) -> Result<ParsedFfu> {
        parse_ffu(&mut Cursor::new(&self.blob))
    }

    fn approval(&self) -> ApprovedImage {
        let info = self.parse().unwrap();
        ApprovedImage {
            sha256: info.sha256,
            format: "ffu-v1".into(),
            profile: "iot-core".into(),
            architecture: "arm32".into(),
            build: "17763.107".into(),
            os_version: info.os_version,
            platform_ids: info.platform_ids,
            recommended_boards: vec!["pi2-v1.1".into(), "pi2-v1.2".into(), "pi3-b".into()],
            file_bytes: info.file_bytes,
            payload_bytes: info.payload_bytes,
            minimum_disk_bytes: info.minimum_disk_bytes,
            sector_size: info.sector_size,
            chunk_size: info.chunk_size,
            source_url: "https://example.invalid/synthetic-fixture".into(),
        }
    }
}

static DIRECTORY: AtomicU64 = AtomicU64::new(0);

struct Files {
    directory: PathBuf,
    source: PathBuf,
    target: PathBuf,
    fixture: Fixture,
}

impl Files {
    fn new() -> Self {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join(".test-state");
        fs::create_dir_all(&root).unwrap();
        let directory = root.join(format!(
            "native-{}-{}",
            std::process::id(),
            DIRECTORY.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&directory).unwrap();
        let directory = directory.canonicalize().unwrap();
        let source = directory.join("not Windows media.ffu");
        let target = directory.join("ordinary-target.raw");
        let fixture = Fixture::new();
        fs::write(&source, &fixture.blob).unwrap();
        fs::write(&target, vec![0xa5; SIZE as usize]).unwrap();
        Self {
            directory,
            source,
            target,
            fixture,
        }
    }

    fn options(&self) -> ApplyOptions {
        ApplyOptions {
            target_size: SIZE,
            ..Default::default()
        }
    }

    fn apply(&self, options: &ApplyOptions) -> Result<ParsedFfu> {
        apply_to_file_fixture(
            &self.source,
            &self.target,
            options,
            None,
            &AtomicBool::new(false),
        )
    }

    fn with_io(
        &self,
        options: &ApplyOptions,
        io: &dyn TargetIo,
        progress: Option<&mut Progress<'_>>,
    ) -> Result<ParsedFfu> {
        apply_with_io(
            &self.source,
            &self.target,
            options,
            true,
            progress,
            Cancel::new(&NEVER_CANCELLED),
            io,
        )
    }

    fn expected(&self, size: u64, fill: u8) -> Vec<u8> {
        let mut data = vec![fill; size as usize];
        for extent in resolve_plan(&self.fixture.parse().unwrap(), size)
            .unwrap()
            .writes
        {
            let start = self.fixture.payload_offset + extent.payload_block as usize * CHUNK;
            let length = extent.block_count as usize * CHUNK;
            data[extent.offset as usize..extent.offset as usize + length]
                .copy_from_slice(&self.fixture.blob[start..start + length]);
        }
        data
    }
}

impl Drop for Files {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.directory).unwrap();
        let _ = fs::remove_dir(self.directory.parent().unwrap());
    }
}

fn assert_error<T: std::fmt::Debug>(result: Result<T>, status: i32, message: &str) {
    let error = result.expect_err("safety guard unexpectedly accepted input");
    assert_eq!(error.exit_status(), status, "{error}");
    assert!(
        error.to_string().contains(message),
        "expected {message:?}, got {error}"
    );
}

#[test]
fn parser_valid_multilocation_and_phases() {
    let fixture = Fixture::new();
    let info = fixture.parse().unwrap();
    assert_eq!(info.sha256, format!("{:x}", Sha256::digest(&fixture.blob)));
    assert_eq!(info.platform_ids, ["Broadcom.RPi.2", "*"]);
    assert_eq!(info.payload_offset, fixture.payload_offset as u64);
    assert_eq!(info.payload_bytes, 6 * CHUNK as u64);
    assert_eq!(info.minimum_disk_bytes, SIZE);
    assert_eq!(info.chunk_hashes().len(), 8 * 32);
    assert_eq!(
        info.descriptors
            .iter()
            .map(|descriptor| descriptor.payload_block)
            .collect::<Vec<_>>(),
        [0, 1, 3, 4, 5]
    );
    assert_eq!(
        info.descriptors
            .iter()
            .map(|descriptor| descriptor.phase)
            .collect::<Vec<_>>(),
        [
            Phase::Initial,
            Phase::Data,
            Phase::Flash,
            Phase::Data,
            Phase::Final
        ]
    );
    assert_eq!(info.descriptors[1].block_count, 2);
    assert_eq!(info.descriptors[1].locations.len(), 2);
}

#[test]
fn payload_hash_guard() {
    let fixture = Fixture::new();
    let blob = fixture.mutate(fixture.payload_offset + CHUNK + 21, 1, 1, false);
    assert_error(parse_ffu(&mut Cursor::new(blob)), 2, "chunk hash");
}

#[test]
fn metadata_and_hash_table_corruption() {
    let fixture = Fixture::new();
    for offset in [
        fixture.image_offset + 80,
        fixture.store_offset + 245,
        fixture.hash_offset + 7 * 32,
    ] {
        let mut blob = fixture.blob.clone();
        blob[offset] ^= 1;
        assert!(
            parse_ffu(&mut Cursor::new(blob)).is_err(),
            "offset {offset}"
        );
    }
}

#[test]
fn catalog_whole_file_digest() {
    let fixture = Fixture::new();
    let blob = fixture.mutate(36, 1, 2, false);
    let info = parse_ffu(&mut Cursor::new(blob)).unwrap();
    assert_ne!(info.sha256, fixture.parse().unwrap().sha256);
    assert_error(
        trust::check_approval(&info, &fixture.approval()),
        2,
        "SHA256",
    );
}

#[test]
fn parser_header_bounds_even_after_rehash() {
    let fixture = Fixture::new();
    let (image, store, desc) = (
        fixture.image_offset,
        fixture.store_offset,
        fixture.descriptor_offset,
    );
    let cases = [
        (0, 4, 40),
        (16, 4, 0),
        (16, 4, 3),
        (16, 4, 2048),
        (20, 4, 0x8004),
        (24, 4, 0),
        (24, 4, u32::MAX as u64),
        (28, 4, 0),
        (28, 4, 33),
        (28, 4, u32::MAX as u64),
        (28, 4, 7 * 32),
        (image, 4, 28),
        (image + 16, 4, 0),
        (image + 16, 4, 65537),
        (image + 20, 4, 2),
        (store, 4, 1),
        (store + 4, 2, 2),
        (store + 6, 2, 1),
        (store + 8, 2, 3),
        (store + 10, 2, 1),
        (store + 204, 4, 512),
        (store + 208, 4, 0),
        (store + 208, 4, u32::MAX as u64),
        (store + 212, 4, u32::MAX as u64),
        (store + 212, 4, 87),
        (store + 216, 4, 1),
        (store + 220, 4, 12),
        (store + 224, 4, 1),
        (store + 228, 4, u32::MAX as u64),
        (store + 232, 4, 2),
        (store + 240, 4, 2),
        (store + 244, 4, 0),
        (desc, 4, 0),
        (desc, 4, 17),
        (desc + 4, 4, 0),
        (desc + 4, 4, u32::MAX as u64),
        (desc + 8, 4, 1),
        (desc + 12, 4, u32::MAX as u64),
    ];
    for (offset, width, value) in cases {
        assert!(
            parse_ffu(&mut Cursor::new(fixture.mutate(offset, width, value, true))).is_err(),
            "{offset}/{width}/{value}"
        );
    }
}

#[test]
fn parser_padding_magic_platform_and_der_bounds() {
    let fixture = Fixture::new();
    for (offset, value) in [
        (4, b'X'),
        (32, 0x31),
        (fixture.hash_offset + 8 * 32, 1),
        (fixture.image_offset + 4, b'X'),
        (fixture.store_offset - 1, 1),
        (fixture.payload_offset - 1, 1),
        (fixture.store_offset + 203, 1),
        (fixture.store_offset + 12, 0xff),
        (33, 0x80),
        (33, 0x85),
        (33, 0),
        (33, 6),
    ] {
        assert!(
            parse_ffu(&mut Cursor::new(fixture.mutate(
                offset,
                1,
                value as u64,
                true
            )))
            .is_err(),
            "offset {offset}: {value}"
        );
    }
    for platforms in [&["", "*"][..], &["A", "A"][..], &["with space", "*"][..]] {
        assert!(
            Fixture::build(default_descriptors(), DEFAULT_RANGES, MANIFEST, platforms)
                .parse()
                .is_err()
        );
    }
    let long = "B".repeat(190);
    assert_error(
        Fixture::build(
            default_descriptors(),
            DEFAULT_RANGES,
            MANIFEST,
            &["A", &long],
        )
        .parse(),
        2,
        "unterminated platform",
    );
}

#[test]
fn manifest_rejects_ambiguity_unsupported_geometry_and_controls() {
    let manifests = [
        format!("{MANIFEST}[Store]\nStoreType = Default\n"),
        MANIFEST.replace("Version = 2.0", "Version = 3.0"),
        MANIFEST.replace("Version = 2.0", "Version = 2.0\nversion = 2.0"),
        MANIFEST.replace("SectorSize = 512", "SectorSize = 4096"),
        MANIFEST.replace("MinSectorCount = 64", "MinSectorCount = -1"),
        MANIFEST.replace(
            "MinSectorCount = 64",
            "MinSectorCount = 999999999999999999999",
        ),
        MANIFEST.replace(
            "MinSectorCount = 64",
            "MinSectorCount = 18446744073709551615",
        ),
        MANIFEST.replace("StoreType = Default", "StoreType = Update"),
        MANIFEST.replace("IsMainOSStore = True", "IsMainOSStore = False"),
        MANIFEST.replace(
            "OnlyAllocateDefinedGptEntries = False",
            "OnlyAllocateDefinedGptEntries = True",
        ),
        MANIFEST.replace(
            "DevicePlatformId0 = Broadcom.RPi.2",
            "DevicePlatformId0 = SomeOther.Board",
        ),
        MANIFEST.replace("OSVersion = 10.0.17763.107", "OSVersion = not-a-version"),
        MANIFEST.replace("Name = NonWindowsFixture", "Name ="),
        format!("{MANIFEST}[Partition]\nName = NonWindowsFixture\n"),
        MANIFEST.replace("AntiTheftVersion", "4Invalid"),
        MANIFEST.replace("AntiTheftVersion", "Invalid-Key"),
        MANIFEST.replace("AntiTheftVersion", "Invalid\u{7f}"),
        MANIFEST.replace("AntiTheftVersion", "\0"),
        format!("{MANIFEST}{}", "[Partition]\nName = Another\n".repeat(129)),
    ];
    for manifest in manifests {
        assert!(
            Fixture::build(
                default_descriptors(),
                DEFAULT_RANGES,
                &manifest,
                &["Broadcom.RPi.2", "*"]
            )
            .parse()
            .is_err(),
            "{manifest}"
        );
    }
    let fixture = Fixture::new();
    assert_error(
        parse_ffu(&mut Cursor::new(fixture.mutate(
            fixture.image_offset + 30,
            1,
            0xff,
            true,
        ))),
        2,
        "ASCII",
    );
}

#[test]
fn truncation_and_trailing_bytes_are_rejected() {
    let fixture = Fixture::new();
    for end in [
        0,
        31,
        fixture.image_offset - 1,
        fixture.payload_offset - 1,
        fixture.blob.len() - 1,
        fixture.blob.len() - CHUNK,
    ] {
        assert!(
            parse_ffu(&mut Cursor::new(&fixture.blob[..end])).is_err(),
            "end {end}"
        );
    }
    for length in [1, CHUNK] {
        let mut blob = fixture.blob.clone();
        blob.extend(vec![0; length]);
        assert!(parse_ffu(&mut Cursor::new(blob)).is_err());
    }
}

#[test]
fn overlap_is_not_last_writer_wins() {
    let cases = [
        (vec![(2, vec![(0, 2)]), (1, vec![(0, 3)])], [0; 6]),
        (vec![(1, vec![(0, 2), (0, 2)])], [0; 6]),
        (vec![(1, vec![(0, 2)]), (1, vec![(0, 2)])], [0; 6]),
        (
            vec![(1, vec![(0, 0)]), (1, vec![(0, 2)]), (1, vec![(0, 1)])],
            [0, 1, 0, 0, 2, 1],
        ),
    ];
    for (descriptors, ranges) in cases {
        assert!(
            Fixture::build(descriptors, ranges, MANIFEST, &["Broadcom.RPi.2", "*"])
                .parse()
                .is_err()
        );
    }
}

#[test]
fn end_relative_multiblock_is_forward_writing() {
    let fixture = Fixture::build(
        vec![(2, vec![(2, 1)])],
        [0; 6],
        MANIFEST,
        &["Broadcom.RPi.2", "*"],
    );
    let info = fixture.parse().unwrap();
    let plan = resolve_plan(&info, SIZE + 512).unwrap();
    assert_eq!(plan.writes[0].offset, SIZE + 512 - 2 * CHUNK as u64);
    assert_error(
        Fixture::build(
            vec![(2, vec![(2, 0)])],
            [0; 6],
            MANIFEST,
            &["Broadcom.RPi.2", "*"],
        )
        .parse(),
        2,
        "invalid FFU minimum geometry",
    );
}

#[test]
fn target_dependent_overlap_and_capacity_rechecked() {
    let fixture = Fixture::build(
        vec![(1, vec![(0, 30)]), (1, vec![(2, 3)])],
        [0; 6],
        MANIFEST,
        &["Broadcom.RPi.2", "*"],
    );
    let info = fixture.parse().unwrap();
    assert_error(resolve_plan(&info, 34 * CHUNK as u64), 2, "overlap");
    for size in [0, SIZE - 512, SIZE + 1, MAX_TARGET_BYTES + 512] {
        assert_error(resolve_plan(&info, size), 3, "target size");
    }
    resolve_plan(&info, MAX_TARGET_BYTES).unwrap();
}

#[test]
fn bounded_reader_and_short_source_reads() {
    struct Reader {
        inner: Cursor<Vec<u8>>,
        short: bool,
    }
    impl Read for Reader {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            assert!(
                buffer.len() <= CHUNK,
                "unexpected read size {}",
                buffer.len()
            );
            let limit = if self.short {
                buffer.len().min(17)
            } else {
                buffer.len()
            };
            self.inner.read(&mut buffer[..limit])
        }
    }
    impl Seek for Reader {
        fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
            self.inner.seek(from)
        }
    }
    let fixture = Fixture::new();
    for short in [true, false] {
        parse_ffu(&mut Reader {
            inner: Cursor::new(fixture.blob.clone()),
            short,
        })
        .unwrap();
        assert!(
            parse_ffu(&mut Reader {
                inner: Cursor::new(fixture.mutate(
                    fixture.descriptor_offset + 4,
                    4,
                    u32::MAX as u64,
                    true
                )),
                short
            })
            .is_err()
        );
    }
}

#[test]
fn read_only_parser_and_cancellation() {
    let files = Files::new();
    parse_ffu(&mut File::open(&files.source).unwrap()).unwrap();
    let flag = AtomicBool::new(true);
    assert_error(
        parse_ffu_with_cancel(&mut Cursor::new(files.fixture.blob.clone()), &flag),
        130,
        "signal 2",
    );
    assert_eq!(fs::read(&files.source).unwrap(), files.fixture.blob);
}

#[test]
fn source_growth_at_final_read_is_rejected() {
    struct Growing {
        source: Cursor<Vec<u8>>,
        grown: bool,
    }
    impl Read for Growing {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            if self.source.position() == self.source.get_ref().len() as u64 && !self.grown {
                self.source.get_mut().push(0);
                self.grown = true;
            }
            self.source.read(buffer)
        }
    }
    impl Seek for Growing {
        fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
            self.source.seek(position)
        }
    }
    let fixture = Fixture::new();
    assert_error(
        parse_ffu(&mut Growing {
            source: Cursor::new(fixture.blob),
            grown: false,
        }),
        2,
        "changed size",
    );
}

#[test]
fn source_and_fixture_locks_fail_closed_without_writes() {
    let files = Files::new();
    for path in [&files.source, &files.target] {
        let locked = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .unwrap();
        unix::lock(&locked, true).unwrap();
        assert_error(files.apply(&files.options()), 4, "lock source/fixture");
        assert_eq!(fs::read(&files.target).unwrap(), vec![0xa5; SIZE as usize]);
    }
}

#[test]
fn fixture_exact_extents_preserve_holes_and_source() {
    let files = Files::new();
    files.apply(&files.options()).unwrap();
    assert_eq!(fs::read(&files.target).unwrap(), files.expected(SIZE, 0xa5));
    assert_eq!(fs::metadata(&files.target).unwrap().len(), SIZE);
    assert_eq!(fs::read(&files.source).unwrap(), files.fixture.blob);
}

#[test]
fn non_chunk_aligned_target_and_replication() {
    let files = Files::new();
    let size = SIZE + 512;
    fs::write(&files.target, vec![0xa5; size as usize]).unwrap();
    let mut options = files.options();
    options.target_size = size;
    files.apply(&options).unwrap();
    assert_eq!(fs::read(&files.target).unwrap(), files.expected(size, 0xa5));
    let fixture = Fixture::build(
        vec![(2, vec![(0, 2), (2, 1)])],
        [0; 6],
        MANIFEST,
        &["Broadcom.RPi.2", "*"],
    );
    fs::write(&files.source, &fixture.blob).unwrap();
    files.apply(&options).unwrap();
    let actual = fs::read(&files.target).unwrap();
    assert_eq!(
        &actual[2 * CHUNK..4 * CHUNK],
        &fixture.blob[fixture.payload_offset..]
    );
    assert_eq!(
        &actual[actual.len() - 2 * CHUNK..],
        &fixture.blob[fixture.payload_offset..]
    );
}

#[test]
fn larger_disk_fixed_layout_unused_tail() {
    let files = Files::new();
    let mut descriptors = default_descriptors();
    descriptors[3] = (1, vec![(0, 10)]);
    let fixture = Fixture::build(
        descriptors,
        DEFAULT_RANGES,
        MANIFEST,
        &["Broadcom.RPi.2", "*"],
    );
    let mut blob = fixture.blob.clone();
    let start = fixture.payload_offset + 5 * CHUNK;
    let mut mbr = vec![0; 512];
    mbr[450] = 0x0c;
    put32(&mut mbr, 454, 4);
    put32(&mut mbr, 458, 4);
    mbr[510..].copy_from_slice(b"\x55\xaa");
    blob[start..start + 512].copy_from_slice(&mbr);
    fs::write(&files.source, fixture.rehash(blob)).unwrap();
    let size = 16 * 1024 * 1024;
    let target = OpenOptions::new().write(true).open(&files.target).unwrap();
    target.set_len(size).unwrap();
    unix::write_at(&target, b"UNUSED CAPACITY", size - 512).unwrap();
    let mut options = files.options();
    options.target_size = size;
    let info = files.apply(&options).unwrap();
    assert_eq!(
        resolve_plan(&info, SIZE).unwrap(),
        resolve_plan(&info, size).unwrap()
    );
    let bytes = fs::read(&files.target).unwrap();
    assert_eq!(&bytes[..512], &mbr);
    assert_eq!(
        &bytes[size as usize - 512..size as usize - 512 + b"UNUSED CAPACITY".len()],
        b"UNUSED CAPACITY"
    );
}

#[test]
fn gpt_preflight_guard() {
    let files = Files::new();
    for (size, offset) in [
        (SIZE, 512),
        (SIZE + 2 * CHUNK as u64, SIZE - 512),
        (SIZE + 2 * CHUNK as u64, SIZE + 2 * CHUNK as u64 - 512),
    ] {
        let mut before = vec![0xa5; size as usize];
        before[offset as usize..offset as usize + 8].copy_from_slice(b"EFI PART");
        fs::write(&files.target, &before).unwrap();
        let mut options = files.options();
        options.target_size = size;
        assert_error(files.apply(&options), 3, "existing GPT");
        assert_eq!(fs::read(&files.target).unwrap(), before);
    }
}

#[test]
fn complete_source_before_target_open_and_allowlist_guard() {
    let files = Files::new();
    let missing = files.directory.join("must-not-be-opened");
    let mut damaged = files.fixture.blob.clone();
    *damaged.last_mut().unwrap() ^= 1;
    fs::write(&files.source, damaged).unwrap();
    assert_error(
        apply_to_file_fixture(
            &files.source,
            &missing,
            &files.options(),
            None,
            &NEVER_CANCELLED,
        ),
        2,
        "chunk hash",
    );
    fs::write(&files.source, &files.fixture.blob).unwrap();
    assert_error(
        apply_ffu(
            &files.source,
            &missing,
            &files.options(),
            None,
            &NEVER_CANCELLED,
        ),
        2,
        "not allowlisted",
    );
    assert!(!missing.exists());
}

#[test]
fn prior_hash_is_never_allowlist_bypass() {
    let files = Files::new();
    let mut options = files.options();
    options.expected_sha256 = Some(files.fixture.parse().unwrap().sha256);
    assert_error(
        apply_ffu(
            &files.source,
            &files.target,
            &options,
            None,
            &NEVER_CANCELLED,
        ),
        2,
        "not allowlisted",
    );
    assert_eq!(fs::read(&files.target).unwrap(), vec![0xa5; SIZE as usize]);
}

#[test]
fn expected_hash_mismatch_before_target() {
    let files = Files::new();
    let mut options = files.options();
    options.expected_sha256 = Some("0".repeat(64));
    let target = files.directory.join("absent");
    assert_error(
        apply_to_file_fixture(&files.source, &target, &options, None, &NEVER_CANCELLED),
        2,
        "changed since inspection",
    );
    assert!(!target.exists());
}

#[test]
fn invalid_actual_size_never_writes() {
    let files = Files::new();
    fs::write(&files.target, vec![0xa5; SIZE as usize + 512]).unwrap();
    assert_error(files.apply(&files.options()), 3, "actual target");
    assert_eq!(
        fs::read(&files.target).unwrap(),
        vec![0xa5; SIZE as usize + 512]
    );
}

#[test]
fn same_file_and_hardlinks_refused() {
    let files = Files::new();
    let alias = files.directory.join("alias.ffu");
    fs::hard_link(&files.source, &alias).unwrap();
    for target in [&files.source, &alias] {
        assert_error(
            apply_to_file_fixture(
                &files.source,
                target,
                &files.options(),
                None,
                &NEVER_CANCELLED,
            ),
            3,
            "same file",
        );
    }
    fs::remove_file(&alias).unwrap();
    fs::hard_link(&files.target, &alias).unwrap();
    assert_error(files.apply(&files.options()), 3, "hard-link");
    assert_eq!(fs::read(&alias).unwrap(), vec![0xa5; SIZE as usize]);
}

#[cfg(unix)]
#[test]
fn source_target_and_parent_symlinks_refused() {
    use std::os::unix::fs::symlink;
    let files = Files::new();
    let source = files.directory.join("source-link");
    let target = files.directory.join("target-link");
    let parent = files.directory.join("parent-link");
    symlink(&files.source, &source).unwrap();
    symlink(&files.target, &target).unwrap();
    symlink(&files.directory, &parent).unwrap();
    for (source, target) in [
        (source, files.target.clone()),
        (files.source.clone(), target),
        (
            parent.join(files.source.file_name().unwrap()),
            files.target.clone(),
        ),
        (
            files.source.clone(),
            parent.join(files.target.file_name().unwrap()),
        ),
    ] {
        assert!(
            apply_to_file_fixture(&source, &target, &files.options(), None, &NEVER_CANCELLED)
                .is_err()
        );
    }
    assert_eq!(fs::read(&files.target).unwrap(), vec![0xa5; SIZE as usize]);
}

#[test]
fn no_creation_truncation_traversal_or_excessive_depth() {
    let files = Files::new();
    let missing = files.directory.join("not-created");
    assert_error(
        apply_to_file_fixture(
            &files.source,
            &missing,
            &files.options(),
            None,
            &NEVER_CANCELLED,
        ),
        4,
        "stat",
    );
    assert!(!missing.exists());
    let traversal = files
        .directory
        .join("..")
        .join(files.directory.file_name().unwrap())
        .join(files.source.file_name().unwrap());
    assert_error(
        apply_to_file_fixture(
            &traversal,
            &files.target,
            &files.options(),
            None,
            &NEVER_CANCELLED,
        ),
        2,
        "path traversal",
    );
    let depth = files.directory.join("x/".repeat(65)).join("x");
    assert_error(
        open_path(&depth, Kind::Fixture, None),
        3,
        "excessive path depth",
    );
    assert_eq!(fs::metadata(&files.target).unwrap().len(), SIZE);
}

#[test]
fn replaced_node_during_open_is_refused() {
    let files = Files::new();
    let replacement = files.directory.join("replacement");
    fs::write(&replacement, vec![0xa5; SIZE as usize]).unwrap();
    assert_error(
        source::open_path_checked(&files.target, Kind::Fixture, None, || {
            fs::rename(&replacement, &files.target).unwrap();
        }),
        3,
        "identity changed while opening",
    );
    assert_eq!(fs::read(&files.target).unwrap(), vec![0xa5; SIZE as usize]);
}

#[cfg(unix)]
#[test]
fn final_component_symlink_race_is_never_followed() {
    let files = Files::new();
    let moved = files.directory.join("same-inode-but-symlinked");
    assert_error(
        source::open_path_checked(&files.target, Kind::Fixture, None, || {
            fs::rename(&files.target, &moved).unwrap();
            std::os::unix::fs::symlink(&moved, &files.target).unwrap();
        }),
        4,
        "no-follow file/device",
    );
    assert_eq!(fs::read(&moved).unwrap(), vec![0xa5; SIZE as usize]);
}

#[test]
fn fixture_cannot_open_device_or_production_regular_file() {
    let files = Files::new();
    assert_error(
        open_path(&files.target, Kind::Device, None),
        3,
        "regular files are refused",
    );
    // /dev/null is never opened: the regular-file guard rejects its lstat metadata first.
    #[cfg(unix)]
    assert_error(
        open_path(Path::new("/dev/null"), Kind::Fixture, None),
        3,
        "regular file",
    );
    assert_error(identify_target(&files.target), 3, "not a file or symlink");
}

#[test]
fn target_identity_guard() {
    let files = Files::new();
    let mut options = files.options();
    options.expected_target_id = Some("1:2:3".into());
    assert_error(files.apply(&options), 3, "changed since confirmation");
    assert_eq!(fs::read(&files.target).unwrap(), vec![0xa5; SIZE as usize]);
    options.expected_target_id = Some(
        TargetIdentity::from(&unix::metadata(&File::open(&files.target).unwrap()).unwrap())
            .to_string(),
    );
    files.apply(&options).unwrap();
}

#[test]
fn signed_native_ids_and_inode_are_exact() {
    for (device, raw_device) in [(-2088985291, 16777240), (42, -16777240)] {
        let identity = TargetIdentity {
            device,
            inode: 687,
            raw_device,
        };
        assert_eq!(identity.to_string(), format!("{device}:687:{raw_device}"));
        assert!(source::valid_identity(&identity.to_string()));
        let other = TargetIdentity {
            inode: 688,
            ..identity.clone()
        };
        assert_ne!(identity, other);
    }
}

#[test]
fn malformed_prior_identities_fail_without_source_open() {
    let files = Files::new();
    for expected in [
        "", "a:b:c", "1:2", "1:2:3\n", "1:-2:3", "--1:2:3", "1:2:--3",
    ] {
        let mut options = files.options();
        options.expected_target_id = Some(expected.into());
        assert_error(
            apply_ffu(
                &files.directory.join("absent"),
                &files.target,
                &options,
                None,
                &NEVER_CANCELLED,
            ),
            3,
            "identity is malformed",
        );
    }
    for expected in [
        "".into(),
        "a".repeat(63),
        "a".repeat(65),
        "A".repeat(64),
        "g".repeat(64),
        format!("{}\n", "0".repeat(64)),
    ] {
        let mut options = files.options();
        options.expected_sha256 = Some(expected);
        assert_error(
            apply_ffu(
                &files.directory.join("absent"),
                &files.target,
                &options,
                None,
                &NEVER_CANCELLED,
            ),
            2,
            "64 lowercase",
        );
    }
}

#[derive(Default)]
struct FaultIo {
    trace: RefCell<Vec<(&'static str, usize, u64)>>,
    short: bool,
    bad_write: Option<usize>,
    bad_read: Option<usize>,
    omit_writes: bool,
    corrupt_at: Cell<Option<u64>>,
    sync_error: bool,
    close_error: bool,
    geometry: Option<(u64, u64)>,
    metadata_mutation: Cell<Option<&'static str>>,
}

impl TargetIo for FaultIo {
    fn close(&self, file: File) -> Result<()> {
        unix::close(file)?;
        if self.close_error {
            return Err(Error::io(
                "close target",
                io::Error::other("fixture close failed"),
            ));
        }
        Ok(())
    }
    fn metadata(&self, file: &File) -> Result<Metadata> {
        let mut value = unix::metadata(file)?;
        match self.metadata_mutation.get() {
            Some("size") => value.size += 512,
            Some("inode") => value.inode += 1,
            Some("rdev") => value.raw_device += 1,
            Some("links") => value.links += 1,
            Some("mode") => value.mode = 0o060600,
            _ => (),
        }
        Ok(value)
    }
    fn geometry(&self, _file: &File) -> Result<(u64, u64)> {
        Ok(self.geometry.unwrap_or((SIZE, 512)))
    }
    fn write(&self, file: &File, bytes: &[u8], offset: u64) -> io::Result<usize> {
        self.trace.borrow_mut().push(("write", bytes.len(), offset));
        if let Some(count) = self.bad_write {
            return Ok(count);
        }
        if self.omit_writes {
            return Ok(bytes.len());
        }
        unix::write_at(
            file,
            &bytes[..if self.short {
                bytes.len().min(512)
            } else {
                bytes.len()
            }],
            offset,
        )
    }
    fn read(&self, file: &File, bytes: &mut [u8], offset: u64) -> io::Result<usize> {
        self.trace.borrow_mut().push(("read", bytes.len(), offset));
        if let Some(count) = self.bad_read {
            return Ok(count);
        }
        let length = if self.short {
            bytes.len().min(512)
        } else {
            bytes.len()
        };
        let count = unix::read_at(file, &mut bytes[..length], offset)?;
        if self.corrupt_at.get() == Some(offset) && count > 0 {
            bytes[0] ^= 1;
        }
        Ok(count)
    }
    fn sync(&self, file: &File, fixture: bool, readback: bool) -> Result<()> {
        self.trace
            .borrow_mut()
            .push(("sync", usize::from(readback), 0));
        if self.sync_error {
            return Err(Error::io(
                "fsync target",
                io::Error::other("fixture fsync failed"),
            ));
        }
        NativeTargetIo.sync(file, fixture, readback)
    }
}

#[test]
fn aligned_short_io_streams_one_chunk() {
    let files = Files::new();
    let io = FaultIo {
        short: true,
        ..Default::default()
    };
    files.with_io(&files.options(), &io, None).unwrap();
    assert_eq!(fs::read(&files.target).unwrap(), files.expected(SIZE, 0xa5));
    let trace = io.trace.borrow();
    assert_eq!(
        trace.iter().filter(|(kind, _, _)| *kind == "write").count(),
        16
    );
    assert_eq!(
        trace.iter().filter(|(kind, _, _)| *kind == "read").count(),
        14
    );
    assert!(
        trace
            .iter()
            .filter(|(kind, _, _)| *kind != "sync")
            .all(|(_, size, offset)| *size > 0
                && *size <= CHUNK
                && size % 512 == 0
                && offset % 512 == 0)
    );
}

#[test]
fn zero_unaligned_oversized_io_rejected() {
    let files = Files::new();
    for count in [0, 1, CHUNK + 512] {
        let io = FaultIo {
            bad_write: Some(count),
            ..Default::default()
        };
        assert_error(files.with_io(&files.options(), &io, None), 4, "write");
    }
    for count in [0, 1, CHUNK + 512] {
        let io = FaultIo {
            bad_read: Some(count),
            ..Default::default()
        };
        assert_error(files.with_io(&files.options(), &io, None), 4, "readback");
    }
}

#[test]
fn flush_and_reads_share_open_target_and_phase_order() {
    let files = Files::new();
    let io = FaultIo::default();
    files.with_io(&files.options(), &io, None).unwrap();
    let trace = io.trace.borrow();
    let kinds: Vec<_> = trace.iter().map(|event| event.0).collect();
    assert_eq!(kinds.iter().filter(|kind| **kind == "write").count(), 8);
    assert_eq!(kinds.iter().filter(|kind| **kind == "read").count(), 8);
    assert_eq!(kinds.iter().filter(|kind| **kind == "sync").count(), 5);
    assert_eq!(
        &kinds[..kinds.iter().position(|kind| *kind == "write").unwrap()],
        ["read", "read"]
    );
    let last_write = kinds.iter().rposition(|kind| *kind == "write").unwrap();
    assert_eq!(kinds[last_write + 1], "sync");
}

#[test]
fn fsync_failure_never_verifies() {
    let files = Files::new();
    let io = FaultIo {
        sync_error: true,
        ..Default::default()
    };
    let mut events = Vec::new();
    assert_error(
        files.with_io(
            &files.options(),
            &io,
            Some(&mut |phase, percent| events.push((phase.to_string(), percent))),
        ),
        4,
        "fsync failed",
    );
    assert!(!events.contains(&("Verifying".into(), 100)));
}

#[test]
fn close_failure_never_returns_success() {
    let files = Files::new();
    let io = FaultIo {
        close_error: true,
        ..Default::default()
    };
    assert_error(
        files.with_io(&files.options(), &io, None),
        4,
        "close failed",
    );
    assert_eq!(fs::read(&files.target).unwrap(), files.expected(SIZE, 0xa5));
}

#[test]
fn source_mutation_after_preflight_blocks_first_write() {
    let files = Files::new();
    let io = FaultIo::default();
    let mut progress = |phase: &str, percent| {
        if (phase, percent) == ("Writing", 0) {
            let mut blob = files.fixture.blob.clone();
            *blob.last_mut().unwrap() ^= 1;
            fs::write(&files.source, blob).unwrap();
        }
    };
    assert_error(
        files.with_io(&files.options(), &io, Some(&mut progress)),
        4,
        "source image changed",
    );
    assert!(!io.trace.borrow().iter().any(|event| event.0 == "write"));
    assert_eq!(fs::read(&files.target).unwrap(), vec![0xa5; SIZE as usize]);
}

#[test]
fn payload_rehash_even_when_stamp_change_is_hidden() {
    let files = Files::new();
    let mut source = Source::open(&files.source).unwrap();
    let info = parser::parse(&mut source.file, Cancel::new(&NEVER_CANCELLED)).unwrap();
    let mut blob = files.fixture.blob.clone();
    blob[files.fixture.payload_offset] ^= 1;
    fs::write(&files.source, blob).unwrap();
    source.stamp = SourceStamp::from(&unix::metadata(&source.file).unwrap());
    assert_error(
        source.payload(&info, 0, Cancel::new(&NEVER_CANCELLED)),
        4,
        "source payload changed",
    );
}

#[test]
fn source_replacement_does_not_supply_new_readback_source() {
    let files = Files::new();
    let io = FaultIo::default();
    let mut progress = |phase: &str, percent| {
        if (phase, percent) == ("Verifying", 0) {
            let other = files.directory.join("replacement-source");
            fs::write(&other, &files.fixture.blob).unwrap();
            fs::rename(other, &files.source).unwrap();
        }
    };
    assert_error(
        files.with_io(&files.options(), &io, Some(&mut progress)),
        4,
        "source image changed",
    );
}

#[test]
fn readback_guard() {
    let files = Files::new();
    let io = FaultIo::default();
    io.corrupt_at.set(Some(2 * CHUNK as u64));
    assert_error(
        files.with_io(&files.options(), &io, None),
        4,
        "readback mismatch",
    );
}

#[test]
fn late_target_mutation_is_not_verified() {
    let files = Files::new();
    let mut progress = |phase: &str, percent| {
        if (phase, percent) == ("Verifying", 99) {
            let file = OpenOptions::new().write(true).open(&files.target).unwrap();
            unix::write_at(&file, b"X", 0).unwrap();
        }
    };
    assert_error(
        apply_to_file_fixture(
            &files.source,
            &files.target,
            &files.options(),
            Some(&mut progress),
            &NEVER_CANCELLED,
        ),
        4,
        "target changed",
    );
}

#[test]
fn progress_has_distinct_monotonic_write_verify_phases() {
    let files = Files::new();
    let mut events = Vec::new();
    apply_to_file_fixture(
        &files.source,
        &files.target,
        &files.options(),
        Some(&mut |phase, percent| events.push((phase.to_string(), percent))),
        &NEVER_CANCELLED,
    )
    .unwrap();
    assert_eq!(events.first(), Some(&("Writing".into(), 0)));
    assert_eq!(events.last(), Some(&("Verifying".into(), 100)));
    let writing = events
        .iter()
        .position(|event| *event == ("Writing".into(), 100))
        .unwrap();
    let verifying = events
        .iter()
        .position(|event| *event == ("Verifying".into(), 0))
        .unwrap();
    assert!(writing < verifying);
    for phase in ["Writing", "Verifying"] {
        let values: Vec<_> = events
            .iter()
            .filter(|event| event.0 == phase)
            .map(|event| event.1)
            .collect();
        assert!(values.windows(2).all(|pair| pair[0] < pair[1]));
    }
}

#[test]
fn cancelled_write_never_verifies() {
    let files = Files::new();
    let flag = AtomicBool::new(false);
    let mut events = Vec::new();
    let mut progress = |phase: &str, percent| {
        events.push((phase.to_string(), percent));
        if phase == "Writing" && percent > 0 {
            flag.store(true, Ordering::Relaxed);
        }
    };
    assert_error(
        apply_to_file_fixture(
            &files.source,
            &files.target,
            &files.options(),
            Some(&mut progress),
            &flag,
        ),
        130,
        "signal 2",
    );
    assert!(!events.iter().any(|event| event.0 == "Verifying"));
}

#[test]
fn cancelled_readback_never_reports_verified() {
    let files = Files::new();
    let flag = AtomicBool::new(false);
    let mut events = Vec::new();
    let mut progress = |phase: &str, percent| {
        events.push((phase.to_string(), percent));
        if phase == "Verifying" && percent > 0 {
            flag.store(true, Ordering::Relaxed);
        }
    };
    assert_error(
        apply_to_file_fixture(
            &files.source,
            &files.target,
            &files.options(),
            Some(&mut progress),
            &flag,
        ),
        130,
        "signal 2",
    );
    assert!(!events.contains(&("Verifying".into(), 100)));
}

fn create_gpt(size: usize) -> Vec<u8> {
    let mut data = vec![0xa5; size];
    let sectors = (size / 512) as u64;
    let mut table = vec![0; 128 * 128];
    table[..16].copy_from_slice(&[1; 16]);
    table[16..32].copy_from_slice(&[0x11; 16]);
    put64(&mut table, 32, 2048);
    put64(&mut table, 40, sectors - 34);
    let mut mbr = vec![0; 512];
    mbr[450] = 0xee;
    put32(&mut mbr, 454, 1);
    put32(&mut mbr, 458, (sectors - 1) as u32);
    mbr[510..].copy_from_slice(b"\x55\xaa");
    data[..512].copy_from_slice(&mbr);
    for (lba, other, entries) in [(1, sectors - 1, 2), (sectors - 1, 1, sectors - 33)] {
        let mut header = vec![0; 512];
        header[..8].copy_from_slice(b"EFI PART");
        put32(&mut header, 8, 0x10000);
        put32(&mut header, 12, 92);
        put64(&mut header, 24, lba);
        put64(&mut header, 32, other);
        put64(&mut header, 40, 34);
        put64(&mut header, 48, sectors - 34);
        header[56..72].copy_from_slice(&[0x22; 16]);
        put64(&mut header, 72, entries);
        put32(&mut header, 80, 128);
        put32(&mut header, 84, 128);
        put32(&mut header, 88, crc32fast::hash(&table));
        let crc = crc32fast::hash(&header[..92]);
        put32(&mut header, 16, crc);
        data[lba as usize * 512..(lba as usize + 1) * 512].copy_from_slice(&header);
        data[entries as usize * 512..entries as usize * 512 + table.len()].copy_from_slice(&table);
    }
    data
}

fn rewrite_gpt_header(data: &mut [u8], offset: usize) {
    put32(data, offset + 16, 0);
    let crc = crc32fast::hash(&data[offset..offset + 92]);
    put32(data, offset + 16, crc);
}

fn fixture_target<'a>(
    files: &Files,
    size: u64,
    io: &'a dyn TargetIo,
    cancel: Cancel<'a>,
) -> Target<'a> {
    let file = open_path(&files.target, Kind::Fixture, None).unwrap();
    let identity = TargetIdentity::from(&unix::metadata(&file).unwrap());
    Target {
        file,
        size,
        sector: 512,
        fixture: true,
        identity,
        io,
        cancel,
    }
}

#[test]
fn gpt_cleanup_clears_only_bounded_metadata() {
    let files = Files::new();
    let size = 2 * 1024 * 1024;
    let before = create_gpt(size);
    fs::write(&files.target, &before).unwrap();
    let info = files.fixture.parse().unwrap();
    let target = fixture_target(
        &files,
        size as u64,
        &NativeTargetIo,
        Cancel::new(&NEVER_CANCELLED),
    );
    let regions = writer::gpt_cleanup_plan(&target, &info).unwrap();
    assert_eq!(
        regions
            .iter()
            .map(|region| (region.offset, region.original.len()))
            .collect::<Vec<_>>(),
        [
            (0, 512),
            (512, 512),
            (1024, 16384),
            ((size - 33 * 512) as u64, 16384),
            ((size - 512) as u64, 512)
        ]
    );
    drop(target);
    let mut options = files.options();
    options.target_size = size as u64;
    options.allow_gpt_cleanup = true;
    let mut events = Vec::new();
    apply_to_file_fixture(
        &files.source,
        &files.target,
        &options,
        Some(&mut |phase, percent| events.push((phase.to_string(), percent))),
        &NEVER_CANCELLED,
    )
    .unwrap();
    let mut expected = before;
    for region in &regions {
        expected[region.offset as usize..region.offset as usize + region.original.len()].fill(0);
    }
    for extent in resolve_plan(&info, size as u64).unwrap().writes {
        let start = files.fixture.payload_offset + extent.payload_block as usize * CHUNK;
        let length = extent.block_count as usize * CHUNK;
        expected[extent.offset as usize..extent.offset as usize + length]
            .copy_from_slice(&files.fixture.blob[start..start + length]);
    }
    assert_eq!(fs::read(&files.target).unwrap(), expected);
    assert!(events.contains(&("Preparing GPT metadata".into(), 100)));
    assert_eq!(events.last(), Some(&("Verifying".into(), 100)));
}

#[test]
fn gpt_crc_guard() {
    let files = Files::new();
    let size = 2 * 1024 * 1024;
    let mut before = create_gpt(size);
    before[512 + 16] ^= 1;
    fs::write(&files.target, &before).unwrap();
    let mut options = files.options();
    options.target_size = size as u64;
    options.allow_gpt_cleanup = true;
    let io = FaultIo::default();
    assert_error(files.with_io(&options, &io, None), 3, "header checksum");
    assert!(!io.trace.borrow().iter().any(|event| event.0 == "write"));
    assert_eq!(fs::read(&files.target).unwrap(), before);
}

#[test]
fn corrupt_or_unbounded_gpt_never_erased() {
    let files = Files::new();
    let size = 2 * 1024 * 1024;
    let mut options = files.options();
    options.target_size = size as u64;
    options.allow_gpt_cleanup = true;
    for damage in [
        "primary-crc",
        "backup-crc",
        "table-crc",
        "oversized-table",
        "table-in-data",
        "copy-disagreement",
        "missing-primary",
        "missing-backup",
        "old-minimum-header",
        "reserved",
        "revision",
        "size",
        "current-lba",
        "other-lba",
        "first-usable",
        "last-usable",
        "zero-table",
        "small-entry",
        "unaligned-entry",
        "overflow-table-lba",
    ] {
        let mut before = create_gpt(size);
        match damage {
            "primary-crc" => before[512 + 16] ^= 1,
            "backup-crc" => before[size - 512 + 16] ^= 1,
            "table-crc" => before[1024 + 16] ^= 1,
            "oversized-table" => put32(&mut before, 512 + 80, 100000),
            "table-in-data" => put64(&mut before, 512 + 72, 2048),
            "copy-disagreement" => {
                before[size - 512 + 56] ^= 1;
                rewrite_gpt_header(&mut before, size - 512);
            }
            "missing-primary" => before[512..520].fill(0),
            "missing-backup" => before[size - 512..size - 504].fill(0),
            "old-minimum-header" => {
                before[SIZE as usize - 512..SIZE as usize - 504].copy_from_slice(b"EFI PART")
            }
            "reserved" => put32(&mut before, 512 + 20, 1),
            "revision" => put32(&mut before, 512 + 8, 0x20000),
            "size" => put32(&mut before, 512 + 12, 513),
            "current-lba" => put64(&mut before, 512 + 24, 2),
            "other-lba" => put64(&mut before, 512 + 32, 3),
            "first-usable" => put64(&mut before, 512 + 40, 2),
            "last-usable" => put64(&mut before, 512 + 48, u64::MAX),
            "zero-table" => put32(&mut before, 512 + 80, 0),
            "small-entry" => put32(&mut before, 512 + 84, 64),
            "unaligned-entry" => put32(&mut before, 512 + 84, 129),
            "overflow-table-lba" => put64(&mut before, 512 + 72, u64::MAX),
            _ => unreachable!(),
        }
        if !["primary-crc", "backup-crc"].contains(&damage) {
            rewrite_gpt_header(&mut before, 512);
        }
        fs::write(&files.target, &before).unwrap();
        let io = FaultIo::default();
        let error = files.with_io(&options, &io, None).unwrap_err();
        assert_eq!(error.exit_status(), 3, "{damage}: {error}");
        assert!(
            !io.trace.borrow().iter().any(|event| event.0 == "write"),
            "{damage}"
        );
        assert_eq!(fs::read(&files.target).unwrap(), before, "{damage}");
    }
}

#[test]
fn gpt_partition_bounds_overlap_and_table_copy_integrity() {
    let files = Files::new();
    let size = 2 * 1024 * 1024;
    let mut options = files.options();
    options.target_size = size as u64;
    options.allow_gpt_cleanup = true;
    for damage in ["outside", "overlap", "table-copy"] {
        let mut before = create_gpt(size);
        let mut table = before[1024..1024 + 16384].to_vec();
        if damage == "outside" {
            put64(&mut table, 32, 1);
        } else if damage == "overlap" {
            table[128..144].fill(2);
            put64(&mut table, 160, 2048);
            put64(&mut table, 168, 2049);
        } else {
            table[16] ^= 1;
        }
        before[1024..1024 + 16384].copy_from_slice(&table);
        if damage != "table-copy" {
            before[size - 33 * 512..size - 512].copy_from_slice(&table);
        }
        for offset in [512, size - 512] {
            put32(&mut before, offset + 88, crc32fast::hash(&table));
            rewrite_gpt_header(&mut before, offset);
        }
        fs::write(&files.target, &before).unwrap();
        let io = FaultIo::default();
        assert!(files.with_io(&options, &io, None).is_err(), "{damage}");
        assert!(!io.trace.borrow().iter().any(|event| event.0 == "write"));
        assert_eq!(fs::read(&files.target).unwrap(), before);
    }
}

#[test]
fn gpt_metadata_change_after_plan_blocks_all_writes() {
    let files = Files::new();
    let size = 2 * 1024 * 1024;
    fs::write(&files.target, create_gpt(size)).unwrap();
    let io = FaultIo::default();
    let target = fixture_target(&files, size as u64, &io, Cancel::new(&NEVER_CANCELLED));
    let info = files.fixture.parse().unwrap();
    let source = Source::open(&files.source).unwrap();
    let regions = writer::gpt_cleanup_plan(&target, &info).unwrap();
    unix::write_at(&target.file, b"\xff", 1024 + 16).unwrap();
    assert_error(
        writer::clear_gpt_metadata(&source, &target, &regions, &mut None),
        3,
        "changed during preflight",
    );
    assert!(!io.trace.borrow().iter().any(|event| event.0 == "write"));
}

#[test]
fn source_change_immediately_before_gpt_cleanup_no_writes() {
    let files = Files::new();
    let size = 2 * 1024 * 1024;
    let before = create_gpt(size);
    fs::write(&files.target, &before).unwrap();
    let mut options = files.options();
    options.target_size = size as u64;
    options.allow_gpt_cleanup = true;
    let io = FaultIo::default();
    let mut progress = |phase: &str, percent| {
        if (phase, percent) == ("Preparing GPT metadata", 0) {
            fs::write(&files.source, b"changed").unwrap();
        }
    };
    assert_error(
        files.with_io(&options, &io, Some(&mut progress)),
        4,
        "source image changed",
    );
    assert!(!io.trace.borrow().iter().any(|event| event.0 == "write"));
    assert_eq!(fs::read(&files.target).unwrap(), before);
}

#[test]
fn cleanup_readback_failure_and_interrupt_cannot_verify() {
    let files = Files::new();
    let size = 2 * 1024 * 1024;
    let before = create_gpt(size);
    fs::write(&files.target, &before).unwrap();
    let mut options = files.options();
    options.target_size = size as u64;
    options.allow_gpt_cleanup = true;
    let io = FaultIo {
        omit_writes: true,
        ..Default::default()
    };
    assert_error(
        files.with_io(&options, &io, None),
        4,
        "GPT cleanup readback failed",
    );
    assert_eq!(fs::read(&files.target).unwrap(), before);
    let flag = AtomicBool::new(false);
    let mut events = Vec::new();
    let mut progress = |phase: &str, percent| {
        events.push((phase.to_string(), percent));
        if phase == "Preparing GPT metadata" && percent > 0 {
            flag.store(true, Ordering::Relaxed);
        }
    };
    assert_error(
        apply_to_file_fixture(
            &files.source,
            &files.target,
            &options,
            Some(&mut progress),
            &flag,
        ),
        130,
        "signal 2",
    );
    assert!(
        !events
            .iter()
            .any(|event| event.0 == "Writing" || event.0 == "Verifying")
    );
}

#[test]
fn gpt_cleanup_readback_guard() {
    let files = Files::new();
    let size = 2 * 1024 * 1024;
    fs::write(&files.target, create_gpt(size)).unwrap();
    let mut options = files.options();
    options.target_size = size as u64;
    options.allow_gpt_cleanup = true;
    let io = FaultIo::default();
    let mut progress = |phase: &str, _percent| {
        if phase == "Verifying" {
            io.corrupt_at.set(Some((size - 33 * 512) as u64));
        }
    };
    assert_error(
        files.with_io(&options, &io, Some(&mut progress)),
        4,
        "old GPT metadata reappeared",
    );
}

#[test]
fn full_wipe_guard_zeroes_every_sector_and_tail_before_image() {
    let files = Files::new();
    let size = WIPE_CHUNK_BYTES + 512;
    fs::write(&files.target, vec![0xa5; size]).unwrap();
    let mut options = files.options();
    options.target_size = size as u64;
    options.wipe_entire_drive = true;
    let mut events = Vec::new();
    let mut progress = |phase: &str, percent| {
        events.push((phase.to_string(), percent));
        if (phase, percent) == ("Verifying blank drive", 100) {
            assert!(
                fs::read(&files.target)
                    .unwrap()
                    .iter()
                    .all(|byte| *byte == 0)
            );
        }
    };
    apply_to_file_fixture(
        &files.source,
        &files.target,
        &options,
        Some(&mut progress),
        &NEVER_CANCELLED,
    )
    .unwrap();
    assert_eq!(
        fs::read(&files.target).unwrap(),
        files.expected(size as u64, 0)
    );
    assert_eq!(events.last(), Some(&("Verifying".into(), 100)));
    for (before, after) in [
        (
            ("Wiping entire drive".into(), 100),
            ("Verifying blank drive".into(), 0),
        ),
        (("Verifying blank drive".into(), 100), ("Writing".into(), 0)),
    ] {
        assert!(
            events.iter().position(|event| *event == before).unwrap()
                < events.iter().position(|event| *event == after).unwrap()
        );
    }
    assert_eq!(fs::read(&files.source).unwrap(), files.fixture.blob);
}

#[test]
fn only_explicit_full_wipe_discards_malformed_gpt() {
    let files = Files::new();
    let size = 2 * 1024 * 1024;
    let mut before = create_gpt(size);
    before[512 + 16] ^= 1;
    fs::write(&files.target, &before).unwrap();
    let mut options = files.options();
    options.target_size = size as u64;
    options.allow_gpt_cleanup = true;
    assert!(files.apply(&options).is_err());
    assert_eq!(fs::read(&files.target).unwrap(), before);
    options.allow_gpt_cleanup = false;
    options.wipe_entire_drive = true;
    files.apply(&options).unwrap();
    assert_eq!(
        fs::read(&files.target).unwrap(),
        files.expected(size as u64, 0)
    );
}

#[test]
fn wipe_readback_guard_prevents_image_application() {
    let files = Files::new();
    let io = FaultIo::default();
    io.corrupt_at.set(Some(0));
    let mut options = files.options();
    options.wipe_entire_drive = true;
    assert_error(
        files.with_io(&options, &io, None),
        4,
        "full-wipe readback mismatch",
    );
    assert_eq!(fs::read(&files.target).unwrap(), vec![0; SIZE as usize]);
    assert_eq!(
        io.trace
            .borrow()
            .iter()
            .filter(|event| event.0 == "write")
            .count(),
        1
    );
}

#[test]
fn source_geometry_and_identity_changes_precede_first_wipe_write() {
    let files = Files::new();
    let mut options = files.options();
    options.wipe_entire_drive = true;
    for change in ["source", "size", "inode", "rdev", "links", "mode"] {
        fs::write(&files.source, &files.fixture.blob).unwrap();
        fs::write(&files.target, vec![0xa5; SIZE as usize]).unwrap();
        let io = FaultIo::default();
        let mut progress = |phase: &str, percent| {
            if (phase, percent) == ("Wiping entire drive", 0) {
                if change == "source" {
                    fs::write(&files.source, b"changed").unwrap();
                } else {
                    io.metadata_mutation.set(Some(change));
                }
            }
        };
        assert!(
            files.with_io(&options, &io, Some(&mut progress)).is_err(),
            "{change}"
        );
        assert!(
            !io.trace.borrow().iter().any(|event| event.0 == "write"),
            "{change}"
        );
        assert_eq!(fs::read(&files.target).unwrap(), vec![0xa5; SIZE as usize]);
    }
}

#[test]
fn interrupted_wipe_never_reaches_image_or_completion() {
    let files = Files::new();
    let size = WIPE_CHUNK_BYTES + 512;
    fs::write(&files.target, vec![0xa5; size]).unwrap();
    let mut options = files.options();
    options.target_size = size as u64;
    options.wipe_entire_drive = true;
    let flag = AtomicBool::new(false);
    let mut events = Vec::new();
    let mut progress = |phase: &str, percent| {
        events.push((phase.to_string(), percent));
        if phase == "Wiping entire drive" && percent > 0 {
            flag.store(true, Ordering::Relaxed);
        }
    };
    assert_error(
        apply_to_file_fixture(
            &files.source,
            &files.target,
            &options,
            Some(&mut progress),
            &flag,
        ),
        130,
        "signal 2",
    );
    assert!(
        !events
            .iter()
            .any(|event| event.0 == "Writing" || event.0 == "Verifying" || event.1 == 100)
    );
    assert_eq!(
        &fs::read(&files.target).unwrap()[size - 512..],
        &[0xa5; 512]
    );
}

#[test]
fn wipe_verification_concurrent_target_change_fails() {
    let files = Files::new();
    let mut options = files.options();
    options.wipe_entire_drive = true;
    let mut progress = |phase: &str, percent| {
        if (phase, percent) == ("Verifying blank drive", 99) {
            let target = OpenOptions::new().write(true).open(&files.target).unwrap();
            unix::write_at(&target, b"X", 0).unwrap();
        }
    };
    assert_error(
        apply_to_file_fixture(
            &files.source,
            &files.target,
            &options,
            Some(&mut progress),
            &NEVER_CANCELLED,
        ),
        4,
        "target changed during full-wipe",
    );
}

#[test]
fn production_cleanup_and_wipe_need_both_identities() {
    let files = Files::new();
    for wipe in [true, false] {
        for (hash, identity) in [
            (None, None),
            (Some("1".repeat(64)), None),
            (None, Some("-1:2:3".into())),
        ] {
            let options = ApplyOptions {
                target_size: SIZE,
                expected_sha256: hash,
                expected_target_id: identity,
                allow_gpt_cleanup: !wipe,
                wipe_entire_drive: wipe,
                ..Default::default()
            };
            assert_error(
                apply_ffu(
                    &files.directory.join("absent"),
                    &files.target,
                    &options,
                    None,
                    &NEVER_CANCELLED,
                ),
                3,
                "verified image digest and confirmed target identity",
            );
        }
    }
    let mut options = files.options();
    options.allow_gpt_cleanup = true;
    options.wipe_entire_drive = true;
    assert_error(files.apply(&options), 3, "not both");
}

#[test]
fn native_geometry_and_cache_protocol_without_devices() {
    struct Queries {
        expected: RefCell<Vec<(u64, usize, u64)>>,
        commands: RefCell<Vec<u64>>,
    }
    impl Ioctl for Queries {
        fn number(&self, _file: &File, request: u64, width: usize) -> Result<u64> {
            let (expected, size, value) = self.expected.borrow_mut().remove(0);
            assert_eq!((request, width), (expected, size));
            Ok(value)
        }
        fn command(&self, _file: &File, request: u64) -> Result<()> {
            self.commands.borrow_mut().push(request);
            Ok(())
        }
    }
    let files = Files::new();
    let file = File::open(&files.target).unwrap();
    for (host, expected) in [
        (Host::Macos, vec![(0x40046418, 4, 512), (0x40086419, 8, 64)]),
        (
            Host::Linux,
            vec![
                (0x80001272 | ((size_of::<usize>() as u64) << 16), 8, SIZE),
                (0x1268, 4, 512),
            ],
        ),
    ] {
        let ioctl = Queries {
            expected: RefCell::new(expected),
            commands: RefCell::new(Vec::new()),
        };
        assert_eq!(
            unix::geometry_with(&file, host, &ioctl).unwrap(),
            (SIZE, 512)
        );
        assert!(ioctl.expected.borrow().is_empty());
        for (fixture, readback) in [(false, false), (false, true), (true, true)] {
            ioctl.commands.borrow_mut().clear();
            unix::cache_sync_with(&file, host, fixture, readback, &ioctl).unwrap();
            let expected = match (host, fixture, readback) {
                (Host::Macos, false, _) => vec![0x20006416],
                (Host::Linux, false, true) => vec![0x1261],
                _ => vec![],
            };
            assert_eq!(*ioctl.commands.borrow(), expected);
        }
    }
    assert!(
        unix::geometry_with(
            &file,
            Host::Unsupported,
            &Queries {
                expected: RefCell::default(),
                commands: RefCell::default()
            }
        )
        .is_err()
    );
}

#[test]
fn cache_capability_failure_and_4kn_before_any_write() {
    let files = Files::new();
    let mut source = Source::open(&files.source).unwrap();
    let info = parser::parse(&mut source.file, Cancel::new(&NEVER_CANCELLED)).unwrap();
    let plan = resolve_plan(&info, SIZE).unwrap();
    for (geometry, sync_error) in [
        ((SIZE + 512, 512), false),
        ((SIZE, 4096), false),
        ((SIZE, 512), true),
    ] {
        let io = FaultIo {
            geometry: Some(geometry),
            sync_error,
            ..Default::default()
        };
        let mut target = fixture_target(&files, SIZE, &io, Cancel::new(&NEVER_CANCELLED));
        target.fixture = false;
        assert!(
            apply_plan(
                &mut source,
                &info,
                &plan,
                &target,
                &files.options(),
                &[],
                &mut None
            )
            .is_err()
        );
        assert!(!io.trace.borrow().iter().any(|event| event.0 == "write"));
    }
}

#[test]
fn actual_geometry_guard() {
    let files = Files::new();
    for geometry in [(SIZE + 512, 512), (SIZE, 4096)] {
        let io = FaultIo {
            geometry: Some(geometry),
            ..Default::default()
        };
        let mut target = fixture_target(&files, SIZE, &io, Cancel::new(&NEVER_CANCELLED));
        target.fixture = false;
        assert_error(target.check(), 3, "actual target");
        assert!(io.trace.borrow().is_empty());
    }
}

#[test]
fn native_host_and_whole_raw_device_guards() {
    for release in ["6.6.87.2-microsoft-standard-WSL2", "wsl", "Microsoft"] {
        assert_error(writer::check_host(Host::Linux, release), 3, "WSL");
    }
    writer::check_host(Host::Linux, "6.8.0-generic").unwrap();
    writer::check_host(Host::Macos, "").unwrap();
    assert_error(
        writer::check_host(Host::Unsupported, ""),
        3,
        "native Linux/macOS",
    );
    let files = Files::new();
    let mut metadata = unix::metadata(&File::open(&files.target).unwrap()).unwrap();
    metadata.mode = 0o020600;
    assert!(source::valid_device(
        &metadata,
        Path::new("/dev/rdisk99"),
        Host::Macos
    ));
    for path in [
        "/dev/disk99",
        "/dev/rdisk99s1",
        "/dev/rdisk",
        "/dev/rdisk99\n",
        "/other/rdisk99",
    ] {
        assert!(
            !source::valid_device(&metadata, Path::new(path), Host::Macos),
            "{path}"
        );
    }
    metadata.mode = 0o060600;
    assert!(source::valid_device(
        &metadata,
        Path::new("/dev/mock-device"),
        Host::Linux
    ));
    assert!(!source::valid_device(
        &metadata,
        Path::new("/dev/mock-device"),
        Host::Unsupported
    ));
}

struct HdmiFixture {
    fixture: Fixture,
    disk: Vec<u8>,
}
impl HdmiFixture {
    const BOOT: usize = 2 * 512;
    const FAT: usize = Self::BOOT + 512;
    const ROOT: usize = Self::BOOT + 65 * 512;
    const DATA: usize = Self::BOOT + 97 * 512;
    const SIZE: u64 = 8 * 1024 * 1024;
    fn new() -> Self {
        let manifest = "\
[FullFlash]
OSVersion=10.0.17763.107
Version=2.0
UEFI=True
DevicePlatformId0=Broadcom.RPi.2
DevicePlatformId1=*
[Store]
MinSectorCount=16384
SectorSize=512
IsMainOSStore=True
StoreType=Default
OnlyAllocateDefinedGptEntries=False
[Partition]
Name=NonWindowsFixture
TotalSectors=8192
";
        let fixture = Fixture::build(
            vec![(128, vec![(0, 0)])],
            [0; 6],
            manifest,
            &["Broadcom.RPi.2", "*"],
        );
        let mut disk = vec![0; 128 * CHUNK];
        disk[450] = 0x0c;
        put32(&mut disk, 454, 2);
        put32(&mut disk, 458, 8192);
        disk[510..512].copy_from_slice(b"\x55\xaa");
        put16(&mut disk, Self::BOOT + 11, 512);
        disk[Self::BOOT + 13] = 1;
        put16(&mut disk, Self::BOOT + 14, 1);
        disk[Self::BOOT + 16] = 2;
        put16(&mut disk, Self::BOOT + 17, 512);
        put16(&mut disk, Self::BOOT + 19, 8192);
        disk[Self::BOOT + 21] = 0xf8;
        put16(&mut disk, Self::BOOT + 22, 32);
        disk[Self::BOOT + 54..Self::BOOT + 62].copy_from_slice(b"FAT16   ");
        disk[Self::BOOT + 510..Self::BOOT + 512].copy_from_slice(b"\x55\xaa");
        for fat in [Self::FAT, Self::FAT + 32 * 512] {
            for (index, value) in [0xfff8, 0xffff, 3, 0xffff, 0xffff].into_iter().enumerate() {
                put16(&mut disk, fat + index * 2, value);
            }
        }
        let text = format!(
            "kernel_old=1\r\ngpu_mem=32\r\nframebuffer_ignore_alpha=1\r\nframebuffer_swap=1\r\n\
disable_overscan=1\r\nhdmi_force_hotplug=1 # Original comment\r\nhdmi_group=2\r\n\
hdmi_cvt 800 480 60 6 0 0 0\r\n{}",
            "# Non-Windows firmware fixture\r\n".repeat(16)
        );
        disk[Self::ROOT..Self::ROOT + 11].copy_from_slice(b"CONFIG  TXT");
        disk[Self::ROOT + 11] = 0x20;
        put16(&mut disk, Self::ROOT + 26, 2);
        put32(&mut disk, Self::ROOT + 28, text.len() as u32);
        disk[Self::ROOT + 32..Self::ROOT + 43].copy_from_slice(b"KERNEL  IMG");
        disk[Self::ROOT + 43] = 0x20;
        put16(&mut disk, Self::ROOT + 58, 4);
        put32(&mut disk, Self::ROOT + 60, 512);
        disk[Self::DATA..Self::DATA + text.len()].copy_from_slice(text.as_bytes());
        disk[Self::DATA + 1024..Self::DATA + 1024 + 17].copy_from_slice(b"NOT AN EXECUTABLE");
        let mut result = Self { fixture, disk };
        result.refresh();
        result
    }
    fn refresh(&mut self) {
        let mut blob = self.fixture.blob.clone();
        blob[self.fixture.payload_offset..].copy_from_slice(&self.disk);
        self.fixture.blob = self.fixture.rehash(blob);
    }
    fn install(&self, files: &Files) {
        fs::write(&files.source, &self.fixture.blob).unwrap();
        OpenOptions::new()
            .write(true)
            .open(&files.target)
            .unwrap()
            .set_len(Self::SIZE)
            .unwrap();
    }
    fn options(mode: &str, custom: &str) -> ApplyOptions {
        ApplyOptions {
            target_size: Self::SIZE,
            hdmi_mode: mode.into(),
            hdmi_config: custom.into(),
            ..Default::default()
        }
    }
    fn config(files: &Files) -> Vec<u8> {
        let data = fs::read(&files.target).unwrap();
        let size = u32_at(&data, Self::ROOT + 28) as usize;
        data[Self::DATA..Self::DATA + size].to_vec()
    }
}

#[test]
fn hdmi_official_is_unchanged_and_does_not_require_boot_filesystem() {
    let files = Files::new();
    let hdmi = HdmiFixture::new();
    hdmi.install(&files);
    files.apply(&HdmiFixture::options("official", "")).unwrap();
    assert_eq!(
        &fs::read(&files.target).unwrap()[..hdmi.disk.len()],
        hdmi.disk
    );
    assert_eq!(fs::read(&files.source).unwrap(), hdmi.fixture.blob);
    let options = hdmi_options("official", "").unwrap();
    assert!(options.settings.is_empty());
    assert!(!options.recommended);
    assert!(!options.label.contains("Recommended"));
    let recommended: Vec<_> = options
        .choices
        .iter()
        .filter(|choice| choice.recommended)
        .collect();
    assert_eq!(recommended.len(), 1);
    assert_eq!(recommended[0].value, "720p60");
    assert!(recommended[0].menu_label.contains("Recommended"));
}

#[test]
fn hdmi_derivation_changes_only_config_allocation_and_directory_size() {
    let files = Files::new();
    let hdmi = HdmiFixture::new();
    hdmi.install(&files);
    for (mode, custom, expected) in [
        ("720p60", "", "hdmi_mode=4\r\n"),
        ("1080p60", "", "hdmi_mode=16\r\n"),
        (
            "custom",
            "# Reviewed DMT timing\nhdmi_group=2\nhdmi_mode=16\nhdmi_drive=1",
            "hdmi_drive=1\r\n",
        ),
    ] {
        files.apply(&HdmiFixture::options(mode, custom)).unwrap();
        let config = String::from_utf8(HdmiFixture::config(&files)).unwrap();
        assert!(config.contains(expected), "{mode}");
        for retained in [
            "kernel_old=1",
            "gpu_mem=32",
            "framebuffer_ignore_alpha=1",
            "framebuffer_swap=1",
            "hdmi_cvt 800 480",
        ] {
            assert!(config.contains(retained), "{mode}: {retained}");
        }
        assert_eq!(config.matches("hdmi_group=").count(), 1);
        assert_eq!(config.matches("hdmi_mode=").count(), 1);
        assert_eq!(fs::read(&files.source).unwrap(), hdmi.fixture.blob);
        let mut disk = fs::read(&files.target).unwrap()[..hdmi.disk.len()].to_vec();
        disk[HdmiFixture::DATA..HdmiFixture::DATA + 1024]
            .copy_from_slice(&hdmi.disk[HdmiFixture::DATA..HdmiFixture::DATA + 1024]);
        disk[HdmiFixture::ROOT + 28..HdmiFixture::ROOT + 32]
            .copy_from_slice(&hdmi.disk[HdmiFixture::ROOT + 28..HdmiFixture::ROOT + 32]);
        assert_eq!(disk, hdmi.disk, "{mode}");
    }
}

#[test]
fn hdmi_invalid_choices_precede_source_and_target_access() {
    let files = Files::new();
    let mut cases: Vec<(&str, String)> = [
        ("unknown", ""),
        ("custom", ""),
        ("custom", "kernel_old=0"),
        ("custom", "hdmi_group=1\nhdmi_mode=4\nkernel_old=0"),
        ("custom", "hdmi_group=1\nhdmi_mode=4\ninclude malicious.txt"),
        ("custom", "hdmi_group=1\nhdmi_mode=4\nhdmi_mode=16"),
        ("custom", "hdmi_group=0\nhdmi_mode=4"),
        ("custom", "hdmi_group=2\nhdmi_mode=87"),
        ("custom", "hdmi_group=1\nhdmi_mode=4\nhdmi_drive=3"),
        ("custom", "hdmi_group=1\nhdmi_mode=4\nhdmi_force_hotplug=2"),
        ("custom", "hdmi_group=1\nhdmi_mode=4\n[all]"),
        ("custom", "# é"),
        ("custom", "\0"),
    ]
    .into_iter()
    .map(|(mode, text)| (mode, text.to_string()))
    .collect();
    cases.push(("custom", "#".repeat(513)));
    for (mode, text) in cases {
        assert_eq!(
            apply_to_file_fixture(
                &files.directory.join("absent"),
                &files.directory.join("missing"),
                &HdmiFixture::options(mode, &text),
                None,
                &NEVER_CANCELLED
            )
            .unwrap_err()
            .exit_status(),
            2,
            "{mode}/{text}"
        );
    }
    let text = "hdmi_group=1\nhdmi_mode=4\n#";
    let boundary = format!("{text}{}", "x".repeat(512 - text.len()));
    assert_eq!(
        hdmi_options("custom", &boundary).unwrap().settings,
        "hdmi_group=1\nhdmi_mode=4\n"
    );
    assert!(hdmi_options("custom", &(boundary + "x")).is_err());
    for (group, maximum) in [(1, 64), (2, 86)] {
        for mode in [1, maximum] {
            hdmi_options("custom", &format!("hdmi_group={group}\nhdmi_mode={mode}")).unwrap();
        }
        assert!(hdmi_options("custom", &format!("hdmi_group={group}\nhdmi_mode=0")).is_err());
    }
    assert_eq!(
        hdmi_options("custom", "hdmi_group=01\nhdmi_mode=0004")
            .unwrap()
            .settings,
        "hdmi_group=1\nhdmi_mode=4\n"
    );
}

#[test]
fn hdmi_bad_filesystem_or_allocation_never_writes() {
    let files = Files::new();
    for mutation in [
        "mbr",
        "mbr-signature",
        "partition-zero",
        "partition-range",
        "fat-copy",
        "cycle",
        "missing",
        "duplicate",
        "oversized",
        "filtered",
        "short-chain",
        "zero-size",
        "high-cluster",
        "attributes",
        "sector",
        "cluster-size",
        "reserved",
        "roots",
        "fat-size",
        "total",
        "clusters",
        "non-ascii",
        "nul",
        "boot-signature",
        "fat-label",
        "fat-count",
    ] {
        let mut hdmi = HdmiFixture::new();
        match mutation {
            "mbr" => hdmi.disk[450] = 0xee,
            "mbr-signature" => hdmi.disk[510] = 0,
            "partition-zero" => put32(&mut hdmi.disk, 454, 0),
            "partition-range" => put32(&mut hdmi.disk, 458, u32::MAX),
            "fat-copy" => hdmi.disk[HdmiFixture::FAT + 32 * 512 + 4] ^= 1,
            "cycle" | "short-chain" => {
                for fat in [HdmiFixture::FAT, HdmiFixture::FAT + 32 * 512] {
                    put16(
                        &mut hdmi.disk,
                        fat + 4,
                        if mutation == "cycle" { 2 } else { 0xffff },
                    );
                }
            }
            "missing" => hdmi.disk[HdmiFixture::ROOT] = 0xe5,
            "duplicate" => {
                let entry = hdmi.disk[HdmiFixture::ROOT..HdmiFixture::ROOT + 32].to_vec();
                hdmi.disk[HdmiFixture::ROOT + 32..HdmiFixture::ROOT + 64].copy_from_slice(&entry);
            }
            "oversized" => put32(&mut hdmi.disk, HdmiFixture::ROOT + 28, 4097),
            "filtered" => {
                hdmi.disk[HdmiFixture::DATA..HdmiFixture::DATA + 6].copy_from_slice(b"[all]\n")
            }
            "zero-size" => put32(&mut hdmi.disk, HdmiFixture::ROOT + 28, 0),
            "high-cluster" => put16(&mut hdmi.disk, HdmiFixture::ROOT + 20, 1),
            "attributes" => hdmi.disk[HdmiFixture::ROOT + 11] = 0x10,
            "sector" => put16(&mut hdmi.disk, HdmiFixture::BOOT + 11, 4096),
            "cluster-size" => hdmi.disk[HdmiFixture::BOOT + 13] = 3,
            "reserved" => put16(&mut hdmi.disk, HdmiFixture::BOOT + 14, 129),
            "roots" => put16(&mut hdmi.disk, HdmiFixture::BOOT + 17, 4097),
            "fat-size" => put16(&mut hdmi.disk, HdmiFixture::BOOT + 22, 0),
            "total" => put16(&mut hdmi.disk, HdmiFixture::BOOT + 19, 8191),
            "clusters" => {
                put32(&mut hdmi.disk, 458, 64);
                put16(&mut hdmi.disk, HdmiFixture::BOOT + 19, 64);
            }
            "non-ascii" => hdmi.disk[HdmiFixture::DATA] = 0xff,
            "nul" => hdmi.disk[HdmiFixture::DATA] = 0,
            "boot-signature" => hdmi.disk[HdmiFixture::BOOT + 510] = 0,
            "fat-label" => hdmi.disk[HdmiFixture::BOOT + 54] = b'X',
            "fat-count" => hdmi.disk[HdmiFixture::BOOT + 16] = 1,
            _ => unreachable!(),
        }
        hdmi.refresh();
        hdmi.install(&files);
        let io = FaultIo::default();
        let error = files
            .with_io(&HdmiFixture::options("720p60", ""), &io, None)
            .unwrap_err();
        assert_eq!(error.exit_status(), 2, "{mutation}: {error}");
        assert!(
            io.trace.borrow().is_empty(),
            "target accessed for {mutation}"
        );
    }
}

#[test]
fn hdmi_configured_block_readback_guard() {
    let files = Files::new();
    let hdmi = HdmiFixture::new();
    hdmi.install(&files);
    let io = FaultIo::default();
    io.corrupt_at
        .set(Some((HdmiFixture::DATA / CHUNK * CHUNK) as u64));
    assert_error(
        files.with_io(&HdmiFixture::options("720p60", ""), &io, None),
        4,
        "readback mismatch",
    );
}

#[test]
fn hdmi_source_integrity_required_and_no_extra_allocation() {
    let files = Files::new();
    let hdmi = HdmiFixture::new();
    hdmi.install(&files);
    let mut blob = hdmi.fixture.blob.clone();
    blob[hdmi.fixture.payload_offset + HdmiFixture::DATA] ^= 1;
    fs::write(&files.source, blob).unwrap();
    let io = FaultIo::default();
    assert_error(
        files.with_io(&HdmiFixture::options("720p60", ""), &io, None),
        2,
        "chunk hash",
    );
    assert!(io.trace.borrow().is_empty());
    let mut hdmi = HdmiFixture::new();
    let text = format!("#{}\r\nkernel_old=1\r\nhdmi_group=2\r\n", "x".repeat(937));
    hdmi.disk[HdmiFixture::DATA..HdmiFixture::DATA + text.len()].copy_from_slice(text.as_bytes());
    put32(&mut hdmi.disk, HdmiFixture::ROOT + 28, text.len() as u32);
    hdmi.refresh();
    hdmi.install(&files);
    assert_error(
        files.with_io(
            &HdmiFixture::options(
                "custom",
                "hdmi_group=1\nhdmi_mode=4\nhdmi_force_hotplug=1\nhdmi_drive=2",
            ),
            &io,
            None,
        ),
        2,
        "exceed the existing",
    );
    assert!(io.trace.borrow().is_empty());
}

#[test]
fn full_wipe_keeps_verified_hdmi_overlay() {
    let files = Files::new();
    let hdmi = HdmiFixture::new();
    hdmi.install(&files);
    fs::write(&files.target, vec![0xa5; HdmiFixture::SIZE as usize]).unwrap();
    let mut options = HdmiFixture::options("720p60", "");
    options.wipe_entire_drive = true;
    files.apply(&options).unwrap();
    let config = String::from_utf8(HdmiFixture::config(&files)).unwrap();
    assert!(config.contains("hdmi_mode=4\r\n"));
    assert!(config.contains("kernel_old=1\r\n"));
    assert!(
        fs::read(&files.target).unwrap()[hdmi.disk.len()..]
            .iter()
            .all(|byte| *byte == 0)
    );
    assert_eq!(fs::read(&files.source).unwrap(), hdmi.fixture.blob);
}

#[test]
fn bundled_provenance_pinned_and_no_runtime_override() {
    let raw = fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../src/config/iot-core-images.json"),
    )
    .unwrap();
    assert_eq!(raw, trust::PROVENANCE);
    assert_eq!(format!("{:x}", Sha256::digest(&raw)), PROVENANCE_SHA256);
    let approved = trust::approved_images().unwrap();
    assert_eq!(approved.len(), 1);
    let approved = &approved[0];
    assert_eq!(
        approved.sha256,
        "15e9451eb7b3e5645e89e25ffc846d4dfd4b4a8c9c63600e177cb16817da67f5"
    );
    assert_eq!(approved.platform_ids, ["Broadcom.RPi.2", "*"]);
    assert_eq!(approved.architecture, "arm32");
    let mut modified = raw;
    modified.push(b' ');
    assert_error(trust::tampered_provenance(&modified), 2, "modified");
    assert_error(
        trust::tampered_provenance(&vec![0; MAX_MANIFEST_BYTES + 1]),
        2,
        "untrusted",
    );
}

#[test]
fn reviewed_profile_is_not_source_verification() {
    for board in ["pi2-v1.1", "pi2-v1.2", "pi3-b"] {
        let profile = image_profile(board).unwrap();
        assert_eq!(profile.source_verification, "not-assessed");
        assert_eq!(profile.trust, "pinned-bundled-profile");
        assert_eq!(profile.minimum_disk_bytes, 3774873600);
        assert_eq!(profile.expected_build, "17763.107");
        assert!(profile.recommended_boards.iter().any(|item| item == board));
        let json = serde_json::to_value(profile).unwrap();
        assert!(json.get("sha256").is_none());
        assert!(json.get("source_file").is_none());
    }
    for board in ["pi3-b-plus", "pi4-b", "pi5", "", "unknown"] {
        assert_error(image_profile(board), 2, "exactly one");
    }
}

#[test]
fn approval_checks_arch_profile_digest_platform_board_build_and_all_geometry() {
    let fixture = Fixture::new();
    let info = fixture.parse().unwrap();
    let approved = fixture.approval();
    trust::check_approval(&info, &approved).unwrap();
    type ApprovalChange = Box<dyn Fn(&mut ApprovedImage)>;
    let changes: Vec<ApprovalChange> = vec![
        Box::new(|image| image.architecture = "arm64".into()),
        Box::new(|image| image.architecture = "x86".into()),
        Box::new(|image| image.profile = "desktop".into()),
        Box::new(|image| image.format = "ffu-v2".into()),
        Box::new(|image| image.sha256 = "0".repeat(64)),
        Box::new(|image| image.build = "17763.253".into()),
        Box::new(|image| image.platform_ids = vec!["*".into()]),
        Box::new(|image| image.platform_ids = vec!["Not.Raspberry.Pi".into(), "*".into()]),
        Box::new(|image| image.recommended_boards = vec!["pi4-b".into()]),
        Box::new(|image| image.recommended_boards = vec!["Pi3B".into()]),
        Box::new(|image| image.recommended_boards.clear()),
        Box::new(|image| image.file_bytes += 1),
        Box::new(|image| image.payload_bytes = 0),
        Box::new(|image| image.minimum_disk_bytes = 512),
        Box::new(|image| image.sector_size = 4096),
        Box::new(|image| image.chunk_size = 4096),
        Box::new(|image| image.os_version = "10.0.1.1".into()),
    ];
    for change in changes {
        let mut approved = approved.clone();
        change(&mut approved);
        assert!(trust::check_approval(&info, &approved).is_err());
    }
}

#[test]
fn inspection_shape_from_validated_identity_not_filename() {
    let fixture = Fixture::new();
    let info = fixture.parse().unwrap();
    let inspected = trust::inspection(
        &info,
        &fixture.approval(),
        InspectionHdmi {
            mode: "official".into(),
            label: hdmi_options("official", "").unwrap().label,
            configured_blocks: 0,
        },
    );
    let value = serde_json::to_value(inspected).unwrap();
    assert_eq!(value["format"], "ffu-v1");
    assert_eq!(value["architecture"], "arm32");
    assert_eq!(value["profile"], "iot-core");
    assert_eq!(value["hardware_tested"], false);
    assert_eq!(value["trust"], "pinned-official-sha256");
    assert_eq!(value["sha256"], info.sha256);
    assert_eq!(value["minimum_disk_bytes"], SIZE);
    assert_eq!(value["payload_bytes"], 6 * CHUNK);
    assert!(value.get("filename").is_none());
}

fn cli(arguments: &[&str]) -> (i32, String, String) {
    let args: Vec<_> = arguments.iter().map(OsString::from).collect();
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let status = run_with_io(&args, &mut out, &mut err).unwrap();
    (
        status,
        String::from_utf8(out).unwrap(),
        String::from_utf8(err).unwrap(),
    )
}

#[test]
fn cli_profiles_and_hdmi_json() {
    for board in ["pi2-v1.1", "pi2-v1.2", "pi3-b"] {
        let (status, out, err) = cli(&["profile", "--board", board, "--json"]);
        assert_eq!(status, 0);
        assert!(err.is_empty());
        let value: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(value["source_verification"], "not-assessed");
        assert_eq!(value["trust"], "pinned-bundled-profile");
    }
    let (status, out, err) = cli(&["hdmi-options", "--hdmi-mode", "720p60", "--json"]);
    assert_eq!(status, 0);
    assert!(err.is_empty());
    let value: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(value["recommended"], true);
    let (status, out, _) = cli(&["profile", "--board=pi3-b"]);
    assert_eq!(status, 0);
    assert_eq!(
        out,
        "Expected Windows 10 IoT Core build: 17763.107; image verification: Not Assessed\n"
    );
}

#[test]
fn cli_errors_have_original_categories_no_success_json() {
    let files = Files::new();
    let source = files.source.to_str().unwrap();
    let target = files.target.to_str().unwrap();
    for (args, status, message) in [
        (vec!["inspect", source, "--json"], 2, "not allowlisted"),
        (
            vec!["identify-target", target, "--json"],
            3,
            "not a file or symlink",
        ),
        (vec!["inspect", "missing-file", "--json"], 4, "I/O failure"),
        (
            vec![
                "apply",
                source,
                target,
                "--target-size",
                "32768",
                "--allow-gpt-cleanup",
            ],
            3,
            "verified image digest",
        ),
        (
            vec![
                "apply",
                source,
                target,
                "--target-size",
                "32768",
                "--wipe-entire-drive",
            ],
            3,
            "full-drive wipe requires",
        ),
        (
            vec![
                "apply",
                source,
                target,
                "--target-size",
                "32768",
                "--expected-sha256",
                "A",
            ],
            2,
            "64 lowercase",
        ),
        (
            vec![
                "apply",
                source,
                target,
                "--target-size",
                "32768",
                "--expected-target-id=--1:2:3",
            ],
            3,
            "malformed",
        ),
        (
            vec!["profile", "--board", "pi4-b", "--json"],
            2,
            "exactly one",
        ),
        (
            vec!["hdmi-options", "--hdmi-mode", "unknown"],
            2,
            "unknown HDMI",
        ),
        (
            vec!["apply", source, target, "--target-size", "not-an-integer"],
            2,
            "invalid integer",
        ),
    ] {
        let (actual, out, err) = cli(&args);
        assert_eq!(actual, status, "{args:?}: {err}");
        assert!(out.is_empty());
        assert!(err.contains(message), "{args:?}: {err}");
    }
}

#[test]
fn cli_no_untrusted_or_fixture_bypass_switches() {
    let files = Files::new();
    for flag in [
        "--allow-untrusted",
        "--allow-regular-file",
        "--skip-verify",
        "--manifest",
    ] {
        let (status, out, err) = cli(&[
            "apply",
            files.source.to_str().unwrap(),
            files.target.to_str().unwrap(),
            "--target-size",
            "32768",
            flag,
        ]);
        assert_eq!(status, 2);
        assert!(out.is_empty());
        assert!(err.contains("unrecognized arguments"));
    }
    assert_eq!(fs::read(&files.target).unwrap(), vec![0xa5; SIZE as usize]);
}

#[test]
fn cli_signed_id_and_wipe_forwarded_as_ordinary_bound_arguments() {
    let files = Files::new();
    let (status, out, err) = cli(&[
        "apply",
        files.source.to_str().unwrap(),
        files.target.to_str().unwrap(),
        "--target-size",
        "32768",
        "--expected-target-id=-2088985291:687:16777240",
        "--expected-sha256",
        &"1".repeat(64),
        "--wipe-entire-drive",
    ]);
    assert_eq!(status, 2);
    assert!(out.is_empty());
    assert!(err.contains("not allowlisted"));
    assert!(!err.contains("malformed"));
    assert!(!err.contains("requires"));
}

#[test]
fn cli_required_arguments_and_help() {
    for args in [
        &[][..],
        &["invalid"][..],
        &["profile"][..],
        &["apply", "source", "target"][..],
        &["inspect"][..],
        &["hdmi-options", "--hdmi-mode"][..],
    ] {
        assert_eq!(cli(args).0, 2, "{args:?}");
    }
    let (status, out, err) = cli(&["--help"]);
    assert_eq!(status, 0);
    assert!(out.contains("Exit statuses"));
    assert!(err.is_empty());
}

#[test]
fn cancellation_statuses_and_error_types() {
    for signal in [2, 15] {
        let signal = std::sync::atomic::AtomicI32::new(signal);
        let error = Cancel {
            flag: &NEVER_CANCELLED,
            signal: Some(&signal),
        }
        .check()
        .unwrap_err();
        assert_eq!(error.exit_status(), 128 + signal.load(Ordering::Relaxed));
    }
    assert_eq!(Error::Source("bad".into()).exit_status(), 2);
    assert_eq!(Error::Target("unsafe".into()).exit_status(), 3);
    assert_eq!(Error::Apply("failure".into()).exit_status(), 4);
    assert_eq!(
        Error::io("fixture", io::Error::other("failure")).exit_status(),
        4
    );
}
