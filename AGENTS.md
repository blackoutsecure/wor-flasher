# AGENTS.md

## What this is

WoR-Flasher is a Bash-based tool for creating bootable Windows 10 or Windows 11
ARM64 media for Raspberry Pi from Debian-based Linux or macOS. It automates the
Windows on Raspberry installation process: obtain or import Windows media, add
UEFI firmware and drivers, partition and write a selected drive, verify the
result, and eject it.

Botspot created WoR-Flasher and also created Pi-Apps. Preserve the project's
original priorities: a straightforward interactive workflow, readable Bash,
community compatibility, and one implementation of the flashing logic. The
Blackout Secure maintained source extends that direction with macOS support,
native GUI paths, stronger validation and verification, caching, automation
interfaces, and broader tests. Do not erase the original authorship or replace
the established design with a new framework.

Read `README.md` for supported hosts, Raspberry Pi models, user-facing behavior,
and parameters. Read `CONTRIBUTING.md` before changing code, `SECURITY.md` before
changing trust or validation behavior, and `NOTICE` before changing licensing or
attribution text.

## Commands

Run the narrowest applicable check first. The repository has no compilation step.

```bash
# Syntax and ShellCheck for all maintained Bash entry points, libraries and tests.
# These are the exact target lists .github/workflows/shellcheck.yml uses.
bash -n src/lib/*.sh install-wor.sh install-wor-gui.sh install-wor-hook.sh WoR-Flasher.app/Contents/MacOS/WoR-Flasher tests/*.sh
shellcheck --severity=error src/lib/*.sh install-wor.sh install-wor-gui.sh install-wor-hook.sh WoR-Flasher.app/Contents/MacOS/WoR-Flasher tests/*.sh

# Full automated suite; does not write physical media
./tests/run-tests.sh

# GUI flow in DRY_RUN mode, or Linux loop-device coverage from macOS via Docker
./tests/run-tests.sh --gui
./tests/run-tests-gui.sh
./tests/run-linux-integration.sh

# Remove loop devices and test state left by an interrupted run
./tests/run-tests.sh --clean
```

On a non-Linux host the suite prints three summaries: the Docker container's nested run,
the integration wrapper, then this host's own run. All three must report `failed 0`, and
the process exit status must be `0`. Read the summaries rather than the exit status alone
when you change the harness itself.

Never test a code change by flashing a physical device without the user's explicit
approval and selected target. For partitioning, firmware, driver, boot, or model
compatibility changes, record real hardware coverage separately when available.

## Architecture

- `install-wor.sh` is the engine and the single source of truth. Drive discovery,
  host and target validation, downloads, caching, ISO handling, partitioning,
  writing, settings summaries, and post-flash verification belong here.
- `install-wor-gui.sh` is presentation only. It sources the engine and presents
  native AppKit/JXA dialogs on macOS or `yad` dialogs on Linux. Do not duplicate
  engine functions or decisions in this file.
- Keep both GUI implementations behaviorally aligned. A setting added to one
  platform's Advanced Options UI must be added to the other platform and to the
  shared `settings_summary` in the same change.
- `install-wor-hook.sh` is the stable, machine-readable process adapter for
  external front-ends and automation. Keep commands small, preserve exit-status
  behavior, and cover interface changes with tests. It may bootstrap a complete
  checkout because the engine depends on repository assets.
- `src/lib/` holds the shared libraries every entry point loads explicitly:
  `metadata.sh` owns the canonical name, version, and asset filenames; `dependencies.sh`,
  `paths.sh`, and `cleanup.sh` own the dependency, path, and cleanup helpers. These files
  must stay safe to source and must not source one another.
- `assets/` holds the shared UI artwork used by the README, both GUIs, and the macOS app
  icon. Reference it through `WOR_ASSETS_DIR` rather than rebuilding the path.
- Both front-ends run the engine in-process and report progress through a native
  progress window. Do not reintroduce a spawned terminal emulator or launcher
  detection; the GUI must not depend on a visible terminal on either platform.
- `WoR-Flasher.app/` is the native macOS launcher for the same GUI and engine. It
  is not a second application implementation. Keep its property-list metadata and
  bundled resources synchronized with the canonical metadata.
- `config-templates/` contains required boot and setup inputs. Keep these files
  tracked and validate changes carefully; a missing or empty template can produce
  unbootable media.
- `tests/` contains static, shared-engine, dry-run, loop-device, GUI, and Docker
  integration coverage. Tests should call real engine functions through the test
  helpers rather than reimplement product logic.

## Safety invariants

This project erases storage devices. Treat target selection and verification as
safety-critical code.

- Preserve the final host, whole-disk, boot-drive, capacity, and installation-mode
  checks in the engine. Discovery output is a snapshot, not authorization to write.
- Never weaken target-drive exclusions or make an unsafe device selectable merely
  to simplify a UI or integration.
- Keep destructive operations behind explicit user confirmation unless the caller
  has deliberately supplied the documented non-interactive inputs.
- Preserve TLS and payload integrity checks, cache manifests, source-image checks,
  and post-flash partition, file, and image verification. Any bypass must remain
  explicit, documented, and disabled by default.
- Do not log passwords, tokens, unattended-setup credentials, or other secrets.
  Keep credential-bearing files limited to the explicitly enabled setup flow.
- Preserve registered cleanup for mounts, loop devices, temporary files, and
  interrupted runs. Do not replace an existing `EXIT` trap in a way that drops
  earlier cleanup handlers.
- Keep source-checkout self-updates opt-in. They may fast-forward a clean checkout
  but must refuse to overwrite uncommitted work, rewrite history, or update from
  an untrusted repository or ref.
- WSL is deliberately rejected because its visible disks are not safe flashing
  targets. Do not broaden host support without a tested device-safety design.
- Never run a real flash against physical media during automated validation. Use
  `DRY_RUN=1`, sparse files, loop devices, mocks, or the Docker integration suite.
  A real-device test requires the user's explicit approval and device selection.

## Platform rules

- Keep Linux and macOS behavior separate only where host tooling requires it; put
  shared decisions in the engine.
- Use the existing host helpers instead of inferring the platform from `DISPLAY`
  or duplicating `uname` checks. The front-end is selected explicitly.
- Linux support targets Debian-family hosts and uses Linux block-device and mount
  tooling. Loop-device tests are expected to be unavailable on macOS.
- macOS support targets macOS 13 or newer with Homebrew. Preserve `diskutil`/
  `hdiutil` discovery and mounting behavior and `sgdisk` GPT partitioning. The
  `WOR_BOOT` EFI partition must remain partition 1.
- The macOS GUI uses JXA inside quoted heredocs and shell command substitutions.
  Do not put apostrophes in those JXA heredoc bodies; an unmatched quote can hide
  later shell definitions while `bash -n` still succeeds. Use wording such as
  "does not" instead of "doesn't".
- The macOS application bundle is a launcher for the same maintained scripts, not
  a separate implementation. Preserve the complete-checkout execution model and
  its bounded preflight: repair only missing tracked files, update only by an
  approved fast-forward of a clean checkout, and install only missing Homebrew
  formulae. Never add `git reset`, `git clean`, `brew upgrade`, or a remote script
  piped into a shell.
- Model and firmware compatibility changes require authoritative upstream evidence
  plus tests. Keep known pins and model restrictions unless the relevant upstream
  issue is resolved and the replacement is tested on applicable hardware.

## Bash conventions

Follow the established Botspot style in nearby code rather than applying a generic
formatter:

- Use `#comment` with no space after `#`.
- Use the existing conditional form, for example `if [ "$value" == expected ];then`.
- Put the concise `#Input: ... Output: ...` description on a function's opening
  line where that pattern is used.
- Comment why a non-obvious choice exists; do not narrate obvious commands.
- Quote paths and expansions unless intentional word splitting is required.
- Reuse existing helpers for errors, progress, cleanup, downloads, platform
  behavior, and device handling before adding another abstraction.
- Keep changes focused. Do not perform broad style rewrites alongside behavioral
  work.

Use the established local pattern rather than introducing a generic shell style:

```bash
get_partition() { #Input: device & partition number. Output: partition /dev entry
  [ -z "$1" ] && error "get_partition(): no /dev device specified as"' $1'
  [ -z "$2" ] && error "get_partition(): no partition number specified as"' $2'
  [ ! -b "$1" ] && error "get_partition(): $1 is not a valid block device!"
  if is_macos ;then
    [ "$2" == all ] && error "get_partition(): macOS does not support listing all partitions through this helper."
    darwin_plist_json diskutil list -plist "$1" | jq -er ".AllDisksAndPartitions[0].Partitions[$(($2 - 1))].DeviceIdentifier" | sed 's+^+/dev/+'
    return
  fi
  command -v lsblk >/dev/null || error "get_partition(): lsblk is required."
}
```

Two Bash-specific hazards are documented in `CONTRIBUTING.md`: do not edit a script
while it is running, because Bash reads incrementally by byte offset, and do not
try to `wait` for a sibling process from a subshell.

## Git workflow

This checkout is the Blackout Secure maintained fork. `origin` is
`blackoutsecure/wor-flasher`, whose default branch is `main`; `upstream` is
Botspot's original `Botspot/wor-flasher`, tracked so original work can be merged
in and changes can be offered back. The working branch is `patch-1`.

- Land changes through a pull request against `blackoutsecure/wor-flasher`, as
  `CONTRIBUTING.md` directs. Do not push straight to `main`.
- One logical change per pull request, with original attribution preserved.
- Say what you tested it on, and be honest about what you could not test.
- Keep the README, the `install-wor.sh` version history, and any release metadata
  in the same commit as the behavior change they describe.

## Boundaries

### Always

- Keep `install-wor.sh` as the shared decision-making engine and test behavior
  through its real functions rather than reimplementing it in tests.
- Run the relevant commands above, mutation-test new coverage, and report what
  was tested and what could not be tested.
- Update `README.md` and the version history at the top of `install-wor.sh` with
  user-visible behavior changes. For a release, update `WOR_FLASHER_VERSION` in
  `src/lib/metadata.sh` and both version fields in
  `WoR-Flasher.app/Contents/Info.plist`; the suite checks all release metadata.
- Preserve Botspot's original authorship, contributor credit, project links, and
  community support paths.

### Ask first

- Changing target-device safety checks, integrity verification, default bypasses,
  supported hosts, or Raspberry Pi and firmware compatibility.
- Adding or changing a public hook command, its output, or its exit-status contract.
- Adding dependencies, modifying the macOS app bundle's update/repair behavior,
  or changing where canonical metadata is sourced.
- Editing licensing, attribution, third-party component notices, or generated
  configuration templates.

### Never

- Remove, bypass, or weaken boot-drive protection, final validation, integrity
  checks, credential redaction, cleanup registration, or the deliberate WSL block.
- Duplicate engine logic in a GUI or make the Linux and macOS Advanced Options
  flows diverge.
- Add apostrophes inside the quoted JXA heredocs, edit a running Bash script, or
  use destructive repair/update commands such as `git reset`, `git clean`, or
  `brew upgrade` in the macOS launcher.
- Commit generated Windows media, downloaded firmware or drivers, caches, logs,
  credentials, mounted-image contents, or test workspaces.
- State that the upstream licensing position is settled; follow `NOTICE` instead.
