# ![app icon](https://github.com/Botspot/wor-flasher/blob/main/logo.png?raw=true) WoR-flasher

**Use a Linux machine to install Windows 10 or Windows 11 on a Raspberry Pi SD card.**

> [!IMPORTANT]
> **This repository is looking for a maintainer.**
> I don't use wor-flasher personally, and I don't have the time to maintain it. If you use wor-flasher, and you can read and understand the script, please contact me somehow and I can grant you write privileges to the repository.

> [!TIP]
> **Consider [the BVM project](https://github.com/Botspot/bvm) instead, especially on a Pi 4 or older.**
>
> BVM runs Windows 11 ARM64 in a KVM virtual machine alongside Linux instead of replacing it. Because networking, audio, and USB are passed through from the host, Wi-Fi and sound work out of the box - neither of which bare-metal Windows on a Raspberry Pi supports today. Microsoft's Prism emulator also lets x86 and x64 applications run.
>
> It does **not** lift the Windows version ceiling. On hardware without ARMv8.1, such as the Pi 4 and older, BVM pins Windows 11 to build `22631.2861` for exactly the same CPU reason described in [The ARMv8.1 limitation](#the-armv81-limitation). No tool can work around that, since it is a property of the processor. BVM requires an ARM64 Linux with the `kvm` kernel module.

In 2020, this was flat-out impossible.  
In 2021, this required following [a complicated tutorial](https://worproject.com/guides/how-to-install/from-other-os).  
Now, using the new WoR-flasher, it's a _piece of cake_.

## Requirements

- A Debian-based Linux machine, ARM or x86, to run the flasher. Only Raspberry Pi OS has been tested.
- A storage device of at least 8 GB. See [Choosing a drive](#choosing-a-drive) for the size tiers.
- A Raspberry Pi 2 v1.2, 3, 4, 400, or 5 to run Windows on.

> [!WARNING]
> Flashing erases the target drive completely. Botspot (the developer of this tool) cannot be held responsible for data loss.

## Compatibility

| Pi model               | CPU        | Architecture | Newest Windows 11 that boots | End of support                                                                                    | Notes                          |
| ---------------------- | ---------- | ------------ | ---------------------------- | ------------------------------------------------------------------------------------------------- | ------------------------------ |
| Pi 2 v1.2 / Pi 3 / CM3 | Cortex-A53 | ARMv8.0      | 23H2 (`22631.x`)             | [November 11, 2025](https://learn.microsoft.com/en-us/lifecycle/products/windows-11-home-and-pro) | Windows 10 recommended         |
| Pi 4 / Pi 400          | Cortex-A72 | ARMv8.0      | 23H2 (`22631.x`)             | [November 11, 2025](https://learn.microsoft.com/en-us/lifecycle/products/windows-11-home-and-pro) | RAM limited to 3 GB by default |
| Pi 5                   | Cortex-A76 | ARMv8.2      | 25H2 and newer               | [October 12, 2027](https://learn.microsoft.com/en-us/lifecycle/products/windows-11-home-and-pro)  | No Windows drivers yet         |

### The ARMv8.1 limitation

> [!IMPORTANT]
> **Windows 11 builds newer than 25163 cannot run on the Pi 3 or Pi 4.** Those builds make extensive use of the atomic instructions introduced in ARMv8.1, which the Cortex-A53 and Cortex-A72 do not implement.

The symptom is distinctive: the Pi reaches the UEFI splash screen normally, then hangs with no error message the moment the bootloader hands off to Windows. In practice this means 23H2 (`22631.x`) is the newest retail release that works, since 24H2 is build 26100. The Pi 5's Cortex-A76 is ARMv8.2 and is unaffected.

WoR-flasher enforces this automatically. When the target is a Pi 3 or Pi 4, incompatible builds are hidden from the version menus, `get_bid` returns the newest build that will actually boot, and flashing aborts with an explanation if an incompatible build is supplied via the `BID` variable. The cutoff is controlled by the [`ARMV80_MAX_BUILD`](#advanced-tuning-variables) variable.

### End of support

End-of-support dates in the table are for the newest bootable build, Home and Pro editions. Enterprise and Education editions are supported longer. If you run Windows 10 instead, its support ended [October 14, 2025](https://learn.microsoft.com/en-us/lifecycle/products/windows-10-home-and-pro).

> [!WARNING]
> Every Windows version a Pi 3 or Pi 4 can run is now past end of support and no longer receives security updates. This is fine for an experimental or offline machine, but worth knowing before putting one on a network you care about.

## Hardware limitations

### Raspberry Pi 4: 3 GB RAM limit

The UEFI firmware limits the Pi 4 to 3 GB of usable RAM by default, regardless of whether your board has 4 GB or 8 GB. This is a conservative default, not a permanent restriction.

To use the full amount of RAM:

1. Keep pressing <kbd>ESC</kbd> after plugging in the power cord, until you see the UEFI setup screen.
2. Go to `Device Manager` → `Raspberry Pi Configuration` → `Advanced Configuration`.
3. Change `Limit RAM to 3 GB` to `Disabled`.
4. Press <kbd>ESC</kbd> several times to go back, then <kbd>Y</kbd> to save when prompted, and reboot.

See the [Windows on Raspberry FAQ](https://worproject.com/faq) for details.

> [!NOTE]
> On the Compute Module 4, USB support requires a RAM limit, so leave this setting enabled there.

### Wi-Fi and Bluetooth

Onboard Wi-Fi is not supported. The necessary drivers simply do not exist yet. Use a wired Ethernet connection instead. See the [RPi-Windows-Drivers status page](https://github.com/worproject/RPi-Windows-Drivers#status) for current hardware support.

### Raspberry Pi 5

Raspberry Pi 5 support is here and it runs fast, but there are no drivers. SD card boot seems more reliable, and for internet you need a USB to Ethernet adapter.

### Preinstalled apps

WoR-Flasher cannot debloat the OS. Performance is about the same either way, but there will be extra preinstalled apps you must remove manually if you want them gone.

## Legal

This tool is **100% legal**. All proprietary Windows components are downloaded straight from Microsoft's update servers using [uupdump](https://uupdump.net). Consider reading [this debate](https://www.raspberrypi.org/forums/viewtopic.php?f=29&t=318599) that took place on the Raspberry Pi Forums. At the conclusion of the thread, Raspberry Pi **employees** [confirm](https://www.raspberrypi.org/forums/viewtopic.php?f=29&t=318599#p1907313) that WoR is completely legal. The OS is unlicenced just like a regular Windows ISO, which can be activated via an activation key or by logging in with a pre-licensed Microsoft account.

## Getting help

| Problem with                         | Where to go                                                                                                                                |
| ------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------ |
| The WoR-flasher tool                 | [Open an issue](https://github.com/Botspot/wor-flasher/issues/new/choose) or the [Botspot Software Discord](https://discord.gg/RXSTvaUvuu) |
| Windows on Raspberry (the OS itself) | [Email the WoR developers](https://worproject.com/contact) or [join their Discord](https://discord.gg/jQCpfVK)                             |

## WoR-flasher walkthrough

### Choosing a drive

The size of the drive you flash determines what it can do:

| Drive size      | What it can do                                                                    |
| --------------- | --------------------------------------------------------------------------------- |
| 25 GB or larger | Installation drive - can install Windows onto itself                              |
| 8 GB to 25 GB   | Recovery drive - can only install Windows onto **other drives larger than 16 GB** |
| Under 8 GB      | Too small to be usable                                                            |

### Install WoR-flasher

The fastest way to get WoR-flasher running on a RPi is by using the [Pi-Apps app store for Raspberry Pi](https://github.com/Botspot/pi-apps):  
[![badge](https://github.com/Botspot/pi-apps/blob/master/icons/badge.png?raw=true)](https://github.com/Botspot/pi-apps)  
Installing WoR-flasher from Pi-Apps has several advantages: it creates a convenient button in the Start menu, uninstalling takes one click, and updates are handled seamlessly.

### To manually download WoR-flasher

```
git clone https://github.com/Botspot/wor-flasher
```

This will download the scripts to a new directory named `wor-flasher`.  
**Dependencies:** No need to install packages manually. Running the script will automatically install these: `yad` `aria2` `cabextract` `wimtools` `chntpw` `genisoimage` `exfat-fuse` `exfat-utils` `wget` `udftools` `bc`

### To run WoR-flasher using the graphical interface

```
~/wor-flasher/install-wor-gui.sh
```

- Choose a Windows version and choose which Raspberry Pi model will be running it.  
  ![page1](https://user-images.githubusercontent.com/54716352/131228226-5d5b8456-b273-48a5-b4c3-5e90790cf21e.png)
- Choose a language for Windows.  
  ![page2](https://user-images.githubusercontent.com/54716352/131228261-e7e1a989-4151-4df7-8aa2-eff95704df41.png)
- Plug in a writable storage device to flash Windows to.  
  ![page3](https://user-images.githubusercontent.com/54716352/131228296-fb61f216-9a12-412a-b7b5-0bcd185891a0.png)
  - See [Choosing a drive](#choosing-a-drive) for what each size can do.
- Double-check that everything looks correct before clicking the Flash button.  
  ![page4](https://user-images.githubusercontent.com/54716352/131921620-7ca69a5c-13fe-4236-8e0e-27ff4cfffa10.png)
- A terminal will launch and run the `install-wor.sh` script:  
  ![terminal3](https://user-images.githubusercontent.com/54716352/131228381-11dc3a4e-96da-40ec-8f46-8b28ade5ee52.png)  
  Note: this can take a lot of time to download individual files from Microsoft, compress them, and generate a Windows image. Fortunately, subsequent runs can skip the lengthy image-generating step if the ISO file exists.
- If all goes well, the terminal will close and you will be told what to do next.  
  ![next steps](https://user-images.githubusercontent.com/54716352/131228409-f84ede9b-a1fc-43f9-a79c-5b1853513960.png)

### To run WoR-flasher using the terminal interface

```
~/wor-flasher/install-wor.sh
```

<details><summary>Example terminal walkthrough (click to expand)</summary>

```
$ ~/wor-flasher/install-wor.sh
Choose Windows version:
1) Windows 11
2) Windows 10
3) More options...
Enter 1, 2 or 3: 1

Choose language: en-us

Choose Raspberry Pi model to deploy Windows on:
1) Raspberry Pi 5
2) Raspberry Pi 4 / 400
3) Raspberry Pi 3 or Pi2 v1.2
Enter 1, 2, or 3: 2

Available devices:
/dev/sdb - 59.5GB - USB Storage
Choose a device to flash the Windows setup files to: /dev/sdb

1) Create an installation drive (minimum 25 GB) capable of installing Windows to itself
2) Create a recovery drive (minimum 7 GB) to install Windows on other >16 GB drives
Choose the installation mode (1 or 2): 1

Input configuration:
DL_DIR: /home/pi/wor-flasher-files
RUN_MODE: cli
RPI_MODEL: 4
DEVICE: /dev/sdb
CAN_INSTALL_ON_SAME_DRIVE: 1
BID: 22631.2861
WIN_LANG: en-us

Using UEFI firmware: https://github.com/pftf/RPi4/releases/download/v1.52/RPi4_UEFI_Firmware_v1.52.zip
Formatting /dev/sdb
Generating partitions
Generating filesystems
# script output continues... It generates a Windows image legally, downloads all necessary drivers, the BIOS, the bootloader, and the modified kernel. Once done it ejects the drive.
```

</details>
This script is actually what does the flashing: The gui script is just a front-end that launches dialog windows and finally runs install-wor.sh in a terminal.

## Troubleshooting

### The Pi is stuck on the rainbow screen

The GPU firmware never handed off to the UEFI firmware. Usually one of:

- **Mismatched firmware files.** `RPI_EFI.fd`, `start4.elf`, `fixup4.dat`, the `.dtb` files, and the `overlays/` folder must all come from the same UEFI release. Hand-copying only some of them causes this. Re-flash rather than patching files individually.
- **Outdated firmware.** Newer Pi 4 board revisions need a recent UEFI release. WoR-flasher downloads the latest release by default.
- **Outdated bootloader EEPROM.** Update it with Raspberry Pi Imager (`Misc Utility Images` → `Bootloader`).

The green ACT LED blink pattern narrows it down: 3 blinks means `start4.elf` was not found, 4 blinks means it failed to launch, and 7 blinks means `RPI_EFI.fd` was not found.

### The Pi reaches the UEFI splash screen, then freezes

If <kbd>ESC</kbd> works at the splash but the system hangs once the countdown finishes, the firmware is healthy and the hang is in the handoff to Windows. Check in this order:

1. **The Windows build is too new.** This is by far the most common cause on a Pi 3 or Pi 4. See [The ARMv8.1 limitation](#the-armv81-limitation).
2. **System Table Selection is not ACPI.** Go to `Device Manager` → `Raspberry Pi Configuration` → `Advanced Configuration` and make sure it is set to `ACPI`. Windows cannot boot from a Device Tree handoff.
3. **Secure Boot is enabled.** Check `Device Manager` → `Secure Boot Configuration` and disable it.

### The keyboard does not work in UEFI

UEFI stops accepting keystrokes once the boot countdown expires, so a keyboard that seems dead is often just a system that has already moved on. Start tapping <kbd>ESC</kbd> the instant power is applied.

If it genuinely does not respond, use a wired keyboard plugged directly into a USB 2.0 port with no hub, and unplug all other USB devices. Wireless dongles and keyboards with built-in hubs frequently fail to enumerate in UEFI.

### Only 3 GB of RAM is available

Expected on a Pi 4. See [Raspberry Pi 4: 3 GB RAM limit](#raspberry-pi-4-3-gb-ram-limit).

### Getting more detail

The generated `config.txt` already sets `enable_uart=1` and `uart_2ndstage=1`, so serial debugging needs no changes. Connect a USB-TTL adapter to GPIO 14 (TX), GPIO 15 (RX), and GND, at **115200 baud**, to see exactly where boot stops.

## Scripting reference

### Environment variable options

The `install-wor.sh` script is designed to be used within other, larger bash scripts. For automation and customization, `install-wor.sh` will detect and obey certain environment variables.

Setting any of the first four suppresses the corresponding interactive prompt.

| Variable                    | Description                                                                                                                                                                             |
| --------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `BID`                       | An exact Windows version ID. Example: `22631.2861`                                                                                                                                      |
| `WIN_LANG`                  | Language for the Windows image. Example: `en-us`                                                                                                                                        |
| `RPI_MODEL`                 | Target Raspberry Pi model. Allowed values: `3`, `4`, `5`                                                                                                                                |
| `DEVICE`                    | The device to flash. Example: `/dev/sda`                                                                                                                                                |
| `DL_DIR`                    | Download location. Defaults to `~/wor-flasher-files`                                                                                                                                    |
| `CAN_INSTALL_ON_SAME_DRIVE` | `1` if the device is larger than 25 GB and should install Windows onto itself, otherwise `0`                                                                                            |
| `CONFIG_TXT`                | Customizes the `/boot/config.txt` of the resulting drive, commonly for overclocking or HDMI settings. [This is the default value.](https://github.com/pftf/RPi4/blob/master/config.txt) |
| `SOURCE_FILE`               | Path to an existing Windows ARM64 ISO to use instead of downloading one. Must be at least 3 GB and end in `.iso`                                                                        |
| `RUN_MODE`                  | Set to `gui` to display graphical error messages                                                                                                                                        |
| `DRY_RUN`                   | Set to `1` to run the whole setup but exit after downloading, without flashing                                                                                                          |

#### Advanced tuning variables

These have working defaults and rarely need changing.

| Variable             | Default      | Description                                                                                                                                                                             |
| -------------------- | ------------ | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `UEFI_USE_LATEST`    | `1`          | Download the newest UEFI firmware release from GitHub. Set to `0` to use the pinned version instead. Pre-releases are never selected, since upstream uses them to mark known-bad builds |
| `UEFI_VER_PI3`       | `v1.39`      | Pinned UEFI version, used when `UEFI_USE_LATEST=0` or GitHub is unreachable                                                                                                             |
| `UEFI_VER_PI4`       | `v1.52`      | Pinned UEFI version, used when `UEFI_USE_LATEST=0` or GitHub is unreachable                                                                                                             |
| `UEFI_VER_PI5`       | `v0.3`       | Pinned UEFI version, used when `UEFI_USE_LATEST=0` or GitHub is unreachable                                                                                                             |
| `ARMV80_MAX_BUILD`   | `25163`      | The last Windows build that boots on an ARMv8.0 Pi. Higher builds are hidden and rejected for the Pi 3 and Pi 4                                                                         |
| `ARMV80_SAFE_BID`    | `22631.2861` | The build suggested when a user picks an incompatible one for a Pi 3 or Pi 4                                                                                                            |
| `WIN11_MIN_BUILD`    | `22000`      | The build number at which a release counts as Windows 11 rather than Windows 10                                                                                                         |
| `WIN10_OLDEST_BUILD` | `17134.112`  | Marks the end of the Windows 10 section of worproject.com's version list                                                                                                                |
| `EXAMPLE_BID`        | `22621.525`  | The example build number shown in prompts                                                                                                                                               |

Example usage:

```
DL_DIR=/media/pi/my-big-flash-drive DEVICE=/dev/sdg DRY_RUN=1 BID=22631.2861 RPI_MODEL=4 WIN_LANG=en-us ~/wor-flasher/install-wor-gui.sh
```

### Functions

The `install-wor.sh` script is designed to be used within other, larger bash scripts. For improved integration, `install-wor.sh` is equipped with a variety of useful functions that frontend scripts like `install-wor-gui.sh` can use.  
**To source the script** so the functions are available:

```
source ~/wor-flasher/install-wor.sh source
```

Question: why does that command say "`source`" twice? Answer: The first "`source`" is a command, and the second "`source`" is a command-line flag that is passed to the script to let it know you are sourcing it.
Once the script is sourced, these new commands (also known as functions) become available:

- `error` - a simple function that Botspot uses in bash scripts to warn the user that something failed and to exit the script with a failure code. (1)  
  Input: string containing the error message  
  Usage:

```
command-that-downloads-windows || error "Windows failed to download! Check your internet connection and try again."
```

- `echo_green` and `echo_red` - announce the success or failure of an action in colored text. `status` prints blue progress text.  
  Input: string containing message  
  Usage:

```
status "Now, downloading windows... please wait"
echo_green "Done"
echo_red "That did not work, but it is not fatal"
```

- `package_available` - Determines if a package is possible to install from the apt repositories
  Input: one name of a package
  Usage:

```
if package_available yad ;then
  echo "yad can be installed"
fi
```

- `install_packages` - Checks for and installs a quoted list of packages.  
  Input: string containing a space-separated list of packages  
  Usage:

```
install_packages 'yad aria2 cabextract wimtools chntpw genisoimage exfat-fuse exfat-utils wget'
```

- `download_from_gdrive` - Downloads a publically shared large-file from Google Drive. [Here's the tutorial](https://medium.com/@acpanjan/download-google-drive-files-using-wget-3c2c025a8b99) I adapted it from.  
  Inputs: File ID, output filename  
  Usage:

```
download_from_gdrive 1WHyHFYjM4WPAAGH2PICGEhT4R5TlxlJC WoR-PE_Package.zip
```

- `get_partition` - A clean, reliable way to determine the block-device of a partition.  
  Input: block device of drive, partition number  
  Usage:

```
get_partition /dev/sda 2
#Assuming partition 2 exists, the above command returns "/dev/sda2"

get_partition /dev/mmcblk0 2
#Assuming partition 2 exists, the above command returns "/dev/mmcblk0p2"

get_partition /dev/mmcblk0 all
#Returns every partition within the drive, each one on a line
```

- `get_device_name` - Determine a human-readable name for the given storage drive.  
  Input: block device of drive  
  Usage:

```
get_device_name /dev/sda
```

- `get_size_raw` - Determines the size of a drive in bytes.  
  Input: block device of drive  
  Usage:

```
get_size_raw /dev/sda
```

- `list_devs` - list available storage drives in a human-readable, colored format.  
  Usage:

```
list_devs
```

- `get_bid` - Get the latest Windows build ID for either Windows 10 or Windows 11 that the target Raspberry Pi can actually boot. When `RPI_MODEL` is "`3`" or "`4`", builds requiring ARMv8.1 are skipped.  
  Input: "`10`" or "`11`"
  Usage:  

```
get_bid 11
```

- `cpu_supports_bid` - Exit 0 if the target Pi's CPU can run the given build, otherwise exit 1. Depends on `RPI_MODEL` being set.  
  Input: build ID  
  Usage:

```
if cpu_supports_bid 26100.1742 ;then
  echo "this build will boot"
fi
```

- `list_bids_supported` - Same as `list_bids`, but omits builds the target Pi cannot run.  
  Input: "`10`" or "`11`"  
  Usage:

```
list_bids_supported 11
```

```

- `get_os_name` - Get human-readable name of operating system.
  Input: valid Windows build ID
  Usage:

```

get_os_name 22631.2861

````

### Example function and variable usage

This code will non-interactively flash Windows 11 to `/dev/sda` and add overclock settings. You can copy and paste the code into a terminal, or save this as a shell script.

```bash
#make all variables we set to be visible to the script (only necessary if you run this in a terminal)
set -a

#First, source the script so its functions are available
source ~/wor-flasher/install-wor.sh source

#Determine the latest Windows 11 update ID using a function
BID="$(get_bid 11)"

#set destination RPi model
RPI_MODEL=4

#choose language
WIN_LANG=en-us

#set the device to flash
DEVICE=/dev/sda

#set a custom config.txt
CONFIG_TXT="over_voltage=6
arm_freq=2147
gpu_freq=750

# don't change anything below this point #
arm_64bit=1
enable_uart=1
uart_2ndstage=1
enable_gic=1
armstub=RPI_EFI.fd
disable_commandline_tags=1
disable_overscan=1
device_tree_address=0x1f0000
device_tree_end=0x200000
dtoverlay=miniuart-bt"

#indicate that drive is large enough to install Windows to itself
CAN_INSTALL_ON_SAME_DRIVE=1

~/wor-flasher/install-wor.sh
````

## Sources

- [Windows 11 Home and Pro lifecycle](https://learn.microsoft.com/en-us/lifecycle/products/windows-11-home-and-pro) - Microsoft Lifecycle
- [Windows 10 Home and Pro lifecycle](https://learn.microsoft.com/en-us/lifecycle/products/windows-10-home-and-pro) - Microsoft Lifecycle
- [Windows 11 Enterprise and Education lifecycle](https://learn.microsoft.com/en-us/lifecycle/products/windows-11-enterprise-and-education-version-21h2) - Microsoft Lifecycle
- [Windows Processor Requirements](https://learn.microsoft.com/en-us/windows-hardware/design/minimum/windows-processor-requirements) - Microsoft Learn
- [Windows on Raspberry FAQ](https://worproject.com/faq) - the ARMv8.1 build cutoff and the 3 GB RAM limit
- [pftf/RPi4 releases](https://github.com/pftf/RPi4/releases) - UEFI firmware for the Pi 4
- [RPi-Windows-Drivers](https://github.com/worproject/RPi-Windows-Drivers#status) - current hardware driver status
