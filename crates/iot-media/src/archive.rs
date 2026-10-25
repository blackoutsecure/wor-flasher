use crate::{MediaError, OfficialImage, Package, Progress, Result, check_cancel, copy_verified};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::os::unix::fs::FileExt;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

const FREE: u32 = 0xffff_ffff;
const END: u32 = 0xffff_fffe;
const FAT: u32 = 0xffff_fffd;
const DIFAT: u32 = 0xffff_fffc;

fn u32_at(data: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(
        data[offset..offset + 4]
            .try_into()
            .expect("bounded sector field"),
    )
}

fn u16_at(data: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(
        data[offset..offset + 2]
            .try_into()
            .expect("bounded sector field"),
    )
}

fn read_at(file: &File, offset: u64, size: usize, base: u64, bound: u64) -> Result<Vec<u8>> {
    if size == 0
        || size > 4096
        || offset
            .checked_add(size as u64)
            .is_none_or(|end| end > bound)
    {
        return Err(MediaError::invalid(
            "compound-storage read exceeds pinned MSI bounds",
        ));
    }
    let mut data = vec![0; size];
    file.read_exact_at(
        &mut data,
        base.checked_add(offset)
            .ok_or_else(|| MediaError::invalid("compound-storage offset overflow"))?,
    )?;
    Ok(data)
}

pub(crate) fn cabinet(
    snapshot: &File,
    image: &OfficialImage,
    is_msi: bool,
    target: &mut File,
    progress: &mut dyn FnMut(Progress),
    cancel: &AtomicBool,
) -> Result<()> {
    let base = if is_msi { 0 } else { image.msi_offset };
    let size = snapshot.metadata()?.len();
    if base
        .checked_add(image.msi.size)
        .is_none_or(|end| end > size)
        || !image.msi.size.is_multiple_of(4096)
        || image.msi.size < 8192
    {
        return Err(MediaError::invalid(
            "pinned MSI extent is outside the verified wrapper",
        ));
    }
    if !is_msi {
        let mut hash = Sha256::new();
        let mut offset = 0;
        while offset < image.msi.size {
            check_cancel(cancel)?;
            let chunk = read_at(snapshot, offset, 4096, base, image.msi.size)?;
            hash.update(&chunk);
            offset += chunk.len() as u64;
        }
        if format!("{:x}", hash.finalize()) != image.msi.sha256 {
            return Err(MediaError::invalid(
                "embedded MSI SHA256 does not match bundled official provenance",
            ));
        }
    }
    let header = read_at(snapshot, 0, 512, base, image.msi.size)?;
    if &header[..8] != b"\xd0\xcf\x11\xe0\xa1\xb1\x1a\xe1"
        || [
            u16_at(&header, 26),
            u16_at(&header, 28),
            u16_at(&header, 30),
            u16_at(&header, 32),
        ] != [4, 0xfffe, 12, 6]
    {
        return Err(MediaError::invalid(
            "only the reviewed version-4 MSI compound-storage layout is supported",
        ));
    }
    let sectors = image.msi.size / 4096 - 1;
    let fat_count = u32_at(&header, 44) as usize;
    let mut difat_start = u32_at(&header, 68);
    let difat_count = u32_at(&header, 72);
    if fat_count == 0 || fat_count as u64 > sectors.div_ceil(1024) || difat_count > 4 {
        return Err(MediaError::invalid(
            "compound-storage allocation tables exceed bounded MSI geometry",
        ));
    }
    let sector = |index: u32| -> Result<Vec<u8>> {
        if u64::from(index) >= sectors {
            return Err(MediaError::invalid(
                "compound-storage sector index is outside the MSI",
            ));
        }
        read_at(
            snapshot,
            (u64::from(index) + 1) * 4096,
            4096,
            base,
            image.msi.size,
        )
    };
    let mut indexes: Vec<u32> = (0..109)
        .map(|index| u32_at(&header, 76 + index * 4))
        .collect();
    let mut seen_difat = HashSet::new();
    for _ in 0..difat_count {
        check_cancel(cancel)?;
        if !seen_difat.insert(difat_start) {
            return Err(MediaError::invalid("cyclic compound-storage DIFAT"));
        }
        let values = sector(difat_start)?;
        indexes.extend((0..1023).map(|index| u32_at(&values, index * 4)));
        difat_start = u32_at(&values, 4092);
    }
    if difat_start != END {
        return Err(MediaError::invalid("unterminated compound-storage DIFAT"));
    }
    indexes.retain(|index| *index != FREE);
    let unique: HashSet<_> = indexes.iter().copied().collect();
    if indexes.len() != fat_count || unique.len() != fat_count || !unique.is_disjoint(&seen_difat) {
        return Err(MediaError::invalid(
            "inconsistent compound-storage FAT sector list",
        ));
    }
    let mut fat = Vec::with_capacity(fat_count * 1024);
    for index in &indexes {
        check_cancel(cancel)?;
        let bytes = sector(*index)?;
        fat.extend((0..1024).map(|index| u32_at(&bytes, index * 4)));
    }
    if fat.len() < sectors as usize
        || indexes
            .iter()
            .any(|index| fat.get(*index as usize) != Some(&FAT))
        || seen_difat
            .iter()
            .any(|index| fat.get(*index as usize) != Some(&DIFAT))
    {
        return Err(MediaError::invalid(
            "invalid compound-storage allocation table coverage",
        ));
    }
    if image.cabinet.size > image.msi.size {
        return Err(MediaError::invalid("pinned cabinet exceeds its MSI bounds"));
    }
    let mut chain = Vec::new();
    let mut seen = vec![false; sectors as usize];
    let mut current = image.cabinet_sector;
    let mut remaining = image.cabinet.size;
    while remaining > 0 {
        check_cancel(cancel)?;
        if u64::from(current) >= sectors || seen[current as usize] {
            return Err(MediaError::invalid(
                "out-of-bounds or cyclic cabinet sector chain",
            ));
        }
        seen[current as usize] = true;
        chain.push(current);
        remaining = remaining.saturating_sub(4096);
        current = fat[current as usize];
    }
    if current != END {
        return Err(MediaError::invalid("cabinet chain exceeds its pinned size"));
    }
    let mut reader = CabinetChain {
        snapshot,
        base,
        bound: image.msi.size,
        chain,
        at: 0,
        remaining: image.cabinet.size,
        buffer: Vec::new(),
    };
    copy_verified(
        &mut reader,
        target,
        &image.cabinet,
        "Reassembling and verifying official cabinet",
        progress,
        cancel,
        Duration::from_secs(600),
    )?;
    Ok(())
}

struct CabinetChain<'a> {
    snapshot: &'a File,
    base: u64,
    bound: u64,
    chain: Vec<u32>,
    at: usize,
    remaining: u64,
    buffer: Vec<u8>,
}

impl Read for CabinetChain<'_> {
    fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
        if output.is_empty() || self.remaining == 0 {
            return Ok(0);
        }
        if self.buffer.is_empty() {
            let sector = self.chain[self.at];
            self.at += 1;
            self.buffer = read_at(
                self.snapshot,
                (u64::from(sector) + 1) * 4096,
                4096,
                self.base,
                self.bound,
            )
            .map_err(std::io::Error::other)?;
        }
        let size = output
            .len()
            .min(self.buffer.len())
            .min(self.remaining as usize);
        output[..size].copy_from_slice(&self.buffer[..size]);
        self.buffer.drain(..size);
        self.remaining -= size as u64;
        Ok(size)
    }
}

fn safe_member(name: &str) -> bool {
    let mut chars = name.bytes();
    matches!(chars.next(), Some(first) if first.is_ascii_alphanumeric())
        && name.len() <= 128
        && chars.all(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(&byte))
}

pub(crate) fn extract(
    cabinet: &mut File,
    target: &mut File,
    image: &OfficialImage,
    progress: &mut dyn FnMut(Progress),
    cancel: &AtomicBool,
) -> Result<()> {
    check_cancel(cancel)?;
    let stamp = crate::fs::Stamp::of(cabinet)?;
    let mut header = [0; 36];
    cabinet.seek(SeekFrom::Start(0))?;
    cabinet.read_exact(&mut header)?;
    if &header[..4] != b"MSCF"
        || u64::from(u32_at(&header, 8)) != image.cabinet.size
        || u16_at(&header, 30) & 3 != 0
        || !(1..=1024).contains(&u16_at(&header, 26))
        || !(1..=4096).contains(&u16_at(&header, 28))
    {
        return Err(MediaError::invalid(
            "invalid, excessive or spanning cabinet geometry",
        ));
    }
    cabinet.seek(SeekFrom::Start(0))?;
    let started = Instant::now();
    {
        let mut archive = cab::Cabinet::new(&mut *cabinet)
            .map_err(|_| MediaError::invalid("cannot parse the verified cabinet"))?;
        let mut names = HashSet::new();
        let mut matches = 0;
        for folder in archive.folder_entries() {
            for entry in folder.file_entries() {
                if !safe_member(entry.name()) || !names.insert(entry.name().to_owned()) {
                    return Err(MediaError::invalid(
                        "unsafe or duplicate cabinet member name",
                    ));
                }
                if entry.name() == image.member {
                    matches += 1;
                    if u64::from(entry.uncompressed_size()) != image.ffu.size {
                        return Err(MediaError::invalid(
                            "cabinet FFU member does not have its pinned size",
                        ));
                    }
                }
            }
        }
        if matches != 1 {
            return Err(MediaError::invalid(
                "cabinet does not contain exactly one approved FFU member",
            ));
        }
        let mut reader = archive.read_file(&image.member).map_err(|_| {
            MediaError::invalid("cannot decompress the approved FFU cabinet member")
        })?;
        copy_verified(
            &mut reader,
            target,
            &Package {
                size: image.ffu.size,
                sha256: image.ffu.sha256.clone(),
            },
            "Extracting approved FFU member",
            progress,
            cancel,
            Duration::from_secs(600).saturating_sub(started.elapsed()),
        )?;
    }
    if crate::fs::Stamp::of(cabinet)? != stamp {
        return Err(MediaError::acquisition(
            "verified package snapshot changed during extraction",
        ));
    }
    // The published FFU is private but writable by a subsequent root writer, as in the source.
    target.set_permissions(std::os::unix::fs::PermissionsExt::from_mode(0o600))?;
    Ok(())
}
