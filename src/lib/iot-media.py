#!/usr/bin/env python3
"""Acquire only the bundled, digest-pinned Raspberry Pi ARM32 IoT Core image.

    prepare SOURCE --cache-dir DIR --json
    download --cache-dir DIR --json

SOURCE is an approved local FFU, ISO, or MSI. Success returns inspect_ffu() fields
plus source_file (absolute FFU path) and acquisition: local-ffu, local-iso,
local-msi, download, or cache. Every cached FFU is fully inspected before reuse.
An invalid existing cache entry is refused, not silently trusted or overwritten.

Only download performs network I/O, using the pinned HTTPS Microsoft URL. This
does not execute Windows code, accept a EULA, mount media, or open storage devices.
The caller must obtain the user's request to download and explain license terms.
cabextract is the only external dependency for ISO/MSI imports and fresh downloads.
The reviewed MSI byte offset and compound-storage stream start come from bundled
provenance. A bounded sector-chain reader reassembles that one cabinet stream and
verifies its pinned size/SHA256 before cabextract sees it. No UDF, directory-name,
mini-stream, or general MSI interpretation is needed. bsdtar cannot read either
reviewed wrapper on macOS. Only the exact pinned FFU member is sent to stdout.

Wrapper size/digest checks precede archive parsing. A private copied snapshot and
its held descriptor prevent source-path replacement between verification and
parsing. /dev/fd here aliases that regular-file descriptor, never a physical disk.
Only a fully inspected FFU is atomically published, without replacing any existing
path. macOS's root-owned /tmp and /var aliases are mapped to their fixed /private
targets before cache traversal. All remaining cache components must have no
symlinks, unsafe owners, or writable non-sticky ancestors; cache files are
private, single-link, current-user files (0600, also readable by a root writer).

No trust override, arbitrary URL, new image/board support, or Windows dependency
is exposed. The sibling iot-ffu.py owns both provenance and FFU authorization.
Exit statuses: 0 success, 2 invalid/unapproved input or unsafe cache, 4 acquisition
or I/O failure, 130 SIGINT, 143 SIGTERM. Progress/diagnostics go only to stderr.
"""

from __future__ import annotations

import argparse
import fcntl
import hashlib
import http.client
import importlib.util
import os
import re
import secrets
import selectors
import shutil
import signal
import ssl
import stat
import struct
import subprocess
import sys
import time
import urllib.parse
import urllib.request
from contextlib import contextmanager
from dataclasses import dataclass
from pathlib import Path
from types import ModuleType
from typing import Any, BinaryIO, Callable, Iterable, Iterator, Optional, Sequence


COPY_BYTES = 1024 * 1024
PIPE_BYTES = 64 * 1024
MAX_DIAGNOSTIC_BYTES = 64 * 1024
MAX_PACKAGE_BYTES = 1024**3
MAX_FFU_BYTES = 2 * 1024**3
NETWORK_TIMEOUT = 30
DOWNLOAD_TIMEOUT = 1200
EXTRACT_TIMEOUT = 600
APPROVED_HOSTS = frozenset({"go.microsoft.com", "software-download.microsoft.com"})
Stamp = tuple[int, int, int, int, int, int, int, int, int]


class MediaError(ValueError):
    exit_status = 2


class AcquisitionError(MediaError):
    exit_status = 4


class InterruptedMedia(Exception):
    def __init__(self, signum: int) -> None:
        super().__init__(f"acquisition interrupted by signal {signum}")
        self.signum = signum


def _load_ffu() -> ModuleType:
    path = Path(__file__).with_name("iot-ffu.py")
    spec = importlib.util.spec_from_file_location("_wor_iot_media_ffu", path)
    if spec is None or spec.loader is None:
        raise MediaError("cannot load the bundled IoT FFU verifier")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


ffu = _load_ffu()


@dataclass(frozen=True)
class Package:
    size: int
    sha256: str


@dataclass(frozen=True)
class OfficialImage:
    iso: Package
    msi: Package
    cabinet: Package
    msi_offset: int
    cabinet_sector: int
    url: str
    member: str
    ffu_size: int
    ffu_sha256: str

    @property
    def cache_name(self) -> str:
        return f"iot-core-{self.ffu_sha256}.ffu"


def _number(entry: dict[str, Any], name: str, maximum: int) -> int:
    value = entry.get(name)
    if type(value) is not int or not 0 < value <= maximum:
        raise MediaError(f"invalid bounded {name} in bundled provenance")
    return value


def _digest(entry: dict[str, Any], name: str) -> str:
    value = entry.get(name)
    if not isinstance(value, str) or not re.fullmatch(r"[0-9a-f]{64}", value):
        raise MediaError(f"missing or invalid {name} in bundled provenance")
    return value


def _check_url(url: str) -> None:
    if not isinstance(url, str) or len(url) > 2048 or any(ord(c) < 33 or ord(c) > 126 for c in url):
        raise MediaError("download URL contains invalid characters")
    try:
        parts = urllib.parse.urlsplit(url)
        port = parts.port
    except ValueError as exc:
        raise MediaError("invalid official download URL") from exc
    if (
        parts.scheme != "https"
        or parts.hostname not in APPROVED_HOSTS
        or port not in (None, 443)
        or parts.username is not None
        or parts.password is not None
        or parts.fragment
    ):
        raise MediaError("download and redirects require HTTPS on an approved Microsoft host")


def _official_image() -> OfficialImage:
    manifest = ffu.load_provenance()
    images = manifest.get("images")
    if not isinstance(images, list) or len(images) != 1 or not isinstance(images[0], dict):
        raise MediaError("acquisition requires the single reviewed bundled image")
    entry = images[0]
    url, member = entry.get("resolved_source_url"), entry.get("source_cab_member")
    _check_url(url)
    if not isinstance(member, str) or not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_.-]{0,127}", member):
        raise MediaError("bundled cabinet member must be an exact, non-pattern filename")
    return OfficialImage(
        Package(_number(entry, "source_bytes", MAX_PACKAGE_BYTES), _digest(entry, "source_sha256")),
        Package(_number(entry, "source_msi_bytes", MAX_PACKAGE_BYTES), _digest(entry, "source_msi_sha256")),
        Package(_number(entry, "source_cab_bytes", MAX_PACKAGE_BYTES), _digest(entry, "source_cab_sha256")),
        _number(entry, "source_msi_offset", MAX_PACKAGE_BYTES),
        _number(entry, "source_cab_start_sector", MAX_PACKAGE_BYTES // 4096),
        url, member, _number(entry, "ffu_bytes", MAX_FFU_BYTES), _digest(entry, "sha256"),
    )


def _absolute(path: Path) -> Path:
    path = Path(path).absolute()
    if ".." in path.parts or len(path.parts) > 64:
        raise MediaError("path traversal or excessive path depth is unsupported")
    return path


def _stamp(value: os.stat_result) -> Stamp:
    return (
        value.st_dev, value.st_ino, value.st_mode, value.st_uid, value.st_gid,
        value.st_size, value.st_mtime_ns, value.st_ctime_ns, value.st_nlink,
    )


def _cache_path(path: Path) -> Path:
    path = _absolute(path)
    if sys.platform != "darwin" or len(path.parts) < 2 or path.parts[1] not in ("tmp", "var"):
        return path
    alias = Path("/") / path.parts[1]
    before = alias.lstat()
    if not stat.S_ISLNK(before.st_mode):
        return path
    target = os.readlink(alias)
    if (
        before.st_uid != 0
        or target not in (f"private/{alias.name}", f"/private/{alias.name}")
        or _stamp(before) != _stamp(alias.lstat())
    ):
        raise MediaError("unsafe or changed macOS system cache alias")
    return (Path("/private") / alias.name).joinpath(*path.parts[2:])


def _directory_owner(value: os.stat_result, leaf: bool) -> None:
    if value.st_uid not in (0, os.geteuid()) or leaf and value.st_uid != os.geteuid():
        raise MediaError("cache path has an unsafe owner; use a current-user cache directory")
    writable = value.st_mode & 0o022
    if writable and (leaf or not value.st_mode & stat.S_ISVTX):
        raise MediaError("cache path is group/world writable without ancestor sticky-bit protection")


@dataclass(frozen=True)
class Directory:
    path: Path
    fd: int
    secure: bool

    def check(self) -> None:
        with _directory(self.path, secure=self.secure) as current:
            before, after = os.fstat(self.fd), os.fstat(current.fd)
            if (before.st_dev, before.st_ino) != (after.st_dev, after.st_ino):
                raise MediaError("source/cache directory changed during acquisition")


@contextmanager
def _directory(path: Path, create: bool = False, secure: bool = False) -> Iterator[Directory]:
    path = _absolute(path)
    flags = os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC
    fd = os.open(path.anchor, flags)
    try:
        if secure:
            _directory_owner(os.fstat(fd), len(path.parts) == 1)
        for index, part in enumerate(path.parts[1:], 1):
            if create:
                try:
                    os.mkdir(part, 0o700, dir_fd=fd)
                except FileExistsError:
                    pass
            before = os.stat(part, dir_fd=fd, follow_symlinks=False)
            if not stat.S_ISDIR(before.st_mode):
                raise MediaError("symlink/non-directory component in source or cache path")
            child = os.open(part, flags, dir_fd=fd)
            os.close(fd)
            fd = child
            current = os.fstat(fd)
            if (before.st_dev, before.st_ino) != (current.st_dev, current.st_ino):
                raise MediaError("directory identity changed while opening")
            if secure:
                _directory_owner(current, index == len(path.parts) - 1)
        yield Directory(path, fd, secure)
    finally:
        os.close(fd)


def _regular(value: os.stat_result, private: bool) -> None:
    if not stat.S_ISREG(value.st_mode):
        raise MediaError("source/cache entry must be a regular file, not a symlink or device")
    if private and (
        value.st_uid != os.geteuid() or value.st_nlink != 1 or value.st_mode & 0o077
    ):
        raise MediaError("cached FFU must be private, single-link, and owned by the current user")


@dataclass(frozen=True)
class Source:
    stream: BinaryIO
    directory: Directory
    name: str
    stamp: Stamp

    def check(self) -> None:
        current = _stamp(os.fstat(self.stream.fileno()))
        named = _stamp(os.stat(self.name, dir_fd=self.directory.fd, follow_symlinks=False))
        if current != self.stamp or named != self.stamp:
            raise AcquisitionError("source/cache file changed during acquisition")
        self.directory.check()


@contextmanager
def _source(directory: Directory, name: str, private: bool = False) -> Iterator[Source]:
    before = os.stat(name, dir_fd=directory.fd, follow_symlinks=False)
    _regular(before, private)
    fd = os.open(name, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK | os.O_CLOEXEC, dir_fd=directory.fd)
    with os.fdopen(fd, "rb", buffering=0) as stream:
        current = os.fstat(fd)
        _regular(current, private)
        if _stamp(before) != _stamp(current):
            raise AcquisitionError("source/cache identity changed while opening")
        fcntl.flock(fd, fcntl.LOCK_SH | fcntl.LOCK_NB)
        yield Source(stream, directory, name, _stamp(current))


@contextmanager
def _workspace(cache: Directory) -> Iterator[Directory]:
    name = ".iot-media-" + secrets.token_hex(16)
    os.mkdir(name, 0o700, dir_fd=cache.fd)
    fd = None
    try:
        fd = os.open(name, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC, dir_fd=cache.fd)
        yield Directory(cache.path / name, fd, True)
    finally:
        try:
            if fd is not None:
                for filename in ("source.package", "source.cab", "image.ffu"):
                    try:
                        os.unlink(filename, dir_fd=fd)
                    except FileNotFoundError:
                        pass
        finally:
            if fd is not None:
                os.close(fd)
            os.rmdir(name, dir_fd=cache.fd)


@contextmanager
def _new_file(directory: Directory, name: str) -> Iterator[BinaryIO]:
    fd = os.open(name, os.O_RDWR | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW | os.O_CLOEXEC, 0o600, dir_fd=directory.fd)
    with os.fdopen(fd, "w+b") as stream:
        yield stream


class Progress:
    def __init__(self, label: str, size: int) -> None:
        self.label, self.size, self.previous = label, size, -1
        self.update(0)

    def update(self, count: int, complete: bool = False) -> None:
        percent = 100 if complete else min(99, 100 * count // self.size)
        if percent == 100 or percent // 5 != self.previous // 5:
            print(f"{self.label}: {percent}%", file=sys.stderr, flush=True)
            self.previous = percent


def _bounded_chunks(read: Callable[[int], bytes], size: int) -> Iterator[bytes]:
    total = 0
    while True:
        chunk = read(min(COPY_BYTES, size - total + 1))
        if not chunk:
            return
        yield chunk
        total += len(chunk)


def _copy_chunks(chunks: Iterable[bytes], target: BinaryIO, package: Package, label: str, deadline: float) -> None:
    digest, total = hashlib.sha256(), 0
    progress = Progress(label, package.size)
    for chunk in chunks:
        if time.monotonic() >= deadline:
            raise AcquisitionError(f"{label} timed out")
        total += len(chunk)
        if total > package.size:
            raise MediaError("package exceeds its pinned size")
        digest.update(chunk)
        if target.write(chunk) != len(chunk):
            raise AcquisitionError("short write while copying verified package")
        progress.update(total)
    if total != package.size:
        raise MediaError("package is truncated or has the wrong pinned size")
    if digest.hexdigest() != package.sha256:
        raise MediaError("package SHA256 does not match bundled official provenance")
    target.flush()
    os.fsync(target.fileno())
    os.fchmod(target.fileno(), 0o400)
    target.seek(0)
    progress.update(total, complete=True)


def _copy_package(source: BinaryIO, target: BinaryIO, package: Package, label: str, deadline: float) -> None:
    _copy_chunks(_bounded_chunks(source.read, package.size), target, package, label, deadline)


def _cabinet_chunks(snapshot: BinaryIO, image: OfficialImage, msi: bool) -> Iterator[bytes]:
    offset = 0 if msi else image.msi_offset
    wrapper_size = os.fstat(snapshot.fileno()).st_size
    if offset + image.msi.size > wrapper_size or image.msi.size % 4096:
        raise MediaError("pinned MSI extent is outside the verified wrapper")

    def read(relative: int, size: int) -> bytes:
        if relative < 0 or not 0 < size <= 4096 or relative + size > image.msi.size:
            raise MediaError("compound-storage read exceeds pinned MSI bounds")
        data = os.pread(snapshot.fileno(), size, offset + relative)
        if len(data) != size:
            raise MediaError("truncated compound-storage sector")
        return data

    header = read(0, 512)
    if header[:8] != b"\xd0\xcf\x11\xe0\xa1\xb1\x1a\xe1" or struct.unpack_from("<4H", header, 26) != (4, 0xFFFE, 12, 6):
        raise MediaError("only the reviewed version-4 MSI compound-storage layout is supported")
    sector_count = image.msi.size // 4096 - 1
    fat_count = struct.unpack_from("<I", header, 44)[0]
    difat_start, difat_count = struct.unpack_from("<II", header, 68)
    if not 0 < fat_count <= (sector_count + 1023) // 1024 or difat_count > 4:
        raise MediaError("compound-storage allocation tables exceed bounded MSI geometry")

    def sector(index: int) -> bytes:
        if not 0 <= index < sector_count:
            raise MediaError("compound-storage sector index is outside the MSI")
        return read((index + 1) * 4096, 4096)

    free, end, fat_marker, difat_marker = 0xFFFFFFFF, 0xFFFFFFFE, 0xFFFFFFFD, 0xFFFFFFFC
    indexes = list(struct.unpack_from("<109I", header, 76))
    seen_difat: set[int] = set()
    for _ in range(difat_count):
        if difat_start in seen_difat:
            raise MediaError("cyclic compound-storage DIFAT")
        seen_difat.add(difat_start)
        values = struct.unpack("<1024I", sector(difat_start))
        indexes.extend(values[:-1])
        difat_start = values[-1]
    if difat_start != end:
        raise MediaError("unterminated compound-storage DIFAT")
    indexes = [index for index in indexes if index != free]
    if len(indexes) != fat_count or len(set(indexes)) != fat_count or seen_difat.intersection(indexes):
        raise MediaError("inconsistent compound-storage FAT sector list")
    fat: list[int] = []
    for index in indexes:
        fat.extend(struct.unpack("<1024I", sector(index)))
    if len(fat) < sector_count or any(fat[index] != fat_marker for index in indexes) or any(fat[index] != difat_marker for index in seen_difat):
        raise MediaError("invalid compound-storage allocation table coverage")
    remaining, current = image.cabinet.size, image.cabinet_sector
    if remaining > image.msi.size:
        raise MediaError("pinned cabinet exceeds its MSI bounds")
    seen = bytearray(sector_count)
    while remaining:
        if not 0 <= current < sector_count or seen[current]:
            raise MediaError("out-of-bounds or cyclic cabinet sector chain")
        seen[current] = 1
        size = min(remaining, 4096)
        yield sector(current)[:size]
        remaining -= size
        current = fat[current]
    if current != end:
        raise MediaError("cabinet chain exceeds its pinned size")


class MicrosoftRedirects(urllib.request.HTTPRedirectHandler):
    max_repeats = 2
    max_redirections = 4

    def redirect_request(
        self, req: urllib.request.Request, fp: Any, code: int,
        msg: str, headers: Any, newurl: str,
    ) -> Optional[urllib.request.Request]:
        _check_url(newurl)
        return super().redirect_request(req, fp, code, msg, headers, newurl)


def _download_package(image: OfficialImage, target: BinaryIO) -> None:
    _check_url(image.url)
    opener = urllib.request.build_opener(
        urllib.request.ProxyHandler({}),
        urllib.request.HTTPSHandler(context=ssl.create_default_context()),
        MicrosoftRedirects(),
    )
    request = urllib.request.Request(
        image.url, headers={"User-Agent": "WoR-Flasher-IoT-Media/1", "Accept-Encoding": "identity"},
    )
    deadline = time.monotonic() + DOWNLOAD_TIMEOUT
    with opener.open(request, timeout=NETWORK_TIMEOUT) as response:
        _check_url(response.geturl())
        if response.status != 200:
            raise AcquisitionError("official download did not return HTTP 200")
        lengths = response.headers.get_all("Content-Length", [])
        if (
            len(lengths) != 1
            or not re.fullmatch(r"[0-9]{1,12}", lengths[0])
            or int(lengths[0]) != image.iso.size
        ):
            raise MediaError("official download Content-Length does not match its pinned size")
        if response.headers.get("Transfer-Encoding") or response.headers.get("Content-Encoding", "identity").lower() != "identity":
            raise MediaError("encoded/chunked official download is not supported")
        #read1 permits a deadline check after each socket read, even for a trickling server.
        _copy_chunks(
            _bounded_chunks(response.read1, image.iso.size), target, image.iso,
            "Downloading and verifying official ISO", deadline,
        )


def _tool() -> str:
    tool = shutil.which("cabextract")
    if tool is None:
        raise AcquisitionError("cabextract is required for ISO/MSI import (macOS: Homebrew cabextract; Debian/Ubuntu: cabextract)")
    return tool


def _tool_diagnostic(data: bytes) -> str:
    text = data.decode("utf-8", errors="replace")[:4096].strip()
    return "".join(c if c in "\n\t" or ord(c) >= 32 and ord(c) != 127 else "?" for c in text)


def _extract(snapshot: BinaryIO, target: BinaryIO, image: OfficialImage, work: Directory) -> None:
    before = _stamp(os.fstat(snapshot.fileno()))
    work.check()
    command = [
        _tool(), "--quiet", "--pipe", "--single", "--filter", image.member,
        f"/dev/fd/{snapshot.fileno()}",
    ]
    progress = Progress("Extracting approved FFU member", image.ffu_size)
    deadline = time.monotonic() + EXTRACT_TIMEOUT
    environment = dict(os.environ, TMPDIR=str(work.path), LC_ALL="C")
    process = subprocess.Popen(
        command, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        pass_fds=(snapshot.fileno(),), cwd=work.path, env=environment, bufsize=0,
    )
    total, diagnostic = 0, bytearray()
    try:
        if process.stdin is None or process.stdout is None or process.stderr is None:
            raise AcquisitionError("cannot establish bounded extraction pipes")
        process.stdin.close()
        with selectors.DefaultSelector() as selector:
            selector.register(process.stdout, selectors.EVENT_READ, "image")
            selector.register(process.stderr, selectors.EVENT_READ, "diagnostic")
            while selector.get_map():
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    raise AcquisitionError("cabextract timed out")
                for key, _ in selector.select(min(1.0, remaining)):
                    limit = image.ffu_size - total if key.data == "image" else MAX_DIAGNOSTIC_BYTES - len(diagnostic)
                    chunk = os.read(key.fd, min(PIPE_BYTES, limit + 1))
                    if not chunk:
                        selector.unregister(key.fileobj)
                        continue
                    if key.data == "image":
                        total += len(chunk)
                        if total > image.ffu_size:
                            raise AcquisitionError("cabextract output exceeds the pinned FFU size")
                        if target.write(chunk) != len(chunk):
                            raise AcquisitionError("short write while extracting FFU")
                        progress.update(total)
                    else:
                        diagnostic.extend(chunk)
                        if len(diagnostic) > MAX_DIAGNOSTIC_BYTES:
                            raise AcquisitionError("cabextract diagnostic output exceeds its limit")
        try:
            code = process.wait(timeout=max(0.001, deadline - time.monotonic()))
        except subprocess.TimeoutExpired as exc:
            raise AcquisitionError("cabextract timed out") from exc
        if code != 0:
            raise AcquisitionError(f"cabextract failed ({code}): {_tool_diagnostic(diagnostic)}")
        if total != image.ffu_size:
            raise AcquisitionError("cabextract returned a truncated or missing FFU member")
        if before != _stamp(os.fstat(snapshot.fileno())):
            raise AcquisitionError("verified package snapshot changed during extraction")
        if diagnostic:
            print(f"cabextract diagnostic: {_tool_diagnostic(diagnostic)}", file=sys.stderr, flush=True)
        target.flush()
        os.fsync(target.fileno())
        work.check()
        progress.update(total, complete=True)
    finally:
        if process.poll() is None:
            process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()
        for pipe in (process.stdin, process.stdout, process.stderr):
            if pipe is not None:
                pipe.close()


def _inspect(directory: Directory, name: str, image: OfficialImage) -> dict[str, object]:
    with _source(directory, name, private=True) as source:
        progress = Progress("Validating complete approved FFU", image.ffu_size)
        result = ffu.inspect_ffu(directory.path / name)
        source.check()
        if result["sha256"] != image.ffu_sha256:
            raise MediaError("FFU does not match this acquisition's pinned image")
        progress.update(image.ffu_size, complete=True)
        return result


def _result(info: dict[str, object], path: Path, mode: str) -> dict[str, object]:
    return dict(info, source_file=str(path), acquisition=mode)


def _cached(cache: Directory, image: OfficialImage) -> Optional[dict[str, object]]:
    try:
        os.stat(image.cache_name, dir_fd=cache.fd, follow_symlinks=False)
    except FileNotFoundError:
        return None
    try:
        info = _inspect(cache, image.cache_name, image)
    except ffu.FfuError as exc:
        raise MediaError(f"cached FFU failed verification; remove that cache entry and retry: {exc}") from exc
    return _result(info, cache.path / image.cache_name, "cache")


def _publish(cache: Directory, work: Directory, image: OfficialImage, mode: str) -> dict[str, object]:
    cache.check()
    work.check()
    staged = os.stat("image.ffu", dir_fd=work.fd, follow_symlinks=False)
    completed = False
    try:
        try:
            os.link("image.ffu", image.cache_name, src_dir_fd=work.fd, dst_dir_fd=cache.fd, follow_symlinks=False)
        except FileExistsError:
            concurrent = _cached(cache, image)
            if concurrent is None:
                raise AcquisitionError("cache entry changed during atomic publication")
            return concurrent
        os.unlink("image.ffu", dir_fd=work.fd)
        os.fsync(cache.fd)
        info = _inspect(cache, image.cache_name, image)
        cache.check()
        completed = True
        return _result(info, cache.path / image.cache_name, mode)
    finally:
        if not completed:
            try:
                current = os.stat(image.cache_name, dir_fd=cache.fd, follow_symlinks=False)
            except FileNotFoundError:
                current = None
            if current is not None and (current.st_dev, current.st_ino) == (staged.st_dev, staged.st_ino):
                os.unlink(image.cache_name, dir_fd=cache.fd)
                os.fsync(cache.fd)


def _acquire(cache: Directory, image: OfficialImage, source: Optional[Source], mode: str) -> dict[str, object]:
    if source is None:
        cached = _cached(cache, image)
        if cached is not None:
            return cached
        _tool()
    package = image.msi if mode == "local-msi" else image.iso
    if source is not None and os.fstat(source.stream.fileno()).st_size != package.size:
        raise MediaError("local package has the wrong pinned size")
    with _workspace(cache) as work:
        with _new_file(work, "source.package") as snapshot:
            if source is None:
                _download_package(image, snapshot)
            else:
                _copy_package(source.stream, snapshot, package, "Copying and verifying official package", time.monotonic() + DOWNLOAD_TIMEOUT)
                source.check()
                cached = _cached(cache, image)
                if cached is not None:
                    return cached
            snapshot_stamp = _stamp(os.fstat(snapshot.fileno()))
            with _new_file(work, "source.cab") as cabinet:
                _copy_chunks(
                    _cabinet_chunks(snapshot, image, mode == "local-msi"), cabinet,
                    image.cabinet, "Reassembling and verifying official cabinet", time.monotonic() + EXTRACT_TIMEOUT,
                )
                if snapshot_stamp != _stamp(os.fstat(snapshot.fileno())):
                    raise AcquisitionError("verified wrapper snapshot changed during cabinet reconstruction")
                with _new_file(work, "image.ffu") as target:
                    _extract(cabinet, target, image, work)
            _inspect(work, "image.ffu", image)
            if source is not None:
                source.check()
            return _publish(cache, work, image, mode)


def _host() -> None:
    if not (sys.platform == "darwin" or sys.platform.startswith("linux")):
        raise MediaError("native IoT acquisition supports only Linux and macOS")
    if not all(hasattr(os, flag) for flag in ("O_NOFOLLOW", "O_DIRECTORY", "O_CLOEXEC")):
        raise MediaError("host lacks required no-follow file-opening safeguards")


def prepare(source: Path, cache_dir: Path) -> dict[str, object]:
    """Validate a local FFU or import an exact official wrapper into a safe cache."""
    _host()
    path = _absolute(source)
    suffix = path.suffix.lower()
    if suffix == ".ffu":
        print("Validating complete approved FFU: 0%", file=sys.stderr, flush=True)
        info = ffu.inspect_ffu(path)
        print("Validating complete approved FFU: 100%", file=sys.stderr, flush=True)
        return _result(info, path, "local-ffu")
    if suffix not in (".iso", ".msi"):
        raise MediaError("source must be an approved local .ffu, .iso, or .msi file")
    image = _official_image()
    with _directory(path.parent) as parent, _source(parent, path.name) as opened:
        with _directory(_cache_path(cache_dir), create=True, secure=True) as cache:
            return _acquire(cache, image, opened, "local-" + suffix[1:])


def download(cache_dir: Path) -> dict[str, object]:
    """Explicitly download the pinned official ISO, or fully verify an existing FFU."""
    _host()
    image = _official_image()
    with _directory(_cache_path(cache_dir), create=True, secure=True) as cache:
        return _acquire(cache, image, None, "download")


@contextmanager
def _interruptions() -> Iterator[None]:
    previous = {sig: signal.getsignal(sig) for sig in (signal.SIGINT, signal.SIGTERM)}

    def interrupted(signum: int, frame: Any) -> None:
        for sig in previous:
            signal.signal(sig, signal.SIG_IGN)
        raise InterruptedMedia(signum)

    try:
        for sig in previous:
            signal.signal(sig, interrupted)
        yield
    finally:
        for sig, handler in previous.items():
            signal.signal(sig, handler)


def main(argv: Optional[Sequence[str]] = None) -> int:
    import json

    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    commands = parser.add_subparsers(dest="command", required=True)
    local = commands.add_parser("prepare", help="validate FFU or import an exact official ISO/MSI")
    local.add_argument("source", type=Path)
    remote = commands.add_parser("download", help="explicitly request the pinned official Microsoft ISO")
    for command in (local, remote):
        command.add_argument("--cache-dir", required=True, type=Path)
        command.add_argument("--json", action="store_true")
    args = parser.parse_args(argv)
    try:
        with _interruptions():
            result = prepare(args.source, args.cache_dir) if args.command == "prepare" else download(args.cache_dir)
            print(json.dumps(result, sort_keys=True) if args.json else result["source_file"])
        return 0
    except InterruptedMedia as exc:
        print(f"IoT media error: {exc}", file=sys.stderr)
        return 128 + exc.signum
    except (MediaError, ffu.FfuError) as exc:
        print(f"IoT media error: {exc}", file=sys.stderr)
        return exc.exit_status
    except (OSError, http.client.HTTPException) as exc:
        print(f"IoT media acquisition failed: {exc}", file=sys.stderr)
        return 4


if __name__ == "__main__":
    raise SystemExit(main())
