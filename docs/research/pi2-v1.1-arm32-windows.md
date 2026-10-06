# Raspberry Pi 2 v1.1: ARM32 Windows and WoR feasibility

Investigation date: **2026-10-02**.

Follow-up: a separate IoT Core FFU implementation and verified native image
acquisition workflow were subsequently added.
See the [current supported workflow](../../README.md#windows-10-iot-core);
the installer descriptions below record the pre-implementation investigation
snapshot, not a claim that the current code is still ARM64-only.

### Recommendation implementation status

| Recommendation | Current disposition |
| --- | --- |
| Separate ARM32 IoT and ARM64 desktop profiles | Implemented in the shared engine, both GUIs, CLI, configuration and hook. No invented ARM64 IoT Core support. |
| Verified local FFU and official package acquisition | Implemented using the same pinned manifest/FFU validation. Native ISO/MSI extraction does not run Windows software; downloading is explicit. |
| Whole-disk, source-disk, consent and target-replacement checks | Implemented, including APFS/LVM backing devices, device-node identity binding and repeated capacity checks. |
| Mandatory payload/read-back verification | Implemented; unsupported formats and unknown images fail closed. Normal flashing replaces validated GPT metadata with bounded cleanup; malformed GPT is refused. A separately confirmed full-drive wipe can discard all old partition data by zeroing and verifying every sector before image application. Target protections remain mandatory; no partition expansion. |
| GUI parity, progress, cancellation and error visibility | Both front-ends share deferred preparation, authentication/retry and deployment functions; no image acquisition occurs before Flash. Source preparation is cancellable and failures preserve logs. |
| Display and post-boot personalization | The user reported that 720p60 resolved an HDMI out-of-range issue; it is now the recommended default, with explicit presets/custom settings preserved. Optional account and host-selected language preferences use post-boot SSH; live account/language application remains unassessed. |
| Offline parser/workflow and mutation tests | Implemented, including synthesized non-Windows images and safe regular-file targets. |
| Native Linux device testing | Opt-in owned-loop CI test added. This is not a physical disk test; local Docker availability determines whether it can also run locally. |
| Exact comparison with a Microsoft-applied reference image | Still requires a Windows technician environment and separately produced reference. Source-image/extent checks do not substitute for this independent qualification. |
| Physical Pi cold/warm boot and peripheral testing | Requires an explicitly selected board/card and approval. No physical board or disk was exercised here. |
| ARM32 desktop port, new driver/UEFI builds, additional images | Research follow-ups, not prerequisites to deploy the reviewed stock IoT image. Not implemented or advertised as supported. |

**Status: research, not newly implemented hardware support.** No flashing engine,
board picker, configuration contract, firmware pin, or existing test was changed
for this investigation. No physical disk was written and no Raspberry Pi boot
was tested.

## Contents

- [Executive assessment](#executive-assessment)
- [A. ARM32 component inventory](#a-arm32-component-inventory)
- [B. Complete reference-file search](#b-complete-reference-file-search)
- [C. Driver and model matrix](#c-driver-and-model-matrix)
- [D. Desktop Windows feasibility](#d-desktop-windows-feasibility)
- [E. ARM32 UEFI and BCM2836 boot chain](#e-arm32-uefi-and-bcm2836-boot-chain-analysis)
- [F. Existing, recoverable and missing pieces](#f-existing-recoverable-and-missing-pieces)
- [G. Repository history and migration](#g-repository-history-and-migration)
- [H. Probability and minimum missing component](#h-probability-assessment-and-minimum-missing-component)
- [I. Current installer and an implementable selection](#i-current-installer-and-an-implementable-selection)
- [Validation performed](#validation-performed)

## Executive assessment

**Windows 10 IoT Core is a credible legacy option for the original Pi 2, not a
way to obtain ordinary desktop Windows on it.**

- **Pi 2 v1.1 really is ARM32-only:** BCM2836, Cortex-A7, ARMv7-A. Pi 2 v1.2 uses
  BCM2837/Cortex-A53 and is a materially different target. The current Pi 3
  firmware route explicitly includes v1.2 and excludes earlier revisions.
  [Processor evidence][soc2836], [BCM2837 comparison][soc2837],
  [ARM64 firmware target list][pi3-targets].
- **IoT Core on BCM2836 is verified historical support, not speculation.**
  Microsoft's 15063 release notes name the Pi 2's Cortex-A7; its 17763.253 notes
  explicitly name BCM2836/2837. [15063 evidence][iot15063],
  [17763.253 evidence][iot17763].
- **That does not establish ARM32 desktop Windows support.** IoT Core supports
  UWP foreground applications and some native Win32 console/services, but not
  the classic HWND windowing APIs required by an ordinary Explorer desktop.
  [Application API limits][iot-apps], [IoT shell behavior][iot-shell].
- **The current installer cannot be fixed with another board label.** Its
  downloads, firmware/driver selection, WoR-PE executable, answer files and
  post-write checks target ARM64. IoT's FFU deployment is a different pipeline.
  See [the installer analysis](#i-current-installer-and-an-implementable-selection).
- **Recommended development direction:** an explicitly named
  `Windows 10 IoT Core (ARM32, legacy)` image profile with an initially
  local-file-only, independently validated FFU path. Keep desktop ARM64 and
  unsupported ARM32 desktop experiments separate. Do not enable a selectable
  flashing target until that backend and actual BCM2836 hardware tests exist.

## Evidence grades and scope

- **Verified:** directly inspected source, build/INF/package metadata, release
  metadata, binary architecture, or explicit first-party support documentation.
  This does **not** mean hardware-tested in this investigation.
- **Indirect:** shared hardware IDs, a build configuration, a related board's
  results, or community deployment instructions without a reproducible Pi 2
  v1.1 boot demonstration.
- **Speculative:** a proposed port or combination of components not established
  by the inspected artifacts.

An `ARM` project configuration is evidence of an ARM32 build target, not a
passing hardware test. `Win32` in a Visual Studio solution or EDK2 host utility
is not evidence of a Windows ARM32 desktop image. An INF is not its referenced
binary; an ACPI device declaration is not a working Windows driver.

Commit dates below mean the **committer date of the last commit touching the
specified path at the inspected revision**, unless explicitly labeled as a
release or snapshot date. File ZIP timestamps and a repository's `pushed_at`
are not substituted for path history. Closed Microsoft OS components do not
have public source-commit dates.

The review includes the three requested repositories, all eleven repositories
listed by the public WoR organization endpoint, and directly relevant Microsoft
BSP, image-building and documentation sources. Generic EDK2/TF-A histories were
searched by commit message; Raspberry Pi platform changes were additionally
searched by diff. This is not a claim to have searched every historical blob in
every transitive upstream dependency, inaccessible branch, or deleted remote.

## A. ARM32 component inventory

The inventory distinguishes **firmware modules**, **Windows drivers**,
**closed OS components**, and **image-building tools**. A firmware GPIO or
display implementation is not a Windows GPIO or display driver.

### Firmware, ACPI and boot components

Unless another repository is named, paths in this table belong to the
identical `ms-iot/RPi-UEFI` / `worproject/RPi-UEFI-Arm32` source snapshot.
`I` means last path commit
[`42edc35271384c6d62233419231c918711ada2d3`][arm32-import],
**2016-10-21**; `R` means
[`4726af1381dd3ebac326b73c802d60fef1531179`][arm32-rs2],
**2017-03-06**. Other dates are stated explicitly.
Binary availability means the [v1.0 firmware ZIP][arm32-uefi-release] exists;
individual firmware-volume modules were not decompressed or matched
byte-for-byte to rebuilt source.

| Component / exact source path | Architecture and model evidence | Source status | Binary status | Last path commit/date |
| --- | --- | --- | --- | --- |
| [Pi2 UEFI platform DSC][arm32-pi2-dsc], `Pi2BoardPkg/Pi2BoardPkg.dsc` | ARM32/ARMv7; Pi2/Cortex-A7 target | Open | Combined `kernel.img` in v1.0 | `5bfd48d6...`, 2019-08-12 |
| [Pi3 UEFI DSC][arm32-pi3-dsc], `Pi3BoardPkg/Pi3BoardPkg.dsc` | ARM32 on Cortex-A53; relevant to Pi3 and the later Pi2 silicon, not an ARM64 build | Open | Combined `kernel.img`; exact-board validation separate | `5bfd48d6...`, 2019-08-12 |
| [MIDR boot stub][arm32-startup], `Pi3BoardPkg/Scripts/startup.S` | ARM32; explicit A7/A53 routing | Open | Entry instruction checked in `kernel.img` | I |
| [`BuildPi2Board.bat`, `BuildPi3Board.bat`, `BuildPi2Pi3Board.bat`][arm32-build] | ARM32 board-package selection | Open scripts | Build tools, not OS binaries | I |
| [SEC entry library][arm32-sec], `Pi2BoardPkg/Library/SecLib/` | ARM32 Pi2 early initialization | Open | Configured into the firmware build | I |
| [Board/memory library][arm32-boardlib], `Pi2BoardPkg/Library/Pi2BoardLib/` | BCM2836 register and memory map | Open | Statically linked, no separate driver download | I |
| [SoC header][arm32-soc-header], `Pi2BoardPkg/Include/Bcm2836.h` | BCM2836 peripheral base `0x3F000000` | Open header | No standalone executable | I |
| [Interrupt DXE][arm32-interrupt], `Pi2BoardPkg/Drivers/InterruptDxe/`; [BCM GIC-interface library][arm32-gic], `Pi2BoardPkg/Library/BcmGicLib/` | ARM32, non-GIC BCM controller; **firmware-stage** support | Open | Configured in Pi2 FDF, not an OS HAL DLL | I |
| [SDHOST DXE][arm32-sdhost], `Pi2BoardPkg/Drivers/SdHostDxe/` | ARM32; Pi2 boot SD controller | Open | Configured in Pi2 FDF | I |
| [MMC protocol/block-I/O layer][arm32-mmc], `Pi2BoardPkg/Drivers/MmcDxe/` | ARM32; works with the configured SD host | Open | Configured in Pi2 FDF | I |
| [Arasan MMC host][arm32-arasan], `Pi2BoardPkg/Drivers/ArasanMmcHostDxe/` | ARM32 source for shared controller IP | Open | **Not selected by the inspected Pi2 FDF**; source presence is not shipped support | I |
| [Framebuffer/GOP][arm32-display], `Pi2BoardPkg/Drivers/DisplayDxe/` | ARM32 Pi2 display, using VideoCore mailbox | Open | Configured in Pi2 FDF; not a Windows accelerated GPU driver | `5bfd48d6...`, 2019-08-12 |
| [Mailbox library][arm32-mailbox], `Pi2BoardPkg/Library/BcmMailboxLib/` | ARM32; VideoCore property protocol | Open | Statically linked | R |
| [Reset library][arm32-reset], `Pi2BoardPkg/Library/ResetSystemLib/` | ARM32; board reset, not complete Windows power management | Open | Linked into firmware runtime reset service | R |
| [Virtual clock][arm32-rtc], `Pi2BoardPkg/Library/VirtualRealTimeClockLib/` | ARM32; software clock, not a newly discovered hardware RTC | Open | Linked into runtime clock service | I |
| [LED library][arm32-led], `Pi2BoardPkg/Library/BcmLedLib/` | ARM32; narrow GPIO register use for LEDs | Open | Statically linked; not a general Windows GPIO service | I |
| [Board PCD library][arm32-pcd], `Pi2BoardPkg/Library/Pi2PcdLib/` | ARM32 platform configuration | Open | Statically linked | I |
| [SMBIOS DXE][arm32-smbios], `Pi2BoardPkg/Drivers/PlatformSmbiosDxe/` | ARM32 board-identification data | Open | Configured in Pi2 FDF | R |
| [DSDT][arm32-dsdt], `Pi2BoardPkg/AcpiTables/Common/DSDT.asl` | ACPI data for ARM32 Pi2; reused by Pi3 | Open ASL | Built into firmware table set, not a Windows binary | `2e4b7dd1...`, 2017-04-14 |
| [FADT][arm32-fadt], [MADT][arm32-madt], [GTDT][arm32-gtdt], [CSRT][arm32-csrt], [Platform.h][arm32-acpi-platform] under `Pi2BoardPkg/AcpiTables/Common/` | Fixed platform, CPU topology, timers and DMA resource metadata; see section E | Open source/data | Configured ACPI payloads | I for each listed file |
| [`Uart.asl`][arm32-uart], [`Sdhc.asl`][arm32-sdhc-asl], [`Rhpx.asl`][arm32-rhpx] under `Pi2BoardPkg/AcpiTables/` | ARM32 Pi2 device bindings; IDs are not revision tests | Open ASL | Configured table payloads | I for each listed file |
| [`PEP.asl`][arm32-pep], [`pep.h`][arm32-pep-h] and `pepd.source.cxx` under `Pi2BoardPkg/AcpiTables/Common/` | Power-plugin description/generation; not a verified runtime Windows PEP binary | Open | Firmware ACPI data; Windows-side implementation unverified | I |
| [TPM table alternatives][arm32-tpm-tables], `SoftwareTpm2Device.asl`, `SoftwareTpm2Table.aslc`, `Tpm2DeviceSpi0.asl`, `Tpm2TableSpi0.aslc` | Optional ARM32 platform descriptions; no implication of a physical TPM on every board | Open | Build-flag-selected table sets | `0e2bf04f...`, 2019-08-06 |
| Broadcom boot files in [v1.0][arm32-uefi-release]: `bootcode.bin`, `start.elf`, `fixup.dat` | **VideoCore firmware/data**, not ARM64 Windows drivers; bundled for ARM32 Pi2/3 boot | No public source in these repositories | Present in inspected ZIP | No source commit in that release; publication 2020-05-05, not a source-change date |
| Pi2 UEFI USB/LAN support | No DWC/USB or Ethernet controller module selected in the inspected [Pi2 FDF][arm32-fdf] | No Pi2 firmware implementation established here | Do not assume USB/network boot from the existence of Windows drivers | Not applicable; Windows-side components are inventoried separately |

General EDK2 timer, UART, boot selection, CPU parking and variable-store
dependencies are listed in the [detailed firmware inventory](pi2-arm32-firmware-components.tsv);
they are reused infrastructure, not additional Windows drivers or independently tested boards.
The availability of a source module also does not mean that the historical
compiler, tools, signing configuration and output can be reproduced unchanged.

### Windows BSP and driver components

The [detailed driver inventory](pi2-arm32-driver-components.tsv) records
architecture evidence, exact representative source paths and last path commits,
source/binary status, and separate model evidence. The table below makes the
principal components readable without treating every C++ file as a separate
driver.

Unless noted, repository **D** is
[`raspberrypi/windows-drivers` at `88ee238c...`][drivers-readme].
`N` denotes
[`b6dcdc76e35af86542e73b85c48eefa971459c91`][driver-inf-normalization],
**2021-03-19**, the latest change to the **listed INF file**, not a claimed
binary build date. Dates for INF normalization are intentionally not replaced
with an older, more interesting algorithm commit.

| Component | Repository and exact representative path | ARM32 / ARM64 evidence | Source and binary status | Last listed-path commit/date |
| --- | --- | --- | --- | --- |
| BSP feature manifest | D: [`bspfiles/Packages/RPiFM.xml`][driver-manifest] | RPi2/RPi3 feature groups; shared base packages | Open metadata; distinguishes `%BSPPKG_DIR%` from external `%MSPKG_DIR%` packages | `740a31be...`, 2020-03-19 |
| Boot firmware sample | D: [`bspfiles/Packages/RPi.BootFirmware/kernel.img`][driver-boot-sample] | ARM32 firmware according to the BSP's IoT build instructions, not a Windows kernel | Binary tracked in tree; UEFI source is the separate repository in the firmware table | `dd06ff9a...`, 2020-02-18 |
| GPIO | D: [`drivers/gpio/bcm2836/bcmgpio.inf`][driver-gpio] | ARM + ARM64 project configurations | Open; ARM32 `bcmgpio.sys` in v0.9 and ARM64 counterpart in v0.17 | N |
| I2C | D: [`drivers/i2c/bcm2836/bcmi2c.inf`][driver-i2c] | ARM + ARM64 | Open; binary variants in v0.9/v0.17 | N |
| SPI0 | D: [`drivers/spi/bcm2836/bcmspi.inf`][driver-spi] | ARM + ARM64 | Open; binary variants in v0.9/v0.17 | N |
| AUX SPI | D: [`drivers/spi/bcmauxspi/bcmauxspi.inf`][driver-auxspi] | ARM + ARM64 | Open; binary variants in v0.9/v0.17 | `64697028...`, 2021-06-03 |
| PWM | D: [`drivers/pwm/bcm2836/bcm2836pwm.inf`][driver-pwm] | ARM + ARM64 | Open; binary variants in v0.9/v0.17 | N |
| PWM DMA helper | D: [`drivers/pwm/bcm2836/dma.cpp`][driver-dma] | Part of the PWM builds | Open code inside `bcm2836pwm.sys`, **not** the separate DMA HAL-extension implementation | `15f73cfc...`, 2020-07-15 |
| Arasan SD controller | D: [`drivers/sd/bcm2836/bcm2836sdhc/bcm2836sdhc.inf`][driver-arasan] | ARM + ARM64 | Open; `bcm2836sdhc.sys` in the inspected packages | `5c5e2742...`, 2021-06-03 |
| SDHOST | D: [`drivers/sd/bcm2836/rpisdhc/rpisdhc.inf`][driver-sdhost] | ARM + ARM64 | Open; `rpisdhc.sys` in v0.9/v0.17 | `5c5e2742...`, 2021-06-03 |
| VideoCore mailbox | D: [`drivers/mailbox/bcm2836/RPIQ.inf`][driver-mailbox-inf] | ARM + ARM64 project configurations | Open; `rpiq/rpiq.sys` in v0.9 and `RPIQ/rpiq.sys` in v0.17; a base package | N |
| USB DWC OTG | External package named by [RPiFM.xml][driver-manifest] | ARM32 `dwchsotg_hcd.sys`/`dwchsotg_hub.sys`; ARM64 `mcci_dwchsotg_*` | No matching Windows host-controller source in the surveyed repos; actual binaries verified in v0.9/v0.17 | No public implementation commit; manifest date 2020-03-19 |
| USB Ethernet | External LAN packages in [RPiFM.xml][driver-manifest] | ARM32 `lan9500-arm-n650f.sys`; ARM64 LAN9500/LAN7800 variants | Closed/source not located here; binaries verified | No public implementation commit |
| Onboard WLAN | External `RASPBERRYPI.RPi.Wifi.bcmdhd63.cab` in [RPiFM.xml][driver-manifest] | ARM32 `bcmdhd63.sys` and chip firmware in v0.9; no corresponding onboard-WLAN binary in inspected v0.17 | Closed/source not located here; **Pi3 wireless chip, not Pi2 onboard hardware** | No public implementation commit |
| Network property helper | D: [`drivers/RpiLanPropertyChange/bcm2836/RpiLanPropertyChange.inf`][driver-lan-helper] | ARM + ARM64 declarations | Open; ARM64 DLL verified in v0.17; **not** an Ethernet MAC driver | N |
| VCHIQ and userland transport | D: [`drivers/misc/vchiq/vchiq.inf`][driver-vchiq] and `drivers/misc/userland/` | ARM32 binaries verified; ARM/ARM64 configuration evidence | Open messaging code; v0.9 contains `vchiq.sys`, `vchiq_arm_kern.dll`, `vcos_win32_kern.dll`; not itself the GPU driver | N for the listed INF; library files have independent histories |
| Basic display/framebuffer | UEFI GOP plus Microsoft's OS basic-display implementation | Historical IoT display verified in documentation | Firmware source available; Windows basic display is OS-supplied, not source in this BSP | No public Windows implementation commit |
| VC4 render-only GPU sample | [`microsoft/graphics-driver-samples`, `render-only-sample/roskmd/RosKmd.h`][ros-header] and [`rosdriver/Ros.inf`][ros-inf] | ARM/ARM64 solution configurations; README explicitly describes Pi2 **ARM** demonstrations | Real Microsoft source for `roskmd.sys`/`rosumd.dll`; limited sample, not a complete accelerated desktop driver; no release binary provenance established here | `992be281...`, 2016-01-11 for header; entire `render-only-sample/` last changed `2b0b0540...`, 2020-04-01 |
| PWM analog audio | D: [`drivers/audio/bcm2836/rpiwav.inf`][driver-audio] | ARM + ARM64 | Open; `rpiwav.sys` in v0.9/v0.17; not evidence of Pi2 HDMI audio | N |
| Original Bluetooth UART transport | External `RASPBERRYPI.RPi.BtwSerialH5Bus.cab` in [RPiFM.xml][driver-manifest] | ARM32 `BtwSerialH5Bus.sys` and `.hcd` firmware in v0.9 | Source not located here; actual binary available; Pi3 hardware/function-specific limitations | No public implementation commit |
| Cypress Bluetooth transport | [`worproject/cywbtserialbus`, `src/vendor/cywbtserialbus.inx`][cywbt-inf] | ARM source configuration; ARM64-only standalone v1.0 release; ARM64 binary also in Pi4 v0.17 | Open transport source plus vendor firmware blobs; no ARM32 published asset verified for this replacement | `b2fa5cf8...`, 2020-12-13 |
| PL011 UART | D: [`drivers/uart/bcm2836/serPL011/SerPL011.inf`][driver-pl011] | ARM + ARM64; Pi2 ACPI explicitly enables matching `BCM2837` HID | Open; binaries in v0.9/v0.17 | `8685d39d...`, 2021-08-02 |
| Mini UART | D: [`drivers/uart/bcm2836/miniUart/pi_miniuart.inf`][driver-miniuart] | ARM + ARM64; **disabled in the inspected Pi2 ACPI** | Open; binaries in v0.9/v0.17; source existence does not override `_STA` | `147fd483...`, 2021-07-26 |
| System power management | ACPI PEP/reset described in firmware table; per-device power handling in drivers | No full Pi2 suspend/DVFS support established | No standalone Windows platform-power source/binary verified | N/A for unlocated Windows component |
| Camera | [Official IoT release notes][iot17763] and [USB hardware list][iot-peripherals] | CSI PiCam explicitly unsupported; selected ARM32 USB webcams supported | No CSI driver found; USB camera capability comes from appropriate OS/device drivers | No public CSI implementation commit |
| Touch | External `RASPBERRYPI.RPi.FT5406.Touch.cab` in [RPiFM.xml][driver-manifest] | ARM32 IoT package reference, tagged `RPI3_DRIVERS`; no ARM64 FT5406 package found | Source not located; not present in inspected standalone archives; image recovery only a candidate | No public implementation commit; manifest date 2020-03-19 |

The detailed inventory also includes Pi4-only `rpiuxflt` (USB filter), GENET,
and HDMI audio, to avoid mistaking their presence in a combined solution or
release for BCM2836 support. A declared `ARM` configuration on a Pi4 project
does not establish a working Pi4 ARM32 image.

### Binary availability: inspected rather than inferred

| Archive | Date / architecture evidence | Contents relevant to this investigation |
| --- | --- | --- |
| [`RPi_BSP.zip` at `17134_v5.3`][kit-v53] | Release 2018-09-20; all 14 SYS/DLL members inspected as ARMNT (`0x01c4`) | Open-BSP driver binaries and VCHIQ helper DLLs in `Rpi2/Packages/RPi.Drivers/`; **not** the complete Windows OS or all external `%MSPKG_DIR%` packages |
| [`RPi3_Windows_ARM32_Drivers_v0.9.zip`][drivers-v09] | Release 2021-04-09; still downloaded successfully during this investigation; all 17 SYS + 2 DLL members inspected as ARMNT | GPIO/I2C/SPI/PWM/SD/mailbox/UART/audio/VCHIQ plus closed USB, LAN, Pi3 WLAN and original Bluetooth |
| [Pi3 and Pi4 ARM64 driver ZIPs at `v0.17`][drivers-v017] | Release 2022-05-07; PE machine types inspected as ARM64 (`0xaa64`) | Useful architecture comparison, **not** usable native drivers for Cortex-A7 |
| [`cywbtserialbus` standalone `v1.0`][cywbt-release] | Release 2020-12-13; published asset is ARM64 Debug | Source has ARM configuration, but this release is not an ARM32 package |

Observed archive SHA-256 values:

```text
RPi_BSP.zip (17134_v5.3)
e4730143cec392fcb89b02afcf8925ee64f21371b0b98e8ea1ba045a57875d08

RPi3_Windows_ARM32_Drivers_v0.9.zip
f2e7fb95c4d93dac4d79172452fb1f585a6d84f655c98fbbe81dbf26d75a187c

RPi3_Windows_ARM64_Drivers_v0.17.zip
633595d5f825faf7e5dce962454ef9a711c474108030c439cc5e55e3959d60fd

RPi4_Windows_ARM64_Drivers_v0.17.zip
3c446231cffa6318aa802db52ab919be45b3eb5f45d9c7ffd3755bceac7c569d
```

These are locally observed content identities, **not** independent attestations
of vendor signatures or redistribution rights. PE `TimeDateStamp` fields in
v0.9 include implausible calendar dates, consistent with reproducible-build
hash fields; they were **not used as last-commit or build dates**.

Crucially, ARM32 components are **still obtainable**. Their absence from the
latest release is not their disappearance from history. Equally, a
Pi3-branded ARM32 archive is not proof of a tested, complete Pi2 v1.1 deployment.

### Other relevant ARM32 source/tooling and ARM64 false leads

| Repository / path | Architecture, source and binary status | Models / limitation | Last relevant path date |
| --- | --- | --- | --- |
| [`ms-iot/iot-adk-addonkit`, `Workspace/Source-arm/BSP/CustomRpi2`][kit-custom] | ARM32 package examples; source, not a whole OS | Pi2/Pi3 BSP customization, not desktop conversion | 2018-10-08, `50c31769...` |
| [`Workspace/Source-arm/Products/RPiRecovery`][kit-recovery] | ARM32 recovery image definitions; source | RPi2 BSP family | 2020-12-16, `d6c37311...` |
| [`Tools/IoTCoreImaging/Classes/IoTFFU.ps1`][kit-ffu] and [FFU export documentation][kit-export] | Windows-host tooling for mounting/exporting images; not ARM32 executable firmware | Needs the correct ADK/IoT tools and OS packages | 2018-11-14, `f54729f9...` |
| [`microsoft/MS_UEFI`, `MsIotSamples/MsIotSamples.dsc`][ms-sample-dsc], `NullSampleDxe/`, `NullHello/`, `BootLogoOverride/` | IA32/X64/**ARM** source configurations; generic loadable EFI samples, not a complete board firmware release | No Pi2 model guarantee; optional boot-logo/sample code | 2017-10-19, `ce5608b1...` |
| [`worproject/edk2`, `ArmPkg/Library/ArmLib/Arm/`][edk-arm-lib] | Generic ARMv7 source; no standalone Pi2 Windows binary | Library support is not board support | 2023-03-16, `28dce5b1...` |
| [`worproject/edk2-platforms`, `Platform/RaspberryPi/RPi3/RPi3.dsc`][platform-dsc] | **ARM64 only**; source | Pi2 v1.2/Pi3, explicitly not earlier Pi2 revisions | 2024-03-16, `207315a9...` |
| [`worproject/arm-trusted-firmware`, `plat/rpi/rpi3/platform.mk`][tfa-rpi3] | AArch64 TF-A; AArch32 payload flag does not make the firmware ARMv7 | Cortex-A53/Pi3-class boot, not BCM2836 | 2023-09-25, `07f867b1...` |
| [`worproject/RaspberryPiPkg`, `Drivers/HypDxe/HypWS.c`][legacy-hyp] | ARM64 HAL-patching source for old Windows builds | Pi3 context; not an ARM32 kernel component | `f94ed4effea31b0c0edc934cc10100fa59b8ea19`, 2018-09-18; fork README 2020-04-04 |
| [Microsoft IoT launcher/default-app samples][iot-launcher-sample] | ARM32-capable application source, not Explorer | Explicit Pi2/Pi3 ARM instructions; no new binary built here | 2021-02-26, `a96f5f00...` |
| Windows boot manager/loader/kernel/HAL | Closed OS components, not source in the surveyed firmware/driver repositories | Known working as part of the documented IoT product; exact image members/versions not independently extracted here | Public source-commit date unavailable |

## B. Complete reference-file search

The companion [reference-path inventory](pi2-arm32-reference-paths.tsv) lists
**every matching tracked text file** in the inspected repository snapshots,
not just selected examples. Each row carries the repository, full snapshot
commit, exact path, all matching line numbers, matched search terms and a
commit-pinned GitHub URL. No source-code dump is needed to reproduce a match.
It contains **1,119 repository/path records**, including the separately listed
identical Microsoft/WoR ARM32 firmware trees.

The requested case-insensitive expression was:

```text
BCM2836|Cortex-A7|ARMv7|Raspberry Pi 2|Pi2|RPi2|2836
```

The reference search uses `git grep -I -n -i -E` at each recorded commit.
`-I` excludes binary files; firmware/binary architecture inspection is recorded
separately. The bare `Pi2` and `2836` terms intentionally retain false-positive
substring matches such as `Acpi2`, `SPI2` and unrelated numeric test vectors.
**A row in this file is a search hit, not a compatibility claim.**

### Snapshot provenance

| Repository | Inspected ref / commit | Scope |
| --- | --- | --- |
| [`worproject/RPi-UEFI-Arm32`][arm32-uefi] | `ms-iot` / `5bfd48d674e6c7efea6e31f9eb97b9da90c20263` | Full tracked-text search; Pi2/Pi3 history, branch differences, tags and firmware release inspected |
| [`ms-iot/RPi-UEFI`][ms-uefi] | `ms-iot` / same `5bfd48d6...` | Identical default-branch tree; upstream provenance and refs checked |
| [`raspberrypi/windows-drivers`][drivers-source] | `master` / `88ee238c9debecce810d208cac1e5f36add3d2a1` | Full tracked-text search, 76 matching paths; BSP, INF/project configuration, history and binary samples |
| [`worproject/RPi-Windows-Drivers`][drivers-release-repo] | `master` / `b19eb98bd8a354349d47b0801360f72d0a9ac04a` | 1 matching text path; historical ARM32 status and release-asset history |
| [`ms-iot/rpi-iotcore`][ms-bsp] (old `ms-iot/bsp` URL redirects here) | `master` / `31e89330c37564d96e246a64210cfeaf8c45007c` | 67 matching text paths; original Microsoft BSP and ARM32 solution history |
| [`worproject/cywbtserialbus`][cywbt-repo] | `master` / `b9301327a566f0cc037258a326f5c8082890eca5` | **Zero requested-pattern text matches**; transport source/build/release evidence inspected separately |
| [`worproject/RaspberryPiPkg`][legacy-pkg] | `master` / `bcd8eaed2a2f6869a3ac848f91f98b901326e742` | Deprecated ARM64 fork; tables, HAL-patcher history and 29 matching text paths |
| [`worproject/edk2`][org-edk2] | `master` / `d7d4f09ff815794761f84d06e307001afe6376c4` | Full tracked-text search, 55 matching paths; generic ARM libraries, commit-message/tag search |
| [`worproject/edk2-platforms`][org-platforms] | `rpi5-dev` / `8e1779b538bcc1e6dc68d7df625394f933651d7a` | Full tracked-text search, 161 matching paths; Pi platform diff/deletion history |
| [`worproject/arm-trusted-firmware`][org-tfa] | `master` / `01610b0d32d746104dd4dacf179c65d69adaaf0a` | Full tracked-text search, 144 matching paths; ARM64 Pi3 port, history/tag search |
| [`worproject/Rockchip-Windows-Drivers`][rockchip] | `master` / `e00e70dabe17c3e150ea2b06b5a2b640bf0552b6` | Full tracked-text search, 3 matching paths; RK35xx is not BCM2836 |
| [`worproject/rpi5-uefi`][rpi5] | `master` / `a6135b06d661b5f11bbf4bd742b42a1919b264dc` | **Zero requested-pattern text matches**; BCM2712-only scope and history checked |
| [`worproject/WoR-Imager-Translations`][org-translations] | `master` / `1712b199d83b14eb1837980d7d0dac5cf7d9dd45` | **Zero requested-pattern text matches**; language files, not installer implementation |
| [`worproject/dldserv-mirror`][org-mirror] | `master` / `a3e7c26213d6717f805b0f2f2b9adc77289b215a` | **Zero requested-pattern text matches**; release `13/02/2024` and WoR-PE binary inspected separately |
| [`MicrosoftDocs/windows-iotcore-docs`][iot-docs-repo] | `main` / `5ce0ec708c82a33d6ba0900d9d6ba4a6d2ce5877` | Full tracked-text search, 34 matching paths; primary OS/board/API documentation |
| [`ms-iot/iot-adk-addonkit`][kit-root] | `master` / `3a9cf8d2ab3506f3dcd51940c48db7e5096575e0` | Full tracked-text search, 28 matching paths; full history, tags, package migration and FFU commands |
| [`microsoft/MS_UEFI`][ms-uefi-samples] | branch **`share/MsIoTSamples`** / `ce5608b1f0d45bf2bab8b00b95982829abb60a36` | Full tracked-text search, 121 matching paths, mostly inherited EDK2; actual IoT samples declare IA32/X64/ARM |
| [`WindowsOnARM32/Surface2Setup`][surface-readme] | `main` / `c6185162dd255c56318855f481682862a7fe0c2a` | **Zero requested-pattern text matches**; a Surface desktop workflow, not Pi2 evidence |

The two identical ARM32 firmware trees each yield 200 matching text paths
(877 matching lines). They remain separate repository entries for traceability.
The WoR organization list was retrieved from
[`/users/worproject/repos`](https://api.github.com/users/worproject/repos?per_page=100);
all eleven returned repositories are covered above.

Additional narrowly scoped citations include the two official IoT application
samples, Win86emu, the FFU converter and Microsoft's
[`graphics-driver-samples` render-only driver][ros-readme].
These were read at the cited paths,
not presented as exhaustive whole-repository history searches. Similarly,
`driver1998/bsp` was checked as a provenance pointer/fork, not treated as an
independently tested OS port.

### Reproducing the searches

Run these read-only commands in an already obtained source checkout, using the
full revision recorded in the table:

```bash
git --no-pager grep -I -n -i -E \
  'BCM2836|Cortex-A7|ARMv7|Raspberry Pi 2|Pi2|RPi2|2836' <snapshot>
git --no-pager log --all --regexp-ignore-case --extended-regexp \
  --grep='ARM32|WIN32|IoTCore|RPi2|BCM2836' \
  --format='%H %cI %s'
git --no-pager log --all -i -G 'ARM32|WIN32|IoTCore|RPi2|BCM2836' -- <relevant-path>
git --no-pager log --all --diff-filter=DR --name-status -- <relevant-path>
git --no-pager log -1 <snapshot> --format='%H %cI %s' -- <component-path>
git rev-parse '<tag>^{commit}'
```

`git grep` returning 1 means no matches, not a successful compatibility check.
Release **publication dates and attached binaries** must additionally come from
the releases API or actual assets; many driver tags point to the same README
commit.

## C. Driver and model matrix

**Legend:** "package evidence" means a source/INF/feature-manifest or archived
status claim, **not a new hardware test**. The existing driver source
[README][drivers-readme] explicitly lists Pi2/Pi3/Compute Modules for
32-bit IoT Core, and separately lists Pi2 **v1.2** for ARM64.
Where the manifest does not distinguish v1.1/v1.2, the matrix says so.

`B32` = inspected historical ARM32 v0.9 package; `B64` = inspected ARM64 v0.17
package; `BSP` = Microsoft-linked `RPi_BSP.zip`. ARM64 entries never apply to
native execution on Pi2 v1.1.

| Driver / component | Pi2 v1.1 evidence | Pi2 v1.2 evidence | Pi3 evidence | ARM32 | ARM64 | Source available? | Binary available? |
| --- | --- | --- | --- | --- | --- | --- | --- |
| GPIO | Original BCM2836 source + `RPI2_DRIVERS` + Pi2 ACPI binding | Shared Pi2 package, not revision-tested here | Shared package baseline | Yes, config + PE | Yes, config + PE | Yes | B32, BSP, B64 |
| I2C / SPI / AUX SPI | Pi2 feature group and matching ACPI IDs | Same, no revision split | Shared peripheral source/package | Yes | Yes | Yes | B32, BSP, B64 |
| PWM / embedded DMA | `RPI2_DRIVERS`; DMA helper is part of PWM | Same package | Shared baseline | Yes | Yes | Yes | B32/BSP/B64; no separate DMA helper binary |
| Mailbox `rpiq` | Mandatory base package | Same base package | Same base package | Yes | Yes | Yes | B32, BSP, B64 |
| Arasan SD / SDHOST | Mandatory storage packages and Pi2 ACPI | Shared storage; separate firmware route for ARM64 | Package and archived/current status evidence | Yes | Yes | Yes | B32, BSP, B64 |
| DWC USB host/hub | Pi2-named base package; official IoT support, no new exact-revision test | Shared package/controller evidence | Archived ARM32 and ARM64 working-status entries | Yes, `dwchsotg_*` | Yes, separate `mcci_dwchsotg_*` | Not in surveyed sources | B32, B64; absent from the small BSP ZIP |
| LAN9514 USB Ethernet | Pi2 USB-LAN hardware / base-package evidence; no new board test | Shared board-class evidence | Archived/current working status | Yes | Yes | Source not located | B32 LAN9500 and B64 variants |
| LAN7515 / LAN7800 | Not Pi2 onboard Ethernet | Not Pi2 onboard Ethernet | Pi3B+ status, not Pi3B's LAN9514 | Historical status only for ARM32 variant, not recovered in B32 | Yes | Source not located | B64 verified; do not infer ARM32 asset from a README row alone |
| Onboard Wi-Fi | **No onboard radio**; external dongles separate | **No onboard radio** | BCM43438 ARM32 driver and archived status; image/version limits | Yes for Pi3 | No matching package in inspected B64 | Source not located | `bcmdhd63.sys` + chip firmware in B32 |
| External USB Wi-Fi / Ethernet | Named ARM32 adapters in Microsoft's [list][iot-peripherals] | Same device-specific documentation, not revision tests | Same | Documented per adapter | Not established by that ARM32 list | Depends on chipset; not a generic "Wi-Fi class" | Appropriate OS/vendor driver needed, not a blanket BSP guarantee |
| Firmware GOP / OS basic display | Official IoT GUI plus Pi2 `DisplayDxe` | Same IoT family; ARM64 firmware is different | Official IoT GUI / WoR framebuffer status | Yes | Yes, separate platform path | GOP yes; OS driver closed | Firmware/OS supplied, not full GPU acceleration |
| VC4 `roskmd` / `rosumd` sample | Microsoft explicitly reports Pi2 ARM demonstrations | No revision-specific evidence | WoR reports unfinished driver, not full support | Source configuration and demo evidence | Configuration, not Pi3 hardware proof | **Yes**, separate Microsoft sample | No binary provenance/working desktop validated here |
| VCHIQ | Source/shared IP; manifest selects it via `RPI3_DRIVERS`, not Pi2 certification | Same limitation | Historical ARM32 partial/crashing; current ARM64 not working | Yes, PE | Source config; functional status negative | Yes | B32/BSP; not a complete GPU driver |
| Analog PWM audio `rpiwav` | `RPI2_DRIVERS` package | Same package | Shared source/package | Yes | Yes | Yes | B32, BSP, B64 |
| USB audio | Named ARM32 class-compatible adapter in [official list][iot-peripherals] | Same caveat | Same | Documented | Not tested here | OS implementation not in these repos | OS supplied for compatible device |
| Original Bluetooth `BtwSerialH5Bus` | **No onboard Bluetooth** | **No onboard Bluetooth** | `RPI3_DRIVERS`; official low-bandwidth/pairing limitations | Yes for Pi3 | Not in inspected B64 | Not located | B32 plus vendor firmware |
| Replacement `cywbtserialbus` | No Pi2 hardware support claim | No Pi2 hardware support claim | README reports Pi3B testing; also Pi4 tests | Source config only | Source + release PE | Yes, transport code | ARM64 standalone release and Pi4 B64; no ARM32 asset verified |
| PL011 UART | **Explicitly enabled by Pi2 `Uart.asl`**, despite HID `BCM2837` | Controller/firmware binding, not separate test | Source/status evidence; Bluetooth shares PL011 on Pi3 | Yes | Yes | Yes | B32, BSP, B64 |
| Mini UART | **Disabled in inspected Pi2 ACPI** | Depends on firmware; no separate test | Source/package evidence | Yes | Yes | Yes | B32, BSP, B64; availability does not enable ACPI device |
| System power / PEP | ACPI source + firmware reset; no full DVFS/suspend proof | Same limit | Same limit | Metadata/device handling | Separate path, not qualified here | ACPI and device code yes; complete Windows PEP not located | No standalone Windows PEP verified |
| CSI camera | Official PiCam **unsupported** | Same IoT limitation | Same limitation | No supported CSI path established | No supported CSI path established | No CSI implementation located | No CSI binary verified |
| USB camera | Named webcams in [official list][iot-peripherals]; USB performance caveats | Same | Same | Documented for selected devices | Not qualified here | Inbox/vendor-specific, not this BSP source | Appropriate OS image supplies required support |
| FT5406 touch | No Pi2-specific working evidence in inspected package metadata | Same | External `RPI3_DRIVERS` package; Pi3B+ preview disables it | Package reference only | No package verified | Not located | Not in inspected standalone archives; image recovery unverified |

Pi2 Model B itself has microSD, not onboard eMMC. Firmware/controller code
for eMMC, and the later Pi3/Compute Module changes, must not be described as
adding an eMMC device to Pi2 or proving every Compute Module variant works.

## D. Desktop Windows feasibility

| Candidate or capability | Evidence | What can actually be concluded |
| --- | --- | --- |
| Windows 10 IoT Core ARM32 | **Verified.** Microsoft explicitly supports Cortex-A7/BCM2836 in [15063][iot15063] and [17763.253][iot17763]. | A Windows NT-derived ARM32 OS can run on Pi 2 v1.1. It is not a desktop edition. |
| Windows RT / RT 8.1 | **Indirect for this board.** [Surface2Setup's documentation][surface-readme] and [deployment script][surface-script] target Surface 2, not a Raspberry Pi. | ARM32 desktop-oriented Windows existed. Neither Surface firmware nor Tegra drivers establish BCM2836 compatibility. No reproducible Pi 2 v1.1 RT boot was established here. |
| Windows 10 ARM32 desktop, build 15035 | **Verified existence of community deployment instructions; indirect OS evidence; no Pi validation.** [Surface2Setup][surface-readme] explicitly references a 15035 image workflow, with a separate WIM and recovery environment. | A research candidate, not an officially obtainable/supported Pi desktop product. This investigation did not download, authenticate, execute or redistribute that OS image. |
| A `Desktop FFU` comment in ARM32 firmware | **Verified literal source text; indirect feasibility clue.** [The Pi2 DSC][arm32-pi2-dsc] labels its active Windows boot-manager path `# Desktop FFU` and a commented alternative `# Mobile FFU`. | Worth preserving as evidence, but a boot-path/layout comment is not a supported desktop-SKU declaration, desktop image, or successful Explorer boot. |
| Windows on ARM Insider builds | **Two distinct families.** [Microsoft's IoT notes][iot15063] discuss IoT Insider images; the community Surface workflow concerns a desktop preview. | The word *Insider* does not make the image desktop, ARM64, or compatible with all Pi revisions. In particular, the [Pi 3B+ IoT preview explicitly will not boot Pi 2][iot3bplus]. |
| Explorer shell | **Verified absent as the supported IoT desktop experience.** [IoT app documentation][iot-apps] excludes classic HWND APIs; [IoT shell][iot-shell] launches headed UWP apps. | Copying `explorer.exe` into IoT does not supply USER/GDI/window-manager dependencies, matching system DLLs, OS packages or their licensing. A Device Portal web page called "File Explorer" is not Explorer running on the Pi. |
| Win32 subsystem/API support | **Verified, but limited.** [Non-UWP app documentation][iot-apps] supports C++ console apps and NT services while explicitly excluding `CreateWindow`/`CreateWindowEx`, MFC, WinForms and WPF. | "IoT cannot run any Win32 code" is false. "IoT is a full Win32 desktop" is also false. Applications and dependencies must target the image's architecture and supported API set. |
| Microsoft x86 emulation | **Verified architecture mismatch.** [Microsoft's emulator documentation][ms-emulation] says it translates x86 to ARM64 and requires ARM64 kernel drivers. | That emulator cannot be transplanted as an ARM32 feature for Cortex-A7. No Microsoft-provided x86 emulation was established for the inspected ARM32 IoT/RT path. |
| Third-party x86 emulation | **Verified separate project; no Pi evidence.** [Win86emu][win86emu] is an abandoned user-mode emulator for Windows RT, with source/archive content in that repository. | ARM32 x86 emulation is not theoretically impossible. This project does not supply an IoT desktop API stack, a BCM2836 port, or proof of usable Pi performance. |
| Current Windows 10/11 ARM64 | **Verified hardware incompatibility.** [BCM2836][soc2836] is Cortex-A7; the [current WoR requirements][wor-downloads] specify ARM64 and Pi 2 **rev 1.2**. | Native execution on rev 1.1 is not possible. Changing a model number, BCD entry, CPU check, or filename cannot add AArch64 instructions. Full-system emulation would be a different project, not native WoR support. |

### A realistic graphical IoT experience

IoT Core can have a local HDMI graphical interface. A custom UWP launcher,
dashboard, kiosk, controller or thin-client application is a plausible useful
project. The [IoT shell documentation][iot-shell] supports switching between
registered foreground apps, one foreground app at a time, with background apps
and services. This should be advertised as **IoT/kiosk UI**, not as an Explorer
desktop or general PC application compatibility.

There is existing Microsoft sample code to start from:
[`Samples/IoTStartApp`][iot-launcher-sample] enumerates installed applications
with `PackageManager` and launches the selected app. Its instructions explicitly
select **ARM for Raspberry Pi 2 or 3**. The more extensive
[`Samples/IoTCoreDefaultApp`][iot-default-sample] is also customizable source.
The latest changes to these sample directories at the inspected ref are
`a96f5f00fd7e3305cd19d590eced8f0ae7321f1a` (2021-02-26). They are useful UI
prior art, not an Explorer replacement or proof that old dependencies rebuild
unchanged with today's SDK.

The [official processor capability table][iot-socs] describes Broadcom graphics
as software-rendered. The [17763.253 release notes][iot17763] warn about video
performance and state that the onboard PiCam/CSI camera is unsupported.
Some USB webcams, Wi-Fi adapters, Bluetooth dongles and audio adapters have
explicit ARM32 entries in the [hardware compatibility list][iot-peripherals].
Those device-specific entries are more useful than a blanket claim that USB
peripherals either all work or all fail.

## E. ARM32 UEFI and BCM2836 boot-chain analysis

### ARM32 UEFI exists, including an actual downloadable firmware package

[`worproject/RPi-UEFI-Arm32`][arm32-uefi] is a fork of
[`ms-iot/RPi-UEFI`][ms-uefi], not an ARM64 firmware with a misleading name.
Both default `ms-iot` branches were inspected at
`5bfd48d674e6c7efea6e31f9eb97b9da90c20263` (2019-08-12).

[`Pi2BoardPkg/Pi2BoardPkg.dsc`][arm32-pi2-dsc] declares
`SUPPORTED_ARCHITECTURES = ARM` and selects the ARMv7 library.
[`Pi3BoardPkg/Scripts/startup.S`][arm32-startup] reads the actual CPU MIDR:

- Cortex-A7 part number `0xC07` selects the Pi2 firmware.
- Cortex-A53 part number `0xD03` selects the Pi3 firmware.
- An unrecognized part parks instead of proceeding.

This is unusually strong evidence for the original Pi2: it selects the A7
silicon explicitly, rather than relying only on a board-name string. The A53
branch also explains why a "Pi2" revision with BCM2837 is a different firmware
case from an original BCM2836 board.

The community [firmware release `v1.0`][arm32-uefi-release], published
2020-05-05, contains:

```text
bootcode.bin
config.txt
fixup.dat
kernel.img
start.elf
```

Its asset is `RPi2-3_ARM_UEFI_Firmware_v1.0.zip`, 2,603,063 bytes, with observed
SHA-256 `aab3481e44b8c7f89722669dcd73b524669614d6639f24d67c58370d82445656`.
The first instruction in `kernel.img` matches the A32 MIDR-read instruction
in the source. This is a static consistency check, **not** a reproducible-build
attestation or a successful boot test.

The upstream Microsoft repository has no standalone GitHub release at this
snapshot. That must not be described as "Microsoft never shipped the firmware":
the official IoT image is a separate binary-distribution channel.

### The expected boot stages

```text
Pi boot ROM / VideoCore boot firmware
  -> bootcode.bin, start.elf, config.txt and matching fixup data
  -> kernel.img (combined ARM32 UEFI, not ntoskrnl.exe)
  -> MIDR selection: Cortex-A7 -> Pi2BoardPkg
  -> ARM32 UEFI boot services + ACPI installation
  -> Windows ARM32 boot manager
  -> Windows loader, kernel/HAL and boot-critical storage stack
  -> IoT shell, or a desktop only if a compatible desktop OS stack exists
```

The Pi2 DSC's `PcdDefaultBootAppPath` explicitly selects
`\efi\microsoft\boot\bootmgfw.efi`
([source][arm32-pi2-dsc], line 495). The firmware therefore has a real Windows
boot-manager handoff, not merely a hypothetical ability to display a logo.
No Windows `bootmgfw.efi`, `winload.efi`, `ntoskrnl.exe` or desktop OS is included
in the standalone firmware release.

The adjacent build-time branches matter. `RAMDISK_BOOT_ENABLE` and
`MBRGPT_WORKAROUND_ENABLE` select full device paths whose final component is
the literal `/bootarm.efi`; the `!else` branch uses the Windows boot-manager
path above. They are mutually exclusive configurations, not three fallback
paths known to execute in every build. The source does **not** spell those
device-path suffixes as `\EFI\BOOT\BOOTARM.EFI`, so this report does not infer
that extra directory. The comments label the active path `# Desktop FFU` and
the commented `\Windows\boot\Efi\bootmgfw.efi` alternative `# Mobile FFU`.
These are real desktop-related references, but their exact SKU implications
are not established.

The architecture of the complete chain must match. Renaming an ARM64 loader
to an ARM32 filename cannot work. Firmware-stage SD/USB/display drivers also
do not automatically replace the Windows drivers needed after
`ExitBootServices`.

### BCM2836 ACPI is present

The inspected ARM32 source includes the following exact table paths under
`Pi2BoardPkg/AcpiTables/`:

| Table / role | Exact source | Important evidence |
| --- | --- | --- |
| DSDT/device namespace | [`Common/DSDT.asl`][arm32-dsdt] | Root namespace and board device descriptions; last touched 2017-04-14. |
| FADT/fixed platform description | [`Common/Fadt.aslc`][arm32-fadt] | The encoded FACP header contains OEMID `BC2836`. |
| MADT/CPU and interrupt topology | [`Common/Madt.aslc`][arm32-madt] | Encoded OEMID `BC2836`; the source explains that the GIC descriptors are placeholders for multicore/parking because BCM2836 has no GIC. |
| GTDT/timer description | [`Common/Gtdt.aslc`][arm32-gtdt] | Timer-description source is available; presence alone is not a guarantee of desktop-kernel compatibility. |
| CSRT/core resources | [`Common/CSRT.aslc`][arm32-csrt] and [`Common/Platform.h`][arm32-acpi-platform] | Macro-derived OEMID is `MCRSFT`, OEM Table ID `RPI2EDK2`, Creator ID `RPI2`. A nearby `BC2836` comment is stale; actual fields matter. |
| UART/debug transport | [`Uart.asl`][arm32-uart] | PL011 uses `_HID "BCM2837"` even in the Pi2 package. Mini UART uses `"BCM2836"` and is disabled in that Pi2 table. These IDs name interfaces, not a requirement for those exact silicon numbers. |
| Power plugin | [`Common/PEP.asl`][arm32-pep] and [`Common/pep.h`][arm32-pep-h] | PEP virtual device and power-management definitions; not the implementation of a Windows HAL or proof of complete suspend/DVFS support. |

[`Pi3BoardPkg/AcpiTables/AcpiTables.inf`][arm32-pi3-acpi] references the common
Pi2 table sources. Consequently a `BC2836` OEMID can also occur in firmware
for a BCM2837 board. Neither an OEMID nor an INF hardware ID by itself
establishes exact-revision test coverage.

### The important unresolved boundary: firmware versus Windows HAL

Do not mistake the public UEFI's BCM interrupt implementation for proof that
an arbitrary Windows kernel can handle BCM2836 interrupts at runtime.
The source's placeholder MADT explicitly acknowledges the non-GIC hardware.
[Microsoft's ACPI guide][ms-csrt] separately explains how CSRT resource-group
identifiers let Windows locate SoC-specific **HAL extension DLLs** for
nonstandard core resources.

In this firmware's [actual CSRT initializer][arm32-csrt], there is one resource
group: vendor `MSFT`, device ID `9`, subvendor/subdevice/revision `0`. It
describes **DMA**, with controller type/subtype `3/1` and channel descriptors
`3/0`; controller addresses include `0x3F007000` and `0x3F007FE0`. There is no
interrupt-controller resource group in this table. Thus it gives concrete
DMA/HAL integration evidence, but does not answer how the closed Windows
kernel supports the non-GIC interrupt controller.

Three different things must be kept separate:

1. A UEFI DXE interrupt driver, used while firmware services operate.
2. ACPI descriptions and the Windows kernel/HAL support that interprets them.
3. A PEP, concerned with platform power management.

No public `HalExtBcm2836` source or matching closed Windows module was verified
by this investigation. That is **not evidence that the historical IoT image
lacked a working HAL**, and it does not establish the name or precise packaging
of the mechanism in Microsoft's closed binaries.

There is useful but architecture-limited historical evidence in
[`worproject/RaspberryPiPkg`][legacy-pkg]: its
[`Drivers/HypDxe/HypWS.c`][legacy-hyp] contains an AArch64, version-sensitive
Windows HAL patching mechanism. Its [README][legacy-pkg-readme] explains that
some older ARM64 Windows builds needed it for the Broadcom interrupt
controller. This is **not an ARM32 DLL or drop-in fix for Pi2 v1.1**.
It demonstrates why exact kernel/firmware pairing matters, rather than proving
that RT or build 15035 can use the IoT boot stack unchanged.

## F. Existing, recoverable and missing pieces

### For an IoT Core installation

The historical OS/board combination already existed. Prefer a complete,
matching Raspberry Pi IoT FFU over assembling unrelated firmware and drivers.
The [official BSP workflow][iot-bsp] uses an `arm` workspace and imports
`RPi_BSP.zip`; [image creation][iot-image-build] produces a complete FFU.

What an appropriately licensed local IoT image can potentially provide:

| Image area | Recoverable material | Important limit |
| --- | --- | --- |
| Boot/EFI partition | Raspberry Pi boot files, ARM32 UEFI payload, Windows boot manager, BCD and possibly embedded ACPI tables | Keep the same board/build set first. A file named `kernel.img` in this platform is firmware, not automatically the Windows NT kernel. |
| MainOS | ARM32 Windows loader/kernel/HAL, inbox class drivers, system DLLs and installed BSP driver packages | Closed Microsoft binaries are not public kernel source and are not a desktop license or desktop API package. Verify actual PE architecture and version before reuse. |
| Driver store and package metadata | INF/CAT/SYS/DLL files, hardware IDs, service registration, package versions and signing metadata | A SYS file alone may be insufficient. Preserve the matching INF, catalog, supporting files and configuration. Cross-build HAL/driver ABI and signing compatibility are unproven. |
| ImageUpdate/device layout | Partition definitions, provisioning and feature/package manifests | Required to reproduce the image layout; not interchangeable with WoR's two-partition recovery-media layout. |
| IoT apps and shell | Default UWP shell/application and supported background-service environment | Does not yield Explorer, a full desktop Win32 stack, or Microsoft's ARM64 x86 emulator. |

The image recovery procedure is itself version-sensitive.
[Microsoft's IoT DISM page][iot-dism] explicitly warns that DISM mounting/offline
servicing of Mobile/OneCore FFUs is unsupported. The official kit instead has
[an `IoTFFU` class that invokes `wpimage mount`][kit-ffu] and
[exports mounted EFIESP/MainOS/Data as separate WIMs][kit-export].
Do not blindly apply modern desktop `/Mount-Image` instructions to old IoT FFUs.

### What needs rebuilding, and what does not

| Goal | Existing/archived material | Rebuild or missing work |
| --- | --- | --- |
| Reproduce stock IoT Core | A complete matching official FFU, ARM32 firmware and drivers in that image | Normally **no firmware/kernel rebuild**. Safely acquire, validate and apply the correct full image; then prove boot on the exact board. |
| Customize ARM32 UEFI | Public [Pi2/Pi3 build scripts][arm32-build], DSC/FDF, ACPI and firmware source | Reconstruct the historical ARM build toolchain, rebuild only necessary firmware changes, compare output and boot-test. Closed VideoCore blobs cannot be rebuilt from these sources. |
| Update an open BSP driver | [GPIO/storage/UART/audio and related source][drivers-readme] plus ARM configurations | Build with an appropriate WDK/SDK, preserve INF/package/hardware-ID contracts, satisfy the target image's signing requirements, and test. A project configuration is not proof that a current toolchain builds it successfully. |
| Replace closed USB/network/touch pieces | Archived binaries/package references, possibly the appropriate IoT image | Recover a compatible licensed binary/package or obtain a replacement implementation. The open BSP does not provide all of these implementations. |
| Change complete image composition | [IoT addonkit][kit-readme], BSP and OS feature packages | Use mutually compatible kit/ADK/OS/BSP versions; create and validate a new FFU. The old BSP ZIP alone cannot manufacture an OS. |
| Add the installer selection | Existing Linux/macOS safety, UI, cache and progress infrastructure | Implement the separate FFU backend/profile and its tests; do not merely rebuild firmware or enable a board label. |
| Boot a full desktop build | ARM32 UEFI/BSP knowledge and historical non-Pi ARM32 desktop evidence | Exact desktop image and BCM2836 kernel/HAL/driver integration remain unverified; the Windows desktop kernel/user-mode stack is not buildable from these public BSP sources. |

### For desktop Windows on rev 1.1

The unresolved deliverable is a **complete and legally available ARM32 desktop
boot/system image with a demonstrated BCM2836-compatible boot-critical stack**.
That is a stronger requirement than finding an ARM32 UEFI repository.

Work would have to establish, in order:

1. An identified ARM32 desktop build and lawful acquisition/usage path, with
   recorded hashes and architecture of its boot manager, loader and kernel.
2. A consistent firmware/ACPI/interrupt-timer/HAL-extension contract for that
   exact kernel, including any required registry/package registration and
   signing policy. An IoT driver building successfully does not establish
   compatibility with Windows RT 8.1 or a different Windows 10 preview.
3. Boot storage and enough I/O for debugging and installation; UART output is
   useful before display, USB or network drivers start.
4. Matching ARM32 desktop user-mode and windowing components, followed by a
   desktop shell. Importing a single executable cannot replace an OS SKU.
5. Actual boot traces and reproducible tests on a **BCM2836** board, not a
   BCM2837 Pi 2 v1.2 or Pi 3.

No inspected evidence establishes which of these would be the *first observed*
desktop boot failure. Naming a definitely missing HAL binary, definitely
missing UEFI, or a single guaranteed patch would therefore overstate the
evidence. Existing IoT components make experimentation plausible; they do not
make the desktop port demonstrated.

## G. Repository history and migration

### ARM32 firmware versus the separate ARM64 fork

| Commit / ref | Date | What changed |
| --- | --- | --- |
| [`42edc35271384c6d62233419231c918711ada2d3`][arm32-import] | 2016-10-21 | Imports the Pi2/Pi3 ARM32 UEFI source for the RS1 release. The inspected board-package history has no deletion/rename cycle that would imply BCM2836 was removed and later restored. |
| [`4726af1381dd3ebac326b73c802d60fef1531179`][arm32-rs2] | 2017-03-06 | RS2 update changes the mailbox, reset and SMBIOS implementation; these components are newer than the initial RS1 import. |
| [`2e4b7dd1fd02b0d2fd7712d37aa31de07e98eba6`][arm32-dsdt-fix] | 2017-04-14 | Updates the Pi2 common DSDT for the boot-volume sentinel. |
| [`19acdb79a2676d0fa09d15c60f3b5f69bc725656`][arm32-3bp-change] | 2018-09-28 | Branch-specific Pi3B+ LED/MAC changes; not evidence that the standard Pi2 image becomes a Pi3B+ preview image. |
| [`48fd8bb20dd4d45a4cf0a8970a65837e45bbaa99`][arm32-emmc-change] | 2019-01-05 | Adds eMMC support/extended GPIO work in **Pi3BoardPkg**; does not modify Pi2BoardPkg. |
| [`799727f31f2e7eee5f8fa55a538f9be6357cf31c`][arm32-emmc-fix] | 2019-04-17 | Fixes the Pi3 MMC ECSD command argument. A Pi3-specific improvement, not an ARM32-to-ARM64 transition. |
| [`0e2bf04ff3a48e7bf25092f1e1fd7dca16c6f109`][arm32-tpm-change] | 2019-08-06 | Software/discrete TPM enablement in firmware. This does not certify every physical board has a TPM or enable a modern supported Windows desktop. |
| [`5bfd48d674e6c7efea6e31f9eb97b9da90c20263`][arm32-display-fix] | 2019-08-12 | Fixes a low-resolution display crash; final inspected default-branch commit and the target of firmware `v1.0`. |
| [`v1.0` firmware release][arm32-uefi-release] | Published 2020-05-05 | A real ARM32 Pi2/3 ZIP. Release publication is distinct from the 2019 source/tag commit. |
| `v2.0-rc0`, `v2.0-rc1`, `v2.1-rc0` | 2014 ancestor tags | Inherited ARM/Linaro history, **not** newer WoR Pi firmware releases. Do not choose the highest-looking tag by SemVer alone. |
| [`359e2baf9985187fb00cc5a84b84035f5225cb58`][arm32-dirty-base] on `rpi4-dirty-port` | 2021-09-19 | Changes Pi2-named peripheral bases from `0x3F000000` to the Pi4 `0xFE000000`. Those addresses are inappropriate for a BCM2836 build. The default `ms-iot` branch is unaffected. |
| [`02e5f637ab18c0f239aabe8cef350d292d1c830d`][arm32-dirty-cpu] on the same branch | 2021-09-19 | Replaces the Cortex-A53 selector with Cortex-A72 rather than adding another supported Pi target. This experimental branch must not be treated as a newer universal Pi2/3 release. |
| [`bcd8eaed2a2f6869a3ac848f91f98b901326e742`][legacy-final] in `worproject/RaspberryPiPkg` | 2020-04-04 | Deprecation/documentation snapshot of the **separate ARM64** `andreiw/RaspberryPiPkg` lineage. It contains Microsoft-derived tables and an old HAL patcher, not an ARM32 replacement. |

The [`RaspberryPiPkg` README][legacy-pkg-readme] says the then-current upstream
UEFI does not include its `HypDxe` mechanism for older Windows builds. This
establishes a difference between the legacy fork and newer firmware; it is not,
without a corresponding deletion commit, proof that that exact directory was
ever merged into and then removed from `edk2-platforms`.

### Microsoft image-building kit

These are changes in [ms-iot/iot-adk-addonkit][kit-root], not guesses based on
directory names:

| Commit / tag | Date | Evidence and interpretation |
| --- | --- | --- |
| [`7223e44eda561b23425de99e6f441e0a25aad134`][kit-pi3-change] | 2016-05-27 | "Updates to RPi2 BSP to support RPi3"; changes the ARM workspace's RPi2/CustomRpi2 feature manifests. **RPi2 names can cover Pi3 as well**, without implying ARM64. |
| [`f6c2018ec11581813ae74eb48219763a681fb0dd`][kit-bsp-external] | 2017-01-21 | RPi2 BSP switches to drivers from `ms-iot/bsp`. A package migration, not removal of BCM2836 support. |
| [`RPiBSP`][kit-rpibsp-tag] -> `5c57454d677689241ed772c93d416de858527a5b` | Commit 2017-01-24; release 2017-01-25 | Archived `rpibsp.zip` release asset. |
| [`7524312d0a89f56c0755cc5091ab2834e9d5dba8`][kit-wm-change] | 2017-10-17 | Migration to `wm.xml` package format. Removed/replaced package files must not be confused with dropped ARM32 support. |
| [`v4.4`][kit-v44] -> `bab0f1ae93b5d64a8a1782addf38122662aad50d` | Commit 2018-05-16; release 2018-05-17 | Archived `rpibsp-wm.zip` asset. |
| [`17134_v5.3`][kit-v53] -> `0bcededaed7a1bb30334c708a3bf838e87e3df40` | Commit 2018-09-05; release 2018-09-20 | `RPi_BSP.zip`, linked explicitly by the official [BSP documentation][iot-bsp]. This is not a full Windows image. |
| [`50c31769c7cd5bb6e34731d6d1f7984fb0400da8`][kit-remove-bsp] | 2018-10-08 | Commit explanation: "Remove RPi2 BSP - always import." Five RPi2 package/example files are deleted, but the [import command][kit-import] remains. **Removed bundled definitions are not proof of removed OS support.** |
| [`v6.0`][kit-v60] | Release 2020-12-08 | Kit for 1809/17763; [README][kit-readme] retains links to older build-matched versions and explains newer OEM-signing requirements. |

The five files deleted by `50c31769...` were:

- `Workspace/Source-arm/BSP/Rpi2/OEMInputSamples/RetailOEMInput.xml`
- `Workspace/Source-arm/BSP/Rpi2/OEMInputSamples/TestOEMInput.xml`
- `Workspace/Source-arm/BSP/Rpi2/Packages/RPi.Customization/RPi.Customization.wm.xml`
- `Workspace/Source-arm/BSP/Rpi2/Packages/RPi2FM.xml`
- `Workspace/Source-arm/BSP/Rpi2/Packages/RPi2FMFileList.xml`

Their deletion and previous content are available in the
[commit diff][kit-remove-bsp]. Current
[`Workspace/Source-arm/BSP/CustomRpi2`][kit-custom] and
[`Workspace/Source-arm/Products/RPiRecovery`][kit-recovery] still provide
ARM32 customization/recovery examples. Their latest path changes are
`50c31769...` (2018-10-08) and
`d6c37311cfb8d5c52b0a7284e2878c607690239b` (2020-12-16), respectively.

### Driver migration: ARM64 was added, not proof that ARM32 disappeared

| Event | Pinned evidence | Interpretation |
| --- | --- | --- |
| Original Pi2 GPIO source, 2015-07-14 | [`c907d0dd69fe035fb23d28ccae736d2d4e3ee413`][gpio-origin] | Explicitly a Raspberry Pi 2/BCM2836 driver, predating the later Pi3/BCM2837 boards. |
| ARM64 project configurations added, 2020-07-15 | GPIO [`318fba8eacb32ab0ff8205e9dfd475cbbefe8960`][gpio-arm64]; audio [`e2693d08ffb3a0cc42f6221be80b6508a83dc5a1`][audio-arm64] | Additive ARM64 work. The present solution and CI still declare ARM and ARM64; this report did not rebuild either. |
| Microsoft BSP custodianship transferred | [Microsoft's final BSP README][ms-bsp-readme] | Directs development to `raspberrypi/windows-drivers`; a repository migration rather than an announcement that Pi2/ARM32 no longer worked. |
| WoR source pointer updated, 2021-04-07 | [`26e53926fe2e894f1de4fb9128bede526b2050ae`][driver-source-move] | Changes the source link from `driver1998/bsp` to `raspberrypi/windows-drivers`. |
| Last newly published ARM32 driver package | [`v0.9`][drivers-v09], 2021-04-09 | Asset named `RPi3_Windows_ARM32_Drivers_v0.9.zip`. A Pi3-branded archive is not automatically a validated Pi2 v1.1 package. |
| Subsequent releases omit ARM32 | [`v0.10`][drivers-v010], 2021-07-03, through [`v0.17`][drivers-v017], 2022-05-07 | **No ARM32 asset in the current release** is different from **no historical ARM32 downloads remain**. |
| AUX SPI hardware ID removed, 2021-06-03 | [`6469702898789e555c6947e50216a3f79e0ddeb9`][spi-hid-removal] | Removes unused `BCM2840`; does not establish that BCM2836 or Pi2 support was removed. |
| ARM32 status table removed, 2022-04-29 | [`b19eb98bd8a354349d47b0801360f72d0a9ac04a`][arm32-status-removed] | Deletes the README's Pi3 ARM32 table, including USB, Ethernet, Wi-Fi and Bluetooth status. This is a documentation/support-surface removal, not proof of deletion of every archived binary. |

Historical ARM32 status reported working USB/Ethernet and Pi3 onboard
Wi-Fi/Bluetooth; the ARM64 table differs. Do not transpose the current
ARM64 "no Wi-Fi driver" statement onto every historical IoT/ARM32 image, or
use the ARM32 Pi3 Wi-Fi result to claim onboard wireless hardware exists on Pi2.

Release-count nuance: `raspberrypi/windows-drivers` has no GitHub releases;
`ms-iot/rpi-iotcore` has the
[`1703_15063` release][ms-bsp-tag] with **no attached binary assets**.
GitHub's generated source archives are not compiled drivers. The actual
archived binaries above come from the separate addonkit/WoR assets.

### The ARM64 platform is not a hidden ARM32 recovery route

- [`worproject/edk2-platforms` initially added its RPi3 platform][platform-added]
  on 2019-02-15 with `SUPPORTED_ARCHITECTURES = AARCH64`. The inspected
  [current DSC][platform-dsc] still declares AARCH64.
- The [platform README][pi3-targets] explicitly names Pi 2B v1.2 and says older
  versions are incompatible. Its last change is
  [`38c387d2dac61d64bcf6dd2b9eb416fffa329de4`][platform-readme-change]
  (2020-11-04). Shared `Bcm2836` register headers in this tree do **not** override
  that architecture constraint.
- [`e168252531c7ea73e4114358e022aa6953b6dc5a`][platform-acpi-move]
  (2020-03-02) factorizes Pi ACPI tables. Deleted board-local table paths were
  consolidated; this is not evidence that a working Pi 2 v1.1 desktop target
  was removed.
- [`04d3ad571a5593f73d7953479a0902e396e97827`][platform-console-remove]
  (2020-04-14) removes the Pi-specific `GraphicsConsoleDxe` files. It does not
  remove an ARM32 Windows display driver.
- The [RPi3 TF-A makefile][tfa-rpi3] uses AArch64 assembly and Cortex-A53 code.
  Its `RPI3_BL33_IN_AARCH32` payload setting is **not** an ARMv7 build of TF-A
  for BCM2836. The original port commit is
  [`d83c1db15cc55fccd051ba023ee2cd9ab35dddd5`][tfa-added]
  (2017-12-01), explicitly an AArch64 Pi3 port.
- The [WoR organization currently directs Windows development toward other
  hardware][wor-faq]. [Rockchip's driver tree][rockchip] targets RK35xx and
  outputs ARM64 drivers; [Pi5 firmware][rpi5] targets BCM2712. Neither supplies
  a BCM2836 desktop image.

## H. Probability assessment and minimum missing component

These are **engineering confidence bands**, not measured success rates.
Assumptions: a genuine working rev 1.1 board, a good power supply/microSD card,
lawfully obtained unmodified matching images, competent engineering effort,
and a goal of local boot rather than production deployment. No hardware trials
were performed here.

| Question | Assessment | Confidence / estimated feasibility |
| --- | --- | --- |
| Can Windows 10 IoT Core run on Pi 2 v1.1? | **Yes, historically supported.** Microsoft's explicit Cortex-A7 and BCM2836 evidence settles the capability question. Reproducing it today still requires choosing and checking the exact image. [Evidence][iot15063] [Evidence][iot17763] | **Very high (>95%)** confidence in capability; not a >95% claim about an arbitrary downloaded image or SD card. |
| Can ARM32 Windows boot on it? | **Yes if that includes IoT Core.** For RT/15035/full desktop, the CPU ISA is not the fundamental impossibility, but a compatible complete boot stack has not been demonstrated in this review. [IoT evidence][iot17763] [Non-Pi desktop evidence][surface-readme] | **Very high** for the existing IoT family; **unquantified/low confidence** for a new desktop port. A numerical desktop success percentage would have no empirical basis. |
| Can current WoR be adapted? | **Probably for legacy IoT FFU deployment, with a separate validated backend.** **No** for executing its present ARM64 stack on Cortex-A7. [WoR requirements][wor-downloads] [Installer analysis](#i-current-installer-and-an-implementable-selection) | **High (roughly 80-95%) engineering confidence** that an IoT importer/writer can be built and validated; **0% native compatibility** for unchanged ARM64 binaries. This estimates feasibility, not schedule or production readiness. |
| What minimum piece is missing? | For **IoT in WoR-Flasher:** a safe, tested Mobile/OneCore FFU import/application/verification path. For **desktop:** an identified, available ARM32 desktop image and validated BCM2836 boot-critical integration for that exact build; no single missing driver has been proven to be the only blocker. [FFU evidence][iot-dism] [Desktop limits][iot-apps] | A demonstrable integration gap, not a probability. |

**Recommendation:** implement and validate an IoT-only laboratory path before
considering desktop experiments. Do not claim desktop support or downgrade
architecture checks to make a new picker option appear to work.

## I. Current installer and an implementable selection

### What was inspected locally

The user referred to "wor-install"; the tagged repository is
`blackoutsecure/wor-flasher`. The inspected checkout's HEAD was
`898bd7dfd806183bb9ba99110e3dc4167128d8c6`, with pre-existing uncommitted work,
including a separate Pi 2 v1.2 board picker and its tests. The findings below
refer to that working tree, not a claim that every inspected line is committed
at that SHA. Existing edits were left alone.

| Surface | Existing behavior | Consequence for an IoT profile |
| --- | --- | --- |
| [Shared GUI definitions](../../src/lib/gui.sh), `wor_rpi_board_options` | Pi 5, Pi 4/400, Pi 3 and Pi 2 v1.2; shared by macOS and Linux | v1.1 would require its own explicitly restricted profile, not a display alias for Pi3. |
| [Engine](../../install-wor.sh), `select_rpi_board` | v1.2 maps to `RPI_MODEL=3` with `WOR_TARGET_BOARD=pi2-v1.2`; v1.1 is rejected | Preserve the existing v1.2 compatibility route. A new board identity must not silently resolve ARM64 packages. |
| [Board tests](../../tests/target-board.test.mjs) | Verify picker parity, v1.2 package routing, summary/export, Back navigation and rejection of v1.1 | These are useful regression tests, not evidence of a working rev 1.1 install. All 14 passed during this investigation. |
| [Engine](../../install-wor.sh), ESD selection | Queries `arch=ARM64&edition=Professional`; build-number filtering is for the current desktop pipeline | IoT needs a separate image source/version policy. An ARM32 catalogue cannot be assumed by changing the query string. |
| [Engine](../../install-wor.sh), `validate_iso_file` | Accepts ISO extension and requires at least 3 GiB | The reachable official IoT ISO is 787,347,456 bytes, so this preflight rejects it before content inspection. Reproduced with an equally sized sparse fixture. |
| [Engine](../../install-wor.sh), ISO/ESD extraction | Expects desktop `boot`/`efi`, `sources/boot.wim`, `sources/install.wim` or `install.esd` | A board FFU or ISO/MSI delivery wrapper is not that layout. Do not loosen the size check and then continue down the same extractor. |
| [Engine](../../install-wor.sh), `release_package_source` | Pi3/Pi4 driver assets named `Windows_ARM64_Drivers`; Pi3 UEFI comes from `pftf/RPi3` | These assets cannot support the original Pi2 CPU. |
| [Canonical metadata](../../src/config/metadata.json) | Pins WoR-PE 1.1.0, ARM64 Pi UEFI/driver releases | No ARM32 PE or IoT image manifest is provided. |
| WoR-PE 1.1.0 release ZIP | `winpe/2/setup.exe` is **PE32+ AArch64**, verified from the downloaded pinned package | It cannot execute on a Cortex-A7. Merely replacing the final OS WIM is insufficient. |
| [Engine](../../install-wor.sh), partition/write functions | Creates WoR recovery/setup media with `WOR_BOOT` FAT and `WOR_INSTALL` exFAT, leaving space for installation | An FFU carries its own partition/block layout. Preserve it; do not prepartition as desktop WoR media. |
| [Engine](../../install-wor.sh), `verify_written_image` | Requires two partitions, `RPI_EFI.fd`, `BOOTAA64.EFI`, BCD, boot/install WIMs and WoR-specific free space | This verifier must remain intact for ARM64. IoT needs a different layout- and image-aware verification path. |
| [Engine](../../install-wor.sh), answer files; [prefinalization](../../config-templates/prefinalize.cmd) | `processorArchitecture="arm64"`, desktop OOBE/account configuration and `BOOTAA64.EFI` shell handoff | Not applicable to stock IoT FFU provisioning. Do not apply Pi4 RAM unlocks or desktop answer files to IoT. |
| [Configuration schema](../../config-templates/config.schema.json) | `rpiModel` enum `[3,4,5]`; source media documented as ARM64 ISO | Requires explicit schema/engine validation, not only GUI text. |
| [Hook](../../install-wor-hook.sh) | Delegates `run` and summary/device operations to the shared engine | Keep one deployment implementation. Do not introduce an unvalidated alternate writer through the hook. |

WoR's Windows-only Imager advertises WIM/ESD/ISO/**FFU**, whereas its
[PE-based installer requirements][wor-downloads] are ARM64 and ISO-based.
These are different tools. FFU support in the Windows Imager is not evidence
that this Linux/macOS shell engine already handles an ARM32 IoT FFU.

### Evidence that image availability is not entirely lost

HTTP HEAD checks on the investigation date, **not full OS-image validation**:

| Official source | Observed result |
| --- | --- |
| [Microsoft's October 2018 Raspberry Pi link][iot-download] | Redirected to `17763.107.181026-1406.rs5_release_svc_prod2_amd64fre_IOTCORE_RPi.iso`; HTTP 200; 787,347,456 bytes |
| [Microsoft's 16299 Raspberry Pi ISO][iot-rs3-download] | HTTP 200; 792,426,496 bytes |
| `https://www.microsoft.com/en-us/software-download/windows10iotcore` | HTTP 404 in this environment |

An old landing page failing does not prove its release packages disappeared.
Conversely, an HTTP 200 does not authenticate the payload or prove it still
boots. The `amd64fre` text in the **delivery ISO filename** must not be used as
the architecture of the contained Pi OS. Inspect the FFU and its PE binaries.

The [17763.253 release notes][iot17763] recommend that servicing release over
the October image because of known issues. The reachable redirect still names
17763.107. **Do not auto-label that URL "latest", "fully patched" or "validated".**
A maintained manifest needs build numbers, source provenance, verified hashes
and known limitations, rather than a moving forward link alone.

### Suggested user-facing selection

This is proposed UI, **not an implemented option**:

```text
Image family: Windows 10 IoT Core (ARM32, legacy)
Board:        Raspberry Pi 2 Model B v1.1 (BCM2836 / Cortex-A7)
Image:        Select a locally supplied Raspberry Pi IoT Core FFU
Deployment:   Apply the complete image to a microSD card
Status:       Experimental; hardware validation required
```

The accompanying explanation should say:

- IoT apps, console utilities and services; **not the Windows desktop**.
- No native ARM64 applications and no built-in desktop x86 emulation.
- No support implied for arbitrary ARM32/phone/Surface images.
- Board-specific limitations; archived maker images should be treated as
  isolated development systems.
- The image determines the disk layout. Flashing erases the selected disk.

Keep the existing `Windows 10/11 (ARM64)` selection unchanged. Do not call IoT
"Windows 10 Lite", an alternative Windows edition inside the current ARM64
catalogue, or a fully supported desktop for Pi2.

### Smallest responsible implementation

1. **Introduce an image-family distinction in the shared engine.** Separate
   physical board identity, target architecture and deployment format. Preserve
   the existing Pi3 package route for v1.2. One capability definition must feed
   CLI, both GUIs, configuration validation, summary and hook execution.
2. **Start with local FFU import and read-only inspection.** Identify the
   Mobile/OneCore FFU variant, target platform, byte/block geometry and complete
   image identity before selecting a destructive operation. Reject unsupported,
   truncated, delta, malformed or architecture-mismatched input explicitly.
   A `.ffu` extension is not adequate validation.
3. **Choose and validate a real FFU backend.** Microsoft documents Windows
   DISM application, but the current application's supported hosts are Linux
   and macOS. A Windows VM/manual external workflow can establish a reference
   image; it is not native support in this application. A portable decoder or
   converter needs independent correctness and integrity validation.
4. **Apply the complete image with the existing device-safety boundary.** Keep
   whole-disk selection, boot/system-disk exclusions, final revalidation,
   capacity checks, explicit erase confirmation, disk claiming/unmounting,
   progress, cancellation and cleanup. Refuse ambiguity. No fallback to the
   desktop writer and no `dd` of the FFU container itself.
5. **Verify the result against the image's own layout and hashes.** Account for
   start/end-relative block destinations, partition metadata and target size.
   Check the correct ARM32 firmware/Windows boot path and required artifacts.
   Do not disable the current ARM64 verifier or declare an IoT skip a pass.
6. **Disable inapplicable desktop options.** WoR-PE mode, desktop account/OOBE
   answer files, ARM64 driver/UEFI release selection, Pi4 memory adjustments and
   the desktop build ceiling do not carry over. IoT provisioning needs its own
   documented mechanism.
7. **Only then consider official ISO/MSI extraction or guided acquisition.**
   Importing the complete official image is safer than downloading individual
   legacy drivers from assorted archives. Do not distribute a home-made Windows
   image or silently source a leaked desktop preview.

There is relevant prior art: [the Python `ffu2img.py3` source][ffu-converter]
explicitly says it was tested with the **2015-05-12 Pi2 IoT Insider image**.
It parses `SignedImage`/`ImageFlash` headers and writes mapped blocks to a raw
image. It also skips over the catalog/hash table rather than verifying their
contents. Its narrow stated compatibility and lack of modern validation make
it **evidence that conversion is possible, not a ready-to-ship trusted backend**.
No converter was installed into or executed by WoR-Flasher for this report.

### Acceptance tests before enabling the option

- Negative selection matrix: v1.1 plus ARM64 desktop, Pi3 ARM64 UEFI, ARM64
  drivers, RT/phone FFU, or unknown board IDs must fail **before any write**.
- Positive routing: the IoT profile must never contact the ARM64 ESD catalogue,
  download WoR-PE, format a WoR two-partition layout, or generate ARM64 answer
  files. Existing v1.2 tests must remain unchanged and passing.
- Both GUI toolkits: label, back-navigation state, file picker, unavailable
  controls, summary and exported configuration must agree with the CLI/hook.
- Parser fixtures: wrong magic/version/platform, missing or corrupted blocks,
  hash/catalog failures, excessive counts, integer/bounds errors, truncated
  descriptors, multiple stores and unsupported compression must fail closed.
- Geometry tests: exact minimum size, smaller target, larger target,
  start/end-relative extents, sparse regions, sector-size assumptions and
  partition metadata; compare decoded bytes with an independently generated
  Microsoft-applied reference image.
- Device-safety tests: selected-device replacement, host disk exclusion,
  read-only media, disconnect, mount failure, privilege failure and
  interruption must not trigger a second, guessed target write.
- Linux loop-device and macOS mocked-device coverage; no physical disk in CI.
- Hardware gate: photographed board/revision/SoC, recorded image/firmware/driver
  hashes, cold and warm boot, serial logs, stable storage, HDMI/UWP screen,
  USB keyboard/mouse, Ethernet, shutdown/reboot. Qualify optional GPIO/I2C/SPI,
  touch/audio/dongles independently.

### Lifecycle and distribution

[Windows 10 IoT Core's maker product retired on 2020-11-10][iot-lifecycle].
[IoT Core LTSC][iot-ltsc] is a separate servicing arrangement requiring IoT Core
Services, with an extended-support window into January 2029. Do not assume
that a public 2018 maker image has that subscription, current servicing or a
commercial deployment entitlement.

Public source availability, a reachable binary, redistribution rights, a
valid signature and actual device compatibility are five separate questions.
The proposed feature should accept appropriately licensed user media and
record provenance; it must not bundle proprietary Windows payloads or promise
that publicly visible community desktop scripts have a safe/legal acquisition
path. The Surface script was inspected as historical evidence only, not
recommended or executed.

## Validation performed

- Direct source/INF/build/package/history inspection and pinned citations.
- Public official image endpoints checked using HEAD; **no full Windows OS
  image downloaded or booted**.
- WoR-PE 1.1.0 downloaded without execution. Its ZIP SHA-256 matched
  `a039e28fe7e39147899b0634c15e336c3b26a6f76201092ebb9732474cd43d0a`
  from [the existing metadata](../../src/config/metadata.json);
  `winpe/2/setup.exe` was identified as PE32+ AArch64.
- ARM32 firmware v1.0, the Microsoft-linked BSP ZIP, ARM32 drivers v0.9 and
  ARM64 drivers v0.17 were inspected without execution. In particular, the
  v0.9 archive's **19 SYS/DLL members** are ARMNT and its download is still
  available; an earlier "latest-only" reading would have missed this.
- Report validation: all requested sections A-H plus the installer analysis
  are present; relative links and reference labels resolve; **77 pinned
  source-file URLs** returned successful HEAD responses. The inventories
  validate as **1,119 reference paths, 40 firmware records and 29 driver records**,
  with consistent tabular schemas.
- `node --test tests/target-board.test.mjs`: **14 passed, 0 failed**.
  The editor test adapter did not discover these Node tests, so the repository's
  native Node runner was used.
- Current ISO preflight reproduced with a sparse 787,347,456-byte fixture:
  rejected as smaller than 3 GiB. This tested the existing preflight, not the
  validity of a fabricated OS image; the fixture was removed.
- The surrounding hub workspace's four required offline contract scripts
  passed. Its unchanged workflow tree is **not actionlint-clean**: a missing
  `security-app` step reference, an invalid `summary_context` action-input key,
  and SC2016 notices were reported. Those unrelated baseline findings were not
  changed as part of this investigation.

**Not validated:** firmware rebuilds, Microsoft driver signing, a full FFU's
internal contents, a new FFU writer, Pi2 hardware boot, ARM32 desktop kernel
compatibility, or Explorer/x86 application operation on BCM2836. None is claimed.

## Primary source links

[soc2836]: https://github.com/raspberrypi/documentation/blob/287523e617dd92cbe006057d7c0514667d790fb2/documentation/asciidoc/computers/processors/bcm2836.adoc
[soc2837]: https://github.com/raspberrypi/documentation/blob/287523e617dd92cbe006057d7c0514667d790fb2/documentation/asciidoc/computers/processors/bcm2837.adoc
[iot15063]: https://github.com/MicrosoftDocs/windows-iotcore-docs/blob/5ce0ec708c82a33d6ba0900d9d6ba4a6d2ce5877/windows-iotcore/release-notes/commercial/CreatorsUpdate.md#L19
[iot17763]: https://github.com/MicrosoftDocs/windows-iotcore-docs/blob/5ce0ec708c82a33d6ba0900d9d6ba4a6d2ce5877/windows-iotcore/release-notes/commercial/17763.md#L22
[iot-apps]: https://github.com/MicrosoftDocs/windows-iotcore-docs/blob/5ce0ec708c82a33d6ba0900d9d6ba4a6d2ce5877/windows-iotcore/develop-your-app/BuildingAppsForIoTCore.md#L92
[iot-shell]: https://github.com/MicrosoftDocs/windows-iotcore-docs/blob/5ce0ec708c82a33d6ba0900d9d6ba4a6d2ce5877/windows-iotcore/develop-your-app/IoTCoreShell.md#L15
[iot3bplus]: https://github.com/MicrosoftDocs/windows-iotcore-docs/blob/5ce0ec708c82a33d6ba0900d9d6ba4a6d2ce5877/windows-iotcore/Troubleshooting.md#L136
[iot-socs]: https://github.com/MicrosoftDocs/windows-iotcore-docs/blob/5ce0ec708c82a33d6ba0900d9d6ba4a6d2ce5877/windows-iotcore/learn-about-hardware/SoCsAndCustomBoards.md#L25
[iot-peripherals]: https://github.com/MicrosoftDocs/windows-iotcore-docs/blob/5ce0ec708c82a33d6ba0900d9d6ba4a6d2ce5877/windows-iotcore/learn-about-hardware/HardwareCompatList.md#L20
[iot-bsp]: https://github.com/MicrosoftDocs/windows-iotcore-docs/blob/5ce0ec708c82a33d6ba0900d9d6ba4a6d2ce5877/windows-iotcore/manufacture/BSPHardware.md#L17
[iot-image-build]: https://github.com/MicrosoftDocs/windows-iotcore-docs/blob/5ce0ec708c82a33d6ba0900d9d6ba4a6d2ce5877/windows-iotcore/manufacture/create-a-basic-image.md#L184
[iot-dism]: https://github.com/MicrosoftDocs/windows-iotcore-docs/blob/5ce0ec708c82a33d6ba0900d9d6ba4a6d2ce5877/windows-iotcore/connect-your-device/DISM.md#L15
[iot-download]: https://go.microsoft.com/fwlink/?LinkId=846058
[iot-rs3-download]: https://download.microsoft.com/download/9/6/2/9629C69B-02B8-4A82-A4C8-860D6E880C66/16299.15.170928-1534.rs3_release_amd64fre_IOTCORE_RPi.iso
[iot-lifecycle]: https://learn.microsoft.com/en-us/lifecycle/products/windows-10-iot-core
[iot-ltsc]: https://learn.microsoft.com/en-us/lifecycle/products/windows-10-iot-core-ltsc
[kit-root]: https://github.com/ms-iot/iot-adk-addonkit
[kit-readme]: https://github.com/ms-iot/iot-adk-addonkit/blob/3a9cf8d2ab3506f3dcd51940c48db7e5096575e0/README.md
[kit-ffu]: https://github.com/ms-iot/iot-adk-addonkit/blob/3a9cf8d2ab3506f3dcd51940c48db7e5096575e0/Tools/IoTCoreImaging/Classes/IoTFFU.ps1#L37
[kit-export]: https://github.com/ms-iot/iot-adk-addonkit/blob/3a9cf8d2ab3506f3dcd51940c48db7e5096575e0/Tools/IoTCoreImaging/Docs/Export-IoTFFUAsWims.md
[kit-import]: https://github.com/ms-iot/iot-adk-addonkit/blob/3a9cf8d2ab3506f3dcd51940c48db7e5096575e0/Tools/IoTCoreImaging/Docs/Import-IoTBSP.md#L57
[kit-custom]: https://github.com/ms-iot/iot-adk-addonkit/tree/3a9cf8d2ab3506f3dcd51940c48db7e5096575e0/Workspace/Source-arm/BSP/CustomRpi2
[kit-recovery]: https://github.com/ms-iot/iot-adk-addonkit/tree/3a9cf8d2ab3506f3dcd51940c48db7e5096575e0/Workspace/Source-arm/Products/RPiRecovery
[kit-pi3-change]: https://github.com/ms-iot/iot-adk-addonkit/commit/7223e44eda561b23425de99e6f441e0a25aad134
[kit-bsp-external]: https://github.com/ms-iot/iot-adk-addonkit/commit/f6c2018ec11581813ae74eb48219763a681fb0dd
[kit-wm-change]: https://github.com/ms-iot/iot-adk-addonkit/commit/7524312d0a89f56c0755cc5091ab2834e9d5dba8
[kit-remove-bsp]: https://github.com/ms-iot/iot-adk-addonkit/commit/50c31769c7cd5bb6e34731d6d1f7984fb0400da8
[kit-rpibsp-tag]: https://github.com/ms-iot/iot-adk-addonkit/releases/tag/RPiBSP
[kit-v44]: https://github.com/ms-iot/iot-adk-addonkit/releases/tag/v4.4
[kit-v53]: https://github.com/ms-iot/iot-adk-addonkit/releases/tag/17134_v5.3
[kit-v60]: https://github.com/ms-iot/iot-adk-addonkit/releases/tag/v6.0
[pi3-targets]: https://github.com/worproject/edk2-platforms/blob/8e1779b538bcc1e6dc68d7df625394f933651d7a/Platform/RaspberryPi/RPi3/Readme.md#L17
[platform-dsc]: https://github.com/worproject/edk2-platforms/blob/8e1779b538bcc1e6dc68d7df625394f933651d7a/Platform/RaspberryPi/RPi3/RPi3.dsc#L17
[platform-added]: https://github.com/worproject/edk2-platforms/commit/54590f3a56e9fe9e55a315eddbac10f789ce4695
[platform-readme-change]: https://github.com/worproject/edk2-platforms/commit/38c387d2dac61d64bcf6dd2b9eb416fffa329de4
[platform-acpi-move]: https://github.com/worproject/edk2-platforms/commit/e168252531c7ea73e4114358e022aa6953b6dc5a
[platform-console-remove]: https://github.com/worproject/edk2-platforms/commit/04d3ad571a5593f73d7953479a0902e396e97827
[tfa-rpi3]: https://github.com/worproject/arm-trusted-firmware/blob/01610b0d32d746104dd4dacf179c65d69adaaf0a/plat/rpi/rpi3/platform.mk#L13
[tfa-added]: https://github.com/worproject/arm-trusted-firmware/commit/d83c1db15cc55fccd051ba023ee2cd9ab35dddd5
[rockchip]: https://github.com/worproject/Rockchip-Windows-Drivers/blob/e00e70dabe17c3e150ea2b06b5a2b640bf0552b6/README.md
[rpi5]: https://github.com/worproject/rpi5-uefi/blob/a6135b06d661b5f11bbf4bd742b42a1919b264dc/README.md
[surface-readme]: https://github.com/WindowsOnARM32/Surface2Setup/blob/c6185162dd255c56318855f481682862a7fe0c2a/README.md
[surface-script]: https://github.com/WindowsOnARM32/Surface2Setup/blob/c6185162dd255c56318855f481682862a7fe0c2a/setup.cmd
[win86emu]: https://github.com/MakiseKurisu/Win86emu/blob/71c723fcab44d5aae3b6b3c86c87154ad3c4abb5/README.md
[ms-emulation]: https://learn.microsoft.com/en-us/windows/arm/apps-on-arm-x86-emulation
[ffu-converter]: https://github.com/ovidiuchis/Python-FFU-converter/blob/5ac66c34d93936beb826ef7dc9ff1a5ae813a47e/ffu2img.py3
[wor-downloads]: https://worproject.com/downloads
[wor-faq]: https://worproject.com/faq
[arm32-uefi]: https://github.com/worproject/RPi-UEFI-Arm32
[ms-uefi]: https://github.com/ms-iot/RPi-UEFI
[arm32-uefi-release]: https://github.com/worproject/RPi-UEFI-Arm32/releases/tag/v1.0
[arm32-pi2-dsc]: https://github.com/worproject/RPi-UEFI-Arm32/blob/5bfd48d674e6c7efea6e31f9eb97b9da90c20263/Pi2BoardPkg/Pi2BoardPkg.dsc#L495
[arm32-startup]: https://github.com/worproject/RPi-UEFI-Arm32/blob/5bfd48d674e6c7efea6e31f9eb97b9da90c20263/Pi3BoardPkg/Scripts/startup.S#L12
[arm32-dsdt]: https://github.com/worproject/RPi-UEFI-Arm32/blob/5bfd48d674e6c7efea6e31f9eb97b9da90c20263/Pi2BoardPkg/AcpiTables/Common/DSDT.asl
[arm32-fadt]: https://github.com/worproject/RPi-UEFI-Arm32/blob/5bfd48d674e6c7efea6e31f9eb97b9da90c20263/Pi2BoardPkg/AcpiTables/Common/Fadt.aslc
[arm32-madt]: https://github.com/worproject/RPi-UEFI-Arm32/blob/5bfd48d674e6c7efea6e31f9eb97b9da90c20263/Pi2BoardPkg/AcpiTables/Common/Madt.aslc#L13
[arm32-gtdt]: https://github.com/worproject/RPi-UEFI-Arm32/blob/5bfd48d674e6c7efea6e31f9eb97b9da90c20263/Pi2BoardPkg/AcpiTables/Common/Gtdt.aslc
[arm32-csrt]: https://github.com/worproject/RPi-UEFI-Arm32/blob/5bfd48d674e6c7efea6e31f9eb97b9da90c20263/Pi2BoardPkg/AcpiTables/Common/CSRT.aslc
[arm32-acpi-platform]: https://github.com/worproject/RPi-UEFI-Arm32/blob/5bfd48d674e6c7efea6e31f9eb97b9da90c20263/Pi2BoardPkg/AcpiTables/Common/Platform.h
[arm32-uart]: https://github.com/worproject/RPi-UEFI-Arm32/blob/5bfd48d674e6c7efea6e31f9eb97b9da90c20263/Pi2BoardPkg/AcpiTables/Uart.asl
[arm32-pep]: https://github.com/worproject/RPi-UEFI-Arm32/blob/5bfd48d674e6c7efea6e31f9eb97b9da90c20263/Pi2BoardPkg/AcpiTables/Common/PEP.asl
[arm32-pep-h]: https://github.com/worproject/RPi-UEFI-Arm32/blob/5bfd48d674e6c7efea6e31f9eb97b9da90c20263/Pi2BoardPkg/AcpiTables/Common/pep.h
[arm32-pi3-acpi]: https://github.com/worproject/RPi-UEFI-Arm32/blob/5bfd48d674e6c7efea6e31f9eb97b9da90c20263/Pi3BoardPkg/AcpiTables/AcpiTables.inf
[ms-csrt]: https://learn.microsoft.com/en-us/windows-hardware/drivers/bringup/acpi-system-description-tables#core-system-resources-table-csrt
[legacy-pkg]: https://github.com/worproject/RaspberryPiPkg
[legacy-pkg-readme]: https://github.com/worproject/RaspberryPiPkg/blob/bcd8eaed2a2f6869a3ac848f91f98b901326e742/readme.md
[legacy-hyp]: https://github.com/worproject/RaspberryPiPkg/blob/bcd8eaed2a2f6869a3ac848f91f98b901326e742/Drivers/HypDxe/HypWS.c
[arm32-import]: https://github.com/worproject/RPi-UEFI-Arm32/commit/42edc35271384c6d62233419231c918711ada2d3
[arm32-dsdt-fix]: https://github.com/worproject/RPi-UEFI-Arm32/commit/2e4b7dd1fd02b0d2fd7712d37aa31de07e98eba6
[arm32-3bp-change]: https://github.com/worproject/RPi-UEFI-Arm32/commit/19acdb79a2676d0fa09d15c60f3b5f69bc725656
[arm32-tpm-change]: https://github.com/worproject/RPi-UEFI-Arm32/commit/0e2bf04ff3a48e7bf25092f1e1fd7dca16c6f109
[arm32-display-fix]: https://github.com/worproject/RPi-UEFI-Arm32/commit/5bfd48d674e6c7efea6e31f9eb97b9da90c20263
[arm32-dirty-base]: https://github.com/worproject/RPi-UEFI-Arm32/commit/359e2baf9985187fb00cc5a84b84035f5225cb58
[arm32-dirty-cpu]: https://github.com/worproject/RPi-UEFI-Arm32/commit/02e5f637ab18c0f239aabe8cef350d292d1c830d
[legacy-final]: https://github.com/worproject/RaspberryPiPkg/commit/bcd8eaed2a2f6869a3ac848f91f98b901326e742
[arm32-rs2]: https://github.com/worproject/RPi-UEFI-Arm32/commit/4726af1381dd3ebac326b73c802d60fef1531179
[arm32-pi3-dsc]: https://github.com/worproject/RPi-UEFI-Arm32/blob/5bfd48d674e6c7efea6e31f9eb97b9da90c20263/Pi3BoardPkg/Pi3BoardPkg.dsc
[arm32-build]: https://github.com/worproject/RPi-UEFI-Arm32/blob/5bfd48d674e6c7efea6e31f9eb97b9da90c20263/BuildPi2Pi3Board.bat
[arm32-sec]: https://github.com/worproject/RPi-UEFI-Arm32/tree/5bfd48d674e6c7efea6e31f9eb97b9da90c20263/Pi2BoardPkg/Library/SecLib
[arm32-boardlib]: https://github.com/worproject/RPi-UEFI-Arm32/tree/5bfd48d674e6c7efea6e31f9eb97b9da90c20263/Pi2BoardPkg/Library/Pi2BoardLib
[arm32-soc-header]: https://github.com/worproject/RPi-UEFI-Arm32/blob/5bfd48d674e6c7efea6e31f9eb97b9da90c20263/Pi2BoardPkg/Include/Bcm2836.h
[arm32-interrupt]: https://github.com/worproject/RPi-UEFI-Arm32/tree/5bfd48d674e6c7efea6e31f9eb97b9da90c20263/Pi2BoardPkg/Drivers/InterruptDxe
[arm32-gic]: https://github.com/worproject/RPi-UEFI-Arm32/tree/5bfd48d674e6c7efea6e31f9eb97b9da90c20263/Pi2BoardPkg/Library/BcmGicLib
[arm32-sdhost]: https://github.com/worproject/RPi-UEFI-Arm32/tree/5bfd48d674e6c7efea6e31f9eb97b9da90c20263/Pi2BoardPkg/Drivers/SdHostDxe
[arm32-mmc]: https://github.com/worproject/RPi-UEFI-Arm32/tree/5bfd48d674e6c7efea6e31f9eb97b9da90c20263/Pi2BoardPkg/Drivers/MmcDxe
[arm32-arasan]: https://github.com/worproject/RPi-UEFI-Arm32/tree/5bfd48d674e6c7efea6e31f9eb97b9da90c20263/Pi2BoardPkg/Drivers/ArasanMmcHostDxe
[arm32-display]: https://github.com/worproject/RPi-UEFI-Arm32/blob/5bfd48d674e6c7efea6e31f9eb97b9da90c20263/Pi2BoardPkg/Drivers/DisplayDxe/DisplayDxe.c
[arm32-mailbox]: https://github.com/worproject/RPi-UEFI-Arm32/blob/5bfd48d674e6c7efea6e31f9eb97b9da90c20263/Pi2BoardPkg/Library/BcmMailboxLib/BcmMailbox.c
[arm32-reset]: https://github.com/worproject/RPi-UEFI-Arm32/blob/5bfd48d674e6c7efea6e31f9eb97b9da90c20263/Pi2BoardPkg/Library/ResetSystemLib/ResetSystemLib.c
[arm32-rtc]: https://github.com/worproject/RPi-UEFI-Arm32/blob/5bfd48d674e6c7efea6e31f9eb97b9da90c20263/Pi2BoardPkg/Library/VirtualRealTimeClockLib/VirtualRealTimeClockLib.c
[arm32-led]: https://github.com/worproject/RPi-UEFI-Arm32/blob/5bfd48d674e6c7efea6e31f9eb97b9da90c20263/Pi2BoardPkg/Library/BcmLedLib/BcmLedLib.c
[arm32-pcd]: https://github.com/worproject/RPi-UEFI-Arm32/blob/5bfd48d674e6c7efea6e31f9eb97b9da90c20263/Pi2BoardPkg/Library/Pi2PcdLib/Pi2PcdLib.c
[arm32-smbios]: https://github.com/worproject/RPi-UEFI-Arm32/blob/5bfd48d674e6c7efea6e31f9eb97b9da90c20263/Pi2BoardPkg/Drivers/PlatformSmbiosDxe/PlatformSmbiosDxe.c
[arm32-sdhc-asl]: https://github.com/worproject/RPi-UEFI-Arm32/blob/5bfd48d674e6c7efea6e31f9eb97b9da90c20263/Pi2BoardPkg/AcpiTables/Sdhc.asl
[arm32-rhpx]: https://github.com/worproject/RPi-UEFI-Arm32/blob/5bfd48d674e6c7efea6e31f9eb97b9da90c20263/Pi2BoardPkg/AcpiTables/Rhpx.asl
[arm32-tpm-tables]: https://github.com/worproject/RPi-UEFI-Arm32/tree/0e2bf04ff3a48e7bf25092f1e1fd7dca16c6f109/Pi2BoardPkg/AcpiTables/Common
[arm32-fdf]: https://github.com/worproject/RPi-UEFI-Arm32/blob/5bfd48d674e6c7efea6e31f9eb97b9da90c20263/Pi2BoardPkg/Pi2BoardPkg.fdf
[ms-sample-dsc]: https://github.com/microsoft/MS_UEFI/blob/ce5608b1f0d45bf2bab8b00b95982829abb60a36/MsIotSamples/MsIotSamples.dsc
[edk-arm-lib]: https://github.com/worproject/edk2/tree/d7d4f09ff815794761f84d06e307001afe6376c4/ArmPkg/Library/ArmLib/Arm
[drivers-readme]: https://github.com/raspberrypi/windows-drivers/blob/88ee238c9debecce810d208cac1e5f36add3d2a1/README.md
[driver-manifest]: https://github.com/raspberrypi/windows-drivers/blob/88ee238c9debecce810d208cac1e5f36add3d2a1/bspfiles/Packages/RPiFM.xml
[driver-inf-normalization]: https://github.com/raspberrypi/windows-drivers/commit/b6dcdc76e35af86542e73b85c48eefa971459c91
[driver-boot-sample]: https://github.com/raspberrypi/windows-drivers/blob/88ee238c9debecce810d208cac1e5f36add3d2a1/bspfiles/Packages/RPi.BootFirmware/kernel.img
[driver-gpio]: https://github.com/raspberrypi/windows-drivers/blob/88ee238c9debecce810d208cac1e5f36add3d2a1/drivers/gpio/bcm2836/bcmgpio.inf
[driver-i2c]: https://github.com/raspberrypi/windows-drivers/blob/88ee238c9debecce810d208cac1e5f36add3d2a1/drivers/i2c/bcm2836/bcmi2c.inf
[driver-spi]: https://github.com/raspberrypi/windows-drivers/blob/88ee238c9debecce810d208cac1e5f36add3d2a1/drivers/spi/bcm2836/bcmspi.inf
[driver-auxspi]: https://github.com/raspberrypi/windows-drivers/blob/88ee238c9debecce810d208cac1e5f36add3d2a1/drivers/spi/bcmauxspi/bcmauxspi.inf
[driver-pwm]: https://github.com/raspberrypi/windows-drivers/blob/88ee238c9debecce810d208cac1e5f36add3d2a1/drivers/pwm/bcm2836/bcm2836pwm.inf
[driver-dma]: https://github.com/raspberrypi/windows-drivers/blob/88ee238c9debecce810d208cac1e5f36add3d2a1/drivers/pwm/bcm2836/dma.cpp
[driver-arasan]: https://github.com/raspberrypi/windows-drivers/blob/88ee238c9debecce810d208cac1e5f36add3d2a1/drivers/sd/bcm2836/bcm2836sdhc/bcm2836sdhc.inf
[driver-sdhost]: https://github.com/raspberrypi/windows-drivers/blob/88ee238c9debecce810d208cac1e5f36add3d2a1/drivers/sd/bcm2836/rpisdhc/rpisdhc.inf
[driver-mailbox]: https://github.com/raspberrypi/windows-drivers/tree/88ee238c9debecce810d208cac1e5f36add3d2a1/drivers/mailbox/bcm2836
[driver-lan-helper]: https://github.com/raspberrypi/windows-drivers/blob/88ee238c9debecce810d208cac1e5f36add3d2a1/drivers/RpiLanPropertyChange/bcm2836/RpiLanPropertyChange.inf
[driver-vchiq]: https://github.com/raspberrypi/windows-drivers/blob/88ee238c9debecce810d208cac1e5f36add3d2a1/drivers/misc/vchiq/vchiq.inf
[driver-audio]: https://github.com/raspberrypi/windows-drivers/blob/88ee238c9debecce810d208cac1e5f36add3d2a1/drivers/audio/bcm2836/rpiwav.inf
[driver-pl011]: https://github.com/raspberrypi/windows-drivers/blob/88ee238c9debecce810d208cac1e5f36add3d2a1/drivers/uart/bcm2836/serPL011/SerPL011.inf
[driver-miniuart]: https://github.com/raspberrypi/windows-drivers/blob/88ee238c9debecce810d208cac1e5f36add3d2a1/drivers/uart/bcm2836/miniUart/pi_miniuart.inf
[cywbt-inf]: https://github.com/worproject/cywbtserialbus/blob/b9301327a566f0cc037258a326f5c8082890eca5/src/vendor/cywbtserialbus.inx
[cywbt-release]: https://github.com/worproject/cywbtserialbus/releases/tag/v1.0
[ros-header]: https://github.com/microsoft/graphics-driver-samples/blob/de4a2161991eda254013da6c18226f5ea06e4a9c/render-only-sample/roskmd/RosKmd.h
[ros-inf]: https://github.com/microsoft/graphics-driver-samples/blob/de4a2161991eda254013da6c18226f5ea06e4a9c/render-only-sample/rosdriver/Ros.inf
[ros-readme]: https://github.com/microsoft/graphics-driver-samples/blob/de4a2161991eda254013da6c18226f5ea06e4a9c/README.md
[driver-mailbox-inf]: https://github.com/raspberrypi/windows-drivers/blob/88ee238c9debecce810d208cac1e5f36add3d2a1/drivers/mailbox/bcm2836/RPIQ.inf
[arm32-emmc-change]: https://github.com/worproject/RPi-UEFI-Arm32/commit/48fd8bb20dd4d45a4cf0a8970a65837e45bbaa99
[arm32-emmc-fix]: https://github.com/worproject/RPi-UEFI-Arm32/commit/799727f31f2e7eee5f8fa55a538f9be6357cf31c
[ms-bsp-tag]: https://github.com/ms-iot/rpi-iotcore/releases/tag/1703_15063
[iot-launcher-sample]: https://github.com/microsoft/Windows-iotcore-samples/blob/a96f5f00fd7e3305cd19d590eced8f0ae7321f1a/Samples/IoTStartApp/README.md
[iot-default-sample]: https://github.com/microsoft/Windows-iotcore-samples/blob/a96f5f00fd7e3305cd19d590eced8f0ae7321f1a/Samples/IoTCoreDefaultApp/README.md
[drivers-source]: https://github.com/raspberrypi/windows-drivers
[drivers-release-repo]: https://github.com/worproject/RPi-Windows-Drivers
[ms-bsp]: https://github.com/ms-iot/rpi-iotcore
[ms-bsp-readme]: https://github.com/ms-iot/rpi-iotcore/blob/31e89330c37564d96e246a64210cfeaf8c45007c/README.md
[cywbt-repo]: https://github.com/worproject/cywbtserialbus
[org-edk2]: https://github.com/worproject/edk2
[org-platforms]: https://github.com/worproject/edk2-platforms
[org-tfa]: https://github.com/worproject/arm-trusted-firmware
[org-translations]: https://github.com/worproject/WoR-Imager-Translations
[org-mirror]: https://github.com/worproject/dldserv-mirror
[iot-docs-repo]: https://github.com/MicrosoftDocs/windows-iotcore-docs
[ms-uefi-samples]: https://github.com/microsoft/MS_UEFI/tree/ce5608b1f0d45bf2bab8b00b95982829abb60a36/MsIotSamples
[gpio-origin]: https://github.com/raspberrypi/windows-drivers/commit/c907d0dd69fe035fb23d28ccae736d2d4e3ee413
[gpio-arm64]: https://github.com/raspberrypi/windows-drivers/commit/318fba8eacb32ab0ff8205e9dfd475cbbefe8960
[audio-arm64]: https://github.com/raspberrypi/windows-drivers/commit/e2693d08ffb3a0cc42f6221be80b6508a83dc5a1
[driver-source-move]: https://github.com/worproject/RPi-Windows-Drivers/commit/26e53926fe2e894f1de4fb9128bede526b2050ae
[drivers-v09]: https://github.com/worproject/RPi-Windows-Drivers/releases/tag/v0.9
[drivers-v010]: https://github.com/worproject/RPi-Windows-Drivers/releases/tag/v0.10
[drivers-v017]: https://github.com/worproject/RPi-Windows-Drivers/releases/tag/v0.17
[spi-hid-removal]: https://github.com/raspberrypi/windows-drivers/commit/6469702898789e555c6947e50216a3f79e0ddeb9
[arm32-status-removed]: https://github.com/worproject/RPi-Windows-Drivers/commit/b19eb98bd8a354349d47b0801360f72d0a9ac04a
