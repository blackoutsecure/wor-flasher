# Native IoT media service

`wor-iot-media` ports `src/lib/iot-media.py` without executing that helper,
an installer, a host shell, or an archive command. It supports the single
reviewed ARM32 image in the embedded, SHA-256-pinned IoT provenance manifest.
It never opens a target disk.

## Library boundary

```rust,no_run
use std::{path::Path, sync::atomic::AtomicBool};
use wor_iot_media::{MediaOptions, prepare};

let options = MediaOptions::new("private-download-cache")?;
let media = prepare(
    Path::new("official.iso"),
    &options,
    &mut |progress| eprintln!("{}: {}%", progress.label, progress.percent),
    &AtomicBool::new(false),
)?;
assert!(media.source_file.is_absolute());
# Ok::<(), wor_iot_media::MediaError>(())
```

- `prepare(&Path, &MediaOptions, &mut dyn FnMut(Progress), &AtomicBool)`
  returns `Result<AcquiredMedia, MediaError>` for `.ffu`, `.iso`, or `.msi`.
- `download(&MediaOptions, progress, cancelled)` has the same result and must
  only be called after an explicit request to obtain Microsoft's archived
  maker image, with its license/legacy-security limitations explained.
- `MediaService { verifier: &dyn FfuVerifier, download_provider:
  &dyn DownloadProvider }` exposes injectable providers. Production provenance
  is not injectable: public entry points still require the embedded image's
  exact whole-file SHA-256 and sizes.
- `AcquiredMedia { inspection, source_file, acquisition }` serializes the
  inspection fields plus the original `source_file`/`acquisition` keys.
  `Inspection` types format, architecture, profile, SHA-256, size and trust;
  `details` preserves all remaining verifier JSON, including geometry/HDMI.
  Acquisition is `local-ffu`, `local-iso`, `local-msi`, `download`, or `cache`.

## Companion and helper contracts

`MediaOptions.companion_executable` selects the current native CLI. `new()`
uses `WOR_IOT_NATIVE_COMPANION`, otherwise `std::env::current_exe()`. No
Python/script fallback exists. The fixed child argument vector is:

```text
<companion> iot-ffu inspect <absolute-regular-file> --json
```

`NativeFfuVerifier` bounds stdout to 1 MiB, stderr to 64 KiB, and execution
to 600 seconds; cancellation terminates and waits for the child. Children
inherit the worker's process group.

`run(&[OsString]) -> Result<i32, MediaError>` retains:

```text
iot-media prepare SOURCE --cache-dir DIR [--json]
iot-media download --cache-dir DIR [--json]
```

Success emits the FFU path or JSON; diagnostics/progress go to stderr.
Statuses are 0 (success), 2 (invalid/unapproved/unsafe), 4 (acquisition/I/O),
130 (SIGINT), and 143 (SIGTERM). Ordinary helper errors return `Ok(status)`;
`Err` is reserved for an adapter/reporting failure.

## Trust and publication

All wrapper, embedded MSI, reconstructed cabinet and FFU digests are checked.
Only the pinned CFB v4 stream chain is read; cabinet extraction is Rust
(`cab`, including MSZIP/LZX support) and writes a single caller-owned file,
never member paths. Traversal, duplicate names, spanning cabinets, size
overflows, invalid FAT/DIFAT chains, cycles and snapshot replacements fail.

HTTPS uses rustls, no proxy, no transparent decompression, approved Microsoft
hosts only, bounded redirects, exact Content-Length, and no encoded/chunked or
partial response. Network errors never carry request URLs or query strings.
Desktop `VERIFY_TLS`, `TRUST_CACHE`, or provenance environment overrides are
not consumed.

Caches use no-follow descriptor traversal, trusted ownership, private
single-link regular files, held identities, complete re-verification on
every reuse, and hard-link publication that never overwrites an existing
path. Failed own publications are rolled back by inode; snapshot/staging
cleanup is descriptor-anchored and cleanup failure blocks success.

## Offline validation

```text
cargo test --locked -p wor-iot-media -p wor-iot-account
cargo test --locked -p wor-iot-media --test mutations -- --ignored --nocapture
cargo clippy --locked -p wor-iot-media -p wor-iot-account --all-targets -- -D warnings
```

Fixtures use owned regular files and private directories under these crates,
not the system temporary directory. Native protocol tests exercise the
companion ABI, error JSON/status behavior and SIGINT/SIGTERM without a real
image download or device. Mutation checks compile disposable workspace
copies only. These checks do not qualify the archived Windows image, real
Microsoft delivery, or physical-media behavior.
