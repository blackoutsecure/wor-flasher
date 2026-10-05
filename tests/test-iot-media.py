#!/usr/bin/env python3
"""Offline native media contracts; no Windows content, disks, mounts, or network.

Run with Python 3.9+: python3 -B tests/test-iot-media.py
TMPDIR can point at a private session directory to contain every temporary file.
The existing FFU fixtures exercise the real FFU parser and allowlist checks.
Only fixture provenance, archive-process output, and HTTPS responses are mocked.
Guard-removal mutations compile modified helper copies in memory, not on disk.
"""

from __future__ import annotations

import ast
import contextlib
import errno
import hashlib
import importlib.util
import io
import json
import os
import signal
import stat
import struct
import subprocess
import sys
import tempfile
import types
import unittest
import urllib.error
import urllib.request
from email.message import Message
from pathlib import Path
from typing import Any, Iterator, Optional
from unittest import mock


ROOT = Path(__file__).resolve().parents[1]
HELPER_PATH = ROOT / "src/lib/iot-media.py"
REAL_POPEN = subprocess.Popen


def load_module(name: str, path: Path, code: Optional[types.CodeType] = None) -> types.ModuleType:
    spec = importlib.util.spec_from_file_location(name, path)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"cannot load test module: {path}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    if code is None:
        spec.loader.exec_module(module)
    else:
        exec(code, module.__dict__)
    return module


media = load_module("iot_media_tests", HELPER_PATH)
fixtures = load_module("iot_media_ffu_fixtures", ROOT / "tests/test-iot-ffu.py")


class Response(io.BytesIO):
    def __init__(self, body: bytes, url: str, length: Optional[int] = None) -> None:
        super().__init__(body)
        self.url, self.status = url, 200
        self.headers = Message()
        self.headers["Content-Length"] = str(len(body) if length is None else length)

    def geturl(self) -> str:
        return self.url


class MediaTests(unittest.TestCase):
    helper = media

    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory(prefix="iot-media-tests-")
        self.addCleanup(self.temporary.cleanup)
        self.base = Path(self.temporary.name).resolve()
        self.cache = self.base / "cache with spaces"
        self.fixture = fixtures.Fixture()
        self.cabinet = (b"synthetic cabinet; not Windows" * 300)[:7000]
        header = bytearray(4096)
        header[:8] = b"\xd0\xcf\x11\xe0\xa1\xb1\x1a\xe1"
        struct.pack_into("<4H", header, 26, 4, 0xFFFE, 12, 6)
        struct.pack_into("<II", header, 44, 1, 1)
        struct.pack_into("<II", header, 68, 0xFFFFFFFE, 0)
        struct.pack_into("<109I", header, 76, 0, *([0xFFFFFFFF] * 108))
        fat = [0xFFFFFFFD, 0xFFFFFFFE, 4, 0xFFFFFFFF, 0xFFFFFFFE] + [0xFFFFFFFF] * 1019
        self.msi = (
            bytes(header) + struct.pack("<1024I", *fat) + bytes(4096)
            + self.cabinet[:4096] + bytes(4096) + self.cabinet[4096:].ljust(4096, b"\0")
        )
        self.iso = b"synthetic ISO; not Windows".ljust(2048, b"\0") + self.msi + b"end"
        self.url = "https://software-download.microsoft.com/download/synthetic.iso"
        info = self.helper.ffu.parse_ffu(io.BytesIO(self.fixture.blob))
        self.entry = {
            "id": "synthetic-iot-core", "format": "ffu-v1", "profile": "iot-core",
            "architecture": "arm32", "build": "17763.107", "os_version": info.os_version,
            "sha256": info.sha256, "ffu_bytes": info.file_bytes,
            "payload_bytes": info.payload_bytes, "minimum_disk_bytes": info.minimum_disk_bytes,
            "sector_size": info.sector_size, "chunk_size": info.chunk_size,
            "platform_ids": list(info.platform_ids), "recommended_boards": ["pi2-v1.1"],
            "source_url": self.url, "resolved_source_url": self.url,
            "source_sha256": hashlib.sha256(self.iso).hexdigest(), "source_bytes": len(self.iso),
            "source_msi": "Windows_10_IoT_Core_for_RPi.msi",
            "source_msi_sha256": hashlib.sha256(self.msi).hexdigest(), "source_msi_bytes": len(self.msi),
            "source_msi_offset": 2048,
            "source_cab": "92c8cb0845792251b644366cf48a802a.cab",
            "source_cab_sha256": hashlib.sha256(self.cabinet).hexdigest(),
            "source_cab_bytes": len(self.cabinet), "source_cab_start_sector": 2,
            "source_cab_member": "fil2dda957cfdb079d52c8f284117ff4a0b",
        }
        self.manifest = {"schema_version": 1, "images": [self.entry]}
        self.original_provenance = self.helper.ffu.load_provenance
        provenance = mock.patch.object(self.helper.ffu, "load_provenance", return_value=self.manifest)
        provenance.start()
        self.addCleanup(provenance.stop)
        tools = mock.patch.object(self.helper.shutil, "which", return_value="/synthetic/cabextract")
        tools.start()
        self.addCleanup(tools.stop)
        self.log = io.StringIO()
        redirect = contextlib.redirect_stderr(self.log)
        redirect.__enter__()
        self.addCleanup(redirect.__exit__, None, None, None)
        self.calls: list[list[str]] = []
        self.children: list[subprocess.Popen[bytes]] = []

    def write(self, name: str, content: bytes) -> Path:
        path = self.base / name
        path.write_bytes(content)
        path.chmod(0o600)
        return path

    def cached_path(self) -> Path:
        return self.cache / f"iot-core-{self.entry['sha256']}.ffu"

    def seed_cache(self, content: Optional[bytes] = None) -> Path:
        self.cache.mkdir(mode=0o700)
        path = self.cached_path()
        path.write_bytes(self.fixture.blob if content is None else content)
        path.chmod(0o600)
        return path

    def assert_clean(self) -> None:
        self.assertEqual(list(self.cache.iterdir()) if self.cache.exists() else [], [])

    @contextlib.contextmanager
    def extractor(
        self, content: Optional[bytes] = None, diagnostic: bytes = b"",
        code: int = 0, pause: float = 0, before: Any = None,
    ) -> Iterator[None]:
        payload = self.fixture.blob if content is None else content
        script = (
            "import sys,time;"
            f"sys.stdout.buffer.write({payload!r});sys.stdout.buffer.flush();"
            f"sys.stderr.buffer.write({diagnostic!r});sys.stderr.buffer.flush();"
            f"time.sleep({pause});sys.exit({code})"
        )

        def spawn(command: list[str], **kwargs: Any) -> subprocess.Popen[bytes]:
            self.calls.append(command)
            self.assertEqual(command[1:5], ["--quiet", "--pipe", "--single", "--filter"])
            self.assertEqual(command[5], self.entry["source_cab_member"])
            self.assertNotIn("shell", kwargs)
            self.assertEqual(len(kwargs["pass_fds"]), 1)
            self.assertEqual(command[-1], f"/dev/fd/{kwargs['pass_fds'][0]}")
            self.assertTrue(stat.S_ISREG(os.fstat(kwargs["pass_fds"][0]).st_mode))
            self.assertEqual(stat.S_IMODE(os.fstat(kwargs["pass_fds"][0]).st_mode), 0o400)
            self.assertEqual(os.pread(kwargs["pass_fds"][0], len(self.cabinet), 0), self.cabinet)
            self.assertEqual(stat.S_IMODE(Path(kwargs["cwd"]).stat().st_mode), 0o700)
            if before is not None:
                before(command, kwargs)
            process = REAL_POPEN([sys.executable, "-B", "-c", script], **kwargs)
            self.children.append(process)
            return process

        with mock.patch.object(self.helper.subprocess, "Popen", side_effect=spawn):
            yield

    @contextlib.contextmanager
    def network(
        self, response: Optional[Response] = None, redirect: Optional[str] = None,
    ) -> Iterator[mock.Mock]:
        value = response if response is not None else Response(self.iso, self.url)
        calls = mock.Mock()

        def build(*handlers: Any) -> mock.Mock:
            handler = next(item for item in handlers if isinstance(item, urllib.request.HTTPRedirectHandler))

            def opened(request: urllib.request.Request, timeout: int) -> Response:
                self.assertEqual(timeout, self.helper.NETWORK_TIMEOUT)
                self.assertEqual(request.full_url, self.url)
                calls.request(request)
                if redirect is not None:
                    followed = handler.redirect_request(request, None, 302, "Found", Message(), redirect)
                    calls.redirect(followed)
                    value.url = redirect
                return value

            return mock.Mock(open=opened)

        with mock.patch.object(self.helper.urllib.request, "build_opener", side_effect=build):
            yield calls

    def check_result(self, result: dict[str, object], mode: str) -> Path:
        path = Path(result["source_file"])
        self.assertTrue(path.is_absolute())
        self.assertEqual(result["acquisition"], mode)
        self.assertEqual(result["sha256"], self.entry["sha256"])
        self.assertEqual(result["architecture"], "arm32")
        self.assertEqual(result["trust"], "pinned-official-sha256")
        self.assertEqual(hashlib.sha256(path.read_bytes()).hexdigest(), self.entry["sha256"])
        self.assertEqual(result["file_bytes"], len(self.fixture.blob))
        self.assertIn("0%", self.log.getvalue())
        self.assertIn("100%", self.log.getvalue())
        return path

    def test_local_ffu_uses_real_full_verifier_without_cache_tools_or_network(self) -> None:
        path = self.write("approved image with spaces.FFU", self.fixture.blob)
        with mock.patch.object(self.helper.urllib.request, "build_opener") as network, \
                mock.patch.object(self.helper.subprocess, "Popen") as extraction:
            result = self.helper.prepare(path, self.cache)
        self.assertEqual(self.check_result(result, "local-ffu"), path)
        self.assertFalse(self.cache.exists())
        network.assert_not_called()
        extraction.assert_not_called()

    def test_local_iso_and_msi_have_the_same_fully_approved_result(self) -> None:
        for suffix, payload in (("iso", self.iso), ("msi", self.msi)):
            with self.subTest(suffix=suffix), self.extractor():
                cache = self.base / ("cache " + suffix)
                path = self.write("official file with spaces." + suffix, payload)
                result = self.helper.prepare(path, cache)
                final = self.check_result(result, "local-" + suffix)
                self.assertEqual(final.parent, cache)
                self.assertEqual(stat.S_IMODE(final.stat().st_mode), 0o600)
                self.assertEqual(final.stat().st_nlink, 1)
                self.assertEqual(list(cache.iterdir()), [final])
                self.assertEqual(path.read_bytes(), payload)

    def test_download_uses_same_real_ffu_verification(self) -> None:
        with self.network() as network, self.extractor():
            result = self.helper.download(self.cache)
        self.check_result(result, "download")
        network.request.assert_called_once()
        self.assertEqual(list(self.cache.iterdir()), [self.cached_path()])

    def test_bad_package_digest_precedes_any_parser(self) -> None:
        source = self.write("changed.iso", self.iso[:-1] + b"x")
        with self.extractor(), self.assertRaisesRegex(self.helper.MediaError, "SHA256"):
            self.helper.prepare(source, self.cache)
        self.assertEqual(self.calls, [])
        self.assert_clean()

    def test_wrong_package_size_precedes_parser(self) -> None:
        for payload in (self.iso[:-1], self.iso + b"x"):
            with self.subTest(size=len(payload)), self.extractor():
                source = self.write("wrong length.iso", payload)
                with self.assertRaisesRegex(self.helper.MediaError, "size"):
                    self.helper.prepare(source, self.cache)
        self.assertEqual(self.calls, [])
        self.assert_clean()

    def test_msi_cannot_use_the_iso_digest(self) -> None:
        source = self.write("iso renamed.msi", self.iso)
        with self.extractor(), self.assertRaisesRegex(self.helper.MediaError, "size"):
            self.helper.prepare(source, self.cache)
        self.assertEqual(self.calls, [])
        self.assert_clean()

    def test_cabinet_digest_is_verified_before_external_parser(self) -> None:
        source = self.write("input.iso", self.iso)
        self.entry["source_cab_sha256"] = "0" * 64
        with self.extractor(), self.assertRaisesRegex(self.helper.MediaError, "SHA256"):
            self.helper.prepare(source, self.cache)
        self.assertEqual(self.calls, [])
        self.assert_clean()

    def test_compound_storage_bounds_and_cycles_are_rejected(self) -> None:
        variants = (
            (26, "<H", 3),
            (44, "<I", 1000000),
            (68, "<I", 999999),
            (72, "<I", 5),
            (76, "<I", 999999),
            (4096, "<I", 0xFFFFFFFF),
            (4096 + 2 * 4, "<I", 999999),
            (4096 + 2 * 4, "<I", 2),
            (4096 + 2 * 4, "<I", 0xFFFFFFFE),
            (4096 + 4 * 4, "<I", 2),
        )
        for offset, encoding, value in variants:
            blob = bytearray(self.msi)
            struct.pack_into(encoding, blob, offset, value)
            self.entry["source_msi_sha256"] = hashlib.sha256(blob).hexdigest()
            source = self.write("synthetic malformed.msi", blob)
            with self.subTest(offset=offset, value=value), self.extractor():
                with self.assertRaises(self.helper.MediaError):
                    self.helper.prepare(source, self.cache)
            self.assertEqual(self.calls, [])
            self.assert_clean()

    def test_pinned_msi_extent_cannot_escape_verified_iso(self) -> None:
        self.entry["source_msi_offset"] = len(self.iso)
        source = self.write("input.iso", self.iso)
        with self.extractor(), self.assertRaisesRegex(self.helper.MediaError, "MSI extent"):
            self.helper.prepare(source, self.cache)
        self.assertEqual(self.calls, [])
        self.assert_clean()

    def test_cabinet_start_or_length_cannot_escape_msi(self) -> None:
        for key, value in (("source_cab_start_sector", 9999), ("source_cab_bytes", len(self.msi) + 1)):
            previous = self.entry[key]
            self.entry[key] = value
            source = self.write("input.msi", self.msi)
            with self.subTest(key=key), self.extractor(), self.assertRaises(self.helper.MediaError):
                self.helper.prepare(source, self.cache)
            self.entry[key] = previous
            self.assertEqual(self.calls, [])
            self.assert_clean()

    def test_changed_source_during_copy_is_rejected(self) -> None:
        source = self.write("mutable.iso", self.iso)
        copy = self.helper._copy_package

        def change(*args: Any) -> None:
            copy(*args)
            source.write_bytes(b"x" * len(self.iso))

        with mock.patch.object(self.helper, "_copy_package", side_effect=change), self.extractor():
            with self.assertRaisesRegex(self.helper.AcquisitionError, "changed"):
                self.helper.prepare(source, self.cache)
        self.assertEqual(self.calls, [])
        self.assert_clean()

    def test_original_path_replacement_during_parser_is_rejected(self) -> None:
        source = self.write("replaced.iso", self.iso)

        def change(command: list[str], kwargs: Any) -> None:
            source.rename(self.base / "original held source.iso")
            source.write_bytes(self.iso)
            self.assertEqual(os.pread(kwargs["pass_fds"][0], len(self.cabinet), 0), self.cabinet)

        with self.extractor(before=change), self.assertRaisesRegex(self.helper.AcquisitionError, "changed"):
            self.helper.prepare(source, self.cache)
        self.assert_clean()

    def test_wrapper_source_inode_is_not_the_parser_snapshot(self) -> None:
        source = self.write("input.iso", self.iso)
        original = source.stat()

        def check(command: list[str], kwargs: Any) -> None:
            snapshot = os.fstat(kwargs["pass_fds"][0])
            self.assertNotEqual((original.st_dev, original.st_ino), (snapshot.st_dev, snapshot.st_ino))
            self.assertEqual(os.pread(kwargs["pass_fds"][0], len(self.cabinet), 0), self.cabinet)

        with self.extractor(before=check):
            self.helper.prepare(source, self.cache)

    def test_changed_snapshot_is_rejected(self) -> None:
        source = self.write("input.iso", self.iso)

        def change(command: list[str], kwargs: Any) -> None:
            os.pwrite(kwargs["pass_fds"][0], b"x", 0)

        with self.extractor(before=change), self.assertRaisesRegex(self.helper.AcquisitionError, "snapshot changed"):
            self.helper.prepare(source, self.cache)
        self.assert_clean()

    def test_extraction_output_limit_is_enforced(self) -> None:
        source = self.write("input.iso", self.iso)
        with self.extractor(self.fixture.blob + b"x"), self.assertRaisesRegex(self.helper.AcquisitionError, "exceeds"):
            self.helper.prepare(source, self.cache)
        self.assert_clean()
        self.assertTrue(all(child.poll() is not None for child in self.children))

    def test_extraction_truncation_is_rejected(self) -> None:
        source = self.write("input.iso", self.iso)
        with self.extractor(self.fixture.blob[:-1]), self.assertRaisesRegex(self.helper.AcquisitionError, "truncated"):
            self.helper.prepare(source, self.cache)
        self.assert_clean()

    def test_extraction_diagnostic_limit_is_enforced(self) -> None:
        source = self.write("input.iso", self.iso)
        with self.extractor(diagnostic=b"x" * 256), \
                mock.patch.object(self.helper, "MAX_DIAGNOSTIC_BYTES", 128):
            with self.assertRaisesRegex(self.helper.AcquisitionError, "diagnostic output exceeds"):
                self.helper.prepare(source, self.cache)
        self.assert_clean()

    def test_extraction_failure_is_explicit_and_clean(self) -> None:
        source = self.write("input.iso", self.iso)
        with self.extractor(diagnostic=b"corrupt cabinet", code=2):
            with self.assertRaisesRegex(self.helper.AcquisitionError, "corrupt cabinet"):
                self.helper.prepare(source, self.cache)
        self.assert_clean()

    def test_extraction_timeout_terminates_child_and_cleans(self) -> None:
        source = self.write("input.iso", self.iso)
        with self.extractor(pause=15), mock.patch.object(self.helper, "EXTRACT_TIMEOUT", 0.05):
            with self.assertRaisesRegex(self.helper.AcquisitionError, "timed out"):
                self.helper.prepare(source, self.cache)
        self.assert_clean()
        self.assertTrue(all(child.poll() is not None for child in self.children))

    def test_nonapproved_extracted_ffu_cannot_be_published(self) -> None:
        source = self.write("input.iso", self.iso)
        blob = bytearray(self.fixture.blob)
        blob[self.fixture.payload_offset + 17] ^= 1
        with self.extractor(bytes(blob)), self.assertRaises(self.helper.ffu.FfuError):
            self.helper.prepare(source, self.cache)
        self.assert_clean()

    def test_self_consistent_but_not_allowlisted_ffu_is_rejected(self) -> None:
        blob = bytearray(self.fixture.blob)
        blob[self.fixture.payload_offset + 17] ^= 1
        source = self.write("self consistent.ffu", self.fixture.rehash(blob))
        with self.assertRaisesRegex(self.helper.ffu.FfuError, "allowlisted"):
            self.helper.prepare(source, self.cache)
        self.assertFalse(self.cache.exists())

    def test_arm64_manifest_cannot_change_backend_authorization(self) -> None:
        self.entry["architecture"] = "arm64"
        source = self.write("unapproved.ffu", self.fixture.blob)
        with self.assertRaisesRegex(self.helper.ffu.FfuError, "ARM32"):
            self.helper.prepare(source, self.cache)

    def test_public_entrypoint_uses_digest_pinned_provenance_loader(self) -> None:
        source = self.write("input.iso", self.iso)
        with mock.patch.object(self.helper.ffu, "load_provenance", side_effect=self.original_provenance), \
                mock.patch.object(self.helper.ffu, "PROVENANCE_SHA256", "0" * 64), self.extractor():
            with self.assertRaisesRegex(self.helper.ffu.FfuError, "provenance"):
                self.helper.prepare(source, self.cache)
        self.assertEqual(self.calls, [])
        self.assertFalse(self.cache.exists())

    def test_cache_is_fully_verified_without_network_or_tools(self) -> None:
        self.seed_cache()
        with mock.patch.object(self.helper.urllib.request, "build_opener") as network, \
                mock.patch.object(self.helper.shutil, "which", return_value=None), \
                mock.patch.object(self.helper.ffu, "inspect_ffu", wraps=self.helper.ffu.inspect_ffu) as inspect:
            result = self.helper.download(self.cache)
        self.check_result(result, "cache")
        inspect.assert_called_once()
        network.assert_not_called()

    def test_local_wrapper_must_match_even_when_cache_is_valid(self) -> None:
        cached = self.seed_cache()
        source = self.write("wrong.iso", b"x" * len(self.iso))
        with self.extractor(), self.assertRaisesRegex(self.helper.MediaError, "SHA256"):
            self.helper.prepare(source, self.cache)
        self.assertEqual(self.calls, [])
        self.assertEqual(list(self.cache.iterdir()), [cached])
        self.assertEqual(cached.read_bytes(), self.fixture.blob)

    def test_local_wrapper_can_reuse_verified_cache_without_tools(self) -> None:
        self.seed_cache()
        source = self.write("valid.iso", self.iso)
        with mock.patch.object(self.helper.shutil, "which", return_value=None):
            result = self.helper.prepare(source, self.cache)
        self.check_result(result, "cache")
        self.assertEqual(list(self.cache.iterdir()), [self.cached_path()])

    def test_corrupt_cache_is_refused_without_download_or_overwrite(self) -> None:
        blob = bytearray(self.fixture.blob)
        blob[self.fixture.payload_offset + 11] ^= 1
        cached = self.seed_cache(bytes(blob))
        with self.network() as network, self.extractor():
            with self.assertRaisesRegex(self.helper.MediaError, "cached FFU failed verification"):
                self.helper.download(self.cache)
        network.request.assert_not_called()
        self.assertEqual(self.calls, [])
        self.assertEqual(cached.read_bytes(), bytes(blob))
        self.assertEqual(list(self.cache.iterdir()), [cached])

    def test_source_symlink_and_fifo_are_refused_without_opening_them(self) -> None:
        original = self.write("regular.iso", self.iso)
        for name in ("link.iso", "fifo.iso"):
            source = self.base / name
            if name.startswith("link"):
                source.symlink_to(original)
            else:
                os.mkfifo(source, 0o600)
            with self.subTest(name=name), self.assertRaisesRegex(self.helper.MediaError, "regular file"):
                self.helper.prepare(source, self.cache)
        self.assertFalse(self.cache.exists())

    def test_local_ffu_symlink_is_not_canonicalized_into_authorization(self) -> None:
        source = self.write("image.ffu", self.fixture.blob)
        link = self.base / "symlink.ffu"
        link.symlink_to(source)
        with self.assertRaises(self.helper.ffu.FfuError):
            self.helper.prepare(link, self.cache)

    def test_source_symlink_parent_is_refused(self) -> None:
        folder = self.base / "real source"
        folder.mkdir()
        (folder / "input.iso").write_bytes(self.iso)
        link = self.base / "source link"
        link.symlink_to(folder, target_is_directory=True)
        with self.assertRaisesRegex(self.helper.MediaError, "symlink"):
            self.helper.prepare(link / "input.iso", self.cache)
        self.assertFalse(self.cache.exists())

    def test_cache_symlink_parent_is_refused_without_touching_destination(self) -> None:
        destination = self.base / "outside"
        destination.mkdir()
        link = self.base / "cache link"
        link.symlink_to(destination, target_is_directory=True)
        with self.assertRaisesRegex(self.helper.MediaError, "symlink"):
            self.helper.download(link / "child")
        self.assertEqual(list(destination.iterdir()), [])

    def test_only_trusted_macos_system_cache_aliases_are_canonicalized(self) -> None:
        value = self.base.stat()
        fields = {
            name: getattr(value, name) for name in (
                "st_mode", "st_uid", "st_gid", "st_dev", "st_ino", "st_size",
                "st_mtime_ns", "st_ctime_ns", "st_nlink",
            )
        }
        fields.update(st_mode=stat.S_IFLNK | 0o777, st_uid=0)
        link = types.SimpleNamespace(**fields)
        with mock.patch.object(self.helper.sys, "platform", "darwin"), \
                mock.patch.object(Path, "lstat", return_value=link):
            for alias in ("tmp", "var"):
                for target in (f"private/{alias}", f"/private/{alias}"):
                    with self.subTest(target=target), mock.patch.object(self.helper.os, "readlink", return_value=target):
                        self.assertEqual(
                            self.helper._cache_path(Path(f"/{alias}/private cache/iot-core")),
                            Path(f"/private/{alias}/private cache/iot-core"),
                        )
            with mock.patch.object(self.helper.os, "readlink") as readlink:
                self.assertEqual(self.helper._cache_path(self.cache), self.cache)
                readlink.assert_not_called()
            with mock.patch.object(self.helper.sys, "platform", "linux"), \
                    mock.patch.object(self.helper.os, "readlink") as readlink:
                self.assertEqual(self.helper._cache_path(Path("/tmp/cache")), Path("/tmp/cache"))
                readlink.assert_not_called()

    def test_macos_alias_owner_target_and_identity_are_checked(self) -> None:
        value = self.base.stat()
        fields = {
            name: getattr(value, name) for name in (
                "st_mode", "st_uid", "st_gid", "st_dev", "st_ino", "st_size",
                "st_mtime_ns", "st_ctime_ns", "st_nlink",
            )
        }
        fields.update(st_mode=stat.S_IFLNK | 0o777, st_uid=0)
        original = types.SimpleNamespace(**fields)
        variants = (
            (types.SimpleNamespace(**dict(fields, st_uid=99999)), "private/tmp", original),
            (original, "/Users/someone/cache", original),
            (original, "private/tmp", types.SimpleNamespace(**dict(fields, st_ino=value.st_ino + 1))),
        )
        for first, target, second in variants:
            with self.subTest(target=target, uid=first.st_uid), \
                    mock.patch.object(self.helper.sys, "platform", "darwin"), \
                    mock.patch.object(Path, "lstat", side_effect=[first, second]), \
                    mock.patch.object(self.helper.os, "readlink", return_value=target):
                with self.assertRaisesRegex(self.helper.MediaError, "unsafe or changed"):
                    self.helper._cache_path(Path("/tmp/cache"))

    def test_canonical_cache_path_is_returned_for_the_elevated_writer(self) -> None:
        source = self.write("input.iso", self.iso)
        requested = Path("/tmp/requested cache")
        with mock.patch.object(self.helper, "_cache_path", return_value=self.cache) as canonical, self.extractor():
            result = self.helper.prepare(source, requested)
        canonical.assert_called_once_with(requested)
        final = self.check_result(result, "local-iso")
        self.assertEqual(final, self.cached_path())
        self.assertEqual(final.resolve(), final)
        self.assertEqual(stat.S_IMODE(final.stat().st_mode), 0o600)
        self.assertEqual(stat.S_IMODE(self.cache.stat().st_mode), 0o700)

    def test_cache_output_symlink_never_changes_target(self) -> None:
        original = self.write("outside.ffu", self.fixture.blob)
        self.cache.mkdir(mode=0o700)
        self.cached_path().symlink_to(original)
        with self.assertRaisesRegex(self.helper.MediaError, "regular file"):
            self.helper.download(self.cache)
        self.assertEqual(original.read_bytes(), self.fixture.blob)
        self.assertTrue(self.cached_path().is_symlink())

    def test_hardlinked_or_public_cache_file_is_rejected(self) -> None:
        cached = self.seed_cache()
        cached.chmod(0o644)
        with self.assertRaisesRegex(self.helper.MediaError, "private"):
            self.helper.download(self.cache)
        cached.chmod(0o600)
        os.link(cached, self.base / "alias.ffu")
        with self.assertRaisesRegex(self.helper.MediaError, "single-link"):
            self.helper.download(self.cache)

    def test_writable_cache_or_ancestor_is_refused(self) -> None:
        self.cache.mkdir(mode=0o700)
        self.cache.chmod(0o777)
        for path in (self.cache, self.cache / "new child"):
            with self.subTest(path=path), self.assertRaisesRegex(self.helper.MediaError, "writable"):
                self.helper.download(path)
        self.assertEqual(list(self.cache.iterdir()), [])

    def test_wrong_cache_owner_is_refused(self) -> None:
        self.seed_cache()
        with mock.patch.object(self.helper.os, "geteuid", return_value=999999):
            with self.assertRaisesRegex(self.helper.MediaError, "owner"):
                self.helper.download(self.cache)

    def test_parent_traversal_is_refused(self) -> None:
        with self.assertRaisesRegex(self.helper.MediaError, "traversal"):
            self.helper.download(self.base / ".." / "unwanted cache")

    def test_missing_dependency_is_explicit_and_never_downloads(self) -> None:
        with mock.patch.object(self.helper.shutil, "which", return_value=None), self.network() as network:
            with self.assertRaisesRegex(self.helper.AcquisitionError, "cabextract is required"):
                self.helper.download(self.cache)
        network.request.assert_not_called()
        self.assert_clean()

    def test_missing_dependency_for_local_import_cleans_snapshot(self) -> None:
        source = self.write("input.msi", self.msi)
        with mock.patch.object(self.helper.shutil, "which", return_value=None):
            with self.assertRaisesRegex(self.helper.AcquisitionError, "cabextract is required"):
                self.helper.prepare(source, self.cache)
        self.assert_clean()

    def test_missing_executable_after_lookup_cleans_snapshot(self) -> None:
        source = self.write("input.iso", self.iso)
        with mock.patch.object(self.helper.subprocess, "Popen", side_effect=FileNotFoundError("cabextract disappeared")):
            with self.assertRaises(FileNotFoundError):
                self.helper.prepare(source, self.cache)
        self.assert_clean()

    def test_disk_full_during_snapshot_has_no_success_file(self) -> None:
        source = self.write("input.iso", self.iso)
        with mock.patch.object(self.helper.os, "fsync", side_effect=OSError(errno.ENOSPC, "disk full")):
            with self.assertRaises(OSError):
                self.helper.prepare(source, self.cache)
        self.assert_clean()

    def test_atomic_publication_only_follows_full_inspection(self) -> None:
        source = self.write("input.iso", self.iso)
        link = self.helper.os.link
        inspected: list[Path] = []
        inspect = self.helper.ffu.inspect_ffu

        def validate(path: Path) -> dict[str, object]:
            result = inspect(path)
            inspected.append(path)
            return result

        def publish(*args: Any, **kwargs: Any) -> None:
            self.assertEqual(len(inspected), 1)
            self.assertFalse(self.cached_path().exists())
            self.assertEqual(inspected[0].name, "image.ffu")
            link(*args, **kwargs)

        with self.extractor(), mock.patch.object(self.helper.ffu, "inspect_ffu", side_effect=validate), \
                mock.patch.object(self.helper.os, "link", side_effect=publish):
            result = self.helper.prepare(source, self.cache)
        self.check_result(result, "local-iso")
        self.assertEqual(inspected[-1], self.cached_path())
        self.assertEqual(len(inspected), 2)

    def test_atomic_publish_failure_cleans_only_own_files(self) -> None:
        source = self.write("input.iso", self.iso)
        self.cache.mkdir(mode=0o700)
        unrelated = self.cache / "unrelated.txt"
        unrelated.write_text("must survive")
        with self.extractor(), mock.patch.object(self.helper.os, "link", side_effect=OSError(errno.ENOSPC, "disk full")):
            with self.assertRaises(OSError):
                self.helper.prepare(source, self.cache)
        self.assertEqual(list(self.cache.iterdir()), [unrelated])
        self.assertEqual(unrelated.read_text(), "must survive")

    def test_directory_sync_failure_rolls_back_own_publication(self) -> None:
        source = self.write("input.iso", self.iso)
        fsync = self.helper.os.fsync

        def sync(fd: int) -> None:
            if stat.S_ISDIR(os.fstat(fd).st_mode):
                raise OSError(errno.EIO, "directory sync failed")
            fsync(fd)

        with self.extractor(), mock.patch.object(self.helper.os, "fsync", side_effect=sync):
            with self.assertRaisesRegex(OSError, "directory sync failed"):
                self.helper.prepare(source, self.cache)
        self.assert_clean()

    def test_mutation_at_publication_cannot_return_stale_inspection(self) -> None:
        source = self.write("input.iso", self.iso)
        link = self.helper.os.link

        def corrupt(*args: Any, **kwargs: Any) -> None:
            fd = os.open("image.ffu", os.O_WRONLY, dir_fd=kwargs["src_dir_fd"])
            try:
                os.write(fd, b"bad!")
            finally:
                os.close(fd)
            link(*args, **kwargs)

        with self.extractor(), mock.patch.object(self.helper.os, "link", side_effect=corrupt):
            with self.assertRaises(self.helper.ffu.FfuError):
                self.helper.prepare(source, self.cache)
        self.assert_clean()

    def test_competing_valid_cache_is_verified_and_not_overwritten(self) -> None:
        source = self.write("input.iso", self.iso)

        def competing(*args: Any, **kwargs: Any) -> None:
            self.cached_path().write_bytes(self.fixture.blob)
            self.cached_path().chmod(0o600)
            raise FileExistsError(errno.EEXIST, "competing publisher")

        with self.extractor(), mock.patch.object(self.helper.os, "link", side_effect=competing):
            result = self.helper.prepare(source, self.cache)
        self.check_result(result, "cache")
        self.assertEqual(list(self.cache.iterdir()), [self.cached_path()])

    def test_http_redirect_is_rejected_before_following(self) -> None:
        with self.network(redirect="http://software-download.microsoft.com/downgrade") as network, self.extractor():
            with self.assertRaisesRegex(self.helper.MediaError, "HTTPS"):
                self.helper.download(self.cache)
        network.redirect.assert_not_called()
        self.assertEqual(self.calls, [])
        self.assert_clean()

    def test_unapproved_redirect_hosts_are_rejected_before_following(self) -> None:
        for url in (
            "https://example.org/image.iso",
            "https://software-download.microsoft.com.evil.invalid/image.iso",
            "https://software-download.microsoft.com@evil.invalid/image.iso",
            "https://software-download.microsoft.com:8443/image.iso",
        ):
            with self.subTest(url=url), self.network(redirect=url) as network, self.extractor():
                with self.assertRaisesRegex(self.helper.MediaError, "HTTPS"):
                    self.helper.download(self.cache)
                network.redirect.assert_not_called()
        self.assertEqual(self.calls, [])
        self.assert_clean()

    def test_approved_https_redirect_is_allowed(self) -> None:
        with self.network(redirect="https://go.microsoft.com/fwlink/?LinkId=846058") as network, self.extractor():
            result = self.helper.download(self.cache)
        self.check_result(result, "download")
        network.redirect.assert_called_once()

    def test_final_response_host_is_revalidated(self) -> None:
        with self.network(Response(self.iso, "https://example.invalid/image.iso")), self.extractor():
            with self.assertRaisesRegex(self.helper.MediaError, "HTTPS"):
                self.helper.download(self.cache)
        self.assertEqual(self.calls, [])
        self.assert_clean()

    def test_untrusted_initial_url_never_reaches_opener(self) -> None:
        self.entry["resolved_source_url"] = "http://software-download.microsoft.com/input.iso"
        with self.network() as network, self.assertRaisesRegex(self.helper.MediaError, "HTTPS"):
            self.helper.download(self.cache)
        network.request.assert_not_called()
        self.assertFalse(self.cache.exists())

    def test_truncated_download_is_rejected_before_extraction(self) -> None:
        response = Response(self.iso[:-1], self.url, len(self.iso))
        with self.network(response), self.extractor():
            with self.assertRaisesRegex(self.helper.MediaError, "truncated"):
                self.helper.download(self.cache)
        self.assertEqual(self.calls, [])
        self.assert_clean()

    def test_oversized_response_body_is_rejected_before_extraction(self) -> None:
        response = Response(self.iso + b"x", self.url, len(self.iso))
        with self.network(response), self.extractor():
            with self.assertRaisesRegex(self.helper.MediaError, "exceeds"):
                self.helper.download(self.cache)
        self.assertEqual(self.calls, [])
        self.assert_clean()

    def test_bad_download_digest_is_rejected_before_extraction(self) -> None:
        response = Response(b"x" * len(self.iso), self.url)
        with self.network(response), self.extractor():
            with self.assertRaisesRegex(self.helper.MediaError, "SHA256"):
                self.helper.download(self.cache)
        self.assertEqual(self.calls, [])
        self.assert_clean()

    def test_bad_absent_duplicate_or_excessive_content_length_is_refused(self) -> None:
        for lengths in ([], ["1"], [str(len(self.iso) + 1)], ["x"], [str(len(self.iso))] * 2):
            response = Response(self.iso, self.url)
            del response.headers["Content-Length"]
            for value in lengths:
                response.headers["Content-Length"] = value
            with self.subTest(lengths=lengths), self.network(response), self.extractor():
                with self.assertRaisesRegex(self.helper.MediaError, "Content-Length"):
                    self.helper.download(self.cache)
        self.assertEqual(self.calls, [])
        self.assert_clean()

    def test_encoding_or_partial_content_response_is_refused(self) -> None:
        for field, value in (("Content-Encoding", "gzip"), ("Transfer-Encoding", "chunked"), ("status", "206")):
            response = Response(self.iso, self.url)
            if field == "status":
                response.status = int(value)
            else:
                response.headers[field] = value
            with self.subTest(field=field), self.network(response), self.extractor():
                with self.assertRaises(self.helper.MediaError):
                    self.helper.download(self.cache)
        self.assertEqual(self.calls, [])
        self.assert_clean()

    def test_network_error_reports_failure_without_success_json(self) -> None:
        output = io.StringIO()
        opener = mock.Mock()
        opener.open.side_effect = urllib.error.URLError("TLS verification failed")
        with mock.patch.object(self.helper.urllib.request, "build_opener", return_value=opener), \
                contextlib.redirect_stdout(output):
            code = self.helper.main(["download", "--cache-dir", str(self.cache), "--json"])
        self.assertEqual(code, 4)
        self.assertEqual(output.getvalue(), "")
        self.assertIn("TLS verification failed", self.log.getvalue())
        self.assert_clean()

    def test_download_total_deadline_is_enforced(self) -> None:
        with self.network(), mock.patch.object(self.helper, "DOWNLOAD_TIMEOUT", 0):
            with self.assertRaisesRegex(self.helper.AcquisitionError, "timed out"):
                self.helper.download(self.cache)
        self.assert_clean()

    def test_download_does_not_wait_to_fill_each_chunk_before_checking_deadline(self) -> None:
        response = Response(self.iso, self.url)
        with mock.patch.object(response, "read", side_effect=AssertionError("unbounded repeated socket reads")), \
                self.network(response), self.extractor():
            self.check_result(self.helper.download(self.cache), "download")

    def test_output_disk_full_terminates_parser_and_removes_partial_files(self) -> None:
        source = self.write("input.iso", self.iso)
        original = self.helper._new_file

        @contextlib.contextmanager
        def new_file(directory: Any, name: str) -> Iterator[Any]:
            with original(directory, name) as stream:
                if name == "image.ffu":
                    proxy = mock.Mock(wraps=stream)
                    proxy.write.side_effect = OSError(errno.ENOSPC, "output disk full")
                    yield proxy
                else:
                    yield stream

        with mock.patch.object(self.helper, "_new_file", side_effect=new_file), self.extractor():
            with self.assertRaisesRegex(OSError, "output disk full"):
                self.helper.prepare(source, self.cache)
        self.assert_clean()
        self.assertTrue(all(child.poll() is not None for child in self.children))

    def test_cancellation_after_link_rolls_back_the_published_file(self) -> None:
        source = self.write("input.iso", self.iso)
        inspect = self.helper.ffu.inspect_ffu

        def interrupt(path: Path) -> dict[str, object]:
            if path == self.cached_path():
                os.kill(os.getpid(), signal.SIGTERM)
            return inspect(path)

        output = io.StringIO()
        with self.extractor(), mock.patch.object(self.helper.ffu, "inspect_ffu", side_effect=interrupt), \
                contextlib.redirect_stdout(output):
            code = self.helper.main(["prepare", str(source), "--cache-dir", str(self.cache), "--json"])
        self.assertEqual(code, 143)
        self.assertEqual(output.getvalue(), "")
        self.assert_clean()

    def test_exact_member_name_cannot_be_a_path_pattern_or_option(self) -> None:
        for member in ("*", "../flash.ffu", "/absolute", "-F", "file[0-9]", "file?"):
            self.entry["source_cab_member"] = member
            with self.subTest(member=member), self.network() as network:
                with self.assertRaisesRegex(self.helper.MediaError, "exact"):
                    self.helper.download(self.cache)
                network.request.assert_not_called()
        self.assertFalse(self.cache.exists())

    def test_cli_json_contains_exact_inspection_plus_acquisition_fields(self) -> None:
        source = self.write("input.ffu", self.fixture.blob)
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            code = self.helper.main(["prepare", str(source), "--cache-dir", str(self.cache), "--json"])
        self.assertEqual(code, 0)
        result = json.loads(output.getvalue())
        self.check_result(result, "local-ffu")
        expected = self.helper.ffu.inspect_ffu(source)
        self.assertEqual(set(result), set(expected) | {"source_file", "acquisition"})
        self.assertEqual({key: result[key] for key in expected}, expected)

    def test_cli_has_no_url_digest_or_trust_override(self) -> None:
        for flag in ("--url", "--sha256", "--trust-cache", "--skip-verification"):
            with self.subTest(flag=flag), self.assertRaises(SystemExit) as error:
                self.helper.main(["download", "--cache-dir", str(self.cache), flag, "ignored"])
            self.assertEqual(error.exception.code, 2)

    def test_environment_trust_cache_does_not_skip_full_validation(self) -> None:
        self.seed_cache(b"invalid")
        with mock.patch.dict(os.environ, {"TRUST_CACHE": "1", "VERIFY_TLS": "0", "IOT_PROVENANCE_SHA256": "0" * 64}):
            with self.assertRaisesRegex(self.helper.MediaError, "failed verification"):
                self.helper.download(self.cache)

    def test_sigint_and_sigterm_cleanup_child_snapshot_and_output(self) -> None:
        source = self.write("input.iso", self.iso)
        update = self.helper.Progress.update
        for signum in (signal.SIGINT, signal.SIGTERM):
            before = {sig: signal.getsignal(sig) for sig in (signal.SIGINT, signal.SIGTERM)}

            def interrupt(progress: Any, count: int, complete: bool = False) -> None:
                update(progress, count, complete)
                if progress.label.startswith("Extracting") and count > 0:
                    os.kill(os.getpid(), signum)

            output = io.StringIO()
            with self.subTest(signum=signum), self.extractor(pause=15), \
                    mock.patch.object(self.helper.Progress, "update", interrupt), contextlib.redirect_stdout(output):
                code = self.helper.main(["prepare", str(source), "--cache-dir", str(self.cache), "--json"])
            self.assertEqual(code, 128 + signum)
            self.assertEqual(output.getvalue(), "")
            self.assertEqual(before, {sig: signal.getsignal(sig) for sig in before})
            self.assertTrue(all(child.poll() is not None for child in self.children))
            self.assert_clean()


class MutationTests(unittest.TestCase):
    def mutant(self, name: str, function: str, symbol: str) -> types.ModuleType:
        tree = ast.parse(HELPER_PATH.read_text(), str(HELPER_PATH))
        removed = 0
        for node in ast.walk(tree):
            if isinstance(node, ast.FunctionDef) and node.name == function:
                for guard in ast.walk(node):
                    if isinstance(guard, ast.If) and any(
                        isinstance(item, ast.Attribute) and item.attr == symbol for item in ast.walk(guard.test)
                    ):
                        guard.test = ast.Constant(value=False)
                        removed += 1
        self.assertEqual(removed, 1)
        ast.fix_missing_locations(tree)
        return load_module(name, HELPER_PATH, compile(tree, str(HELPER_PATH), "exec"))

    def rejected(self, module: types.ModuleType, test: str) -> None:
        case = MediaTests(test)
        case.helper = module
        result = unittest.TestResult()
        case.run(result)
        self.assertFalse(result.errors, result.errors)
        self.assertEqual(len(result.failures), 1, "guard-removal mutant escaped its public-entrypoint test")
        self.assertIn("not raised", result.failures[0][1])

    def test_removing_package_digest_guard_is_detected(self) -> None:
        module = self.mutant("iot_media_no_digest", "_copy_chunks", "hexdigest")
        self.rejected(module, "test_bad_package_digest_precedes_any_parser")

    def test_removing_https_redirect_guard_is_detected(self) -> None:
        module = self.mutant("iot_media_no_https_guard", "_check_url", "scheme")
        self.rejected(module, "test_http_redirect_is_rejected_before_following")


if __name__ == "__main__":
    unittest.main()
