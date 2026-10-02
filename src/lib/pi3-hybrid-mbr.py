#!/usr/bin/env python3
"""Expose the GPT boot partition to the Raspberry Pi 3 first-stage loader."""

from __future__ import annotations

import argparse
import os
import struct
from pathlib import Path


SECTOR_SIZE = 512
GPT_HEADER_LBA = 1
GPT_SIGNATURE = b"EFI PART"
PARTITION_ENTRY_OFFSET = 446
PARTITION_ENTRY_SIZE = 16
MBR_SIGNATURE_OFFSET = 510


def read_at(handle: int, offset: int, size: int) -> bytes:
    data = os.pread(handle, size, offset)
    if len(data) != size:
        raise ValueError(f"short read at byte offset {offset}")
    return data


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

    backup_lba = struct.unpack_from("<Q", header, 32)[0]
    entries_lba = struct.unpack_from("<Q", header, 72)[0]
    entry_count = struct.unpack_from("<I", header, 80)[0]
    entry_size = struct.unpack_from("<I", header, 84)[0]
    if backup_lba < 34:
        raise ValueError("GPT backup header LBA is invalid")
    if entry_count < 1 or entry_size < 128 or entry_size % 8:
        raise ValueError("GPT partition-entry geometry is invalid")

    first_entry = read_at(handle, entries_lba * SECTOR_SIZE, entry_size)
    type_guid = first_entry[:16]
    if type_guid == b"\0" * 16:
        raise ValueError("GPT partition 1 is empty")
    start_lba, end_lba = struct.unpack_from("<QQ", first_entry, 32)
    if start_lba < 34 or end_lba < start_lba or end_lba >= backup_lba:
        raise ValueError("GPT partition 1 has invalid bounds")

    sector_count = end_lba - start_lba + 1
    if start_lba > 0xFFFFFFFF or sector_count > 0xFFFFFFFF:
        raise ValueError("GPT partition 1 does not fit in an MBR entry")

    mbr = bytearray(read_at(handle, 0, SECTOR_SIZE))
    mbr[PARTITION_ENTRY_OFFSET : PARTITION_ENTRY_OFFSET + 64] = b"\0" * 64
    mbr[PARTITION_ENTRY_OFFSET : PARTITION_ENTRY_OFFSET + PARTITION_ENTRY_SIZE] = (
        partition_entry(0x0C, start_lba, sector_count)
    )
    protective_offset = PARTITION_ENTRY_OFFSET + PARTITION_ENTRY_SIZE
    mbr[protective_offset : protective_offset + PARTITION_ENTRY_SIZE] = partition_entry(
        0xEE, 1, start_lba - 1
    )
    mbr[MBR_SIGNATURE_OFFSET : MBR_SIGNATURE_OFFSET + 2] = b"\x55\xaa"
    return bytes(mbr)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("device", type=Path)
    args = parser.parse_args()
    flags = os.O_RDWR
    handle = os.open(args.device, flags)
    try:
        mbr = build_mbr(handle)
        written = os.pwrite(handle, mbr, 0)
        if written != SECTOR_SIZE:
            raise OSError(f"short write: wrote {written} of {SECTOR_SIZE} bytes")
        os.fsync(handle)
    finally:
        os.close(handle)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
