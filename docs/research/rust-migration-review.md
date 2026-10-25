# WoR-Flasher Rust migration review

Review date: 2026-10-24. Source baseline: `patch-1`, commit `505f501`.

**Status: native host software replacements implemented; production qualification
is gated.** Configuration, media preparation, device safety, desktop writing,
FFU deployment, acquisition, SSH personalization and the guided Dioxus workflow
now have native Rust implementations. The native host runtime contains no
Bash/Python/Node bridge. Stable classic releases remain unchanged. The Windows
ARM64 helper now cross-links through open-source LLVM-MinGW and is packaged as a
separate candidate. It is not promoted over its shipped Go payload before
hardware qualification; a proprietary SDK is no longer a software build blocker.

The policy source is the user-supplied **Blackout Secure Recommended Language &
Framework Standard**, whose footer identifies it as the **Recommended Rust
Technology Standard**. Pages 1-2 select Rust for production software, Dioxus for
desktop presentation, Clap for command-line applications, and framework-independent
domain/application logic. The supplied PDF is not redistributed here.

## Recommendation

**Use Dioxus Desktop with a shared Rust backend and a Clap CLI.** Dioxus is the
company-standard choice, not a recommendation to introduce Tauri, Electron, Iced,
or egui as a second application framework.

Migrate incrementally, keeping exactly one authoritative implementation for each
flashing workflow. A Rust window that still delegates everything to Bash is a
useful transition, but is **not** a completed Rust conversion. Conversely, a
one-pass replacement of the existing disk writer would discard important
compatibility and failure-handling evidence.

Keep the current product scope: Debian-family Linux and macOS 13+ hosts,
desktop ARM64 media, and the separate legacy ARM32 IoT Core workflow. Dioxus
supporting another platform does not qualify WoR-Flasher to flash from that host
or expand its Raspberry Pi/image compatibility.

### Workload-specific stack

| Boundary | Recommended technology | Reason and constraint |
| --- | --- | --- |
| Domain | Plain Rust; `thiserror` where typed errors are needed | Board/image compatibility, settings validation, capacity rules and write-plan models; no UI, HTTP, filesystem or OS services |
| Application | Rust use cases and narrowly scoped backend interfaces | Own preparation, confirmation, cancellation and verification sequencing; depend on domain, not concrete providers |
| Desktop | Dioxus Desktop | One presentation shell for the supported Linux/macOS hosts; no disk-writing decisions in components |
| CLI | Clap | Keep argument parsing and exit-status presentation outside reusable logic |
| Configuration and messages | Serde at serialization boundaries | Preserve typed values, explicit `false`, aliases and precedence; avoid untyped JSON as the application model |
| Concurrent I/O | Tokio; Reqwest for Rust-owned HTTP clients | Add when downloads, progress and cancellation need it; do not block the UI or create competing runtimes |
| Diagnostics | Tracing | Structured, redacted diagnostics; no passwords, tokens or credential-bearing request dumps |
| Executable errors | `anyhow` for application-boundary context | Reusable crates retain typed errors, rather than exposing `anyhow::Error` as their public contract |
| Platform/image tools | Rust infrastructure adapters around existing native tools initially | Keep mature WIM, partition and mount tooling until there is a separately validated reason to replace it |

Do **not** add Axum, an HTTP listener, SQLx, SQLite, a cloud backend, a JavaScript
application stack, or an agent framework. This is a local desktop/CLI imaging tool,
not a web service; its current file-based cache does not establish a database need.
Rust is justified by production-language policy and maintainability, not an
unmeasured claim that rewriting orchestration accelerates native disk I/O.

## Current implementation and migration implications

| Surface | Current evidence | Migration implication |
| --- | --- | --- |
| Shared engine | [`install-wor.sh`](../../install-wor.sh), including `drive_capability`, `validate_target_before_write`, `load_config_json` and `verify_written_image` | Port shared decisions and complete use cases, not separate implementations for GUI and CLI |
| GUI and lifecycle | [`install-wor-gui.sh`](../../install-wor-gui.sh), including `gui_start_installer`, `gui_cleanup` and the platform Advanced Options flows | Replace presentation while preserving process ownership, cancellation, authentication and option parity |
| Public integration | [`install-wor-hook.sh`](../../install-wor-hook.sh) and the [adapter contract](../../README.md#integration-adapter) | Preserve the four commands, textual outputs, progress protocol and exit statuses; this is not an existing JSON RPC API |
| Run configuration | [`config.schema.json`](../../config-templates/config.schema.json) and `load_config_json` in the engine | Maintain environment/CLI > JSON > defaults, nested-setting precedence and explicit boolean handling |
| Canonical metadata | [`metadata.json`](../../src/config/metadata.json), [`metadata.sh`](../../src/lib/metadata.sh) and [`node-runtime.mjs`](../../src/lib/node-runtime.mjs) | Keep one source for product identity, pins, guardrails and runtime inventory; Cargo metadata must not become a competing source |
| IoT deployment | [`iot-core.sh`](../../src/lib/iot-core.sh), [`iot-ffu.py`](../../src/lib/iot-ffu.py) and [`iot-media.py`](../../src/lib/iot-media.py) | Critical Python/shell runtime code is part of the eventual backend conversion, not disposable build scripting |
| Boot configuration and Pi 3 preparation | [`boot-config.py`](../../src/lib/boot-config.py) and [`pi3-hybrid-mbr.py`](../../src/lib/pi3-hybrid-mbr.py) | Separate pure validation/layout calculations from file and device access; retain the current fixtures and protected settings |
| Post-boot IoT setup | [`iot-account.py`](../../src/lib/iot-account.py) | Preserve separate, opt-in, fingerprint-pinned SSH personalization and its pending/verified outcomes |
| On-device Pi 3 helper | [`bootstrap.go`](../../src/pi3-boot-refresh/bootstrap.go), [`main_windows.go`](../../src/pi3-boot-refresh/main_windows.go) and its [build contract](../../CONTRIBUTING.md#running-the-tests) | This runs under Windows ARM64/WoR-PE, not on the flashing host; port and qualify separately, retaining the ARMv8.0 CPU requirement |
| Packaging and updates | [`package.json`](../../package.json), [`build-release.mjs`](../../src/build-release.mjs), [`updater.mjs`](../../src/updater.mjs) and the [macOS launcher](../../src/macos-app/Contents/MacOS/WoR-Flasher) | Preserve artifact completeness, version agreement, integrity checks and the bounded macOS runtime-update exception |

The main engine and GUI alone contain 9,835 lines at the reviewed commit, before
the IoT, boot, packaging and test surfaces. This is a substantial behavioral
migration, not a framework substitution.

### Architecture and contract decisions required before implementation

1. **Record the approved repository architecture change.**
   At the reviewed baseline, [AGENTS.md](../../AGENTS.md) required the Bash engine,
   prohibited replacing the established design with a framework, and required
   approval for dependencies, public hooks, safety behavior and canonical
   metadata changes.
   [CONTRIBUTING.md](../../CONTRIBUTING.md#house-style) reinforces that design.
   The user subsequently approved implementing Rust replacements. The staged
   implementation retains the qualified shared writer, compatible classic entry
   points, attribution and safety requirements; it does not silently retire them.

2. **Do not mistake the hook for a complete desktop backend.**
   It exposes `list-devices`, `describe-device`, `summary` and `run`.
   Device paths are line-oriented, descriptions are human-readable, summaries
   are tab-separated, and usage errors return `2`. It does not expose a typed
   model/build/language catalog, Advanced Options API, or versioned request/
   response protocol. A complete Dioxus GUI therefore needs Rust-owned use cases
   or an explicitly approved bridge extension; inventing commands or parsing
   arbitrary display strings into safety decisions is not sufficient.

3. **Reconcile the actual process lifecycle, not just old prose.**
   The GUI sources shared functions and `gui_start_installer` starts the same
   engine as an owned background worker without a visible terminal. The
   in-process-only description in [AGENTS.md](../../AGENTS.md#architecture) does
   not describe that worker. A Rust controller must preserve owned-child
   cancellation, exit status and cleanup, not introduce a terminal or detach a
   privileged writer when the window closes.

4. **Qualify the renderer on the real host matrix.**
   [Dioxus Desktop](https://github.com/DioxusLabs/dioxus/blob/main/packages/desktop/README.md)
   uses the operating system's webview, not native AppKit/yad controls.
   Its [Wry backend](https://github.com/tauri-apps/wry#platform-considerations)
   uses WebKitGTK on Linux and WebKit on macOS. The current upstream Debian/
   Ubuntu development dependency is `libwebkit2gtk-4.1-dev`; that alone is not a
   complete end-user installation specification. Verify the selected release's
   build/runtime packages, display support and minimum distribution versions.
   Inventory Raspberry Pi OS host architectures, including any existing 32-bit
   installations; ARM32 IoT output and ARM64 desktop output are not host
   architecture declarations. Keep the headless CLI independent of Dioxus,
   WebKit and a display session. Do not silently drop older supported hosts.

5. **Keep licensing and release obligations unchanged.**
   [NOTICE](../../NOTICE) explicitly records unresolved upstream licensing.
   A Rust rewrite does not resolve that grant, remove Botspot's authorship, or
   justify changing GPL-3.0-or-later metadata to MIT/Apache. Review new dependency
   notices and any copied/derived code before distribution. Windows media,
   downloaded firmware and drivers must not be bundled as application assets.
   The current macOS app is unsigned/unnotarized; Dioxus bundling does not
   automatically add signing, notarization or release authenticity.

## Target architecture

Implemented workspace layout; the application boundaries are still being migrated:

```text
Cargo.toml
Cargo.lock
rust-toolchain.toml
crates/
  domain/             board/image types, settings rules, capacity/layout models
  application/        preparation and flash use cases; backend interfaces
  infrastructure/     Linux/macOS tools, HTTP/cache, device I/O, legacy bridge
apps/
  cli/                Clap entry point and compatibility command handling
  desktop/            Dioxus components and desktop entry point
  pi3-boot-refresh/    native on-device helper candidate and synthetic fixtures
```

Both executable entry points wire infrastructure into application use cases.
Infrastructure implements application interfaces using domain types. Application
depends on domain; domain depends on neither infrastructure nor a framework.
Do not add an interfaces crate, plugin framework or one crate per platform helper
without a concrete need.

Keep the physical board revision, image family, architecture and firmware/package
route distinct. In particular, Pi 2 v1.2's desktop route uses `RPI_MODEL=3` but must
not be relabeled as Pi 3 or admit Pi 2 v1.1 to desktop ARM64.

Expose typed progress and result states in the Rust application layer. Adapt those
to the existing hook's `STATUS`, `STEP`, `SUBSTEP` and `TASK` records at its boundary.
Distinguish failed, canceled, dry-run/not-written, verification-skipped and fully
verified results. IoT account/language personalization is a separate result;
pending reboot or a skipped operation must never become verified success.

### Privilege and write boundary

- Keep the Dioxus UI unprivileged and load local, bundled UI assets. Do not give
  remote pages, navigation or arbitrary JavaScript access to a privileged command
  executor. A desktop webview does not require a local HTTP API.
- Invoke fixed executables using argument arrays, not generated `sh -c` commands.
  Use private input channels for credentials, never command-line `--set` values,
  logs or serialized UI/debug dumps.
- A temporary bridge must select the complete, local, verified release runtime.
  Do not rely on the standalone hook's mutable-ref clone bootstrap as the new
  app's installed backend or source individual scripts from mixed revisions.
- Preserve the engine's final host, whole-disk, boot/source-disk, capacity and
  installation-mode checks after preparation and immediately before writing.
  Discovery and settings previews do not authorize erasure.
- Bind IoT consent to source and target identities and geometry, preserving
  no-symlink checks, native device/inode identity, signed macOS device identifiers,
  bounded GPT cleanup and mandatory read-back. Do not generalize stronger IoT
  checks into a claim that the current desktop path already has identical checks.
- Keep mounts, helper processes and device handles under explicit lifecycle
  ownership. Cancellation waits for owned work to stop and reports cleanup
  failures; dropping a future or window is not proof that a privileged child
  stopped. Any new helper/IPC or authorization design requires its own approval.

## Phased conversion

| Stage | Deliverable | Exit condition |
| --- | --- | --- |
| 0. Approve and qualify | Approved architecture/bridge scope; host/CPU matrix; pinned Rust, Dioxus and matching `dx` tooling; agreed compatibility fixtures | Desktop prototype launches on supported host architectures; CLI builds without GUI libraries; no device writes or support expansion |
| 1. Shared core and CLI | Domain/application crates, typed configuration and a read-only Clap interface; legacy engine remains authoritative | Old/new settings and compatibility fixtures agree, including invalid inputs; no public command or default changes |
| 2. Dioxus presentation | One Linux/macOS wizard and application controller over the approved backend boundary | All existing choices, Advanced Options, confirmations, progress, error paths and lifecycle tests pass on both hosts; incomplete functionality stays explicitly preview-only |
| 3. Rust desktop workflow | Rust-owned media preparation, cache/integrity checks, staging, write orchestration, verification and cleanup; native image/OS tools remain adapters | Each complete use case passes differential and fault-injection tests before switching its single authoritative implementation |
| 4. Remaining critical runtime | IoT acquisition/FFU/GPT/read-back, boot helpers, SSH personalization and separately qualified Windows ARM64 helper | Existing binary/format fixtures and guard-removal mutations pass against the Rust implementation; no weaker allowlist or compatibility assumptions |
| 5. Package and cut over | Versioned Rust CLI/desktop artifacts, complete assets/notices, updated compatibility wrappers, CI and operator documentation | Packaged offline launch, failure/cancel/update tests and approved hardware evidence pass; obsolete engine/runtime paths retire only after parity |

Stages are ordered to preserve one source of truth, not to run competing writers.
During extraction, shadow comparisons may exercise pure rules on fixtures, but
the existing engine still authorizes legacy writes. Switch a complete use case
coherently; do not divide final validation and destructive execution between
independent implementations. Never automatically retry a failed Rust flash
through the Bash engine.

Retain the three root script entry points and the hook contract until an explicit
versioned compatibility decision permits changing them. Any proposed GUI/helper
protocol must be versioned, bounded, validated and tested independently of display
text, with explicit nonzero errors for malformed messages.

Maintainer-only Node packaging may remain temporarily where it has a documented
workload-specific justification. Critical, long-running or reusable production
logic, including integrity/update verification, cannot be declared compliant
merely by calling it a script. Porting native tools themselves is not required
just because their orchestration moves into Rust.

## Required parity and cutover evidence

| Area | Measurable acceptance gate |
| --- | --- |
| Configuration | Existing JSON/environment/CLI precedence, explicit `false`, nested/legacy aliases, malformed input and paths with spaces retain their tested behavior |
| Desktop limits | Test `N-1`, `N`, `N+1` bytes at 8 GiB and 25 GiB; preserve recovery versus direct-install behavior and the Pi 3/Pi 4 `25163` build ceiling, including rejection above it |
| Device protection | Root disks and aliases, layered Linux root devices, macOS APFS physical stores, read-only devices, partitions, missing geometry and post-preparation changes cannot reach the writer |
| Profile separation | Desktop ARM64 and allowlisted IoT ARM32 remain separate; reject unsupported boards/revisions, arbitrary FFUs and ARM64 IoT Core |
| IoT integrity | Existing parser limits, hashes, source/target substitution, GPT CRC/bounds, full-wipe consent, restricted HDMI patches, short I/O and read-back corruption fixtures fail or succeed identically |
| GUI parity | Both hosts cover every existing Advanced Option, Back/Cancel/refresh transaction, no-drive Quit, authentication failure/retry, progress ordering and visible diagnostics; no visible terminal is required |
| Lifecycle and results | Cancel before authentication/preparation causes no write; cancel during work stops owned descendants and cleans resources; nonzero worker/verification failures cannot report success |
| Personalization | SSH identity confirmation, credential redaction, administrator SID checks, DefaultAccount language execution, task cleanup and pending-reboot versus verified results remain covered |
| Distribution | Build/run each supported host architecture; test headless CLI without WebKit/display; preserve runtime/asset digests, metadata agreement, damaged-archive rejection and the macOS staged-update rollback contract |
| Pi 3 on-device helper | Qualify the Windows ARM64 binary in WoR-PE and on ARMv8.0 hardware, not merely by cross-compiling; retain GPT/FAT fixtures and source/binary freshness checks |

Keep the [existing tests](../../tests/) during migration. Port tests to exercise
real Rust production code rather than duplicating rules in test helpers; static
shell-text assertions alone cannot certify the replacement. Mutation-test new
safety coverage as [CONTRIBUTING.md](../../CONTRIBUTING.md#running-the-tests)
requires. No automated validation may flash physical media.

Extend both the [test workflow](../../.github/workflows/shellcheck.yml) and
[release workflow](../../.github/workflows/release.yml) for Rust source, Cargo
manifests/lockfile, toolchain, Dioxus inputs and relevant assets. Keep current
checks while their runtime is shipped. Future Rust gates should include locked
build/test runs, formatting, Clippy, headless core/CLI tests and separately
provisioned desktop jobs on Linux/macOS. Select exact dependency/tool versions
after qualification; the company PDF does not specify them.

[Dioxus bundling](https://dioxuslabs.com/learn/0.7/guides/deploy/#desktop-creating-an-installer)
is a packaging mechanism, not evidence that WoR's external image tools, assets,
privilege flow or update manifests are included correctly. Preserve ordinary
read-only update notices and the existing bounded, digest-verified macOS runtime
updater with staging/rollback; do not introduce checkout self-replacement.

Before declaring a workflow hardware-qualified, record the board revision,
image hash, cold/warm boot, serial logs, storage, display, networking and
shutdown/reboot evidence described in [Development](../../README.md#development).
Real-device tests require explicit user approval and a selected target.

## Validation performed for this review

- Read the supplied two-page standard locally and checked the framework boundary
  against the source surfaces linked above.
- Ran `npm run check` on macOS with Node `v24.15.0`: **840 passed, 0 failed,
  1 skipped (841 tests total)**, shell syntax passed, package metadata matched,
  and the isolated release packaging plan was valid. The skip is the test for
  refusing native DMG creation on non-macOS hosts, which does not apply here.
- These are existing implementation/package checks, **not** Rust migration,
  Linux loop-device integration, interactive Dioxus UI or physical-board evidence.
  No Rust toolchain or framework dependencies were installed, and no physical
  media was accessed.

### First implementation evidence after approval (B01)

The preceding baseline records the original review-only pass. The subsequent
approved implementation uses Rust 1.99.1 and Dioxus 0.7.10:

- 73 native Rust tests cover Python-oracle boot validation, synthetic
  Pi 3 GPT/FAT fixtures, runtime tampering, preview-bound configuration snapshots
  and owned-process cancellation.
- Formatting, workspace/all-target Clippy and Windows ARM64 helper type-checking
  pass. The Windows helper has not been cross-linked or hardware-qualified; the
  shipped Go executable is intentionally unchanged.
- Boot-validator, Pi 3 and consent guard mutations are detected by their tests.
  Static source-branch mappings are not a measured branch-coverage claim.
- The complete existing check reports 844 passed, one platform-inapplicable skip
  and zero failures in both classic mode and with native Rust boot validation
  selected.
- A real Dioxus webview mounts and closes in the no-engine smoke test. Complete
  host-native CLI/app staging passes runtime integrity and version smoke checks.

At B01 the desktop was a configuration runner and the writer remained legacy.
The continuation below replaces those host paths. Linux desktop execution,
privileged cancellation on a real write,
Windows API execution and physical-board boot behavior are not certified here.
An upstream `block` 0.1.6 future-compatibility warning remains in the pinned
Dioxus dependency graph; current compiler checks pass.

### Completed native host continuation

- The merged native suite passes 299 tests with zero failures. Two expensive
  isolated mutation entry points are skipped by the ordinary suite and both
  pass when explicitly invoked, including their isolated cross-target checks.
- The FFU engine passes 83 tests, 225 parser/geometry and 33 writer oracle cases,
  and 19 isolated guard mutations. Its provenance is embedded directly from the
  canonical manifest; no approval override was added.
- IoT acquisition/account services pass 42 unit tests, two credential Debug
  rejection doctests, two native protocol executables and seven isolated mutants.
- Desktop preparation passes 64 tests and four isolated mutants. The writer
  consumes its complete private tree and propagates source/mount cleanup errors
  before reporting success.
- Source-compatible configuration and native topology/identity tests cover
  explicit false/empty values, update/sound precedence, exact capacity/build
  thresholds, Linux layered roots and macOS external APFS physical stores.
  CLI/desktop share source-compatible host regional settings and sound fallback;
  terminal IoT choices also request independent erase/full-wipe consent when
  every other setting is already complete.
- Five additional parent integration mutations prove root exclusions, target
  identity, active-write/Accessibility alert gating, preview consent and the
  independent experimental-write opt-in.
- The guided Dioxus webview mounts successfully. A sparse native-only data
  fixture executes without source scripts; native helpers preserve status/JSON
  and avoid credential arguments. The final native package excludes generated
  source build caches and is around 70 MiB with matching sources and runtime
  notices, not a multi-gigabyte cache dump.

Physical native writes are **explicitly experimental**, not production-qualified.
The CLI requires `--allow-native-write`; the GUI requires separate native-risk,
erase and (where applicable) full-wipe consent. Root/source/runtime/cache/log
disks are excluded again before destructive work. No physical device was flashed.

The first MSVC linking route required SDK-license approval, which was unavailable;
no Microsoft SDK was downloaded or license accepted implicitly. That software
blocker was resolved by a real macOS cross-link using the checksum-verified
LLVM-MinGW 20261020 release and Rust's `aarch64-pc-windows-gnullvm` target.
The initial non-system `libunwind.dll` dependency was detected and removed through
static runtime linkage. The packaged candidate verifies ARM64 PE32+ console
headers, ASLR/DEP and a strict Windows-system DLL import set, and carries its
source and Rust/LLVM/MinGW notices. CI now repeats the open-source cross-link on
both build hosts; the optional MSVC job remains an additional unexecuted check.

The candidate manifest explicitly says Not Assessed and not installed by default.
The Go PE payload and its source/license remain until ARMv8.0/WoR-PE qualification
can safely replace them. macOS signing/notarization likewise needs operator
credentials; stable release promotion was not performed.

**Conclusion:** Dioxus + Clap over a shared Rust domain/application/backend is
the implemented native host architecture. Software replacements, including an
actual linked Windows helper candidate, are complete. Remaining release gates are
real platform/hardware qualification, signing and deliberate release promotion,
not another Bash/Python wrapper disguised as a Rust conversion.
