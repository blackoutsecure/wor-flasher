# Native desktop media preparation

`wor-desktop-media` implements T010 / REQ-012. It reads or downloads Windows ARM64
media, verifies it, and constructs a complete private startup tree. It never
discovers, partitions, formats, opens, or writes a target disk. Configuration
resolution, summaries, host/device safety and target writers remain outside this
crate.

## In-process contracts

```rust,ignore
PrepareOptions {
    runtime_dir: PathBuf,
    download_dir: PathBuf,
    settings: BTreeMap<String, String>,
}

prepare(&options, &mut progress, &cancelled) -> Result<PreparedMedia, MediaError>
catalog(&options, kind, &mut progress, &cancelled) -> Result<serde_json::Value, MediaError>
```

The settings map is the compatibility boundary for already-resolved
`NativeConfig::values()`. `DesktopOptions::from_compat` converts it to typed
models, build identifiers, source/cache/install modes, package policies and
setup/boot options. It does not load user JSON or implement another summary.
`CAN_INSTALL_ON_SAME_DRIVE` must be selected for preparation; catalogue queries
do not require a device or installation mode.

`PreparedMedia` retains its private staging directory and exposes:

- `boot_dir`: Windows boot/EFI files, the **configured** PE WIM (image 2), WoR-PE,
  drivers or the Pi 5 critical marker, model firmware, validated boot config,
  and applicable root answer files.
- `install_wim`: verified Windows editions; Pi 3 additionally requires non-solid
  LZX, 32 KiB chunks, preserved edition metadata and selected-partition fit.
- `install_extras`: applicable answer file and Pi 4 RAM-unlock script for
  `WOR_INSTALL`; an empty directory is valid.
- `build`, `language`, and `report`.

A writer copies these outputs; it must not run another PE-customization engine.
Call **`PreparedMedia::cleanup(self)` and propagate its result before reporting
verified success**. Drop is only a best-effort backstop. Preparation errors
explicitly check cleanup. A failed source-ISO unmount retains the containing
private workspace and reports its mount path rather than recursively deleting a
possibly mounted tree.

### Catalogue/report shapes

`catalog` accepts exactly `builds`, `uefi`, `drivers`, or `languages`:

| Kind | JSON array entries |
| --- | --- |
| `builds` | `{build, date, version}`; compatible Windows 11 rows, then Windows 10 rows; `version` is `Windows11` or `Windows10` |
| `uefi`, `drivers` | `{tag, asset_name, download_url}`; only uploaded, matching-model, non-draft, non-prerelease assets |
| `languages` | `{code, label, locale}`; for example `en-us`, `English (United States)`, `en-US` |

Pi 5 driver queries and unknown kinds are explicit errors. Build queries fetch
both families from one catalogue response. Tags retain published order and are
deduplicated; release pagination refuses partial results after ten 100-row pages.
`query_release_versions` separately supports the authoritative `Latest` endpoint
and its own five-minute cache.

Additional typed APIs include `QueryOptions`, `query_builds`, `query_catalogue`,
`query_release_versions`, `cached_windows_media`, `languages`,
`default_language`, `windows_locale`, `locales_from_xml`,
`query_locale_options`, `parse_image_metadata`, and the WIM/resource parsers.
`query_locale_options` preserves cached ISO/ESD image locales before the
published, English-first regional-setting fallbacks. Provider variants allow
offline fixtures without admitting test image packages to production.

`PreparationReport` is serializable and contains:

```json
{
  "model": "Pi4",
  "build": "22621.525",
  "language": "en-us",
  "source": "cached-esd",
  "cache_mode": "Verify",
  "packages": [
    {"kind": "uefi", "version": "v1.50", "origin": "pinned", "cache_reused": true}
  ],
  "warnings": []
}
```

Reports and errors exclude account passwords and download URLs. A direct
`query_catalogue` response intentionally includes the publisher's ESD URL and
digests as data; do not log that URL.

## Native providers and trust

Required native image tool: **`wimlib-imagex`** (wimlib / Debian `wimtools`).
Root-free ISO inspection uses **`bsdtar`** (libarchive; Debian
`libarchive-tools`). macOS includes bsdtar. For an unsupported/UDF-only archive,
the fallback is an owned read-only `hdiutil` source mount on macOS, or fixed
`sudo -n mount -o loop,ro,nosuid,nodev,noexec` / `umount` on Linux. Linux preserves
the UDF-module retry. The caller owns authentication; this crate never asks a
shell to run a mount. Tests never invoke a real mount.

All native subprocesses inherit the worker group. Cancellation is checked
between operations and during large copies, hashes, downloads and WIM tools.
Source mount/detach operations complete their ownership transition before
observing cancellation, so cleanup can reliably detach them. Tool output is
bounded and private; tool diagnostics and command arguments never enter errors.
`NativeTools` allows fixed executable overrides, not settings-map command strings.
WIM-update tokens use wimlib's own quoting grammar; paths containing both quote
delimiters or line breaks are rejected rather than interpreted.

- Runtime defaults/pins come from `runtime_dir/src/config/metadata.json`.
  Pi 4 remains pinned to its reviewed v1.50 default; Pi 3 follows its existing
  latest preference, with explicit warnings on permitted pinned fallback.
- Metadata always uses verified HTTPS. `VERIFY_TLS=0` explicitly downgrades
  payload certificate checks only and emits a warning; hashes remain required.
  Redirects cannot downgrade HTTPS or introduce URL credentials.
- WoR-PE and the optional ARM64 UEFI Shell package require their expected SHA-256.
  Microsoft ESDs require exact size and their published SHA-1 and/or SHA-256;
  all supplied digests must agree. Driver/firmware packages retain the source's
  TLS + release selection + extracted SHA-256-manifest trust model.
- Linux checks the download filesystem's FAT file-size limit before downloading
  an ESD of 4 GiB or more. `df` is the existing system filesystem-information tool;
  ExFAT is not incorrectly classified as FAT32.
- ZIPs are structurally validated before extraction: no traversal, absolute or
  Windows-special paths, links/devices, case collisions, encrypted members or
  expansion-limit violations. CRC failures abort extraction.
- `USE_CACHE=1` requires source tokens and stable source-compatible SHA-256
  manifests, including newly imported Windows bases. Old `alldone`-only Windows
  caches are not silently treated as integrity-verified. Mode 2 deliberately
  skips manifest/version checks but still enforces required files, WIM integrity,
  actual ARM64/build compatibility and the Pi 3 helper's hashes.
  Mode 0 refetches packages and the verified raw ESD rather than accidentally
  retaining a second hidden download cache.
- Per-run changes **never modify verified base caches**. PE scripts are rebuilt
  privately, settings.ini is private, and Pi 3 normalization writes a separate
  candidate. Cache replacement occurs only after successful import/verification.
- The shipped, qualified **Go** `Pi3BootRefresh.exe` and Go license remain the
  PE payload. Both must match the existing runtime manifest, including in cache
  trust mode. This crate never promotes the Rust helper.
- Required templates and firmware/boot/driver files fail closed when absent.
  Installed-image metadata must agree with the selected build and ARM64 model;
  compatibility ceilings cannot be widened through the adapter.

## Optional fixed helper command

`run(&[OsString]) -> Result<i32, MediaError>` supports the root executable's
`desktop-media` dispatcher before runtime initialization. stdout is one JSON
document; progress uses the existing tab protocol on stderr. Commands:

```text
languages [--json]
builds --runtime-dir PATH --cache-dir PATH --model 3|4|5 --version 10|11
releases --runtime-dir PATH --cache-dir PATH --model 3|4|5 --kind uefi|drivers [--latest]
catalogue --runtime-dir PATH --cache-dir PATH --build BUILD --language CODE
cached --download-dir PATH
prepare --runtime-dir PATH --download-dir PATH --settings PRIVATE_JSON --output-dir NEW_PATH
cleanup --staging-dir PATH
```

Explicit helper preparation calls the same library pipeline. `--settings` is a
flat normalized string map, not the repository's nested user configuration;
never pass passwords as command arguments. Normal in-process writers should
retain `PreparedMedia` rather than use this optional persistent mode.

`persist_to` requires a **new** output directory on the staging filesystem.
The retained tree stays private (0700 on Unix); it may contain opt-in answer-file
credentials. `StagedMedia` / `cleanup` bind an ownership receipt to its canonical
path, directory identity and owner and never delete the download/base caches.
The host still owns signals and the worker process group.

## Validation

From the workspace root, with the committed Rust environment:

```text
cargo test -p wor-desktop-media
cargo test -p wor-desktop-media --test guard_mutations -- --ignored --nocapture
cargo fmt -p wor-desktop-media -- --check
cargo clippy -p wor-desktop-media --all-targets --no-deps -- -D warnings
```

The ordinary suite includes strict-format/cache cases, fake native subprocesses
and read-only mounts, real small WIM/ESD preparation, native ISO/archive
preparation, and Bash **test-only** source differentials. The ISO fixture has a
small real filesystem and a sparse 3 GiB logical file to preserve the source's
minimum-size guard. No Windows download or target disk is used. The real ISO
builder fixture uses macOS `hdiutil makehybrid`; it does not attach an image.

The explicit mutation test compiles copies under this crate's ignored `target/`,
never changes the working source or oracles, verifies the isolated baseline,
and kills CPU-ceiling, converted-layout, qualified-helper-digest and actual
ARM64-image guard removals. Source differential tests use Bash solely as an
offline oracle; there is no Bash/Python runtime delegation in the library.

These checks do not qualify real Windows installation, Linux mount privileges,
Pi firmware, physical media, or the PE helper on ARMv8.0 hardware.
