#!/usr/bin/env python3
"""Offline FFU contracts. All generated payload bytes are non-Windows fixtures.

Run: python3 -B tests/test-iot-ffu.py
No devices, mounts, network, external packages, or Windows binaries are needed.
Mutation checks load altered copies of the actual helper in memory, never edit it.
"""

from __future__ import annotations

import ast
import contextlib
import hashlib
import io
import json
import os
import re
import shutil
import signal
import struct
import subprocess
import sys
import tempfile
import types
import unittest
import zlib
from dataclasses import replace
from pathlib import Path
from unittest import mock


HELPER_PATH = Path(__file__).resolve().parents[1] / "src/lib/iot-ffu.py"
CHUNK = 1024
TARGET_SIZE = 64 * 512
DEFAULT_DESCRIPTORS = (
    (1, ((0, 0),)),
    (2, ((0, 2), (0, 6))),
    (1, ((0, 0),)),
    (1, ((2, 0),)),
    (1, ((0, 0),)),
)
DEFAULT_RANGES = (0, 1, 3, 1, 5, 1)
PLATFORMS = ("Broadcom.RPi.2", "*")


def load_helper(name: str, code: types.CodeType | None = None) -> types.ModuleType:
    module = types.ModuleType(name)
    module.__file__ = str(HELPER_PATH)
    sys.modules[name] = module
    if code is None:
        code = compile(HELPER_PATH.read_text(), str(HELPER_PATH), "exec")
    exec(code, module.__dict__)
    return module


ffu = load_helper("iot_ffu")


def pad(data: bytes) -> bytes:
    return data + b"\0" * (-len(data) % CHUNK)


def create_gpt_target(path: Path, size: int = 2 * 1024 * 1024) -> tuple[tuple[int, int], ...]:
    data = bytearray(b"\xA5" * size)
    sectors = size // 512
    table = bytearray(128 * 128)
    table[:16] = bytes.fromhex("a2a0d0ebe5b9334487c068b6b72699c7")
    table[16:32] = b"\x11" * 16
    struct.pack_into("<QQ", table, 32, 2048, sectors - 34)
    mbr = bytearray(512)
    mbr[450] = 0xEE
    struct.pack_into("<II", mbr, 454, 1, sectors - 1)
    mbr[510:512] = b"\x55\xaa"
    data[:512] = mbr
    for lba, other, entries in ((1, sectors - 1, 2), (sectors - 1, 1, sectors - 33)):
        header = bytearray(512)
        header[:8] = b"EFI PART"
        struct.pack_into("<II", header, 8, 0x10000, 92)
        struct.pack_into("<QQQQ", header, 24, lba, other, 34, sectors - 34)
        header[56:72] = b"\x22" * 16
        struct.pack_into("<QIII", header, 72, entries, 128, 128, zlib.crc32(table))
        struct.pack_into("<I", header, 16, zlib.crc32(header[:92]))
        data[lba * 512 : (lba + 1) * 512] = header
        data[entries * 512 : entries * 512 + len(table)] = table
    path.write_bytes(data)
    return ((0, 512), (512, 512), (1024, 16384), ((sectors - 33) * 512, 16384), ((sectors - 1) * 512, 512))


def rewrite_gpt_header(data: bytearray, offset: int) -> None:
    struct.pack_into("<I", data, offset + 16, 0)
    struct.pack_into("<I", data, offset + 16, zlib.crc32(data[offset : offset + 92]))


class Fixture:
    def __init__(
        self,
        descriptors: tuple[tuple[int, tuple[tuple[int, int], ...]], ...] = DEFAULT_DESCRIPTORS,
        ranges: tuple[int, ...] = DEFAULT_RANGES,
        manifest: str | None = None,
        platforms: tuple[str, ...] = PLATFORMS,
    ) -> None:
        self.manifest = manifest if manifest is not None else (
            "[FullFlash]\n"
            "AntiTheftVersion = 1.1\n"
            "Description = Synthetic non-Windows test fixture\n"
            "StateSeparationLevel = 0\n"
            "OSVersion = 10.0.17763.107\n"
            "Version = 2.0\n"
            "UEFI = True\n"
            "DevicePlatformId0 = Broadcom.RPi.2\n"
            "DevicePlatformId1 = *\n"
            "[Store]\n"
            "OnlyAllocateDefinedGptEntries = False\n"
            "StoreType = Default\n"
            "MinSectorCount = 64\n"
            "IsMainOSStore = True\n"
            "SectorSize = 512\n"
            "[Partition]\n"
            "Name = NonWindowsFixture\n"
            "TotalSectors = 16\n"
        )
        manifest_bytes = self.manifest.encode("ascii")
        descriptor_bytes = b"".join(
            struct.pack("<II", len(locations), count)
            + b"".join(struct.pack("<II", method, index) for method, index in locations)
            for count, locations in descriptors
        )
        platform_bytes = b"\0".join(item.encode("ascii") for item in platforms) + b"\0"
        store = struct.pack(
            "<IHHHH192s11I", 0, 1, 0, 2, 0, platform_bytes, CHUNK,
            len(descriptors), len(descriptor_bytes), 0, 0, *ranges,
        )
        image = pad(struct.pack("<I12sII", 24, b"ImageFlash  ", len(manifest_bytes), 1) + manifest_bytes)
        store_area = pad(store + descriptor_bytes)
        self.payload = tuple(
            bytes([48 + index % 40]) * CHUNK for index in range(sum(count for count, _ in descriptors))
        )
        body = image + store_area + b"".join(self.payload)
        hashes = b"".join(hashlib.sha256(body[i : i + CHUNK]).digest() for i in range(0, len(body), CHUNK))
        catalog = b"\x30\x03\x02\x01\x01"
        security = struct.pack("<I12sIIII", 32, b"SignedImage ", 1, 0x800C, len(catalog), len(hashes))
        prefix = pad(security + catalog + hashes)
        self.blob = prefix + body
        self.hash_offset = 32 + len(catalog)
        self.image_offset = len(prefix)
        self.store_offset = self.image_offset + len(image)
        self.descriptor_offset = self.store_offset + 248
        self.payload_offset = self.store_offset + len(store_area)

    def rehash(self, data: bytearray) -> bytes:
        for index, offset in enumerate(range(self.image_offset, len(data), CHUNK)):
            digest = hashlib.sha256(data[offset : offset + CHUNK]).digest()
            start = self.hash_offset + index * 32
            data[start : start + 32] = digest
        return bytes(data)

    def mutate(self, offset: int, encoding: str, value: int, rehash: bool = True) -> bytes:
        data = bytearray(self.blob)
        struct.pack_into(encoding, data, offset, value)
        return self.rehash(data) if rehash else bytes(data)


def fixture_approval(info: object) -> object:
    return ffu.ApprovedImage(
        info.sha256, "ffu-v1", "iot-core", "arm32", "17763.107",
        info.os_version, info.platform_ids, ("pi2-v1.1", "pi2-v1.2", "pi3-b"),
        info.file_bytes, info.payload_bytes, info.minimum_disk_bytes,
        info.sector_size, info.chunk_size, "https://example.invalid/synthetic-fixture",
    )


class ParserTests(unittest.TestCase):
    def setUp(self) -> None:
        self.fixture = Fixture()

    def parse(self, blob: bytes | None = None) -> object:
        return ffu.parse_ffu(io.BytesIO(self.fixture.blob if blob is None else blob))

    def test_valid_multi_location_multi_block_and_table_phases(self) -> None:
        result = self.parse()
        self.assertEqual(result.sha256, hashlib.sha256(self.fixture.blob).hexdigest())
        self.assertEqual(result.platform_ids, PLATFORMS)
        self.assertEqual(result.payload_offset, self.fixture.payload_offset)
        self.assertEqual(result.payload_bytes, 6 * CHUNK)
        self.assertEqual(result.minimum_disk_bytes, TARGET_SIZE)
        self.assertEqual(len(result.chunk_hashes), 8 * 32)
        self.assertEqual([d.payload_block for d in result.descriptors], [0, 1, 3, 4, 5])
        self.assertEqual([d.phase for d in result.descriptors], ["initial", "data", "flash", "data", "final"])
        self.assertEqual(result.descriptors[1].block_count, 2)
        self.assertEqual(len(result.descriptors[1].locations), 2)

    def test_payload_corruption_is_rejected(self) -> None:
        data = bytearray(self.fixture.blob)
        data[self.fixture.payload_offset + CHUNK + 21] ^= 1
        with self.assertRaisesRegex(ffu.FfuError, "chunk hash"):
            self.parse(bytes(data))

    def test_metadata_chunks_are_hashed_too(self) -> None:
        for offset in (self.fixture.image_offset + 80, self.fixture.store_offset + 245):
            with self.subTest(offset=offset):
                data = bytearray(self.fixture.blob)
                data[offset] ^= 1
                with self.assertRaises(ffu.FfuError):
                    self.parse(bytes(data))

    def test_hash_table_corruption_is_rejected(self) -> None:
        data = bytearray(self.fixture.blob)
        data[self.fixture.hash_offset + 7 * 32] ^= 1
        with self.assertRaisesRegex(ffu.FfuError, "chunk hash"):
            self.parse(bytes(data))

    def test_catalog_is_in_the_whole_file_digest(self) -> None:
        data = bytearray(self.fixture.blob)
        data[36] = 2
        result = self.parse(bytes(data))
        self.assertNotEqual(result.sha256, self.parse().sha256)
        with self.assertRaisesRegex(ffu.FfuError, "SHA256"):
            ffu._check_approval(result, fixture_approval(self.parse()))

    def test_invalid_header_fields_fail_even_with_recomputed_hashes(self) -> None:
        image, store, desc = self.fixture.image_offset, self.fixture.store_offset, self.fixture.descriptor_offset
        cases = {
            "security size": (0, "<I", 40),
            "chunk zero": (16, "<I", 0),
            "chunk non-power": (16, "<I", 3),
            "chunk excessive": (16, "<I", 2048),
            "SHA1": (20, "<I", 0x8004),
            "catalog zero": (24, "<I", 0),
            "catalog excessive": (24, "<I", 2**32 - 1),
            "hash zero": (28, "<I", 0),
            "hash unaligned": (28, "<I", 33),
            "hash excessive": (28, "<I", 2**32 - 1),
            "hash incomplete": (28, "<I", 7 * 32),
            "extended image": (image, "<I", 28),
            "manifest empty": (image + 16, "<I", 0),
            "manifest excessive": (image + 16, "<I", 65537),
            "chunk disagreement": (image + 20, "<I", 2),
            "delta": (store, "<I", 1),
            "multistore": (store + 4, "<H", 2),
            "store minor": (store + 6, "<H", 1),
            "compressed": (store + 8, "<H", 3),
            "FFU minor": (store + 10, "<H", 1),
            "block size": (store + 204, "<I", 512),
            "descriptor count zero": (store + 208, "<I", 0),
            "descriptor count excessive": (store + 208, "<I", 2**32 - 1),
            "descriptor length excessive": (store + 212, "<I", 2**32 - 1),
            "descriptor length short": (store + 212, "<I", 87),
            "conditional count": (store + 216, "<I", 1),
            "conditional bytes": (store + 220, "<I", 12),
            "initial not first": (store + 224, "<I", 1),
            "initial overflow": (store + 228, "<I", 2**32 - 1),
            "phase crossing": (store + 232, "<I", 2),
            "unordered final phase": (store + 240, "<I", 2),
            "absent final": (store + 244, "<I", 0),
            "zero locations": (desc, "<I", 0),
            "excessive locations": (desc, "<I", 17),
            "zero blocks": (desc + 4, "<I", 0),
            "excessive blocks": (desc + 4, "<I", 2**32 - 1),
            "unsupported method": (desc + 8, "<I", 1),
            "huge target index": (desc + 12, "<I", 2**32 - 1),
        }
        for label, (offset, encoding, value) in cases.items():
            with self.subTest(label=label), self.assertRaises(ffu.FfuError):
                self.parse(self.fixture.mutate(offset, encoding, value))

    def test_bad_magic_padding_catalog_and_platform_encoding(self) -> None:
        for offset, value in (
            (4, ord("X")),
            (32, 0x31),
            (self.fixture.hash_offset + 8 * 32, 1),
            (self.fixture.image_offset + 4, ord("X")),
            (self.fixture.store_offset - 1, 1),
            (self.fixture.payload_offset - 1, 1),
            (self.fixture.store_offset + 12 + 191, 1),
            (self.fixture.store_offset + 12, 0xFF),
        ):
            with self.subTest(offset=offset), self.assertRaises(ffu.FfuError):
                self.parse(self.fixture.mutate(offset, "<B", value))
        with self.assertRaisesRegex(ffu.FfuError, "unterminated platform"):
            self.parse(Fixture(platforms=("A", "B" * 190)).blob)

    def test_manifest_rejects_ambiguity_and_unsupported_geometry(self) -> None:
        text = self.fixture.manifest
        manifests = (
            text + "[Store]\nStoreType = Default\n",
            text.replace("Version = 2.0", "Version = 3.0"),
            text.replace("Version = 2.0", "Version = 2.0\nversion = 2.0"),
            text.replace("SectorSize = 512", "SectorSize = 4096"),
            text.replace("MinSectorCount = 64", "MinSectorCount = -1"),
            text.replace("MinSectorCount = 64", "MinSectorCount = 999999999999999999999"),
            text.replace("StoreType = Default", "StoreType = Update"),
            text.replace("IsMainOSStore = True", "IsMainOSStore = False"),
            text.replace("OnlyAllocateDefinedGptEntries = False", "OnlyAllocateDefinedGptEntries = True"),
            text.replace("DevicePlatformId0 = Broadcom.RPi.2", "DevicePlatformId0 = SomeOther.Board"),
            text.replace("OSVersion = 10.0.17763.107", "OSVersion = not-a-version"),
            text.replace("Name = NonWindowsFixture", "Name ="),
            text + "[Partition]\nName = NonWindowsFixture\n",
        )
        for manifest in manifests:
            with self.subTest(manifest=manifest), self.assertRaises(ffu.FfuError):
                self.parse(Fixture(manifest=manifest).blob)
        with self.assertRaisesRegex(ffu.FfuError, "ASCII"):
            self.parse(self.fixture.mutate(self.fixture.image_offset + 30, "<B", 0xFF))

    def test_truncation_and_trailing_data(self) -> None:
        blob = self.fixture.blob
        for cut in (0, 31, self.fixture.image_offset - 1, self.fixture.payload_offset - 1, len(blob) - 1, len(blob) - CHUNK):
            with self.subTest(cut=cut), self.assertRaises(ffu.FfuError):
                self.parse(blob[:cut])
        for extra in (b"\0", b"\0" * CHUNK):
            with self.subTest(extra=len(extra)), self.assertRaises(ffu.FfuError):
                self.parse(blob + extra)

    def test_overlaps_are_not_generic_last_writer_wins(self) -> None:
        cases = (
            (((2, ((0, 2),)), (1, ((0, 3),))), (0,) * 6),
            (((1, ((0, 2), (0, 2))),), (0,) * 6),
            (((1, ((0, 2),)), (1, ((0, 2),))), (0,) * 6),
            (((1, ((0, 0),)), (1, ((0, 2),)), (1, ((0, 1),))), (0, 1, 0, 0, 2, 1)),
        )
        for descriptors, ranges in cases:
            with self.subTest(descriptors=descriptors), self.assertRaises(ffu.FfuError):
                self.parse(Fixture(descriptors, ranges).blob)

    def test_end_relative_multi_block_starts_before_forward_run(self) -> None:
        good = Fixture(((2, ((2, 1),)),), (0,) * 6)
        info = self.parse(good.blob)
        plan = ffu._resolve_plan(info, TARGET_SIZE + 512)
        self.assertEqual(plan.writes[0].offset, TARGET_SIZE + 512 - 2 * CHUNK)
        self.assertEqual(plan.writes[0].block_count, 2)
        with self.assertRaises(ffu.FfuError):
            self.parse(Fixture(((2, ((2, 0),)),), (0,) * 6).blob)

    def test_target_dependent_start_end_overlap_is_rechecked(self) -> None:
        fixture = Fixture(((1, ((0, 30),)), (1, ((2, 3),))), (0,) * 6)
        info = self.parse(fixture.blob)
        with self.assertRaisesRegex(ffu.FfuError, "overlap"):
            ffu._resolve_plan(info, 34 * CHUNK)

    def test_large_counts_do_not_cause_large_reads(self) -> None:
        class BoundedReader(io.BytesIO):
            def read(self, size: int = -1) -> bytes:
                if not 0 <= size <= CHUNK:
                    raise AssertionError(f"unbounded fixture read: {size}")
                return super().read(size)

        ffu.parse_ffu(BoundedReader(self.fixture.blob))
        with self.assertRaises(ffu.FfuError):
            ffu.parse_ffu(BoundedReader(self.fixture.mutate(self.fixture.descriptor_offset + 4, "<I", 2**32 - 1)))


class FileFixture(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory(prefix="iot-ffu-test-")
        self.addCleanup(self.temporary.cleanup)
        self.directory = Path(self.temporary.name).resolve()
        self.fixture = Fixture()
        self.source = self.directory / "not Windows media.ffu"
        self.target = self.directory / "ordinary-target.raw"
        self.source.write_bytes(self.fixture.blob)
        self.original = b"\xA5" * TARGET_SIZE
        self.target.write_bytes(self.original)

    def apply(self, **kwargs: object) -> object:
        return ffu.apply_to_file_fixture(self.source, self.target, TARGET_SIZE, **kwargs)

    def expected_target(self, size: int = TARGET_SIZE) -> bytes:
        data = bytearray(b"\xA5" * size)
        data[:CHUNK] = self.fixture.payload[5]
        data[2 * CHUNK : 4 * CHUNK] = self.fixture.payload[1] + self.fixture.payload[2]
        data[6 * CHUNK : 8 * CHUNK] = self.fixture.payload[1] + self.fixture.payload[2]
        data[-CHUNK:] = self.fixture.payload[4]
        return bytes(data)


class HdmiFixture(Fixture):
    boot_start = 2 * 512
    fat_start = boot_start + 512
    root_start = boot_start + 65 * 512
    data_start = boot_start + 97 * 512
    size = 8 * 1024 * 1024
    config = (
        "kernel_old=1\r\ngpu_mem=32\r\nframebuffer_ignore_alpha=1\r\nframebuffer_swap=1\r\n"
        "disable_overscan=1\r\nhdmi_force_hotplug=1 # Original comment\r\nhdmi_group=2\r\n"
        "hdmi_cvt 800 480 60 6 0 0 0\r\n" + "# Non-Windows firmware fixture\r\n" * 16
    ).encode("ascii")

    def __init__(self) -> None:
        super().__init__(((128, ((0, 0),)),), (0,) * 6, manifest=(
            "[FullFlash]\nOSVersion=10.0.17763.107\nVersion=2.0\nUEFI=True\n"
            "DevicePlatformId0=Broadcom.RPi.2\nDevicePlatformId1=*\n"
            "[Store]\nMinSectorCount=16384\nSectorSize=512\nIsMainOSStore=True\n"
            "StoreType=Default\nOnlyAllocateDefinedGptEntries=False\n"
            "[Partition]\nName=NonWindowsFixture\nTotalSectors=8192\n"
        ))
        self.disk = bytearray(128 * CHUNK)
        self.disk[450] = 0x0C
        struct.pack_into("<II", self.disk, 454, 2, 8192)
        self.disk[510:512] = b"\x55\xaa"
        boot = self.boot_start
        struct.pack_into("<HBHBHH", self.disk, boot + 11, 512, 1, 1, 2, 512, 8192)
        self.disk[boot + 21] = 0xF8
        struct.pack_into("<H", self.disk, boot + 22, 32)
        self.disk[boot + 54 : boot + 62] = b"FAT16   "
        self.disk[boot + 510 : boot + 512] = b"\x55\xaa"
        for fat in (self.fat_start, self.fat_start + 32 * 512):
            struct.pack_into("<5H", self.disk, fat, 0xFFF8, 0xFFFF, 3, 0xFFFF, 0xFFFF)
        entry = self.root_start
        self.disk[entry : entry + 11] = b"CONFIG  TXT"
        self.disk[entry + 11] = 0x20
        struct.pack_into("<H", self.disk, entry + 26, 2)
        struct.pack_into("<I", self.disk, entry + 28, len(self.config))
        self.disk[entry + 32 : entry + 43] = b"KERNEL  IMG"
        self.disk[entry + 43] = 0x20
        struct.pack_into("<H", self.disk, entry + 58, 4)
        struct.pack_into("<I", self.disk, entry + 60, 512)
        self.disk[self.data_start : self.data_start + len(self.config)] = self.config
        self.disk[self.data_start + 1024 : self.data_start + 1536] = b"NOT AN EXECUTABLE".ljust(512, b"\0")
        self.refresh()

    def refresh(self) -> None:
        blob = bytearray(self.blob)
        blob[self.payload_offset:] = self.disk
        self.blob = self.rehash(blob)


class HdmiTests(FileFixture):
    def setUp(self) -> None:
        super().setUp()
        self.fixture = HdmiFixture()
        self.source.write_bytes(self.fixture.blob)
        with self.target.open("wb") as target:
            target.truncate(self.fixture.size)

    def write(self, **kwargs: object) -> None:
        ffu.apply_to_file_fixture(self.source, self.target, self.fixture.size, **kwargs)

    def read_config(self) -> bytes:
        data = self.target.read_bytes()
        size = struct.unpack_from("<I", data, self.fixture.root_start + 28)[0]
        return data[self.fixture.data_start : self.fixture.data_start + size]

    def test_official_is_unchanged_and_does_not_need_a_boot_config(self) -> None:
        self.write()
        self.assertEqual(self.target.read_bytes()[:len(self.fixture.disk)], self.fixture.disk)
        self.assertEqual(self.source.read_bytes(), self.fixture.blob)
        self.assertEqual(ffu.hdmi_options()["settings"], "")
        self.assertFalse(ffu.hdmi_options()["recommended"])
        self.assertNotIn("Recommended", ffu.hdmi_options()["label"])
        recommended = [choice for choice in ffu.hdmi_options()["choices"] if choice["recommended"]]
        self.assertEqual([choice["value"] for choice in recommended], ["720p60"])
        self.assertIn("Recommended", recommended[0]["menu_label"])

    def test_presets_and_custom_preserve_every_non_display_byte(self) -> None:
        for mode, custom, wanted in (
            ("720p60", "", b"hdmi_mode=4\r\n"),
            ("1080p60", "", b"hdmi_mode=16\r\n"),
            ("custom", "# Reviewed DMT timing\nhdmi_group=2\nhdmi_mode=16\nhdmi_drive=1", b"hdmi_drive=1\r\n"),
        ):
            with self.subTest(mode=mode):
                self.write(hdmi_mode=mode, hdmi_config=custom)
                config = self.read_config()
                self.assertIn(wanted, config)
                for retained in (b"kernel_old=1", b"gpu_mem=32", b"framebuffer_ignore_alpha=1", b"framebuffer_swap=1", b"hdmi_cvt 800 480"):
                    self.assertIn(retained, config)
                source = self.source.read_bytes()
                self.assertEqual(source, self.fixture.blob)
                disk = bytearray(self.target.read_bytes()[:len(self.fixture.disk)])
                disk[self.fixture.data_start : self.fixture.data_start + 1024] = self.fixture.disk[self.fixture.data_start : self.fixture.data_start + 1024]
                directory_size = self.fixture.root_start + 28
                disk[directory_size : directory_size + 4] = self.fixture.disk[directory_size : directory_size + 4]
                self.assertEqual(disk, self.fixture.disk)
                self.assertEqual(config.count(b"hdmi_group="), 1)
                self.assertEqual(config.count(b"hdmi_mode="), 1)

    def test_invalid_preferences_fail_before_the_target_is_opened(self) -> None:
        for mode, text in (
            ("unknown", ""), ("custom", ""), ("custom", "kernel_old=0"),
            ("custom", "hdmi_group=1\nhdmi_mode=4\nkernel_old=0"),
            ("custom", "hdmi_group=1\nhdmi_mode=4\ninclude malicious.txt"),
            ("custom", "hdmi_group=1\nhdmi_mode=4\nhdmi_mode=16"),
            ("custom", "hdmi_group=0\nhdmi_mode=4"), ("custom", "hdmi_group=2\nhdmi_mode=87"),
            ("custom", "hdmi_group=1\nhdmi_mode=4\nhdmi_drive=3"),
            ("custom", "hdmi_group=1\nhdmi_mode=4\nhdmi_force_hotplug=2"),
            ("custom", "hdmi_group=1\nhdmi_mode=4\n[all]"),
            ("custom", "#" * 513), ("custom", "# \u00e9"), ("custom", "\0"),
        ):
            with self.subTest(mode=mode, text=text), mock.patch.object(ffu, "_open_path", wraps=ffu._open_path) as opened:
                with self.assertRaises(ffu.FfuError):
                    self.write(hdmi_mode=mode, hdmi_config=text)
                self.assertTrue(all(call.args[2] == "source" for call in opened.call_args_list))

    def test_corrupt_fat_config_or_allocation_fails_before_any_write(self) -> None:
        for mutation in ("mbr", "fat-copy", "cycle", "missing", "duplicate", "oversized", "filtered", "short-chain"):
            with self.subTest(mutation=mutation):
                fixture = HdmiFixture()
                if mutation == "mbr":
                    fixture.disk[450] = 0xEE
                elif mutation == "fat-copy":
                    fixture.disk[fixture.fat_start + 32 * 512 + 4] ^= 1
                elif mutation in ("cycle", "short-chain"):
                    for fat in (fixture.fat_start, fixture.fat_start + 32 * 512):
                        struct.pack_into("<H", fixture.disk, fat + 4, 2 if mutation == "cycle" else 0xFFFF)
                elif mutation == "missing":
                    fixture.disk[fixture.root_start] = 0xE5
                elif mutation == "duplicate":
                    fixture.disk[fixture.root_start + 32 : fixture.root_start + 64] = fixture.disk[fixture.root_start : fixture.root_start + 32]
                elif mutation == "oversized":
                    struct.pack_into("<I", fixture.disk, fixture.root_start + 28, 4097)
                elif mutation == "filtered":
                    fixture.disk[fixture.data_start : fixture.data_start + 6] = b"[all]\n"
                fixture.refresh()
                self.source.write_bytes(fixture.blob)
                with mock.patch.object(ffu, "_write_all") as write:
                    with self.assertRaises(ffu.FfuError):
                        self.write(hdmi_mode="720p60")
                    self.assertEqual(write.call_count, 0)

    def test_configured_block_corruption_is_not_reported_as_verified(self) -> None:
        original_read = ffu._read_target

        def corrupt(fd: int, size: int, offset: int, sector: int) -> bytes:
            actual = original_read(fd, size, offset, sector)
            if offset <= self.fixture.data_start < offset + size:
                return bytes([actual[0] ^ 1]) + actual[1:]
            return actual

        with mock.patch.object(ffu, "_read_target", side_effect=corrupt):
            with self.assertRaisesRegex(ffu.ApplyError, "readback mismatch"):
                self.write(hdmi_mode="720p60")

    def test_source_integrity_is_required_for_configured_blocks(self) -> None:
        blob = bytearray(self.fixture.blob)
        blob[self.fixture.payload_offset + self.fixture.data_start] ^= 1
        self.source.write_bytes(blob)
        with mock.patch.object(ffu, "_write_all") as write:
            with self.assertRaises(ffu.FfuError):
                self.write(hdmi_mode="720p60")
            self.assertEqual(write.call_count, 0)

    def test_exact_custom_text_limit_and_required_timing(self) -> None:
        text = "hdmi_group=1\nhdmi_mode=4\n#"
        boundary = text + "x" * (512 - len(text))
        self.assertEqual(ffu.hdmi_options("custom", boundary)["settings"], "hdmi_group=1\nhdmi_mode=4\n")
        with self.assertRaises(ffu.FfuError):
            ffu.hdmi_options("custom", boundary + "x")

    def test_oversized_output_does_not_allocate_new_clusters_or_write(self) -> None:
        fixture = HdmiFixture()
        text = (b"#" + b"x" * 937 + b"\r\n") + b"kernel_old=1\r\nhdmi_group=2\r\n"
        fixture.disk[fixture.data_start : fixture.data_start + len(text)] = text
        struct.pack_into("<I", fixture.disk, fixture.root_start + 28, len(text))
        fixture.refresh()
        self.source.write_bytes(fixture.blob)
        custom = "hdmi_group=1\nhdmi_mode=4\nhdmi_force_hotplug=1\nhdmi_drive=2"
        with mock.patch.object(ffu, "_write_all") as write:
            with self.assertRaisesRegex(ffu.FfuError, "exceed the existing"):
                self.write(hdmi_mode="custom", hdmi_config=custom)
            self.assertEqual(write.call_count, 0)

    def test_full_wipe_preserves_the_verified_hdmi_overlay(self) -> None:
        self.target.write_bytes(b"\xA5" * self.fixture.size)
        self.write(hdmi_mode="720p60", wipe_entire_drive=True)
        self.assertIn(b"hdmi_mode=4\r\n", self.read_config())
        self.assertIn(b"kernel_old=1\r\n", self.read_config())
        self.assertFalse(any(self.target.read_bytes()[len(self.fixture.disk):]))
        self.assertEqual(self.source.read_bytes(), self.fixture.blob)


class FileTests(FileFixture):
    def test_exact_final_extents_preserve_holes_and_size(self) -> None:
        self.apply()
        self.assertEqual(self.target.read_bytes(), self.expected_target())
        self.assertEqual(self.target.stat().st_size, TARGET_SIZE)
        self.assertEqual(self.source.read_bytes(), self.fixture.blob)

    def test_non_chunk_aligned_but_sector_aligned_disk(self) -> None:
        size = TARGET_SIZE + 512
        self.target.write_bytes(b"\xA5" * size)
        ffu.apply_to_file_fixture(self.source, self.target, size)
        self.assertEqual(self.target.read_bytes(), self.expected_target(size))

    def test_multiblock_payload_is_replicated_at_start_and_end_locations(self) -> None:
        fixture = Fixture(((2, ((0, 2), (2, 1))),), (0,) * 6)
        self.source.write_bytes(fixture.blob)
        self.apply()
        expected = bytearray(self.original)
        expected[2 * CHUNK : 4 * CHUNK] = b"".join(fixture.payload)
        expected[-2 * CHUNK :] = b"".join(fixture.payload)
        self.assertEqual(self.target.read_bytes(), expected)

    def test_larger_target_keeps_fixed_mbr_layout_and_unused_tail(self) -> None:
        descriptors = list(DEFAULT_DESCRIPTORS)
        descriptors[3] = (1, ((0, 10),))
        fixture = Fixture(tuple(descriptors))
        mbr = bytearray(512)
        mbr[450] = 0x0C
        struct.pack_into("<II", mbr, 454, 4, 4)
        mbr[510:512] = b"\x55\xaa"
        blob = bytearray(fixture.blob)
        start = fixture.payload_offset + 5 * CHUNK
        blob[start : start + 512] = mbr
        self.source.write_bytes(fixture.rehash(blob))
        size = 16 * 1024 * 1024
        sentinel = b"UNUSED CAPACITY".ljust(512, b"\0")
        with self.target.open("r+b") as target:
            target.truncate(size)
            target.seek(size - 512)
            target.write(sentinel)
        info = ffu.apply_to_file_fixture(self.source, self.target, size)
        self.assertEqual(ffu._resolve_plan(info, TARGET_SIZE), ffu._resolve_plan(info, size))
        self.assertEqual(self.target.stat().st_size, size)
        with self.target.open("rb") as target:
            self.assertEqual(target.read(512), mbr)
            target.seek(size - 512)
            self.assertEqual(target.read(512), sentinel)

    def test_existing_gpt_headers_are_rejected_before_writes(self) -> None:
        for size, offset in (
            (TARGET_SIZE, 512),
            (TARGET_SIZE + 2 * CHUNK, TARGET_SIZE - 512),
            (TARGET_SIZE + 2 * CHUNK, TARGET_SIZE + 2 * CHUNK - 512),
        ):
            before = bytearray(b"\xA5" * size)
            before[offset : offset + 8] = b"EFI PART"
            self.target.write_bytes(before)
            with self.subTest(size=size, offset=offset), mock.patch.object(ffu.os, "pwrite") as write:
                with self.assertRaisesRegex(ffu.TargetError, "existing GPT"):
                    ffu.apply_to_file_fixture(self.source, self.target, size)
            write.assert_not_called()
            self.assertEqual(self.target.read_bytes(), before)

    def test_orphaned_backup_gpt_refused_before_any_write(self) -> None:
        before = bytearray(self.original)
        before[-512:-504] = b"EFI PART"
        self.target.write_bytes(before)
        with self.assertRaisesRegex(ffu.TargetError, "existing GPT"):
            self.apply()
        self.assertEqual(self.target.read_bytes(), before)

    def test_entire_source_validated_before_target_is_opened(self) -> None:
        blob = bytearray(self.fixture.blob)
        blob[-1] ^= 1
        self.source.write_bytes(blob)
        opener = ffu._open_path

        def guarded(path: Path, flags: int, kind: str, source_id: object = None) -> int:
            self.assertEqual(kind, "source", "target opened before complete source validation")
            return opener(path, flags, kind, source_id)

        with mock.patch.object(ffu, "_open_path", side_effect=guarded), self.assertRaises(ffu.FfuError):
            self.apply()
        self.assertEqual(self.target.read_bytes(), self.original)

    def test_unapproved_preflight_never_opens_target(self) -> None:
        opener = ffu._open_path

        def guarded(path: Path, flags: int, kind: str, source_id: object = None) -> int:
            self.assertEqual(kind, "source", "target opened for an unapproved FFU")
            return opener(path, flags, kind, source_id)

        with mock.patch.object(ffu, "_open_path", side_effect=guarded), self.assertRaisesRegex(ffu.FfuError, "allowlisted"):
            ffu.apply_ffu(self.source, self.target, TARGET_SIZE)
        self.assertEqual(self.target.read_bytes(), self.original)

    def test_target_bounds_fail_before_open(self) -> None:
        for size in (TARGET_SIZE - 512, TARGET_SIZE + 1, 0, -1, True, ffu.MAX_TARGET_BYTES + 512):
            with self.subTest(size=size), mock.patch.object(ffu.os, "pwrite") as write:
                with self.assertRaises(ffu.TargetError):
                    ffu.apply_to_file_fixture(self.source, self.target, size)
                write.assert_not_called()
        self.assertEqual(self.target.read_bytes(), self.original)

    def test_actual_target_size_must_match(self) -> None:
        self.target.write_bytes(self.original + b"\xA5" * 512)
        with mock.patch.object(ffu.os, "pwrite") as write, self.assertRaisesRegex(ffu.TargetError, "actual target"):
            self.apply()
        write.assert_not_called()
        self.assertEqual(self.target.read_bytes(), self.original + b"\xA5" * 512)

    def test_samefile_and_hardlinks_are_refused(self) -> None:
        alias = self.directory / "alias.ffu"
        os.link(self.source, alias)
        for target in (self.source, alias):
            with self.subTest(target=target), self.assertRaisesRegex(ffu.TargetError, "same file"):
                ffu.apply_to_file_fixture(self.source, target, TARGET_SIZE)
        self.assertEqual(self.source.read_bytes(), self.fixture.blob)
        alias.unlink()
        os.link(self.target, alias)
        with self.assertRaisesRegex(ffu.TargetError, "hard-link"):
            self.apply()
        self.assertEqual(alias.read_bytes(), self.original)

    def test_source_target_and_parent_symlinks_are_refused(self) -> None:
        source_link = self.directory / "source-link"
        target_link = self.directory / "target-link"
        parent_link = self.directory / "directory-link"
        source_link.symlink_to(self.source)
        target_link.symlink_to(self.target)
        parent_link.symlink_to(self.directory, target_is_directory=True)
        for source, target in (
            (source_link, self.target), (self.source, target_link),
            (parent_link / self.source.name, self.target), (self.source, parent_link / self.target.name),
        ):
            with self.subTest(source=source, target=target), self.assertRaises((ffu.FfuError, OSError)):
                ffu.apply_to_file_fixture(source, target, TARGET_SIZE)
        self.assertEqual(self.target.read_bytes(), self.original)

    def test_no_target_creation_or_truncation(self) -> None:
        missing = self.directory / "not-created"
        with self.assertRaises(FileNotFoundError):
            ffu.apply_to_file_fixture(self.source, missing, TARGET_SIZE)
        self.assertFalse(missing.exists())
        opener = ffu.os.open

        def guarded(path: object, flags: int, *args: object, **kwargs: object) -> int:
            self.assertFalse(flags & (os.O_TRUNC | os.O_CREAT))
            self.assertTrue(flags & os.O_NOFOLLOW)
            return opener(path, flags, *args, **kwargs)

        with mock.patch.object(ffu.os, "open", side_effect=guarded):
            self.apply()

    def test_target_replaced_during_open_is_refused(self) -> None:
        replacement = self.directory / "replacement"
        replacement.write_bytes(self.original)
        opener = ffu.os.open

        def swap(path: object, flags: int, *args: object, **kwargs: object) -> int:
            if path == self.target.name and flags & os.O_RDWR:
                os.replace(replacement, self.target)
            return opener(path, flags, *args, **kwargs)

        with mock.patch.object(ffu.os, "open", side_effect=swap), self.assertRaisesRegex(ffu.TargetError, "identity changed"):
            self.apply()
        self.assertEqual(self.target.read_bytes(), self.original)

    def test_aligned_short_io_retries_and_streams_one_chunk(self) -> None:
        real_write, real_read = os.pwrite, os.pread
        writes: list[tuple[int, int]] = []
        reads: list[tuple[int, int]] = []

        def write(fd: int, data: bytes, offset: int) -> int:
            writes.append((len(data), offset))
            return real_write(fd, data[:512], offset)

        def read(fd: int, size: int, offset: int) -> bytes:
            reads.append((size, offset))
            return real_read(fd, min(size, 512), offset)

        with mock.patch.object(ffu.os, "pwrite", side_effect=write), mock.patch.object(ffu.os, "pread", side_effect=read):
            self.apply()
        self.assertEqual(self.target.read_bytes(), self.expected_target())
        self.assertTrue(writes and reads)
        self.assertTrue(all(0 < size <= CHUNK and size % 512 == 0 and offset % 512 == 0 for size, offset in writes + reads))
        self.assertEqual(len(writes), 16)
        self.assertEqual(len(reads), 14)

    def test_zero_unaligned_write_and_readback_eof_fail(self) -> None:
        for value in (0, 1):
            with self.subTest(write=value), mock.patch.object(ffu.os, "pwrite", return_value=value):
                with self.assertRaisesRegex(ffu.ApplyError, "write"):
                    self.apply()
        with mock.patch.object(ffu.os, "pread", return_value=b""), self.assertRaisesRegex(ffu.ApplyError, "readback"):
            self.apply()

    def test_fsync_and_all_reads_happen_on_same_open_target(self) -> None:
        real_read, real_write, real_sync = os.pread, os.pwrite, os.fsync
        trace: list[tuple[str, int]] = []

        def read(fd: int, size: int, offset: int) -> bytes:
            trace.append(("read", fd))
            return real_read(fd, size, offset)

        def write(fd: int, data: bytes, offset: int) -> int:
            trace.append(("write", fd))
            return real_write(fd, data, offset)

        def sync(fd: int) -> None:
            trace.append(("sync", fd))
            real_sync(fd)

        with mock.patch.object(ffu.os, "pread", side_effect=read), mock.patch.object(ffu.os, "pwrite", side_effect=write), mock.patch.object(ffu.os, "fsync", side_effect=sync):
            self.apply()
        self.assertEqual(len({fd for _, fd in trace}), 1)
        kinds = [kind for kind, _ in trace]
        self.assertEqual(kinds.count("write"), 8)
        self.assertEqual(kinds.count("read"), 8)
        self.assertEqual(kinds.count("sync"), 5)
        self.assertEqual(kinds[:kinds.index("write")], ["read", "read"])
        last_write = max(index for index, kind in enumerate(kinds) if kind == "write")
        self.assertEqual(kinds[kinds.index("read", last_write + 1) - 1], "sync")

    def test_fsync_failure_is_not_success(self) -> None:
        progress: list[tuple[str, int]] = []
        with mock.patch.object(ffu.os, "fsync", side_effect=OSError("fixture fsync failed")):
            with self.assertRaisesRegex(OSError, "fsync failed"):
                self.apply(progress=lambda phase, percent: progress.append((phase, percent)))
        self.assertNotIn(("Verifying", 100), progress)

    def test_source_is_opened_once_through_readback(self) -> None:
        opener = ffu._open_path
        source_opens = 0

        def count(path: Path, flags: int, kind: str, source_id: object = None) -> int:
            nonlocal source_opens
            if path == self.source:
                source_opens += 1
            return opener(path, flags, kind, source_id)

        with mock.patch.object(ffu, "_open_path", side_effect=count):
            self.apply()
        self.assertEqual(source_opens, 1)

    def test_source_mutation_after_preflight_prevents_writes(self) -> None:
        def progress(phase: str, percent: int) -> None:
            if (phase, percent) == ("Writing", 0):
                data = bytearray(self.fixture.blob)
                data[-1] ^= 1
                self.source.write_bytes(data)

        with mock.patch.object(ffu.os, "pwrite") as write, self.assertRaisesRegex(ffu.ApplyError, "source image changed"):
            self.apply(progress=progress)
        write.assert_not_called()
        self.assertEqual(self.target.read_bytes(), self.original)

    def test_payload_hash_is_rechecked_even_if_stat_change_is_hidden(self) -> None:
        def progress(phase: str, percent: int) -> None:
            if (phase, percent) == ("Writing", 0):
                data = bytearray(self.fixture.blob)
                data[self.fixture.payload_offset] ^= 1
                self.source.write_bytes(data)

        with mock.patch.object(ffu, "_check_source"), mock.patch.object(ffu.os, "pwrite") as write:
            with self.assertRaisesRegex(ffu.ApplyError, "source payload changed"):
                self.apply(progress=progress)
        write.assert_not_called()
        self.assertEqual(self.target.read_bytes(), self.original)

    def test_source_replacement_at_verify_is_not_a_new_readback_source(self) -> None:
        def progress(phase: str, percent: int) -> None:
            if (phase, percent) == ("Verifying", 0):
                other = self.directory / "replacement-source"
                other.write_bytes(self.fixture.blob)
                os.replace(other, self.source)

        with self.assertRaisesRegex(ffu.ApplyError, "source image changed"):
            self.apply(progress=progress)

    def test_readback_corruption_is_rejected(self) -> None:
        real_read = os.pread

        def corrupt_read(fd: int, size: int, offset: int) -> bytes:
            data = bytearray(real_read(fd, size, offset))
            if offset == 2 * CHUNK:
                data[0] ^= 1
            return bytes(data)

        with mock.patch.object(ffu.os, "pread", side_effect=corrupt_read):
            with self.assertRaisesRegex(ffu.ApplyError, "readback mismatch"):
                self.apply()

    def test_late_concurrent_target_mutation_is_rejected(self) -> None:
        def progress(phase: str, percent: int) -> None:
            if (phase, percent) == ("Verifying", 99):
                with self.target.open("r+b") as target:
                    target.write(b"X")

        with self.assertRaisesRegex(ffu.ApplyError, "target changed"):
            self.apply(progress=progress)

    def test_progress_has_distinct_complete_write_and_verify_phases(self) -> None:
        progress: list[tuple[str, int]] = []
        self.apply(progress=lambda phase, percent: progress.append((phase, percent)))
        self.assertEqual(progress[0], ("Writing", 0))
        self.assertEqual(progress[-1], ("Verifying", 100))
        self.assertLess(progress.index(("Writing", 100)), progress.index(("Verifying", 0)))
        for phase in ("Writing", "Verifying"):
            values = [percent for name, percent in progress if name == phase]
            self.assertEqual(values, sorted(set(values)))

    def test_interrupt_during_write_never_reports_verification_success(self) -> None:
        progress: list[tuple[str, int]] = []

        def interrupt(phase: str, percent: int) -> None:
            progress.append((phase, percent))
            if phase == "Writing" and percent:
                raise KeyboardInterrupt

        with self.assertRaises(KeyboardInterrupt):
            self.apply(progress=interrupt)
        self.assertNotIn(("Verifying", 100), progress)
        self.assertEqual(self.source.read_bytes(), self.fixture.blob)


class GptCleanupTests(FileFixture):
    def prepare_gpt(self) -> tuple[int, bytes, tuple[tuple[int, int], ...]]:
        regions = create_gpt_target(self.target)
        before = self.target.read_bytes()
        return len(before), before, regions

    def test_clears_both_gpt_copies_and_preserves_unwritten_data(self) -> None:
        size, before, regions = self.prepare_gpt()
        info = ffu.parse_ffu(io.BytesIO(self.fixture.blob))
        with self.target.open("rb") as target:
            cleanup = ffu._gpt_cleanup_plan(target.fileno(), info, size)
        self.assertEqual(tuple((region.offset, len(region.original)) for region in cleanup), regions)
        progress: list[tuple[str, int]] = []
        ffu.apply_to_file_fixture(
            self.source, self.target, size, allow_gpt_cleanup=True,
            progress=lambda phase, percent: progress.append((phase, percent)),
        )
        expected = bytearray(before)
        for offset, length in regions:
            expected[offset : offset + length] = b"\0" * length
        for extent in ffu._resolve_plan(info, size).writes:
            start = info.payload_offset + extent.payload_block * CHUNK
            length = extent.block_count * CHUNK
            expected[extent.offset : extent.offset + length] = self.fixture.blob[start : start + length]
        self.assertEqual(self.target.read_bytes(), expected)
        self.assertEqual(self.source.read_bytes(), self.fixture.blob)
        self.assertIn(("Preparing GPT metadata", 100), progress)
        self.assertEqual(progress[-1], ("Verifying", 100))
        self.assertEqual(self.target.read_bytes()[size - 33 * 512 : size - CHUNK], b"\0" * (33 * 512 - CHUNK))

    def test_default_application_still_refuses_gpt(self) -> None:
        size, before, _ = self.prepare_gpt()
        with mock.patch.object(ffu.os, "pwrite") as write, self.assertRaisesRegex(ffu.TargetError, "existing GPT"):
            ffu.apply_to_file_fixture(self.source, self.target, size)
        write.assert_not_called()
        self.assertEqual(self.target.read_bytes(), before)

    @unittest.skipUnless(shutil.which("sgdisk"), "sgdisk is unavailable")
    def test_native_sgdisk_layout_is_cleared_on_a_regular_file(self) -> None:
        size = 8 * 1024 * 1024
        with self.target.open("r+b") as target:
            target.truncate(size)
        subprocess.run(
            ["sgdisk", "--clear", "--new=1:2048:0", "--typecode=1:0700", str(self.target)],
            check=True, capture_output=True, text=True, timeout=15,
        )
        info = ffu.parse_ffu(io.BytesIO(self.fixture.blob))
        with self.target.open("rb") as target:
            cleanup = ffu._gpt_cleanup_plan(target.fileno(), info, size)
        self.assertEqual(len(cleanup), 5)
        ffu.apply_to_file_fixture(self.source, self.target, size, allow_gpt_cleanup=True)
        with self.target.open("rb") as target:
            self.assertNotEqual(target.read(1024)[512:520], b"EFI PART")
            target.seek(size - 512)
            self.assertNotEqual(target.read(8), b"EFI PART")

    def test_invalid_headers_tables_or_bounds_never_reach_cleanup(self) -> None:
        for damage in ("primary-crc", "backup-crc", "table-crc", "oversized-table", "table-in-data", "copy-disagreement", "missing-primary", "missing-backup"):
            with self.subTest(damage=damage):
                size, original, _ = self.prepare_gpt()
                data = bytearray(original)
                if damage == "primary-crc":
                    data[512 + 16] ^= 1
                elif damage == "backup-crc":
                    data[size - 512 + 16] ^= 1
                elif damage == "table-crc":
                    data[1024 + 16] ^= 1
                elif damage == "oversized-table":
                    struct.pack_into("<I", data, 512 + 80, 100000)
                    rewrite_gpt_header(data, 512)
                elif damage == "table-in-data":
                    struct.pack_into("<Q", data, 512 + 72, 2048)
                    rewrite_gpt_header(data, 512)
                elif damage == "copy-disagreement":
                    data[size - 512 + 56] ^= 1
                    rewrite_gpt_header(data, size - 512)
                elif damage == "missing-primary":
                    data[512:520] = b"\0" * 8
                else:
                    data[-512:-504] = b"\0" * 8
                self.target.write_bytes(data)
                with mock.patch.object(ffu.os, "pwrite") as write, self.assertRaises(ffu.TargetError):
                    ffu.apply_to_file_fixture(self.source, self.target, size, allow_gpt_cleanup=True)
                write.assert_not_called()
                self.assertEqual(self.target.read_bytes(), data)

    def test_corrupt_source_and_unconfirmed_target_do_not_erase_gpt(self) -> None:
        size, before, _ = self.prepare_gpt()
        corrupted = bytearray(self.fixture.blob)
        corrupted[-1] ^= 1
        self.source.write_bytes(corrupted)
        with mock.patch.object(ffu.os, "pwrite") as write, self.assertRaises(ffu.FfuError):
            ffu.apply_to_file_fixture(self.source, self.target, size, allow_gpt_cleanup=True)
        write.assert_not_called()
        self.source.write_bytes(self.fixture.blob)
        with mock.patch.object(ffu.os, "pwrite") as write, self.assertRaisesRegex(ffu.TargetError, "changed since confirmation"):
            ffu._apply(self.source, self.target, size, True, None, expected_target_id="1:2:3", allow_gpt_cleanup=True)
        write.assert_not_called()
        self.assertEqual(self.target.read_bytes(), before)

    def test_gpt_header_checksum_is_required(self) -> None:
        size, before, _ = self.prepare_gpt()
        damaged = bytearray(before)
        damaged[512 + 16] ^= 1
        self.target.write_bytes(damaged)
        with mock.patch.object(ffu.os, "pwrite", wraps=os.pwrite) as write, self.assertRaisesRegex(ffu.TargetError, "header checksum"):
            ffu.apply_to_file_fixture(self.source, self.target, size, allow_gpt_cleanup=True)
        write.assert_not_called()

    def test_production_cleanup_requires_both_prior_identities(self) -> None:
        for kwargs in ({}, {"expected_sha256": "1" * 64}, {"expected_target_id": "1:2:3"}):
            with self.subTest(kwargs=kwargs), mock.patch.object(ffu, "_open_path") as opened:
                with self.assertRaisesRegex(ffu.TargetError, "verified image digest and confirmed target"):
                    ffu.apply_ffu(self.source, self.target, TARGET_SIZE, allow_gpt_cleanup=True, **kwargs)
                opened.assert_not_called()

    def test_metadata_change_after_planning_prevents_all_writes(self) -> None:
        size, _, _ = self.prepare_gpt()
        clear = ffu._clear_gpt_metadata

        def changed(*args: object) -> None:
            with self.target.open("r+b") as target:
                target.seek(1024 + 16)
                target.write(b"\xff")
            clear(*args)

        with mock.patch.object(ffu, "_clear_gpt_metadata", side_effect=changed):
            with mock.patch.object(ffu.os, "pwrite") as write, self.assertRaisesRegex(ffu.TargetError, "changed during preflight"):
                ffu.apply_to_file_fixture(self.source, self.target, size, allow_gpt_cleanup=True)
            write.assert_not_called()

    def test_source_change_immediately_before_cleanup_prevents_all_writes(self) -> None:
        size, before, _ = self.prepare_gpt()

        def changed_source(phase: str, percent: int) -> None:
            if phase == "Preparing GPT metadata" and percent == 0:
                damaged = bytearray(self.fixture.blob)
                damaged[-1] ^= 1
                self.source.write_bytes(damaged)

        with mock.patch.object(ffu.os, "pwrite") as write, self.assertRaisesRegex(ffu.ApplyError, "source image changed"):
            ffu.apply_to_file_fixture(
                self.source, self.target, size, allow_gpt_cleanup=True, progress=changed_source,
            )
        write.assert_not_called()
        self.assertEqual(self.target.read_bytes(), before)

    def test_cleanup_readback_and_interruption_cannot_report_success(self) -> None:
        size, before, _ = self.prepare_gpt()
        with mock.patch.object(ffu, "_write_all"), self.assertRaisesRegex(ffu.ApplyError, "GPT cleanup readback failed"):
            ffu.apply_to_file_fixture(self.source, self.target, size, allow_gpt_cleanup=True)
        self.assertEqual(self.target.read_bytes(), before)
        progress: list[tuple[str, int]] = []

        def interrupted(phase: str, percent: int) -> None:
            progress.append((phase, percent))
            if phase == "Preparing GPT metadata" and percent > 0:
                raise KeyboardInterrupt()

        with self.assertRaises(KeyboardInterrupt):
            ffu.apply_to_file_fixture(self.source, self.target, size, allow_gpt_cleanup=True, progress=interrupted)
        self.assertNotIn(("Verifying", 100), progress)
        self.assertNotIn(("Writing", 0), progress)
        self.assertEqual(self.source.read_bytes(), self.fixture.blob)

    def test_cleared_tail_is_verified_after_the_ffu_payload(self) -> None:
        size, _, _ = self.prepare_gpt()
        reading = ffu._read_target
        verifying = False

        def progress(phase: str, _percent: int) -> None:
            nonlocal verifying
            if phase == "Verifying":
                verifying = True

        def corrupted(fd: int, length: int, offset: int, sector: int) -> bytes:
            data = reading(fd, length, offset, sector)
            if verifying and offset == size - 33 * 512:
                return b"\xff" + data[1:]
            return data

        with mock.patch.object(ffu, "_read_target", side_effect=corrupted), self.assertRaisesRegex(ffu.ApplyError, "old GPT metadata reappeared"):
            ffu.apply_to_file_fixture(self.source, self.target, size, allow_gpt_cleanup=True, progress=progress)


class FullWipeTests(FileFixture):
    def test_entire_drive_and_non_chunk_aligned_tail_are_zeroed_before_image_write(self) -> None:
        size = ffu.WIPE_CHUNK_BYTES + 512
        self.target.write_bytes(b"\xA5" * size)
        phases: list[tuple[str, int]] = []

        def progress(phase: str, percent: int) -> None:
            phases.append((phase, percent))
            if phase == "Verifying blank drive" and percent == 100:
                self.assertFalse(any(self.target.read_bytes()), "every target byte must be zero before image writes")

        ffu.apply_to_file_fixture(self.source, self.target, size, wipe_entire_drive=True, progress=progress)
        self.assertTrue(self.target.read_bytes() == self.expected_target(size).replace(b"\xA5", b"\0"),
                        "only the verified image extents may be nonzero after a full reset")
        self.assertEqual(self.target.stat().st_size, size)
        self.assertEqual(self.source.read_bytes(), self.fixture.blob)
        self.assertLess(phases.index(("Wiping entire drive", 100)), phases.index(("Verifying blank drive", 0)))
        self.assertLess(phases.index(("Verifying blank drive", 100)), phases.index(("Writing", 0)))
        self.assertEqual(phases[-1], ("Verifying", 100))

    def test_malformed_gpt_is_only_discarded_by_an_explicit_full_wipe(self) -> None:
        size = 2 * 1024 * 1024
        create_gpt_target(self.target, size)
        original = bytearray(self.target.read_bytes())
        original[512 + 16] ^= 1
        self.target.write_bytes(original)
        with self.assertRaises(ffu.TargetError):
            ffu.apply_to_file_fixture(self.source, self.target, size, allow_gpt_cleanup=True)
        self.assertEqual(self.target.read_bytes(), original)
        ffu.apply_to_file_fixture(self.source, self.target, size, wipe_entire_drive=True)
        self.assertEqual(self.target.read_bytes(), self.expected_target(size).replace(b"\xA5", b"\0"))

    def test_blank_readback_failure_prevents_image_application(self) -> None:
        original_read = ffu._read_target

        def corrupt(fd: int, size: int, offset: int, sector: int) -> bytes:
            data = original_read(fd, size, offset, sector)
            return b"\x01" + data[1:]

        with mock.patch.object(ffu, "_read_target", side_effect=corrupt):
            with mock.patch.object(ffu, "_write_all", wraps=ffu._write_all) as writes:
                with self.assertRaisesRegex(ffu.ApplyError, "full-wipe readback mismatch"):
                    self.apply(wipe_entire_drive=True)
                self.assertTrue(writes.called)
                self.assertTrue(all(not any(call.args[1]) for call in writes.call_args_list))
        self.assertEqual(self.target.read_bytes(), bytes(TARGET_SIZE))

    def test_source_and_target_failures_stop_before_the_first_wipe_write(self) -> None:
        for changed in ("source", "geometry", "identity"):
            with self.subTest(changed=changed):
                self.source.write_bytes(self.fixture.blob)
                self.target.write_bytes(self.original)
                context = contextlib.ExitStack()
                with context, mock.patch.object(ffu, "_write_all") as writes:
                    def progress(phase: str, percent: int) -> None:
                        if phase == "Wiping entire drive" and percent == 0:
                            if changed == "source":
                                self.source.write_bytes(self.fixture.blob + b"x")
                            elif changed == "geometry":
                                context.enter_context(mock.patch.object(ffu, "_device_geometry", return_value=(1, 512)))
                                context.enter_context(mock.patch.object(ffu, "_check_target", side_effect=ffu.TargetError("geometry changed")))
                            else:
                                context.enter_context(mock.patch.object(ffu, "_target_id", return_value="changed"))
                    with self.assertRaises(ffu.FfuError):
                        self.apply(wipe_entire_drive=True, progress=progress)
                    self.assertEqual(writes.call_count, 0)
                self.assertEqual(self.target.read_bytes(), self.original)

    def test_interrupted_wipe_never_reaches_image_writes_or_completion(self) -> None:
        size = ffu.WIPE_CHUNK_BYTES + 512
        self.target.write_bytes(b"\xA5" * size)
        phases: list[tuple[str, int]] = []

        def interrupt(phase: str, percent: int) -> None:
            phases.append((phase, percent))
            if phase == "Wiping entire drive" and percent > 0:
                raise ffu.InterruptedApply(signal.SIGINT)

        with self.assertRaises(ffu.InterruptedApply):
            ffu.apply_to_file_fixture(self.source, self.target, size, wipe_entire_drive=True, progress=interrupt)
        self.assertFalse(any(phase in ("Writing", "Verifying") for phase, _ in phases))
        self.assertNotIn(("Wiping entire drive", 100), phases)
        self.assertEqual(self.target.read_bytes()[-512:], b"\xA5" * 512)

    def test_invalid_source_never_reaches_wipe(self) -> None:
        self.source.write_bytes(self.fixture.blob[:-1])
        with mock.patch.object(ffu, "_wipe_target") as wiped:
            with self.assertRaises(ffu.FfuError):
                self.apply(wipe_entire_drive=True)
            wiped.assert_not_called()

    def test_production_wipe_requires_both_prior_identities(self) -> None:
        for identities in ({}, {"expected_sha256": "1" * 64}, {"expected_target_id": "-1:2:3"}):
            with self.subTest(identities=identities), mock.patch.object(ffu, "_open_path") as opened:
                with self.assertRaisesRegex(ffu.TargetError, "full-drive wipe requires"):
                    ffu.apply_ffu(self.source, self.target, TARGET_SIZE, wipe_entire_drive=True, **identities)
                opened.assert_not_called()
        with self.assertRaisesRegex(ffu.TargetError, "not both"):
            self.apply(wipe_entire_drive=True, allow_gpt_cleanup=True)
        with self.assertRaisesRegex(ffu.TargetError, "must be a boolean"):
            self.apply(wipe_entire_drive="true")

    def test_same_size_target_replacement_blocks_wipe(self) -> None:
        with mock.patch.object(ffu, "_wipe_target") as wiped:
            with self.assertRaisesRegex(ffu.TargetError, "changed since confirmation"):
                ffu._apply(self.source, self.target, TARGET_SIZE, True, None,
                           expected_target_id="-2088985291:687:16777240", wipe_entire_drive=True)
            wiped.assert_not_called()

    def test_cli_forwards_full_wipe_and_signed_identity(self) -> None:
        with mock.patch.object(ffu, "apply_ffu") as applied:
            self.assertEqual(ffu.main([
                "apply", str(self.source), "/dev/rdisk99", "--target-size", str(TARGET_SIZE),
                "--expected-target-id=-2088985291:687:16777240",
                "--expected-sha256", "1" * 64, "--wipe-entire-drive",
            ]), 0)
            self.assertTrue(applied.call_args.kwargs["wipe_entire_drive"])
            self.assertEqual(applied.call_args.kwargs["expected_target_id"], "-2088985291:687:16777240")
            self.assertFalse(applied.call_args.kwargs["allow_gpt_cleanup"])


class ApprovalAndCliTests(FileFixture):
    def test_target_identity_is_metadata_only(self) -> None:
        for host, mode, path in (
            ("darwin", ffu.stat.S_IFCHR, "/dev/rdisk99"),
            ("linux", ffu.stat.S_IFBLK, "/dev/mock-device"),
        ):
            value = types.SimpleNamespace(st_mode=mode, st_dev=1, st_ino=2, st_rdev=3)
            output = io.StringIO()
            with self.subTest(host=host), mock.patch.object(ffu.sys, "platform", host):
                with mock.patch.object(ffu.os, "stat", return_value=value), mock.patch.object(ffu.os, "open") as opened:
                    with contextlib.redirect_stdout(output):
                        self.assertEqual(ffu.main(["identify-target", path, "--json"]), 0)
                    opened.assert_not_called()
            self.assertEqual(json.loads(output.getvalue()), {"target_id": "1:2:3"})

    def test_target_identity_rejects_regular_files_and_symlinks(self) -> None:
        link = self.directory / "target-link"
        link.symlink_to(self.target)
        for path in (self.target, link):
            with self.subTest(path=path), self.assertRaisesRegex(ffu.TargetError, "not a file or symlink"):
                ffu.identify_target(path)

    def test_confirmation_target_identity_checked_before_writing(self) -> None:
        with mock.patch.object(ffu, "_write_all", wraps=ffu._write_all) as write:
            with self.assertRaisesRegex(ffu.TargetError, "changed since confirmation"):
                ffu._apply(self.source, self.target, TARGET_SIZE, True, None, expected_target_id="1:2:3")
            write.assert_not_called()
        self.assertEqual(self.target.read_bytes(), self.original)

    def test_matching_confirmation_identity_uses_real_fixture_writer(self) -> None:
        expected = ffu._target_id(os.stat(self.target))
        result = ffu._apply(self.source, self.target, TARGET_SIZE, True, None, expected_target_id=expected)
        self.assertEqual(result.sha256, hashlib.sha256(self.fixture.blob).hexdigest())
        self.assertNotEqual(self.target.read_bytes(), self.original)

    def test_signed_native_device_identity_is_preserved_and_checked(self) -> None:
        for device, raw_device in ((-2088985291, 16777240), (42, -16777240)):
            with self.subTest(device=device, raw_device=raw_device):
                native = types.SimpleNamespace(st_dev=device, st_ino=687, st_rdev=raw_device)
                expected = f"{device}:687:{raw_device}"
                self.assertEqual(ffu._target_id(native), expected)
                with mock.patch.object(ffu, "_target_id", return_value=expected):
                    result = ffu._apply(self.source, self.target, TARGET_SIZE, True, None, expected_target_id=expected)
                    self.assertEqual(result.sha256, hashlib.sha256(self.fixture.blob).hexdigest())
                with mock.patch.object(ffu, "_target_id", return_value=f"{device}:688:{raw_device}"):
                    with self.assertRaisesRegex(ffu.TargetError, "changed since confirmation"):
                        ffu._apply(self.source, self.target, TARGET_SIZE, True, None, expected_target_id=expected)

    def test_malformed_confirmation_identity_rejected_before_source_open(self) -> None:
        for value in ("", "a:b:c", "1:2", "1:2:3\n", "1:-2:3", "--1:2:3", "1:2:--3"):
            with self.subTest(value=value), mock.patch.object(ffu, "_open_path") as opened:
                with self.assertRaisesRegex(ffu.TargetError, "identity is malformed"):
                    ffu.apply_ffu(self.source, self.target, TARGET_SIZE, expected_target_id=value)
                opened.assert_not_called()

    def test_bundled_provenance_pin_and_official_identity(self) -> None:
        raw = ffu.PROVENANCE_PATH.read_bytes()
        self.assertEqual(hashlib.sha256(raw).hexdigest(), ffu.PROVENANCE_SHA256)
        approved, = ffu._approved_images()
        self.assertEqual(approved.sha256, "15e9451eb7b3e5645e89e25ffc846d4dfd4b4a8c9c63600e177cb16817da67f5")
        self.assertEqual(approved.build, "17763.107")
        self.assertEqual(approved.architecture, "arm32")
        self.assertEqual(approved.platform_ids, PLATFORMS)
        self.assertEqual(approved.recommended_boards, ("pi2-v1.1", "pi2-v1.2", "pi3-b"))

    def test_modified_provenance_is_not_an_approval_override(self) -> None:
        manifest = self.directory / "changed-provenance.json"
        manifest.write_bytes(ffu.PROVENANCE_PATH.read_bytes() + b" ")
        with mock.patch.object(ffu, "PROVENANCE_PATH", manifest), self.assertRaisesRegex(ffu.FfuError, "modified"):
            ffu._approved_images()

    def test_reviewed_profile_is_not_image_verification(self) -> None:
        for board in ("pi2-v1.1", "pi2-v1.2", "pi3-b"):
            output = io.StringIO()
            with self.subTest(board=board), mock.patch.object(ffu, "inspect_ffu") as inspected:
                with mock.patch.object(ffu, "apply_ffu") as applied, mock.patch.object(ffu, "identify_target") as target:
                    with contextlib.redirect_stdout(output):
                        self.assertEqual(ffu.main(["profile", "--board", board, "--json"]), 0)
                    inspected.assert_not_called()
                    applied.assert_not_called()
                    target.assert_not_called()
            profile = json.loads(output.getvalue())
            self.assertEqual(profile["source_verification"], "not-assessed")
            self.assertEqual(profile["trust"], "pinned-bundled-profile")
            self.assertEqual(profile["expected_build"], "17763.107")
            self.assertEqual(profile["minimum_disk_bytes"], 3774873600)
            self.assertEqual(profile["architecture"], "arm32")
            self.assertIn(board, profile["recommended_boards"])
            self.assertNotIn("sha256", profile)
            self.assertNotIn("source_file", profile)

    def test_profile_rejects_unsupported_board_and_modified_manifest(self) -> None:
        for board in ("pi3-b-plus", "pi4-b", "pi5", "", "unknown"):
            with self.subTest(board=board), self.assertRaisesRegex(ffu.FfuError, "exactly one"):
                ffu.image_profile(board)
        manifest = self.directory / "changed-profile.json"
        manifest.write_bytes(ffu.PROVENANCE_PATH.read_bytes() + b" ")
        with mock.patch.object(ffu, "PROVENANCE_PATH", manifest), self.assertRaisesRegex(ffu.FfuError, "modified"):
            ffu.image_profile("pi3-b")

    def test_wrong_arch_profile_digest_platform_board_build_and_geometry(self) -> None:
        info = ffu.parse_ffu(io.BytesIO(self.fixture.blob))
        approved = fixture_approval(info)
        for change in (
            {"architecture": "arm64"}, {"architecture": "x86"}, {"profile": "desktop"},
            {"format": "ffu-v2"}, {"sha256": "0" * 64}, {"build": "17763.253"},
            {"platform_ids": ("*",)}, {"platform_ids": ("Not.Raspberry.Pi", "*")},
            {"recommended_boards": ("pi4-b",)}, {"recommended_boards": ("Pi3B",)},
            {"file_bytes": info.file_bytes + 1},
            {"payload_bytes": 0}, {"minimum_disk_bytes": 512}, {"sector_size": 4096},
        ):
            with self.subTest(change=change), self.assertRaises(ffu.FfuError):
                ffu._check_approval(info, replace(approved, **change))

    def test_recomputed_hash_table_does_not_authorize_a_changed_image(self) -> None:
        info = ffu.parse_ffu(io.BytesIO(self.fixture.blob))
        data = bytearray(self.fixture.blob)
        data[-1] ^= 1
        self.source.write_bytes(self.fixture.rehash(data))
        with mock.patch.object(ffu, "_approved_images", return_value=(fixture_approval(info),)):
            with self.assertRaisesRegex(ffu.FfuError, "allowlisted"):
                ffu.inspect_ffu(self.source)

    def test_inspect_json_uses_identity_not_filename(self) -> None:
        info = ffu.parse_ffu(io.BytesIO(self.fixture.blob))
        renamed = self.directory / "ARM64-desktop-windows11.iso"
        self.source.rename(renamed)
        stdout, stderr = io.StringIO(), io.StringIO()
        with mock.patch.object(ffu, "_approved_images", return_value=(fixture_approval(info),)):
            with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
                status = ffu.main(["inspect", str(renamed), "--json"])
        self.assertEqual(status, 0)
        result = json.loads(stdout.getvalue())
        self.assertEqual(result["format"], "ffu-v1")
        self.assertEqual(result["architecture"], "arm32")
        self.assertEqual(result["profile"], "iot-core")
        self.assertEqual(result["recommended_boards"], ["pi2-v1.1", "pi2-v1.2", "pi3-b"])
        self.assertEqual(result["minimum_disk_bytes"], TARGET_SIZE)
        self.assertEqual(result["payload_bytes"], 6 * CHUNK)
        self.assertEqual(result["sha256"], info.sha256)
        self.assertFalse(result["hardware_tested"])
        self.assertEqual(stderr.getvalue(), "")

    def test_production_cli_refuses_regular_file_even_for_approved_image(self) -> None:
        info = ffu.parse_ffu(io.BytesIO(self.fixture.blob))
        for expected in ([], ["--expected-sha256", info.sha256]):
            with self.subTest(expected=expected):
                stdout, stderr = io.StringIO(), io.StringIO()
                with mock.patch.object(ffu, "_approved_images", return_value=(fixture_approval(info),)):
                    with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
                        status = ffu.main([
                            "apply", str(self.source), str(self.target),
                            "--target-size", str(TARGET_SIZE), *expected,
                        ])
                self.assertEqual(status, 3)
                self.assertIn("regular files are refused", stderr.getvalue())
                self.assertEqual(stdout.getvalue(), "")
                self.assertEqual(self.target.read_bytes(), self.original)

    def test_expected_digest_mismatch_never_opens_target(self) -> None:
        info = ffu.parse_ffu(io.BytesIO(self.fixture.blob))
        stderr = io.StringIO()
        with mock.patch.object(ffu, "_approved_images", return_value=(fixture_approval(info),)):
            with mock.patch.object(ffu, "_open_path", wraps=ffu._open_path) as opener:
                with contextlib.redirect_stderr(stderr):
                    status = ffu.main([
                        "apply", str(self.source), str(self.target), "--target-size", str(TARGET_SIZE),
                        "--expected-sha256", "0" * 64,
                    ])
        self.assertEqual(status, 2)
        self.assertIn("changed since inspection", stderr.getvalue())
        self.assertTrue(all(call.args[2] == "source" for call in opener.call_args_list))
        self.assertEqual(self.target.read_bytes(), self.original)

    def test_expected_digest_is_not_an_allowlist_bypass(self) -> None:
        digest = hashlib.sha256(self.fixture.blob).hexdigest()
        with mock.patch.object(ffu, "_open_path", wraps=ffu._open_path) as opener:
            with self.assertRaisesRegex(ffu.FfuError, "not allowlisted"):
                ffu.apply_ffu(self.source, self.target, TARGET_SIZE, expected_sha256=digest)
        self.assertTrue(all(call.args[2] == "source" for call in opener.call_args_list))
        self.assertEqual(self.target.read_bytes(), self.original)

    def test_malformed_expected_digest_fails_before_any_file_open(self) -> None:
        for expected in ("", "a" * 63, "a" * 65, "A" * 64, "g" * 64, "0" * 64 + "\n"):
            with self.subTest(expected=expected), mock.patch.object(ffu, "_open_path") as opener:
                with self.assertRaisesRegex(ffu.FfuError, "64 lowercase"):
                    ffu.apply_ffu(self.source, self.target, TARGET_SIZE, expected_sha256=expected)
            opener.assert_not_called()

    def test_changed_allowlisted_source_still_must_match_prior_inspection(self) -> None:
        original = ffu.parse_ffu(io.BytesIO(self.fixture.blob))
        modified = bytearray(self.fixture.blob)
        modified[-1] ^= 1
        modified_blob = self.fixture.rehash(modified)
        changed = ffu.parse_ffu(io.BytesIO(modified_blob))
        with mock.patch.object(ffu, "_approved_images", return_value=(
            fixture_approval(original), fixture_approval(changed),
        )):
            metadata = ffu.inspect_ffu(self.source)
            self.source.write_bytes(modified_blob)
            with mock.patch.object(ffu, "_open_path", wraps=ffu._open_path) as opener:
                with self.assertRaisesRegex(ffu.FfuError, "changed since inspection"):
                    ffu.apply_ffu(self.source, self.target, TARGET_SIZE, expected_sha256=metadata["sha256"])
        self.assertTrue(all(call.args[2] == "source" for call in opener.call_args_list))
        self.assertEqual(self.target.read_bytes(), self.original)

    def test_untrusted_cli_has_no_file_or_verification_bypass_flags(self) -> None:
        for flag in ("--allow-untrusted", "--allow-regular-file", "--skip-verify", "--manifest"):
            result = subprocess.run(
                [sys.executable, "-B", str(HELPER_PATH), "apply", str(self.source), str(self.target),
                 "--target-size", str(TARGET_SIZE), flag],
                stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, timeout=10, check=False,
            )
            self.assertEqual(result.returncode, 2, flag)
            self.assertIn("unrecognized arguments", result.stderr)
            self.assertEqual(result.stdout, "")
        self.assertEqual(self.target.read_bytes(), self.original)

    def test_unapproved_cli_returns_two_without_json_success(self) -> None:
        result = subprocess.run(
            [sys.executable, "-B", str(HELPER_PATH), "inspect", str(self.source), "--json"],
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, timeout=10, check=False,
        )
        self.assertEqual(result.returncode, 2)
        self.assertEqual(result.stdout, "")
        self.assertIn("not allowlisted", result.stderr)

    def test_invalid_source_geometry_has_source_exit_status(self) -> None:
        self.source.write_bytes(self.fixture.mutate(self.fixture.descriptor_offset + 12, "<I", 2**32 - 1))
        stderr = io.StringIO()
        with contextlib.redirect_stderr(stderr):
            self.assertEqual(ffu.main(["inspect", str(self.source), "--json"]), 2)
        self.assertIn("invalid FFU minimum geometry", stderr.getvalue())

    def test_exit_statuses_and_interrupt_messages(self) -> None:
        for error, code in (
            (ffu.FfuError("bad fixture"), 2), (ffu.TargetError("unsafe fixture"), 3),
            (OSError("fixture I/O error"), 4), (KeyboardInterrupt(), 130),
            (ffu.InterruptedApply(signal.SIGTERM), 143),
        ):
            stderr = io.StringIO()
            with self.subTest(code=code), mock.patch.object(ffu, "inspect_ffu", side_effect=error):
                with contextlib.redirect_stderr(stderr):
                    self.assertEqual(ffu.main(["inspect", str(self.source), "--json"]), code)
            self.assertIn("IoT Core FFU", stderr.getvalue())
        with self.assertRaises(ffu.InterruptedApply):
            ffu._interrupt(signal.SIGTERM, None)

    def test_stderr_progress_protocol(self) -> None:
        stderr = io.StringIO()
        with contextlib.redirect_stderr(stderr):
            ffu._progress("Writing", 25)
            ffu._progress("Verifying", 50)
        self.assertEqual(stderr.getvalue(), "Writing IoT Core 25%\nVerifying IoT Core 50%\n")

    def test_native_device_geometry_queries_without_devices(self) -> None:
        for host, calls in (
            ("darwin", [(0x40046418, 4, 512), (0x40086419, 8, 64)]),
            ("linux", [(0x80001272 | (struct.calcsize("P") << 16), 8, TARGET_SIZE), (0x1268, 4, 512)]),
        ):
            remaining = list(calls)

            def ioctl(fd: int, request: int, buffer: bytearray, mutate: bool) -> int:
                expected, size, value = remaining.pop(0)
                self.assertEqual((fd, request, len(buffer), mutate), (123, expected, size, True))
                buffer[:] = value.to_bytes(size, sys.byteorder)
                return 0

            with self.subTest(host=host), mock.patch.object(ffu.sys, "platform", host):
                with mock.patch.object(ffu.fcntl, "ioctl", side_effect=ioctl):
                    self.assertEqual(ffu._device_geometry(123), (TARGET_SIZE, 512))
            self.assertEqual(remaining, [])

    def test_native_cache_flush_required_before_readback(self) -> None:
        for host, fixture, readback, request in (
            ("darwin", False, False, 0x20006416),
            ("darwin", False, True, 0x20006416),
            ("linux", False, False, None),
            ("linux", False, True, 0x1261),
            ("darwin", True, True, None),
            ("linux", True, True, None),
        ):
            calls: list[str] = []
            with self.subTest(host=host, fixture=fixture, readback=readback):
                with mock.patch.object(ffu.sys, "platform", host):
                    with mock.patch.object(ffu.os, "fsync", side_effect=lambda fd: calls.append("fsync")) as sync:
                        with mock.patch.object(ffu.fcntl, "ioctl", side_effect=lambda *args: calls.append("ioctl")) as ioctl:
                            ffu._sync_target(123, fixture, before_readback=readback)
                sync.assert_called_once_with(123)
                if request is None:
                    ioctl.assert_not_called()
                    self.assertEqual(calls, ["fsync"])
                else:
                    ioctl.assert_called_once_with(123, request)
                    self.assertEqual(calls, ["fsync", "ioctl"])

    def test_cache_flush_failure_is_never_ignored(self) -> None:
        with mock.patch.object(ffu.sys, "platform", "darwin"), mock.patch.object(ffu.os, "fsync"):
            with mock.patch.object(ffu.fcntl, "ioctl", side_effect=OSError("cache flush failed")):
                with self.assertRaisesRegex(OSError, "cache flush failed"):
                    ffu._sync_target(123, False, before_readback=True)

    def test_cache_capability_failure_precedes_first_write(self) -> None:
        with ffu._source_handle(self.source) as (source, stamp):
            info = ffu.parse_ffu(source)
            plan = ffu._resolve_plan(info, TARGET_SIZE)
            with mock.patch.object(ffu, "_check_target"), mock.patch.object(ffu.os, "pwrite") as write:
                with mock.patch.object(ffu, "_sync_target", side_effect=OSError("cache capability unavailable")):
                    with self.assertRaisesRegex(OSError, "capability unavailable"):
                        ffu._apply_plan(source, stamp, info, plan, 123, TARGET_SIZE, False, None)
            write.assert_not_called()

    def test_mismatched_device_geometry_and_4kn_refused(self) -> None:
        for geometry in ((TARGET_SIZE + 512, 512), (TARGET_SIZE, 4096)):
            with self.subTest(geometry=geometry), mock.patch.object(ffu, "_device_geometry", return_value=geometry):
                with self.assertRaises(ffu.TargetError):
                    ffu._check_target(123, TARGET_SIZE, 512, False)

    def test_fixture_api_cannot_open_a_device(self) -> None:
        original_stat = ffu.os.stat

        def device_stat(path: object, *args: object, **kwargs: object) -> object:
            value = original_stat(path, *args, **kwargs)
            if path == self.target.name:
                fields = list(value)
                fields[0] = 0o060600
                return os.stat_result(fields)
            return value

        with mock.patch.object(ffu.os, "stat", side_effect=device_stat), self.assertRaisesRegex(ffu.TargetError, "regular file"):
            self.apply()
        self.assertEqual(self.target.read_bytes(), self.original)

    def test_production_rejects_wsl(self) -> None:
        with mock.patch.object(ffu.sys, "platform", "linux"):
            with mock.patch.object(ffu.os, "uname", return_value=types.SimpleNamespace(release="6.6.87.2-microsoft-standard-WSL2")):
                with self.assertRaisesRegex(ffu.TargetError, "WSL"):
                    ffu.apply_ffu(self.source, self.target, TARGET_SIZE)


@unittest.skipUnless(
    os.environ.get("WOR_TEST_NATIVE_LOOP") == "1",
    "native Linux loop-device coverage is opt-in and runs in the isolated CI job",
)
class NativeLinuxDeviceTests(FileFixture):
    def test_owned_loop_device_write_and_readback(self) -> None:
        self.assertTrue(sys.platform.startswith("linux"), "native loop test requires Linux")
        self.assertEqual(os.geteuid(), 0, "native loop test requires isolated-runner root privileges")
        self.assertIsNotNone(shutil.which("losetup"), "losetup is required")
        created = subprocess.run(
            ["losetup", "--find", "--show", "--sector-size", "512", str(self.target)],
            check=True, capture_output=True, text=True, timeout=15,
        ).stdout.strip()
        self.assertRegex(created, r"^/dev/loop[0-9]+$")
        try:
            details = subprocess.run(
                ["losetup", "--json", "--list", "--output", "NAME,BACK-FILE", created],
                check=True, capture_output=True, text=True, timeout=15,
            )
            devices = json.loads(details.stdout)["loopdevices"]
            self.assertEqual(len(devices), 1)
            self.assertEqual(Path(devices[0]["back-file"]).resolve(), self.target.resolve())
            self.assertEqual(devices[0]["name"], created)
            info = ffu.parse_ffu(io.BytesIO(self.fixture.blob))
            with mock.patch.object(ffu, "_approved_images", return_value=(fixture_approval(info),)):
                ffu.apply_ffu(
                    self.source, Path(created), TARGET_SIZE,
                    expected_sha256=info.sha256,
                    expected_target_id=ffu.identify_target(Path(created))["target_id"],
                )
            self.assertNotEqual(self.target.read_bytes(), self.original)
        finally:
            if re.fullmatch(r"/dev/loop[0-9]+", created):
                subprocess.run(["losetup", "--detach", created], check=True, capture_output=True, timeout=15)


class MutationSensitivityTests(unittest.TestCase):
    def assert_mutation_is_caught(self, mutation: str, case: type[unittest.TestCase], method: str) -> None:
        tree = ast.parse(HELPER_PATH.read_text(), filename=str(HELPER_PATH))
        changes = 0

        class RemoveGuard(ast.NodeTransformer):
            def visit_Compare(self, node: ast.Compare) -> ast.AST:
                nonlocal changes
                if mutation == "hash" and len(node.comparators) == 1:
                    right = node.comparators[0]
                    if isinstance(right, ast.Subscript) and isinstance(right.value, ast.Name) and right.value.id == "hashes":
                        changes += 1
                        return ast.copy_location(ast.Constant(False), node)
                if mutation == "readback" and isinstance(node.left, ast.Name) and node.left.id == "actual":
                    if len(node.comparators) == 1 and isinstance(node.comparators[0], ast.Name) and node.comparators[0].id == "expected":
                        changes += 1
                        return ast.copy_location(ast.Constant(False), node)
                if mutation == "target_identity" and isinstance(node.left, ast.Call):
                    if isinstance(node.left.func, ast.Name) and node.left.func.id == "_target_id":
                        if len(node.comparators) == 1 and isinstance(node.comparators[0], ast.Name) and node.comparators[0].id == "expected_target_id":
                            changes += 1
                            return ast.copy_location(ast.Constant(False), node)
                if mutation == "gpt_crc" and isinstance(node.left, ast.Call):
                    if isinstance(node.left.func, ast.Attribute) and node.left.func.attr == "crc32":
                        if node.left.args and isinstance(node.left.args[0], ast.Name) and node.left.args[0].id == "checked":
                            changes += 1
                            return ast.copy_location(ast.Constant(False), node)
                if mutation == "wipe_readback" and isinstance(node.left, ast.Call):
                    if isinstance(node.left.func, ast.Name) and node.left.func.id == "_read_target":
                        if len(node.comparators) == 1 and isinstance(node.comparators[0], ast.Subscript):
                            value = node.comparators[0].value
                            if isinstance(value, ast.Name) and value.id == "zeros":
                                changes += 1
                                return ast.copy_location(ast.Constant(False), node)
                return self.generic_visit(node)

            def visit_Expr(self, node: ast.Expr) -> ast.AST:
                nonlocal changes
                guard = {"approval": "_approve", "gpt": "_reject_existing_gpt", "gpt_readback": "_verify_gpt_cleanup",
                         "full_wipe": "_wipe_target"}.get(mutation)
                if guard is not None and isinstance(node.value, ast.Call):
                    if isinstance(node.value.func, ast.Name) and node.value.func.id == guard:
                        changes += 1
                        return ast.copy_location(ast.Pass(), node)
                return self.generic_visit(node)

        tree = RemoveGuard().visit(tree)
        self.assertEqual(changes, 1, "mutation must disable exactly one real guard")
        name = "iot_ffu_mutant_" + mutation
        mutant = load_helper(name, compile(ast.fix_missing_locations(tree), str(HELPER_PATH), "exec"))
        self.addCleanup(sys.modules.pop, name, None)
        result = unittest.TestResult()
        with mock.patch.object(sys.modules[__name__], "ffu", mutant):
            case(method).run(result)
        self.assertEqual(result.testsRun, 1)
        self.assertEqual(len(result.errors), 0, result.errors)
        self.assertEqual(len(result.failures), 1, "negative test survived removal of its production safety guard")

    def test_payload_hash_guard_mutant_is_killed(self) -> None:
        self.assert_mutation_is_caught("hash", ParserTests, "test_payload_corruption_is_rejected")

    def test_readback_guard_mutant_is_killed(self) -> None:
        self.assert_mutation_is_caught("readback", FileTests, "test_readback_corruption_is_rejected")

    def test_allowlist_guard_mutant_is_killed(self) -> None:
        self.assert_mutation_is_caught("approval", FileTests, "test_unapproved_preflight_never_opens_target")

    def test_gpt_preflight_guard_mutant_is_killed(self) -> None:
        self.assert_mutation_is_caught("gpt", FileTests, "test_orphaned_backup_gpt_refused_before_any_write")

    def test_target_identity_guard_mutant_is_killed(self) -> None:
        self.assert_mutation_is_caught("target_identity", ApprovalAndCliTests, "test_confirmation_target_identity_checked_before_writing")

    def test_gpt_cleanup_crc_guard_mutant_is_killed(self) -> None:
        self.assert_mutation_is_caught("gpt_crc", GptCleanupTests, "test_gpt_header_checksum_is_required")

    def test_gpt_cleanup_readback_guard_mutant_is_killed(self) -> None:
        self.assert_mutation_is_caught("gpt_readback", GptCleanupTests, "test_cleared_tail_is_verified_after_the_ffu_payload")

    def test_full_wipe_mutant_is_killed(self) -> None:
        self.assert_mutation_is_caught("full_wipe", FullWipeTests, "test_entire_drive_and_non_chunk_aligned_tail_are_zeroed_before_image_write")

    def test_full_wipe_readback_mutant_is_killed(self) -> None:
        self.assert_mutation_is_caught("wipe_readback", FullWipeTests, "test_blank_readback_failure_prevents_image_application")


if __name__ == "__main__":
    unittest.main(verbosity=2)
