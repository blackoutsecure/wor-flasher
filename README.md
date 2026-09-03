# ![WoR-flasher icon](https://github.com/Botspot/wor-flasher/blob/main/logo.png?raw=true) WoR-flasher

Create a bootable Windows 10 or Windows 11 ARM64 drive for a Raspberry Pi from Linux or macOS.

> [!WARNING]
> Flashing erases the selected drive. Check the device carefully, keep the computer powered, and do not remove the drive until verification and ejection finish.

WoR-flasher downloads or imports Windows, adds the required UEFI firmware and available drivers, writes the target, and verifies the completed drive.

## Compatibility

| Raspberry Pi         | Newest usable Windows 11 | Limitations                                                            |
| -------------------- | ------------------------ | ---------------------------------------------------------------------- |
| Pi 2 v1.2, Pi 3, CM3 | 23H2 (`22631.x`)         | No Wi-Fi or graphics acceleration; Windows 10 is usually faster        |
| Pi 4, Pi 400, CM4    | 23H2 (`22631.x`)         | No Wi-Fi or graphics acceleration; RAM is limited to 3 GB by default   |
| Pi 5                 | 25H2 and newer           | No native Pi hardware drivers; USB Ethernet is required for networking |

Pi 3 and Pi 4 cannot run builds newer than `25163` because those builds require ARMv8.1 atomics. WoR-flasher rejects incompatible builds. Windows versions that run on these models are past end of support and should be treated as experimental or offline systems.

## Requirements

- Raspberry Pi OS, Debian, Ubuntu, Linux Mint, or another Debian-based Linux distribution; or macOS 13 or newer with [Homebrew](https://brew.sh/)
- Administrator or `sudo` access
- Internet access, unless every required file is already cached
- About 10 GB of free space in the download directory
- A target drive of at least 8 GB
- A desktop session for the graphical interface

Windows, WSL2, and non-Debian Linux distributions are not supported. Dependencies are installed automatically on supported hosts.

## Install

```bash
git clone https://github.com/Botspot/wor-flasher
cd wor-flasher
./install-wor-gui.sh
```

For the terminal interface, run `./install-wor.sh` instead. Use the complete repository; neither installer is designed to be downloaded or piped into Bash by itself.

The selected drive is erased. Drives from 8 GB to under 25 GB can create recovery media for another drive. Drives of 25 GB or more can also install Windows onto themselves. The current boot drive is excluded.

### Existing Windows ISO

Use an official Windows ARM64 ISO containing `sources/install.wim` or `sources/install.esd`:

```bash
SOURCE_FILE=/path/to/windows-arm64.iso ./install-wor.sh
```

Customized Windows images are not supported.

### Download Cache

Downloads are stored in `~/wor-flasher-files` by default.

| `USE_CACHE` | Behavior                                                                      |
| ----------- | ----------------------------------------------------------------------------- |
| `0`         | Remove cached components and download them again                              |
| `1`         | Reuse cache only when its source and SHA-256 payload manifest match (default) |
| `2`         | Trust existing cache without update or integrity checks                       |

Example: `USE_CACHE=0 ./install-wor-gui.sh`

Mode `1` refreshes changed, missing, extra, or outdated cached content. Delete `~/wor-flasher-files` when you no longer need the downloads or extracted Windows images.

## What To Expect

WoR-flasher:

1. Downloads and verifies the PE installer, UEFI firmware, drivers, and Windows image.
2. Extracts or imports the Windows image.
3. Creates FAT32 `WOR_BOOT` and ExFAT `WOR_INSTALL` partitions.
4. Copies the startup and installation files and updates `boot.wim`.
5. Verifies the partition layout, filesystems, boot files, WIM images, and copied `install.wim` checksum.
6. Unmounts and ejects the drive.

Downloads and final verification may take a long time, especially on slow SD cards. Progress is shown for long operations. Do not remove the drive until WoR-flasher reports success.

Move the completed drive to the Pi and connect a display, wired keyboard, and wired mouse. Windows Setup may restart several times; do not remove power or the drive until setup completes.

## Troubleshooting

### Rainbow Screen

The Raspberry Pi firmware did not start UEFI. Reflash the drive and wait for verification to finish. Also update the Pi EEPROM bootloader and avoid `UEFI_USE_LATEST=1` unless intentionally testing firmware.

On Pi 4, three ACT LED blinks indicate that `start4.elf` is missing, four indicate that it failed to launch, and seven indicate that `RPI_EFI.fd` is missing.

### UEFI Splash Then Freeze

On Pi 3 or Pi 4, use Windows 11 build `22631.2861` or another compatible `22631.x` release. In UEFI, verify that `System Table Selection` is `ACPI` and Secure Boot is disabled.

### PXE Boot Or No Local Boot Option

Reflash with the default pinned UEFI firmware and wait for `Written image verified successfully`. Pi 4 UEFI v1.52 has a known microSD boot regression; WoR-flasher uses v1.51 by default.

During first boot, Windows Setup creates Windows Boot Manager. If an installed system has lost that entry, open `Boot Maintenance Manager > Boot Options > Add Boot Option`, select `EFI\Microsoft\Boot\bootmgfw.efi`, and place it above network boot entries.

### WoR-PE Says The Initialization Disk Must Be Recreated

The installer cannot find unallocated space for the Windows target partition. Reflash the drive with a current WoR-flasher checkout and wait for final verification. Do not manually expand `WOR_INSTALL`; unused space after that staging partition is required during installation.

### Keyboard Does Not Work In UEFI

Press `Esc` repeatedly immediately after power-on. Connect a wired keyboard directly to a USB 2.0 port and disconnect hubs and unnecessary USB devices.

### Only 3 GB Of RAM On Pi 4

In UEFI, set `Device Manager > Raspberry Pi Configuration > Advanced Configuration > Limit RAM to 3 GB` to `Disabled`. Leave the limit enabled on Compute Module 4 when its USB controller is required.

## Help

- [WoR-flasher issues](https://github.com/Botspot/wor-flasher/issues/new/choose)
- [Botspot Software Discord](https://discord.gg/RXSTvaUvuu)
- [Windows on Raspberry support](https://worproject.com/contact)

A valid Windows license is required for activation. WoR-flasher does not redistribute Windows.

WoR-flasher builds on [Windows on Raspberry](https://worproject.com/), [RPi-Windows-Drivers](https://github.com/worproject/RPi-Windows-Drivers), [pftf Raspberry Pi UEFI](https://github.com/pftf/RPi4), [WoR Pi 5 UEFI](https://github.com/worproject/rpi5-uefi), and [UUP dump](https://uupdump.net/).
