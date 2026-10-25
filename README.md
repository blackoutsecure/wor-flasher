# WoR-Flasher

[![CI](https://img.shields.io/github/actions/workflow/status/blackoutsecure/wor-flasher/rust.yml?style=for-the-badge&labelColor=0b1829&logo=githubactions&logoColor=ffffff&color=1158af&label=CI)](https://github.com/blackoutsecure/wor-flasher/actions/workflows/rust.yml)
[![License](https://img.shields.io/github/license/blackoutsecure/wor-flasher?style=for-the-badge&labelColor=0b1829&color=1158af)](LICENSE)

WoR-Flasher prepares Windows 10/11 ARM64 installation media for Raspberry Pi
from Debian-family Linux and macOS. The command-line interface, desktop
application and host-side imaging implementation are Rust. There is no shell or
Python host application or fallback.

## Install

Download the Linux or universal macOS archive matching your host from
[Releases](https://github.com/blackoutsecure/wor-flasher/releases), together
with `SHA256SUMS`. Verify and extract the archive:

```bash
shasum -a 256 -c SHA256SUMS
mkdir wor-flasher
tar -xzf wor-flasher-<version>-<platform>-<architecture>.tar.gz -C wor-flasher
```

Run the guided desktop workflow or inspect the CLI:

```bash
./wor-flasher/bin/wor-flasher wizard
./wor-flasher/bin/wor-flasher --help
```

The package contains the CLI, desktop app, digest-verified runtime, matching
source snapshot, third-party notices and checksums. Keep the extracted directory
together. On macOS, open `WoR-Flasher.app` in the package or run the CLI. The
app has no Developer ID signature or notarization. Linux desktop use requires the
WebKitGTK runtime; both systems require the image and disk utilities described
under [Build and test](#build-and-test).

## Supported hosts and media

- Ubuntu/Debian-family Linux and macOS 13 or newer. Build/release coverage:

  | Host | Architecture | Coverage |
  | --- | --- | --- |
  | Ubuntu 22.04+ | x86_64 and ARM64 | Release builds on 22.04; native GUI/CLI CI on 24.04 |
  | Raspberry Pi OS Bookworm+, 64-bit | ARM64 | Linux ARM64 package with glibc 2.35 baseline; requires WebKitGTK 4.1/system libraries; physical Pi testing is Not Assessed |
  | Raspberry Pi OS / Raspbian, 32-bit | ARMv7 | CLI cross-build check only; no 32-bit GUI release or runtime qualification yet |
  | macOS 13+ | Intel x86_64 and Apple Silicon ARM64 | Universal CLI and app; both slices checked, native CI jobs for both architectures |
  | Windows | x86_64 | Shared domain/application checks only; no Windows host GUI/CLI or flashing backend |

  Linux packages are built on Ubuntu 22.04 and are not standalone static
  binaries. Raspberry Pi OS must provide compatible glibc and WebKitGTK
  libraries; a matching CPU alone does not establish runtime compatibility.
- Raspberry Pi 3, 4 and 5 for Windows ARM64 desktop media, subject to the
  documented Windows build and firmware compatibility limits.
- The reviewed Windows 10 IoT Core ARM32 workflow for supported legacy Pi
  models.
- Windows host support still requires a native device-discovery, volume-locking,
  elevation, imaging-tool and cancellation backend. The Windows ARM64 Pi boot
  helper is a target-side payload, not a Windows host application.
- Windows, WSL and non-Debian Linux hosts are unsupported. WSL is deliberately
  rejected because its visible virtual disks are unsafe flashing targets.

The application can acquire, inspect, prepare and verify media. Its native
physical writer is experimental and remains disabled unless explicitly
authorized. The desktop requires a fresh preview, experimental-risk acceptance
and erase confirmation. The CLI additionally requires `--allow-native-write`.
This opt-in is not hardware qualification; do not use physical media unless you
accept that status.

## Runtime boundaries

Rust owns configuration, validation, image and firmware preparation, storage
safety, desktop/FFU writing, acquisition, SSH personalization, the CLI and the
Dioxus desktop interface. The same Rust domain and application logic serve both
frontends.

Two Windows payloads remain because the Rust host uses them:

- The existing Go `Pi3BootRefresh.exe`, its manifest and license are installed
  into WoR-PE for Pi 3 boot finalization. A Rust replacement candidate exists,
  but is not promoted until it passes ARMv8.0/WoR-PE hardware qualification.
- The PowerShell account and language scripts are compile-time inputs to the
  native IoT companion; they are not host-side application code.

Node.js scripts are maintainer tooling for metadata, payload verification,
release packaging and visual tests. Node.js is not required to run the released
application. The native package does not contain Bash, Python, or a host-side
shell engine.

## Build and test

Install the pinned toolchain with [rustup](https://rustup.rs/), then use the
committed `rust-toolchain.toml` and `Cargo.lock`:

```bash
cargo fmt --all -- --check
cargo test --locked --workspace --exclude wor-desktop
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked -p wor-desktop --features visual-fixtures --bin wor-desktop
cargo clippy --locked -p wor-desktop --features visual-fixtures --all-targets -- -D warnings
```

For Node.js packaging and offline contracts, use Node.js 20 or newer:

```bash
npm ci --ignore-scripts --no-audit --no-fund
npm run check
node src/package-rust.mjs --desktop --check
```

On macOS, build the universal package (not just the current host architecture):

```bash
rustup target add aarch64-apple-darwin x86_64-apple-darwin
node src/package-rust.mjs --desktop --universal --check
node src/package-rust.mjs --desktop --universal
```

Both `bin/` executables and both executables in the app bundle are assembled
with `lipo`, checked for distinct Intel/Apple Silicon slices and ad-hoc signed.
Ad-hoc signing is not a Developer ID signature or notarization. The macOS
release is named `wor-flasher-<version>-darwin-universal.tar.gz`; Linux releases
are `linux-x64` and `linux-arm64`. CI build checks are not physical-media or
Raspberry Pi boot qualification.

The package build needs Cargo and `rustc`. Linux desktop builds need
`libwebkit2gtk-4.1-dev`, `libxdo-dev` and `libayatana-appindicator3-dev`.
Runtime image preparation uses native host tools including `wimlib-imagex` and
`bsdtar`; Linux also uses system disk, partition and mount utilities. Tests use
synthetic files and mocks; automated validation never writes physical media.

The optional Rust Pi 3 Windows ARM64 candidate can be built with the pinned
LLVM-MinGW toolchain by setting `LLVM_MINGW_ROOT` and adding the
`aarch64-pc-windows-gnullvm` Rust target:

```bash
node src/package-rust.mjs --desktop --pi3-helper --check
```

Cross-linking and synthetic tests do not qualify it for WoR-PE or a physical
Raspberry Pi.

## Support and attribution

WoR-Flasher was created by **[Botspot](https://github.com/Botspot)**, who also
created [Pi-Apps](https://github.com/Botspot/pi-apps) and
[BVM](https://github.com/Botspot/bvm). **[Blackout Secure](https://blackoutsecure.app/)**
directly maintains this fork in cooperation with Botspot and the wider Windows
on Raspberry community. The native Rust migration preserves the original
authorship, project links and GPL-3.0-or-later license; read [NOTICE](NOTICE)
for the upstream licensing caveat and [LICENSE](LICENSE) for the license text.

- [Report a bug or request a feature](https://github.com/blackoutsecure/wor-flasher/issues)
- [Private vulnerability reporting](SECURITY.md)
- [Botspot Software Discord](https://discord.gg/RXSTvaUvuu)
- [WoR project](https://worproject.com/)
- [Sponsor Botspot](https://github.com/sponsors/Botspot)
- [Support Blackout Secure](https://github.com/sponsors/blackoutsecure?frequency=one-time&amount=8)

## Version

The product version is defined in [`src/config/metadata.json`](src/config/metadata.json)
and synchronized to `package.json`. Release notes are published with each
version in [GitHub Releases](https://github.com/blackoutsecure/wor-flasher/releases).

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for development setup, tests and pull
request guidance.
