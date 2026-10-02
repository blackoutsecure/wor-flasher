#!/usr/bin/env python3

from __future__ import annotations

import os
import struct
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


REPO_DIR = Path(__file__).resolve().parents[1]
HELPER_PATH = REPO_DIR / "src/lib/pi3-hybrid-mbr.py"

SECTOR_SIZE = 512
MBR_ENTRY_OFFSET = 446


def create_gpt_image(path: Path, start_lba: int = 2048, sector_count: int = 65536) -> bytes:
    total_sectors = start_lba + sector_count + 4096
    backup_lba = total_sectors - 1
    image_size = total_sectors * SECTOR_SIZE
    with path.open("wb") as handle:
        handle.truncate(image_size)

    sector_zero = bytearray((index % 251 for index in range(SECTOR_SIZE)))
    sector_zero[MBR_ENTRY_OFFSET : MBR_ENTRY_OFFSET + 64] = b"\0" * 64
    sector_zero[510:512] = b"\x55\xaa"

    header = bytearray(SECTOR_SIZE)
    header[:8] = b"EFI PART"
    struct.pack_into("<Q", header, 24, 1)
    struct.pack_into("<Q", header, 32, backup_lba)
    struct.pack_into("<Q", header, 72, 2)
    struct.pack_into("<I", header, 80, 128)
    struct.pack_into("<I", header, 84, 128)

    first_entry = bytearray(128)
    first_entry[:16] = bytes.fromhex("28732ac11ff8d211ba4b00a0c93ec93b")
    struct.pack_into("<QQ", first_entry, 32, start_lba, start_lba + sector_count - 1)

    with path.open("r+b") as handle:
        handle.write(sector_zero)
        handle.seek(SECTOR_SIZE)
        handle.write(header)
        handle.seek(2 * SECTOR_SIZE)
        handle.write(first_entry)
    return bytes(sector_zero)


def unpack_entry(sector: bytes, slot: int) -> tuple[int, int, int]:
    offset = MBR_ENTRY_OFFSET + (slot * 16)
    entry = sector[offset : offset + 16]
    return entry[4], *struct.unpack_from("<II", entry, 8)


def unpack_chs_end(sector: bytes, slot: int) -> bytes:
    offset = MBR_ENTRY_OFFSET + (slot * 16)
    return sector[offset + 5 : offset + 8]


class Pi3HybridMbrTests(unittest.TestCase):
    def test_writes_pi_boot_entry_before_bounded_protective_entry(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            image = Path(temp_dir) / "disk.img"
            original_sector = create_gpt_image(image)

            subprocess.run([sys.executable, str(HELPER_PATH), str(image)], check=True)

            with image.open("rb") as handle:
                sector = handle.read(SECTOR_SIZE)
                gpt_header = os.pread(handle.fileno(), SECTOR_SIZE, SECTOR_SIZE)

            self.assertEqual(sector[:MBR_ENTRY_OFFSET], original_sector[:MBR_ENTRY_OFFSET])
            self.assertEqual(unpack_entry(sector, 0), (0x0C, 2048, 65536))
            self.assertEqual(unpack_entry(sector, 1), (0xEE, 1, 2047))
            self.assertEqual(sector[MBR_ENTRY_OFFSET + 32 : MBR_ENTRY_OFFSET + 64], b"\0" * 32)
            self.assertEqual(sector[510:512], b"\x55\xaa")
            self.assertEqual(gpt_header[:8], b"EFI PART")

    def test_caps_chs_fields_when_the_partition_end_exceeds_the_chs_ceiling(self) -> None:
        #CHS can only address 1024*255*63 = 16450560 sectors, so anything past that must be
        #pinned to the maximum triplet while the 32-bit LBA fields stay exact.
        with tempfile.TemporaryDirectory() as temp_dir:
            image = Path(temp_dir) / "disk.img"
            create_gpt_image(image, start_lba=2048, sector_count=16_500_000)

            subprocess.run([sys.executable, str(HELPER_PATH), str(image)], check=True)

            with image.open("rb") as handle:
                sector = handle.read(SECTOR_SIZE)

            self.assertEqual(unpack_entry(sector, 0), (0x0C, 2048, 16_500_000))
            self.assertEqual(unpack_chs_end(sector, 0), b"\xfe\xff\xff")

    def test_rejects_non_gpt_target_without_writing(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            image = Path(temp_dir) / "disk.img"
            image.write_bytes(b"\xa5" * (SECTOR_SIZE * 4))
            before = image.read_bytes()

            result = subprocess.run(
                [sys.executable, str(HELPER_PATH), str(image)],
                check=False,
                capture_output=True,
                text=True,
            )

            self.assertNotEqual(result.returncode, 0)
            self.assertEqual(image.read_bytes(), before)
            self.assertIn("valid GPT header", result.stderr)


if __name__ == "__main__":
    unittest.main()
