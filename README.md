# ![WoR-Flasher icon](logo.png) WoR-Flasher

[![Version](https://img.shields.io/badge/version-1.0.0-0a7ea4?style=for-the-badge&labelColor=555555&logo=semanticrelease&logoColor=ffffff)](#versions)
[![CI](https://img.shields.io/github/actions/workflow/status/blackoutsecure/wor-flasher/shellcheck.yml?style=for-the-badge&labelColor=555555&logo=githubactions&logoColor=ffffff&color=0a7ea4&label=CI)](https://github.com/blackoutsecure/wor-flasher/actions/workflows/shellcheck.yml)
[![License](https://img.shields.io/badge/license-GPL--3.0-0a7ea4?style=for-the-badge&labelColor=555555&logo=gnu&logoColor=ffffff)](LICENSE)
[![Platform](https://img.shields.io/badge/host-Linux%20%7C%20macOS-0a7ea4?style=for-the-badge&labelColor=555555&logo=linux&logoColor=ffffff)](#requirements)
[![Shell](https://img.shields.io/badge/written%20in-bash-0a7ea4?style=for-the-badge&labelColor=555555&logo=gnubash&logoColor=ffffff)](install-wor.sh)

[![Upstream](https://img.shields.io/github/stars/Botspot/wor-flasher?style=for-the-badge&labelColor=555555&logo=github&logoColor=ffffff&color=0a7ea4&label=upstream)](https://github.com/Botspot/wor-flasher)
[![Discord](https://img.shields.io/badge/Discord-Botspot%20Software-5865F2?style=for-the-badge&labelColor=555555&logo=discord&logoColor=ffffff)](https://discord.gg/RXSTvaUvuu)
[![Sponsor](https://img.shields.io/badge/sponsor-Botspot-EA4AAA?style=for-the-badge&labelColor=555555&logo=githubsponsors&logoColor=ffffff)](https://github.com/sponsors/Botspot)
[![Blackout Secure](https://img.shields.io/badge/maintained%20by-Blackout%20Secure-0a7ea4?style=for-the-badge&labelColor=555555&logo=shieldsdotio&logoColor=ffffff)](https://blackoutsecure.app)

Create a bootable Windows 10 or Windows 11 ARM64 drive for a Raspberry Pi, from Linux or macOS.

WoR-Flasher downloads or imports Windows, adds the required UEFI firmware and available drivers, writes the target drive, and verifies the finished result. It automates the manual process described in worproject's [How to install from other OSes](https://worproject.com/guides/how-to-install/from-other-os) guide.

> [!WARNING]
> Flashing erases the selected drive. Check the device carefully, keep the computer powered, and do not remove the drive until verification and ejection finish.

> [!NOTE]
> This is the [Blackout Secure fork](https://github.com/blackoutsecure/wor-flasher) of [Botspot/wor-flasher](https://github.com/Botspot/wor-flasher). It adds macOS host support, native progress windows, post-flash verification and a test suite. Upstream [is looking for a maintainer](https://github.com/Botspot/wor-flasher#-this-repository-is-looking-for-a-maintainer) — if a fix here also applies there, please send it upstream too.

---

## Table of contents

- [Compatibility](#compatibility)
- [Requirements](#requirements)
- [Install](#install)
- [Usage](#usage)
  - [Graphical interface](#graphical-interface)
  - [Terminal interface](#terminal-interface)
  - [Non-interactive use](#non-interactive-use)
- [Parameters](#parameters)
- [Application setup](#application-setup)
  - [Existing Windows ISO](#existing-windows-iso)
  - [Pi 4 RAM unlock](#pi-4-ram-unlock)
  - [Offline Windows setup](#offline-windows-setup)
  - [Customization templates](#customization-templates)
  - [WoR-PE options](#wor-pe-options)
  - [Download cache](#download-cache)
- [What to expect](#what-to-expect)
- [Updating](#updating)
- [Troubleshooting](#troubleshooting)
- [Development](#development)
- [Support](#support)
- [Related resources](#related-resources)
- [Versions](#versions)
- [Contributing](#contributing)
- [Contributors](#contributors)
- [License](#license)

---

## Compatibility

| Raspberry Pi         | Newest usable Windows 11 | Limitations                                                                                                                                                                                   |
| -------------------- | ------------------------ | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Pi 2 v1.2, Pi 3, CM3 | 23H2 (`22631.x`)         | No Wi-Fi or graphics acceleration; Windows 10 is usually faster                                                                                                                               |
| Pi 4, Pi 400         | 23H2 (`22631.x`)         | No Wi-Fi or graphics acceleration; RAM is limited to 3 GB by default                                                                                                                          |
| CM4                  | 23H2 (`22631.x`)         | Known to freeze at the UEFI boot screen on some units ([pftf/RPi4#146](https://github.com/pftf/RPi4/issues/146)); USB requires the RAM limit set to 1 GB, and PCIe does not work              |
| Pi 5                 | 25H2 and newer           | Community/unofficial support only ([worproject FAQ](https://worproject.com/faq#is-raspberry-pi-5-or-newer-supported)); no native Pi hardware drivers; USB Ethernet is required for networking |

Pi 3 and Pi 4 cannot run builds newer than `25163`, because those builds require ARMv8.1 atomics. WoR-Flasher rejects incompatible builds. Windows versions that run on these models are past end of support and should be treated as experimental or offline systems.

## Requirements

| Requirement  | Detail                                                                                                                    |
| ------------ | ------------------------------------------------------------------------------------------------------------------------- |
| Host OS      | Raspberry Pi OS, Debian, Ubuntu, Linux Mint or another Debian-based Linux; or macOS 13+ with [Homebrew](https://brew.sh/) |
| Privileges   | Administrator or `sudo` access                                                                                            |
| Network      | Internet access, unless every required file is already cached                                                             |
| Free space   | About 10 GB in the download directory                                                                                     |
| Target drive | At least 8 GB                                                                                                             |
| Display      | A desktop session, for the graphical interface only                                                                       |

Dependencies are installed automatically on supported hosts.

Windows, WSL and non-Debian Linux distributions are **not** supported. WoR-Flasher deliberately refuses to run under WSL: WSL cannot reach USB drives directly, and the drives it does list are WSL's own virtual disks, so erasing one would damage the WSL installation. On Windows, use the official [Windows on Raspberry Imager](https://worproject.com/downloads) instead.

## Install

```bash
git clone https://github.com/blackoutsecure/wor-flasher
cd wor-flasher
./install-wor-gui.sh
```

Use the complete repository. Neither script is designed to be downloaded on its own or piped into Bash.

To install the original upstream version instead, clone `https://github.com/Botspot/wor-flasher`. On a Raspberry Pi, the [Pi-Apps app store](https://github.com/Botspot/pi-apps) packages upstream WoR-Flasher with a Start menu entry and one-click uninstall.

## Usage

There are two front-ends over one engine:

- **`install-wor.sh`** holds all of the logic — drive detection, download and cache handling, ISO validation, partitioning, flashing and verification.
- **`install-wor-gui.sh`** sources it and adds only the windows. It collects your choices in native dialogs, then runs `install-wor.sh` and shows a native progress window (AppKit on macOS, `yad` on Linux) instead of a visible terminal.

Both therefore write identical media from identical settings.

### Graphical interface

```bash
./install-wor-gui.sh
# or, equivalently:
./install-wor.sh --gui
```

The front-end is never chosen automatically. `DISPLAY` is also set over SSH and in CI, and a tool that erases a drive should do exactly what it was asked to do.

An **Advanced Options** window is reachable from the confirmation screen on both platforms. It exposes every configuration-only option as a checkbox, plus an editable `config.txt`: [offline Windows setup](#offline-windows-setup), the [Pi 4 RAM unlock](#pi-4-ram-unlock), whether to use the latest UEFI firmware or drivers instead of the tested pinned versions (the pinned version is shown in each label), whether to skip the final written-image verification, and dry run. `APPLY_CUSTOM_CONFIG_TXT` controls whether the editable `config.txt` is applied at all; unchecking it dims the editor and leaves the UEFI firmware package's own default in place. A **Downloaded files** menu selects the [cache mode](#download-cache), since it has three settings rather than two.

Administrator access is requested through a native password dialog on both platforms — there is no terminal to type into.

### Terminal interface

```bash
./install-wor.sh
```

```text
Usage: install-wor.sh [--gui]

  (no arguments)  run the interactive text-mode installer
  --gui           run the graphical front-end instead
  --version       print the version and exit
  --help          show this message
```

The selected drive is erased. Drives from 8 GB to under 25 GB can create recovery media for another drive. Drives of 25 GB or more can also install Windows onto themselves. The host's current boot drive is always excluded.

### Non-interactive use

`install-wor.sh` is designed to be driven from a larger script. Set the variables it would otherwise ask about and it will not prompt:

```bash
set -a
source ~/wor-flasher/install-wor.sh source

BID="$(get_bid 11)"        # newest Windows 11 build this Pi model can run
RPI_MODEL=4
WIN_LANG=en-us
DEVICE=/dev/sda
CAN_INSTALL_ON_SAME_DRIVE=1

~/wor-flasher/install-wor.sh
```

Sourcing with the `source` argument makes the engine's functions available without running a flash. Useful ones include `list_devs`, `list_dev_paths`, `drive_capability`, `describe_device`, `get_bid`, `get_os_name`, `list_langs`, `validate_iso_file`, `list_cached_winfiles`, `settings_summary` and `install_packages`.

## Parameters

Every prompt has a matching environment variable.

| Variable                    | Default               | Function                                                                                    |
| --------------------------- | --------------------- | ------------------------------------------------------------------------------------------- |
| `DL_DIR`                    | `~/wor-flasher-files` | Where components are downloaded and Windows images are extracted                            |
| `RPI_MODEL`                 | _ask_                 | Target Raspberry Pi: `3`, `4` or `5`                                                        |
| `BID`                       | _ask_                 | Exact Windows build ID, e.g. `22631.2861`                                                   |
| `WIN_LANG`                  | _ask_                 | Windows language code, e.g. `en-us`                                                         |
| `DEVICE`                    | _ask_                 | Target drive, e.g. `/dev/sda` or `/dev/disk4`                                               |
| `CAN_INSTALL_ON_SAME_DRIVE` | _ask_                 | `1` to install Windows onto the target itself, `0` to make recovery media for another drive |
| `SOURCE_FILE`               | unset                 | Path to an existing Windows ARM64 ISO, instead of downloading                               |
| `CONFIG_TXT`                | shipped template      | Body of `config.txt` written to the boot partition                                          |
| `APPLY_CUSTOM_CONFIG_TXT`   | `1`                   | `0` leaves the UEFI firmware package's own `config.txt` in place                            |
| `OOBE_NETWORK_BYPASS`       | `1`                   | `0` requires the standard network-connected Windows setup flow                              |
| `PI4_AUTO_DISABLE_3GB`      | `1`                   | Pi 4 only. `0` keeps the 3 GB RAM limit                                                     |
| `UEFI_USE_LATEST`           | `0`                   | `1` queries GitHub for the newest UEFI firmware instead of the pinned version               |
| `DRIVERS_USE_LATEST`        | `1`                   | `0` uses the pinned driver package version                                                  |
| `SKIP_IMAGE_VERIFICATION`   | `0`                   | `1` skips post-flash verification. Not recommended                                          |
| `HIDE_EMPTY_DRIVES`         | `1`                   | `0` shows empty card-reader slots as selectable drives in WoR-PE                            |
| `USE_CACHE`                 | `1`                   | See [Download cache](#download-cache)                                                       |
| `DRY_RUN`                   | `0`                   | `1` runs every step except writing to the drive                                             |
| `WOR_LOG_FILE`              | `$DL_DIR/last-run.log` | Where a failed run's log is kept                                                            |
| `VERIFY_TLS`                | `1`                   | `0` skips TLS certificate verification, for hosts with an outdated CA bundle                |
| `NO_UPDATE`                 | `1`                   | `0` opts in to the self-updater. See [Updating](#updating)                                  |
| `RUN_MODE`                  | `cli`                 | `gui` makes the engine show graphical error dialogs                                         |
| `SKIP_PACKAGE_INSTALL`      | unset                 | `1` assumes dependencies are already present                                                |

Example:

```bash
DL_DIR=/media/pi/big-drive DEVICE=/dev/sdg RPI_MODEL=4 WIN_LANG=en-us DRY_RUN=1 ./install-wor.sh
```

## Application setup

### Existing Windows ISO

Use an official Windows ARM64 ISO containing `sources/install.wim` or `sources/install.esd`:

```bash
SOURCE_FILE=/path/to/windows-arm64.iso ./install-wor.sh
```

The build number and language are read from the filename where possible, and you are asked for them if not. Customized Windows images are not supported.

### Pi 4 RAM unlock

On Raspberry Pi 4 only, the 3 GB RAM limit is disabled automatically after WoR-PE installs Windows and reboots. This setting is ignored for every other model. To keep the limit enabled:

```bash
PI4_AUTO_DISABLE_3GB=0 ./install-wor.sh
```

Windows Setup changes the pftf `RamLimitTo3GB` firmware variable during the `specialize` pass, after the injected drivers are installed, and reboots once before OOBE so the new memory map takes effect. It also clears any BCD-level `truncatememory` cap, a separate Windows Boot Manager memory limit noted in worproject's [imager customization guide](https://worproject.com/guides/wor-imager-customization#configuration-file).

> [!IMPORTANT]
> **Set `PI4_AUTO_DISABLE_3GB=0` on Compute Module 4.** Per the [worproject FAQ](https://worproject.com/faq#does-it-work-on-the-compute-module-cm), CM4 requires the RAM limit set to 1 GB — not simply left at 3 GB — for USB to work at all, and PCIe does not work regardless. WoR-Flasher cannot distinguish a CM4 from a Pi 4/400, so do not rely on the automatic default for CM4 hardware.

### Offline Windows setup

Enabled by default. WoR-Flasher writes a minimal Microsoft unattended-setup answer file that hides the OOBE network and online-account screens, so setup can continue with a local account when Pi networking is not ready yet. It does not automate accounts, licenses, partitions or privacy choices.

```bash
OOBE_NETWORK_BYPASS=0 ./install-wor.sh  # require network
```

### Customization templates

[`config-templates/`](config-templates) holds the files injected onto the media:

| File                            | Purpose                                                      |
| ------------------------------- | ------------------------------------------------------------ |
| `pi3.config.txt`                | `config.txt` body for Pi 2 v1.2 / Pi 3                       |
| `pi4.config.txt`                | `config.txt` body for Pi 4 / Pi 400                          |
| `pi5.config.txt`                | `config.txt` body for Pi 5                                   |
| `pi4-ram-unlock.ps1`            | PowerShell action that clears the Pi 4 3 GB limit            |
| `pi4-ram-unlock-specialize.xml` | Answer-file fragment that runs the above during `specialize` |
| `oobe-network-bypass.xml`       | Answer-file fragment for offline OOBE                        |

Edit these directly to customize what gets written. Both the CLI and the GUI start from the same template, so they produce identical media. Updating your checkout picks up any changes to them.

### WoR-PE options

`HIDE_EMPTY_DRIVES` (default `1`) writes `HideEmptyDrives=1` into the cached WoR-PE `settings.ini` before each run, matching worproject's [WoR-PE package option](https://worproject.com/guides/wor-imager-customization#configuration-file) of the same name, so empty card-reader slots do not show up as selectable drives during setup.

### Download cache

Downloads are stored in `~/wor-flasher-files` by default.

| `USE_CACHE` | Behaviour                                                                     |
| ----------- | ----------------------------------------------------------------------------- |
| `0`         | Remove cached components and download them again                              |
| `1`         | Reuse cache only when its source and SHA-256 payload manifest match (default) |
| `2`         | Trust the existing cache without update or integrity checks                   |

```bash
USE_CACHE=0 ./install-wor-gui.sh
```

Mode `1` refreshes changed, missing, extra or outdated cached content. Delete `~/wor-flasher-files` when you no longer need the downloads or extracted Windows images.

## What to expect

1. Downloads and verifies the PE installer, UEFI firmware, drivers and Windows image.
2. Extracts or imports the Windows image.
3. Creates FAT32 `WOR_BOOT` and ExFAT `WOR_INSTALL` partitions.
4. Copies the startup and installation files and updates `boot.wim`.
5. Verifies the partition layout, filesystems, boot files, WIM images and the copied `install.wim` checksum.
6. Unmounts and ejects the drive.

Downloads and final verification take a long time, especially on slow SD cards. Progress is shown for long operations. **Do not remove the drive until WoR-Flasher reports success.**

Move the completed drive to the Pi and connect a display, a wired keyboard and a wired mouse. Windows Setup may restart several times; do not remove power or the drive until setup completes.

## Updating

WoR-Flasher is a git checkout, so updating is a pull:

```bash
cd wor-flasher
git pull
```

An opt-in self-updater is also built in. It is **off by default** (`NO_UPDATE=1`). When enabled it only fast-forwards a clean checkout, and refuses to touch one with uncommitted changes:

```bash
NO_UPDATE=0 ./install-wor.sh
```

Check what you are running with `./install-wor.sh --version`.

## Troubleshooting

If a flash fails from the GUI, the full log is kept at `$DL_DIR/last-run.log` — or wherever `WOR_LOG_FILE` points — and the path is shown in the error dialog. It is also listed on the confirmation screen before you start. Attach it to any bug report.

<details>
<summary><b>Rainbow screen</b></summary>

The Raspberry Pi firmware did not start UEFI. Reflash the drive and wait for verification to finish. Also update the Pi EEPROM bootloader, and avoid `UEFI_USE_LATEST=1` unless you are intentionally testing firmware.

On Pi 4, three ACT LED blinks indicate that `start4.elf` is missing, four indicate that it failed to launch, and seven indicate that `RPI_EFI.fd` is missing.

</details>

<details>
<summary><b>UEFI splash, then freeze</b></summary>

On Pi 3 or Pi 4, use Windows 11 build `22631.2861` or another compatible `22631.x` release. In UEFI, verify that `System Table Selection` is `ACPI` and that Secure Boot is disabled.

</details>

<details>
<summary><b>PXE boot, or no local boot option</b></summary>

Reflash with the default pinned UEFI firmware and wait for `Written image verified successfully`. Pi 4 UEFI v1.52 has a known microSD boot regression, so WoR-Flasher pins v1.51.

During first boot, Windows Setup creates Windows Boot Manager. If an installed system has lost that entry, open `Boot Maintenance Manager > Boot Options > Add Boot Option`, select `EFI\Microsoft\Boot\bootmgfw.efi`, and place it above the network boot entries.

</details>

<details>
<summary><b>WoR-PE says the initialization disk must be recreated</b></summary>

The installer cannot find unallocated space for the Windows target partition. Reflash with a current checkout and wait for final verification. Do not manually expand `WOR_INSTALL`; the unused space after that staging partition is required during installation.

</details>

<details>
<summary><b>Keyboard does not work in UEFI</b></summary>

Press `Esc` repeatedly immediately after power-on. Connect a wired keyboard directly to a USB 2.0 port, and disconnect hubs and unnecessary USB devices.

</details>

<details>
<summary><b>Only 3 GB of RAM on Pi 4</b></summary>

This is disabled automatically by default; see [Pi 4 RAM unlock](#pi-4-ram-unlock). To do it manually instead, set `Device Manager > Raspberry Pi Configuration > Advanced Configuration > Limit RAM to 3 GB` to `Disabled` (see the [worproject FAQ](https://worproject.com/faq#only-3-gb-of-ram-are-available-how-can-i-fix-this)). On Compute Module 4, set the limit to 1 GB instead; leaving it fully disabled or at 3 GB breaks USB.

</details>

## Development

```bash
./tests/run-tests.sh                # static checks, plus Linux integration where available
./tests/run-tests.sh --gui          # walk the GUI in DRY_RUN mode
./tests/run-tests.sh --walkthrough  # fake drives, then the CLI interactively
./tests/run-linux-integration.sh    # force the Dockerised Linux suite
shellcheck --severity=error install-wor.sh install-wor-gui.sh tests/*.sh
```

The suite creates loopback devices as stand-in drives, so nothing can be written to physical storage. Tests call the real functions out of `install-wor.sh` rather than restating their logic, which means a test cannot pass against behaviour the shipped script no longer has.

CI runs ShellCheck plus the suite on Ubuntu and macOS, and a one-model dry-run integration pass. See [CONTRIBUTING.md](CONTRIBUTING.md) for house style and for the traps that have already caught us.

## Support

| Where                                                                                 | For                                              |
| ------------------------------------------------------------------------------------- | ------------------------------------------------ |
| [This fork's issues](https://github.com/blackoutsecure/wor-flasher/issues/new/choose) | Bugs in macOS support, the GUI or the test suite |
| [Upstream issues](https://github.com/Botspot/wor-flasher/issues/new/choose)           | Bugs that also affect Botspot/wor-flasher        |
| [Botspot Software Discord](https://discord.gg/RXSTvaUvuu)                             | Real-time help with WoR-Flasher                  |
| [WoR project Discord](https://discord.gg/jQCpfVK)                                     | Windows on Raspberry, the operating system       |
| [worproject.com contact](https://worproject.com/contact)                              | The WoR developers directly                      |
| [Security policy](SECURITY.md)                                                        | Anything that should not be public               |

## Related resources

- [worproject.com](https://worproject.com/) — the upstream WoR-PE installer, UEFI firmware and drivers that WoR-Flasher assembles
- [Advanced customization guide](https://worproject.com/guides/wor-imager-customization) — the `scripts/prefinalize.cmd` hook and `settings.ini` options. Written for the official WoR imager and not verified against WoR-Flasher's headless media
- [How can I update the drivers?](https://worproject.com/faq#how-can-i-update-the-drivers) — updating drivers on an already-installed system
- [Boot partition mount utility](https://worproject.com/downloads#boot-partition-mount-utility) — mount the boot partition later to edit `config.txt` or firmware
- [PiMon](https://worproject.com/downloads#pimon) — hardware monitor (CPU temperature and so on) for Windows on Raspberry Pi
- [How to perform OS updates](https://worproject.com/guides/performing-os-updates) — using Windows Update on a WoR installation
- [BVM](https://github.com/Botspot/bvm) — Botspot's newer project: Windows 11 in a KVM virtual machine on ARM Linux, rather than on bare metal

## Versions

Upstream has never published a git tag or a GitHub release, so this fork keeps its own version line. The same history is repeated at the top of [`install-wor.sh`](install-wor.sh).

- **1.0.0** — First versioned release of this fork.
  - **macOS host support**: `diskutil`/`hdiutil` drive discovery, and `sgdisk` GPT partitioning that keeps `WOR_BOOT` as partition 1. An extra ESP made the Pi 4 fall back to PXE boot.
  - **A native macOS interface**: AppKit/JXA wizard, progress window, Advanced Options window and error dialogs.
  - **No visible terminal in GUI mode**: the engine reports progress over a file, and each front-end renders it — AppKit on macOS, `yad` on Linux. Administrator access is requested through a native password dialog on both.
  - **Post-flash verification** of partitions, filesystems, boot files, WIM images and the copied `install.wim` checksum.
  - **Offline Windows OOBE** via a shipped `Autounattend.xml`, on by default.
  - **Automatic Pi 4 3 GB RAM unlock** after the WoR-PE reboot, including the BCD `truncatememory` cap.
  - **Pinned, overridable UEFI firmware and driver versions.** Pi 4 stays on UEFI v1.51; v1.52 does not boot reliably from microSD.
  - **Cache modes with SHA-256 payload manifests**, a free-space preflight, and `HideEmptyDrives` written into the cached WoR-PE `settings.ini`.
  - **Editable `config.txt` from `config-templates/`**, applied by the CLI and the GUI alike.
  - **One engine, two front-ends**: `install-wor-gui.sh` sources `install-wor.sh` and adds only windows. A function defined in both files now fails a test.
  - **Explicit `--gui` entry point.** The front-end is never chosen by sniffing `DISPLAY`.
  - **A test suite**, plus ShellCheck, macOS and Linux dry-run CI.
- **0.x** — Upstream Botspot releases, never tagged. Highlights, oldest first: the initial WoR automation, the self-updater, the "next steps" window, a complete rewrite to use ESD releases, download-to-RAM support, Pi 5 support, a GitHub API fallback for UEFI firmware, empty block devices filtered out of the drive list, and SHA-256 hashed ESD image handling.

## Contributing

Pull requests are welcome — please read [CONTRIBUTING.md](CONTRIBUTING.md) first, and [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md).

The short version: if your fix also applies to [Botspot/wor-flasher](https://github.com/Botspot/wor-flasher), send it there too. Upstream is looking for a maintainer and benefits far more from your patch than this fork does.

## Contributors

**Original author.** WoR-Flasher was created and is owned by **[Botspot](https://github.com/Botspot)**, who also created [Pi-Apps](https://github.com/Botspot/pi-apps) and [BVM](https://github.com/Botspot/bvm). Everything this fork does rests on five years of his work, given away for free. If you find WoR-Flasher useful, [consider sponsoring him](https://github.com/sponsors/Botspot).

**Upstream contributors** ([full list](https://github.com/Botspot/wor-flasher/graphs/contributors)):

|                                                                                                                      |                                                                                                                          |                                                                                                                              |                                                                                                                        |                                                                                                                           |                                                                                                                              |
| :------------------------------------------------------------------------------------------------------------------: | :----------------------------------------------------------------------------------------------------------------------: | :--------------------------------------------------------------------------------------------------------------------------: | :--------------------------------------------------------------------------------------------------------------------: | :-----------------------------------------------------------------------------------------------------------------------: | :--------------------------------------------------------------------------------------------------------------------------: |
| [<img src="https://avatars.githubusercontent.com/u/54716352?v=4" width="64"><br>Botspot](https://github.com/Botspot) | [<img src="https://avatars.githubusercontent.com/u/44128563?v=4" width="64"><br>NoozAbooz](https://github.com/NoozAbooz) | [<img src="https://avatars.githubusercontent.com/u/70802936?v=4" width="64"><br>Itai-Nelken](https://github.com/Itai-Nelken) | [<img src="https://avatars.githubusercontent.com/u/176234?v=4" width="64"><br>larskanis](https://github.com/larskanis) | [<img src="https://avatars.githubusercontent.com/u/2014596?v=4" width="64"><br>Marcinoo97](https://github.com/Marcinoo97) | [<img src="https://avatars.githubusercontent.com/u/71036629?v=4" width="64"><br>ryanfortner](https://github.com/ryanfortner) |

**This fork** is maintained by **[Blackout Secure](https://blackoutsecure.app)** — **Dr Bill McIlhargey** ([links](https://linktr.ee/billmcilhargey)) — who contributed the macOS host support, the native progress and Advanced Options windows, post-flash verification, the shared-engine refactor and the test suite.

**Upstream projects** this tool assembles, each with its own authors and license:

- [Windows on Raspberry](https://worproject.com/) — the PE-based installer
- [RPi-Windows-Drivers](https://github.com/worproject/RPi-Windows-Drivers) — Windows ARM64 drivers for the Pi
- [pftf/RPi4](https://github.com/pftf/RPi4) and [pftf/RPi3](https://github.com/pftf/RPi3) — Raspberry Pi UEFI firmware
- [worproject/rpi5-uefi](https://github.com/worproject/rpi5-uefi) — Pi 5 UEFI firmware
- [UUP dump](https://uupdump.net/) — retrieves Windows directly from Microsoft's update servers

## License

Released under the [GNU General Public License v3.0](LICENSE), matching Botspot's [BVM](https://github.com/Botspot/bvm).

> [!IMPORTANT]
> Upstream `Botspot/wor-flasher` ships **no** license file, so its code carries no explicit grant. The Blackout Secure additions are offered under GPL-3.0 without reservation; the pre-existing upstream code is a different matter. Read [NOTICE](NOTICE) before redistributing this fork commercially or relicensing it.

WoR-Flasher does **not** redistribute Windows. Proprietary components are downloaded straight from Microsoft's own update servers via [UUP dump](https://uupdump.net/). This is legal — Raspberry Pi employees [confirmed as much](https://www.raspberrypi.org/forums/viewtopic.php?f=29&t=318599#p1907313) on the Raspberry Pi Forums. The resulting installation is unlicensed, exactly like a retail Windows ISO, and needs a product key or a pre-licensed Microsoft account to activate.

No warranty. Neither Botspot nor Blackout Secure can be held responsible for data loss.
