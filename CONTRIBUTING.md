# Contributing

WoR-Flasher was created by Botspot and is directly maintained by Blackout
Secure. Preserve the original authorship, project links and community support
paths. Keep changes focused, explain user-visible behavior and submit work as a
pull request to [blackoutsecure/wor-flasher](https://github.com/blackoutsecure/wor-flasher).

## Development setup

```bash
git clone https://github.com/blackoutsecure/wor-flasher
cd wor-flasher
```

Install Rust through [rustup](https://rustup.rs/). The committed
`rust-toolchain.toml` pins the compiler and components; `Cargo.lock` pins the
dependency graph. Node.js 20 or newer is required for package metadata,
release tooling and offline contracts. Desktop builds need the native
WebKitGTK/XDo/AppIndicator development libraries on Linux.
The release matrix is Linux x64/ARM64 and universal macOS Intel/Apple Silicon.
Raspbian ARMv7 has a CLI cross-build check, not a qualified desktop release.
Windows runs shared domain tests and the separate Pi boot-helper build only;
it does not yet have a host GUI/CLI backend. Do not remove host guards to make
a build appear supported.

## Validation

```bash
cargo fmt --all -- --check
cargo test --locked --workspace --exclude wor-desktop
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked -p wor-desktop --features visual-fixtures --bin wor-desktop
cargo clippy --locked -p wor-desktop --features visual-fixtures --all-targets -- -D warnings

npm ci --ignore-scripts --no-audit --no-fund
npm run check
node src/package-rust.mjs --desktop --check
```

For universal macOS packages, install both `aarch64-apple-darwin` and
`x86_64-apple-darwin` targets, then add `--universal` to the package command.
The packager verifies both Mach-O slices and preserves the macOS minimum
deployment target. Test on both native architectures in CI.

`npm run check` runs the Node syntax, metadata, payload and packaging contracts
without physical devices. `npm test` also runs the browser-based visual suite;
install the browsers with `npx playwright install --with-deps chromium firefox
webkit` first.

The optional Rust Pi 3 boot-finalizer candidate can be checked with
`rustup target add aarch64-pc-windows-msvc` and:

```bash
cargo test --locked -p wor-pi3-boot-refresh --lib --tests
cargo check --locked -p wor-pi3-boot-refresh --target aarch64-pc-windows-msvc
cargo clippy --locked -p wor-pi3-boot-refresh --all-targets -- -D warnings
```

This candidate is not installed in released media until it is qualified on
ARMv8.0 hardware and WoR-PE. Keep the existing Go executable, source, manifest
and license together until then.

## Implementation boundaries

- Put shared behavior in the Rust domain/application/infrastructure and media
  crates. CLI and desktop code should remain presentation/adaptation layers.
- Keep physical writes behind existing preview-bound consent and experimental
  opt-in. Never use real drives in automated tests.
- Treat native errors, incomplete cleanup, changed targets, missing payloads
  and interrupted writes as explicit failures, never verified success.
- Keep PowerShell files only where the Rust IoT companion compiles them as
  Windows payloads. Do not add shell or Python host-runtime fallbacks.
- Update `README.md`, metadata and focused Rust/Node tests when public behavior
  or the native runtime inventory changes.
- Preserve source snapshots, license notices and the project's original
  attribution.

## Pull requests

Include a concise summary, the relevant test commands and their results, and any
hardware or platform validation that remains outstanding. Do not claim that
software tests qualify a physical media write or the Pi 3 PE helper.
