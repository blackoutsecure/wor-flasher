#!/usr/bin/env python3
"""Test-only corpus from the unchanged Python implementation, never a runtime bridge."""

from __future__ import annotations

import hashlib
import io
import json
import os
import random
import runpy
import shutil
import struct
import subprocess
import sys
from dataclasses import asdict
from pathlib import Path


ROOT = Path(__file__).resolve().parents[3]
original = runpy.run_path(str(ROOT / "tests/test-iot-ffu.py"), run_name="ffu_oracle_fixtures")
ffu = original["ffu"]
Fixture = original["Fixture"]
HdmiFixture = original["HdmiFixture"]
create_gpt = original["create_gpt_target"]
fixture = Fixture()
directory = Path(sys.argv[1]).resolve()
directory.mkdir(parents=True, exist_ok=True)
parser_cases: list[dict[str, object]] = []
writer_cases: list[dict[str, object]] = []


def parse_case(label: str, blob: bytes, size: int = original["TARGET_SIZE"]) -> None:
    path = directory / f"parse-{len(parser_cases):04}.ffu"
    path.write_bytes(blob)
    result: dict[str, object] = {"label": label, "path": str(path), "target_size": size}
    try:
        info = ffu.parse_ffu(io.BytesIO(blob))
        result["info"] = asdict(info)
        result["info"]["chunk_hashes"] = info.chunk_hashes.hex()
        result["plan"] = asdict(ffu._resolve_plan(info, size))
        result["status"] = 0
    except ffu.FfuError as error:
        result["status"] = error.exit_status
        result["error"] = str(error)
    parser_cases.append(result)


parse_case("all phases, multi-location and multi-block", fixture.blob)
parse_case("non-chunk-aligned target", fixture.blob, original["TARGET_SIZE"] + 512)
parse_case("maximum 2 TiB target", fixture.blob, ffu.MAX_TARGET_BYTES)
for size in (0, original["TARGET_SIZE"] - 512, original["TARGET_SIZE"] + 1, ffu.MAX_TARGET_BYTES + 512):
    parse_case(f"target bounds {size}", fixture.blob, size)

image, store, desc = fixture.image_offset, fixture.store_offset, fixture.descriptor_offset
fields = {
    "security size": (0, "<I", 40), "chunk zero": (16, "<I", 0),
    "chunk non-power": (16, "<I", 3), "chunk excessive": (16, "<I", 2048),
    "SHA1": (20, "<I", 0x8004), "catalog zero": (24, "<I", 0),
    "catalog excessive": (24, "<I", 2**32 - 1), "hash zero": (28, "<I", 0),
    "hash unaligned": (28, "<I", 33), "hash excessive": (28, "<I", 2**32 - 1),
    "hash incomplete": (28, "<I", 7 * 32), "extended image": (image, "<I", 28),
    "manifest empty": (image + 16, "<I", 0), "manifest excessive": (image + 16, "<I", 65537),
    "chunk disagreement": (image + 20, "<I", 2), "delta": (store, "<I", 1),
    "multistore": (store + 4, "<H", 2), "store minor": (store + 6, "<H", 1),
    "compressed": (store + 8, "<H", 3), "FFU minor": (store + 10, "<H", 1),
    "block size": (store + 204, "<I", 512), "descriptor count zero": (store + 208, "<I", 0),
    "descriptor count excessive": (store + 208, "<I", 2**32 - 1),
    "descriptor length excessive": (store + 212, "<I", 2**32 - 1),
    "descriptor length short": (store + 212, "<I", 87),
    "conditional count": (store + 216, "<I", 1), "conditional bytes": (store + 220, "<I", 12),
    "initial not first": (store + 224, "<I", 1), "initial overflow": (store + 228, "<I", 2**32 - 1),
    "phase crossing": (store + 232, "<I", 2), "unordered final": (store + 240, "<I", 2),
    "absent final": (store + 244, "<I", 0), "zero locations": (desc, "<I", 0),
    "excessive locations": (desc, "<I", 17), "zero blocks": (desc + 4, "<I", 0),
    "excessive blocks": (desc + 4, "<I", 2**32 - 1), "unsupported method": (desc + 8, "<I", 1),
    "huge target index": (desc + 12, "<I", 2**32 - 1),
}
for label, (offset, encoding, value) in fields.items():
    parse_case(label, fixture.mutate(offset, encoding, value))

for offset, value in (
    (4, ord("X")), (32, 0x31), (fixture.hash_offset + 8 * 32, 1),
    (image + 4, ord("X")), (store - 1, 1), (fixture.payload_offset - 1, 1),
    (store + 203, 1), (store + 12, 0xFF), (image + 30, 0xFF),
):
    parse_case(f"magic/padding/platform byte {offset}", fixture.mutate(offset, "<B", value))

for catalog in (
    b"\x30\x00", b"\x30\x80", b"\x30\x85\0\0\0\0\0",
    b"\x30\x81\x7f" + b"x" * 127, b"\x30\x82\0\x80" + b"x" * 128,
    b"\x30\x81\x80" + b"x" * 128, b"\x30\x82\x01\0" + b"x" * 256,
    b"\x30\x84\xff\xff\xff\xff",
    b"\x30\x01x", b"\x30\x02x", b"\x30\x01xx",
):
    header = bytearray(fixture.blob[:32])
    struct.pack_into("<I", header, 24, len(catalog))
    hashes = fixture.blob[fixture.hash_offset : fixture.hash_offset + 8 * 32]
    prefix = original["pad"](bytes(header) + catalog + hashes)
    parse_case(f"DER {catalog[:5].hex()} / {len(catalog)}", prefix + fixture.blob[image:])

text = fixture.manifest
for manifest in (
    text + "[Store]\nStoreType=Default\n", text.replace("Version = 2.0", "Version = 3.0"),
    text.replace("Version = 2.0", "Version = 2.0\nversion = 2.0"),
    text.replace("SectorSize = 512", "SectorSize = 4096"),
    text.replace("MinSectorCount = 64", "MinSectorCount = -1"),
    text.replace("MinSectorCount = 64", "MinSectorCount = 999999999999999999999"),
    text.replace("MinSectorCount = 64", "MinSectorCount = 18446744073709551615"),
    text.replace("StoreType = Default", "StoreType = Update"),
    text.replace("IsMainOSStore = True", "IsMainOSStore = False"),
    text.replace("OnlyAllocateDefinedGptEntries = False", "OnlyAllocateDefinedGptEntries = True"),
    text.replace("DevicePlatformId0 = Broadcom.RPi.2", "DevicePlatformId0 = Other.Board"),
    text.replace("OSVersion = 10.0.17763.107", "OSVersion = invalid"),
    text.replace("Name = NonWindowsFixture", "Name ="), text + "[Partition]\nName=NonWindowsFixture\n",
    text.replace("AntiTheftVersion", "Invalid-Key"), text.replace("AntiTheftVersion", "\0"),
    text + "[Partition]\nName=Another\n" * 129,
):
    parse_case(f"manifest {len(parser_cases)}", Fixture(manifest=manifest).blob)

for descriptors, ranges in (
    (((2, ((0, 2),)), (1, ((0, 3),))), (0,) * 6),
    (((1, ((0, 2), (0, 2))),), (0,) * 6),
    (((1, ((0, 2),)), (1, ((0, 2),))), (0,) * 6),
    (((1, ((0, 0),)), (1, ((0, 2),)), (1, ((0, 1),))), (0, 1, 0, 0, 2, 1)),
    (((2, ((2, 1),)),), (0,) * 6),
    (((2, ((2, 0),)),), (0,) * 6),
    (((1, ((0, 3),)),), (0,) * 6),
    (((1, ((0, 0),)), (1, ((0, 0),))), (0, 1, 0, 0, 1, 1)),
    (((1, ((0, 0),)),), (0, 0, 0, 0, 0, 1)),
):
    parse_case(f"extent/phase {len(parser_cases)}", Fixture(descriptors, ranges).blob)
dependent = Fixture(((1, ((0, 30),)), (1, ((2, 3),))), (0,) * 6)
parse_case("target-dependent overlap", dependent.blob, 34 * 1024)

for platforms in (("", "*"), ("A", "A"), ("with space", "*"), ("A", "B" * 190), tuple(str(i) for i in range(17))):
    parse_case(f"platform {platforms!r}", Fixture(platforms=platforms).blob)
for cut in (0, 31, image - 1, fixture.payload_offset - 1, len(fixture.blob) - 1, len(fixture.blob) - 1024):
    parse_case(f"truncation {cut}", fixture.blob[:cut])
for extra in (b"\0", b"\0" * 1024):
    parse_case(f"trailing {len(extra)}", fixture.blob + extra)
rng = random.Random(0xFF001)
for index in range(96):
    blob = bytearray(fixture.blob)
    offset = rng.randrange(len(blob))
    blob[offset] ^= 1 << rng.randrange(8)
    parse_case(f"seeded bit flip {index}", bytes(blob))
for index in range(24):
    blob = bytearray(fixture.blob)
    blob[fixture.payload_offset + rng.randrange(len(blob) - fixture.payload_offset)] ^= 1 << rng.randrange(8)
    parse_case(f"rehashed synthetic payload {index}", fixture.rehash(blob))


def write_case(
    label: str, image: object = fixture, size: int = original["TARGET_SIZE"],
    target: bytes | None = None, **options: object,
) -> None:
    index = len(writer_cases)
    source = directory / f"write-{index:03}.ffu"
    expected = directory / f"expected-{index:03}.raw"
    initial = directory / f"initial-{index:03}.raw"
    source.write_bytes(image.blob)
    initial.write_bytes(target if target is not None else bytes([0xA5]) * size)
    expected.write_bytes(initial.read_bytes())
    progress: list[tuple[str, int]] = []
    result: dict[str, object] = {
        "label": label, "source": str(source), "initial": str(initial),
        "expected": str(expected), "target_size": size, "options": options,
    }
    try:
        ffu._apply(source, expected, size, True, lambda phase, percent: progress.append((phase, percent)), **options)
        result["status"] = 0
    except ffu.FfuError as error:
        result["status"] = error.exit_status
        result["error"] = str(error)
    result["progress"] = progress
    result["source_sha256_after"] = hashlib.sha256(source.read_bytes()).hexdigest()
    writer_cases.append(result)


write_case("exact final extents")
write_case("sector/non-chunk tail", size=original["TARGET_SIZE"] + 512)
write_case("replicated start/end", Fixture(((2, ((0, 2), (2, 1))),), (0,) * 6))
write_case("larger fixed-layout target", Fixture(
    ((1, ((0, 0),)), (2, ((0, 2), (0, 6))), (1, ((0, 0),)), (1, ((0, 10),)), (1, ((0, 0),)))
), size=2 * 1024 * 1024)
write_case("full wipe + partial final chunk", size=ffu.WIPE_CHUNK_BYTES + 512, wipe_entire_drive=True)
write_case("full wipe and cleanup mutually exclusive", wipe_entire_drive=True, allow_gpt_cleanup=True)
write_case("changed expected hash", expected_sha256="0" * 64)
write_case("changed expected target", expected_target_id="-2088985291:687:16777240", wipe_entire_drive=True)
write_case("actual target geometry", target=b"\xA5" * (original["TARGET_SIZE"] + 512))
for location in (512, original["TARGET_SIZE"] - 512):
    target = bytearray(b"\xA5" * original["TARGET_SIZE"])
    target[location:location + 8] = b"EFI PART"
    write_case(f"default GPT refusal {location}", target=target)
path = directory / "gpt-initial.raw"
create_gpt(path)
gpt = path.read_bytes()
write_case("bounded valid GPT cleanup", size=len(gpt), target=gpt, allow_gpt_cleanup=True)
if shutil.which("sgdisk"):
    native_gpt = directory / "native-sgdisk.raw"
    with native_gpt.open("wb") as target:
        target.truncate(8 * 1024 * 1024)
    subprocess.run(
        ["sgdisk", "--clear", "--new=1:2048:0", "--typecode=1:0700", str(native_gpt)],
        capture_output=True, check=True, timeout=15,
    )
    write_case(
        "native sgdisk GPT on regular file", size=8 * 1024 * 1024,
        target=native_gpt.read_bytes(), allow_gpt_cleanup=True,
    )
for damage in ("primary-crc", "backup-crc", "table-crc", "missing-primary", "missing-backup", "old-minimum-header"):
    damaged = bytearray(gpt)
    if damage == "primary-crc":
        damaged[512 + 16] ^= 1
    elif damage == "backup-crc":
        damaged[-512 + 16] ^= 1
    elif damage == "table-crc":
        damaged[1024 + 16] ^= 1
    elif damage == "missing-primary":
        damaged[512:520] = b"\0" * 8
    elif damage == "missing-backup":
        damaged[-512:-504] = b"\0" * 8
    else:
        damaged[original["TARGET_SIZE"] - 512 : original["TARGET_SIZE"] - 504] = b"EFI PART"
    write_case(f"invalid GPT {damage}", size=len(gpt), target=bytes(damaged), allow_gpt_cleanup=True)
write_case("explicit wipe ignores malformed GPT", size=len(gpt), target=bytes(damaged), wipe_entire_drive=True)

hdmi = HdmiFixture()
for mode, custom, wipe in (
    ("official", "", False), ("720p60", "", False), ("1080p60", "", False),
    ("custom", "# Reviewed timing\nhdmi_group=2\nhdmi_mode=16\nhdmi_drive=1", False),
    ("720p60", "", True),
):
    write_case(f"HDMI {mode}, full wipe {wipe}", hdmi, hdmi.size, hdmi_mode=mode, hdmi_config=custom, wipe_entire_drive=wipe)
for damage in ("mbr", "fat-copy", "cycle", "missing", "duplicate", "oversized", "filtered", "short-chain"):
    damaged = HdmiFixture()
    if damage == "mbr":
        damaged.disk[450] = 0xEE
    elif damage == "fat-copy":
        damaged.disk[damaged.fat_start + 32 * 512 + 4] ^= 1
    elif damage in ("cycle", "short-chain"):
        for fat in (damaged.fat_start, damaged.fat_start + 32 * 512):
            struct.pack_into("<H", damaged.disk, fat + 4, 2 if damage == "cycle" else 0xFFFF)
    elif damage == "missing":
        damaged.disk[damaged.root_start] = 0xE5
    elif damage == "duplicate":
        damaged.disk[damaged.root_start + 32 : damaged.root_start + 64] = damaged.disk[damaged.root_start : damaged.root_start + 32]
    elif damage == "oversized":
        struct.pack_into("<I", damaged.disk, damaged.root_start + 28, 4097)
    elif damage == "filtered":
        damaged.disk[damaged.data_start : damaged.data_start + 6] = b"[all]\n"
    damaged.refresh()
    write_case(f"invalid HDMI {damage}", damaged, hdmi.size, hdmi_mode="720p60")

hdmi_cases = []
for mode, custom in (
    ("official", ""), ("720p60", ""), ("1080p60", ""),
    ("custom", "hdmi_group=1\nhdmi_mode=64\nhdmi_force_hotplug=0"),
    ("custom", "hdmi_group=2\nhdmi_mode=86\nhdmi_drive=2"),
    ("custom", "hdmi_mode=0004 # Comment\nhdmi_group=01"),
    ("custom", "hdmi_group=1\nhdmi_mode=4\n#" + "x" * (512 - len("hdmi_group=1\nhdmi_mode=4\n#"))),
):
    hdmi_cases.append({"mode": mode, "custom": custom, "expected": ffu.hdmi_options(mode, custom)})

identity_path = directory / "native-identity-fixture.raw"
identity_path.write_bytes(b"\xA5" * original["TARGET_SIZE"])
print(json.dumps({
    "parser": parser_cases, "writer": writer_cases, "hdmi": hdmi_cases,
    "profiles": {board: ffu.image_profile(board) for board in ("pi2-v1.1", "pi2-v1.2", "pi3-b")},
    "native_identity": {
        "path": str(identity_path), "target_id": ffu._target_id(os.stat(identity_path)),
        "source": writer_cases[0]["source"],
    },
}, sort_keys=True))
