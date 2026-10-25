# WoR-Flasher repository guidance

WoR-Flasher prepares Windows ARM64 media for Raspberry Pi. The product is
implemented in Rust: CLI, Dioxus desktop, domain rules, host operations and
native media workflows. Do not reintroduce a shell or Python product runtime,
fallback, installer, or GUI.

Botspot created WoR-Flasher and Pi-Apps. Preserve original authorship, project
links, community support and attribution. Read [README.md](README.md) for
supported hosts, platforms and the current qualification status. Read
[CONTRIBUTING.md](CONTRIBUTING.md) before changing code, [SECURITY.md](SECURITY.md)
before changing trust or validation, and [NOTICE](NOTICE) before changing
licensing or attribution.

## Commands

Run focused checks first. Tests must never write physical media.

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

`npm run check` runs Node syntax checks, offline metadata/payload/package
contracts and the Go Pi 3 payload integrity check. `npm test` also runs browser
visual tests and needs the configured Playwright browsers. Linux desktop builds
need WebKitGTK, XDo and Ayatana AppIndicator development libraries. The Rust
Pi 3 helper candidate is optional; cross-linking it does not qualify hardware.
Release CI builds Linux x64/ARM64 and universal macOS packages. Universal
packaging uses `--desktop --universal` and requires both Apple Rust targets.
ARMv7 Raspberry Pi OS currently has a CLI cross-build check only; Windows host
support is not implemented. Windows domain tests and the Pi boot-helper build
must never be presented as a Windows host application qualification.

Never test a code change by flashing physical media. Use fixture files, mocks,
test providers and synthetic devices. Real-device testing requires the user's
explicit approval and a specifically selected target.

## Architecture

- `crates/domain` owns typed configuration, compatibility, boot and storage
  invariants; it must not depend on UI or OS providers.
- `crates/application` owns shared requests, settings, preview-bound consent
  and operation results.
- `crates/infrastructure` owns native process supervision, runtime validation,
  OS/device discovery and worker protocol.
- `crates/desktop-media`, `crates/ffu`, `crates/iot-media` and
  `crates/iot-account` own native image preparation, bounded media writing,
  acquisition and post-boot SSH services.
- `apps/cli` and `apps/desktop` are the Rust presentation boundaries. Both use
  the same application and infrastructure implementations.
- `apps/pi3-boot-refresh` is an unpromoted Rust replacement candidate for the
  Pi 3 Windows PE helper. Keep the shipped Go executable, manifest, source and
  license until the replacement passes ARMv8.0/WoR-PE hardware qualification.
- `src/lib/iot-account.ps1`, `iot-locale.ps1` and `iot-locale-user.ps1` are
  compile-time Windows payloads included by the Rust IoT companion; they are
  not host application code. `config-templates/` contains required inputs
  consumed by Rust media preparation.
- `src/config/metadata.json` owns product metadata, platform defaults and the
  native runtime inventory. `src/package-rust.mjs` creates packages and
  corresponding-source snapshots. Other Node scripts are maintainer tools;
  they are not application runtime dependencies.
- `assets/` contains shared product artwork. `tests/visual/` and
  `apps/desktop/src/visual_fixtures.rs` exercise the actual Rust UI.

## Safety invariants

The application erases storage devices. Treat target selection and verification
as safety-critical.

- Preserve host, whole-disk, boot-disk, capacity, image-family and installation
  mode checks. Device discovery is not write authorization.
- Keep destructive operations behind explicit confirmation. Native physical
  writes require the existing experimental opt-in, fresh target-bound preview
  and erase consent; never enable them in automated tests.
- Preserve source/download hashes, pinned image manifests, cache integrity,
  target identity binding and mandatory post-write read-back.
- Never log passwords, tokens, unattended-setup credentials or private SSH
  state. Preserve private temporary files, strict host-key checks and credential
  redaction.
- Preserve cleanup ownership for mounts, worker processes and temporary files.
  Cleanup failure or interrupted writes must never report verified success.
- Keep Windows and WSL unsupported. Do not broaden host support without a
  tested device-safety design.
- The shipped Go Pi 3 PE helper remains the installed payload until its Rust
  candidate is hardware-qualified. A successful build or synthetic test is not
  qualification.
- Do not add a host shell/Python bridge or silently retry through another
  writer. Surface native failures explicitly.

## Change boundaries

- Changes to target safety, image trust, write consent, supported platforms,
  model/firmware compatibility or credential handling need explicit review and
  focused tests.
- Keep compatibility rules and destructive decisions in shared Rust crates,
  not separately in the CLI or desktop.
- Update user-facing documentation and Rust/Node contracts when behavior or
  runtime inventory changes.
- Preserve exact first-party source snapshots and third-party notices in
  release packages. Do not include build caches, credentials, generated test
  media or ignored evidence.
- Preserve original authorship and follow the upstream licensing caveat in
  [NOTICE](NOTICE). Do not change license or attribution text without review.
- Keep every automated validation offline and independent of physical devices.

## Git workflow

This maintained fork is `blackoutsecure/wor-flasher`; the original project is
`Botspot/wor-flasher`. Submit changes through a pull request against the
maintained repository. Do not push directly to `main`. Do not create or move a
release tag by hand.
