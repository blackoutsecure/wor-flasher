#!/usr/bin/env python3
"""Bounded, stdlib-only reader/writer for the allowlisted Raspberry Pi IoT FFU.

CLI:
    inspect FILE --json
    identify-target TARGET --json
    apply FILE TARGET --target-size BYTES [--expected-sha256 SHA256]
        [--expected-target-id ID] [--allow-gpt-cleanup | --wipe-entire-drive]
        [--hdmi-mode official|720p60|1080p60|custom] [--hdmi-config TEXT]

Only local, full, uncompressed, single-store Mobile/OneCore V1 FFUs are parsed.
Production inspection/application also requires the exact official-image SHA256
in the bundled, digest-pinned provenance manifest. Catalog signatures are not
interpreted: the independently pinned whole-file digest is the trust anchor.
Neither the filename, a wildcard platform ID, nor an FFU's own hashes authorize
an image. No CLI/environment/config override can approve another image.
Apply independently hashes the entire FFU before opening the target. The optional
expected digest must be 64 lowercase hex characters matching a previous inspect;
it adds a source-identity check and never replaces the production allowlist.

The caller MUST confirm the selected whole disk, exclude the host/root/source
disks, and unmount its volumes. This helper additionally rejects symlink paths,
non-device targets, changed source files, and mismatched device size/sector size.
It does not resize filesystems or install drivers/UEFI. An interrupted/failed
application, including partition-metadata cleanup, can leave an incomplete target.
The sole approved image has a fixed MBR/EBR layout and only start-relative writes.
Larger 512-byte-sector targets are supported up to 2 TiB, with extra space unused.
GPT targets require --allow-gpt-cleanup, the prior verified image digest and the
confirmed target identity. Both GPT headers/tables are validated and bounded before
any cleanup; malformed, orphaned or inconsistent GPT is refused. The selected
disk's old MBR/GPT metadata is cleared, flushed and read back before FFU writes.
Cleanup regions outside final FFU extents are checked again during readback.
This is destructive layout replacement, not GPT repair, resizing or a secure erase.
An explicit --wipe-entire-drive instead zeroes and reads back every addressable
sector before image application, without trusting the old partition tables. It
requires both prior identities and never runs before full source verification.
It is not a hardware repair or a secure erase of flash-controller spare blocks.
Linux device readback requires permission to invalidate the block cache (normally
root); macOS uses the raw device and explicitly synchronizes its hardware cache.

parse_ffu(BinaryIO) is a read-only, untrusted-format parser. The explicitly
test-only apply_to_file_fixture() uses that same parser and writer, but accepts
only existing regular files and never authorizes a device. Tests need no Windows
payloads. Readback checks all final extents against the SAME opened source;
declared initial/flash-only table writes may be superseded by final table writes.
Opt-in HDMI settings alter only the existing boot config file and its directory
size. The source FFU remains unchanged and allowlisted; configured output blocks
are derived before the target is opened and included in mandatory readback.

Exit statuses: 0 success, 2 invalid/unapproved source or arguments, 3 unsafe or
incompatible target, 4 I/O/source-mutation/write/readback failure, 130 interrupted
by SIGINT, 143 interrupted by SIGTERM. Progress is written only to stderr.
"""

from __future__ import annotations

import argparse
import fcntl
import hashlib
import itertools
import json
import os
import re
import signal
import stat
import struct
import sys
import zlib
from contextlib import contextmanager
from dataclasses import dataclass
from pathlib import Path
from typing import Any, BinaryIO, Callable, Iterator, Optional, Sequence


SECURITY_HEADER = struct.Struct("<I12sIIII")
IMAGE_HEADER = struct.Struct("<I12sII")
STORE_HEADER = struct.Struct("<IHHHH192s11I")
MAX_IMAGE_BYTES = 16 * 1024**3
MAX_TARGET_BYTES = 2 * 1024**4
MAX_CATALOG_BYTES = 1024 * 1024
MAX_MANIFEST_BYTES = 64 * 1024
MAX_HASH_BYTES = 16 * 1024 * 1024
MAX_DESCRIPTORS = 131072
MAX_LOCATIONS = 16
MAX_EXTENTS = 262144
MAX_GPT_TABLE_BYTES = 1024 * 1024
WIPE_CHUNK_BYTES = 4 * 1024 * 1024
MAX_HDMI_CONFIG_BYTES = 512
MAX_BOOT_CONFIG_BYTES = 4096
SECTOR_SIZE = 512
PROVENANCE_SHA256 = "74e9324c77e3e560a34742de1cd13e6c234789bf5089a75ba334d80c098ca41a"
PROVENANCE_PATH = Path(__file__).resolve().parents[1] / "config/iot-core-images.json"
Progress = Callable[[str, int], None]
SourceStamp = tuple[int, int, int, int, int, int, int]
HDMI_PRESETS = {
    "official": ("Official image default", ""),
    "720p60": ("1280 x 720 / 60 Hz (compatibility)", "hdmi_force_hotplug=1\nhdmi_group=1\nhdmi_mode=4\n"),
    "1080p60": ("1920 x 1080 / 60 Hz", "hdmi_force_hotplug=1\nhdmi_group=1\nhdmi_mode=16\n"),
    "custom": ("Custom video settings", ""),
}


class FfuError(ValueError):
    exit_status = 2


class TargetError(FfuError):
    exit_status = 3


class ApplyError(FfuError):
    exit_status = 4


class InterruptedApply(Exception):
    def __init__(self, signum: int) -> None:
        super().__init__(f"interrupted by signal {signum}")
        self.signum = signum


@dataclass(frozen=True)
class DiskLocation:
    method: int
    block_index: int


@dataclass(frozen=True)
class WriteDescriptor:
    payload_block: int
    block_count: int
    locations: tuple[DiskLocation, ...]
    phase: str


@dataclass(frozen=True)
class ParsedFfu:
    sha256: str
    file_bytes: int
    chunk_size: int
    sector_size: int
    minimum_disk_bytes: int
    image_offset: int
    payload_offset: int
    payload_bytes: int
    os_version: str
    platform_ids: tuple[str, ...]
    chunk_hashes: bytes
    descriptors: tuple[WriteDescriptor, ...]


@dataclass(frozen=True)
class ApprovedImage:
    sha256: str
    format: str
    profile: str
    architecture: str
    build: str
    os_version: str
    platform_ids: tuple[str, ...]
    recommended_boards: tuple[str, ...]
    file_bytes: int
    payload_bytes: int
    minimum_disk_bytes: int
    sector_size: int
    chunk_size: int
    source_url: str


@dataclass(frozen=True)
class Extent:
    offset: int
    payload_block: int
    block_count: int
    phase: str


@dataclass(frozen=True)
class WritePlan:
    writes: tuple[Extent, ...]
    final_extents: tuple[Extent, ...]


@dataclass(frozen=True)
class GptHeader:
    current_lba: int
    other_lba: int
    first_usable: int
    last_usable: int
    disk_guid: bytes
    entries_lba: int
    entry_count: int
    entry_size: int
    table_crc: int


@dataclass(frozen=True)
class MetadataRegion:
    offset: int
    original: bytes


@dataclass(frozen=True)
class ConfiguredBlock:
    offset: int
    payload_block: int
    data: bytes


def hdmi_options(mode: str = "official", custom: str = "") -> dict[str, object]:
    """Validate display-only preferences without accessing an image or target."""
    if type(mode) is not str or mode not in HDMI_PRESETS:
        raise FfuError("unknown HDMI mode; choose official, 720p60, 1080p60 or custom")
    if (
        type(custom) is not str or len(custom) > MAX_HDMI_CONFIG_BYTES or not custom.isascii()
        or any(ord(char) < 32 and char not in "\t\r\n" for char in custom)
    ):
        raise FfuError("custom HDMI settings must be at most 512 ASCII characters")
    label, settings = HDMI_PRESETS[mode]
    if mode == "custom":
        values: dict[str, int] = {}
        for line in custom.splitlines():
            line = line.split("#", 1)[0].strip()
            if not line:
                continue
            match = re.fullmatch(r"(hdmi_force_hotplug|hdmi_group|hdmi_mode|hdmi_drive)\s*=\s*([0-9]+)", line)
            if match is None:
                raise FfuError("custom HDMI settings accept only hdmi_force_hotplug, hdmi_group, hdmi_mode and hdmi_drive")
            key, value = match.groups()
            if key in values:
                raise FfuError(f"duplicate HDMI setting: {key}")
            values[key] = int(value)
        if values.get("hdmi_group") not in (1, 2) or "hdmi_mode" not in values:
            raise FfuError("custom HDMI settings require hdmi_group=1 or 2 and hdmi_mode")
        maximum = 64 if values["hdmi_group"] == 1 else 86
        if not 1 <= values["hdmi_mode"] <= maximum:
            raise FfuError("HDMI mode is outside the legacy Pi 2/3 CEA/DMT range; custom modelines are not supported")
        if "hdmi_force_hotplug" in values and values["hdmi_force_hotplug"] not in (0, 1):
            raise FfuError("hdmi_force_hotplug must be 0 or 1")
        if "hdmi_drive" in values and values["hdmi_drive"] not in (1, 2):
            raise FfuError("hdmi_drive must be 1 (DVI) or 2 (HDMI)")
        settings = "".join(f"{key}={value}\n" for key, value in values.items())
        label = f"Custom HDMI (group {values['hdmi_group']}, mode {values['hdmi_mode']})"
    return {
        "mode": mode, "label": label, "settings": settings, "recommended": mode == "720p60",
        "custom": custom or HDMI_PRESETS["720p60"][1],
        "choices": [
            {
                "value": value, "label": preset[0], "settings": preset[1],
                "recommended": value == "720p60",
                "menu_label": preset[0] + (" (Recommended)" if value == "720p60" else ""),
            }
            for value, preset in HDMI_PRESETS.items()
        ],
    }


def _align(value: int, alignment: int) -> int:
    return ((value + alignment - 1) // alignment) * alignment


def _read_exact(source: BinaryIO, size: int) -> bytes:
    data = bytearray()
    while len(data) < size:
        part = source.read(size - len(data))
        if not part:
            raise FfuError("truncated FFU")
        data.extend(part)
    return bytes(data)


def _zero_padding(source: BinaryIO, end: int) -> None:
    length = end - source.tell()
    if length < 0 or length > 1024 * 1024:
        raise FfuError("invalid FFU alignment")
    if any(_read_exact(source, length)):
        raise FfuError("nonzero FFU padding is unsupported")


def _manifest(data: bytes) -> list[tuple[str, dict[str, str]]]:
    try:
        text = data.decode("ascii")
    except UnicodeDecodeError as exc:
        raise FfuError("manifest must be ASCII") from exc
    if any(ord(c) < 32 and c not in "\r\n\t" or ord(c) == 127 for c in text):
        raise FfuError("invalid manifest control character")
    sections: list[tuple[str, dict[str, str]]] = []
    for raw_line in text.splitlines():
        line = raw_line.strip()
        if not line:
            continue
        if line in ("[FullFlash]", "[Store]", "[Partition]"):
            if len(sections) >= 130:
                raise FfuError("too many manifest sections")
            sections.append((line[1:-1], {}))
            continue
        if not sections or "=" not in line:
            raise FfuError("unsupported manifest section or syntax")
        key, value = (part.strip() for part in line.split("=", 1))
        if not re.fullmatch(r"[A-Za-z][A-Za-z0-9_]*", key):
            raise FfuError("invalid manifest key")
        key = key.lower()
        if key in sections[-1][1] or not value:
            raise FfuError("duplicate or empty manifest value")
        sections[-1][1][key] = value
    names = [name for name, _ in sections]
    if names[:2] != ["FullFlash", "Store"] or not names[2:] or any(
        name != "Partition" for name in names[2:]
    ):
        raise FfuError("only a single-store FullFlash manifest is supported")
    return sections


def _positive_decimal(value: str, maximum: int, label: str) -> int:
    if not re.fullmatch(r"[0-9]{1,20}", value):
        raise FfuError(f"invalid {label}")
    number = int(value)
    if not 0 < number <= maximum:
        raise FfuError(f"{label} exceeds supported bounds")
    return number


def _platform_ids(raw: bytes) -> tuple[str, ...]:
    if not raw.endswith(b"\0"):
        raise FfuError("unterminated platform IDs")
    pieces = raw.split(b"\0")
    if len(pieces) < 2:
        raise FfuError("unterminated platform IDs")
    end = pieces.index(b"")
    if not end or any(pieces[end:]):
        raise FfuError("invalid platform ID padding")
    ids = pieces[:end]
    if len(ids) > 16 or len(set(ids)) != len(ids):
        raise FfuError("invalid platform ID count")
    if any(any(c < 33 or c > 126 for c in item) for item in ids):
        raise FfuError("invalid platform ID characters")
    return tuple(item.decode("ascii") for item in ids)


def _catalog_envelope(catalog: bytes) -> None:
    #The catalog is opaque CMS; its bytes are authenticated by the pinned FFU SHA256.
    if len(catalog) < 2 or catalog[0] != 0x30:
        raise FfuError("invalid catalog DER envelope")
    size = catalog[1]
    start = 2
    if size & 0x80:
        width = size & 0x7F
        if not 1 <= width <= 4 or len(catalog) < 2 + width:
            raise FfuError("unsupported catalog DER length")
        size = int.from_bytes(catalog[2 : 2 + width], "big")
        if size < 128 or catalog[2] == 0:
            raise FfuError("non-canonical catalog DER length")
        start += width
    if not size or start + size != len(catalog):
        raise FfuError("catalog size does not match DER envelope")


def parse_ffu(source: BinaryIO) -> ParsedFfu:
    """Validate an untrusted FFU completely, without opening or authorizing a target."""
    file_bytes = source.seek(0, os.SEEK_END)
    if not SECURITY_HEADER.size <= file_bytes <= MAX_IMAGE_BYTES:
        raise FfuError("FFU size exceeds supported bounds")
    source.seek(0)
    size, magic, kb, algorithm, catalog_size, hash_size = SECURITY_HEADER.unpack(
        _read_exact(source, SECURITY_HEADER.size)
    )
    if size != SECURITY_HEADER.size or magic != b"SignedImage ":
        raise FfuError("invalid security header (expected FFU SignedImage)")
    if not 1 <= kb <= 1024 or kb & (kb - 1):
        raise FfuError("unsupported hash chunk size")
    chunk = kb * 1024
    if algorithm != 0x800C:
        raise FfuError("only SHA256 FFU chunk hashes are supported")
    if not 2 <= catalog_size <= MAX_CATALOG_BYTES:
        raise FfuError("invalid catalog size")
    if not 32 <= hash_size <= MAX_HASH_BYTES or hash_size % 32:
        raise FfuError("invalid hash table size")
    image_offset = _align(size + catalog_size + hash_size, chunk)
    if image_offset >= file_bytes or (file_bytes - image_offset) % chunk:
        raise FfuError("truncated or misaligned FFU image")
    if (file_bytes - image_offset) // chunk != hash_size // 32:
        raise FfuError("hash table does not cover the complete FFU image")
    _catalog_envelope(_read_exact(source, catalog_size))
    hashes = _read_exact(source, hash_size)
    _zero_padding(source, image_offset)

    image_size, image_magic, manifest_size, image_kb = IMAGE_HEADER.unpack(
        _read_exact(source, IMAGE_HEADER.size)
    )
    if image_size != IMAGE_HEADER.size or image_magic != b"ImageFlash  ":
        raise FfuError("unsupported image header or FFU variant")
    if image_kb != kb or not 1 <= manifest_size <= MAX_MANIFEST_BYTES:
        raise FfuError("invalid image chunk size or manifest length")
    sections = _manifest(_read_exact(source, manifest_size))
    full, store_manifest = sections[0][1], sections[1][1]
    if full.get("version") != "2.0" or full.get("uefi") != "True":
        raise FfuError("unsupported FullFlash manifest version")
    version = full.get("osversion", "")
    if not re.fullmatch(r"[0-9]{1,5}(?:\.[0-9]{1,5}){3}", version):
        raise FfuError("invalid manifest OSVersion")
    if (
        store_manifest.get("storetype") != "Default"
        or store_manifest.get("ismainosstore") != "True"
        or store_manifest.get("onlyallocatedefinedgptentries") != "False"
        or store_manifest.get("sectorsize") != str(SECTOR_SIZE)
    ):
        raise FfuError("unsupported store type, operation, or sector size")
    sectors = _positive_decimal(
        store_manifest.get("minsectorcount", ""),
        MAX_TARGET_BYTES // SECTOR_SIZE,
        "minimum sector count",
    )
    partition_names = [part.get("name", "") for _, part in sections[2:]]
    if not all(partition_names) or len(set(partition_names)) != len(partition_names):
        raise FfuError("missing or duplicate partition name")
    _zero_padding(source, _align(source.tell(), chunk))
    header = STORE_HEADER.unpack(_read_exact(source, STORE_HEADER.size))
    update, major, minor, full_major, full_minor, platform_raw = header[:6]
    if update != 0:
        raise FfuError("delta/partial FFUs are unsupported")
    if (major, minor, full_major, full_minor) != (1, 0, 2, 0):
        raise FfuError("compressed, multistore, or other store versions are unsupported")
    platforms = _platform_ids(platform_raw)
    expected_platforms = {f"deviceplatformid{i}": value for i, value in enumerate(platforms)}
    manifest_platforms = {key: value for key, value in full.items() if key.startswith("deviceplatformid")}
    if manifest_platforms != expected_platforms:
        raise FfuError("manifest and store platform IDs disagree")
    (
        block_size, descriptor_count, descriptor_bytes, validation_count,
        validation_bytes, initial_index, initial_count, flash_index,
        flash_count, final_index, final_count,
    ) = header[6:]
    if block_size != chunk:
        raise FfuError("store block size must equal the hash chunk size")
    if validation_count or validation_bytes:
        raise FfuError("conditional validation/delta operations are unsupported")
    if not 1 <= descriptor_count <= MAX_DESCRIPTORS:
        raise FfuError("write descriptor count exceeds supported bounds")
    if not descriptor_count * 16 <= descriptor_bytes <= descriptor_count * (8 + 8 * MAX_LOCATIONS):
        raise FfuError("write descriptor length exceeds supported bounds")
    descriptor_end = source.tell() + descriptor_bytes
    payload_offset = _align(descriptor_end, chunk)
    if payload_offset >= file_bytes:
        raise FfuError("missing or truncated FFU payload")
    payload_blocks = (file_bytes - payload_offset) // chunk
    ranges = (
        (initial_index, initial_count, "initial"),
        (flash_index, flash_count, "flash"),
        (final_index, final_count, "final"),
    )
    previous_end = 0
    for index, count, _ in ranges:
        if not count:
            if index:
                raise FfuError("empty table range has a nonzero index")
            continue
        if index < previous_end or index + count > payload_blocks:
            raise FfuError("invalid partition-table phase bounds")
        previous_end = index + count
    if initial_count and initial_index != 0:
        raise FfuError("initial table must begin the payload")
    if final_count and final_index + final_count != payload_blocks:
        raise FfuError("final table must end the payload")
    if (initial_count or flash_count) and not final_count:
        raise FfuError("temporary tables require a final table")
    boundaries = {value for index, count, _ in ranges if count for value in (index, index + count)}
    descriptors: list[WriteDescriptor] = []
    block = 0
    location_total = 0
    for _ in range(descriptor_count):
        if source.tell() + 8 > descriptor_end:
            raise FfuError("truncated write descriptor")
        location_count, block_count = struct.unpack("<II", _read_exact(source, 8))
        if not 1 <= location_count <= MAX_LOCATIONS:
            raise FfuError("unsupported descriptor location count")
        location_total += location_count
        if location_total > MAX_EXTENTS:
            raise FfuError("too many target extents")
        if not 1 <= block_count <= payload_blocks - block:
            raise FfuError("descriptor block count exceeds payload bounds")
        if source.tell() + location_count * 8 > descriptor_end:
            raise FfuError("truncated disk locations")
        if any(block < boundary < block + block_count for boundary in boundaries):
            raise FfuError("descriptor crosses a partition-table phase boundary")
        phase = next((name for index, count, name in ranges if index <= block < index + count), "data")
        locations: list[DiskLocation] = []
        for _ in range(location_count):
            method, index = struct.unpack("<II", _read_exact(source, 8))
            if method not in (0, 2):
                raise FfuError("unsupported disk access method")
            locations.append(DiskLocation(method, index))
        descriptors.append(WriteDescriptor(block, block_count, tuple(locations), phase))
        block += block_count
    if source.tell() != descriptor_end or block != payload_blocks:
        raise FfuError("descriptor lengths/counts do not match the complete payload")
    _zero_padding(source, payload_offset)

    digest = hashlib.sha256()
    source.seek(0)
    for offset in range(0, file_bytes, chunk):
        data = _read_exact(source, chunk)
        digest.update(data)
        if offset >= image_offset:
            index = (offset - image_offset) // chunk
            if hashlib.sha256(data).digest() != hashes[index * 32 : (index + 1) * 32]:
                raise FfuError(f"FFU chunk hash mismatch at chunk {index}")
    if source.read(1):
        raise FfuError("FFU changed size during validation")
    info = ParsedFfu(
        digest.hexdigest(), file_bytes, chunk, SECTOR_SIZE, sectors * SECTOR_SIZE,
        image_offset, payload_offset, file_bytes - payload_offset, version,
        platforms, hashes, tuple(descriptors),
    )
    try:
        _resolve_plan(info, info.minimum_disk_bytes)
    except TargetError as exc:
        raise FfuError(f"invalid FFU minimum geometry: {exc}") from exc
    return info


def _resolve_plan(info: ParsedFfu, target_size: int) -> WritePlan:
    if (
        type(target_size) is not int
        or not info.minimum_disk_bytes <= target_size <= MAX_TARGET_BYTES
        or target_size % info.sector_size
    ):
        raise TargetError("target size is too small, too large, or not sector-aligned")
    writes: list[Extent] = []
    for descriptor in info.descriptors:
        length = descriptor.block_count * info.chunk_size
        for location in descriptor.locations:
            if location.method == 0:
                offset = location.block_index * info.chunk_size
            else:
                #DISK_END counts backwards to the START of a forward-writing extent.
                offset = target_size - (location.block_index + 1) * info.chunk_size
            if offset < 0 or offset + length > target_size or offset % info.sector_size:
                raise TargetError("FFU extent exceeds target bounds or sector alignment")
            writes.append(Extent(offset, descriptor.payload_block, descriptor.block_count, descriptor.phase))
    ordered = sorted(writes, key=lambda extent: (extent.offset, extent.block_count))
    final: list[Extent] = []
    previous_end = 0
    ranks = {"initial": 0, "flash": 1, "final": 2}
    for (offset, count), matches in itertools.groupby(
        ordered, key=lambda extent: (extent.offset, extent.block_count)
    ):
        group = tuple(matches)
        if offset < previous_end:
            raise FfuError("partially overlapping target extents are unsupported")
        previous_end = offset + count * info.chunk_size
        if len(group) > 1:
            phases = [ranks.get(extent.phase, -1) for extent in group]
            if -1 in phases or phases != sorted(set(phases)) or phases[-1] != 2:
                raise FfuError("overlapping data or duplicate table locations are unsafe")
        if group[-1].phase in ("initial", "flash"):
            raise FfuError("temporary table extent is not replaced by an exact final extent")
        final.append(group[-1])
    return WritePlan(tuple(writes), tuple(final))


def _open_path(path: Path, flags: int, kind: str, source_id: Optional[tuple[int, int]] = None) -> int:
    error = FfuError if kind == "source" else TargetError
    absolute = path.absolute()
    if ".." in absolute.parts or len(absolute.parts) > 64:
        raise error("path traversal or excessive path depth is unsupported")
    if not hasattr(os, "O_NOFOLLOW") or not hasattr(os, "O_DIRECTORY"):
        raise error("host lacks required no-follow file-opening safeguards")
    base_flags = os.O_NOFOLLOW | os.O_CLOEXEC
    parent = os.open(absolute.anchor, os.O_RDONLY | os.O_DIRECTORY | base_flags)
    try:
        for part in absolute.parts[1:-1]:
            child = os.open(part, os.O_RDONLY | os.O_DIRECTORY | base_flags, dir_fd=parent)
            os.close(parent)
            parent = child
        before = os.stat(absolute.name, dir_fd=parent, follow_symlinks=False)
        if stat.S_ISLNK(before.st_mode):
            raise error("symlink paths are not allowed")
        if source_id == (before.st_dev, before.st_ino):
            raise TargetError("source and target are the same file")
        if kind in ("source", "fixture"):
            if not stat.S_ISREG(before.st_mode):
                raise error("source/fixture must be an existing regular file")
            if kind == "fixture" and before.st_nlink != 1:
                raise TargetError("fixture target must not have hard-link aliases")
        elif kind == "device":
            if sys.platform == "darwin":
                valid = stat.S_ISCHR(before.st_mode) and re.fullmatch(r"/dev/rdisk[0-9]+", str(absolute))
            elif sys.platform.startswith("linux"):
                valid = stat.S_ISBLK(before.st_mode)
                flags |= os.O_EXCL
            else:
                raise TargetError("only native Linux/macOS device targets are supported")
            if not valid:
                raise TargetError("production apply requires a block device or macOS whole /dev/rdiskN; regular files are refused")
        else:
            raise error("invalid file-opening mode")
        fd = os.open(absolute.name, flags | base_flags | os.O_NONBLOCK, dir_fd=parent)
        success = False
        try:
            after = os.fstat(fd)
            if (before.st_dev, before.st_ino, before.st_mode, before.st_rdev) != (
                after.st_dev, after.st_ino, after.st_mode, after.st_rdev
            ):
                raise error("file/device identity changed while opening")
            success = True
            return fd
        finally:
            if not success:
                os.close(fd)
    finally:
        os.close(parent)


def _source_stamp(fd: int) -> SourceStamp:
    value = os.fstat(fd)
    return (
        value.st_dev, value.st_ino, value.st_mode, value.st_size,
        value.st_mtime_ns, value.st_ctime_ns, value.st_nlink,
    )


def _target_id(value: os.stat_result) -> str:
    #Darwin exposes signed dev_t values; preserve the native ID rather than truncating it.
    return f"{value.st_dev}:{value.st_ino}:{value.st_rdev}"


def identify_target(path: Path) -> dict[str, str]:
    """Snapshot a device node without opening it or requiring write access."""
    path = Path(path).absolute()
    value = os.stat(path, follow_symlinks=False)
    if sys.platform == "darwin":
        valid = stat.S_ISCHR(value.st_mode) and re.fullmatch(r"/dev/rdisk[0-9]+", str(path))
    else:
        valid = sys.platform.startswith("linux") and stat.S_ISBLK(value.st_mode)
    if not valid:
        raise TargetError("target identity requires a native device node, not a file or symlink")
    return {"target_id": _target_id(value)}


def _check_source(source: BinaryIO, stamp: SourceStamp) -> None:
    if _source_stamp(source.fileno()) != stamp:
        raise ApplyError("source image changed; stop and do not boot an incomplete target")


@contextmanager
def _source_handle(path: Path) -> Iterator[tuple[BinaryIO, SourceStamp]]:
    fd = _open_path(path, os.O_RDONLY, "source")
    with os.fdopen(fd, "rb", buffering=0) as source:
        fcntl.flock(source.fileno(), fcntl.LOCK_SH | fcntl.LOCK_NB)
        stamp = _source_stamp(source.fileno())
        yield source, stamp


def load_provenance() -> dict[str, Any]:
    with _source_handle(PROVENANCE_PATH) as (source, stamp):
        raw = source.read(MAX_MANIFEST_BYTES + 1)
        _check_source(source, stamp)
    if len(raw) > MAX_MANIFEST_BYTES or hashlib.sha256(raw).hexdigest() != PROVENANCE_SHA256:
        raise FfuError("bundled IoT provenance manifest is missing, modified, or untrusted")
    manifest = json.loads(raw)
    if manifest.get("schema_version") != 1:
        raise FfuError("unsupported bundled provenance schema")
    return manifest


def _approved_images() -> tuple[ApprovedImage, ...]:
    manifest = load_provenance()
    return tuple(
        ApprovedImage(
            item["sha256"], item["format"], item["profile"], item["architecture"],
            item["build"], item["os_version"], tuple(item["platform_ids"]),
            tuple(item["recommended_boards"]), item["ffu_bytes"], item["payload_bytes"],
            item["minimum_disk_bytes"], item["sector_size"], item["chunk_size"], item["source_url"],
        )
        for item in manifest["images"]
    )


def image_profile(board: str) -> dict[str, object]:
    """Return reviewed expectations, not verification of any downloaded image."""
    matches = tuple(image for image in _approved_images() if board in image.recommended_boards)
    if len(matches) != 1:
        raise FfuError("board must identify exactly one reviewed Raspberry Pi IoT Core image")
    approved = matches[0]
    if (
        approved.architecture != "arm32" or approved.profile != "iot-core"
        or approved.format != "ffu-v1" or approved.sector_size != 512
        or approved.minimum_disk_bytes <= 0
    ):
        raise FfuError("the reviewed image profile is incompatible with this IoT Core workflow")
    return {
        "architecture": approved.architecture,
        "profile": approved.profile,
        "format": approved.format,
        "expected_build": approved.build,
        "expected_sha256": approved.sha256,
        "minimum_disk_bytes": approved.minimum_disk_bytes,
        "recommended_boards": list(approved.recommended_boards),
        "source_url": approved.source_url,
        "source_verification": "not-assessed",
        "trust": "pinned-bundled-profile",
    }


def _check_approval(info: ParsedFfu, approved: ApprovedImage) -> None:
    if approved.architecture != "arm32" or approved.profile != "iot-core" or approved.format != "ffu-v1":
        raise FfuError("only approved ARM32 Windows 10 IoT Core V1 images are supported")
    if info.sha256 != approved.sha256:
        raise FfuError("FFU SHA256 is not approved")
    if (
        info.os_version != approved.os_version
        or info.os_version != f"10.0.{approved.build}"
        or info.platform_ids != approved.platform_ids
        or "Broadcom.RPi.2" not in info.platform_ids
        or not approved.recommended_boards
        or not set(approved.recommended_boards) <= {"pi2-v1.1", "pi2-v1.2", "pi3-b"}
        or (info.file_bytes, info.payload_bytes, info.minimum_disk_bytes, info.sector_size, info.chunk_size)
        != (approved.file_bytes, approved.payload_bytes, approved.minimum_disk_bytes, approved.sector_size, approved.chunk_size)
    ):
        raise FfuError("FFU identity/geometry does not match the pinned Raspberry Pi image")


def _approve(info: ParsedFfu) -> ApprovedImage:
    for approved in _approved_images():
        if approved.sha256 == info.sha256:
            _check_approval(info, approved)
            return approved
    raise FfuError("FFU SHA256 is not allowlisted; custom, ARM64, non-IoT and other images are unsupported")


def _inspection(info: ParsedFfu, approved: ApprovedImage) -> dict[str, object]:
    return {
        "format": approved.format,
        "architecture": approved.architecture,
        "profile": approved.profile,
        "build": approved.build,
        "os_version": info.os_version,
        "platform_ids": list(info.platform_ids),
        "recommended_boards": list(approved.recommended_boards),
        "minimum_disk_bytes": info.minimum_disk_bytes,
        "payload_bytes": info.payload_bytes,
        "file_bytes": info.file_bytes,
        "chunk_size": info.chunk_size,
        "sector_size": info.sector_size,
        "sha256": info.sha256,
        "source_url": approved.source_url,
        "trust": "pinned-official-sha256",
        "hardware_tested": False,
    }


def inspect_ffu(
    path: Path, *, hdmi_mode: str = "official", hdmi_config: str = "",
) -> dict[str, object]:
    """Return JSON-compatible identity only for an entirely validated, approved FFU."""
    with _source_handle(Path(path)) as (source, stamp):
        info = parse_ffu(source)
        approved = _approve(info)
        preferences = hdmi_options(hdmi_mode, hdmi_config)
        blocks = _hdmi_blocks(source, stamp, info, _resolve_plan(info, info.minimum_disk_bytes), hdmi_mode, hdmi_config)
        _check_source(source, stamp)
        result = _inspection(info, approved)
        result["hdmi"] = {"mode": hdmi_mode, "label": preferences["label"], "configured_blocks": len(blocks)}
        return result


def _ioctl_number(fd: int, request: int, size: int) -> int:
    buffer = bytearray(size)
    fcntl.ioctl(fd, request, buffer, True)
    return int.from_bytes(buffer, sys.byteorder)


def _device_geometry(fd: int) -> tuple[int, int]:
    if sys.platform == "darwin":
        sector = _ioctl_number(fd, 0x40046418, 4)  # DKIOCGETBLOCKSIZE
        count = _ioctl_number(fd, 0x40086419, 8)  # DKIOCGETBLOCKCOUNT
        return sector * count, sector
    if sys.platform.startswith("linux"):
        #BLKGETSIZE64 uses sizeof(size_t) in its ioctl number, even for the 64-bit result.
        request = 0x80001272 | (struct.calcsize("P") << 16)
        return _ioctl_number(fd, request, 8), _ioctl_number(fd, 0x1268, 4)
    raise TargetError("unsupported host for device geometry")


def _check_target(fd: int, target_size: int, sector_size: int, fixture: bool) -> None:
    if fixture:
        current = os.fstat(fd)
        actual, sector = current.st_size, SECTOR_SIZE
        if not stat.S_ISREG(current.st_mode) or current.st_nlink != 1:
            raise TargetError("fixture target identity changed")
    else:
        actual, sector = _device_geometry(fd)
    if actual != target_size or sector != sector_size:
        raise TargetError("actual target size/sector size does not match validated geometry")


def _payload_block(source: BinaryIO, stamp: SourceStamp, info: ParsedFfu, block: int) -> bytes:
    _check_source(source, stamp)
    source.seek(info.payload_offset + block * info.chunk_size)
    data = _read_exact(source, info.chunk_size)
    index = (info.payload_offset - info.image_offset) // info.chunk_size + block
    if hashlib.sha256(data).digest() != info.chunk_hashes[index * 32 : (index + 1) * 32]:
        raise ApplyError("source payload changed after preflight; target may be incomplete")
    _check_source(source, stamp)
    return data


def _image_bytes(
    source: BinaryIO, stamp: SourceStamp, info: ParsedFfu, plan: WritePlan, offset: int, size: int,
) -> bytes:
    if offset < 0 or not 0 <= size <= 512 * 1024:
        raise FfuError("boot configuration read exceeds its bounded range")
    result = bytearray()
    while size:
        extent = next((
            item for item in plan.final_extents
            if item.offset <= offset < item.offset + item.block_count * info.chunk_size
        ), None)
        if extent is None:
            raise FfuError("boot configuration references bytes not supplied by the verified FFU")
        index, within = divmod(offset - extent.offset, info.chunk_size)
        take = min(size, info.chunk_size - within)
        data = _payload_block(source, stamp, info, extent.payload_block + index)
        result.extend(data[within : within + take])
        offset += take
        size -= take
    return bytes(result)


def _hdmi_blocks(
    source: BinaryIO, stamp: SourceStamp, info: ParsedFfu, plan: WritePlan,
    mode: str, custom: str,
) -> tuple[ConfiguredBlock, ...]:
    preferences = hdmi_options(mode, custom)
    if mode == "official":
        return ()
    settings = str(preferences["settings"])

    def read(offset: int, size: int) -> bytes:
        return _image_bytes(source, stamp, info, plan, offset, size)

    mbr = read(0, SECTOR_SIZE)
    start_sector, partition_sectors = struct.unpack_from("<II", mbr, 454)
    if (
        mbr[510:512] != b"\x55\xaa" or mbr[450] not in (0x06, 0x0E, 0x0C)
        or not start_sector or not partition_sectors
        or (start_sector + partition_sectors) * SECTOR_SIZE > info.minimum_disk_bytes
    ):
        raise FfuError("HDMI customization requires the image's existing first FAT boot partition")
    start = start_sector * SECTOR_SIZE
    boot = read(start, SECTOR_SIZE)
    sector = struct.unpack_from("<H", boot, 11)[0]
    sectors_per_cluster = boot[13]
    reserved, roots = struct.unpack_from("<H", boot, 14)[0], struct.unpack_from("<H", boot, 17)[0]
    fat_sectors = struct.unpack_from("<H", boot, 22)[0]
    total_sectors = struct.unpack_from("<H", boot, 19)[0] or struct.unpack_from("<I", boot, 32)[0]
    if (
        boot[510:512] != b"\x55\xaa" or boot[54:62] != b"FAT16   " or sector != SECTOR_SIZE
        or sectors_per_cluster not in (1, 2, 4, 8, 16, 32, 64, 128) or boot[16] != 2
        or not 1 <= reserved <= 128 or not 1 <= roots <= 4096
        or not 1 <= fat_sectors <= 1024 or total_sectors != partition_sectors
    ):
        raise FfuError("HDMI customization requires a consistent bounded FAT16 boot filesystem")
    root_sectors = (roots * 32 + sector - 1) // sector
    data_sector = reserved + 2 * fat_sectors + root_sectors
    clusters = (total_sectors - data_sector) // sectors_per_cluster
    if not 4085 <= clusters < 65525 or (clusters + 2) * 2 > fat_sectors * sector:
        raise FfuError("invalid FAT16 allocation bounds for HDMI customization")
    fat_start = start + reserved * sector
    fat = read(fat_start, fat_sectors * sector)
    if fat != read(fat_start + fat_sectors * sector, fat_sectors * sector):
        raise FfuError("boot FAT copies disagree; HDMI customization was not applied")
    root_start = start + (reserved + 2 * fat_sectors) * sector
    directory = read(root_start, roots * 32)
    entries: list[tuple[int, bytes]] = []
    for position in range(0, len(directory), 32):
        entry = directory[position : position + 32]
        if entry[0] == 0:
            break
        if entry[0] == 0xE5 or entry[11] == 0x0F:
            continue
        if entry[:11].upper() == b"CONFIG  TXT":
            entries.append((position, entry))
    if len(entries) != 1 or entries[0][1][11] & 0x18:
        raise FfuError("boot config.txt must be one regular file in the FAT16 root")
    entry_position, entry = entries[0]
    file_size = struct.unpack_from("<I", entry, 28)[0]
    cluster = struct.unpack_from("<H", entry, 26)[0]
    if not 0 < file_size <= MAX_BOOT_CONFIG_BYTES or struct.unpack_from("<H", entry, 20)[0]:
        raise FfuError("boot config.txt size or first cluster is invalid")
    chain: list[int] = []
    while cluster < 0xFFF8:
        if not 2 <= cluster < clusters + 2 or cluster in chain or len(chain) >= 16:
            raise FfuError("boot config.txt has an invalid or excessive FAT chain")
        chain.append(cluster)
        cluster = struct.unpack_from("<H", fat, cluster * 2)[0]
    cluster_size = sectors_per_cluster * sector
    capacity = len(chain) * cluster_size
    if not chain or not (len(chain) - 1) * cluster_size < file_size <= capacity:
        raise FfuError("boot config.txt allocation does not match its size")
    offsets = [start + data_sector * sector + (item - 2) * cluster_size for item in chain]
    original = b"".join(read(offset, cluster_size) for offset in offsets)[:file_size]
    try:
        text = original.decode("ascii")
    except UnicodeDecodeError as exc:
        raise FfuError("boot config.txt is not an ASCII firmware configuration") from exc
    if "\0" in text or any(line.strip().startswith("[") for line in text.splitlines()):
        raise FfuError("filtered or binary boot configurations are not supported for HDMI customization")
    keys = {line.split("=", 1)[0] for line in settings.splitlines()}
    retained: list[str] = []
    for line in text.splitlines(keepends=True):
        assignment = re.match(r"^\s*([a-z][a-z0-9_]*)\s*=", line)
        if assignment is None or assignment.group(1) not in keys:
            retained.append(line)
    newline = "\r\n" if "\r\n" in text else "\n"
    prefix = "".join(retained).rstrip("\r\n") + newline
    replacement = (prefix + "# WoR-Flasher HDMI timing override" + newline + settings.replace("\n", newline)).encode("ascii")
    if len(replacement) > capacity or len(replacement) > MAX_BOOT_CONFIG_BYTES:
        raise FfuError("HDMI settings exceed the existing config.txt allocation; no filesystem resize is attempted")
    padded = replacement.ljust(capacity, b"\0")
    changes = [
        (offset, padded[index * cluster_size : (index + 1) * cluster_size])
        for index, offset in enumerate(offsets)
    ]
    changes.append((root_start + entry_position + 28, struct.pack("<I", len(replacement))))
    changed: dict[int, tuple[int, bytearray]] = {}
    for offset, data in changes:
        while data:
            block_offset = offset // info.chunk_size * info.chunk_size
            within = offset - block_offset
            take = min(len(data), info.chunk_size - within)
            if block_offset not in changed:
                extent = next((
                    item for item in plan.final_extents
                    if item.offset <= block_offset < item.offset + item.block_count * info.chunk_size
                ), None)
                if extent is None:
                    raise FfuError("configured boot block is not supplied by the verified FFU")
                payload = extent.payload_block + (block_offset - extent.offset) // info.chunk_size
                changed[block_offset] = (payload, bytearray(_payload_block(source, stamp, info, payload)))
            changed[block_offset][1][within : within + take] = data[:take]
            offset += take
            data = data[take:]
    return tuple(
        ConfiguredBlock(offset, payload, bytes(data))
        for offset, (payload, data) in sorted(changed.items())
    )


def _write_all(fd: int, data: bytes, offset: int, sector: int) -> None:
    view = memoryview(data)
    done = 0
    while done < len(view):
        count = os.pwrite(fd, view[done:], offset + done)
        if count <= 0 or count > len(view) - done or count % sector:
            raise ApplyError("short or unaligned target write; target may be incomplete")
        done += count


def _read_target(fd: int, size: int, offset: int, sector: int) -> bytes:
    data = bytearray()
    while len(data) < size:
        part = os.pread(fd, size - len(data), offset + len(data))
        if not part or len(part) % sector:
            raise ApplyError("short or unaligned target readback; target may be incomplete")
        data.extend(part)
    return bytes(data)


def _sync_target(fd: int, fixture: bool, *, before_readback: bool = False) -> None:
    os.fsync(fd)
    if not fixture:
        if sys.platform == "darwin":
            #XNU fsync is a no-op for VCHR; raw disks need an explicit media flush.
            fcntl.ioctl(fd, 0x20006416)  # DKIOCSYNCHRONIZECACHE
        elif before_readback:
            fcntl.ioctl(fd, 0x1261)  # BLKFLSBUF: discard cached blocks before readback.


def _reject_existing_gpt(fd: int, info: ParsedFfu, target_size: int) -> None:
    #The fixed MBR image would leave an old GPT backup outside its write extents.
    offsets = {info.sector_size, info.minimum_disk_bytes - info.sector_size, target_size - info.sector_size}
    for offset in sorted(offsets):
        data = _read_target(fd, info.sector_size, offset, info.sector_size)
        if data[:8] == b"EFI PART":
            raise TargetError(
                "existing GPT target is unsupported by this fixed MBR/EBR IoT image; "
                "use a deliberately prepared blank/MBR target, not automatic GPT conversion"
            )


def _gpt_header(data: bytes, lba: int, last_lba: int) -> GptHeader:
    if data[:8] != b"EFI PART":
        raise TargetError("GPT cleanup requires both valid primary and backup headers; no metadata was erased")
    revision, size, checksum, reserved = struct.unpack_from("<IIII", data, 8)
    if revision != 0x10000 or not 92 <= size <= SECTOR_SIZE or reserved != 0:
        raise TargetError("unsupported GPT header geometry; no metadata was erased")
    checked = bytearray(data[:size])
    struct.pack_into("<I", checked, 16, 0)
    if zlib.crc32(checked) != checksum:
        raise TargetError("GPT header checksum is invalid; no metadata was erased")
    current, other, first, last = struct.unpack_from("<QQQQ", data, 24)
    entries, count, entry_size, table_crc = struct.unpack_from("<QIII", data, 72)
    expected_other = last_lba if lba == 1 else 1
    if current != lba or other != expected_other or not 2 < first <= last < last_lba:
        raise TargetError("GPT header bounds do not match the confirmed disk; no metadata was erased")
    table_bytes = count * entry_size
    sectors = (table_bytes + SECTOR_SIZE - 1) // SECTOR_SIZE
    if count == 0 or entry_size < 128 or entry_size % 128 or table_bytes > MAX_GPT_TABLE_BYTES:
        raise TargetError("GPT partition table exceeds supported cleanup bounds; no metadata was erased")
    if lba == 1:
        valid_table = 2 <= entries and entries + sectors <= first
    else:
        valid_table = last < entries and entries + sectors <= last_lba
    if not valid_table:
        raise TargetError("GPT table overlaps usable data or a header; no metadata was erased")
    return GptHeader(current, other, first, last, data[56:72], entries, count, entry_size, table_crc)


def _gpt_cleanup_plan(fd: int, info: ParsedFfu, target_size: int) -> tuple[MetadataRegion, ...]:
    last_lba = target_size // SECTOR_SIZE - 1
    positions = {SECTOR_SIZE, info.minimum_disk_bytes - SECTOR_SIZE, target_size - SECTOR_SIZE}
    probes = {offset: _read_target(fd, SECTOR_SIZE, offset, SECTOR_SIZE) for offset in positions}
    signatures = {offset for offset, data in probes.items() if data[:8] == b"EFI PART"}
    if not signatures:
        return ()
    expected = {SECTOR_SIZE, target_size - SECTOR_SIZE}
    if signatures != expected:
        raise TargetError("orphaned or unexpectedly located GPT metadata; no metadata was erased")
    primary = _gpt_header(probes[SECTOR_SIZE], 1, last_lba)
    backup = _gpt_header(probes[target_size - SECTOR_SIZE], last_lba, last_lba)
    primary_identity = (
        primary.first_usable, primary.last_usable, primary.disk_guid,
        primary.entry_count, primary.entry_size, primary.table_crc,
    )
    backup_identity = (
        backup.first_usable, backup.last_usable, backup.disk_guid,
        backup.entry_count, backup.entry_size, backup.table_crc,
    )
    if primary_identity != backup_identity:
        raise TargetError("GPT copies disagree; no metadata was erased")
    table_bytes = primary.entry_count * primary.entry_size
    table_span = _align(table_bytes, SECTOR_SIZE)
    primary_table = _read_target(fd, table_span, primary.entries_lba * SECTOR_SIZE, SECTOR_SIZE)
    backup_table = _read_target(fd, table_span, backup.entries_lba * SECTOR_SIZE, SECTOR_SIZE)
    if (
        zlib.crc32(primary_table[:table_bytes]) != primary.table_crc
        or zlib.crc32(backup_table[:table_bytes]) != backup.table_crc
        or primary_table[:table_bytes] != backup_table[:table_bytes]
    ):
        raise TargetError("GPT partition-table checksum or copy mismatch; no metadata was erased")
    partitions: list[tuple[int, int]] = []
    for index in range(primary.entry_count):
        entry = primary_table[index * primary.entry_size : (index + 1) * primary.entry_size]
        if entry[:16] == b"\0" * 16:
            continue
        start, end = struct.unpack_from("<QQ", entry, 32)
        if not primary.first_usable <= start <= end <= primary.last_usable:
            raise TargetError("GPT partition lies outside usable bounds; no metadata was erased")
        partitions.append((start, end))
    previous_end = -1
    for start, end in sorted(partitions):
        if start <= previous_end:
            raise TargetError("GPT partitions overlap; no metadata was erased")
        previous_end = end
    regions = (
        MetadataRegion(0, _read_target(fd, SECTOR_SIZE, 0, SECTOR_SIZE)),
        MetadataRegion(SECTOR_SIZE, probes[SECTOR_SIZE]),
        MetadataRegion(primary.entries_lba * SECTOR_SIZE, primary_table),
        MetadataRegion(backup.entries_lba * SECTOR_SIZE, backup_table),
        MetadataRegion(target_size - SECTOR_SIZE, probes[target_size - SECTOR_SIZE]),
    )
    ordered = tuple(sorted(regions, key=lambda region: region.offset))
    previous_end = 0
    for region in ordered:
        if region.offset < previous_end or region.offset + len(region.original) > target_size:
            raise TargetError("GPT cleanup regions overlap or exceed the target; no metadata was erased")
        previous_end = region.offset + len(region.original)
    return ordered


def _clear_gpt_metadata(
    source: BinaryIO, stamp: SourceStamp, info: ParsedFfu, target: int,
    target_size: int, fixture: bool, regions: tuple[MetadataRegion, ...],
    progress: Optional[Progress],
) -> None:
    _check_source(source, stamp)
    _check_target(target, target_size, info.sector_size, fixture)
    for region in regions:
        if _read_target(target, len(region.original), region.offset, SECTOR_SIZE) != region.original:
            raise TargetError("GPT metadata changed during preflight; no metadata was erased")
    if progress is not None:
        progress("Preparing GPT metadata", 0)
    for index, region in enumerate(regions):
        _check_source(source, stamp)
        _check_target(target, target_size, info.sector_size, fixture)
        _write_all(target, b"\0" * len(region.original), region.offset, SECTOR_SIZE)
        if progress is not None:
            progress("Preparing GPT metadata", min(99, (index + 1) * 100 // len(regions)))
    _sync_target(target, fixture, before_readback=True)
    for region in regions:
        if _read_target(target, len(region.original), region.offset, SECTOR_SIZE) != b"\0" * len(region.original):
            raise ApplyError("GPT cleanup readback failed; target may be incomplete, do not boot it")
    _check_source(source, stamp)
    _check_target(target, target_size, info.sector_size, fixture)
    if progress is not None:
        progress("Preparing GPT metadata", 100)


def _verify_gpt_cleanup(
    target: int, info: ParsedFfu, plan: WritePlan, regions: tuple[MetadataRegion, ...],
) -> None:
    final = sorted(plan.final_extents, key=lambda extent: extent.offset)
    for region in regions:
        position = region.offset
        end = position + len(region.original)
        for extent in final:
            extent_end = extent.offset + extent.block_count * info.chunk_size
            if extent_end <= position:
                continue
            if extent.offset >= end:
                break
            clear_end = min(end, extent.offset)
            if clear_end > position:
                actual = _read_target(target, clear_end - position, position, SECTOR_SIZE)
                if actual != b"\0" * len(actual):
                    raise ApplyError("old GPT metadata reappeared during readback; do not boot this target")
            position = max(position, min(end, extent_end))
        if position < end:
            actual = _read_target(target, end - position, position, SECTOR_SIZE)
            if actual != b"\0" * len(actual):
                raise ApplyError("old GPT metadata reappeared during readback; do not boot this target")
        if region.original[:8] == b"EFI PART":
            if _read_target(target, SECTOR_SIZE, region.offset, SECTOR_SIZE)[:8] == b"EFI PART":
                raise ApplyError("GPT header remains after FFU application; do not boot this target")


def _wipe_target(
    source: BinaryIO, stamp: SourceStamp, info: ParsedFfu, target: int,
    target_size: int, fixture: bool, progress: Optional[Progress],
) -> None:
    identity = _target_id(os.fstat(target))
    target_stamp = _source_stamp(target)
    zeros = bytes(WIPE_CHUNK_BYTES)

    def guard() -> None:
        _check_source(source, stamp)
        _check_target(target, target_size, info.sector_size, fixture)
        if _target_id(os.fstat(target)) != identity:
            raise TargetError("target identity changed during the full wipe; stop using this target")

    for phase in ("Wiping entire drive", "Verifying blank drive"):
        guard()
        last_percent = -1
        if progress is not None:
            progress(phase, 0)
        for offset in range(0, target_size, WIPE_CHUNK_BYTES):
            guard()
            size = min(WIPE_CHUNK_BYTES, target_size - offset)
            if phase == "Wiping entire drive":
                _write_all(target, zeros[:size], offset, info.sector_size)
            elif _read_target(target, size, offset, info.sector_size) != zeros[:size]:
                raise ApplyError(f"full-wipe readback mismatch at byte {offset}; image was not applied")
            percent = min(99, (offset + size) * 100 // target_size)
            if progress is not None and percent != last_percent:
                progress(phase, percent)
            last_percent = percent
        if phase == "Wiping entire drive":
            _sync_target(target, fixture, before_readback=True)
            target_stamp = _source_stamp(target)
        elif _source_stamp(target) != target_stamp:
            raise ApplyError("target changed during full-wipe verification; image was not applied")
        guard()
        if progress is not None:
            progress(phase, 100)


def _apply_plan(
    source: BinaryIO, stamp: SourceStamp, info: ParsedFfu, plan: WritePlan,
    target: int, target_size: int, fixture: bool, progress: Optional[Progress],
    allow_gpt_cleanup: bool = False,
    configured_blocks: tuple[ConfiguredBlock, ...] = (),
    wipe_entire_drive: bool = False,
) -> None:
    _check_source(source, stamp)
    _check_target(target, target_size, info.sector_size, fixture)
    if not fixture:
        _sync_target(target, fixture, before_readback=True)
    cleanup: tuple[MetadataRegion, ...] = ()
    if wipe_entire_drive:
        _wipe_target(source, stamp, info, target, target_size, fixture, progress)
    elif allow_gpt_cleanup:
        cleanup = _gpt_cleanup_plan(target, info, target_size)
        if cleanup:
            _clear_gpt_metadata(source, stamp, info, target, target_size, fixture, cleanup, progress)
    else:
        _reject_existing_gpt(target, info, target_size)
    last_percent = -1
    configured = {block.offset: block for block in configured_blocks}

    def output_block(offset: int, payload: int) -> bytes:
        data = _payload_block(source, stamp, info, payload)
        block = configured.get(offset)
        return block.data if block is not None and block.payload_block == payload else data

    def report(phase: str, percent: int) -> None:
        nonlocal last_percent
        if progress is not None and percent != last_percent:
            progress(phase, percent)
        last_percent = percent

    total = sum(extent.block_count for extent in plan.writes)
    done = 0
    previous_phase = ""
    report("Writing", 0)
    for extent in plan.writes:
        if previous_phase and previous_phase != extent.phase:
            _sync_target(target, fixture)
        previous_phase = extent.phase
        for index in range(extent.block_count):
            offset = extent.offset + index * info.chunk_size
            data = output_block(offset, extent.payload_block + index)
            _write_all(target, data, offset, info.sector_size)
            done += 1
            report("Writing", min(99, done * 100 // total))
    _sync_target(target, fixture, before_readback=True)
    _check_source(source, stamp)
    target_stamp = _source_stamp(target)
    report("Writing", 100)
    last_percent = -1
    total = sum(extent.block_count for extent in plan.final_extents)
    done = 0
    report("Verifying", 0)
    for extent in plan.final_extents:
        for index in range(extent.block_count):
            offset = extent.offset + index * info.chunk_size
            expected = output_block(offset, extent.payload_block + index)
            actual = _read_target(target, info.chunk_size, offset, info.sector_size)
            if actual != expected:
                raise ApplyError(f"target readback mismatch at byte {offset}; do not boot this target")
            done += 1
            report("Verifying", min(99, done * 100 // total))
    _verify_gpt_cleanup(target, info, plan, cleanup)
    _check_source(source, stamp)
    _check_target(target, target_size, info.sector_size, fixture)
    if _source_stamp(target) != target_stamp:
        raise ApplyError("target changed during readback; do not boot this target")
    report("Verifying", 100)


def _apply(
    path: Path, target_path: Path, target_size: int, fixture: bool,
    progress: Optional[Progress], expected_sha256: Optional[str] = None,
    expected_target_id: Optional[str] = None,
    allow_gpt_cleanup: bool = False,
    hdmi_mode: str = "official", hdmi_config: str = "",
    wipe_entire_drive: bool = False,
) -> ParsedFfu:
    hdmi_options(hdmi_mode, hdmi_config)
    if type(allow_gpt_cleanup) is not bool:
        raise TargetError("GPT cleanup authorization must be a boolean")
    if type(wipe_entire_drive) is not bool:
        raise TargetError("full-drive wipe authorization must be a boolean")
    if wipe_entire_drive and allow_gpt_cleanup:
        raise TargetError("choose a full-drive wipe or bounded GPT cleanup, not both")
    if wipe_entire_drive and not fixture and (expected_sha256 is None or expected_target_id is None):
        raise TargetError("full-drive wipe requires the verified image digest and confirmed target identity")
    if allow_gpt_cleanup and not fixture and (expected_sha256 is None or expected_target_id is None):
        raise TargetError("GPT cleanup requires the verified image digest and confirmed target identity")
    if expected_sha256 is not None and not re.fullmatch(r"[0-9a-f]{64}", expected_sha256):
        raise FfuError("expected SHA256 must be 64 lowercase hexadecimal characters")
    if expected_target_id is not None and not re.fullmatch(r"-?[0-9]+:[0-9]+:-?[0-9]+", expected_target_id):
        raise TargetError("expected target identity is malformed")
    with _source_handle(Path(path)) as (source, stamp):
        info = parse_ffu(source)
        if not fixture:
            _approve(info)
        if expected_sha256 is not None and info.sha256 != expected_sha256:
            raise FfuError("FFU SHA256 changed since inspection (expected hash mismatch)")
        plan = _resolve_plan(info, target_size)
        configured_blocks = _hdmi_blocks(source, stamp, info, plan, hdmi_mode, hdmi_config)
        _check_source(source, stamp)
        target = _open_path(
            Path(target_path), os.O_RDWR, "fixture" if fixture else "device", (stamp[0], stamp[1])
        )
        try:
            if expected_target_id is not None and _target_id(os.fstat(target)) != expected_target_id:
                raise TargetError("target device changed since confirmation; no image blocks were written")
            if fixture:
                fcntl.flock(target, fcntl.LOCK_EX | fcntl.LOCK_NB)
            _apply_plan(
                source, stamp, info, plan, target, target_size, fixture, progress,
                allow_gpt_cleanup, configured_blocks, wipe_entire_drive,
            )
        finally:
            os.close(target)
        return info


def apply_ffu(
    path: Path, target_path: Path, target_size: int, *, progress: Optional[Progress] = None,
    expected_sha256: Optional[str] = None,
    expected_target_id: Optional[str] = None,
    allow_gpt_cleanup: bool = False,
    hdmi_mode: str = "official", hdmi_config: str = "",
    wipe_entire_drive: bool = False,
) -> ParsedFfu:
    """Apply an approved FFU, optionally bound to the caller's prior inspection hash."""
    if sys.platform.startswith("linux"):
        release = os.uname().release.lower()
        if "microsoft" in release or "wsl" in release:
            raise TargetError("WSL is not a safe native disk-flashing host")
    elif sys.platform != "darwin":
        raise TargetError("only native Linux/macOS device targets are supported")
    return _apply(
        path, target_path, target_size, False, progress, expected_sha256, expected_target_id,
        allow_gpt_cleanup, hdmi_mode, hdmi_config, wipe_entire_drive,
    )


def apply_to_file_fixture(
    path: Path, target_path: Path, target_size: int, *, progress: Optional[Progress] = None,
    allow_gpt_cleanup: bool = False,
    hdmi_mode: str = "official", hdmi_config: str = "",
    wipe_entire_drive: bool = False,
) -> ParsedFfu:
    """TEST ONLY: use the real parser/writer on pre-sized regular files, never devices."""
    return _apply(
        path, target_path, target_size, True, progress, allow_gpt_cleanup=allow_gpt_cleanup,
        hdmi_mode=hdmi_mode, hdmi_config=hdmi_config, wipe_entire_drive=wipe_entire_drive,
    )


def _progress(phase: str, percent: int) -> None:
    print(f"{phase} IoT Core {percent}%", file=sys.stderr, flush=True)


def _interrupt(signum: int, _frame: object) -> None:
    raise InterruptedApply(signum)


def main(argv: Optional[Sequence[str]] = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    commands = parser.add_subparsers(dest="command", required=True)
    hdmi_command = commands.add_parser("hdmi-options", help="validate display preferences without opening an image")
    hdmi_command.add_argument("--hdmi-mode", default="official")
    hdmi_command.add_argument("--hdmi-config", default="")
    hdmi_command.add_argument("--json", action="store_true")
    profile_command = commands.add_parser("profile", help="read the reviewed board profile without accessing an image")
    profile_command.add_argument("--board", required=True)
    profile_command.add_argument("--json", action="store_true")
    inspect_command = commands.add_parser("inspect", help="validate and identify an allowlisted local FFU")
    inspect_command.add_argument("file", type=Path)
    inspect_command.add_argument("--json", action="store_true", help="emit machine-readable JSON")
    inspect_command.add_argument("--hdmi-mode", default="official")
    inspect_command.add_argument("--hdmi-config", default="")
    identity_command = commands.add_parser("identify-target", help="read device-node identity without opening it")
    identity_command.add_argument("target", type=Path)
    identity_command.add_argument("--json", action="store_true")
    apply_command = commands.add_parser("apply", help="write and readback-verify an approved FFU")
    apply_command.add_argument("file", type=Path)
    apply_command.add_argument("target", type=Path)
    apply_command.add_argument("--target-size", type=int, required=True)
    apply_command.add_argument(
        "--expected-sha256", help="also require the lowercase SHA256 returned by prior inspection"
    )
    apply_command.add_argument("--expected-target-id", help="require the device node shown at confirmation")
    apply_command.add_argument(
        "--allow-gpt-cleanup", action="store_true",
        help="erase validated old GPT metadata before applying; requires both expected identity arguments",
    )
    apply_command.add_argument("--hdmi-mode", default="official")
    apply_command.add_argument("--hdmi-config", default="")
    apply_command.add_argument(
        "--wipe-entire-drive", action="store_true",
        help="zero and verify every target sector before applying; requires both expected identity arguments",
    )
    args = parser.parse_args(argv)
    try:
        if args.command == "hdmi-options":
            preferences = hdmi_options(args.hdmi_mode, args.hdmi_config)
            print(json.dumps(preferences, sort_keys=True) if args.json else preferences["label"])
        elif args.command == "profile":
            profile = image_profile(args.board)
            if args.json:
                print(json.dumps(profile, sort_keys=True))
            else:
                print(f"Expected Windows 10 IoT Core build: {profile['expected_build']}; image verification: Not Assessed")
        elif args.command == "identify-target":
            print(json.dumps(identify_target(args.target), sort_keys=True))
        elif args.command == "inspect":
            result = inspect_ffu(args.file, hdmi_mode=args.hdmi_mode, hdmi_config=args.hdmi_config)
            if args.json:
                print(json.dumps(result, sort_keys=True))
            else:
                print(f"Windows 10 IoT Core {result['build']} / ARM32 / legacy maker image")
                print(f"Minimum disk: {result['minimum_disk_bytes']} bytes; SHA256: {result['sha256']}")
                print(f"HDMI configuration checked: {hdmi_options(args.hdmi_mode, args.hdmi_config)['label']}")
        else:
            apply_ffu(
                args.file, args.target, args.target_size,
                progress=_progress, expected_sha256=args.expected_sha256,
                expected_target_id=args.expected_target_id,
                allow_gpt_cleanup=args.allow_gpt_cleanup,
                hdmi_mode=args.hdmi_mode, hdmi_config=args.hdmi_config,
                wipe_entire_drive=args.wipe_entire_drive,
            )
            if args.hdmi_mode != "official":
                print(f"HDMI configuration written and readback-verified: {hdmi_options(args.hdmi_mode, args.hdmi_config)['label']}", file=sys.stderr)
        return 0
    except FfuError as exc:
        print(f"IoT Core FFU: {exc}", file=sys.stderr)
        return exc.exit_status
    except OSError as exc:
        print(f"IoT Core FFU I/O failure: {exc}; if writing began, target may be incomplete", file=sys.stderr)
        return 4
    except KeyboardInterrupt:
        print("IoT Core FFU interrupted; target may be incomplete, do not boot it", file=sys.stderr)
        return 130
    except InterruptedApply as exc:
        print("IoT Core FFU interrupted; target may be incomplete, do not boot it", file=sys.stderr)
        return 128 + exc.signum


if __name__ == "__main__":
    signal.signal(signal.SIGTERM, _interrupt)
    raise SystemExit(main())
