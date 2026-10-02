#!/usr/bin/env python3

from __future__ import annotations

import importlib.util
import os
import shutil
import struct
import subprocess
import sys
import tempfile
import unittest
import unittest.mock
import zlib
from importlib.machinery import SourceFileLoader
from pathlib import Path
from types import ModuleType


REPO_DIR = Path(__file__).resolve().parents[1]
HELPER_PATH = REPO_DIR / "src/lib/pi3-hybrid-mbr.py"

SECTOR_SIZE = 512
MBR_ENTRY_OFFSET = 446
EFI_TYPE = bytes.fromhex("28732ac11ff8d211ba4b00a0c93ec93b")
BASIC_DATA_TYPE = bytes.fromhex("a2a0d0ebe5b9334487c068b6b72699c7")


def load_helper_module() -> ModuleType:
    #The helper filename is hyphenated, so it can only be imported through an explicit spec.
    loader = SourceFileLoader("pi3_gpt_bootstrap", str(HELPER_PATH))
    spec = importlib.util.spec_from_loader(loader.name, loader)
    assert spec is not None
    module = importlib.util.module_from_spec(spec)
    loader.exec_module(module)
    return module


def update_header_checksum(header: bytearray) -> None:
    struct.pack_into("<I", header, 16, 0)
    struct.pack_into("<I", header, 16, zlib.crc32(header[:92]))


def create_gpt_image(
    path: Path,
    start_lba: int = 2048,
    sector_count: int = 131072,
    reserved_sectors: int = 32,
    sectors_per_cluster: int = 1,
    fat_sectors: int = 1024,
    root_cluster: int = 2,
    partition_type: bytes = EFI_TYPE,
) -> bytes:
    total_sectors = start_lba + sector_count + 4096
    backup_lba = total_sectors - 1
    with path.open("wb") as handle:
        handle.truncate(total_sectors * SECTOR_SIZE)

    sector_zero = bytearray((index % 251 for index in range(SECTOR_SIZE)))
    sector_zero[MBR_ENTRY_OFFSET : MBR_ENTRY_OFFSET + 64] = b"\0" * 64
    sector_zero[450] = 0xEE
    struct.pack_into("<II", sector_zero, 454, 1, backup_lba)
    sector_zero[510:512] = b"\x55\xaa"

    entries = bytearray(128 * 128)
    entries[:16] = partition_type
    entries[16:32] = b"\x11" * 16
    struct.pack_into("<QQ", entries, 32, start_lba, start_lba + sector_count - 1)
    entries[128:144] = BASIC_DATA_TYPE
    entries[144:160] = b"\x22" * 16
    struct.pack_into("<QQ", entries, 160, start_lba + sector_count + 64, backup_lba - 33)

    header = bytearray(SECTOR_SIZE)
    header[:8] = b"EFI PART"
    struct.pack_into("<II", header, 8, 0x10000, 92)
    struct.pack_into("<Q", header, 24, 1)
    struct.pack_into("<Q", header, 32, backup_lba)
    struct.pack_into("<QQ", header, 40, 34, backup_lba - 33)
    header[56:72] = b"\x33" * 16
    struct.pack_into("<Q", header, 72, 2)
    struct.pack_into("<I", header, 80, 128)
    struct.pack_into("<I", header, 84, 128)
    struct.pack_into("<I", header, 88, zlib.crc32(entries))
    update_header_checksum(header)

    backup_header = bytearray(header)
    struct.pack_into("<QQ", backup_header, 24, backup_lba, 1)
    struct.pack_into("<Q", backup_header, 72, backup_lba - 32)
    update_header_checksum(backup_header)

    boot_sector = bytearray(SECTOR_SIZE)
    boot_sector[:11] = b"\xeb\x58\x90MSWIN4.1"
    struct.pack_into("<H", boot_sector, 11, SECTOR_SIZE)
    boot_sector[13] = sectors_per_cluster
    struct.pack_into("<H", boot_sector, 14, reserved_sectors)
    boot_sector[16] = 2
    boot_sector[21] = 0xF8
    struct.pack_into("<HHIII", boot_sector, 24, 63, 255, start_lba, sector_count, fat_sectors)
    struct.pack_into("<IHH", boot_sector, 44, root_cluster, 1, 6)
    boot_sector[64] = 0x80
    boot_sector[66] = 0x29
    struct.pack_into("<I", boot_sector, 67, 123456)
    boot_sector[71:90] = b"WOR_BOOT   FAT32   "
    boot_sector[510:512] = b"\x55\xaa"

    cluster_bytes = sectors_per_cluster * SECTOR_SIZE
    payload = b"P" * cluster_bytes + b"Pi3 GPT bootstrap regression payload"
    root = bytearray(cluster_bytes)
    root[:11] = b"RPI_EFI FD "
    root[11] = 0x20
    struct.pack_into("<H", root, 26, 3)
    struct.pack_into("<I", root, 28, len(payload))

    with path.open("r+b") as handle:
        handle.write(sector_zero)
        handle.seek(SECTOR_SIZE)
        handle.write(header)
        handle.seek(2 * SECTOR_SIZE)
        handle.write(entries)
        handle.seek((backup_lba - 32) * SECTOR_SIZE)
        handle.write(entries)
        handle.seek(backup_lba * SECTOR_SIZE)
        handle.write(backup_header)
        handle.seek(start_lba * SECTOR_SIZE)
        handle.write(boot_sector)
        handle.seek((start_lba + 6) * SECTOR_SIZE)
        handle.write(boot_sector)
        for fat_index in range(2):
            fat_start = (start_lba + reserved_sectors + fat_index * fat_sectors) * SECTOR_SIZE
            for cluster, value in ((0, 0x0FFFFFF8), (1, 0xFFFFFFFF), (root_cluster, 0x0FFFFFFF), (3, 7), (7, 0x0FFFFFFF)):
                handle.seek(fat_start + cluster * 4)
                handle.write(struct.pack("<I", value))
        data_start = (start_lba + reserved_sectors + 2 * fat_sectors) * SECTOR_SIZE
        handle.seek(data_start + (root_cluster - 2) * cluster_bytes)
        handle.write(root)
        handle.seek(data_start + cluster_bytes)
        handle.write(payload[:cluster_bytes])
        handle.seek(data_start + 5 * cluster_bytes)
        handle.write(payload[cluster_bytes:])
    return bytes(sector_zero)


def unpack_entry(sector: bytes, slot: int) -> tuple[int, int, int]:
    offset = MBR_ENTRY_OFFSET + (slot * 16)
    entry = sector[offset : offset + 16]
    return entry[4], *struct.unpack_from("<II", entry, 8)


def unpack_chs_end(sector: bytes, slot: int) -> bytes:
    offset = MBR_ENTRY_OFFSET + (slot * 16)
    return sector[offset + 5 : offset + 8]


def read_at(path: Path, offset: int, length: int = SECTOR_SIZE) -> bytes:
    with path.open("rb") as handle:
        handle.seek(offset)
        return handle.read(length)


def read_firmware_file(path: Path, base_lba: int) -> bytes:
    boot_sector = read_at(path, base_lba * SECTOR_SIZE)
    sector_size = struct.unpack_from("<H", boot_sector, 11)[0]
    if sector_size != SECTOR_SIZE:
        raise ValueError("bootstrap does not expose a 512-byte FAT sector")
    cluster_bytes = boot_sector[13] * sector_size
    reserved = struct.unpack_from("<H", boot_sector, 14)[0]
    fat_size = struct.unpack_from("<I", boot_sector, 36)[0]
    fat_start = (base_lba + reserved) * sector_size
    data_start = fat_start + boot_sector[16] * fat_size * sector_size
    root_cluster = struct.unpack_from("<I", boot_sector, 44)[0]
    root = read_at(path, data_start + (root_cluster - 2) * cluster_bytes, cluster_bytes)
    if root[:11] != b"RPI_EFI FD ":
        raise ValueError("bootstrap does not reach the firmware directory entry")
    cluster = struct.unpack_from("<H", root, 26)[0]
    file_size = struct.unpack_from("<I", root, 28)[0]
    contents = bytearray()
    for _ in range(8):
        contents.extend(read_at(path, data_start + (cluster - 2) * cluster_bytes, cluster_bytes))
        cluster = struct.unpack("<I", read_at(path, fat_start + cluster * 4, 4))[0] & 0x0FFFFFFF
        if cluster >= 0x0FFFFFF8:
            return bytes(contents[:file_size])
    raise ValueError("bootstrap does not reach the end of the firmware file")


class Pi3GptBootstrapTests(unittest.TestCase):
    def test_writes_windows_compatible_protective_mbr_and_is_idempotent(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            image = Path(temp_dir) / "disk.img"
            original_sector = create_gpt_image(image)
            gpt_before = read_at(image, SECTOR_SIZE, 33 * SECTOR_SIZE)
            fat_before = read_at(image, 2048 * SECTOR_SIZE)

            subprocess.run([sys.executable, str(HELPER_PATH), str(image)], check=True)
            sector = read_at(image, 0)
            self.assertEqual(sector[:11], b"\0" * 11)
            self.assertEqual(sector[440:446], original_sector[440:446])
            self.assertEqual(sector[446], 0)
            self.assertEqual(unpack_entry(sector, 0), (0xEE, 1, image.stat().st_size // SECTOR_SIZE - 1))
            self.assertEqual(sector[MBR_ENTRY_OFFSET + 16 : MBR_ENTRY_OFFSET + 64], b"\0" * 48)
            self.assertEqual(sector[510:512], b"\x55\xaa")
            self.assertEqual(read_at(image, SECTOR_SIZE, 33 * SECTOR_SIZE), gpt_before)
            self.assertEqual(read_at(image, 2048 * SECTOR_SIZE), fat_before)
            subprocess.run([sys.executable, str(HELPER_PATH), str(image)], check=True)
            self.assertEqual(read_at(image, 0), sector)

    def test_bootstrap_and_partition_read_identical_fragmented_firmware_files(self) -> None:
        geometries = [
            (2048, 131072, 32, 1, 1024, 2),
            (2048, 3145728, 32, 8, 3066, 2),
            (4096, 262144, 6206, 2, 1024, 5),
            (63, 131072, 64, 1, 1024, 2),
            (65503, 131072, 32, 1, 1024, 2),
        ]
        for start, count, reserved, cluster_size, fat_size, root in geometries:
            with self.subTest(start=start, count=count, reserved=reserved), tempfile.TemporaryDirectory() as temp_dir:
                image = Path(temp_dir) / "disk.img"
                create_gpt_image(image, start, count, reserved, cluster_size, fat_size, root)
                expected = read_firmware_file(image, start)
                subprocess.run([sys.executable, str(HELPER_PATH), str(image)], check=True)
                sector = read_at(image, 0)
                self.assertEqual(struct.unpack_from("<H", sector, 14)[0], start + reserved)
                self.assertEqual(struct.unpack_from("<I", sector, 32)[0], start + count)
                self.assertEqual(struct.unpack_from("<HH", sector, 48), (start + 1, start + 6))
                self.assertEqual(read_firmware_file(image, 0), expected)

    def test_caps_chs_fields_when_the_partition_end_exceeds_the_chs_ceiling(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            image = Path(temp_dir) / "disk.img"
            create_gpt_image(image, sector_count=16_500_000, sectors_per_cluster=8, fat_sectors=16384)

            subprocess.run([sys.executable, str(HELPER_PATH), str(image)], check=True)
            sector = read_at(image, 0)
            self.assertEqual(unpack_entry(sector, 0), (0xEE, 1, image.stat().st_size // SECTOR_SIZE - 1))
            self.assertEqual(unpack_chs_end(sector, 0), b"\xfe\xff\xff")

    def test_check_mode_is_read_only_and_rejects_a_hybrid_mbr(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            image = Path(temp_dir) / "disk.img"
            create_gpt_image(image)
            subprocess.run([sys.executable, str(HELPER_PATH), str(image)], check=True)
            module = load_helper_module()
            with unittest.mock.patch.object(sys, "argv", [str(HELPER_PATH), "--check", str(image)]), \
                    unittest.mock.patch.object(module.os, "pwrite") as write, \
                    unittest.mock.patch.object(module.os, "open", wraps=os.open) as open_device:
                self.assertEqual(module.main(), 0)
            write.assert_not_called()
            open_device.assert_called_once_with(image, os.O_RDONLY)

            with image.open("r+b") as handle:
                handle.seek(466)
                handle.write(b"\x0c")
            before = read_at(image, 0)
            result = subprocess.run(
                [sys.executable, str(HELPER_PATH), "--check", str(image)],
                capture_output=True, text=True, check=False,
            )
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("does not match", result.stderr)
            self.assertEqual(read_at(image, 0), before)

    def test_invalid_fat_geometry_is_rejected_without_a_write(self) -> None:
        changes = [
            (11, struct.pack("<H", 4096), "512-byte"),
            (13, b"\x00", "cluster"),
            (13, b"\x03", "cluster"),
            (14, b"\0\0", "reserved"),
            (16, b"\0", "FAT"),
            (17, b"\x01\0", "FAT32"),
            (19, b"\x01\0", "FAT32"),
            (22, b"\x01\0", "FAT32"),
            (32, struct.pack("<I", 200000), "partition"),
            (32, struct.pack("<I", 10000), "FAT32"),
            (36, struct.pack("<I", 0), "FAT"),
            (36, struct.pack("<I", 1), "FAT"),
            (40, struct.pack("<H", 0x82), "FAT"),
            (42, b"\x01\0", "FAT32"),
            (44, struct.pack("<I", 1), "root cluster"),
            (44, struct.pack("<I", 200000), "root cluster"),
            (48, struct.pack("<H", 33), "FSInfo"),
            (50, struct.pack("<H", 33), "backup"),
            (510, b"\0\0", "signature"),
        ]
        for offset, value, message in changes:
            with self.subTest(offset=offset, value=value), tempfile.TemporaryDirectory() as temp_dir:
                image = Path(temp_dir) / "disk.img"
                create_gpt_image(image)
                with image.open("r+b") as handle:
                    handle.seek(2048 * SECTOR_SIZE + offset)
                    handle.write(value)
                before = read_at(image, 0)
                result = subprocess.run(
                    [sys.executable, str(HELPER_PATH), str(image)],
                    capture_output=True, text=True, check=False,
                )
                self.assertNotEqual(result.returncode, 0)
                self.assertIn(message, result.stderr)
                self.assertEqual(read_at(image, 0), before)

    def test_rejects_unrepresentable_bootstrap_offset_without_writing(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            image = Path(temp_dir) / "disk.img"
            before = create_gpt_image(image, start_lba=65520)
            result = subprocess.run(
                [sys.executable, str(HELPER_PATH), str(image)],
                capture_output=True, text=True, check=False,
            )
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("65535", result.stderr)
            self.assertEqual(read_at(image, 0), before)

    def test_rejects_damaged_gpt_checksums_without_writing(self) -> None:
        for offset in (SECTOR_SIZE + 56, 2 * SECTOR_SIZE + 32):
            with self.subTest(offset=offset), tempfile.TemporaryDirectory() as temp_dir:
                image = Path(temp_dir) / "disk.img"
                before = create_gpt_image(image)
                with image.open("r+b") as handle:
                    handle.seek(offset)
                    handle.write(b"\xff")
                result = subprocess.run(
                    [sys.executable, str(HELPER_PATH), str(image)],
                    capture_output=True, text=True, check=False,
                )
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("checksum", result.stderr)
                self.assertEqual(read_at(image, 0), before)

    @unittest.skipUnless(shutil.which("sgdisk"), "sgdisk is not installed")
    def test_macos_final_retag_preserves_bootstrap(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            image = Path(temp_dir) / "disk.img"
            create_gpt_image(image, partition_type=BASIC_DATA_TYPE)
            subprocess.run([sys.executable, str(HELPER_PATH), str(image)], check=True)
            before = read_at(image, 0)
            subprocess.run(
                ["sgdisk", "-t", "1:ef00", "-c", "1:WOR_BOOT", "-t", "2:0700", "-c", "2:WOR_INSTALL", str(image)],
                check=True, capture_output=True, text=True,
            )
            subprocess.run(
                ["sgdisk", "-A", "1:clear:63", "-A", "2:clear:63", str(image)],
                check=True, capture_output=True, text=True,
            )
            self.assertEqual(read_at(image, 0), before)
            subprocess.run([sys.executable, str(HELPER_PATH), "--check", str(image)], check=True)

    def test_writes_only_sector_zero_and_reports_short_writes(self) -> None:
        module = load_helper_module()
        with tempfile.TemporaryDirectory() as temp_dir:
            image = Path(temp_dir) / "disk.img"
            create_gpt_image(image)
            with unittest.mock.patch.object(sys, "argv", [str(HELPER_PATH), str(image)]), \
                    unittest.mock.patch.object(module.os, "pwrite", wraps=os.pwrite) as write:
                self.assertEqual(module.main(), 0)
            self.assertEqual(write.call_count, 1)
            self.assertEqual(len(write.call_args.args[1]), SECTOR_SIZE)
            self.assertEqual(write.call_args.args[2], 0)
            with unittest.mock.patch.object(sys, "argv", [str(HELPER_PATH), str(image)]), \
                    unittest.mock.patch.object(module.os, "pwrite", return_value=511):
                with self.assertRaisesRegex(OSError, "short write"):
                    module.main()

    def test_successful_write_result_still_requires_matching_readback(self) -> None:
        module = load_helper_module()
        with tempfile.TemporaryDirectory() as temp_dir:
            image = Path(temp_dir) / "disk.img"
            create_gpt_image(image)
            with unittest.mock.patch.object(sys, "argv", [str(HELPER_PATH), str(image)]), \
                    unittest.mock.patch.object(module.os, "pwrite", return_value=512):
                with self.assertRaisesRegex(ValueError, "does not match"):
                    module.main()

    @unittest.skipUnless(
        sys.platform == "darwin" and Path("/sbin/newfs_msdos").is_file()
        and Path("/sbin/fsck_msdos").is_file(),
        "native macOS FAT tools are unavailable",
    )
    def test_native_formatted_bootstrap_geometry_passes_read_only_fsck(self) -> None:
        with tempfile.TemporaryDirectory() as temp_dir:
            fat = Path(temp_dir) / "fat.img"
            image = Path(temp_dir) / "disk.img"
            count = 3145728
            with fat.open("wb") as handle:
                handle.truncate(count * SECTOR_SIZE)
            subprocess.run(
                ["/sbin/newfs_msdos", "-F", "32", "-S", "512", "-s", str(count), "-o", "2048", str(fat)],
                check=True, capture_output=True, text=True,
            )
            boot = read_at(fat, 0)
            reserved = struct.unpack_from("<H", boot, 14)[0]
            fat_size = struct.unpack_from("<I", boot, 36)[0]
            create_gpt_image(
                image, sector_count=count, reserved_sectors=reserved,
                sectors_per_cluster=boot[13], fat_sectors=fat_size,
            )
            prefix_size = (reserved + boot[16] * fat_size + boot[13]) * SECTOR_SIZE
            with image.open("r+b") as handle:
                handle.seek(2048 * SECTOR_SIZE)
                handle.write(read_at(fat, 0, prefix_size))
            subprocess.run([sys.executable, str(HELPER_PATH), str(image)], check=True)
            self.assertEqual(read_at(image, 0, 3), b"\0" * 3)
            #fsck requires a DOS jump signature; the Pi bootstrap follows upstream's
            #zero jump bytes. Change only this disposable probe, not its FAT geometry.
            with image.open("r+b") as handle:
                handle.write(boot[:3])
            result = subprocess.run(
                ["/sbin/fsck_msdos", "-n", str(image)], capture_output=True, text=True, check=False,
            )
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

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

    def test_reads_whole_sectors_so_raw_devices_do_not_reject_the_request(self) -> None:
        #Raw character devices such as /dev/rdiskN on macOS fail every read whose offset or
        #length is not a sector multiple, and the regular-file fixtures above accept those
        #reads happily, so assert on the arguments handed to os.pread rather than on bytes.
        module = load_helper_module()
        real_pread = os.pread
        calls: list[tuple[int, int]] = []

        def recording_pread(fd: int, length: int, offset: int) -> bytes:
            calls.append((length, offset))
            return real_pread(fd, length, offset)

        with tempfile.TemporaryDirectory() as temp_dir:
            image = Path(temp_dir) / "disk.img"
            create_gpt_image(image)
            expected_entry = read_at(image, 1024, 128)
            expected_unaligned = read_at(image, 1030, 20)

            handle = os.open(image, os.O_RDONLY)
            try:
                with unittest.mock.patch("os.pread", recording_pread):
                    entry = module.read_at(handle, 1024, 128)
                    unaligned = module.read_at(handle, 1030, 20)
            finally:
                os.close(handle)

        self.assertEqual(entry, expected_entry)
        self.assertEqual(unaligned, expected_unaligned)
        self.assertTrue(calls)
        for length, offset in calls:
            self.assertEqual(offset % SECTOR_SIZE, 0)
            self.assertEqual(length % SECTOR_SIZE, 0)


if __name__ == "__main__":
    unittest.main()
