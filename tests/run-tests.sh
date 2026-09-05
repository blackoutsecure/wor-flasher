#!/bin/bash

#Test harness for wor-flasher. Creates loopback devices to stand in for real drives,
#so nothing can be written to physical storage. Every run is a fresh download.
#
#Usage:
#  ./tests/run-tests.sh               run the automated suite; uses Docker for Linux integration on non-Linux hosts when available
#  ./tests/run-tests.sh --walkthrough create fake drives, then run the CLI interactively
#  ./tests/run-tests.sh --gui         launch the GUI in DRY_RUN mode (Linux creates fake drives; macOS needs a removable drive)
#  ./tests/run-tests-gui.sh           run the GUI walkthrough with host-specific preflight
#  ./tests/run-tests.sh --full        also download the real Windows image (several GB)
#  ./tests/run-tests.sh --keep        leave the fake drives and downloads in place afterwards
#  ./tests/run-tests.sh --clean       remove the test workspace and detach fake drives

######## Defaults. Every one can be overridden from the environment.

TEST_SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd -P)"
#shellcheck disable=SC1091
source "$TEST_SCRIPT_DIR/test-lib.sh"

REPO_DIR="$(script_dir "$TEST_SCRIPT_DIR/..")"

#Source-checkout updates are opt-in. Export this explicitly for every invocation below,
#including sourced library functions. The self-updater test further down deliberately opts in
#against a disposable clone, never against this working tree.
export NO_UPDATE=1

#Everything this script creates lives here. It is listed in .gitignore.
[ -z "$TEST_DIR" ] && TEST_DIR="$REPO_DIR/.test-workspace"
[ -z "$TEST_DL_DIR" ] && TEST_DL_DIR="$TEST_DIR/downloads"

[ -z "$TEST_WIN_LANG" ] && TEST_WIN_LANG='en-us'
[ -z "$TEST_COMMAND_TIMEOUT" ] && TEST_COMMAND_TIMEOUT=120

#Which Raspberry Pi models to exercise. Each one pulls a different UEFI package.
[ -z "$TEST_MODELS" ] && TEST_MODELS='3 4 5'

#Loopback images are sparse, so a larger size costs nothing on disk. These are the exact
#thresholds drive_capability() switches on, which makes them boundary tests rather than
#arbitrary sizes.
[ -z "$SIZE_TOO_SMALL" ] && SIZE_TOO_SMALL=1G #under the 8GB minimum, must be refused
[ -z "$SIZE_RECOVERY" ] && SIZE_RECOVERY=8G   #exactly the recovery threshold
[ -z "$SIZE_INSTALL" ] && SIZE_INSTALL=25G    #exactly the self-install threshold

######## End of defaults

LOOP_DEVICES=()
LAST_OUT=''
LAST_CODE=0
KEEP=0
MODE=suite
SKIP_ESD=1

detach_all() {
  command -v losetup >/dev/null || return 0
  local dev
  for dev in "${LOOP_DEVICES[@]}" ;do
    sudo losetup -d "$dev" 2>/dev/null
  done
  #also catch devices left behind by an interrupted run, including deleted backing files
  while read -r dev ;do
    [ ! -z "$dev" ] && sudo losetup -d "$dev" 2>/dev/null
  done < <(losetup -a 2>/dev/null | grep -F "$TEST_DIR" | cut -d: -f1)
}

cleanup() {
  detach_all
  if [ "$KEEP" == 1 ];then
    info "Left in place: $TEST_DIR"
    return
  fi
  rm -rf "$TEST_DIR"
  #install-wor.sh writes this beside itself whenever it is sourced or run
  rm -rf "$REPO_DIR/cache"
}

static_checks() {
  info "== Static checks =="
  #a git that cannot read the repo (e.g. an unusable checkout in a container) is not a whitespace error
  if ! command -v git >/dev/null || ! git -C "$REPO_DIR" rev-parse --git-dir >/dev/null 2>&1 ;then
    skip "git cannot read this checkout; cannot check for whitespace errors"
  else
    git -C "$REPO_DIR" diff --check >/dev/null 2>&1 \
      && pass "working tree has no whitespace errors" || fail "working tree has whitespace errors"
  fi
  for f in install-wor.sh install-wor-gui.sh terminal-run ;do
    bash -n "$REPO_DIR/$f" 2>/dev/null && pass "$f parses" || fail "$f has a syntax error"
  done

  if command -v shellcheck >/dev/null ;then
    shellcheck --severity=error "$REPO_DIR"/install-wor.sh "$REPO_DIR"/install-wor-gui.sh "$REPO_DIR"/terminal-run >/dev/null 2>&1 \
      && pass "shellcheck reports no errors" || fail "shellcheck reports errors"
  else
    skip "shellcheck is not installed"
  fi

  deprecated_update_file='no-''update'
  if grep -RIn --exclude-dir=.git --exclude-dir=.test-workspace --exclude='*.svg' "$deprecated_update_file" "$REPO_DIR" >/dev/null 2>&1 ;then
    fail "deprecated updater sentinel file hook is not referenced"
  else
    pass "deprecated updater sentinel file hook is not referenced"
  fi

  grep -qF 'register_device_cleanup "$ISO_DEVICE"' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'register_mount_cleanup "$isomount"' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'register_mount_cleanup "$mntpnt/bootpart"' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'register_mount_cleanup "$mntpnt/winpart"' "$REPO_DIR/install-wor.sh" \
    && pass "all temporary mounts use the shared cleanup handler" \
    || fail "a temporary mount bypasses the shared cleanup handler"

  grep -qF 'if ! command sudo -n -v >/dev/null 2>&1 && { [ "$RUN_MODE" == gui ] || ! sudo -v >/dev/null 2>&1; };then' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'Administrator authentication failed or was canceled.' "$REPO_DIR/install-wor.sh" \
    && pass "macOS checks administrator access before partitioning" \
    || fail "macOS does not check administrator access before partitioning"

  grep -qF 'require_free_space()' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'required_download_space' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'mkdir -p "$DL_DIR"' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'require_free_space "$required_download_space" "$DL_DIR"' "$REPO_DIR/install-wor.sh" \
    && pass "download directory free-space check runs before downloads" \
    || fail "download directory free-space check is missing"

  #prove the behaviour rather than the wording: winfiles_from_iso_* is also matched by winfiles_*
  cache_clear_dir="$(mktemp -d)"
  mkdir -p "$cache_clear_dir/dl/peinstaller" "$cache_clear_dir/dl/driverpackage" \
    "$cache_clear_dir/dl/pi4-uefipackage" "$cache_clear_dir/dl/winfiles_22631_en-us" \
    "$cache_clear_dir/dl/winfiles_from_iso_22631_en-us" "$cache_clear_dir/dl/keep-me" "$cache_clear_dir/repo/cache"
  (
    cd "$cache_clear_dir/dl" || exit 1
    #shellcheck disable=SC1090
    DIRECTORY="$cache_clear_dir/repo" source "$REPO_DIR/install-wor.sh" source >/dev/null 2>&1
    DIRECTORY="$cache_clear_dir/repo" clear_cached_components >/dev/null 2>&1
  )
  [ ! -e "$cache_clear_dir/dl/winfiles_22631_en-us" ] \
    && [ ! -e "$cache_clear_dir/dl/winfiles_from_iso_22631_en-us" ] \
    && [ ! -e "$cache_clear_dir/dl/peinstaller" ] \
    && [ ! -e "$cache_clear_dir/dl/driverpackage" ] \
    && [ ! -e "$cache_clear_dir/dl/pi4-uefipackage" ] \
    && [ -d "$cache_clear_dir/dl/keep-me" ] \
    && [ -d "$cache_clear_dir/repo/cache" ] \
    && pass "USE_CACHE=0 clears extracted Windows image caches" \
    || fail "USE_CACHE=0 leaves extracted Windows image caches in place"
  rm -rf "$cache_clear_dir"

  grep -qF '[ -z "$USE_CACHE" ] && USE_CACHE=1' "$REPO_DIR/install-wor.sh" \
    && pass "validated cache reuse is the default" \
    || fail "validated cache reuse is not the default"

  grep -qF "printf 'Downloaded files\\t%s\\n' \"\$(cache_mode_label \"\$USE_CACHE\")\"" "$REPO_DIR/install-wor.sh" \
    && pass "startup summary shows cache policy" \
    || fail "startup summary does not show cache policy"

  grep -qF 'sources/install.esd' "$REPO_DIR/install-wor.sh" \
    && pass "ISO import accepts install.esd media" \
    || fail "ISO import does not accept install.esd media"

  grep -qF 'mkfs.fat -F 32 -n WOR_BOOT' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'mkfs.exfat -n WOR_INSTALL' "$REPO_DIR/install-wor.sh" \
    && pass "Linux writes stable partition labels" \
    || fail "Linux does not write stable partition labels"

  grep -qF 'for formula in aria2 cabextract jq wget wimlib gptfdisk pv' "$REPO_DIR/install-wor.sh" \
    && grep -qF "git pv' || exit 1" "$REPO_DIR/install-wor.sh" \
    && pass "supported hosts install the progress utility" \
    || fail "a supported host does not install the progress utility"

  grep -qF 'copy_startup_environment_with_progress' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'copy_local_file_with_progress "$(basename "$install_image")"' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'sha1_file_with_progress downloaded-esd' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'sha256_file_with_progress downloaded-esd' "$REPO_DIR/install-wor.sh" \
    && ! grep -qF 'errors="$(wimextract' "$REPO_DIR/install-wor.sh" \
    && ! grep -qF 'errors="$(wimexport' "$REPO_DIR/install-wor.sh" \
    && ! grep -qF 'errors="$(wimdelete' "$REPO_DIR/install-wor.sh" \
    && pass "long image operations stream progress" \
    || fail "a long image operation still hides progress"

  awk '/#install dependencies before using them/{deps=NR} /#check for internet connection/{net=NR} END{exit !(deps && net && deps < net)}' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'Missing required dependency: wget.' "$REPO_DIR/install-wor.sh" \
    && ! grep -qF 'No internet connection!' "$REPO_DIR/install-wor.sh" \
    && pass "dependency checks run before internet check with specific wget error" \
    || fail "dependency checks do not run before internet check or wget error is misleading"

  grep -qF "'.DiskSize // .TotalSize // .Size'" "$REPO_DIR/install-wor.sh" \
    && pass "Darwin drive sizing supports current diskutil metadata" \
    || fail "Darwin drive sizing does not support current diskutil metadata"

  grep -qF 'darwin_apfs_volume_names()' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'diskutil apfs list -plist' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'Labels: %s' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'labels = labels ? labels ", " $0 : $0' "$REPO_DIR/install-wor.sh" \
    && pass "Darwin device choices include user-facing disk labels" \
    || fail "Darwin device choices do not include disk labels"

  grep -qF 'MACOS_ASKPASS="$(mktemp)"' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'register_file_cleanup "$MACOS_ASKPASS"' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'Formatting " & targetDevice & return & return & "There is no turning back now.' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'WOR_FLASH_TARGET="$DEVICE" SUDO_ASKPASS="$MACOS_ASKPASS" command sudo -A "$@"' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'SUDO_ASKPASS="$MACOS_ASKPASS" command sudo -A "$@"' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'export RUN_MODE=gui' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF "name: 'WorErrorController'" "$REPO_DIR/install-wor.sh" \
    && grep -qF 'app.requestUserAttention($.NSInformationalRequest)' "$REPO_DIR/install-wor.sh" \
    && ! grep -qF 'display alert' "$REPO_DIR/install-wor.sh" \
    && pass "macOS GUI shows a native password dialog instead of a terminal prompt" \
    || fail "macOS GUI does not use a native password dialog"

  grep -qF 'macos_start_cli()' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'macos_show_announcement()' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'partnership.png' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'Proceed with WoR-Flasher' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF "textView:clickedOnLink:atIndex:" "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'NSLinkAttributeName' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'Botspot and Blackout Secure' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'const isMessageMode = choices.length === 0' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'const imageView = $.NSImageView' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'const iconPath = ObjC.unwrap(args.objectAtIndex(12))' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'const cancelValue = ObjC.unwrap(args.objectAtIndex(14))' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'app.setApplicationIconImage' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF "function cancelAndExit()" "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF "cancelAndExit()" "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF '"$WOR_LOGO_PATH"' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'WOR_LOGO_PATH="$DIRECTORY/logo-full.png"' "$REPO_DIR/install-wor.sh" \
    && grep -qF ': "${WOR_ANNOUNCEMENT_TIMEOUT:=30}"' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF "'countdownTick:'" "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF -- '--timeout="$WOR_ANNOUNCEMENT_TIMEOUT"' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'function writeResult(value)' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'fileHandleWithStandardOutput.writeData(data)' "$REPO_DIR/install-wor-gui.sh" \
    && ! grep -qF 'echo -e "\\\\e[91m' "$REPO_DIR/install-wor-gui.sh" \
    && ! grep -qF 'console.log(selectedValue)' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'Choose Windows version' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF "'Windows 11'" "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'Choose Raspberry Pi model' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF "ObjC.import('AppKit')" "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'NSWindowStyleMaskClosable' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'windowWillClose:' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'app.activateIgnoringOtherApps(true)' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'app.requestUserAttention($.NSInformationalRequest)' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF "NSButton.buttonWithTitleTargetAction(cancelLabel, controller, 'cancelClicked:')" "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'Choose Windows language' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF "macos_choose '' 'No external, physical, writable drive was found. Connect a removable drive, then click Refresh.' __REFRESH__ Back" "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'darwin_list_device_choices' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'const actionValue = ObjC.unwrap(args.objectAtIndex(9))' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF "NSButton.buttonWithTitleTargetAction(actionLabel, controller, 'actionClicked:')" "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'Back Refresh __REFRESH__' "$REPO_DIR/install-wor-gui.sh" \
    && ! grep -qF 'Refresh detected devices' "$REPO_DIR/install-wor-gui.sh" \
    && ! grep -qF 'tkinter' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'Choose installation mode' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF "macos_choose '' \"\$confirm_summary\" Flash Back 'Advanced...' Advanced '' Flash" "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF '[ "$confirmation" == Cancel ] && exit 0' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'macos_advanced_options' "$REPO_DIR/install-wor-gui.sh" \
    && ! grep -qF 'display alert' "$REPO_DIR/install-wor-gui.sh" \
    && ! grep -qF 'display dialog' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'All data on the target drive will be erased.' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF "name: 'WorCompletionController'" "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'osascript -l JavaScript - "$completion_text"' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'next-steps.png' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'const imagePath = ObjC.unwrap(args.objectAtIndex(7)' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'exit "$installer_status"' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'WOR_GUI_ERROR_MARKER="$error_marker"' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'The Windows on Raspberry script stopped unexpectedly (exit code $installer_status).' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF "name: 'WorProgressController'" "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'NSTimer.scheduledTimerWithTimeIntervalTargetSelectorUserInfoRepeats' "$REPO_DIR/install-wor-gui.sh" \
    && ! grep -qF 'tell application "Terminal" to close front window' "$REPO_DIR/install-wor-gui.sh" \
    && ! grep -qF "terminal_runner=\"\$(mktemp)\"" "$REPO_DIR/install-wor-gui.sh" \
    && ! grep -qF 'do script (item 1 of argv)' "$REPO_DIR/install-wor-gui.sh" \
    && ! grep -qF 'choose from list' "$REPO_DIR/install-wor-gui.sh" \
    && pass "macOS GUI collects choices and shows a native progress window" \
    || fail "macOS GUI progress window is missing"

  grep -qF 'device_tree_address=0x3e0000' "$REPO_DIR/config-templates/pi4.config.txt" \
    && grep -qF 'device_tree_end=0x400000' "$REPO_DIR/config-templates/pi4.config.txt" \
    && grep -qF 'read_config_template "pi${RPI_MODEL}.config.txt"' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'set_default_config_txt' "$REPO_DIR/install-wor-gui.sh" \
    && pass "Pi 4 GUI config matches the current UEFI memory range" \
    || fail "Pi 4 GUI config uses stale device-tree addresses"
  #v1.51/v1.52 report a zero MAC (pftf/RPi4#283); v1.52/v1.53 do not boot from microSD (pftf/RPi4#285)
  grep -qF "UEFI_VER_PI4='v1.50'" "$REPO_DIR/install-wor.sh" \
    && ! grep -qF "UEFI_VER_PI4='v1.51'" "$REPO_DIR/install-wor.sh" \
    && ! grep -qF "UEFI_VER_PI4='v1.52'" "$REPO_DIR/install-wor.sh" \
    && ! grep -qF "UEFI_VER_PI4='v1.53'" "$REPO_DIR/install-wor.sh" \
    && pass "Pi 4 pins the only UEFI release with a working MAC address and microSD boot" \
    || fail "Pi 4 pins a UEFI release with a zero Ethernet MAC or a microSD boot regression"

  grep -qF '#Raspberry Pi 4 only; this setting is ignored for every other model.' "$REPO_DIR/install-wor.sh" \
    && grep -qF '[ -z "$PI4_AUTO_DISABLE_3GB" ] && PI4_AUTO_DISABLE_3GB=1' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'SetFirmwareEnvironmentVariableEx("RamLimitTo3GB", "{CD7CC258-31DB-22E6-9F22-63B0B8EED6B5}"' "$REPO_DIR/config-templates/pi4-ram-unlock.ps1" \
    && grep -qF '<settings pass="specialize">' "$REPO_DIR/config-templates/pi4-ram-unlock-specialize.xml" \
    && grep -qF '<WillReboot>Always</WillReboot>' "$REPO_DIR/config-templates/pi4-ram-unlock-specialize.xml" \
    && grep -qF 'read_config_template pi4-ram-unlock.ps1' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'read_config_template pi4-ram-unlock-specialize.xml' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'PI4_AUTO_DISABLE_3GB OOBE_NETWORK_BYPASS' "$REPO_DIR/install-wor.sh" \
    && ! grep -qF 'Prepare a one-time post-install RAM unlock' "$REPO_DIR/install-wor-gui.sh" \
    && pass "Pi 4 RAM unlock is config-only and runs automatically after the PE reboot" \
    || fail "Pi 4 automatic RAM unlock is unsafe or incomplete"

  grep -qF 'Pi 4 driver package is incomplete:' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'wimdir "$boot_mount/sources/boot.wim" 2 --path=/drivers/bcmgenet/bcmgenet.inf' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'wimdir "$boot_mount/sources/boot.wim" 2 --path=/drivers/mcci_dwchsotg/mcci_dwchsotg_hcd.inf' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'wimdir "$boot_mount/sources/boot.wim" 2 --path=/drivers/mcci_dwchsotg/mcci_dwchsotg_hub.inf' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'wimdir "$boot_mount/sources/boot.wim" 2 --path=/drivers/rpiuxflt/rpiuxflt.inf' "$REPO_DIR/install-wor.sh" \
    && pass "Pi 4 Ethernet, USB, and DMA filter drivers are checked before and after flashing" \
    || fail "Pi 4 Ethernet, USB, and DMA filter driver verification is incomplete"

  grep -qF 'if [ "$RPI_MODEL" == 4 ] && [ "$PI4_AUTO_DISABLE_3GB" == 1 ];then' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'after the injected drivers are installed' "$REPO_DIR/README.md" \
    && grep -qF 'CM4 requires the RAM limit set to 1 GB' "$REPO_DIR/README.md" \
    && grep -qF 'bcdedit /deletevalue' "$REPO_DIR/config-templates/pi4-ram-unlock.ps1" \
    && pass "Pi 4 RAM unlock runs only after Setup has installed the DMA filter, and clears the BCD memory cap" \
    || fail "Pi 4 RAM unlock may run before the DMA filter is installed, or CM4 guidance is missing"

  grep -qF '[ -z "$OOBE_NETWORK_BYPASS" ] && OOBE_NETWORK_BYPASS=1' "$REPO_DIR/install-wor.sh" \
    && grep -qF '<HideOnlineAccountScreens>true</HideOnlineAccountScreens>' "$REPO_DIR/config-templates/oobe-network-bypass.xml" \
    && grep -qF '<HideWirelessSetupInOOBE>true</HideWirelessSetupInOOBE>' "$REPO_DIR/config-templates/oobe-network-bypass.xml" \
    && grep -qF 'read_config_template oobe-network-bypass.xml' "$REPO_DIR/install-wor.sh" \
    && [ "$(grep -cF 'install_windows_setup_configuration ' "$REPO_DIR/install-wor.sh")" == 2 ] \
    && grep -qF 'cmp -s "$boot_mount/Autounattend.xml" "$install_mount/Autounattend.xml"' "$REPO_DIR/install-wor.sh" \
    && pass "Windows OOBE network bypass is default-on, configurable, written to both partitions, and verified" \
    || fail "Windows OOBE network bypass is incomplete"

  [ -f "$REPO_DIR/config-templates/pi3.config.txt" ] \
    && [ -f "$REPO_DIR/config-templates/pi4.config.txt" ] \
    && [ -f "$REPO_DIR/config-templates/pi5.config.txt" ] \
    && [ -f "$REPO_DIR/config-templates/pi4-ram-unlock.ps1" ] \
    && [ -f "$REPO_DIR/config-templates/pi4-ram-unlock-specialize.xml" ] \
    && [ -f "$REPO_DIR/config-templates/oobe-network-bypass.xml" ] \
    && [ -f "$REPO_DIR/config-templates/prefinalize.cmd" ] \
    && grep -qF 'read_config_template() {' "$REPO_DIR/install-wor.sh" \
    && ! grep -qF 'sync_repo_template' "$REPO_DIR/install-wor.sh" \
    && ! grep -qF 'raw.githubusercontent.com' "$REPO_DIR/install-wor.sh" \
    && pass "config-templates/ files exist as static, locally-editable files with no redundant per-file repo sync" \
    || fail "config-templates/ files are missing, or the removed per-file sync mechanism is still present"

  #these must be committed: a fresh clone without them silently writes a blank config.txt and the Pi will not boot
  if command -v git >/dev/null && git -C "$REPO_DIR" rev-parse --git-dir >/dev/null 2>&1 ;then
    [ "$(git -C "$REPO_DIR" ls-files config-templates/ | wc -l | tr -d ' ')" == 7 ] \
      && grep -qF 'This file ships with WoR-Flasher and is required to write a bootable drive.' "$REPO_DIR/install-wor.sh" \
      && pass "config-templates/ files are tracked by git and a missing one aborts instead of writing a blank config.txt" \
      || fail "config-templates/ files are untracked, or a missing template does not abort"
  else
    skip "git is unavailable; cannot verify that config-templates/ is tracked"
  fi

  grep -qF 'macos_advanced_options() {' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF "name: 'WorAdvancedController'" "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'NSButton.checkboxWithTitleTargetAction' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'Automatically disable the Pi 4 3 GB RAM limit after install' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'Skip flashing the device (dry run)' "$REPO_DIR/install-wor-gui.sh" \
    && [ "$(grep -cF 'export_installer_settings' "$REPO_DIR/install-wor-gui.sh")" == 1 ] \
    && grep -qF 'export "${WOR_INSTALLER_SETTINGS[@]}"' "$REPO_DIR/install-wor.sh" \
    && grep -qF "editMenu.addItemWithTitleActionKeyEquivalent('Copy', 'copy:', 'c')" "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'app.mainMenu = mainMenu' "$REPO_DIR/install-wor-gui.sh" \
    && pass "macOS and Linux GUIs expose an Advanced Options window for site-documented customizations" \
    || fail "Advanced Options window is missing or incomplete"

  ! grep -qF 'OVERRIDE_CONFIG_TXT' "$REPO_DIR/install-wor-gui.sh" \
    && ! grep -qF 'OVERRIDE_PI4_RAM_UNLOCK' "$REPO_DIR/install-wor-gui.sh" \
    && ! grep -qF 'OVERRIDE_OOBE_TEMPLATE' "$REPO_DIR/install-wor-gui.sh" \
    && pass "Redundant per-file repo-sync checkboxes were removed from both GUIs" \
    || fail "Redundant per-file repo-sync checkboxes are still present"

  #WSL reports uname -s as Linux, so it would otherwise pass the host gate and offer
  #WSL's own virtual disks as erasable targets
  grep -qF 'is_wsl() {' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'WSL_DISTRO_NAME' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'WSLENV' "$REPO_DIR/install-wor.sh" \
    && grep -qF '/proc/version' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'if is_wsl ;then' "$REPO_DIR/install-wor.sh" \
    && pass "WSL hosts are rejected before any drive is listed or erased" \
    || fail "WSL is not detected, so it would pass the Linux host gate"

  #a modal session never services default-mode run loop sources, so the Dock's quit Apple Event
  #is only delivered because each window registers a handler and pumps default mode from a timer
  [ "$(grep -cF "'handleQuitEvent:withReplyEvent:': {" "$REPO_DIR/install-wor-gui.sh")" == 4 ] \
    && [ "$(grep -cF "'pumpEvents:': {" "$REPO_DIR/install-wor-gui.sh")" == 4 ] \
    && [ "$(grep -cF '0x61657674, 0x71756974' "$REPO_DIR/install-wor-gui.sh")" == 4 ] \
    && [ "$(grep -cF 'addTimerForMode(pumpTimer' "$REPO_DIR/install-wor-gui.sh")" == 4 ] \
    && [ "$(grep -cF 'app.runModalForWindow(window)' "$REPO_DIR/install-wor-gui.sh")" == 4 ] \
    && pass "every macOS window responds to the Dock's Quit menu item" \
    || fail "a macOS window cannot receive the Dock's quit Apple Event"

  #these heredocs sit inside "$( ... )", so bash still tracks quote state through the body:
  #a lone apostrophe (e.g. "doesn't") silently swallows every function defined after it
  [ -z "$(awk '/<<.JXA.$/{inh=1; next} inh && /^JXA$/{inh=0; next} inh{n=gsub(/'"'"'/,""); if(n%2==1) print NR}' "$REPO_DIR/install-wor-gui.sh")" ] \
    && pass "JXA heredoc bodies contain no unpaired apostrophes" \
    || fail "an unpaired apostrophe in a JXA heredoc will corrupt shell parsing"

  #guards against the same class of breakage from any cause: run the real script far enough to
  #register its function definitions, then confirm every macos_* helper actually became a function
  macos_fn_expected="$(grep -c '^macos_[a-z_]*() {' "$REPO_DIR/install-wor-gui.sh")"
  macos_fn_probe="$(mktemp)"
  awk -v line="$(grep -n '^macos_start_cli() {' "$REPO_DIR/install-wor-gui.sh" | cut -d: -f1)" \
    'NR==line{print "declare -F | grep -c \"^declare -f macos_\"; exit 0"} {print}' \
    "$REPO_DIR/install-wor-gui.sh" > "$macos_fn_probe"
  #macos_start_cli itself is not defined yet at the probe point.
  #DIRECTORY is supplied because the probe is a copy: the GUI resolves install-wor.sh relative to its own path.
  [ "$(cd "$REPO_DIR" && DIRECTORY="$REPO_DIR" bash "$macos_fn_probe" 2>/dev/null | tail -n1)" == "$((macos_fn_expected - 1))" ] \
    && pass "all macOS helper functions parse as separate top-level definitions" \
    || fail "a macOS function definition is being swallowed by a preceding heredoc"
  rm -f "$macos_fn_probe"

  #the engine ignores PI4_AUTO_DISABLE_3GB unless RPI_MODEL is 4, so the GUIs must not offer it as a live choice
  grep -qF 'checked: parts[1]' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF "enabled: parts[2] !== '0'" "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'checkbox.enabled = rows[i].enabled' "$REPO_DIR/install-wor-gui.sh" \
    && [ "$(grep -cF 'pi4_applicable=1 || pi4_applicable=0' "$REPO_DIR/install-wor-gui.sh")" == 2 ] \
    && grep -qF 'not applicable to the Pi $RPI_MODEL' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF '2) [ "$pi4_applicable" == 1 ] && PI4_AUTO_DISABLE_3GB="$line" ;;' "$REPO_DIR/install-wor-gui.sh" \
    && pass "the Pi 4 RAM-unlock toggle is greyed out and ignored on other Pi models" \
    || fail "the Pi 4 RAM-unlock toggle is not gated on the selected Pi model"

  #in recovery mode the custom config.txt only boots the installer media
  grep -qF "config_scope='applied to the boot partition'" "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'not the installed Windows drive' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF "labelWithString('config.txt (' + configScope + '):')" "$REPO_DIR/install-wor-gui.sh" \
    && pass "the config.txt editor states its scope for the selected installation mode" \
    || fail "the config.txt editor does not state its scope per installation mode"

  grep -qF '[ -z "$SKIP_IMAGE_VERIFICATION" ] && SKIP_IMAGE_VERIFICATION=0' "$REPO_DIR/install-wor.sh" \
    && [ "$(grep -cF 'if [ "$SKIP_IMAGE_VERIFICATION" == 1 ];then' "$REPO_DIR/install-wor.sh")" == 2 ] \
    && [ "$(grep -cF 'verify_written_image "$DEVICE" "$PART1" "$PART2"' "$REPO_DIR/install-wor.sh")" == 2 ] \
    && grep -qF 'Use the latest UEFI firmware instead of the tested pinned version ($uefi_pinned)' "$REPO_DIR/install-wor-gui.sh" \
    && [ "$(grep -cF 'Use the latest Windows ARM64 drivers instead of the pinned version ($DRIVER_VER)' "$REPO_DIR/install-wor-gui.sh")" == 2 ] \
    && grep -qF 'Skip verifying the written image after flashing (not recommended)' "$REPO_DIR/install-wor-gui.sh" \
    && [ "$(grep -cF 'To continue, click Flash. To review or change these settings, click Advanced.' "$REPO_DIR/install-wor-gui.sh")" == 2 ] \
    && pass "Skip-verification option defaults off, wraps both verify_written_image calls, and confirm screens show pinned versions and guidance" \
    || fail "Skip-verification option or confirm-screen guidance is missing or incomplete"

  grep -qF '[ -z "$APPLY_CUSTOM_CONFIG_TXT" ] && APPLY_CUSTOM_CONFIG_TXT=1' "$REPO_DIR/install-wor.sh" \
    && grep -qF '[ -z "$CONFIG_TXT" ] || [ "$APPLY_CUSTOM_CONFIG_TXT" != 1 ] || echo "$CONFIG_TXT" | sudo tee "$boot_mount/config.txt" >/dev/null' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'if [ ! -z "$CONFIG_TXT" ] && [ "$APPLY_CUSTOM_CONFIG_TXT" == 1 ];then' "$REPO_DIR/install-wor.sh" \
    && grep -qF "applyConfigCheckbox = \$.NSButton.checkboxWithTitleTargetAction('Apply the customized config.txt below" "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'function updateConfigEditableState() {' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'textView.editable = enabled' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'scrollView.alphaValue = enabled ? 1.0 : 0.5' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF "Apply the customized config.txt below (unchecked uses the firmware's default config.txt)" "$REPO_DIR/install-wor-gui.sh" \
    && pass "Applying the customized config.txt is a togglable checkbox that greys out the editor on macOS" \
    || fail "Apply-customized-config.txt toggle is missing or incomplete"

  grep -qF '[ -z "$HIDE_EMPTY_DRIVES" ] && HIDE_EMPTY_DRIVES=1' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'configure_pe_settings_ini() {' "$REPO_DIR/install-wor.sh" \
    && grep -qF "settings_ini=\"\$PWD/peinstaller/winpe/2/settings.ini\"" "$REPO_DIR/install-wor.sh" \
    && [ "$(grep -cF 'configure_pe_settings_ini' "$REPO_DIR/install-wor.sh")" == 3 ] \
    && pass "HideEmptyDrives is written into the cached PE settings.ini before boot.wim assembly" \
    || fail "HideEmptyDrives support is missing or incomplete"

  grep -qF 'Allow Windows setup to continue without a network connection' "$REPO_DIR/install-wor-gui.sh" \
    && ! grep -qF "step=oobe" "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'OOBE_NETWORK_BYPASS' "$REPO_DIR/install-wor.sh" \
    && pass "macOS and Linux GUIs expose the OOBE network choice only in Advanced Options" \
    || fail "GUI OOBE network choice is incomplete"

  grep -qF 'emit_gui_progress() { #Input: line.' "$REPO_DIR/install-wor.sh" \
    && grep -qF $'STEP\t$STEP_NUM\t$STEP_TOTAL\t$1' "$REPO_DIR/install-wor.sh" \
    && grep -qF $'STATUS\t$1' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'LINUX_ASKPASS="$(mktemp)"' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'WOR_FLASH_TARGET="$DEVICE" WOR_ICON_PATH="$WOR_LOGO_PATH" SUDO_ASKPASS="$LINUX_ASKPASS" command sudo -A "$@"' "$REPO_DIR/install-wor.sh" \
    && grep -qF "yad \"\${yadflags[@]}\" --progress --no-buttons" "$REPO_DIR/install-wor-gui.sh" \
    && ! grep -qF '"$DIRECTORY/terminal-run"' "$REPO_DIR/install-wor-gui.sh" \
    && pass "GUI mode runs the installer without a visible terminal on macOS and Linux" \
    || fail "GUI mode still depends on a visible terminal"

  #a subshell cannot wait on a sibling, so `( wait "$installer_pid" ... )` wrote 127 immediately:
  #the progress window closed at once and the GUI reported failure while the flash kept running.
  #Both front-ends now go through one shared launcher, so there is a single copy to get right.
  [ "$(grep -cF '{ "$cli_script" > "$output_log" 2>&1; echo $? > "$done_marker"; } &' "$REPO_DIR/install-wor-gui.sh")" == 1 ] \
    && [ "$(grep -cF 'gui_start_installer' "$REPO_DIR/install-wor-gui.sh")" == 3 ] \
    && ! grep -qF '( wait "$installer_pid"; echo $? > "$done_marker" ) &' "$REPO_DIR/install-wor-gui.sh" \
    && pass "both GUIs record the installer exit status from the job itself, not a sibling wait" \
    || fail "a GUI still waits on a sibling process, so it reports completion immediately"

  #the password dialog cannot render in front of the progress window, so the installer authenticates
  #itself and the front-end holds the window back until it has. Asking in the GUI instead would ask
  #twice: a credential is recorded against the terminal of the process that collected it, and the
  #installer runs as a separate job.
  gui_auth_wait_line="$(grep -n 'while \[ ! -e "\$auth_marker" \] && \[ ! -f "\$done_marker" \] ;do' "$REPO_DIR/install-wor-gui.sh" | cut -d: -f1)"
  macos_progress_line="$(grep -n '<<<"\$progress_jxa"' "$REPO_DIR/install-wor-gui.sh" | head -n1 | cut -d: -f1)"
  [ -n "$gui_auth_wait_line" ] && [ -n "$macos_progress_line" ] \
    && [ "$gui_auth_wait_line" -lt "$macos_progress_line" ] \
    && grep -qF 'export WOR_GUI_AUTH_MARKER="$auth_marker"' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'gui_preauthenticate() {' "$REPO_DIR/install-wor.sh" \
    && grep -qF '[ "$RUN_MODE" == gui ] && gui_preauthenticate' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'command sudo -n -v >/dev/null 2>&1; sleep 30' "$REPO_DIR/install-wor.sh" \
    && pass "the installer authenticates once, before the progress window opens, and keeps it alive" \
    || fail "the password is asked for behind the progress window, or asked for twice"

  #exactly one place may prompt: the GUI collecting a credential of its own was the second dialog users saw
  [ "$(grep -cE '(^|[^n]) *sudo -v' "$REPO_DIR/install-wor-gui.sh")" == 0 ] \
    && [ "$(grep -cF 'sudo -v ||' "$REPO_DIR/install-wor.sh")" == 1 ] \
    && pass "the flash asks for the password once, in the process that uses it" \
    || fail "a credential is collected in more than one place, so the user is asked twice"

  #a failed flash must leave the log behind; the GUI has no terminal to fall back on
  grep -qF 'saved_log="$(wor_log_file)"' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'Installer log saved to $saved_log' "$REPO_DIR/install-wor-gui.sh" \
    && [ "$(grep -cF 'saved_log="$(gui_save_failure_log)"' "$REPO_DIR/install-wor-gui.sh")" == 3 ] \
    && pass "a failed run keeps its installer log for diagnosis" \
    || fail "a failed run deletes the only record of what went wrong"

  #the bar should advance inside a step, not jump once per step
  grep -qF 'emit_gui_substep() {' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'with_progress_capture pv -f -N' "$REPO_DIR/install-wor.sh" \
    && [ "$(grep -cF 'with_progress_capture pv -f -N' "$REPO_DIR/install-wor.sh")" == 6 ] \
    && grep -qF 'bar.maxValue = stepTotal * 100' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'bar.doubleValue = (stepNum - 1) * 100 + within' "$REPO_DIR/install-wor-gui.sh" \
    && pass "progress is captured within each step and carried into both progress bars" \
    || fail "progress still jumps a whole step at a time"

  #sub() is a built-in awk function, so using it as a variable is a syntax error
  linux_awk="$(sed -n "/^awk -F'\\\\t' '$/,/^' < \"\$progress_fifo\"/p" "$REPO_DIR/install-wor-gui.sh" | sed '1d;$d')"
  if [ -n "$linux_awk" ] && printf 'STEP\t3\t8\tThird\nSUBSTEP\t50\n' | awk -F'\t' "$linux_awk" >/dev/null 2>&1 ;then
    [ "$(printf 'STEP\t3\t8\tThird\nSUBSTEP\t50\n' | awk -F'\t' "$linux_awk" | grep -vE '^#' | tail -n1)" == 31 ] \
      && pass "the Linux progress program runs and maps a mid-step percentage correctly" \
      || fail "the Linux progress program computes the wrong overall percentage"
  else
    fail "the Linux progress awk program has a syntax error"
  fi

  #USE_CACHE has three values, so both GUIs need a menu, and it has to reach the installer
  grep -qF 'cachePopup.addItemWithTitle' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF "out.push(String(cachePopup.indexOfSelectedItem))" "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF '8) [ "$line" == 0 ] || [ "$line" == 1 ] || [ "$line" == 2 ] && USE_CACHE="$line" ;;' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF '"--field=Downloaded files":CB "$cache_items"' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'SKIP_IMAGE_VERIFICATION HIDE_EMPTY_DRIVES USE_CACHE' "$REPO_DIR/install-wor.sh" \
    && pass "both GUIs can choose the download cache mode and pass it to the installer" \
    || fail "the download cache mode is not adjustable from the GUI"

  #a one-line `case` inside $( ) is mis-parsed at the first ')', leaking raw shell into the dialog
  if ! grep -qE '\$\(case .* in [^)]*\)' "$REPO_DIR/install-wor-gui.sh" \
    && ! grep -qE '\$\(case .* in [^)]*\)' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'cache_mode_label() {' "$REPO_DIR/install-wor.sh" ;then
    #render it the way the confirmation screens do
    cache_label_fn="$(sed -n '/^cache_mode_label() {/,/^}/p' "$REPO_DIR/install-wor.sh")"
    cache_rendered="$(bash -c "$cache_label_fn"$'\n''cache_mode_label 2' 2>/dev/null)"
    [ "$cache_rendered" == 'Trust the cache without checking' ] \
      && pass "the confirmation screens show the cache mode as words, not shell source" \
      || fail "the cache mode does not render correctly on the confirmation screens"
  else
    fail "an inline case in a command substitution will leak shell source into a dialog"
  fi

  #clearing several GB used to sit on a dead bar with no indication anything was happening
  grep -qF 'clear_cached_components() {' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'status "Deleting $(basename "$target") ($((removed+1)) of $total)"' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'emit_gui_substep $((removed * 100 / total))' "$REPO_DIR/install-wor.sh" \
    && ! grep -qF 'rm -rf "$PWD/peinstaller" "$PWD/driverpackage"' "$REPO_DIR/install-wor.sh" \
    && grep -qF '} else if (subLine.length > 0) {' "$REPO_DIR/install-wor-gui.sh" \
    && pass "clearing the cache reports each deletion and moves the bar before step 1" \
    || fail "clearing the cache gives no progress feedback"

  #the progress window needs a way out, a step counter, and the usual window buttons
  grep -qF "abortClicked:" "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF "'windowShouldClose:'" "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'NSWindowStyleMaskTitled | $.NSWindowStyleMaskClosable | $.NSWindowStyleMaskMiniaturizable' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF "stepLabel.stringValue = 'Step ' + stepNum + ' of ' + stepTotal" "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'abort_marker="$(mktemp -u)"' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'kill_process_tree "$installer_pid"' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'Stop flashing this drive?' "$REPO_DIR/install-wor-gui.sh" \
    && pass "the progress window can be aborted, shows step x of y, and has close and minimise" \
    || fail "the progress window cannot be aborted or lacks its window controls"

  #clicking the Dock icon sends aevt/rapp; without a handler a minimised window can never be restored
  [ "$(grep -cF "'handleReopenEvent:withReplyEvent:': {" "$REPO_DIR/install-wor-gui.sh")" == 4 ] \
    && [ "$(grep -cF '0x61657674, 0x72617070' "$REPO_DIR/install-wor-gui.sh")" == 4 ] \
    && grep -qF 'if (window.isMiniaturized) window.deminiaturize(null)' "$REPO_DIR/install-wor-gui.sh" \
    && pass "clicking the Dock icon restores a minimised window" \
    || fail "a minimised window cannot be restored from the Dock"

  #Quit during a flash used to exit on the spot: a bogus failure dialog, with the flash left running
  progress_block="$(sed -n "/^  progress_jxa=\"\$(cat <<'JXA'\$/,/^JXA\$/p" "$REPO_DIR/install-wor-gui.sh")"
  ! printf '%s' "$progress_block" | grep -qF '$.exit(0)' \
    && printf '%s' "$progress_block" | grep -qF 'if (confirmAbort()) app.stopModalWithCode($.NSCancelButton)' \
    && [ "$(grep -c '        \$.exit(0)' "$REPO_DIR/install-wor-gui.sh")" == 4 ] \
    && grep -qF '[ -f "$done_marker" ] || touch "$abort_marker"' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'wait "$installer_pid" 2>/dev/null' "$REPO_DIR/install-wor-gui.sh" \
    && pass "quitting mid-flash confirms first and any unexpected window exit stops the installer" \
    || fail "quitting mid-flash reports a bogus failure or leaves the installer running"

  #the completion dialog has an "Open Log" button that extracts the log path and opens it in the default editor
  completion_block="$(sed -n "/^  completion_jxa=\"\$(cat <<'JXA'\$/,/^JXA\$/p" "$REPO_DIR/install-wor-gui.sh")"
  printf '%s' "$completion_block" | grep -qF "'openLogClicked:': {" \
    && printf '%s' "$completion_block" | grep -qF "openButton = $.NSButton.buttonWithTitleTargetAction('Open Log'" \
    && printf '%s' "$completion_block" | grep -qF "$.NSWorkspace.sharedWorkspace.openFileWithApplication" \
    && printf '%s' "$completion_block" | grep -qF "const logMatch = message.match(/Full log: (.+)$/)" \
    && pass "completion dialog can open the log file from the message" \
    || fail "completion dialog cannot open the log file"

  #the completion dialog has a "Copy" button that copies the log path to the clipboard
  printf '%s' "$completion_block" | grep -qF "'copyLogClicked:': {" \
    && printf '%s' "$completion_block" | grep -qF "copyButton = $.NSButton.buttonWithTitleTargetAction('Copy'" \
    && printf '%s' "$completion_block" | grep -qF "$.NSPasteboard.generalPasteboard" \
    && printf '%s' "$completion_block" | grep -qF "pb.setStringForType($(logPath)" \
    && pass "completion dialog can copy the log path to the clipboard" \
    || fail "completion dialog cannot copy the log path"

  #when an error occurs in the installer, gui_error_dialog writes to WOR_GUI_ERROR_MARKER with touch+sync before showing its dialog
  gui_error_block="$(sed -n "/^gui_error_dialog() {/,/^}/p" "$REPO_DIR/install-wor.sh" | head -n 20)"
  printf '%s' "$gui_error_block" | grep -qF 'mkdir -p "$(dirname "$WOR_GUI_ERROR_MARKER")' \
    && printf '%s' "$gui_error_block" | grep -qF 'touch "$WOR_GUI_ERROR_MARKER"' \
    && printf '%s' "$gui_error_block" | grep -qF 'sync' \
    && pass "gui_error_dialog reliably creates error_marker with touch+sync before dialog" \
    || fail "gui_error_dialog does not reliably create error_marker"

  #the GUI checks for error_marker with -s (non-empty) to ensure it was actually written, not just the file existing
  marker_check_fn="$(sed -n '/^installer_showed_own_error() {/,/^}/p' "$REPO_DIR/install-wor-gui.sh")"
  printf '%s' "$marker_check_fn" | grep -qF '[ -e "$error_marker" ] && [ -s "$error_marker" ]' \
    && [ "$(grep -cF 'if installer_showed_own_error ;then' "$REPO_DIR/install-wor-gui.sh")" == 2 ] \
    && pass "both GUIs check error_marker exists AND is non-empty before skipping the completion dialog" \
    || fail "GUI does not verify error_marker is non-empty before trusting it"

  #aborting must take down the sudo-owned children too, not just the top-level job
  kill_tree_dir="$(mktemp -d)"

  sed -n '/^kill_process_tree() {/,/^}/p' "$REPO_DIR/install-wor-gui.sh" > "$kill_tree_dir/fn.sh"
  printf '#!/bin/bash\nsleep 60\n' > "$kill_tree_dir/child.sh"
  printf '#!/bin/bash\n"%s/child.sh" &\nsleep 60\n' "$kill_tree_dir" > "$kill_tree_dir/parent.sh"
  chmod +x "$kill_tree_dir/child.sh" "$kill_tree_dir/parent.sh"
  (
    #shellcheck disable=SC1090
    . "$kill_tree_dir/fn.sh"
    "$kill_tree_dir/parent.sh" & tree_pid=$!
    sleep 1
    kill_process_tree "$tree_pid" 2>/dev/null
    sleep 1
    kill -0 "$tree_pid" 2>/dev/null && exit 1
    pgrep -f "$kill_tree_dir/child.sh" >/dev/null 2>&1 && exit 1
    exit 0
  ) >/dev/null 2>&1 \
    && pass "aborting stops the installer and every process it started" \
    || fail "aborting leaves the flash running in the background"
  pkill -f "$kill_tree_dir/child.sh" 2>/dev/null
  rm -rf "$kill_tree_dir"

  shared_function_checks
}

#Runs the real functions out of install-wor.sh rather than restating their logic here, so a test can
#never pass against behaviour the shipped script no longer has.
run_in_engine() { #Input: shell code. Runs it with install-wor.sh sourced and a representative run configured.
  env -u CONFIG_TXT -u DEVICE -u DL_DIR -u SOURCE_FILE NO_UPDATE=1 DIRECTORY="$REPO_DIR" \
    bash -c '
      #shellcheck disable=SC1090
      source "$DIRECTORY/install-wor.sh" source >/dev/null 2>&1
      RPI_MODEL=4 BID=22631.2861 WIN_LANG=en-us DEVICE=/dev/does-not-exist
      CAN_INSTALL_ON_SAME_DRIVE=1 DL_DIR=/tmp/wor-test-dl
      '"$1"
}

shared_function_checks() {
  info "== Shared engine functions =="

  #the CLI banner and both GUI overviews must describe a run from one place, or they drift apart
  summary_labels="$(run_in_engine 'settings_summary | cut -f1 | tr "\n" ","')"
  [ "$summary_labels" == 'WoR-Flasher version,Target drive,Target hardware,Operating system,Installation mode,Offline OOBE,Pi 4 RAM unlock,UEFI firmware,Windows ARM64 drivers,Custom config.txt,Hide empty drives,Verify written image,Downloaded files,Dry run,Download directory,Log file,' ] \
    && [ "$(run_in_engine 'settings_summary | awk -F"\t" "NF != 2" | wc -l | tr -d " "')" == 0 ] \
    && pass "settings_summary emits one tab-separated label/value pair per setting" \
    || fail "settings_summary is missing settings or emits malformed lines: $summary_labels"

  #every toggle the Advanced Options windows offer has to be visible on the confirmation screen
  [ "$(run_in_engine 'DRY_RUN=1 SKIP_IMAGE_VERIFICATION=1 USE_CACHE=2 APPLY_CUSTOM_CONFIG_TXT=0 UEFI_USE_LATEST=1 DRIVERS_USE_LATEST=0 OOBE_NETWORK_BYPASS=0 PI4_AUTO_DISABLE_3GB=0 HIDE_EMPTY_DRIVES=0 settings_summary | tail -n +2 | cut -f2 | tr "\n" "|"')" \
       == "/dev/does-not-exist|Raspberry Pi 4|Windows 11 (en-us) arm64 build 22631.2861|Install Windows onto this drive|Disabled|Disabled|Latest|Pinned (v0.17)|Using the firmware default|No|No (skipped)|Trust the cache without checking|Yes (no changes will be written)|/tmp/wor-test-dl|/tmp/wor-test-dl/last-run.log|" ] \
    && pass "every Advanced Options toggle changes what the confirmation screens show" \
    || fail "a setting is not reflected in settings_summary"

  #the summary must never abort a run, however little can be read back about the chosen drive
  [ "$(run_in_engine 'DEVICE=/dev/definitely-not-here settings_summary >/dev/null 2>&1; echo $?')" == 0 ] \
    && pass "an unreadable target drive does not break the summary" \
    || fail "settings_summary fails when the drive details cannot be read"

  #yad renders its text as pango markup, so an ISO filename containing markup characters would corrupt the window
  markup_expected="- Windows source: <b>/tmp/a&lt;b&gt;&amp;c.iso</b>"
  [ "$(run_in_engine 'SOURCE_FILE="/tmp/a<b>&c.iso"; settings_summary_markup | sed -n "/Windows source/p"')" == "$markup_expected" ] \
    && [ "$(run_in_engine 'settings_summary_markup | grep -c "^- Target drive: <b>/dev/does-not-exist</b>$"')" == 1 ] \
    && grep -qF 'window_text="$(settings_summary_markup)' "$REPO_DIR/install-wor-gui.sh" \
    && pass "the Linux overview escapes pango markup in every value it shows" \
    || fail "a value containing markup characters would corrupt the Linux overview window"

  #the macOS confirmation screen and the CLI banner list the same settings, from the same renderer
  [ "$(run_in_engine 'settings_summary_plain | sed -n 2p')" == 'Target drive: /dev/does-not-exist' ] \
    && [ "$(run_in_engine 'settings_summary_plain "  %-24s %s\n" | sed -n 2p')" == '  Target drive:            /dev/does-not-exist' ] \
    && grep -qF 'confirm_summary="$(settings_summary_plain)' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF "settings_summary_plain '  %-24s %s" "$REPO_DIR/install-wor.sh" \
    && pass "the CLI banner and the macOS confirmation screen render one shared summary" \
    || fail "the CLI banner and the macOS confirmation screen do not share a renderer"

  #a Pi 3 or Pi 5 has no 3 GB RAM limit, so offering the line at all would be misleading
  ! run_in_engine 'RPI_MODEL=5 settings_summary' | grep -q 'Pi 4 RAM unlock' \
    && run_in_engine 'RPI_MODEL=4 settings_summary' | grep -q 'Pi 4 RAM unlock' \
    && pass "the Pi 4 RAM unlock line appears only for a Pi 4" \
    || fail "the Pi 4 RAM unlock line is shown for the wrong models"

  [ "$(run_in_engine 'for RPI_MODEL in 3 4 5 ;do uefi_pinned_version ;done | tr "\n" " "')" == "$(grep -oE "UEFI_VER_PI[345]='[^']*'" "$REPO_DIR/install-wor.sh" | cut -d"'" -f2 | tr '\n' ' ')" ] \
    && pass "uefi_pinned_version returns the pinned firmware for every supported model" \
    || fail "uefi_pinned_version does not match the pinned UEFI versions"

  #the CLI used to write the firmware's own config.txt while the GUI wrote the shipped template
  config_from_engine="$(run_in_engine 'set_default_config_txt; printf "%s" "$CONFIG_TXT"')"
  [ -n "$config_from_engine" ] \
    && [ "$config_from_engine" == "$(printf '\n\n%s' "$(cat "$REPO_DIR/config-templates/pi4.config.txt")")" ] \
    && [ "$(run_in_engine 'CONFIG_TXT=mine; set_default_config_txt; printf "%s" "$CONFIG_TXT"')" == mine ] \
    && grep -qF 'set_default_config_txt' "$REPO_DIR/install-wor.sh" \
    && pass "a CLI run and a GUI run start from the same shipped config.txt" \
    || fail "the CLI and the GUI do not agree on the default config.txt"

  #the two front-ends used to keep their own export lists, so one could silently drop a setting
  exported_settings="$(run_in_engine 'SOURCE_FILE=/tmp/x.iso; set_default_config_txt; export_installer_settings
    comm -23 <(printf "%s\n" "${WOR_INSTALLER_SETTINGS[@]}" | sort -u) <(bash -c compgen\ -e | sort -u)')"
  [ -z "$exported_settings" ] \
    && [ "$(run_in_engine 'printf "%s\n" "${WOR_INSTALLER_SETTINGS[@]}" | sort | uniq -d | wc -l | tr -d " "')" == 0 ] \
    && pass "export_installer_settings hands the installer every collected setting exactly once" \
    || fail "these settings never reach the installer: $exported_settings"

  #shared behaviour belongs to install-wor.sh; a second copy in the GUI silently shadows it on sourcing
  duplicate_functions="$(comm -12 \
    <(grep -oE '^[a-zA-Z_][a-zA-Z0-9_]*\(\) \{' "$REPO_DIR/install-wor.sh" | sort -u) \
    <(grep -oE '^[a-zA-Z_][a-zA-Z0-9_]*\(\) \{' "$REPO_DIR/install-wor-gui.sh" | sort -u))"
  [ -z "$duplicate_functions" ] \
    && pass "no function is defined in both install-wor.sh and install-wor-gui.sh" \
    || fail "these functions are defined twice and will drift apart: $duplicate_functions"

  #the GUI's own helpers call shared ones, so the source has to happen before any of them are defined
  gui_source_line="$(grep -n 'source "$cli_script" source' "$REPO_DIR/install-wor-gui.sh" | head -n1 | cut -d: -f1)"
  gui_first_function_line="$(grep -nE '^[a-zA-Z_][a-zA-Z0-9_]*\(\) \{' "$REPO_DIR/install-wor-gui.sh" | head -n1 | cut -d: -f1)"
  [ -n "$gui_source_line" ] && [ -n "$gui_first_function_line" ] \
    && [ "$gui_source_line" -lt "$gui_first_function_line" ] \
    && [ "$(grep -c 'source "$cli_script" source' "$REPO_DIR/install-wor-gui.sh")" == 1 ] \
    && pass "the GUI sources install-wor.sh once, before it defines anything of its own" \
    || fail "the GUI defines functions before sourcing install-wor.sh, so shared ones are unavailable"

  #every shared name the GUI relies on has to survive as a function, on both platforms
  missing_shared="$(run_in_engine 'for fn in error warning status echo_red gui_error_dialog settings_summary \
    cache_mode_label install_mode_label uefi_pinned_version set_default_config_txt describe_device human_size \
    export_installer_settings read_config_template drive_capability validate_install_mode is_safe_target_device \
    list_bids list_bids_supported get_bid get_os_name list_langs default_win_lang get_device_name get_size_raw \
    get_file_size setup ;do declare -F "$fn" >/dev/null || echo "$fn" ;done')"
  [ -z "$missing_shared" ] \
    && pass "install-wor.sh exports every shared function the GUI depends on" \
    || fail "the GUI calls these functions, but install-wor.sh does not define them: $missing_shared"

  #`warning` was called by the self-updater without ever being defined, so a failed update printed nothing
  [ "$(run_in_engine 'warning "update failed" 2>&1 | sed "s/\x1b\[[0-9;]*m//g"')" == 'update failed' ] \
    && grep -qF 'warning "Automatic update failed. Continuing..."' "$REPO_DIR/install-wor.sh" \
    && pass "a failed automatic update reports itself instead of dying on an undefined command" \
    || fail "the self-updater still calls an undefined warning command"

  #ISO acceptance used to be written out three times, so the CLI and the GUI could disagree on what is usable
  iso_dir="$(mktemp -d)"
  : > "$iso_dir/small.iso"
  : > "$iso_dir/notanimage.txt"
  #sparse, so these cost nothing on disk. 2 GB sits just under the 3 GB floor, making it a boundary test
  dd if=/dev/zero of="$iso_dir/22631.2861_ARM64_en-us.iso" bs=1 count=0 seek=4g >/dev/null 2>&1
  dd if=/dev/zero of="$iso_dir/truncated.iso" bs=1 count=0 seek=2g >/dev/null 2>&1
  [ "$(run_in_engine 'validate_iso_file "'"$iso_dir"'/22631.2861_ARM64_en-us.iso"; echo "rc=$?"')" == 'rc=0' ] \
    && [ "$(run_in_engine 'validate_iso_file "'"$iso_dir"'/missing.iso" >/dev/null; echo "rc=$?"')" == 'rc=1' ] \
    && [ "$(run_in_engine 'validate_iso_file "'"$iso_dir"'/notanimage.txt" >/dev/null; echo "rc=$?"')" == 'rc=1' ] \
    && [ "$(run_in_engine 'validate_iso_file "'"$iso_dir"'/truncated.iso" >/dev/null; echo "rc=$?"')" == 'rc=1' ] \
    && [ "$(run_in_engine 'validate_iso_file "'"$iso_dir"'/truncated.iso" 2>&1')" == 'This file is smaller than 3GB and is probably incomplete.' ] \
    && [ "$(grep -cF 'validate_iso_file' "$REPO_DIR/install-wor.sh")" == 3 ] \
    && [ "$(grep -cF 'validate_iso_file' "$REPO_DIR/install-wor-gui.sh")" == 1 ] \
    && pass "the CLI and the GUI accept and reject exactly the same ISO files" \
    || fail "ISO validation is duplicated or disagrees between the CLI and the GUI"

  #the build number and language are read back out of the filename, in both front-ends
  [ "$(run_in_engine 'bid_from_iso_name "'"$iso_dir"'/22631.2861_ARM64_en-us.iso"')" == '22631.2861' ] \
    && [ "$(run_in_engine 'lang_from_iso_name "'"$iso_dir"'/22631.2861_ARM64_EN-US.iso"')" == 'en-us' ] \
    && [ -z "$(run_in_engine 'bid_from_iso_name "'"$iso_dir"'/small.iso"')" ] \
    && ! grep -qF "tr '_ -' " "$REPO_DIR/install-wor-gui.sh" \
    && pass "ISO build number and language are inferred by one shared function" \
    || fail "ISO build number or language inference is duplicated or wrong"

  #the CLI listed only winfiles_from_iso_*, the GUI listed both, from two different pipelines
  winfiles_dir="$(mktemp -d)"
  mkdir -p "$winfiles_dir/winfiles_22631.2861_en-us" "$winfiles_dir/winfiles_from_iso_22000.1_de-de" \
    "$winfiles_dir/notwinfiles_9_9" "$winfiles_dir/winfiles_incomplete_xx"
  touch "$winfiles_dir/winfiles_22631.2861_en-us/alldone" "$winfiles_dir/winfiles_from_iso_22000.1_de-de/alldone" \
    "$winfiles_dir/notwinfiles_9_9/alldone"
  [ "$(run_in_engine 'list_cached_winfiles "'"$winfiles_dir"'" | tr "\n" " "')" == 'winfiles_from_iso_22000.1_de-de winfiles_22631.2861_en-us ' ] \
    && [ "$(run_in_engine 'bid_from_winfiles_dir winfiles_from_iso_22000.1_de-de')" == '22000.1' ] \
    && [ "$(run_in_engine 'lang_from_winfiles_dir winfiles_22631.2861_en-us')" == 'en-us' ] \
    && ! grep -qF "name 'alldone'" "$REPO_DIR/install-wor-gui.sh" \
    && pass "cached Windows files are discovered the same way by the CLI and the GUI" \
    || fail "cached Windows file discovery is duplicated or lists the wrong folders"

  #the GUI enumerated drives with its own lsblk call, so a filter added here would not apply there
  ! grep -qF 'lsblk -I 8,179,259' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'for device in $(list_dev_paths) ;do' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'list_dev_paths() {' "$REPO_DIR/install-wor.sh" \
    && [ "$(grep -cF 'lsblk -I 8,179,259' "$REPO_DIR/install-wor.sh")" == 1 ] \
    && pass "both front-ends enumerate candidate drives through one function" \
    || fail "drive enumeration is duplicated between the CLI and the GUI"

  #en-us has to come first so it is the preselected row, and both GUIs offer the same order
  [ "$(run_in_engine 'list_langs_preferred | cut -d: -f1 | head -n1')" == 'en-us' ] \
    && [ "$(run_in_engine 'list_langs_preferred | wc -l | tr -d " "')" == "$(run_in_engine 'list_langs | wc -l | tr -d " "')" ] \
    && [ "$(grep -cF 'list_langs_preferred' "$REPO_DIR/install-wor-gui.sh")" == 2 ] \
    && run_in_engine 'is_known_win_lang en-us' \
    && ! run_in_engine 'is_known_win_lang en' \
    && pass "both GUIs offer the same language order and accept the same codes" \
    || fail "the language list differs between front-ends, or an invalid code is accepted"

  rm -rf "$iso_dir" "$winfiles_dir"

  #one entry point, but never a guess: DISPLAY is also set over SSH and in CI, and this tool erases disks
  [ "$(cd "$REPO_DIR" && ./install-wor.sh --help | head -n1)" == "WoR-Flasher $(run_in_engine 'printf %s "$WOR_FLASHER_VERSION"')" ] \
    && grep -qF 'exec "$DIRECTORY/install-wor-gui.sh" "$@"' "$REPO_DIR/install-wor.sh" \
    && ! grep -qE 'if .*-n .\$DISPLAY|command -v yad .*&&.*exec' "$REPO_DIR/install-wor.sh" \
    && [ "$(cd "$REPO_DIR" && ./install-wor.sh --bogus 2>&1 | sed 's/\x1b\[[0-9;]*m//g'; echo "rc=${PIPESTATUS[0]}")" == "Unknown argument '--bogus'. Run 'install-wor.sh --help' for usage.
rc=1" ] \
    && pass "install-wor.sh --gui hands over explicitly and never auto-detects a display" \
    || fail "the CLI entry point is missing, or it guesses whether to open a GUI"

  #a bug report is unactionable without knowing which version produced it
  version="$(run_in_engine 'printf %s "$WOR_FLASHER_VERSION"')"
  [[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] \
    && [ "$(cd "$REPO_DIR" && ./install-wor.sh --version)" == "WoR-Flasher $version" ] \
    && [ "$(cd "$REPO_DIR" && ./install-wor.sh -V)" == "WoR-Flasher $version" ] \
    && [ "$(run_in_engine 'settings_summary | head -n1')" == "$(printf 'WoR-Flasher version\t%s' "$version")" ] \
    && pass "the version is a single semantic value, reported by --version and on every run" \
    || fail "the version is missing, malformed, or not reported: '$version'"

  #the version history and the README must not describe different releases
  grep -qE "^#$version - \S" "$REPO_DIR/install-wor.sh" \
    && grep -qE "^- \*\*$version\*\*" "$REPO_DIR/README.md" \
    && grep -qF "version-$version-" "$REPO_DIR/README.md" \
    && pass "the version history in install-wor.sh, the README and the version badge all agree" \
    || fail "the version history or the README badge is out of step with WOR_FLASHER_VERSION"

  #GitHub only surfaces these community files if they are named exactly right and are tracked
  missing_community=''
  for community_file in LICENSE NOTICE README.md CONTRIBUTING.md CODE_OF_CONDUCT.md SECURITY.md \
    .github/FUNDING.yml .github/PULL_REQUEST_TEMPLATE.md .github/ISSUE_TEMPLATE/config.yml ;do
    [ -s "$REPO_DIR/$community_file" ] || missing_community="$missing_community $community_file"
  done
  [ -z "$missing_community" ] \
    && grep -qF 'GNU GENERAL PUBLIC LICENSE' "$REPO_DIR/LICENSE" \
    && grep -qF 'Version 3, 29 June 2007' "$REPO_DIR/LICENSE" \
    && pass "every community health file is present and the license is GPL-3.0" \
    || fail "these community health files are missing or empty:$missing_community"

  [ -x "$REPO_DIR/install-wor-hook.sh" ] \
    && grep -qF 'list-devices)' "$REPO_DIR/install-wor-hook.sh" \
    && grep -qF 'describe-device)' "$REPO_DIR/install-wor-hook.sh" \
    && grep -qF 'summary)' "$REPO_DIR/install-wor-hook.sh" \
    && grep -qF 'exec "$ENGINE"' "$REPO_DIR/install-wor-hook.sh" \
    && pass "integration adapter exposes the shared installer engine" \
    || fail "integration adapter is missing or does not use the shared engine"

  #upstream ships no license, so the fork must say so rather than implying a grant it cannot make
  grep -qF 'ships no LICENSE file' "$REPO_DIR/NOTICE" \
    && grep -qF 'https://github.com/Botspot/wor-flasher' "$REPO_DIR/NOTICE" \
    && grep -qF 'does not distribute' "$REPO_DIR/NOTICE" 2>/dev/null || grep -qF 'downloads, but does not redistribute' "$REPO_DIR/NOTICE" \
    && grep -qF '[NOTICE](NOTICE)' "$REPO_DIR/README.md" \
    && pass "the upstream licensing caveat is recorded in NOTICE and linked from the README" \
    || fail "the upstream licensing caveat is missing"

  #attribution is the one thing a fork must never get wrong
  missing_attribution=''
  for attribution in 'https://github.com/Botspot' 'https://blackoutsecure.app' \
    'https://linktr.ee/billmcilhargey' 'https://discord.gg/RXSTvaUvuu' 'https://discord.gg/jQCpfVK' \
    'https://github.com/sponsors/Botspot' 'https://uupdump.net' ;do
    grep -qF "$attribution" "$REPO_DIR/README.md" || missing_attribution="$missing_attribution $attribution"
  done
  [ -z "$missing_attribution" ] \
    && grep -qF 'github: [Botspot, blackoutsecure]' "$REPO_DIR/.github/FUNDING.yml" \
    && grep -qF 'https://github.com/sponsors/blackoutsecure' "$REPO_DIR/README.md" \
    && pass "the README credits the original author, this fork's maintainer and every upstream project" \
    || fail "the README is missing this attribution:$missing_attribution"

  #WoR-PE applies install.wim with DISM rather than running Windows Setup's media flow, so nothing
  #performs the implicit answer-file search and Autounattend.xml on the media root is never read.
  #Windows then stops at "Let's connect you to a network". The documented prefinalize.cmd hook is
  #the only point that can put the answer file where the installed OS looks for it.
  pe_hook_dir="$(mktemp -d)"
  mkdir -p "$pe_hook_dir/peinstaller/winpe/2"
  printf 'stub\n' > "$pe_hook_dir/peinstaller/winpe/2/setup.exe"
  pe_hook_out="$(cd "$pe_hook_dir" && env -u CONFIG_TXT NO_UPDATE=1 DIRECTORY="$REPO_DIR" bash -c '
    source "$DIRECTORY/install-wor.sh" source >/dev/null 2>&1
    RPI_MODEL=4; OOBE_NETWORK_BYPASS=1; PI4_AUTO_DISABLE_3GB=0
    mark_cache "$PWD/peinstaller" token-v1
    configure_pe_prefinalize
    #the answer file has to reach the installed OS, and the hook must not invalidate the cache
    grep -qF "<HideWirelessSetupInOOBE>true</HideWirelessSetupInOOBE>" peinstaller/winpe/2/scripts/unattend.xml && echo answer-staged
    grep -qF "Windows\\Panther" peinstaller/winpe/2/scripts/prefinalize.cmd && echo targets-panther
    grep -q "exit /b 0" peinstaller/winpe/2/scripts/prefinalize.cmd && echo always-exits-zero
    grep -qU $'"'"'\r'"'"' peinstaller/winpe/2/scripts/prefinalize.cmd && echo crlf
    cache_is_current "$PWD/peinstaller" token-v1 && echo cache-intact
    #turning both customizations off must not leave a stale hook behind in the cache
    OOBE_NETWORK_BYPASS=0; RPI_MODEL=5
    configure_pe_prefinalize
    [ -e peinstaller/winpe/2/scripts ] || echo stale-hook-removed
  ')"
  rm -rf "$pe_hook_dir"
  for pe_hook_expected in answer-staged targets-panther always-exits-zero crlf cache-intact stale-hook-removed ;do
    printf '%s\n' "$pe_hook_out" | grep -qx "$pe_hook_expected" || missing_pe_hook="$missing_pe_hook $pe_hook_expected"
  done
  [ -z "$missing_pe_hook" ] \
    && [ "$(grep -cF 'configure_pe_prefinalize' "$REPO_DIR/install-wor.sh")" == 3 ] \
    && pass "the offline-OOBE answer file is delivered through WoR-PE's prefinalize hook" \
    || fail "the answer file would never be read, so Windows stops at the network screen:$missing_pe_hook"

  #both the media copies and the hook copy have to come from one builder, or they can disagree
  [ "$(run_in_engine 'OOBE_NETWORK_BYPASS=1; RPI_MODEL=5; unattend_xml | grep -c "HideWirelessSetupInOOBE"')" == 1 ] \
    && [ "$(run_in_engine 'OOBE_NETWORK_BYPASS=0; PI4_AUTO_DISABLE_3GB=0; RPI_MODEL=5; unattend_xml >/dev/null 2>&1; echo $?')" == 1 ] \
    && grep -qF 'unattend_xml | sudo tee "$destination"' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'unattend_xml > "$scripts_dir/unattend.xml"' "$REPO_DIR/install-wor.sh" \
    && pass "the media copies and the prefinalize copy of the answer file share one builder" \
    || fail "the answer file is built in more than one place"

  #one variable decides where the log goes, and it is resolved on use: the Linux GUI can still
  #change DL_DIR after this script has been sourced
  [ "$(run_in_engine 'wor_log_file')" == '/tmp/wor-test-dl/last-run.log' ] \
    && [ "$(run_in_engine 'WOR_LOG_FILE=/tmp/elsewhere.log; wor_log_file')" == '/tmp/elsewhere.log' ] \
    && [ "$(run_in_engine 'DL_DIR=/tmp/moved-later; wor_log_file')" == '/tmp/moved-later/last-run.log' ] \
    && [ "$(grep -cF 'last-run.log' "$REPO_DIR/install-wor-gui.sh")" == 0 ] \
    && pass "one variable decides where the run log goes, resolved when it is needed" \
    || fail "the log path is hardcoded, or does not follow DL_DIR and WOR_LOG_FILE"

  #logs and caches are generated beside the scripts; none of it may ever be committed
  if command -v git >/dev/null && git -C "$REPO_DIR" rev-parse --git-dir >/dev/null 2>&1 ;then
    tracked_junk="$(git -C "$REPO_DIR" ls-files | grep -E '(^|/)(cache/|wget-log|.*\.log$)' || true)"
    unignored=''
    for junk_path in cache/x wget-log some.log .test-workspace/x ;do
      #-q takes a single pathname only, so ask about them one at a time
      (cd "$REPO_DIR" && git check-ignore -q "$junk_path") || unignored="$unignored $junk_path"
    done
    [ -z "$tracked_junk" ] && [ -z "$unignored" ] \
      && pass "generated logs and caches are ignored and none are tracked" \
      || fail "generated files are tracked ($tracked_junk) or not ignored ($unignored)"
  else
    skip "git is unavailable; cannot check that generated files are ignored"
  fi

  #prefinalize.cmd is stored with LF and gains CRLF when written, so a CRLF copy here would give CRCRLF
  ! grep -qU $'\r' "$REPO_DIR/config-templates/prefinalize.cmd" \
    && grep -qF '*.cmd text eol=lf' "$REPO_DIR/.gitattributes" \
    && ! grep -qF 'config_txt_tips' "$REPO_DIR/.gitattributes" \
    && pass "the batch template is stored with LF, so the CR is added exactly once" \
    || fail "the batch template has CRLF in the repo, or .gitattributes does not pin it"

  #the specialize action runs on the installed OS, where the media may no longer be lettered, so the
  #script it invokes has to be copied into Windows too - the answer file alone is not enough
  ram_hook_dir="$(mktemp -d)"
  mkdir -p "$ram_hook_dir/peinstaller/winpe/2"
  printf 'stub\n' > "$ram_hook_dir/peinstaller/winpe/2/setup.exe"
  ram_hook_out="$(cd "$ram_hook_dir" && env -u CONFIG_TXT NO_UPDATE=1 DIRECTORY="$REPO_DIR" bash -c '
    source "$DIRECTORY/install-wor.sh" source >/dev/null 2>&1
    RPI_MODEL=4; PI4_AUTO_DISABLE_3GB=1; OOBE_NETWORK_BYPASS=0
    configure_pe_prefinalize
    [ -s peinstaller/winpe/2/scripts/Pi4Disable3GB.ps1 ] && echo script-staged
    grep -qF "Setup\\Scripts" peinstaller/winpe/2/scripts/prefinalize.cmd && echo hook-copies-it
    #a model without the 3 GB limit must not carry the action at all
    RPI_MODEL=5
    configure_pe_prefinalize
    [ -e peinstaller/winpe/2/scripts/Pi4Disable3GB.ps1 ] || echo not-staged-for-pi5
  ')"
  rm -rf "$ram_hook_dir"
  for ram_hook_expected in script-staged hook-copies-it not-staged-for-pi5 ;do
    printf '%s\n' "$ram_hook_out" | grep -qx "$ram_hook_expected" || missing_ram_hook="$missing_ram_hook $ram_hook_expected"
  done
  #a RunSynchronousCommand that exits non-zero fails Windows Setup outright, so it must swallow its own errors
  [ -z "$missing_ram_hook" ] \
    && grep -qF 'Setup\Scripts\Pi4Disable3GB.ps1' "$REPO_DIR/config-templates/pi4-ram-unlock-specialize.xml" \
    && grep -qF '; exit 0"' "$REPO_DIR/config-templates/pi4-ram-unlock-specialize.xml" \
    && ! grep -qF 'exit 1' "$REPO_DIR/config-templates/pi4-ram-unlock-specialize.xml" \
    && pass "the Pi 4 RAM unlock reaches the installed OS and cannot fail Windows setup" \
    || fail "the RAM unlock is not delivered to the installed OS:$missing_ram_hook"

  #the answer file is concatenated from fragments, and Windows silently ignores one that is not
  #well-formed - so a bad escape or a missing newline between fragments would fail invisibly
  if command -v python3 >/dev/null ;then
    answer_passes="$(run_in_engine 'RPI_MODEL=4; PI4_AUTO_DISABLE_3GB=1; OOBE_NETWORK_BYPASS=1; unattend_xml' \
      | python3 -c 'import sys,xml.dom.minidom
d = xml.dom.minidom.parseString(sys.stdin.read())
print(",".join(s.getAttribute("pass") for s in d.getElementsByTagName("settings")))' 2>/dev/null)"
    [ "$answer_passes" == 'specialize,oobeSystem' ] \
      && pass "the generated answer file is well-formed XML with both passes" \
      || fail "the generated answer file is malformed or missing a pass: '$answer_passes'"
  else
    skip "python3 is unavailable; cannot check that the answer file is well-formed"
  fi

  #a table of contents that points at a heading which no longer exists is worse than none
  broken_anchors=''
  while read -r anchor ;do
    [ -z "$anchor" ] && continue
    #GitHub slugs: lowercase, punctuation dropped, spaces to hyphens
    grep -qE "^#{2,4} " "$REPO_DIR/README.md" || break
    grep -E "^#{2,4} " "$REPO_DIR/README.md" \
      | sed 's/^#* //; s/!\[[^]]*\]([^)]*)//g; s/`//g' \
      | tr 'A-Z' 'a-z' | sed 's/[^a-z0-9 -]//g; s/^ *//; s/ *$//; s/ /-/g' \
      | grep -qx "$anchor" || broken_anchors="$broken_anchors #$anchor"
  done < <(grep -oE '^\s*- \[[^]]+\]\(#[a-z0-9-]+\)' "$REPO_DIR/README.md" | grep -oE '#[a-z0-9-]+' | tr -d '#')
  [ -z "$broken_anchors" ] \
    && pass "every table-of-contents entry points at a heading that exists" \
    || fail "the table of contents links to headings that do not exist:$broken_anchors"
}

make_disk() { #Input: size, name. Output: loop device path
  local size="$1"
  local name="$2"
  local img="$TEST_DIR/${name}.img"
  mkdir -p "$TEST_DIR"
  truncate -s "$size" "$img" || die "Failed to create $img"
  local dev
  dev="$(sudo losetup -f 2>/dev/null)"
  [ -z "$dev" ] && die "No free loop device is available."
  #containers ship a fixed set of /dev/loop* nodes, so the next free name may not exist yet
  [ ! -e "$dev" ] && sudo mknod "$dev" b 7 "${dev##*loop}" 2>/dev/null
  sudo losetup "$dev" "$img" || die "Failed to attach $img to $dev"
  LOOP_DEVICES+=("$dev")
  echo "$dev"
}

require_tools() {
  sudo -n true 2>/dev/null || die "This harness needs passwordless sudo to create loopback devices."
  command -v losetup >/dev/null || die "losetup is not installed."
  [ -e /dev/loop-control ] || die "No /dev/loop-control, so loopback devices cannot be created here."
}

ensure_non_root_harness() {
  [ "$(id -u)" != 0 ] && return 0
  if [ -z "${WOR_FLASHER_REEXEC_NONROOT:-}" ] && [ -n "${SUDO_USER:-}" ] && [ "$SUDO_USER" != root ] && id "$SUDO_USER" >/dev/null 2>&1 ;then
    info "Re-running test harness as $SUDO_USER"
    exec sudo -E -H -u "$SUDO_USER" env WOR_FLASHER_REEXEC_NONROOT=1 "$0" "$@"
  fi
  die "This harness must run as a non-root user with passwordless sudo."
}

stub_kernel_modules() {
  #containers have no /lib/modules, which install-wor.sh treats as a pending reboot
  local moddir="/lib/modules/$(uname -r)"
  [ ! -d "$moddir" ] && sudo mkdir -p "$moddir"
  return 0
}

seed_winfiles() { #Input: build id. Makes install-wor.sh skip the multi-gigabyte Windows download.
  [ "$SKIP_ESD" == 0 ] && return 0
  mkdir -p "$TEST_DL_DIR/winfiles_${1}_${TEST_WIN_LANG}"
  touch "$TEST_DL_DIR/winfiles_${1}_${TEST_WIN_LANG}/alldone"
}

seed_bootable_winfiles() { #Input: build id. Creates a small, valid WinPE fixture for a real loop-device flash.
  local bid="$1" winfiles fixture_one fixture_two
  winfiles="$TEST_DL_DIR/winfiles_${bid}_${TEST_WIN_LANG}"
  fixture_one="$TEST_DIR/boot-wim-index-one"
  fixture_two="$TEST_DIR/boot-wim-index-two"
  rm -rf "$winfiles" "$fixture_one" "$fixture_two"
  mkdir -p "$winfiles/bootpart/sources" "$winfiles/bootpart/efi/boot" "$winfiles/bootpart/boot" "$fixture_one" "$fixture_two" "$TEST_DL_DIR/peinstaller/winpe/2"
  printf 'first boot image\n' > "$fixture_one/fixture.txt"
  printf 'second boot image\n' > "$fixture_two/fixture.txt"
  printf 'PE installer fixture\n' > "$TEST_DL_DIR/peinstaller/winpe/2/fixture.txt"
  printf 'fallback bootloader fixture\n' > "$winfiles/bootpart/efi/boot/bootaa64.efi"
  wimcapture "$fixture_one" "$winfiles/bootpart/sources/boot.wim" boot-one >/dev/null
  wimappend "$fixture_two" "$winfiles/bootpart/sources/boot.wim" boot-two >/dev/null
  wimcapture "$fixture_one" "$winfiles/install.wim" install >/dev/null
  touch "$winfiles/alldone"
}

run_with_timeout() {
  if command -v timeout >/dev/null ;then
    timeout "$TEST_COMMAND_TIMEOUT" "$@"
  else
    "$@"
  fi
}

run_flasher_with_dry_run() { #Input: dry-run flag, then VAR=VALUE pairs. Sets LAST_OUT and LAST_CODE.
  local dry_run="$1"
  shift
  local output_file
  progress "running install-wor.sh with $(printf '%s ' "$@")"
  progress "live installer output follows (timeout: ${TEST_COMMAND_TIMEOUT}s)"
  output_file="$TEST_DIR/flasher-output.log"
  #stdin is closed so an unexpected prompt (e.g. the root-user confirmation) fails fast instead of hanging
  (cd "$TEST_DIR" && run_with_timeout env ROOT_DEV=/dev/__wor_flasher_test_root__ DL_DIR="$TEST_DL_DIR" WIN_LANG="$TEST_WIN_LANG" RUN_MODE=cli DRY_RUN="$dry_run" SKIP_PACKAGE_INSTALL="${SKIP_PACKAGE_INSTALL:-0}" "$@" "$REPO_DIR/install-wor.sh" </dev/null) 2>&1 | tee "$output_file" 1>&2
  LAST_CODE="${PIPESTATUS[0]}"
  LAST_OUT="$(<"$output_file")"
  progress "install-wor.sh finished with exit $LAST_CODE"
}

run_flasher() { #Input: VAR=VALUE pairs. Sets LAST_OUT and LAST_CODE.
  run_flasher_with_dry_run 1 "$@"
}

run_flasher_real() { #Input: VAR=VALUE pairs. Sets LAST_OUT and LAST_CODE.
  run_flasher_with_dry_run 0 "$@"
}

run_flasher_interrupted() { #Input: seconds to wait before SIGINT, then VAR=VALUE pairs. Sets LAST_OUT and LAST_CODE.
  local delay="$1" output_file
  shift
  progress "running install-wor.sh with $(printf '%s ' "$@"), sending SIGINT after ${delay}s"
  output_file="$TEST_DIR/flasher-output.log"
  (cd "$TEST_DIR" && timeout --signal=INT --kill-after=10 "$delay" env ROOT_DEV=/dev/__wor_flasher_test_root__ DL_DIR="$TEST_DL_DIR" WIN_LANG="$TEST_WIN_LANG" RUN_MODE=cli DRY_RUN=0 SKIP_PACKAGE_INSTALL="${SKIP_PACKAGE_INSTALL:-0}" "$@" "$REPO_DIR/install-wor.sh" </dev/null) 2>&1 | tee "$output_file" 1>&2
  LAST_CODE="${PIPESTATUS[0]}"
  LAST_OUT="$(<"$output_file")"
  progress "install-wor.sh finished with exit $LAST_CODE"
}

run_flasher_with_input() { #Input: stdin text, then VAR=VALUE pairs. Sets LAST_OUT and LAST_CODE.
  local input="$1" output_file
  shift
  progress "running install-wor.sh with stdin and $(printf '%s ' "$@")"
  progress "live installer output follows (timeout: ${TEST_COMMAND_TIMEOUT}s)"
  output_file="$TEST_DIR/flasher-output.log"
  printf '%b' "$input" | (cd "$TEST_DIR" && run_with_timeout env ROOT_DEV=/dev/__wor_flasher_test_root__ DL_DIR="$TEST_DL_DIR" WIN_LANG="$TEST_WIN_LANG" RUN_MODE=cli DRY_RUN=1 SKIP_PACKAGE_INSTALL="${SKIP_PACKAGE_INSTALL:-0}" "$@" "$REPO_DIR/install-wor.sh") 2>&1 | tee "$output_file" 1>&2
  LAST_CODE="${PIPESTATUS[1]}"
  LAST_OUT="$(<"$output_file")"
  progress "install-wor.sh finished with exit $LAST_CODE"
}

show_last_out() {
  printf '    last output:\n' 1>&2
  sed 's/^/      /' <<<"$LAST_OUT" | tail -n 40 1>&2
}

expect_ok() {
  if [ "$LAST_CODE" == 0 ];then
    pass "$1"
  else
    fail "$1 (exit $LAST_CODE)"
    show_last_out
  fi
}

expect_fail() {
  if [ "$LAST_CODE" != 0 ];then
    pass "$1"
  else
    fail "$1 (unexpectedly succeeded)"
    show_last_out
  fi
}

expect_output() {
  if grep -qF "$2" <<<"$LAST_OUT" ;then
    pass "$1"
  else
    fail "$1 (did not find: $2)"
    show_last_out
  fi
}

expect_no_output() {
  if grep -qF "$2" <<<"$LAST_OUT" ;then
    fail "$1 (unexpectedly found: $2)"
    show_last_out
  else
    pass "$1"
  fi
}

######## Argument parsing

for arg in "$@" ;do
  case "$arg" in
    --walkthrough) MODE=walkthrough ;;
    --gui) MODE=gui ;;
    --clean) MODE=clean ;;
    --full) SKIP_ESD=0 ;;
    --keep) KEEP=1 ;;
    -h|--help) sed -n '3,12p' "$0" | sed 's/^#//' ; exit 0 ;;
    *) die "Unknown option: $arg" ;;
  esac
done

if [ "$MODE" == clean ];then
  detach_all
  rm -rf "$TEST_DIR" "$REPO_DIR/cache"
  info "Removed $TEST_DIR and detached its loop devices."
  exit 0
fi

if [ "$(uname -s)" == Linux ];then
  ensure_non_root_harness "$@"
fi

static_checks

info "== WoR-Flasher test suite =="

if [ "$(uname -s)" == Darwin ] && [ "$MODE" == gui ];then
  command -v osascript >/dev/null || die "The macOS GUI walkthrough needs osascript."
  info "DRY_RUN is set, so the selected removable drive will not be modified."
  DL_DIR="$TEST_DL_DIR" DRY_RUN=1 USE_CACHE=0 "$REPO_DIR/install-wor-gui.sh"
  exit $?
fi

if [ "$(uname -s)" != Linux ];then
  if [ "$MODE" == suite ] && [ -z "${WOR_FLASHER_CONTAINER_TEST:-}" ] && [ -x "$REPO_DIR/tests/run-linux-integration.sh" ];then
    if command -v docker >/dev/null && docker info >/dev/null 2>&1 ;then
      info "== Linux integration via Docker =="
      progress "starting Docker integration; dependency setup can take several minutes"
      "$REPO_DIR/tests/run-linux-integration.sh" "$@"
      exit $?
    fi
    skip "integration tests require Linux loop devices; Docker is unavailable"
  else
    skip "integration tests require Linux loop devices"
  fi
  summary
fi

trap cleanup EXIT
require_tools
stub_kernel_modules
mkdir -p "$TEST_DL_DIR"

######## Fake drives, one per tier drive_capability() recognises

info "== Fake drives =="
DEV_INSTALL="$(make_disk "$SIZE_INSTALL" install)"
DEV_RECOVERY="$(make_disk "$SIZE_RECOVERY" recovery)"
DEV_SMALL="$(make_disk "$SIZE_TOO_SMALL" toosmall)"
echo "  $DEV_INSTALL  $SIZE_INSTALL (install tier)"
echo "  $DEV_RECOVERY  $SIZE_RECOVERY (recovery tier)"
echo "  $DEV_SMALL  $SIZE_TOO_SMALL (must be refused)"

######## Interactive modes

if [ "$MODE" == walkthrough ] || [ "$MODE" == gui ];then
  echo
  info "DRY_RUN is set, so nothing will be written even if you pick a drive."
  echo
  if [ "$MODE" == walkthrough ];then
    DL_DIR="$TEST_DL_DIR" DRY_RUN=1 USE_CACHE=0 "$REPO_DIR/install-wor.sh"
  else
    [ -z "$DISPLAY" ] && [ -z "$WAYLAND_DISPLAY" ] && die "The GUI needs a display. Run this on a desktop session."
    DL_DIR="$TEST_DL_DIR" DRY_RUN=1 USE_CACHE=0 "$REPO_DIR/install-wor-gui.sh"
  fi
  exit $?
fi

######## Automated suite

info "== Library functions =="
#install-wor.sh derives DIRECTORY from $0, which is this harness, so point it at the repo first
DIRECTORY="$REPO_DIR"
#shellcheck disable=SC1090
source "$REPO_DIR/install-wor.sh" source >/dev/null 2>&1

#configure_pe_settings_ini edits cached payload, so it has to re-record the manifest or every run re-downloads the PE installer
pe_cache_dir="$(mktemp -d)"
mkdir -p "$pe_cache_dir/peinstaller/winpe/2"
printf '[WoR Configuration File]\n' > "$pe_cache_dir/peinstaller/winpe/2/settings.ini"
printf 'payload\n' > "$pe_cache_dir/peinstaller/winpe/2/other.bin"
(
  cd "$pe_cache_dir" || exit 1
  #both are read by the sourced install-wor.sh helpers below
  #shellcheck disable=SC2034
  USE_CACHE=1
  #shellcheck disable=SC2034
  HIDE_EMPTY_DRIVES=1
  mark_cache "$pe_cache_dir/peinstaller" PE_TOKEN >/dev/null 2>&1 || exit 1
  configure_pe_settings_ini || exit 1
  grep -q '^HideEmptyDrives=1$' "$pe_cache_dir/peinstaller/winpe/2/settings.ini" || exit 1
  cache_is_current "$pe_cache_dir/peinstaller" PE_TOKEN || exit 1
  #a genuinely altered payload must still be rejected
  printf 'tampered\n' > "$pe_cache_dir/peinstaller/winpe/2/other.bin"
  cache_is_current "$pe_cache_dir/peinstaller" PE_TOKEN && exit 1
  exit 0
) && pass "editing the WoR-PE settings keeps the cache valid while tampering is still detected" \
  || fail "configure_pe_settings_ini invalidates the PE installer cache, forcing a re-download every run"
rm -rf "$pe_cache_dir"

progress_test_dir="$(mktemp -d)"
printf 'progress helper fixture\n' > "$progress_test_dir/source"
original_sudo_function="$(declare -f sudo)"
sudo() { "$@"; }
copy_file_with_progress test-copy "$progress_test_dir/source" "$progress_test_dir/destination" 2>/dev/null \
  && [ "$(sha256_file "$progress_test_dir/source")" == "$(sha256_file_with_progress test-hash "$progress_test_dir/destination" 2>/dev/null)" ] \
  && pass "copy and checksum progress helpers preserve file contents" \
  || fail "copy or checksum progress helper corrupted its output"
eval "$original_sudo_function"
rm -rf "$progress_test_dir"

for pair in '19045.1234:Windows 10' '22631.2861:Windows 11' ;do
  bid="${pair%%:*}"; want="${pair#*:}"
  [[ "$(get_os_name "$bid")" == "$want"* ]] && pass "get_os_name $bid -> $want" || fail "get_os_name $bid"
done

[ "$(drive_capability "$DEV_INSTALL")" == install ] && pass "drive_capability $SIZE_INSTALL -> install" || fail "drive_capability $SIZE_INSTALL"
[ "$(drive_capability "$DEV_RECOVERY")" == recovery ] && pass "drive_capability $SIZE_RECOVERY -> recovery" || fail "drive_capability $SIZE_RECOVERY"
[ "$(drive_capability "$DEV_SMALL")" == too-small ] && pass "drive_capability $SIZE_TOO_SMALL -> too-small" || fail "drive_capability $SIZE_TOO_SMALL"

if command -v jq >/dev/null ;then
  original_darwin_plist_json="$(declare -f darwin_plist_json)"
  test_host_os="$HOST_OS"
  test_root_dev="$ROOT_DEV"
  HOST_OS=Darwin
  ROOT_DEV=/dev/disk0
  DARWIN_DEVICE_INFO='{"WholeDisk":true,"Internal":false,"VirtualOrPhysical":"Physical","ReadOnlyMedia":false,"DiskSize":64000000000,"MediaName":"USB Drive"}'
  darwin_plist_json() {
    if [ "$2" == list ] && [ "$4" == /dev/disk2 ];then
      printf '%s\n' '{"AllDisksAndPartitions":[{"Partitions":[{"VolumeName":"WOR_BOOT","DeviceIdentifier":"disk2s1"},{"VolumeName":"WOR_INSTALL","DeviceIdentifier":"disk2s2"}]}]}'
    elif [ "$2" == list ];then
      printf '%s\n' '{"AllDisks":["disk2"]}'
    else
      printf '%s\n' "$DARWIN_DEVICE_INFO"
    fi
  }
  is_safe_target_device /dev/disk2 && pass "Darwin accepts an external physical writable disk" || fail "Darwin rejected an external physical writable disk"
  is_safe_target_device /dev/disk0 && fail "Darwin accepted the startup disk" || pass "Darwin rejects the startup disk"
  [ "$(darwin_list_device_paths)" == /dev/disk2 ] && pass "Darwin GUI lists safe external disks" || fail "Darwin GUI listed an unexpected disk"
  [ "$(darwin_list_device_choices)" == $'/dev/disk2\t59.6 GB   USB Drive   Labels: WOR_BOOT, WOR_INSTALL   Volumes: WOR_BOOT, WOR_INSTALL' ] \
    && pass "Darwin GUI lists detected volumes" || fail "Darwin GUI did not show detected volume details"
  [ "$(darwin_partition_by_volume_name /dev/disk2 WOR_BOOT)" == /dev/disk2s1 ] \
    && [ "$(darwin_partition_by_volume_name /dev/disk2 WOR_INSTALL)" == /dev/disk2s2 ] \
    && pass "Darwin resolves formatted partitions by volume name" \
    || fail "Darwin used fixed partition numbers instead of volume names"
  grep -qF 'sgdisk_bin="$(command -v sgdisk)"' "$REPO_DIR/install-wor.sh" \
    && grep -qF -- '-n "1:0:+${boot_size_mb}M" -t 1:ef00 -c 1:WOR_BOOT' "$REPO_DIR/install-wor.sh" \
    && grep -qF '[ "$CAN_INSTALL_ON_SAME_DRIVE" == 1 ] && install_size_mb=18000 || install_size_mb=6000' "$REPO_DIR/install-wor.sh" \
    && grep -qF -- '-n "2:0:+${install_size_mb}M" -t 2:0700 -c 2:WOR_INSTALL' "$REPO_DIR/install-wor.sh" \
    && ! grep -qF -- '-n 2:0:0' "$REPO_DIR/install-wor.sh" \
    && grep -qF -- '-A 1:set:63 -A 2:set:63' "$REPO_DIR/install-wor.sh" \
    && grep -qF -- '-A 1:clear:63 -A 2:clear:63' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'boot_size_mb=$((boot_payload_kb / 1024 + 512))' "$REPO_DIR/install-wor.sh" \
    && ! grep -qF 'diskutil partitionDisk' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'Failed to unmount newly created partitions on $DEVICE."' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'raw_part2="/dev/r${PART2#/dev/}"' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'newfs_exfat -R -v WOR_INSTALL "$raw_part2"' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'less than 1 GiB remains unallocated for the Windows target partition' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'verify_written_image "$DEVICE" "$PART1" "$PART2" "$boot_mount" "$win_mount"' "$REPO_DIR/install-wor.sh" \
    && pass "Darwin creates WOR_BOOT as a real EFI System Partition" \
    || fail "Darwin does not type WOR_BOOT as an EFI System Partition"
  DARWIN_DEVICE_INFO='{"WholeDisk":true,"Internal":false,"VirtualOrPhysical":"Physical","WritableMedia":true,"TotalSize":64000000000,"MediaName":"USB Drive"}'
  is_safe_target_device /dev/disk2 && pass "Darwin accepts current writable-media metadata" || fail "Darwin rejected current writable-media metadata"
  for safety_case in \
    'internal:{"WholeDisk":true,"Internal":true,"VirtualOrPhysical":"Physical","ReadOnlyMedia":false}' \
    'virtual:{"WholeDisk":true,"Internal":false,"VirtualOrPhysical":"Virtual","ReadOnlyMedia":false}' \
    'read-only:{"WholeDisk":true,"Internal":false,"VirtualOrPhysical":"Physical","ReadOnlyMedia":true}' \
    'partition:{"WholeDisk":false,"Internal":false,"VirtualOrPhysical":"Physical","ReadOnlyMedia":false}'; do
    case_name="${safety_case%%:*}"
    DARWIN_DEVICE_INFO="${safety_case#*:}"
    is_safe_target_device /dev/disk2 && fail "Darwin accepted a $case_name disk" || pass "Darwin rejects a $case_name disk"
  done
  eval "$original_darwin_plist_json"
  unset DARWIN_DEVICE_INFO
  HOST_OS="$test_host_os"
  ROOT_DEV="$test_root_dev"
else
  skip "Darwin disk safety test needs jq"
fi

info "== Detecting builds from the catalog =="
#the newest build an ARMv8.0 Pi can boot, and the newest build overall
GOOD_BID="$(RPI_MODEL=4 get_bid 11)"
NEWEST_BID="$(RPI_MODEL=5 get_bid 11)"
[ ! -z "$GOOD_BID" ] && pass "newest build for a Pi 4: $GOOD_BID" || fail "could not detect a build for a Pi 4"
[ ! -z "$NEWEST_BID" ] && pass "newest build for a Pi 5: $NEWEST_BID" || fail "could not detect a build for a Pi 5"
[ -z "$GOOD_BID" ] && die "Cannot continue without a build id. Is worproject.com reachable?"

RPI_MODEL=4 cpu_supports_bid "$GOOD_BID" && pass "cpu_supports_bid allows $GOOD_BID on a Pi 4" || fail "cpu_supports_bid rejected $GOOD_BID on a Pi 4"
unset RPI_MODEL

catalog_fixture=$'<LanguageCode>en-gb</LanguageCode>\n<FilePath>https://example.com/en-gb.esd</FilePath>\n<Size>1</Size>\n</File>\n<LanguageCode>en-us</LanguageCode>\n<FilePath>https://example.com/en-us.esd</FilePath>\n<Size>2</Size>\n</File>\n<Languages>'
catalog_entry="$(get_esd_catalog_entry "$catalog_fixture" en-us)"
grep -qF 'https://example.com/en-us.esd' <<<"$catalog_entry" \
  && ! grep -qF 'https://example.com/en-gb.esd' <<<"$catalog_entry" \
  && pass "ESD catalog parser selects the requested language" \
  || fail "ESD catalog parser selected the wrong language"

######## Every model gets a fresh, uncached run

#sourcing install-wor.sh sets IFS to a newline, so split the model list explicitly
for model in $(tr ' ' '\n' <<<"$TEST_MODELS") ;do
  info "== Raspberry Pi $model, fresh run =="
  bid="$(RPI_MODEL=$model get_bid 11)"
  if [ -z "$bid" ];then
    skip "no build id detected for a Pi $model"
    continue
  fi
  echo "  using build $bid"
  seed_winfiles "$bid"

  run_flasher TERM=unknown BID="$bid" RPI_MODEL="$model" DEVICE="$DEV_INSTALL" CAN_INSTALL_ON_SAME_DRIVE=1 USE_CACHE=0
  expect_ok "Pi $model dry run completes"
  expect_output "Pi $model downloads the PE installer" "Downloading WoR PE-based installer"
  expect_output "Pi $model downloads UEFI firmware" "UEFI firmware"
  expect_output "Pi $model stops before flashing" "DRY_RUN"
  expect_no_output "Pi $model suppresses unknown terminal warnings" "unknown terminal"

  if [ "$model" == 5 ];then
    expect_no_output "Pi 5 skips the ARM64 drivers" "Downloading ARM64 drivers"
  else
    expect_output "Pi $model downloads the ARM64 drivers" "Downloading ARM64 drivers"
  fi

  for folder in peinstaller "pi${model}-uefipackage" ;do
    [ -f "$TEST_DL_DIR/$folder/.wor-flasher-version" ] && pass "Pi $model stamped $folder" || fail "Pi $model left $folder unstamped"
    [ -s "$TEST_DL_DIR/$folder/.wor-flasher-sha256" ] && pass "Pi $model checksummed $folder" || fail "Pi $model left $folder without an integrity manifest"
  done
done

######## Cache modes, which need a populated cache to test against

info "== Cache modes =="
run_flasher BID="$GOOD_BID" RPI_MODEL=4 DEVICE="$DEV_INSTALL" CAN_INSTALL_ON_SAME_DRIVE=1 USE_CACHE=0
run_flasher BID="$GOOD_BID" RPI_MODEL=4 DEVICE="$DEV_INSTALL" CAN_INSTALL_ON_SAME_DRIVE=1 USE_CACHE=1
expect_output "USE_CACHE=1 reuses a current cache" "cached copy is up to date"
printf 'tampered\n' >> "$TEST_DL_DIR/pi4-uefipackage/RPI_EFI.fd"
run_flasher BID="$GOOD_BID" RPI_MODEL=4 DEVICE="$DEV_INSTALL" CAN_INSTALL_ON_SAME_DRIVE=1 USE_CACHE=1
expect_output "USE_CACHE=1 refreshes modified cached content" "Downloading Pi4 UEFI firmware"
info "== Linux boot partition layout =="
seed_bootable_winfiles "$GOOD_BID"
run_flasher_real BID="$GOOD_BID" RPI_MODEL=4 DEVICE="$DEV_INSTALL" CAN_INSTALL_ON_SAME_DRIVE=1 USE_CACHE=2
expect_ok "Pi 4 real loop-device flash completes"
expect_output "Pi 4 verifies the written image" "Written image verified successfully"

boot_partition="$(get_partition "$DEV_INSTALL" 1)"
partition_count="$(parted -ms "$DEV_INSTALL" unit s print | awk -F: '$1 ~ /^[0-9]+$/ { count++ } END { print count + 0 }')"
boot_filesystem="$(parted -ms "$DEV_INSTALL" unit s print | awk -F: '$1 == 1 { print $5 }')"
boot_mount="$(mktemp -d)"
if sudo mount "$boot_partition" "$boot_mount"; then
  [ "$partition_count" == 2 ] \
    && pass "Pi 4 flash creates exactly two partitions" \
    || fail "Pi 4 flash creates an unexpected partition layout"
  [ "$boot_filesystem" == fat32 ] \
    && pass "Pi 4 boot partition is FAT32" \
    || fail "Pi 4 first partition is not FAT32"
  sudo test -f "$boot_mount/EFI/BOOT/BOOTAA64.EFI" \
    && pass "Pi 4 boot loader is on the first FAT partition" \
    || fail "Pi 4 first partition lacks the UEFI fallback boot loader"
  sudo umount "$boot_mount"
else
  fail "Pi 4 boot partition could not be mounted for verification"
fi
rmdir "$boot_mount"

info "== Interruption and authorization failures =="

fake_sudo_dir="$(mktemp -d)"
cat > "$fake_sudo_dir/sudo" <<'FAKESUDO'
#!/bin/bash
echo "sudo: a password is required" 1>&2
exit 1
FAKESUDO
chmod +x "$fake_sudo_dir/sudo"

run_flasher_real PATH="$fake_sudo_dir:$PATH" BID="$GOOD_BID" RPI_MODEL=4 DEVICE="$DEV_INSTALL" CAN_INSTALL_ON_SAME_DRIVE=1 USE_CACHE=2
expect_fail "flashing stops immediately when sudo/authorization fails"
expect_output "the failure names the affected step" "Failed to make GPT partition table"
expect_no_output "nothing continues past an authorization failure" "Generating partitions"
expect_no_output "nothing continues past an authorization failure" "Copying files"
expect_no_output "nothing continues past an authorization failure" "script has completed"
rm -rf "$fake_sudo_dir"

hanging_sudo_dir="$(mktemp -d)"
cat > "$hanging_sudo_dir/sudo" <<'HANGINGSUDO'
#!/bin/bash
#simulates being stuck at an interactive password prompt, like the real sudo would be
echo "Password:"
sleep 30
HANGINGSUDO
chmod +x "$hanging_sudo_dir/sudo"

run_flasher_interrupted 3 PATH="$hanging_sudo_dir:$PATH" BID="$GOOD_BID" RPI_MODEL=4 DEVICE="$DEV_INSTALL" CAN_INSTALL_ON_SAME_DRIVE=1 USE_CACHE=2
expect_fail "Ctrl+C while stuck at the password prompt stops the flasher"
expect_output "Ctrl+C shows a clear interrupted message" "Interrupted"
expect_no_output "Ctrl+C does not let the flash continue in the background" "script has completed"
rm -rf "$hanging_sudo_dir"

expect_no_output "USE_CACHE=1 downloads nothing" "Downloading ARM64 drivers"

echo 'https://example.com/stale.zip' > "$TEST_DL_DIR/driverpackage/.wor-flasher-version"
run_flasher BID="$GOOD_BID" RPI_MODEL=4 DEVICE="$DEV_INSTALL" CAN_INSTALL_ON_SAME_DRIVE=1 USE_CACHE=1
expect_output "USE_CACHE=1 refreshes a stale component" "Downloading ARM64 drivers"
expect_output "USE_CACHE=1 keeps the current ones" "peinstaller - cached copy is up to date"

run_flasher BID="$GOOD_BID" RPI_MODEL=4 DEVICE="$DEV_INSTALL" CAN_INSTALL_ON_SAME_DRIVE=1 USE_CACHE=2
expect_output "USE_CACHE=2 skips update checks" "without checking for updates"

run_flasher BID="$GOOD_BID" RPI_MODEL=4 DEVICE="$DEV_INSTALL" CAN_INSTALL_ON_SAME_DRIVE=1 USE_CACHE=9
expect_fail "USE_CACHE=9 is rejected"

info "== Guards =="
if [ "$NEWEST_BID" == "$GOOD_BID" ];then
  skip "no ARMv8.1-only build is currently listed, so the CPU guard cannot be exercised"
else
  run_flasher BID="$NEWEST_BID" RPI_MODEL=4 DEVICE="$DEV_INSTALL" CAN_INSTALL_ON_SAME_DRIVE=1 USE_CACHE=2
  expect_fail "$NEWEST_BID is refused on a Pi 4"
  expect_output "the refusal explains why" "ARMv8.1"

  seed_winfiles "$NEWEST_BID"
  run_flasher BID="$NEWEST_BID" RPI_MODEL=5 DEVICE="$DEV_INSTALL" CAN_INSTALL_ON_SAME_DRIVE=1 USE_CACHE=2
  expect_ok "$NEWEST_BID is allowed on a Pi 5"
fi

run_flasher BID="$GOOD_BID" RPI_MODEL=4 DEVICE="$DEV_SMALL" CAN_INSTALL_ON_SAME_DRIVE=0 USE_CACHE=2
expect_fail "a drive under 8GB is refused"

run_flasher BID="$GOOD_BID" RPI_MODEL=4 DEVICE="$DEV_RECOVERY" CAN_INSTALL_ON_SAME_DRIVE=1 USE_CACHE=2
expect_fail "self-install is refused on a recovery-sized drive"

run_flasher_with_input '' BID="$GOOD_BID" RPI_MODEL=4 DEVICE="$DEV_RECOVERY" USE_CACHE=2
expect_ok "recovery-sized drive automatically uses recovery mode"
expect_output "automatic recovery mode is recorded" "CAN_INSTALL_ON_SAME_DRIVE: 0"
expect_no_output "recovery-sized drive does not ask for install mode" "Choose the installation mode"

run_flasher_with_input '2\n' BID="$GOOD_BID" RPI_MODEL=4 DEVICE="$DEV_INSTALL" USE_CACHE=2
expect_ok "large drive can be used as a recovery drive"
expect_output "large-drive recovery choice is recorded" "CAN_INSTALL_ON_SAME_DRIVE: 0"

run_flasher BID="$GOOD_BID" RPI_MODEL=9 DEVICE="$DEV_INSTALL" CAN_INSTALL_ON_SAME_DRIVE=1 USE_CACHE=2
expect_fail "an unknown RPI_MODEL is rejected"

info "== GUI handoff =="
#Regression test: lxterminal and gnome-terminal reuse an existing process, so the launched
#terminal does not inherit exported variables. env -i reproduces that.
injection_marker="$TEST_DIR/CONFIG_TXT_INJECTED"
CONFIG_TXT="arm_64bit=1
# a \"quoted\" line with \$(touch \"$injection_marker\")
armstub=RPI_EFI.fd"
cli_script="$REPO_DIR/install-wor.sh"
env_file="$(mktemp)"
runner_file="$(mktemp)"
# declare -p emits shell-escaped declarations; sourcing that generated file is the
# behavior under test, so SC2090's warning about indirect command expansion is a false positive.
#shellcheck disable=SC2090
declare -p CONFIG_TXT cli_script > "$env_file"
printf 'source ' > "$runner_file"
printf '%q' "$env_file" >> "$runner_file"
printf '\n' >> "$runner_file"
printf '%s\n' 'printf "%s\n" "$cli_script"' >> "$runner_file"
printf '%s\n' 'printf "%s\n" "$CONFIG_TXT" | wc -l' >> "$runner_file"
handoff_out="$(env -i /bin/bash "$runner_file")"
rm -f "$env_file" "$runner_file"
unset CONFIG_TXT cli_script injection_marker
[ "$(head -n1 <<<"$handoff_out")" == "$REPO_DIR/install-wor.sh" ] \
  && pass "values survive a terminal that does not inherit the environment" \
  || fail "values were lost in a terminal that does not inherit the environment"
[ "$(tail -n1 <<<"$handoff_out")" == 3 ] && pass "a multi-line CONFIG_TXT stays intact" || fail "CONFIG_TXT was mangled"
if [ -e "$TEST_DIR/CONFIG_TXT_INJECTED" ];then
  fail "CONFIG_TXT was executed as code"
else
  pass "CONFIG_TXT is not executed as code"
fi

info "== Self-updater =="
if ! command -v git >/dev/null || ! git -C "$REPO_DIR" rev-parse --git-dir >/dev/null 2>&1 ;then
  skip "self-updater tests need a git checkout"
else
  branch="$(git -C "$REPO_DIR" rev-parse --abbrev-ref HEAD)"
  if [ "$branch" == HEAD ];then
    skip "self-updater tests need a checked-out branch, not a detached HEAD"
  else
    #a disposable clone of the branch actually being worked on - never runs against $REPO_DIR itself
    clone_dir="$TEST_DIR/self-update-clone"
    rm -rf "$clone_dir"
    if ! git clone -q --branch "$branch" "$REPO_DIR" "$clone_dir" 2>/dev/null ;then
      fail "could not clone $REPO_DIR (branch $branch) to test the self-updater"
    else
      #point the clone's self-updater at this same repo/branch, so it dynamically pulls whatever is actually being worked on
      update_env=(UPDATE_REPO_URL="$REPO_DIR" UPDATE_REF="$branch" NO_UPDATE=0 ROOT_DEV=/dev/__wor_flasher_test_root__ DL_DIR="$TEST_DL_DIR" WIN_LANG="$TEST_WIN_LANG" RUN_MODE=cli DRY_RUN=1 BID="$GOOD_BID" RPI_MODEL=4 DEVICE="$DEV_INSTALL" CAN_INSTALL_ON_SAME_DRIVE=1 USE_CACHE=2)

      progress "self-updater: checking clean clone already on $branch"
      LAST_OUT="$(run_with_timeout env "${update_env[@]}" "$clone_dir/install-wor.sh" 2>&1)"
      LAST_CODE=$?
      progress "self-updater clean-clone check finished with exit $LAST_CODE"
      expect_ok "already level with $branch: flasher still runs"
      expect_no_output "already level with $branch: self-updater stays quiet" "Auto-updating wor-flasher"

      #fall one commit behind $REPO_DIR's $branch, so the clone actually has something to pull
      if git -C "$clone_dir" reset -q --hard HEAD~1 2>/dev/null ;then
        printf '\n' >> "$clone_dir/README.md"
        progress "self-updater: checking dirty clone protection"
        LAST_OUT="$(run_with_timeout env "${update_env[@]}" "$clone_dir/install-wor.sh" 2>&1)"
        LAST_CODE=$?
        progress "self-updater dirty-clone check finished with exit $LAST_CODE"
        expect_ok "dirty behind $branch: flasher still runs"
        expect_output "dirty behind $branch: self-updater preserves local changes" "Skipping automatic update because this checkout has uncommitted changes"
        git -C "$clone_dir" diff --quiet \
          && fail "dirty behind $branch: self-updater discarded local changes" \
          || pass "dirty behind $branch: local changes were preserved"
        git -C "$clone_dir" reset -q --hard HEAD

        progress "self-updater: checking behind clone fast-forward and reload"
        LAST_OUT="$(run_with_timeout env "${update_env[@]}" "$clone_dir/install-wor.sh" 2>&1)"
        LAST_CODE=$?
        progress "self-updater behind-clone check finished with exit $LAST_CODE"
        expect_ok "behind $branch: flasher still runs after update"
        expect_output "behind $branch: self-updater detects the update" "Auto-updating wor-flasher"
        expect_output "behind $branch: self-updater pulls and reloads" "Reloading script"
        [ "$(git -C "$clone_dir" rev-parse HEAD)" == "$(git -C "$REPO_DIR" rev-parse "$branch")" ] \
          && pass "self-updater fast-forwarded the clone to $branch" \
          || fail "self-updater did not fast-forward the clone to $branch"
      else
        skip "$branch has no earlier commit to test dirty update protection"
        skip "$branch has no earlier commit to fall behind"
      fi
    fi
  fi
fi

info "== terminal-run =="
if [ -z "$DISPLAY" ] && [ -z "$WAYLAND_DISPLAY" ];then
  skip "terminal-run needs a display"
elif ! command -v lxterminal >/dev/null && ! command -v xfce4-terminal >/dev/null && ! command -v mate-terminal >/dev/null && ! command -v xterm >/dev/null && ! command -v konsole >/dev/null && ! command -v terminator >/dev/null && ! command -v gnome-terminal >/dev/null && ! command -v qterminal >/dev/null && ! command -v x-terminal-emulator >/dev/null ;then
  skip "terminal-run needs a terminal emulator"
else
  DEBUG=1 timeout 30 "$REPO_DIR/terminal-run" 'true' 'wor-flasher test' >/dev/null 2>&1 \
    && pass "terminal-run launches a terminal" || fail "terminal-run could not launch a terminal"
fi

######## Summary

echo
summary
