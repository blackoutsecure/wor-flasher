# Native IoT Core FFU engine

`wor-ffu` is the source-anchored native replacement for
`src/lib/iot-ffu.py`. Attribution and licensing remain governed by the
repository's `NOTICE` and `LICENSE`. Native device application remains
experimental until separately hardware-qualified.

## APIs

- `parse_ffu(&mut impl Read + Seek)` and `parse_ffu_with_cancel(..., &AtomicBool)`
  completely validate an untrusted, bounded, single-store, uncompressed V1 FFU.
  They are read-only and **do not authorize** an image or target.
- `resolve_plan(&ParsedFfu, target_bytes)` resolves forward-writing start/end
  locations, bounded geometry and exact phase replacement. The returned plan
  is not a write capability.
- `load_provenance()`, `image_profile(board)`, and `hdmi_options(mode, custom)`
  provide reviewed expectations/preferences without accessing an image/target.
  A profile reports image verification as `not-assessed`.
- `inspect_ffu(path, hdmi_mode, hdmi_config)` (and its `_with_cancel` variant)
  verifies the complete source, exact compiled approval and derived HDMI blocks.
- `identify_target(path)` reads native device-node metadata without opening the
  target device. It preserves signed macOS device/raw-device IDs and the inode.
- `apply_ffu(source, target, &ApplyOptions, Option<&mut Progress>, &AtomicBool)`
  independently reopens/verifies the complete source and applies only the
  approved image to native devices. The caller must confirm the whole disk,
  exclude host/root/source/runtime/log disks and unmount all target volumes.
- `apply_to_file_fixture(...)` is explicitly **test-only**. It shares the real
  parser/writer but can open only existing, singly linked regular targets.
  It cannot authorize a device and is never exposed through a CLI/config/env
  switch. Publicly mutable parsed metadata is never accepted as write authority.

All service errors are typed as `Error::{Source, Arguments, Target, Apply, Io,
Interrupted}`. `exit_status()` returns the original helper's category:
`0` success, `2` invalid/unapproved source/arguments, `3` unsafe target,
`4` I/O/write/readback failure, `130` SIGINT/cancellation, `143` SIGTERM.

`run(&[OsString]) -> Result<i32, Error>` accepts arguments **after** a parent
`iot-ffu` selector. `run_with_io` additionally accepts stdout/stderr sinks.
Both diagnose ordinary failures and return `Ok(status)`; an `Err` means
diagnostic output or signal-handler restoration itself failed. The optional
native `iot-ffu` binary is the same thin adapter. Commands/JSON/progress match
the source helper: `inspect`, `profile`, `hdmi-options`, `identify-target`,
`apply`. Progress goes only to stderr as `PHASE IoT Core PERCENT%`.

Library callers own cancellation through the supplied atomic flag; no worker
is detached. The CLI temporarily handles SIGINT/SIGTERM using an atomic signal
flag, checks cancellation between bounded I/O operations, and restores the
previous handlers. SIGINT and SIGTERM never select another writer.

## Safeguards and bounds

The canonical repository `src/config/iot-core-images.json` is embedded directly
at compile time, with no maintained crate-local copy, and authenticated against
`74e9324c77e3e560a34742de1cd13e6c234789bf5089a75ba334d80c098ca41a`.
Private workspace/source bundles must include that canonical file. The compiled
binary needs no runtime manifest file. Offline tests also check the embedded
bytes and digest against the canonical file; isolated mutation builds reproduce
the same repository-relative source layout using an unchanged test-only copy.
The only approved FFU SHA-256 is
`15e9451eb7b3e5645e89e25ffc846d4dfd4b4a8c9c63600e177cb16817da67f5`.
Catalog CMS signatures are opaque, not the trust anchor. Filename, self-declared
chunk hashes, wildcard platform, expected hash, environment and configuration
cannot approve another image. Approval also binds build, ARM32/IoT/V1 profile,
platform IDs, boards, exact file/payload/minimum/chunk/sector geometry.

- Images: at most 16 GiB; target: aligned 512-byte sectors, image minimum through
  2 TiB, unused larger capacity unchanged unless an explicit wipe is requested.
- Chunk: power-of-two 1–1024 KiB; catalog: 2 bytes–1 MiB with canonical definite
  DER envelope; manifest: 1–64 KiB ASCII, at most 130 sections; SHA-256 table:
  32 bytes–16 MiB; descriptors: 1–131072, 1–16 locations each, at most 262144
  extents. Every metadata/payload chunk and the entire source are hashed.
- Linux uses exclusive block-device opens and rejects WSL. macOS accepts only
  character devices named whole `/dev/rdiskN`. Every path component is opened
  no-follow; traversal, depth over 64, source/target aliasing and open-time node
  replacement are refused. A shared source lock and complete native stat stamp
  bind the same source descriptor through readback.
- Existing GPT is refused by default. Cleanup requires both prior identities,
  consistent bounded primary/backup headers/tables, header/table CRCs, matching
  copies, valid usable/partition ranges and no overlapping metadata/partitions.
  Table data is capped at 1 MiB. Snapshots must still match before erasure.
  Cleared metadata is flushed/read back before image writes, and regions outside
  final extents are checked again afterwards.
- A separate full-drive wipe requires both identities and excludes GPT cleanup.
  It zeros and reads every addressable sector in at most 4 MiB operations before
  writing the image, without trusting old partition offsets. It does not securely
  erase remapped flash or repair hardware.
- HDMI custom text is at most 512 ASCII bytes. Only `hdmi_group`, `hdmi_mode`,
  `hdmi_force_hotplug`, `hdmi_drive` are accepted, with Pi 2/3 CEA/DMT bounds.
  Derivation occurs before target access, requires consistent bounded FAT16
  copies, one regular root config file (at most 4096 bytes), an acyclic chain of
  at most 16 clusters, and sufficient existing allocation. Only that allocation
  and its directory size change; the source, FATs and other files stay intact.
- Sector-aligned short I/O is retried; zero/unaligned/excessive I/O fails. Phase
  transitions flush. Linux invalidates the block cache before readback; macOS
  explicitly flushes the raw-device hardware cache. Cache capability failures
  precede writes. Every final (including configured) block is compared against
  the same still-checked source. Late source/target changes fail, never verify.
  Target and source close errors are checked before returning success.

The sole added dependency is foundational `libc`, restricted to no-follow
`openat`/native `stat`, checked `close`, `flock`, disk geometry/flush ioctls, `uname`, and signal
handling. There are no production subprocesses, network requests or runtime
Python/Bash/Node dependencies.

## Offline validation

From the repository root, using the assigned Rust environment:

```sh
cargo test --locked --offline -p wor-ffu
cargo clippy --locked --offline -p wor-ffu --all-targets -- -D warnings
cargo fmt -p wor-ffu -- --check
python3 -B crates/ffu/tests/mutations.py
```

Tests generate non-Windows regular-file fixtures under this crate and remove
them afterwards. The Python test-only oracle imports the unchanged source
helper/fixtures, compares parser plans, real native output bytes, progress, and
JSON, and optionally exercises an `sgdisk`-generated regular-file GPT. The
signal test waits for the owned child's source lock before sending SIGINT/SIGTERM
and verifies termination and lock release. Mutation checks change only isolated,
uniquely named source-copy packages, never the live implementation.

No test opens physical storage for writing, downloads official Windows media,
mounts devices or claims Linux/macOS hardware qualification.
