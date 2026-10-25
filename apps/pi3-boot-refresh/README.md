# Native Pi 3 boot finalizer (qualification pending)

This crate is a source-anchored Rust replacement candidate for
`src/pi3-boot-refresh/bootstrap.go` and `main_windows.go`. It does **not** replace
the shipped Go `Pi3BootRefresh.exe`, its manifest/license, the prefinalize hook or
release build tooling. Botspot's original project authorship and the repository's
existing attribution and [NOTICE](../../NOTICE) remain unchanged.

## Runtime contract

The Windows console entry point consumes the same `WOR_*` environment variables,
not a device-path argument. Only an explicitly selected Pi 3 desktop ARM64/GPT
installation may reach raw-device access. Pi 4/5 and Pi 3 MBR installations return
the original not-applicable message without accessing a disk.

For applicable installations it requires Windows PE (`HKLM\...\MiniNT`), the
installed Windows kernel and Pi 3 firmware, identifies the selected physical
disk's first mounted partition, binds both volume extents and verifies the raw
whole-disk identity. WoR-PE 1.1's duplicate exported drive letters trigger mounted
volume discovery, never adjacent-letter guessing.

The pure library streams and checks primary GPT header/table CRCs and bounds,
matches both partition extents and types, and validates FAT32 geometry and
relocated BPB limits. It makes a **protective-only**, not hybrid, MBR. A changed
bootstrap requires a unique flushed backup in
`<Windows drive>\Windows\Logs\WoR-Flasher`, complete metadata revalidation, one
512-byte sector-zero write, a flush and verified readback. Errors never trigger a
retry or automatic rollback. An already matching bootstrap makes no backup or
write.

`WoR-Event|LogInfo/LogWarn/LogDebug/LogFatal|...` output and exit statuses retain the
Go helper's behavior; message pipes and line breaks are sanitized. The
**non-Windows executable always refuses with status 1**, including for unrelated
boards. It is not a simulated successful finalizer.

## Boundaries and APIs

- `parse_context`, `select_boot_drive`, `parse_volume_extent`,
  `validate_disk_number` and `DiskLayout::validate`: typed, pure guards.
- `build_bootstrap` and `refresh_bootstrap`: generic `ReadAt`/`BootstrapDisk`
  adapters; no host device-opening implementation exists on macOS.
- `workflow::Platform`, `Volume` and `PhysicalDisk`: testable Windows orchestration,
  including re-identification, flush and close-error handling.
- `files`: regular-file readiness and durable create-new backups.
- `windows.rs`: private selected-volume/disk handles and the only unsafe boundary:
  checked synchronous `DeviceIoControl`, `CloseHandle`, `RegOpenKeyExW`,
  `RegCloseKey` and `GetLogicalDrives`. Every IOCTL output has a fixed, checked
  length before decoding. Raw writes are additionally restricted to one sector
  at offset zero in the Windows adapter.

## Offline verification

Use the workspace's pinned Rust toolchain:

```sh
cargo test -p wor-pi3-boot-refresh --lib --tests
rustup target add aarch64-pc-windows-msvc
cargo check -p wor-pi3-boot-refresh --target aarch64-pc-windows-msvc
cargo clippy -p wor-pi3-boot-refresh --all-targets -- -D warnings
cargo clippy -p wor-pi3-boot-refresh --target aarch64-pc-windows-msvc -- -D warnings
cargo fmt -p wor-pi3-boot-refresh --check
```

The helper also cross-links from macOS/Linux without Microsoft's SDK using
[LLVM-MinGW](https://github.com/mstorsjo/llvm-mingw). CI verifies the pinned
20261020 archive's SHA-256 before using it. With the extracted toolchain installed:

```sh
rustup target add aarch64-pc-windows-gnullvm
export LLVM_MINGW_ROOT=/absolute/path/to/llvm-mingw-20261020-ucrt-macos-universal
node src/package-rust.mjs --desktop --pi3-helper
```

This builds the native host package plus
`candidates/pi3-boot-refresh/Pi3BootRefresh.exe` and a qualification/import manifest.
The build fixes the generic CPU baseline, static runtime linkage and deterministic
PE timestamp, remaps source paths, verifies ARM64/PE32+/console/ASLR/DEP and rejects
non-system DLL dependencies such as `libunwind.dll`. Rust runtime, LLVM and MinGW
notices accompany the corresponding source. No SDK is downloaded or license
accepted by this command. The candidate is not installed by the media writer;
cross-linking is not proof of ARMv8.0, Windows PE or physical-board behavior.

Synthetic byte fixtures cover invalid contexts, IOCTL layouts, GPT/FAT guards,
backup races, partial/failed writes, flushing, idempotence and readback.
Windows orchestration is exercised through in-memory fake handles, including
close and discovery failures. All tests use the Rust implementation and
synthetic fixtures; no host scripting runtime is involved.

These checks do not execute Windows APIs, prove Windows PE privileges/volume
semantics, qualify an executable for Pi 3 ARMv8.0 or replace an end-to-end WoR-PE
hardware installation. Any cross-linked executable is an unqualified ignored
build artifact, not a promoted runtime. Hardware qualification and packaging
switch-over remain separate work.
