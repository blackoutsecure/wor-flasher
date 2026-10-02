#!/usr/bin/env python3
"""Expose GPT partition 1 as a FAT bootstrap without creating a hybrid MBR."""

#Keep the historical filename: local startup repair restores missing tracked runtime paths.

from __future__ import annotations

import argparse
import os
import struct
import zlib
from pathlib import Path


SECTOR_SIZE = 512
GPT_HEADER_LBA = 1
GPT_SIGNATURE = b"EFI PART"
PARTITION_ENTRY_OFFSET = 446
PARTITION_ENTRY_SIZE = 16
MBR_SIGNATURE_OFFSET = 510
FAT_PARTITION_TYPES = (
    bytes.fromhex("28732ac11ff8d211ba4b00a0c93ec93b"),
    bytes.fromhex("a2a0d0ebe5b9334487c068b6b72699c7"),
)


def read_at(handle: int, offset: int, size: int) -> bytes:
    #Raw character devices (/dev/rdiskN on macOS) reject any read whose offset or length is
    #not a whole number of sectors with EINVAL, so widen every request to sector boundaries
    #and slice the caller's window back out in memory.
    base = (offset // SECTOR_SIZE) * SECTOR_SIZE
    leading = offset - base
    span = leading + size
    length = -(-span // SECTOR_SIZE) * SECTOR_SIZE
    data = os.pread(handle, length, base)
    window = data[leading : leading + size]
    if len(window) != size:
        raise ValueError(f"short read at byte offset {offset}")
    return window


def lba_to_chs(lba: int) -> bytes:
    sector = (lba % 63) + 1
    track = lba // 63
    head = track % 255
    cylinder = track // 255
    if cylinder > 1023:
        return b"\xfe\xff\xff"
    return bytes((head, sector | ((cylinder >> 2) & 0xC0), cylinder & 0xFF))


def partition_entry(partition_type: int, start_lba: int, sector_count: int) -> bytes:
    entry = bytearray(PARTITION_ENTRY_SIZE)
    entry[0] = 0x00
    entry[1:4] = lba_to_chs(start_lba)
    entry[4] = partition_type
    entry[5:8] = lba_to_chs(start_lba + sector_count - 1)
    struct.pack_into("<II", entry, 8, start_lba, sector_count)
    return bytes(entry)


def build_mbr(handle: int) -> bytes:
    header = read_at(handle, GPT_HEADER_LBA * SECTOR_SIZE, SECTOR_SIZE)
    if header[:8] != GPT_SIGNATURE:
        raise ValueError("target does not contain a valid GPT header at LBA 1")

    header_size = struct.unpack_from("<I", header, 12)[0]
    if not 92 <= header_size <= SECTOR_SIZE:
        raise ValueError("GPT header size is invalid")
    checksum_header = bytearray(header[:header_size])
    struct.pack_into("<I", checksum_header, 16, 0)
    if zlib.crc32(checksum_header) != struct.unpack_from("<I", header, 16)[0]:
        raise ValueError("GPT header checksum is invalid")

    current_lba = struct.unpack_from("<Q", header, 24)[0]
    backup_lba = struct.unpack_from("<Q", header, 32)[0]
    first_usable, last_usable = struct.unpack_from("<QQ", header, 40)
    entries_lba = struct.unpack_from("<Q", header, 72)[0]
    entry_count = struct.unpack_from("<I", header, 80)[0]
    entry_size = struct.unpack_from("<I", header, 84)[0]
    if current_lba != 1 or not 34 <= first_usable <= last_usable < backup_lba:
        raise ValueError("GPT usable-sector bounds are invalid")
    table_size = entry_count * entry_size
    table_end = entries_lba + (table_size + SECTOR_SIZE - 1) // SECTOR_SIZE
    if entry_count < 1 or entry_size < 128 or entry_size % 128 or entries_lba < 2 or table_end > first_usable:
        raise ValueError("GPT partition-entry geometry is invalid")

    table_checksum = 0
    for offset in range(0, table_size, 65536):
        block = read_at(handle, entries_lba * SECTOR_SIZE + offset, min(65536, table_size - offset))
        table_checksum = zlib.crc32(block, table_checksum)
    if table_checksum != struct.unpack_from("<I", header, 88)[0]:
        raise ValueError("GPT partition-entry checksum is invalid")

    first_entry = read_at(handle, entries_lba * SECTOR_SIZE, 128)
    type_guid = first_entry[:16]
    if type_guid not in FAT_PARTITION_TYPES:
        raise ValueError("GPT partition 1 is not EFI System or Microsoft Basic Data")
    start_lba, end_lba = struct.unpack_from("<QQ", first_entry, 32)
    if not first_usable <= start_lba <= end_lba <= last_usable:
        raise ValueError("GPT partition 1 has invalid bounds")

    boot = read_at(handle, start_lba * SECTOR_SIZE, SECTOR_SIZE)
    if boot[510:512] != b"\x55\xaa":
        raise ValueError("FAT boot-sector signature is invalid")
    if struct.unpack_from("<H", boot, 11)[0] != SECTOR_SIZE:
        raise ValueError("Pi3 GPT bootstrap requires 512-byte FAT sectors")
    sectors_per_cluster = boot[13]
    reserved = struct.unpack_from("<H", boot, 14)[0]
    fat_count = boot[16]
    total_sectors = struct.unpack_from("<I", boot, 32)[0]
    fat_sectors = struct.unpack_from("<I", boot, 36)[0]
    fat_flags, version = struct.unpack_from("<HH", boot, 40)
    root_cluster = struct.unpack_from("<I", boot, 44)[0]
    if sectors_per_cluster == 0 or sectors_per_cluster & (sectors_per_cluster - 1):
        raise ValueError("FAT sectors per cluster must be a power of two")
    if reserved == 0:
        raise ValueError("FAT reserved-sector count is zero")
    if fat_count not in (1, 2) or fat_sectors == 0:
        raise ValueError("FAT count or size is invalid")
    if any(struct.unpack_from("<H", boot, offset)[0] for offset in (17, 19, 22)) or version != 0:
        raise ValueError("boot partition must contain a FAT32 version 0 filesystem")
    if fat_flags & 0x80 and (fat_flags & 0xF) >= fat_count:
        raise ValueError("FAT32 active FAT index is invalid")
    if total_sectors == 0 or total_sectors > end_lba - start_lba + 1:
        raise ValueError("FAT filesystem exceeds its GPT partition bounds")
    data_sectors = total_sectors - reserved - fat_count * fat_sectors
    clusters = data_sectors // sectors_per_cluster
    if not 65525 <= clusters < 0x0FFFFFF5:
        raise ValueError("boot partition does not have a valid FAT32 data-cluster count")
    if (clusters + 2) * 4 > fat_sectors * SECTOR_SIZE:
        raise ValueError("FAT table is too small for the data-cluster count")
    if not 2 <= root_cluster < clusters + 2:
        raise ValueError("FAT32 root cluster is outside the data region")
    if start_lba + reserved > 65535:
        raise ValueError("Pi3 GPT bootstrap needs partition offset + FAT reserved sectors <= 65535")
    if start_lba + total_sectors > 0xFFFFFFFF:
        raise ValueError("Pi3 GPT bootstrap sector count exceeds the FAT32 limit")

    #Like upstream WoR's pi3/gptpatch.img, expose a FAT view starting at LBA 0.
    #Shift its metadata pointers, not the actual FATs or files. Windows must still
    #see only one protective MBR entry, or it treats the GPT disk as legacy MBR.
    mbr = bytearray(read_at(handle, 0, SECTOR_SIZE))
    mbr[:440] = b"\0" * 440
    mbr[11:90] = boot[11:90]
    struct.pack_into("<H", mbr, 14, start_lba + reserved)
    struct.pack_into("<I", mbr, 32, start_lba + total_sectors)
    for offset, name in ((48, "FSInfo"), (50, "backup boot sector")):
        relative_sector = struct.unpack_from("<H", boot, offset)[0]
        if relative_sector not in (0, 0xFFFF):
            if relative_sector >= reserved:
                raise ValueError(f"FAT32 {name} is outside its reserved region")
            struct.pack_into("<H", mbr, offset, start_lba + relative_sector)

    mbr[PARTITION_ENTRY_OFFSET : PARTITION_ENTRY_OFFSET + 64] = b"\0" * 64
    mbr[PARTITION_ENTRY_OFFSET : PARTITION_ENTRY_OFFSET + PARTITION_ENTRY_SIZE] = (
        partition_entry(0xEE, 1, min(backup_lba, 0xFFFFFFFF))
    )
    mbr[MBR_SIGNATURE_OFFSET : MBR_SIGNATURE_OFFSET + 2] = b"\x55\xaa"
    return bytes(mbr)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true", help="verify the bootstrap without writing")
    parser.add_argument("device", type=Path)
    args = parser.parse_args()
    flags = os.O_RDONLY if args.check else os.O_RDWR
    handle = os.open(args.device, flags)
    try:
        mbr = build_mbr(handle)
        if not args.check:
            written = os.pwrite(handle, mbr, 0)
            if written != SECTOR_SIZE:
                raise OSError(f"short write: wrote {written} of {SECTOR_SIZE} bytes")
            os.fsync(handle)
        if read_at(handle, 0, SECTOR_SIZE) != mbr:
            raise ValueError("Pi3 GPT bootstrap does not match the FAT geometry and protective-only MBR")
    finally:
        os.close(handle)
    print("Pi3 GPT-compatible bootstrap verified." if args.check else "Pi3 GPT-compatible bootstrap written and verified.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
