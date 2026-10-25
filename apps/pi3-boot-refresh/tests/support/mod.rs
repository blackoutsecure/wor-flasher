#![allow(dead_code)]

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

use wor_pi3_boot_refresh::{BootstrapDisk, DiskLayout, ReadAt, SECTOR_SIZE, Sector, VolumeExtent};

pub const EFI_TYPE: [u8; 16] = [
    0x28, 0x73, 0x2a, 0xc1, 0x1f, 0xf8, 0xd2, 0x11, 0xba, 0x4b, 0, 0xa0, 0xc9, 0x3e, 0xc9, 0x3b,
];
pub const BASIC_TYPE: [u8; 16] = [
    0xa2, 0xa0, 0xd0, 0xeb, 0xe5, 0xb9, 0x33, 0x44, 0x87, 0xc0, 0x68, 0xb6, 0xb7, 0x26, 0x99, 0xc7,
];
pub type Image = BTreeMap<u64, Sector>;

pub fn put16(data: &mut [u8], offset: usize, value: u16) {
    data[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}

pub fn put32(data: &mut [u8], offset: usize, value: u32) {
    data[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

pub fn put64(data: &mut [u8], offset: usize, value: u64) {
    data[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

pub fn get32(data: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap())
}

pub fn header_crc(image: &mut Image) {
    let header = image.get_mut(&1).unwrap();
    put32(header, 16, 0);
    let size = get32(header, 12) as usize;
    let crc = crc32fast::hash(&header[..size]);
    put32(header, 16, crc);
}

pub fn table_crc(image: &mut Image) {
    let header = image[&1];
    let table_size = u64::from(get32(&header, 80)) * u64::from(get32(&header, 84));
    let table_lba = u64::from_le_bytes(header[72..80].try_into().unwrap());
    let mut crc = crc32fast::Hasher::new();
    for offset in (0..table_size).step_by(SECTOR_SIZE) {
        let data = image[&(table_lba + offset / 512)];
        let count = (table_size - offset).min(512) as usize;
        crc.update(&data[..count]);
    }
    put32(image.get_mut(&1).unwrap(), 88, crc.finalize());
    header_crc(image);
}

#[derive(Clone, Copy)]
pub struct Geometry {
    pub start: u64,
    pub count: u32,
    pub reserved: u16,
    pub cluster: u8,
    pub fat: u32,
    pub root: u32,
}

impl Default for Geometry {
    fn default() -> Self {
        Self {
            start: 2048,
            count: 131072,
            reserved: 32,
            cluster: 1,
            fat: 1024,
            root: 2,
        }
    }
}

/// Byte fixture matching create_gpt_image in tests/test-pi3-gpt-bootstrap.py.
/// Sparse sectors let the bounds tests cover large media without host-device I/O.
pub fn fixture(geometry: Geometry) -> (MemoryDisk, DiskLayout) {
    let backup_lba = geometry.start + u64::from(geometry.count) + 4095;
    let boot_end = geometry.start + u64::from(geometry.count) - 1;
    let windows_start = boot_end + 65;
    let mut image = Image::new();
    let mut original = [0; SECTOR_SIZE];
    for (index, value) in original.iter_mut().enumerate() {
        *value = (index % 251) as u8;
    }
    original[446..510].fill(0);
    original[450] = 0xee;
    put32(&mut original, 454, 1);
    put32(
        &mut original,
        458,
        backup_lba.min(u64::from(u32::MAX)) as u32,
    );
    original[510..].copy_from_slice(&[0x55, 0xaa]);
    image.insert(0, original);

    let mut table = [0; 16384];
    table[..16].copy_from_slice(&EFI_TYPE);
    table[16..32].fill(0x11);
    put64(&mut table, 32, geometry.start);
    put64(&mut table, 40, boot_end);
    table[128..144].copy_from_slice(&BASIC_TYPE);
    table[144..160].fill(0x22);
    put64(&mut table, 160, windows_start);
    put64(&mut table, 168, backup_lba - 33);
    for (index, sector) in table.as_chunks::<SECTOR_SIZE>().0.iter().enumerate() {
        image.insert(2 + index as u64, *sector);
        image.insert(backup_lba - 32 + index as u64, *sector);
    }

    let mut header = [0; SECTOR_SIZE];
    header[..8].copy_from_slice(b"EFI PART");
    put32(&mut header, 8, 0x10000);
    put32(&mut header, 12, 92);
    put64(&mut header, 24, 1);
    put64(&mut header, 32, backup_lba);
    put64(&mut header, 40, 34);
    put64(&mut header, 48, backup_lba - 33);
    header[56..72].fill(0x33);
    put64(&mut header, 72, 2);
    put32(&mut header, 80, 128);
    put32(&mut header, 84, 128);
    put32(&mut header, 88, crc32fast::hash(&table));
    image.insert(1, header);
    header_crc(&mut image);
    let mut backup = image[&1];
    put64(&mut backup, 24, backup_lba);
    put64(&mut backup, 32, 1);
    put64(&mut backup, 72, backup_lba - 32);
    put32(&mut backup, 16, 0);
    let crc = crc32fast::hash(&backup[..92]);
    put32(&mut backup, 16, crc);
    image.insert(backup_lba, backup);

    let mut boot = [0; SECTOR_SIZE];
    boot[..11].copy_from_slice(b"\xeb\x58\x90MSWIN4.1");
    put16(&mut boot, 11, 512);
    boot[13] = geometry.cluster;
    put16(&mut boot, 14, geometry.reserved);
    boot[16] = 2;
    boot[21] = 0xf8;
    put16(&mut boot, 24, 63);
    put16(&mut boot, 26, 255);
    put32(&mut boot, 28, geometry.start as u32);
    put32(&mut boot, 32, geometry.count);
    put32(&mut boot, 36, geometry.fat);
    put32(&mut boot, 44, geometry.root);
    put16(&mut boot, 48, 1);
    put16(&mut boot, 50, 6);
    boot[64] = 0x80;
    boot[66] = 0x29;
    put32(&mut boot, 67, 123456);
    boot[71..90].copy_from_slice(b"WOR_BOOT   FAT32   ");
    boot[510..].copy_from_slice(&[0x55, 0xaa]);
    image.insert(geometry.start, boot);
    image.insert(geometry.start + 6, boot);

    let layout = DiskLayout {
        disk_index: 7,
        bytes: (backup_lba + 1) * 512,
        bytes_per_sector: 512,
        boot: VolumeExtent {
            disk_index: 7,
            offset: geometry.start * 512,
            length: u64::from(geometry.count) * 512,
        },
        windows: VolumeExtent {
            disk_index: 7,
            offset: windows_start * 512,
            length: (backup_lba - 33 - windows_start + 1) * 512,
        },
    };
    (MemoryDisk::new(image, layout.bytes), layout)
}

#[derive(Default)]
pub enum WriteFault {
    #[default]
    None,
    Error,
    Short,
    Dropped,
}

pub struct MemoryDisk {
    pub image: Rc<RefCell<Image>>,
    pub bytes: u64,
    pub reads: Vec<u64>,
    pub writes: usize,
    pub syncs: usize,
    pub write_fault: WriteFault,
    pub sync_error: bool,
    pub read_error: Option<u64>,
    pub short_read: Option<u64>,
    pub after_write: Option<fn(&mut Image)>,
}

impl MemoryDisk {
    pub fn new(image: Image, bytes: u64) -> Self {
        Self {
            image: Rc::new(RefCell::new(image)),
            bytes,
            reads: vec![],
            writes: 0,
            syncs: 0,
            write_fault: WriteFault::None,
            sync_error: false,
            read_error: None,
            short_read: None,
            after_write: None,
        }
    }
}

impl ReadAt for MemoryDisk {
    fn read_at(&mut self, data: &mut [u8], offset: u64) -> io::Result<usize> {
        assert_eq!(data.len(), SECTOR_SIZE, "unaligned raw-device read length");
        assert_eq!(offset % 512, 0, "unaligned raw-device read offset");
        assert!(offset <= self.bytes - 512, "out-of-bounds raw-device read");
        let lba = offset / 512;
        self.reads.push(lba);
        if self.read_error == Some(lba) {
            return Err(io::Error::other("fixture read error"));
        }
        let zero = [0; SECTOR_SIZE];
        data.copy_from_slice(self.image.borrow().get(&lba).unwrap_or(&zero));
        Ok(if self.short_read == Some(lba) {
            511
        } else {
            SECTOR_SIZE
        })
    }
}

impl BootstrapDisk for MemoryDisk {
    fn write_at(&mut self, data: &[u8], offset: u64) -> io::Result<usize> {
        assert_eq!(offset, 0, "write outside sector 0");
        assert_eq!(data.len(), SECTOR_SIZE, "write not exactly one sector");
        self.writes += 1;
        match self.write_fault {
            WriteFault::Error => return Err(io::Error::other("fixture write error")),
            WriteFault::Short => return Ok(511),
            WriteFault::Dropped => return Ok(SECTOR_SIZE),
            WriteFault::None => {}
        }
        let mut image = self.image.borrow_mut();
        image.insert(0, data.try_into().unwrap());
        if let Some(change) = self.after_write {
            change(&mut image);
        }
        Ok(SECTOR_SIZE)
    }

    fn sync(&mut self) -> io::Result<()> {
        self.syncs += 1;
        if self.sync_error {
            Err(io::Error::other("fixture flush error"))
        } else {
            Ok(())
        }
    }
}

pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .find(|path| path.join("src/config/metadata.json").is_file())
        .expect("native fixtures require the wor-flasher source checkout")
        .to_owned()
}

pub struct FixtureDirectory(pub PathBuf);

impl FixtureDirectory {
    pub fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let base = repo_root().join("target/pi3-native-fixtures");
        std::fs::create_dir_all(&base).unwrap();
        for _ in 0..10000 {
            let path = base.join(format!(
                "{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            match std::fs::create_dir(&path) {
                Ok(()) => return Self(path),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("creating fixture directory: {error}"),
            }
        }
        panic!("fixture directory names exhausted")
    }
}

impl Drop for FixtureDirectory {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}
