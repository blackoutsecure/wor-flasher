#!/bin/bash

#WoR-Flasher - install Windows 10/11 on a Raspberry Pi from Linux or macOS.
#Originally written by Botspot: https://github.com/Botspot/wor-flasher
#Automates this tutorial: https://worproject.com/guides/how-to-install/from-other-os
#
#This is the Blackout Secure fork: https://github.com/blackoutsecure/wor-flasher
#Upstream has never published a git tag or a GitHub release, so this fork keeps its own
#version line. WOR_FLASHER_VERSION below is the single source of truth for it.
#
#Version history
#---------------
#1.0.0 - First versioned release of this fork.
#        macOS host support: diskutil/hdiutil drive discovery, sgdisk GPT partitioning that keeps
#          WOR_BOOT as partition 1 (an extra ESP made the Pi 4 fall back to PXE boot), and a native
#          AppKit/JXA wizard, progress window, Advanced Options window and error dialogs.
#        No visible terminal in GUI mode: install-wor.sh reports progress over WOR_GUI_PROGRESS_FILE
#          and both front-ends render it (AppKit on macOS, yad on Linux). Administrator access is
#          requested through a native password dialog on both platforms.
#        Post-flash verification of partitions, boot files and checksums (SKIP_IMAGE_VERIFICATION).
#        Offline Windows OOBE via a shipped Autounattend.xml (OOBE_NETWORK_BYPASS, default on).
#        Automatic Pi 4 3 GB RAM unlock after the WoR-PE reboot (PI4_AUTO_DISABLE_3GB).
#        Pinned, overridable UEFI firmware and driver versions; Pi 4 stays on UEFI v1.51 because
#          v1.52 does not boot reliably from microSD.
#        Cache modes with SHA-256 payload manifests (USE_CACHE), free-space preflight, and
#          HideEmptyDrives written into the cached WoR-PE settings.ini.
#        Editable config.txt sourced from config-templates/, applied by the CLI and the GUI alike
#          (APPLY_CUSTOM_CONFIG_TXT).
#        One engine, two front-ends: install-wor-gui.sh sources this script and adds only windows.
#        Explicit entry point '--gui'; the front-end is never chosen by sniffing DISPLAY.
#        Test suite (tests/run-tests.sh) plus ShellCheck, macOS and Linux dry-run CI.
#0.x   - Upstream Botspot releases, never tagged. Highlights, oldest first: initial WoR automation,
#        self-updater, "next steps" window, complete rewrite to use ESD releases, download-to-RAM
#        support, Pi 5 support, GitHub API fallback for UEFI firmware, empty block devices filtered
#        out of the drive list, and SHA-256 hashed ESD image handling.

#Single source of truth for this fork's version. Reported by '--version'.
WOR_FLASHER_VERSION='1.0.0'

#shared app title, used for window titles and dialog titles across this file and install-wor-gui.sh
: "${WOR_APP_TITLE:=Windows on Raspberry}"

CLEANUP_MOUNTS=()
CLEANUP_DEVICES=()
CLEANUP_FILES=()

HOST_OS="$(uname -s)"

is_macos() {
  [ "$HOST_OS" == Darwin ]
}

sudo() { #On the GUI, show a native password dialog instead of blocking a hidden/absent terminal.
  if is_macos && [ "$RUN_MODE" == gui ];then
    if [ -z "$MACOS_ASKPASS" ] && MACOS_ASKPASS="$(mktemp)" && chmod +x "$MACOS_ASKPASS";then
      cat > "$MACOS_ASKPASS" <<'ASKPASS'
#!/bin/bash
osascript - "$WOR_FLASH_TARGET" <<'APPLESCRIPT'
on run argv
  set targetDevice to item 1 of argv
  set promptText to "WoR-Flasher needs administrator access to flash the drive." & return & return & "Formatting " & targetDevice & return & return & "There is no turning back now."
  set dialogResult to display dialog promptText default answer "" with hidden answer with title "Windows on Raspberry" with icon caution
  return text returned of dialogResult
end run
APPLESCRIPT
ASKPASS
      register_file_cleanup "$MACOS_ASKPASS"
    fi
    if [ -n "$MACOS_ASKPASS" ];then
      WOR_FLASH_TARGET="$DEVICE" SUDO_ASKPASS="$MACOS_ASKPASS" command sudo -A "$@"
      return
    fi
  elif ! is_macos && [ "$RUN_MODE" == gui ];then
    if [ -z "$LINUX_ASKPASS" ] && LINUX_ASKPASS="$(mktemp)" && chmod +x "$LINUX_ASKPASS";then
      cat > "$LINUX_ASKPASS" <<'ASKPASS'
#!/bin/bash
prompt="WoR-Flasher needs administrator access to flash the drive.\n\nFormatting $WOR_FLASH_TARGET\n\nThere is no turning back now."
if command -v yad >/dev/null ;then
  yad --center --window-icon="$WOR_ICON_PATH" --title="$WOR_APP_TITLE" --entry --hide-text --text="$prompt"
elif command -v zenity >/dev/null ;then
  zenity --password --title="$WOR_APP_TITLE"
fi
ASKPASS
      register_file_cleanup "$LINUX_ASKPASS"
    fi
    if [ -n "$LINUX_ASKPASS" ];then
      WOR_FLASH_TARGET="$DEVICE" WOR_ICON_PATH="$DIRECTORY/logo.png" SUDO_ASKPASS="$LINUX_ASKPASS" command sudo -A "$@"
      return
    fi
  fi
  command sudo "$@"
}

cleanup_mounts() {
  local mountpoint
  for mountpoint in "${CLEANUP_MOUNTS[@]}" ;do
    if is_macos ;then
      sudo diskutil unmount force "$mountpoint" >/dev/null 2>&1
    else
      sudo umount -q "$mountpoint" 2>/dev/null
    fi
  done
  for mountpoint in "${CLEANUP_DEVICES[@]}" ;do
    if is_macos ;then
      sudo hdiutil detach "$mountpoint" >/dev/null 2>&1
    fi
  done
  local file
  for file in "${CLEANUP_FILES[@]}" ;do
    rm -f "$file"
  done
}

register_mount_cleanup() { #Input: mountpoint
  CLEANUP_MOUNTS+=("$1")
  trap cleanup_mounts EXIT
}

register_device_cleanup() { #Input: hdiutil device
  CLEANUP_DEVICES+=("$1")
  trap cleanup_mounts EXIT
}

register_file_cleanup() { #Input: file path
  CLEANUP_FILES+=("$1")
  trap cleanup_mounts EXIT
}

gui_error_dialog() { #Input: error message
  local plain icon_path
  plain="$(echo -e "An error has occurred:\n$1\nExiting now." | sed 's/\x1b\[[0-9;]*m//g' | sed 's/\x1b\[[0-9;]*//g' | sed "s,\x1B\[[0-9;]*[a-zA-Z],,g")"
  #write the error marker before showing the dialog, so the GUI doesn't race with the installer
  if [ -n "$WOR_GUI_ERROR_MARKER" ] ;then
    mkdir -p "$(dirname "$WOR_GUI_ERROR_MARKER")" 2>/dev/null
    touch "$WOR_GUI_ERROR_MARKER" 2>/dev/null
    sync 2>/dev/null || true
  fi
  [ -f "$DIRECTORY/logo.png" ] && icon_path="$DIRECTORY/logo.png" || icon_path=''
  if command -v osascript >/dev/null ;then
    osascript -l JavaScript - "$plain" "$icon_path" "$WOR_APP_TITLE" <<'JXA' >/dev/null 2>&1
ObjC.import('AppKit')
const args = $.NSProcessInfo.processInfo.arguments
const message = ObjC.unwrap(args.objectAtIndex(4))
const iconPath = ObjC.unwrap(args.objectAtIndex(5))
const appTitle = ObjC.unwrap(args.objectAtIndex(6))

const app = $.NSApplication.sharedApplication
$.NSProcessInfo.processInfo.processName = appTitle
app.setActivationPolicy($.NSApplicationActivationPolicyRegular)
if (iconPath.length > 0) {
  app.setApplicationIconImage($.NSImage.alloc.initWithContentsOfFile($(iconPath)))
}

let window
const Controller = ObjC.registerSubclass({
  name: 'WorErrorController',
  superclass: 'NSObject',
  methods: {
    'okClicked:': {
      types: ['void', ['id']],
      implementation: function() {
        app.stopModalWithCode($.NSOKButton)
        window.orderOut(null)
      }
    },
    'windowWillClose:': {
      types: ['void', ['id']],
      implementation: function() {
        app.stopModalWithCode($.NSOKButton)
      }
    }
  }
})
const controller = $.WorErrorController.alloc.init

const width = 560
const height = 220
const style = $.NSWindowStyleMaskTitled | $.NSWindowStyleMaskClosable
window = $.NSWindow.alloc.initWithContentRectStyleMaskBackingDefer($.NSMakeRect(0, 0, width, height), style, $.NSBackingStoreBuffered, false)
window.title = appTitle
window.setDelegate(controller)
window.center

const content = $.NSView.alloc.initWithFrame($.NSMakeRect(0, 0, width, height))
window.contentView = content

const label = $.NSTextField.labelWithString(message)
label.frame = $.NSMakeRect(20, 70, width - 40, height - 100)
label.font = $.NSFont.systemFontOfSizeWeight(14, $.NSFontWeightMedium)
label.setUsesSingleLineMode(false)
label.cell.setWraps(true)
label.cell.setScrollable(false)
content.addSubview(label)

const okButton = $.NSButton.buttonWithTitleTargetAction('OK', controller, 'okClicked:')
okButton.bezelStyle = $.NSBezelStyleRounded
okButton.keyEquivalent = '\r'
okButton.sizeToFit
const okWidth = Math.max(96, okButton.frame.size.width)
okButton.frame = $.NSMakeRect(width - 20 - okWidth, 22, okWidth, 32)
content.addSubview(okButton)

window.makeKeyAndOrderFront(null)
if (!app.isActive) app.requestUserAttention($.NSInformationalRequest)
app.activateIgnoringOtherApps(true)
app.runModalForWindow(window)
app.terminate(null)
JXA
  elif command -v yad >/dev/null ;then
    yad --center --window-icon="$icon_path" --title="$WOR_APP_TITLE" --text="$plain"
  elif command -v zenity >/dev/null ;then
    zenity --error --title "$WOR_APP_TITLE" --width 360 --text "$plain"
  fi
}

error() { #Input: error message
  printf '\033[91m%b\033[0m\n' "$1" 1>&2
  [ "$RUN_MODE" == gui ] && gui_error_dialog "$1"
  exit 1
}

status() { #blue text to indicate what is happening
  if [[ "$1" == '-'* ]] && [ ! -z "$2" ];then
    printf '\033[96m%b\033[0m' "$2" 1>&2
    [ "$1" == '-n' ] || printf '\n' 1>&2
    emit_gui_progress "STATUS	$2"
  else
    printf '\033[96m%b\033[0m\n' "$1" 1>&2
    emit_gui_progress "STATUS	$1"
  fi
}

echo_green() { #announce the success of a major action
  printf '\033[92m%b\033[0m\n' "$1" 1>&2
}

echo_red() { #announce the failure of a nonfatal action
  printf '\033[91m%b\033[0m\n' "$1" 1>&2
}

warning() { #Input: message. A nonfatal problem the user should know about, but which does not stop the run.
  printf '\033[93m%b\033[0m\n' "$1" 1>&2
}

emit_gui_progress() { #Input: line. Lets a native GUI progress window show live status without a visible terminal.
  [ -n "$WOR_GUI_PROGRESS_FILE" ] && printf '%s\n' "$1" >> "$WOR_GUI_PROGRESS_FILE" 2>/dev/null
}

emit_gui_substep() { #Input: percent complete of the current step, so the bar moves within a step too.
  [ -n "$WOR_GUI_PROGRESS_FILE" ] && printf 'SUBSTEP\t%s\n' "$1" >> "$WOR_GUI_PROGRESS_FILE" 2>/dev/null
}

gui_percent_stream() { #Mirrors a progress stream back out while reporting any percentage it carries.
  local line
  while IFS= read -r line ;do
    printf '%s\n' "$line" 1>&2
    [[ "$line" =~ ([0-9]{1,3})% ]] && emit_gui_substep "${BASH_REMATCH[1]}"
  done
}

with_progress_capture() { #Input: command. Feeds the GUI bar from tools that already print a percentage.
  if [ -n "$WOR_GUI_PROGRESS_FILE" ];then
    #pv and wimlib redraw with carriage returns, which read would otherwise never see as lines
    "$@" 2> >(tr '\r' '\n' | gui_percent_stream)
  else
    "$@"
  fi
}

clear_cached_components() { #Deletes every cached download, one at a time so the GUI can show it happening.
  local targets=() target s seen removed=0 total
  #winfiles_from_iso_* is also matched by winfiles_*, so skip anything already listed
  for target in "$PWD/peinstaller" "$PWD/driverpackage" "$PWD"/pi[345]-uefipackage "$PWD"/winfiles_* ;do
    #an unmatched glob stays literal, so only keep entries that exist
    [ -e "$target" ] || continue
    seen=0
    for s in "${targets[@]}" ;do [ "$s" == "$target" ] && seen=1 && break ;done
    [ "$seen" == 0 ] && targets+=("$target")
  done
  [ -n "$DIRECTORY" ] && [ -e "$DIRECTORY/cache" ] && targets+=("$DIRECTORY/cache")
  total="${#targets[@]}"
  if [ "$total" == 0 ];then
    status "Nothing cached to delete"
    emit_gui_substep 100
  else
    for target in "${targets[@]}" ;do
      status "Deleting $(basename "$target") ($((removed+1)) of $total)"
      rm -rf "$target"
      removed=$((removed+1))
      emit_gui_substep $((removed * 100 / total))
    done
  fi
  [ -z "$DIRECTORY" ] || mkdir -p "${DIRECTORY}/cache"
}

phase() { #Input: message. A numbered status() line marking one of the major installation stages.
  STEP_NUM=$((STEP_NUM+1))
  status "[Step $STEP_NUM/$STEP_TOTAL] $1"
  emit_gui_progress "STEP	$STEP_NUM	$STEP_TOTAL	$1"
  #must follow the STEP line: the window reads the newest SUBSTEP as belonging to the current step
  emit_gui_substep 0
}

cli_pause() {
  if [ "$RUN_MODE" != gui ] && [ -t 0 ];then
    printf 'Press Enter to exit.\n'
    read -r </dev/tty
  fi
}

resolve_path() { #Input: path. Output: absolute path, using GNU or BSD tools when available
  [ -z "$1" ] && return 1
  if command -v realpath >/dev/null ;then
    realpath "$1" && return 0
  fi
  if readlink -f "$1" >/dev/null 2>&1 ;then
    readlink -f "$1" && return 0
  fi
  if [ -d "$1" ];then
    (cd "$1" && pwd -P)
  else
    local dir
    local base
    dir="$(dirname "$1")"
    base="$(basename "$1")"
    printf '%s/%s\n' "$(cd "$dir" && pwd -P)" "$base"
  fi
}

is_wsl() { #WSL reports itself as Linux, so uname alone cannot rule it out
  [ -n "${WSL_DISTRO_NAME:-}" ] || [ -n "${WSLENV:-}" ] || grep -qi 'microsoft\|wsl' /proc/version 2>/dev/null
}

require_linux_host() {
  if is_wsl ;then
    #WSL cannot reach USB storage without usbipd, and lsblk there lists WSL's own virtual disks as erasable targets
    error "WoR-Flasher does not support WSL.
WSL cannot access USB drives directly, and the drives it does list are WSL's own virtual disks.
Erasing one of those would damage your WSL installation.
On Windows, use the official Windows on Raspberry Imager instead: https://worproject.com/downloads
To use WoR-Flasher, run it from a Debian-based Linux host or macOS."
  fi
  [ "$HOST_OS" == Linux ] || is_macos && return 0
  error "WoR-Flasher supports Linux and macOS hosts only. This host is $HOST_OS.
On Windows, use the official Windows on Raspberry Imager instead: https://worproject.com/downloads"
}

require_macos_tools() {
  if ! command -v brew >/dev/null ;then
    if [ -x /opt/homebrew/bin/brew ];then
      PATH="/opt/homebrew/bin:/opt/homebrew/sbin:$PATH"
    elif [ -x /usr/local/bin/brew ];then
      PATH="/usr/local/bin:/usr/local/sbin:$PATH"
    fi
  fi
  local tool
  for tool in diskutil hdiutil plutil; do
    command -v "$tool" >/dev/null || error "macOS support requires '$tool' on PATH. Run this script from a normal macOS shell, then try again."
  done
}

darwin_plist_json() { #Input: a diskutil or hdiutil command. Output: its property list as JSON.
  "$@" | plutil -convert json -o - -
}

darwin_is_safe_device() { #Input: whole disk /dev path. Only external, physical, writable disks can be erased.
  local details
  details="$(darwin_plist_json diskutil info -plist "$1")" || return 1
  jq -e '.WholeDisk == true and .Internal == false and .VirtualOrPhysical == "Physical" and (.ReadOnlyMedia == false or .WritableMedia == true)' >/dev/null <<<"$details"
}

is_safe_target_device() { #Input: target device. Protects the host boot disk and Darwin internal/virtual disks.
  if is_macos ;then
    [ "$1" != "$ROOT_DEV" ] && darwin_is_safe_device "$1"
  else
    [ "$1" != "$ROOT_DEV" ]
  fi
}

darwin_device_value() { #Input: device, jq filter. Output: a value from diskutil info.
  darwin_plist_json diskutil info -plist "$1" | jq -er "$2"
}

darwin_partition_by_volume_name() { #Input: whole disk, volume name. Output: matching partition /dev path.
  local identifier
  identifier="$(darwin_plist_json diskutil list -plist "$1" | jq -er --arg name "$2" '.AllDisksAndPartitions[0].Partitions[]? | select(.VolumeName == $name) | .DeviceIdentifier' | head -n1)" || return 1
  [ -n "$identifier" ] || return 1
  printf '/dev/%s\n' "$identifier"
}

darwin_list_devices() { #Output: external physical whole disks from diskutil's plist interface.
  local device
  while read -r device; do
    [ -z "$device" ] && continue
    device="/dev/$device"
    if darwin_is_safe_device "$device"; then
      printf '\e[1m\e[97m%s\e[0m - \e[92m%sB\e[0m - \e[36m%s\e[0m\n' \
        "$device" \
        "$(darwin_device_value "$device" '.DiskSize // .TotalSize // .Size')" \
        "$(darwin_device_value "$device" '.MediaName // .DeviceIdentifier')"
    fi
  done < <(darwin_plist_json diskutil list -plist external physical | jq -r '.AllDisks[]')
}

darwin_list_device_paths() { #Output: external physical whole-disk paths suitable for a GUI choice list.
  local device
  while read -r device; do
    device="/dev/$device"
    darwin_is_safe_device "$device" && printf '%s\n' "$device"
  done < <(darwin_plist_json diskutil list -plist external physical | jq -r '.AllDisks[]')
}

human_size() { #Input: size in bytes. Output: human-readable size (e.g. 1.9 TB)
  awk -v bytes="$1" 'BEGIN {
    units[0]="B"; units[1]="KB"; units[2]="MB"; units[3]="GB"; units[4]="TB"
    size = bytes + 0
    i = 0
    while (size >= 1024 && i < 4) { size /= 1024; i++ }
    printf "%.1f %s", size, units[i]
  }'
}

darwin_apfs_volume_names() { #Input: whole disk. Output: APFS volume labels backed by this physical disk.
  local apfs_details partition_ids
  partition_ids="$(darwin_plist_json diskutil list -plist "$1" | jq -r '.AllDisksAndPartitions[0].Partitions[]?.DeviceIdentifier')" || return 0
  [ -z "$partition_ids" ] && return 0
  apfs_details="$(diskutil apfs list -plist 2>/dev/null | plutil -convert json -o - -)" || return 0
  jq -r --arg ids "$partition_ids" '
    ($ids | split("\n") | map(select(length > 0))) as $stores |
    [.Containers[]?
      | select([.PhysicalStores[]?.DeviceIdentifier] | any(. as $id | $stores | index($id)))
      | .Volumes[]?.Name]
    | unique
    | if length == 0 then empty else join(", ") end
  ' <<<"$apfs_details"
}

darwin_list_device_choices() { #Output: tab-delimited safe disk path, size, media name, labels, and detected volumes.
  local device details label_names volume_names
  while read -r device; do
    device="/dev/$device"
    darwin_is_safe_device "$device" || continue
    details="$(darwin_plist_json diskutil list -plist "$device")" || continue
    label_names="$(
      {
        jq -r '.AllDisksAndPartitions[0].Partitions[]?.VolumeName? // empty' <<<"$details"
        darwin_apfs_volume_names "$device"
      } | awk 'NF && !seen[$0]++ { labels = labels ? labels ", " $0 : $0 } END { print labels }'
    )"
    [ -z "$label_names" ] && label_names='No labels'
    volume_names="$(jq -r '[.AllDisksAndPartitions[0].Partitions[]? | .VolumeName // .DeviceIdentifier] | if length == 0 then "No volumes" else join(", ") end' <<<"$details")"
    #only the field before the first tab is parsed as the device path, so the remaining
    #fields use readable spacing instead of raw tabs, which render squished together
    printf '%s\t%s   %s   Labels: %s   Volumes: %s\n' \
      "$device" \
      "$(human_size "$(darwin_device_value "$device" '.DiskSize // .TotalSize // .Size')")" \
      "$(darwin_device_value "$device" '.MediaName // .DeviceIdentifier')" \
      "$label_names" \
      "$volume_names"
  done < <(darwin_plist_json diskutil list -plist external physical | jq -r '.AllDisks[]')
}

darwin_mount_iso() { #Input: ISO path. Sets ISO_MOUNTPOINT and ISO_DEVICE.
  local details
  details="$(hdiutil attach -nobrowse -readonly -plist "$1")" || return 1
  ISO_MOUNTPOINT="$(plutil -convert json -o - - <<<"$details" | jq -er '."system-entities"[] | select(.["mount-point"] != null) | .["mount-point"]' | head -n1)" || return 1
  ISO_DEVICE="$(plutil -convert json -o - - <<<"$details" | jq -er '."system-entities"[] | select(.["mount-point"] != null) | .["dev-entry"]' | head -n1)" || return 1
}

unattend_xml() { #Output: the answer file for this run, or nothing when neither customization is wanted.
  local auto_disable_3gb=0
  [ "$RPI_MODEL" == 4 ] && [ "$PI4_AUTO_DISABLE_3GB" == 1 ] && auto_disable_3gb=1
  [ "$OOBE_NETWORK_BYPASS" == 1 ] || [ "$auto_disable_3gb" == 1 ] || return 1

  cat <<'EOF'
<?xml version="1.0" encoding="utf-8"?>
<unattend xmlns="urn:schemas-microsoft-com:unattend">
EOF
  [ "$auto_disable_3gb" == 1 ] && read_config_template pi4-ram-unlock-specialize.xml
  [ "$OOBE_NETWORK_BYPASS" == 1 ] && read_config_template oobe-network-bypass.xml
  cat <<'EOF'
</unattend>
EOF
}

install_windows_setup_configuration() { #Input: mounted boot partition, mounted installation partition.
  local auto_disable_3gb=0 destination
  [ "$RPI_MODEL" == 4 ] && [ "$PI4_AUTO_DISABLE_3GB" == 1 ] && auto_disable_3gb=1
  [ "$OOBE_NETWORK_BYPASS" == 1 ] || [ "$auto_disable_3gb" == 1 ] || return 0

  if [ "$auto_disable_3gb" == 1 ];then
    for destination in "$1/Pi4Disable3GB.ps1" "$2/Pi4Disable3GB.ps1";do
      read_config_template pi4-ram-unlock.ps1 | sudo tee "$destination" >/dev/null
    done
  fi

  for destination in "$1/Autounattend.xml" "$2/Autounattend.xml";do
    unattend_xml | sudo tee "$destination" >/dev/null
  done
}

darwin_flash_device() {
  is_safe_target_device "$DEVICE" || error "Refusing to overwrite $DEVICE. Choose an external, physical, writable whole disk that is not the current boot drive."
  local boot_payload_kb boot_size_mb install_size_mb sgdisk_bin raw_device raw_part1 raw_part2 attempt
  sgdisk_bin="$(command -v sgdisk)" || error "sgdisk is required to partition $DEVICE correctly. Install it with 'brew install gptfdisk', then run this script again."
  #GUI mode authenticated at startup, while a dialog could still reach the front; prompting from here
  #would put it behind the progress window, where it can never be answered
  if ! command sudo -n -v >/dev/null 2>&1 && { [ "$RUN_MODE" == gui ] || ! sudo -v >/dev/null 2>&1; };then
    error "Administrator authentication failed or was canceled. Enter the correct macOS password and try again."
  fi
  raw_device="/dev/r${DEVICE#/dev/}"
  boot_payload_kb="$(du -sk "$PWD/$winfiles/bootpart" "$PWD/peinstaller/winpe/2" "$PWD/peinstaller/efi" 2>/dev/null | awk '{total += $1} END {print total + 0}')"
  boot_size_mb=$((boot_payload_kb / 1024 + 512))
  [ "$boot_size_mb" -lt 1536 ] && boot_size_mb=1536
  [ "$CAN_INSTALL_ON_SAME_DRIVE" == 1 ] && install_size_mb=18000 || install_size_mb=6000
  phase "Partitioning and formatting $DEVICE"
  printf '  There is no turning back now.\n' 1>&2
  status "  Unmounting existing volumes"
  sudo diskutil unmountDisk force "$DEVICE" || error "Failed to unmount $DEVICE."

  status "  Creating WOR_BOOT (${boot_size_mb} MB) and WOR_INSTALL (${install_size_mb} MB)"
  sudo "$sgdisk_bin" --zap-all "$raw_device" >/dev/null || error "Failed to clear the existing partition table on $DEVICE."
  sudo "$sgdisk_bin" -og \
    -n "1:0:+${boot_size_mb}M" -t 1:ef00 -c 1:WOR_BOOT \
    -n "2:0:+${install_size_mb}M" -t 2:0700 -c 2:WOR_INSTALL \
    -A 1:set:63 -A 2:set:63 \
    "$raw_device" >/dev/null || error "Failed to partition $DEVICE."
  status "  Releasing partition locks"
  sudo diskutil unmountDisk force "$DEVICE" >/dev/null \
    || error "Failed to unmount newly created partitions on $DEVICE."
  PART1="${DEVICE}s1"
  PART2="${DEVICE}s2"

  #raw partition writes don't republish device nodes instantly; wait for macOS to notice them
  for attempt in $(seq 1 15) ;do
    [ -e "$PART1" ] && [ -e "$PART2" ] && break
    [ "$attempt" == 5 ] && sudo diskutil unmountDisk force "$DEVICE" >/dev/null 2>&1
    sleep 1
  done
  [ -e "$PART1" ] || error "$PART1 did not appear after partitioning $DEVICE."
  [ -e "$PART2" ] || error "$PART2 did not appear after partitioning $DEVICE."
  raw_part1="/dev/r${PART1#/dev/}"
  raw_part2="/dev/r${PART2#/dev/}"

  status "  Formatting WOR_BOOT and WOR_INSTALL"
  sudo newfs_msdos -F 32 -v WOR_BOOT "$raw_part1" >/dev/null || error "Failed to format the boot partition on $PART1."
  sudo newfs_exfat -R -v WOR_INSTALL "$raw_part2" >/dev/null || error "Failed to format the installation partition on $PART2."
  sudo "$sgdisk_bin" -A 1:clear:63 -A 2:clear:63 "$raw_device" >/dev/null \
    || error "Failed to enable mounting of the formatted partitions on $DEVICE."

  sudo diskutil mount "$PART1" >/dev/null || error "Failed to mount $PART1."
  sudo diskutil mount "$PART2" >/dev/null || error "Failed to mount $PART2."
  boot_mount="$(darwin_device_value "$PART1" '.MountPoint')" || error "Failed to determine the boot partition mount point."
  win_mount="$(darwin_device_value "$PART2" '.MountPoint')" || error "Failed to determine the installation partition mount point."
  register_mount_cleanup "$boot_mount"
  register_mount_cleanup "$win_mount"

  phase "Copying files to $DEVICE:"
  echo "  - Startup environment"
  copy_startup_environment_with_progress "$PWD/$winfiles/bootpart" "$boot_mount" || error "Failed to copy startup files to $boot_mount"
  echo "  - Installation files"
  copy_file_with_progress install.wim "$PWD/$winfiles/install.wim" "$win_mount/install.wim" || error "Failed to copy installation files to $win_mount"
  echo "  - EFI files"
  sudo cp -r "$PWD/peinstaller/efi" "$boot_mount" || error "Failed to copy EFI files to $boot_mount"
  echo "  - PE installer"
  configure_pe_settings_ini
  configure_pe_prefinalize
  sudo wimupdate "$boot_mount/sources/boot.wim" 2 --command="add peinstaller/winpe/2 /" || error "The wimupdate command failed to add $PWD/peinstaller to boot.wim"

  if [ "$RPI_MODEL" == 5 ];then
    echo "  - ARM64 drivers"
    : > "$PWD/critical"
    sudo wimupdate "$boot_mount/sources/boot.wim" 2 --command="add critical /drivers/critical" || error "The wimupdate command failed to add $PWD/critical to boot.wim"
    rm "$PWD/critical"
  else
    echo "  - ARM64 drivers"
    sudo wimupdate "$boot_mount/sources/boot.wim" 2 --command="add driverpackage /drivers" || error "The wimupdate command failed to add $PWD/driverpackage to boot.wim"
  fi

  echo "  - Windows Setup configuration"
  install_windows_setup_configuration "$boot_mount" "$win_mount" || error "Failed to install the Windows Setup configuration."

  echo "  - UEFI firmware"
  sudo cp -r "$PWD/pi${RPI_MODEL}-uefipackage"/* "$boot_mount" || error "Failed to copy UEFI firmware to $boot_mount"
  [ -z "$CONFIG_TXT" ] || [ "$APPLY_CUSTOM_CONFIG_TXT" != 1 ] || echo "$CONFIG_TXT" | sudo tee "$boot_mount/config.txt" >/dev/null
  [ "$RPI_MODEL" != 3 ] || sudo dd if="$PWD/peinstaller/pi3/gptpatch.img" of="/dev/r${DEVICE#/dev/}" conv=fsync || error "Failed to apply the Pi3 GPT partition-table fix to $DEVICE"

  if [ "$SKIP_IMAGE_VERIFICATION" == 1 ];then
    echo_red "Skipping written-image verification (SKIP_IMAGE_VERIFICATION=1). This is not recommended."
  else
    verify_written_image "$DEVICE" "$PART1" "$PART2" "$boot_mount" "$win_mount" "$PWD/$winfiles/install.wim"
  fi
  sudo diskutil unmountDisk "$DEVICE" || echo_red "Warning: failed to unmount $DEVICE"
  sudo diskutil eject "$DEVICE" || echo_red "Warning: failed to eject $DEVICE"
  phase "$WOR_APP_TITLE script has completed."
  cli_pause
}

get_file_size() { #Input: file. Output: size in bytes, portable across GNU and BSD userlands
  wc -c < "$1" | tr -d ' '
}

get_esd_catalog_entry() { #Input: catalog text, language. Output: the language's first file entry before the Languages section.
  awk -v language="$2" '
    /<Languages>/ { exit }
    index($0, "<LanguageCode>" language) == 1 { found = 1 }
    found { print }
    found && /^<\/File>$/ { exit }
  ' <<<"$1"
}

sha1_file() { #Input: file. Output: SHA1 hash
  if command -v sha1sum >/dev/null ;then
    sha1sum "$1" | awk '{print $1}'
  else
    shasum -a 1 "$1" | awk '{print $1}'
  fi
}

remark_pe_cache() { #Re-records the manifest after editing cached PE payload, or the next run treats the cache as corrupt and re-downloads.
  local token
  token="$(cat "$PWD/peinstaller/.wor-flasher-version" 2>/dev/null)"
  [ -n "$token" ] && mark_cache "$PWD/peinstaller" "$token"
  return 0
}

configure_pe_settings_ini() { #Sets HideEmptyDrives in the cached WoR-PE settings.ini before it's added to boot.wim.
  local settings_ini="$PWD/peinstaller/winpe/2/settings.ini" tmp_ini
  [ -f "$settings_ini" ] || return 0
  tmp_ini="$(mktemp)" || return 1
  if grep -q '^HideEmptyDrives=' "$settings_ini";then
    awk -v val="$HIDE_EMPTY_DRIVES" '{ if ($0 ~ /^HideEmptyDrives=/) print "HideEmptyDrives=" val; else print }' "$settings_ini" > "$tmp_ini"
  else
    cat "$settings_ini" > "$tmp_ini"
    printf 'HideEmptyDrives=%s\n' "$HIDE_EMPTY_DRIVES" >> "$tmp_ini"
  fi
  mv "$tmp_ini" "$settings_ini"
  remark_pe_cache
  return 0
}

configure_pe_prefinalize() { #Stages the answer file and WoR-PE's prefinalize hook, which is what actually delivers it.
  #WoR-PE applies install.wim with DISM instead of running Windows Setup's media flow, so nothing ever
  #performs the implicit answer-file search that would find Autounattend.xml at the root of the media.
  #Its documented prefinalize.cmd hook runs with the applied Windows partition still mounted, which is
  #the only point where the answer file can be put somewhere the installed OS will read it.
  local app_dir="$PWD/peinstaller/winpe/2" scripts_dir
  [ -d "$app_dir" ] || return 0
  scripts_dir="$app_dir/scripts"

  #a stale hook left in the cache would keep applying settings the user has since turned off
  rm -rf "$scripts_dir"
  unattend_xml >/dev/null 2>&1 || { remark_pe_cache; return 0; }

  mkdir -p "$scripts_dir" || return 1
  unattend_xml > "$scripts_dir/unattend.xml" || return 1
  #the specialize action runs on the installed OS, so its script has to travel there too
  if [ "$RPI_MODEL" == 4 ] && [ "$PI4_AUTO_DISABLE_3GB" == 1 ];then
    read_config_template pi4-ram-unlock.ps1 > "$scripts_dir/Pi4Disable3GB.ps1" || return 1
  fi
  #batch files are parsed by cmd.exe, which needs CRLF line endings
  read_config_template prefinalize.cmd | sed 's/$/\r/' > "$scripts_dir/prefinalize.cmd" || return 1
  remark_pe_cache
  return 0
}

sha256_file() { #Input: file. Output: SHA256 hash
  if command -v sha256sum >/dev/null ;then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}

copy_file_with_progress() { #Input: progress label, source file, destination file
  local label="$1" source="$2" destination="$3" size pipe_status
  size="$(get_file_size "$source")" || return 1
  with_progress_capture pv -f -N "$label" -s "$size" "$source" | sudo tee "$destination" >/dev/null
  pipe_status=("${PIPESTATUS[@]}")
  [ "${pipe_status[0]}" == 0 ] && [ "${pipe_status[1]}" == 0 ]
}

copy_local_file_with_progress() { #Input: progress label, source file, destination file
  local label="$1" source="$2" destination="$3" size pipe_status
  size="$(get_file_size "$source")" || return 1
  with_progress_capture pv -f -N "$label" -s "$size" "$source" | tee "$destination" >/dev/null
  pipe_status=("${PIPESTATUS[@]}")
  [ "${pipe_status[0]}" == 0 ] && [ "${pipe_status[1]}" == 0 ]
}

copy_startup_environment_with_progress() { #Input: source boot-media root, destination root, optional local flag
  local source="$1" destination="$2" mode="${3:-device}"
  if [ "$mode" == local ];then
    cp -R "$source/boot" "$source/efi" "$destination" || return 1
    mkdir -p "$destination/sources" || return 1
    copy_local_file_with_progress boot.wim "$source/sources/boot.wim" "$destination/sources/boot.wim"
  else
    sudo cp -R "$source/boot" "$source/efi" "$destination" || return 1
    sudo mkdir -p "$destination/sources" || return 1
    copy_file_with_progress boot.wim "$source/sources/boot.wim" "$destination/sources/boot.wim"
  fi
}

sha1_file_with_progress() { #Input: progress label, file. Output: SHA1 hash
  local label="$1" file="$2" size
  size="$(get_file_size "$file")" || return 1
  if command -v sha1sum >/dev/null ;then
    (set -o pipefail; with_progress_capture pv -f -N "$label" -s "$size" "$file" | sha1sum | awk '{print $1}')
  else
    (set -o pipefail; with_progress_capture pv -f -N "$label" -s "$size" "$file" | shasum -a 1 | awk '{print $1}')
  fi
}

sha256_file_with_progress() { #Input: progress label, file. Output: SHA256 hash
  local label="$1" file="$2" size
  size="$(get_file_size "$file")" || return 1
  if command -v sha256sum >/dev/null ;then
    (set -o pipefail; with_progress_capture pv -f -N "$label" -s "$size" "$file" | sha256sum | awk '{print $1}')
  else
    (set -o pipefail; with_progress_capture pv -f -N "$label" -s "$size" "$file" | shasum -a 256 | awk '{print $1}')
  fi
}

verify_written_image() { #Input: device, boot partition, install partition, boot mount, install mount, source install.wim
  local device="$1" boot_partition="$2" install_partition="$3" boot_mount="$4" install_mount="$5" source_install="$6"
  local partition_count boot_content install_content boot_filesystem install_filesystem boot_label install_label source_hash written_hash
  local device_size install_offset install_size trailing_free geometry

  phase "Verifying the written image"
  sync

  if is_macos ;then
    partition_count="$(darwin_plist_json diskutil list -plist "$device" | jq '[.AllDisksAndPartitions[0].Partitions[]?] | length')"
    boot_content="$(darwin_device_value "$boot_partition" '.Content')"
    install_content="$(darwin_device_value "$install_partition" '.Content')"
    boot_filesystem="$(darwin_device_value "$boot_partition" '.FilesystemType')"
    install_filesystem="$(darwin_device_value "$install_partition" '.FilesystemType')"
    boot_label="$(darwin_device_value "$boot_partition" '.VolumeName')"
    install_label="$(darwin_device_value "$install_partition" '.VolumeName')"
    device_size="$(darwin_device_value "$device" '.TotalSize // .Size')"
    install_offset="$(darwin_device_value "$install_partition" '.PartitionMapPartitionOffset')"
    install_size="$(darwin_device_value "$install_partition" '.Size')"
  else
    geometry="$(parted -ms "$device" unit B print)" || error "Written-image verification failed: could not read partition geometry from $device."
    partition_count="$(awk -F: '$1 ~ /^[0-9]+$/ {count++} END {print count + 0}' <<<"$geometry")"
    boot_content="$(awk -F: '$1 == 1 {print $7}' <<<"$geometry")"
    install_content="$(awk -F: '$1 == 2 {print $7}' <<<"$geometry")"
    device_size="$(awk -F: 'NR == 2 {sub(/B$/, "", $2); print $2}' <<<"$geometry")"
    install_offset="$(awk -F: '$1 == 2 {sub(/B$/, "", $2); print $2}' <<<"$geometry")"
    install_size="$(awk -F: '$1 == 2 {sub(/B$/, "", $4); print $4}' <<<"$geometry")"
    boot_filesystem="$(sudo blkid -s TYPE -o value "$boot_partition")"
    install_filesystem="$(sudo blkid -s TYPE -o value "$install_partition")"
    boot_label="$(sudo blkid -s LABEL -o value "$boot_partition")"
    install_label="$(sudo blkid -s LABEL -o value "$install_partition")"
  fi

  [ "$partition_count" == 2 ] || error "Written-image verification failed: expected exactly 2 partitions on $device, found $partition_count."
  [ "$boot_content" == EFI ] || [[ "$boot_content" == *esp* ]] || error "Written-image verification failed: partition 1 is not an EFI System Partition."
  [ "$install_content" == "Microsoft Basic Data" ] || [[ "$install_content" == *msftdata* ]] || error "Written-image verification failed: partition 2 is not Microsoft Basic Data."
  [ "$boot_filesystem" == msdos ] || [ "$boot_filesystem" == vfat ] || error "Written-image verification failed: partition 1 is not FAT32."
  [ "$install_filesystem" == exfat ] || error "Written-image verification failed: partition 2 is not ExFAT."
  [ "$boot_label" == WOR_BOOT ] || error "Written-image verification failed: partition 1 is labeled '$boot_label', not 'WOR_BOOT'."
  [ "$install_label" == WOR_INSTALL ] || error "Written-image verification failed: partition 2 is labeled '$install_label', not 'WOR_INSTALL'."
  [[ "$device_size" =~ ^[0-9]+$ ]] && [[ "$install_offset" =~ ^[0-9]+$ ]] && [[ "$install_size" =~ ^[0-9]+$ ]] \
    || error "Written-image verification failed: could not determine unallocated space on $device."
  trailing_free=$((device_size - install_offset - install_size))
  [ "$trailing_free" -ge $((1024*1024*1024)) ] \
    || error "Written-image verification failed: less than 1 GiB remains unallocated for the Windows target partition."

  status "  Checking required boot artifacts"
  sudo test -s "$boot_mount/RPI_EFI.fd" || error "Written-image verification failed: RPI_EFI.fd is missing or empty."
  case "$RPI_MODEL" in
    3)
      sudo test -s "$boot_mount/bootcode.bin" || error "Written-image verification failed: bootcode.bin is missing or empty."
      sudo test -s "$boot_mount/start.elf" || error "Written-image verification failed: start.elf is missing or empty."
      ;;
    4)
      sudo test -s "$boot_mount/start4.elf" || error "Written-image verification failed: start4.elf is missing or empty."
      sudo test -s "$boot_mount/fixup4.dat" || error "Written-image verification failed: fixup4.dat is missing or empty."
      ;;
    5)
      sudo test -s "$boot_mount/bcm2712-rpi-5-b.dtb" || error "Written-image verification failed: bcm2712-rpi-5-b.dtb is missing or empty."
      ;;
  esac
  sudo test -s "$boot_mount/EFI/BOOT/BOOTAA64.EFI" || error "Written-image verification failed: EFI/BOOT/BOOTAA64.EFI is missing or empty."
  sudo test -s "$boot_mount/EFI/Microsoft/Boot/bcd" || error "Written-image verification failed: EFI/Microsoft/Boot/bcd is missing or empty."
  sudo test -s "$boot_mount/sources/boot.wim" || error "Written-image verification failed: sources/boot.wim is missing or empty."
  sudo test -s "$install_mount/install.wim" || error "Written-image verification failed: install.wim is missing or empty."
  if [ "$OOBE_NETWORK_BYPASS" == 1 ] || { [ "$RPI_MODEL" == 4 ] && [ "$PI4_AUTO_DISABLE_3GB" == 1 ]; };then
    sudo cmp -s "$boot_mount/Autounattend.xml" "$install_mount/Autounattend.xml" \
      || error "Written-image verification failed: the answer file differs between the media partitions."
  fi
  if [ "$OOBE_NETWORK_BYPASS" == 1 ];then
    sudo grep -qF '<HideWirelessSetupInOOBE>true</HideWirelessSetupInOOBE>' "$boot_mount/Autounattend.xml" \
      || error "Written-image verification failed: the OOBE network bypass is missing from the boot partition."
  fi
  if [ "$RPI_MODEL" == 4 ] && [ "$PI4_AUTO_DISABLE_3GB" == 1 ];then
    sudo grep -qF '<WillReboot>Always</WillReboot>' "$boot_mount/Autounattend.xml" \
      || error "Written-image verification failed: the automatic Pi 4 RAM unlock is missing from the answer file."
    sudo grep -qF 'SetFirmwareEnvironmentVariableEx' "$boot_mount/Pi4Disable3GB.ps1" \
      || error "Written-image verification failed: the automatic Pi 4 RAM unlock script is missing."
    sudo cmp -s "$boot_mount/Pi4Disable3GB.ps1" "$install_mount/Pi4Disable3GB.ps1" \
      || error "Written-image verification failed: the Pi 4 RAM unlock script differs between the media partitions."
  fi

  if [ "$RPI_MODEL" == 4 ];then
    status "  Checking Pi 4 Setup drivers"
    sudo wimdir "$boot_mount/sources/boot.wim" 2 --path=/drivers/bcmgenet/bcmgenet.inf >/dev/null \
      || error "Written-image verification failed: the Pi 4 Ethernet driver is missing from boot.wim."
    sudo wimdir "$boot_mount/sources/boot.wim" 2 --path=/drivers/mcci_dwchsotg/mcci_dwchsotg_hcd.inf >/dev/null \
      || error "Written-image verification failed: the Pi 4 USB host driver is missing from boot.wim."
    sudo wimdir "$boot_mount/sources/boot.wim" 2 --path=/drivers/mcci_dwchsotg/mcci_dwchsotg_hub.inf >/dev/null \
      || error "Written-image verification failed: the Pi 4 USB hub driver is missing from boot.wim."
    sudo wimdir "$boot_mount/sources/boot.wim" 2 --path=/drivers/rpiuxflt/rpiuxflt.inf >/dev/null \
      || error "Written-image verification failed: the Pi 4 USB DMA filter driver is missing from boot.wim."
  fi

  status "  Verifying boot.wim integrity"
  sudo wimverify "$boot_mount/sources/boot.wim" || error "Written-image verification failed: boot.wim is invalid or corrupted."
  status "  Verifying install.wim integrity"
  sudo wimverify "$install_mount/install.wim" || error "Written-image verification failed: install.wim is invalid or corrupted."

  status "  Comparing install.wim with its source"
  source_hash="$(sha256_file_with_progress source "$source_install")" || error "Written-image verification failed: could not hash source install.wim."
  written_hash="$(sha256_file_with_progress written "$install_mount/install.wim")" || error "Written-image verification failed: could not hash written install.wim."
  [ "$source_hash" == "$written_hash" ] || error "Written-image verification failed: install.wim does not match its source."
  echo_green "Written image verified successfully"
}

wget() { #Intercept all wget commands. When possible, uses aria2c.
  local file=''
  local url=''
  local use=aria2c
  local quiet=0
  local check_cert=true
  local -a aria2_flags
  [ "$VERIFY_TLS" == 0 ] && check_cert=false
  aria2_flags=(-x 16 -s 16 --max-tries=10 --retry-wait=30 --max-file-not-found=5 --http-no-cache=true "--check-certificate=$check_cert" \
    --allow-overwrite=true --auto-file-renaming=false --remove-control-file --auto-save-interval=0 \
    --console-log-level=error --show-console-readout=false --summary-interval=1)

  local IFS=$'\n'
  local opts
  opts="$(IFS=$'\n'; echo "$*")"
  for opt in $opts ;do
    if [[ "$opt" == '--'* ]];then
      if [ "$opt" == '--quiet' ];then
        quiet=1
      elif [[ "$opt" == '--load-cookies='* ]];then
        aria2_flags+=("$opt")
      else #for any other arguments, fallback to wget
        use=wget
      fi

    elif [ "$opt" == '-' ];then
      #writing to stdout, use wget and hide output
      use=wget
      quiet=1
    elif [[ "$opt" == '-'* ]];then
      #this opt is a flag beginning with one '-'

      #check the value of every letter in this argument
      local i
      for i in $(fold -w1 <<<"$opt" | tail -n +2) ;do

        if [ "$i" == q ];then
          quiet=1
        elif [ "$i" == O ];then
          true
        elif [ "$i" == '-' ];then
          #writing to stdout, use wget and hide output
          use=wget
          quiet=1
        else #any other wget arguments
          use=wget
        fi
      done

    elif [[ "$opt" == *'://'* ]]; then
      #this opt is web address
      url="$opt"
    elif [[ "$opt" == '/'* ]]; then
      #this opt is file output
      if [ -z "$file" ];then
        file="$opt"
        #if output file is /dev/stdout, /dev/null, etc, use wget
        if [[ "$file" == /dev/* ]];then
          use=wget
          quiet=1
        fi
      else #file var already populated
        use=wget
      fi
    else
      #This argument does not begin with '-', contain '://', or begin with '/'.
      #Assume output file specified shorthand if file-argument is not already set
      if [ -z "$file" ];then
        file="$(pwd)/${opt}"
      else #file var already populated
        use=wget
      fi
    fi
  done

  if ! command -v aria2c >/dev/null ;then
    #aria2c command not found
    use=wget
  fi

  #now, perform the download using the chosen method
  if [ "$use" == wget ];then
    #run the true wget binary with all this function's args

    command wget --progress=bar:force:noscroll "$@"
    local exitcode=$?
  elif [ "$use" == aria2c ];then

    #if $file empty, generate it based on url
    if [ -z "$file" ];then
      file="$(pwd)/$(basename "$url")"
    fi

    aria2_flags+=("$url" -d "$(dirname "${file}")" -o "$(basename "${file}")")

    #suppress output if -q flag passed
    if [ "$quiet" == 1 ];then
      aria2c --quiet "${aria2_flags[@]}"
      local exitcode
      exitcode=$?

    else #run aria2c without quietness and format download-progress output
      local terminal_width
      terminal_width="$(tput cols 2>/dev/null || :)"
      [[ "$terminal_width" =~ ^[0-9]+$ ]] || terminal_width=80

      #run aria2c and reduce its output.
      aria2c "${aria2_flags[@]}" | while read -r line ;do

        #filter out unnecessary lines
        line="$(grep --line-buffered -v '\-\-\-\-\-\-\-\-\|======\|^FILE:\|^$\|Summary\|Results:\|download completed\.\|^Status Legend:\||OK\||stat' <<<"$line" || :)"

        if [ ! -z "$line" ];then #if this line still contains something and was not erased by grep

          #check if this line is a progress-stat line, like: "[#a6567f 20MiB/1.1GiB(1%) CN:16 DL:14MiB ETA:1m19s]"
          if [[ "$line" == '['*']' ]];then

            #hide cursor
            printf "\033[?25l"

            #print the total data only, like: "0.9GiB/1.1GiB"
            statsline="$(echo "$line" | awk '{print $2}' | sed 's/(.*//g' | tr -d '\n') "
            #get the length of statsline
            characters_subtract=${#statsline}

            #determine how many characters are available for the progress bar
            available_width=$(($terminal_width - $characters_subtract))
            #make sure available_width is a positove number (in case bash-variable COLUMNS is empty)
            [ "$available_width" -le 0 ] && available_width=20

            #get progress percentage from aria2c output
            percent="$(grep -o '(.*)' <<<"$line" | tr -d '()%')"

            #echo "percent: $percent"
            #echo "available_width: $available_width"

            #determine how many characters in progress bar to light up
            progress_characters=$(((percent*available_width)/100))

            progress_bar="$(for ((i=0; i<$progress_characters; i++)); do printf "—"; done)"
            printf -v colored_progress_bar '\033[92m\033[1m%s\033[39m' "$progress_bar"
            statsline+="$colored_progress_bar" # other possible characters to put here: █🭸
            printf '\033[0K%s\r\033[0m' "$statsline" 1>&2 #clear and print over previous line

            #reduce the line and print over the previous line, like: "1.1GiB/1.1GiB(98%) DL:18MiB"
            #echo "$line" | awk '{print $2 " " $4 " " substr($5, 1, length($5)-1)}' | tr -d '\n'

          else
            #this line is not a progress-stat line; don't format output
            echo "$line"
          fi
        fi

      done
      local exitcode=${PIPESTATUS[0]}
    fi
  fi

  #display a "download complete" message
  if [ $exitcode == 0 ] && [ "$quiet" == 0 ];then

    #show cursor
    printf "\033[?25h"

    #display "done" message
    if [ "$use" == aria2c ];then
      local progress_characters=$(($terminal_width - 5))
      printf '\033[0KDone \033[92m\033[1m%s\033[39m\033[0m\n' "$(for ((i=0; i<$progress_characters; i++)); do printf "—"; done)" 1>&2 #clear and print over previous line
    else
      echo
      echo_green "Done" 1>&2
    fi
  elif [ $exitcode != 0 ] && [ "$quiet" == 0 ];then
    #show cursor
    printf "\033[?25h"

    echo -e "\n\e[91mFailed to download: $url\nPlease review errors above.\e[0m" 1>&2
  fi

  return $exitcode
}

cache_downloader() { #returns contents of url, using cached output from a previous run if necessary
  [ -z "$1" ] && error "cache_downloader(): no url specified!"
  [ -z "$DIRECTORY" ] && error "cache_downloader(): DIRECTORY variable not set!"
  local output
  output="$(wget -qO- "$1")"

  if [ -z "$output" ];then
    output="$(cat "$DIRECTORY/cache/$(basename "$1")" 2>/dev/null)" || error "Unable to download $1"
  else
    echo "$output" > "$DIRECTORY/cache/$(basename "$1")"
  fi
  echo "$output"
}

download_from_gdrive() { #Input: file UUID and filename
  [ -z "$1" ] && error "download_from_gdrive(): requires a Google Drive file UUID!\nFile UUID is the end of a sharable link: https://drive.google.com/uc?export=download&id=XXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX"
  [ -z "$2" ] && error "download_from_gdrive(): requires specifying a filename to save to."

  local FILEUUID="$1"
  local FILENAME="$2"

  wget --load-cookies=/tmp/cookies.txt "https://drive.usercontent.google.com/download?$(wget --quiet --save-cookies /tmp/cookies.txt --keep-session-cookies 'https://drive.usercontent.google.com/download?export=download&id='"$FILEUUID" -O- | sed 's/input type="hidden" name="//g ; s/" value="/=/g ; s/"></\&/g' | grep -o '><export=.*/form>' | sed 's/><//g ; s+&/form>++g')" -O "$FILENAME"
  rm -rf /tmp/cookies.txt

}

package_available() { #determine if the specified package-name exists in a repository
  local package="$1"
  [ -z "$package" ] && error "package_available(): no package name specified!"
  #using grep to do this is nearly instantaneous, rather than apt-cache which takes several seconds
  grep -rqx "Package: $package" /var/lib/apt/lists --exclude="lock" --exclude-dir="partial" 2>/dev/null && return 0
  #the grep above cannot read compressed indexes, which apt uses when Acquire::GzipIndexes is set
  local candidate
  candidate="$(apt-cache policy "$package" 2>/dev/null | awk '/Candidate:/{print $2}')"
  [ ! -z "$candidate" ] && [ "$candidate" != '(none)' ]
}

package_installed() { #exit 0 if $1 package is installed, otherwise exit 1
  local package="$1"
  [ -z "$package" ] && error "package_installed(): no package specified!"
  #find the package listed in /var/lib/dpkg/status
  #package_info "$package"

  #directly search /var/lib/dpkg/status
  grep "^Package: $package$" /var/lib/dpkg/status -A 1 | tail -n 1 | grep -q 'Status: install ok installed'
}

install_packages() { #input: space-separated list of apt packages to install
  [ -z "$1" ] && error "install_packages(): requires a list of apt packages to install"
  if is_macos ;then
    command -v brew >/dev/null || error "macOS support requires Homebrew. Install it from https://brew.sh, then run this script again."
    local formula
    for formula in aria2 cabextract jq wget wimlib gptfdisk pv; do
      brew list --formula "$formula" >/dev/null 2>&1 || brew install "$formula" || error "Failed to install Homebrew dependency '$formula'."
    done
    return 0
  fi
  local dependencies="$1"
  local install_list=''
  local package

  local IFS=' '
  for package in $dependencies ;do
    if ! package_installed "$package" ;then
      #if the currently-checked package is not installed, add it to the list of packages to install
      if [ -z "$install_list" ];then
        install_list="$package"
      else
        install_list="$install_list $package"
      fi
    fi
  done

  if [ ! -z "$install_list" ];then
    status "Installing packages: $install_list"
    sudo apt update || error "Failed to run 'sudo apt update'! This is not an error in WoR-flasher."
    sudo apt install -yf $install_list --no-install-recommends || error "Failed to install dependency packages! This is not an error in WoR-flasher."
  fi
}

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

  if [ "$2" == 'all' ];then
    #special mode: return every partition if $2 is 'all'
    lsblk -nro NAME "$1" | sort -n | sed 's+^+/dev/+g' | grep -vx "$1"
  else #provided with partition number
    #list drive and partitions in $1, filter out the drive, then get the Nth line
    lsblk -nro NAME "$1" | sort -n | sed 's+^+/dev/+g' | grep -vx "$1" | sed -n "$2"p
  fi
}

get_device_name() { #get human-readable name of storage device: manufacturer and model name
  #Botspot made this by reverse-engineering the usb-devices command and udevadm commands.
  #input: /dev device
  [ -z "$1" ] && error "get_device_name(): requires an argument"
  [ ! -b "$1" ] && error "get_device_name(): Specified block device '$1' does not exist!"

  if is_macos ;then
    darwin_device_value "$1" '.MediaName // .DeviceIdentifier'
    return
  fi

  sys_path="$(find /sys/devices/platform -type d -name "$(basename "$1")")"
  #sys_path may be: /sys/devices/platform/scb/fd500000.pcie/pci0000:00/0000:00:00.0/0000:01:00.0/usb2/2-2/2-2:1.0/host0/target0:0:0/0:0:0:0/block/sda

  if [ -z "$sys_path" ];then
    echo "get_device_name(): Failed to find a /sys/devices/platform entry for '$1'. Continuing." 1>&2
    return 1
  fi

  #Go up 6 directories:
  sys_path="$(echo "$sys_path" | tr '/' '\n' | head -n -6 | tr '\n' '/')"
  #sys_path may be: /sys/devices/platform/scb/fd500000.pcie/pci0000:00/0000:00:00.0/0000:01:00.0/usb2/2-2/

  product="$(cat "${sys_path}product" 2>/dev/null)"
  manufacturer="$(cat "${sys_path}manufacturer" 2>/dev/null)"
  #serial="$(cat "$sys_path"/serial)"

  if [ -z "$product$manufacturer" ] && [[ "$1" == /dev/mmcblk* ]];then
    manufacturer="SD card"
  fi

  if [ "$manufacturer" != "$product" ];then
    echo "$manufacturer $product" | sed 's/ $//g' | sed 's/^ //g'
  else
    echo "$manufacturer"
  fi
}

get_size_raw() { #Input: device. Output: total size of device in bytes
  if is_macos ;then
    darwin_device_value "$1" '.DiskSize // .TotalSize // .Size'
    return
  fi
  command -v lsblk >/dev/null || error "get_size_raw(): lsblk is required."
  lsblk -b --output SIZE -n -d "$1"
}

drive_capability() { #Input: block device. Output: 'too-small', 'recovery' or 'install'
  #single source of truth for the size tiers, used by both this script and the GUI
  local size
  size="$(get_size_raw "$1")"
  if [ "$size" -lt $((8*1024*1024*1024)) ];then
    echo too-small
  elif [ "$size" -lt $((25*1024*1024*1024)) ];then
    echo recovery
  else
    echo install
  fi
}

validate_install_mode() { #Input: drive capability. Validates CAN_INSTALL_ON_SAME_DRIVE when supplied.
  case "$1" in
    too-small)
      error "Drive $DEVICE is smaller than 8GB and cannot be used."
      ;;
    recovery|install)
      ;;
    *)
      error "Unexpected drive capability '$1' for $DEVICE"
      ;;
  esac

  if [ -n "$CAN_INSTALL_ON_SAME_DRIVE" ];then
    if [ "$CAN_INSTALL_ON_SAME_DRIVE" != 0 ] && [ "$CAN_INSTALL_ON_SAME_DRIVE" != 1 ];then
      error "Unknown value for CAN_INSTALL_ON_SAME_DRIVE. Expected '0' or '1'."
    elif [ "$CAN_INSTALL_ON_SAME_DRIVE" == 1 ] && [ "$1" != install ];then
      error "Drive $DEVICE is smaller than 25GB and cannot be used for self-installation.\nPlease set CAN_INSTALL_ON_SAME_DRIVE=0"
    fi
  fi
}

get_space_free() { #Input: folder to check. Output: how many bytes can fit before the disk is full.
  if df -B 1 "$1" --output=avail >/dev/null 2>&1 ;then
    df -B 1 "$1" --output=avail | tail -1 | tr -d ' '
  else
    df -Pk "$1" | awk 'NR==2 {print $4 * 1024}'
  fi
}

require_free_space() { #Input: minimum required bytes, path. Abort with a clear error if the local disk is too full.
  local required_bytes="$1"
  local target_path="${2:-$DL_DIR}"
  local free_bytes required_human free_human shortage_human

  [ -n "$required_bytes" ] || return 0
  free_bytes="$(get_space_free "$target_path")" || return 0
  if [ "$free_bytes" -lt "$required_bytes" ];then
    required_human="$(human_size "$required_bytes")"
    free_human="$(human_size "$free_bytes")"
    shortage_human="$(human_size $((required_bytes - free_bytes)))"
    error "Not enough free space in $target_path to download the required Windows files.
Required for this download set: $required_human
Available now: $free_human
Please free up at least $shortage_human and try again, or choose a different download directory."
  fi
}

cache_manifest() { #Input: folder. Output: stable SHA-256 manifest of cached payload files.
  (
    local file hash
    cd "$1" || return 1
    find . -type f ! -name '.wor-flasher-version' ! -name '.wor-flasher-sha256' ! -name '.wor-flasher-sha256.tmp' -print \
      | LC_ALL=C sort \
      | while IFS= read -r file ;do
          hash="$(sha256_file "$file")" || exit 1
          [ -n "$hash" ] || exit 1
          printf '%s  %s\n' "$hash" "$file"
        done
  )
}

cache_contents_are_current() { #Input: folder. Exit 0 if its payload matches the recorded manifest.
  [ -s "$1/.wor-flasher-sha256" ] || return 1
  cache_manifest "$1" | cmp -s - "$1/.wor-flasher-sha256"
}

cache_is_current() { #Input: folder, version token. Exit 0 if the cached folder can be reused.
  local folder="$1"
  local token="$2"
  [ ! -d "$folder" ] && return 1
  [ "$USE_CACHE" == 2 ] && return 0 #trust the cache without checking anything
  [ "$USE_CACHE" == 1 ] \
    && [ "$(cat "${folder}/.wor-flasher-version" 2>/dev/null)" == "$token" ] \
    && cache_contents_are_current "$folder" \
    && return 0
  return 1
}

mark_cache() { #Input: folder, version token. Records payload integrity and source version.
  local manifest="${1}/.wor-flasher-sha256"
  rm -f "${1}/.wor-flasher-version"
  cache_manifest "$1" > "${manifest}.tmp" || {
    rm -f "${manifest}.tmp"
    return 1
  }
  mv "${manifest}.tmp" "$manifest" || return 1
  printf '%s\n' "$2" > "${1}/.wor-flasher-version"
}

list_dev_paths() { #Output: whole-disk paths that may be written to. Omits /dev/loop* and the root device.
  if is_macos ;then
    darwin_list_device_paths
    return
  fi
  [ -z "$ROOT_DEV" ] && detect_root_dev
  lsblk -I 8,179,259 -dno NAME | sed 's+^+/dev/+g' | grep -v loop | grep -vx "$ROOT_DEV"
}

list_devs() { #Output: human-readable, colorized list of valid block devices to write to. Omits /dev/loop* and the root device. Returns code 1 if no drives found
  if is_macos ;then
    darwin_list_devices
    return
  fi
  local IFS=$'\n'
  local device size exitcode=1
  for device in $(list_dev_paths) ;do
    size="$(lsblk -dnbo SIZE "$device")" || continue
    if [[ "$size" =~ ^[0-9]+$ ]] && [ "$size" -gt 0 ];then
      echo -e "\e[1m\e[97m${device}\e[0m - \e[92m$(lsblk -dno SIZE "$device")B\e[0m - \e[36m$(get_device_name "$device")\e[0m"
      exitcode=0
    fi
  done
  return $exitcode
}

detect_root_dev() { #Set ROOT_DEV to the Linux block device backing /
  if is_macos ;then
    ROOT_DEV="/dev/$(darwin_device_value / '.ParentWholeDisk // .DeviceIdentifier')"
    return
  fi
  command -v findmnt >/dev/null || error "findmnt is required to detect the current boot drive."
  command -v lsblk >/dev/null || error "lsblk is required to detect the current boot drive. WoR-Flasher's flashing path currently supports Linux hosts only."
  local root_source
  local root_parent
  root_source="$(findmnt -n -o SOURCE /)"
  root_parent="$(lsblk -no pkname "$root_source")"
  [ -z "$root_parent" ] && error "Failed to determine the current boot drive from $root_source."
  ROOT_DEV="/dev/$root_parent"
}

list_langs() { #Output: colon-delimited list of languages. Format is <lang-code>:<lang-name>
  #echo "$catalog" | sed 's/></>\n</g' | sed -n '/<Languages>/q;p' | grep '<LanguageCode>\|<Language>' | tr -d '\n' | sed 's/<\/Language><LanguageCode>/\n/g' | sed 's/<\/LanguageCode><Language>/:/g' | sed 's/^<LanguageCode>//g' | sed 's/<\/Language>$/\n/g' | sed 's/&#xE5;/å/g' | sort

  echo -e "ar-sa:Arabic (Saudi Arabia)\nbg-bg:Bulgarian (Bulgaria)\ncs-cz:Czech (Czechia)\nda-dk:Danish (Denmark)\nde-de:German (Germany)\nel-gr:Greek (Greece)\nen-gb:English (United Kingdom)\nen-us:English (United States)\nes-es:Spanish (Spain, International Sort)
es-mx:Spanish (Mexico)\net-ee:Estonian (Estonia)\nfi-fi:Finnish (Finland)\nfr-ca:French (Canada)\nfr-fr:French (France)\nhe-il:Hebrew (Israel)\nhr-hr:Croatian (Croatia)\nhu-hu:Hungarian (Hungary)\nit-it:Italian (Italy)\nja-jp:Japanese (Japan)\nko-kr:Korean (Korea)
lt-lt:Lithuanian (Lithuania)\nlv-lv:Latvian (Latvia)\nnb-no:Norwegian Bokmål (Norway)\nnl-nl:Dutch (Netherlands)\npl-pl:Polish (Poland)\npt-br:Portuguese (Brazil)\npt-pt:Portuguese (Portugal)\nro-ro:Romanian (Romania)\nru-ru:Russian (Russia)\nsk-sk:Slovak (Slovakia)
sl-si:Slovenian (Slovenia)\nsr-latn-rs:Serbian (Latin, Serbia)\nsv-se:Swedish (Sweden)\nth-th:Thai (Thailand)\ntr-tr:Turkish (Turkey)\nuk-ua:Ukrainian (Ukraine)\nzh-cn:Chinese (Simplified, China)\nzh-tw:Chinese (Traditional, Taiwan)"
}

default_win_lang() { #Output: the host locale's matching Windows language code, or en-us.
  local host_locale language_code
  if is_macos ;then
    host_locale="$(defaults read -g AppleLocale 2>/dev/null)"
  else
    host_locale="${LC_ALL:-${LC_MESSAGES:-${LANG:-}}}"
  fi
  language_code="$(printf '%s' "$host_locale" | sed 's/@.*//' | sed 's/\..*//' | tr '_' '-' | tr '[:upper:]' '[:lower:]')"
  if list_langs | awk -F: '{print $1}' | grep -qFx "$language_code" ;then
    printf '%s\n' "$language_code"
  else
    printf '%s\n' 'en-us'
  fi
}

list_bids() { #input: '10' or '11', Output: build IDs for ESD releases. Format: "$BID ($date)"
  if [ -z "$versions" ];then
    #Get list of major Windows ESD versions from worproject.com
    versions="$(cache_downloader 'https://worproject.com/dldserv/esd/getversions.php')" || return 1
    #format variable
    versions="$(echo "$versions" | sed 's/<release /\n<release /g' | sed 's+</release></releases>+\n</release></releases>+g')"
  fi

  if [ "$1" == 11 ];then
    #List Windows 11 versions
    echo "$versions" | sed -n '/version number="11"/,/version number="10"/p' | grep 'release build' | sed 's/^<release build="//g' | sed 's/"><date>/ (/g' | sed 's/<\/date>.*/)/g' | sed 's/.....$/-&/' | sed 's/...$/-&/'
  elif [ "$1" == 10 ];then
    #List Windows 10 versions
    echo "$versions" | sed -n '/version number="10"/,/release build="'"$WIN10_OLDEST_BUILD"'"/p' | grep 'release build' | sed 's/^<release build="//g' | sed 's/"><date>/ (/g' | sed 's/<\/date>.*/)/g' | sed 's/.....$/-&/' | sed 's/...$/-&/'
  else
    error "list_bids(): unrecognized OS version. Expected '10' or '11'."
  fi
}

cpu_supports_bid() { #input: build id. Exit 0 if the target Pi's CPU can run this Windows build.
  #Pi3 (Cortex-A53) and Pi4 (Cortex-A72) are ARMv8.0. Builds past ARMV80_MAX_BUILD need ARMv8.1 atomics.
  local major
  major="$(echo "$1" | awk -F. '{print $1}')"
  [ -z "$major" ] && return 0
  if [ "$RPI_MODEL" == 3 ] || [ "$RPI_MODEL" == 4 ];then
    [ "$major" -gt "$ARMV80_MAX_BUILD" ] && return 1
  fi
  return 0
}

list_bids_supported() { #input: '10' or '11', Output: same as list_bids, minus builds the target Pi cannot run
  local line
  for line in $(list_bids "$1") ;do
    cpu_supports_bid "$(echo "$line" | awk '{print $1}')" && echo "$line"
  done
}

get_bid() { #input: '10' or '11', Output: newest build ID the target Pi's CPU can run
  list_bids_supported "$1" | awk '{print $1}' | head -n1
}

get_os_name() { #input: build id, Output: either "Windows 10 build $BID" or "Windows 11 build $BID"
  local BID="$1"
  if [ "$(echo "$BID" | awk -F. '{print $1}')" -ge "$WIN11_MIN_BUILD" ];then
    echo "Windows 11 build $BID"
  elif [ "$(echo "$BID" | awk -F. '{print $1}')" -lt "$WIN11_MIN_BUILD" ];then
    echo "Windows 10 build $BID"
  fi
}

uefi_pinned_version() { #Output: the pinned UEFI firmware version for the selected RPI_MODEL.
  case "$RPI_MODEL" in
    3) echo "$UEFI_VER_PI3" ;;
    4) echo "$UEFI_VER_PI4" ;;
    5) echo "$UEFI_VER_PI5" ;;
  esac
}

cache_mode_label() { #Input: USE_CACHE value. Output: how that mode reads on a summary or confirmation screen.
  case "$1" in
    0) echo 'Re-download everything' ;;
    2) echo 'Trust the cache without checking' ;;
    *) echo 'Reuse when they still match' ;;
  esac
}

install_mode_label() { #Input: CAN_INSTALL_ON_SAME_DRIVE value. Output: how that mode reads on a summary screen.
  case "$1" in
    1) echo 'Install Windows onto this drive' ;;
    *) echo 'Recovery drive for another >16 GB drive' ;;
  esac
}

set_default_config_txt() { #Sets CONFIG_TXT from the selected model's shipped template, unless the caller already supplied one.
  [ -n "$CONFIG_TXT" ] && return 0
  case "$RPI_MODEL" in
    3 | 4 | 5) CONFIG_TXT="

$(read_config_template "pi${RPI_MODEL}.config.txt")" ;;
  esac
}

describe_device() { #Input: device. Output: the path plus its size and model when those can be read.
  local size name detail
  [ -z "$1" ] && return 0
  size="$(get_size_raw "$1" 2>/dev/null)"
  name="$(get_device_name "$1" 2>/dev/null)"
  #a summary line must never be the thing that stops a run, so unreadable details are simply omitted
  if [ -n "$size" ] && [ "$size" -gt 0 ] 2>/dev/null ;then
    detail="$(human_size "$size")"
  fi
  [ -n "$name" ] && detail="${detail:+$detail }$name"
  [ -n "$detail" ] && printf '%s (%s)\n' "$1" "$detail" || printf '%s\n' "$1"
}

wor_log_file() { #Output: where a failed run's log is kept. Resolved on use, since the Linux GUI can still change DL_DIR.
  printf '%s\n' "${WOR_LOG_FILE:-$DL_DIR/last-run.log}"
}

settings_summary() { #Output: tab-separated "label<TAB>value" lines describing this run. One source of truth for the CLI banner and both GUI confirmation screens.
  printf 'WoR-Flasher version\t%s\n' "$WOR_FLASHER_VERSION"
  printf 'Target drive\t%s\n' "$(describe_device "$DEVICE")"
  printf 'Target hardware\tRaspberry Pi %s\n' "$RPI_MODEL"
  printf 'Operating system\t%s\n' "$(get_os_name "$BID" | sed "s/ build / ($WIN_LANG) arm64 build /g")"
  printf 'Installation mode\t%s\n' "$(install_mode_label "$CAN_INSTALL_ON_SAME_DRIVE")"
  [ -n "$SOURCE_FILE" ] && printf 'Windows source\t%s\n' "$SOURCE_FILE"
  printf 'Offline OOBE\t%s\n' "$([ "$OOBE_NETWORK_BYPASS" == 1 ] && echo 'Allowed' || echo 'Disabled')"
  [ "$RPI_MODEL" == 4 ] && printf 'Pi 4 RAM unlock\t%s\n' "$([ "$PI4_AUTO_DISABLE_3GB" == 1 ] && echo 'Enabled' || echo 'Disabled')"
  printf 'UEFI firmware\t%s\n' "$([ "$UEFI_USE_LATEST" == 1 ] && echo 'Latest' || echo "Pinned ($(uefi_pinned_version))")"
  printf 'Windows ARM64 drivers\t%s\n' "$([ "$DRIVERS_USE_LATEST" == 1 ] && echo 'Latest' || echo "Pinned ($DRIVER_VER)")"
  printf 'Custom config.txt\t%s\n' "$([ "$APPLY_CUSTOM_CONFIG_TXT" == 1 ] && echo 'Applied' || echo "Using the firmware default")"
  printf 'Hide empty drives\t%s\n' "$([ "$HIDE_EMPTY_DRIVES" == 1 ] && echo 'Yes' || echo 'No')"
  printf 'Verify written image\t%s\n' "$([ "$SKIP_IMAGE_VERIFICATION" == 1 ] && echo 'No (skipped)' || echo 'Yes')"
  printf 'Downloaded files\t%s\n' "$(cache_mode_label "$USE_CACHE")"
  printf 'Dry run\t%s\n' "$([ "$DRY_RUN" == 1 ] && echo 'Yes (no changes will be written)' || echo 'No')"
  printf 'Download directory\t%s\n' "$DL_DIR"
  printf 'Log file\t%s\n' "$(wor_log_file)"
  return 0
}

validate_iso_file() { #Input: path to a Windows ISO. Output: why it is unusable, or nothing. Exit 0 when it can be used.
  local iso="$1"
  if [ ! -f "$iso" ];then
    echo "This file does not exist. Check spelling and try again."
  elif [[ "$iso" != *'.ISO' ]] && [[ "$iso" != *'.iso' ]];then
    echo "This file does not have a .ISO file extension."
  elif [ "$(get_file_size "$iso")" -lt $((3*1024*1024*1024)) ];then
    echo "This file is smaller than 3GB and is probably incomplete."
  else
    return 0
  fi
  return 1
}

bid_from_iso_name() { #Input: ISO path. Output: the Windows build number in its filename, if there is one.
  basename "$1" | tr '_ -' '\n' | grep -E -m 1 '^[0-9]{5}'
}

lang_from_iso_name() { #Input: ISO path. Output: the Windows language code in its filename, if there is one.
  basename "$1" | tr '_ ' '\n' | grep -io -m 1 "$(list_langs | awk -F: '{print $1}' | tr '\n' ';' | sed 's/;/\\|/g' | sed 's/\\|$/\n/g')" | tr '[A-Z]' '[a-z]'
}

is_known_win_lang() { #Input: language code. Exit 0 if it appears in the published language list.
  list_langs | awk -F: '{print $1}' | grep -qFx "$1"
}

list_langs_preferred() { #Output: list_langs with en-us first, then the other English variants, then the rest.
  list_langs | grep '^en-us:'
  list_langs | grep '^en-' | grep -v '^en-us:'
  list_langs | grep -v '^en-'
}

list_cached_winfiles() { #Input: optional directory, default DL_DIR. Output: names of winfiles folders that finished extracting.
  find "${1:-$DL_DIR}" -maxdepth 2 -type f -name 'alldone' 2>/dev/null \
    | sed 's+/alldone$++' | xargs -I{} basename {} 2>/dev/null \
    | grep -E '^winfiles(_from_iso)?_' | sort -r
}

bid_from_winfiles_dir() { #Input: winfiles folder name. Output: the build ID it holds.
  echo "$1" | sed 's/^winfiles_from_iso_//g; s/^winfiles_//g' | awk -F_ '{print $1}'
}

lang_from_winfiles_dir() { #Input: winfiles folder name. Output: the Windows language it holds.
  echo "$1" | sed 's/^winfiles_from_iso_//g; s/^winfiles_//g' | awk -F_ '{print $2}'
}

settings_summary_plain() { #Input: optional printf format taking the label then the value. Output: one plain line per setting.
  local format="${1:-%s %s\n}" label value
  settings_summary | while IFS=$'\t' read -r label value ;do
    #shellcheck disable=SC2059
    printf "$format" "$label:" "$value"
  done
}

settings_summary_markup() { #Output: one pango-markup line per setting, for a yad --text window.
  local label value
  settings_summary | while IFS=$'\t' read -r label value ;do
    #yad renders its text as markup, so a value carrying <, > or & would corrupt the whole window
    printf -- '- %s: <b>%s</b>\n' "$label" "$(printf '%s' "$value" | sed 's/&/\&amp;/g; s/</\&lt;/g; s/>/\&gt;/g')"
  done
}

#Every setting a GUI front-end collects and the installer subprocess has to see. Kept in one place so
#the macOS and Linux front-ends can never drift into exporting different subsets of the same run.
WOR_INSTALLER_SETTINGS=(DIRECTORY DL_DIR RPI_MODEL BID WIN_LANG DEVICE CAN_INSTALL_ON_SAME_DRIVE SOURCE_FILE
  CONFIG_TXT APPLY_CUSTOM_CONFIG_TXT PI4_AUTO_DISABLE_3GB OOBE_NETWORK_BYPASS UEFI_USE_LATEST DRIVERS_USE_LATEST
  SKIP_IMAGE_VERIFICATION HIDE_EMPTY_DRIVES USE_CACHE DRY_RUN WOR_APP_TITLE)

export_installer_settings() { #Exports every collected setting, so the installer subprocess runs exactly what was confirmed.
  export "${WOR_INSTALLER_SETTINGS[@]}"
}

gui_preauthenticate() { #Collects the admin credential once, before the GUI covers the screen with its progress window.
  #This has to happen in the process that will actually use sudo: a credential obtained in the GUI is
  #recorded against that process's terminal, and the installer runs as a separate job.
  if [ "$DRY_RUN" != 1 ];then
    sudo -v || error "Administrator authentication failed or was canceled. Enter the correct macOS password and try again."
    #a flash outlasts the sudo timestamp, and by then no dialog could reach the front to renew it
    ( while kill -0 "$$" 2>/dev/null ;do command sudo -n -v >/dev/null 2>&1; sleep 30; done ) &
  fi
  #the front-end waits for this before opening its progress window
  if [ -n "$WOR_GUI_AUTH_MARKER" ];then
    touch "$WOR_GUI_AUTH_MARKER" 2>/dev/null
    sync 2>/dev/null || true
  fi
}

setup() { #run safety checks and install packages
  require_linux_host

  if is_macos ;then
    require_macos_tools
  fi

  if [ "$(id -u)" == 0 ];then
    status "WoR-Flasher is not designed to be run as root.\nDoing so is known to cause problems."
    echo -n "Are you sure you want to continue? [y/N]"
    read answer
    echo "$answer"
    if [ -z "$answer" ] || [ "$answer" != y ];then
      exit 1
    fi
    for i in {60..0}; do
      echo -ne "You have $i seconds to reconsider your decision.\033[0K\r"
      sleep 1
    done
  fi

  #Make sure modules exist for the running kernel - otherwise a kernel upgrade occurred and the user needs to reboot. See https://github.com/Botspot/wor-flasher/issues/35
  if [ "$HOST_OS" == Linux ] && [ ! -d "/lib/modules/$(uname -r)" ];then
    error "The running kernel ($(uname -r)) does not match any directory in /lib/modules.
Usually this means you have not yet rebooted since upgrading the kernel.
Try rebooting.
If this error persists, contact Botspot - the WoR-flasher developer."
  fi

  #install dependencies before using them for setup checks
  if [ "$SKIP_PACKAGE_INSTALL" != 1 ];then
    install_packages 'yad aria2 cabextract wimtools chntpw genisoimage exfat-fuse wget udftools bc parted dosfstools unzip git pv' || exit 1

    #install exfat partition manipulation utility. exfatprogs replaces exfat-utils, but they cannot both be installed at once.
    if [ "$HOST_OS" == Linux ];then
      if package_available exfatprogs && ! package_installed exfat-utils ;then
        install_packages exfatprogs || exit 1
      else
        install_packages exfat-utils || exit 1
      fi
    fi
  fi

  if ! command -v wget >/dev/null ;then
    error "Missing required dependency: wget.
WoR-Flasher needs wget to verify GitHub connectivity and download files.
Install wget, or leave SKIP_PACKAGE_INSTALL unset so WoR-Flasher can install dependencies automatically."
  fi

  #check for internet connection
  echo -n "Checking for internet connection... "
  local errors
  errors="$(command wget --spider github.com 2>&1)"
  if [ $? != 0 ];then
    error "Could not reach github.com.
Check your internet connection, DNS/proxy settings, or firewall, then run this script again.
Errors: $errors"
  fi
  echo Done

  [ -z "$ROOT_DEV" ] && detect_root_dev
  return 0
}

#
######## END OF FUNCTIONS, BEGINNING OF SCRIPT
#

#Determine the directory to download windows component files to
[ -z "$DL_DIR" ] && DL_DIR="$HOME/wor-flasher-files"

#Where a failed run's log is kept. Left unset so it follows DL_DIR even if the GUI changes that later;
#set it to an absolute path to put the log somewhere else.

#UEFI firmware selection.
#Set UEFI_USE_LATEST=1 to query GitHub for the newest release instead of using the pinned versions below.
[ -z "$UEFI_USE_LATEST" ] && UEFI_USE_LATEST=0

#Raspberry Pi 4 only; this setting is ignored for every other model.
#Disable the pftf 3 GB RAM limit during specialize after WoR-PE reboots.
[ -z "$PI4_AUTO_DISABLE_3GB" ] && PI4_AUTO_DISABLE_3GB=1
case "$PI4_AUTO_DISABLE_3GB" in
  0 | 1) ;;
  *) error "Unknown value for PI4_AUTO_DISABLE_3GB. Expected '0' or '1'.";;
esac

#Set to 1 to hide the Windows OOBE network and online-account screens, or 0 to require the standard flow. Adjustable in the GUI's Advanced Options window.
[ -z "$OOBE_NETWORK_BYPASS" ] && OOBE_NETWORK_BYPASS=1
if [ "$OOBE_NETWORK_BYPASS" != 0 ] && [ "$OOBE_NETWORK_BYPASS" != 1 ];then
  error "Unknown value for OOBE_NETWORK_BYPASS. Expected '0' or '1'."
fi

#Pinned versions. Used by default, or as a fallback when the GitHub API is unreachable.
[ -z "$UEFI_VER_PI3" ] && UEFI_VER_PI3='v1.39'
[ -z "$UEFI_VER_PI4" ] && UEFI_VER_PI4='v1.51'
[ -z "$UEFI_VER_PI5" ] && UEFI_VER_PI5='v0.3'

#Windows driver package. The upstream project is archived, so v0.17 is the final release.
[ -z "$DRIVERS_USE_LATEST" ] && DRIVERS_USE_LATEST=1
[ -z "$DRIVER_VER" ] && DRIVER_VER='v0.17'

#Set to 1 to run every step except writing to the drive.
[ -z "$DRY_RUN" ] && DRY_RUN=0
case "$DRY_RUN" in
  0 | 1) ;;
  *) error "Unknown value for DRY_RUN. Expected '0' or '1'.";;
esac

#Set to 1 to skip the post-flash written-image verification (partition, boot file, and checksum checks). Not recommended.
[ -z "$SKIP_IMAGE_VERIFICATION" ] && SKIP_IMAGE_VERIFICATION=0
case "$SKIP_IMAGE_VERIFICATION" in
  0 | 1) ;;
  *) error "Unknown value for SKIP_IMAGE_VERIFICATION. Expected '0' or '1'.";;
esac

#Set to 0 to leave the UEFI firmware package's own default config.txt in place instead of writing the CONFIG_TXT variable.
[ -z "$APPLY_CUSTOM_CONFIG_TXT" ] && APPLY_CUSTOM_CONFIG_TXT=1
case "$APPLY_CUSTOM_CONFIG_TXT" in
  0 | 1) ;;
  *) error "Unknown value for APPLY_CUSTOM_CONFIG_TXT. Expected '0' or '1'.";;
esac

#Set to 0 to show empty card-reader slots as selectable drives in WoR-PE. Written to the cached PE settings.ini before each run.
[ -z "$HIDE_EMPTY_DRIVES" ] && HIDE_EMPTY_DRIVES=1
case "$HIDE_EMPTY_DRIVES" in
  0 | 1) ;;
  *) error "Unknown value for HIDE_EMPTY_DRIVES. Expected '0' or '1'.";;
esac

#Set to 0 to skip TLS certificate verification, for systems with an outdated CA bundle.
[ -z "$VERIFY_TLS" ] && VERIFY_TLS=1

#Cache mode: 0 downloads components again every run, 1 reuses them while they are still the newest version, 2 reuses them without checking.
[ -z "$USE_CACHE" ] && USE_CACHE=1

#WoR PE-based installer. worproject.com redirects to a versioned asset on their GitHub mirror.
[ -z "$PE_USE_LATEST" ] && PE_USE_LATEST=1
[ -z "$PE_INSTALLER_URL" ] && PE_INSTALLER_URL='https://github.com/worproject/dldserv-mirror/releases/download/13%2F02%2F2024/WoR-PE_Package_1.1.0.zip'
[ -z "$PE_INSTALLER_SHA256" ] && PE_INSTALLER_SHA256='A039E28FE7E39147899B0634C15E336C3B26A6F76201092EBB9732474CD43D0A'

#Last Windows build that boots on the ARMv8.0 Pi3/Pi4; newer ones use ARMv8.1 atomics.
#Source: https://worproject.com/faq "Does Windows 11 work?"
[ -z "$ARMV80_MAX_BUILD" ] && ARMV80_MAX_BUILD=25163

#Windows build reference points.
[ -z "$WIN11_MIN_BUILD" ] && WIN11_MIN_BUILD=22000        #builds at or above this are Windows 11, below are Windows 10
[ -z "$WIN10_OLDEST_BUILD" ] && WIN10_OLDEST_BUILD='17134.112' #marks the end of the Windows 10 section of worproject.com's version list
[ -z "$EXAMPLE_BID" ] && EXAMPLE_BID='22621.525'          #shown to the user as an example of the expected build-number format
[ -z "$ARMV80_SAFE_BID" ] && ARMV80_SAFE_BID='22631.2861' #newest Windows 11 build suggested for the ARMv8.0 Pi3/Pi4

#Determine the directory that contains this script
[ -z "$DIRECTORY" ] && DIRECTORY="$(resolve_path "$(dirname "$0")")"

#clear the variable storing path to this script, if the folder does not contain a file named 'install-wor.sh'
[ ! -f "${DIRECTORY}/install-wor.sh" ] && DIRECTORY=''
IFS=$'\n'

#Self-updater target: which repo/ref this script compares its local git commit against, and pulls from.
[ -z "$UPDATE_REPO_URL" ] && UPDATE_REPO_URL='https://github.com/Botspot/wor-flasher'
[ -z "$UPDATE_REF" ] && UPDATE_REF='HEAD' #the branch/ref on UPDATE_REPO_URL to compare against, e.g. HEAD or refs/heads/main

#Set NO_UPDATE=0 to opt in to source-checkout updates. Packaged releases should use signed release updates instead.
[ -z "$NO_UPDATE" ] && NO_UPDATE=1

{ #check for updates and auto-update unless disabled via NO_UPDATE
if [ -e "$DIRECTORY" ] && [ "$NO_UPDATE" != 1 ] && command -v git >/dev/null && git -C "$DIRECTORY" rev-parse --git-dir >/dev/null 2>&1 ;then
  prepwd="$PWD"
  cd "$DIRECTORY" || error "Failed to open the WoR-flasher directory: $DIRECTORY"
  local_commit="$(git rev-parse HEAD)" #commit this local checkout at $DIRECTORY is on
  remote_commit="$(git ls-remote "$UPDATE_REPO_URL" "$UPDATE_REF" | awk 'NR == 1 {print $1}')" #latest commit on UPDATE_REPO_URL/UPDATE_REF

  if [ "$local_commit" != "$remote_commit" ] && [ ! -z "$remote_commit" ] && [ ! -z "$local_commit" ];then
    if ! git diff --quiet || ! git diff --cached --quiet ;then
      status "Skipping automatic update because this checkout has uncommitted changes."
    else
      status "Auto-updating wor-flasher for the latest features and improvements..."
      status "To disable this next time, set NO_UPDATE=1"
      sleep 1

      if git fetch --quiet "$UPDATE_REPO_URL" "$UPDATE_REF" && git merge --ff-only FETCH_HEAD ;then
        status "Update finished. Reloading script..."
        NO_UPDATE=1 "$0" "$@"
        exit $?
      else
        warning "Automatic update failed. Continuing..."
      fi
    fi
  fi
  cd "$prepwd" || error "Failed to return to the original directory: $prepwd"
fi
}

read_config_template() { #Input: path relative to config-templates/. Output: file contents.
  local path="$DIRECTORY/config-templates/$1"
  #continuing without one of these silently produces a blank config.txt or answer file, which is worse than stopping
  [ -s "$path" ] || error "Missing config-templates/$1
This file ships with WoR-Flasher and is required to write a bootable drive.
Update or re-clone your WoR-Flasher checkout, then run this script again."
  cat "$path"
}

mkdir -p "${DIRECTORY}/cache"

[ "$1" == 'source' ] && return 0 #If being sourced, exit here at this point in the script
#past this point, this script is being run, not sourced.

#A single entry point, without guessing: --gui hands over to the graphical front-end, which then runs
#this script again to do the work. Never auto-detect a display - DISPLAY is also set over SSH and in CI,
#and a tool that erases a disk must do exactly what it was asked to do.
if [ "$1" == '--gui' ];then
  shift
  [ -x "$DIRECTORY/install-wor-gui.sh" ] || error "No script found named install-wor-gui.sh
Both scripts must be in the same directory."
  exec "$DIRECTORY/install-wor-gui.sh" "$@"
fi
if [ "$1" == '--version' ] || [ "$1" == '-V' ];then
  printf 'WoR-Flasher %s\n' "$WOR_FLASHER_VERSION"
  exit 0
fi
if [ "$1" == '--help' ] || [ "$1" == '-h' ];then
  cat <<HELP
WoR-Flasher $WOR_FLASHER_VERSION
Usage: $(basename "$0") [--gui]

  (no arguments)  run the interactive text-mode installer
  --gui           run the graphical front-end instead
  --version       print the version and exit
  --help          show this message

Every setting can also be supplied as an environment variable; see the README.
HELP
  exit 0
fi
[ -n "$1" ] && error "Unknown argument '$1'. Run '$(basename "$0") --help' for usage."

require_linux_host

#Ensure this script's parent directory is valid
[ ! -e "$DIRECTORY" ] && error "$(basename "$0"): Failed to determine the directory that contains this script. Try running this script with full paths."

#Guarantee a clean stop with no further steps on Ctrl+C or a termination signal, same as any other error.
trap 'error "Interrupted."' INT TERM

LANG=C
LC_ALL=C
LANGUAGE=C
ANSI_CYAN=$'\e[96m'
ANSI_RESET=$'\e[0m'

setup || exit 1

#ask for the password while the screen is still clear; the GUI holds its progress window back until this returns
[ "$RUN_MODE" == gui ] && gui_preauthenticate

#Create folder to download everything to
mkdir -p "$DL_DIR"
cd "$DL_DIR" || error "Failed to open the download directory: $DL_DIR"

#We are about to download the PE installer, drivers, firmware, and likely a multi-GB Windows image.
#Fail now with a clear message instead of wasting time and ending up with a partial, unusable cache.
required_download_space=$((2 * 1024 * 1024 * 1024))
if [ ! -d "$PWD/peinstaller" ] || [ "$USE_CACHE" != 2 ];then
  required_download_space=$((required_download_space + 256 * 1024 * 1024))
fi
if [ ! -d "$PWD/driverpackage" ] || [ "$USE_CACHE" != 2 ];then
  required_download_space=$((required_download_space + 256 * 1024 * 1024))
fi
if [ ! -d "$PWD/pi${RPI_MODEL}-uefipackage" ] || [ "$USE_CACHE" != 2 ];then
  required_download_space=$((required_download_space + 192 * 1024 * 1024))
fi
require_free_space "$required_download_space" "$DL_DIR"

#unless specified otherwise, run this script in cli mode
[ -z "$RUN_MODE" ] && RUN_MODE=cli #RUN_MODE=gui

if [ "$USE_CACHE" != 0 ] && [ "$USE_CACHE" != 1 ] && [ "$USE_CACHE" != 2 ];then
  error "Unknown value for USE_CACHE. Expected '0', '1' or '2'."
fi

if [ "$USE_CACHE" == 0 ];then
  status "USE_CACHE=0: clearing cached components so everything is fetched again"
  clear_cached_components
fi

{ #choose windows version
using_esd=true #indicate that ESD download is required - will be changed to false otherwise
if [ -f "${DL_DIR}/winfiles_from_iso_${BID}_${WIN_LANG}/alldone" ];then
  #If pre-provided DL_DIR, BID and WIN_LANG show that a previously extracted ISO will be used, don't use ESD
  using_esd=false
fi

if [ -z "$BID" ];then

  while [ -z "$BID" ];do
    echo -ne "\nChoose Windows version:
\e[96m1\e[0m) Windows 11
\e[96m2\e[0m) Windows 10
\e[96m3\e[0m) More options...
Enter \e[96m1\e[0m, \e[96m2\e[0m or \e[96m3\e[0m: "
    read reply

    case $reply in
      1 | 2)
        #latest Windows 10/11 chosen
        echo -e "\nFinding newest build..."

        if [ "$reply" == 1 ];then
          #Windows 11
          BID="$(get_bid 11)" || exit 1
        elif [ "$reply" == 2 ];then
          #Windows 10
          BID="$(get_bid 10)" || exit 1
        fi
        ;;
      3)
        #more options
        while true;do

          #Discover past extracted ISO files so user does not need to keep ISO
          num_opts=3 #default number of options already in the "Additional options" menu
          add_options='' #Store additional options to display to the user
          available_extracted_isos="$(list_cached_winfiles "$PWD" | grep '^winfiles_from_iso_' | sed 's/^winfiles_from_iso_//g' | sort)"

          for folder in $available_extracted_isos ;do
            BID="$(echo "$folder" | awk -F_ '{print $1}')"
            WIN_LANG="$(echo "$folder" | awk -F_ '{print $2}')"
            add_options+="\n\e[96m${num_opts}\e[0m) Use $(get_os_name "$BID") $WIN_LANG (extracted from your ISO last time)"
            num_opts=$((num_opts+1))
          done
          unset BID WIN_LANG #Avoid leaving these variables set from the loop

          echo -ne "\nAdditional options:
\e[96m1\e[0m) Enter an exact Windows version to download
\e[96m2\e[0m) Use a Windows ISO file${add_options}
\e[96m$num_opts\e[0m) Go back
$([ $num_opts == 3 ] && echo 'Enter \e[96m1\e[0m, \e[96m2\e[0m or \e[96m3\e[0m: ' || echo 'Enter a number: ')"
          read reply

          case $reply in
            1) #Enter an exact Windows version to download
              echo -e "\nFinding builds..."

              #versions=''
              list_bids 10 >/dev/null #set $versions globally so it is not downloaded twice
              list_bids_supported 11 | sed "s/ /${ANSI_RESET} /g" | sed "s/^/Windows 11 ${ANSI_CYAN}/g"
              list_bids_supported 10 | sed "s/ /${ANSI_RESET} /g" | sed "s/^/Windows 10 ${ANSI_CYAN}/g"

              read -p $'\nFrom the list above, enter a Windows version number: ' BID
              if (list_bids_supported 11 ; list_bids_supported 10) | awk '{print $1}' | grep -qFx "$BID" ;then
                break #exit the while loop
              else
                echo_red "Invalid answer. Expected to see something like '${EXAMPLE_BID}'. Try again."
              fi
              ;;

            2) #Use a Windows ISO file
              while [ -z "$SOURCE_FILE" ];do
                read -p $'\nEnter the full path to a Windows 10/11 ARM64 ISO file: ' SOURCE_FILE
                if [ -z "$SOURCE_FILE" ];then
                  break #exit ISO file menu
                elif ! iso_problem="$(validate_iso_file "$SOURCE_FILE")" ;then
                  echo_red "$iso_problem"
                  SOURCE_FILE=''
                else #ISO file looks good
                  #Infer Build ID based on filename of ISO
                  BID="$(bid_from_iso_name "$SOURCE_FILE")"
                  if [ -z "$BID" ];then
                    read -p $'\nTo store files from this ISO, this script needs to know the Windows build number of this ISO.\nPlease enter it now: (example: '"$EXAMPLE_BID"') ' BID
                    [ -z "$BID" ] && error "Cannot proceed without a build number for your ISO file."
                  fi
                  #Infer language based on filename of ISO
                  WIN_LANG="$(lang_from_iso_name "$SOURCE_FILE")"
                  if [ -z "$WIN_LANG" ];then
                    read -p $'\nTo store files from this ISO, this script needs to know the language of this Windows ISO.\nPlease enter it now: (example: en-us) ' WIN_LANG
                    if [ -z "$WIN_LANG" ];then
                      error "Cannot proceed without a language for your ISO file."
                    elif ! is_known_win_lang "$WIN_LANG" ;then
                      error "Language code was not found in the list!\n$(list_langs | awk '{print $1}')"
                    fi
                  fi
                  using_esd=false #indicate that ESD will not be downloaded
                fi
              done

              if [ ! -z "$SOURCE_FILE" ];then
                break #exit "more options" menu
              fi
              ;;
            "$num_opts")
              #go back
              break
              ;;
            *)
              #any number chosen other than 1,2,3: could be an additional option to choose pre-extracted ISO
              if [ "$reply" -le $num_opts ];then
                i="$(echo "$available_extracted_isos" | sed -n $((reply-2))p)"
                BID="$(echo "$i" | awk -F_ '{print $1}')"
                WIN_LANG="$(echo "$i" | awk -F_ '{print $2}')"
                using_esd=false #indicate that ESD will not be downloaded
                break
              else
                echo_red "Invalid option ${reply}.$([ $num_opts == 3 ] && echo " Expected '1', '2' or '3'.")"
              fi
              ;;
          esac
        done #End of loop for more options when choosing OS
        ;;
      *) echo_red "Invalid answer '${reply}'. Expected '1', '2' or '3'.";;
    esac
  done

  echo "Selected version: $(get_os_name "$BID") $WIN_LANG"
else

  #Verify SOURCE_FILE value provided to script
  if [ ! -z "$SOURCE_FILE" ];then
    iso_problem="$(validate_iso_file "$SOURCE_FILE")" || error "Specified ISO file '$SOURCE_FILE': $iso_problem"
  #Verify BID value provided to script
  elif [ "$using_esd" == true ] && ! (list_bids 10 ; list_bids 11) | awk '{print $1}' | grep -Fqx "$BID" ;then
    error "Build ID '$BID' not found on list of available ones."
  fi
fi
}

{ #choose language
if [ -z "$WIN_LANG" ];then
  default_language="$(default_win_lang)"
  #list languages and highlight the language codes
  echo
  list_langs | sed "s/^/${ANSI_CYAN}/g" | sed "s/:/${ANSI_RESET} - /g" | sort

  while true; do
    read -p $'\nFrom the list above, enter a language ['"$default_language"']: ' WIN_LANG
    [ -z "$WIN_LANG" ] && WIN_LANG="$default_language"

    if is_known_win_lang "$WIN_LANG" ;then
      #if selected language matches line in language list
      break
    else
      echo_red "Invalid answer. Expected to see something like 'en-us'. Try again."
    fi

  done

#Verify WIN_LANG value provided to script
elif ! is_known_win_lang "$WIN_LANG" ;then
  error "Invalid WIN_LANG value '$WIN_LANG'.\nAvailable languages:\n$(list_langs | awk -F: '{print $1}')"
fi
}

{ #choose destination RPi model
if [ -z "$RPI_MODEL" ];then
  while true; do
    echo -ne "\nChoose Raspberry Pi model to deploy Windows on:
\e[96m1\e[0m) Raspberry Pi 5
\e[96m2\e[0m) Raspberry Pi 4 / 400
\e[96m3\e[0m) Raspberry Pi 3 or Pi2 v1.2
Enter \e[96m1\e[0m, \e[96m2\e[0m, or \e[96m3\e[0m: "
    read reply
    case $reply in
      1)
        RPI_MODEL=5
        break
        ;;
      2)
        RPI_MODEL=4
        break
        ;;
      3)
        RPI_MODEL=3
        break
        ;;
      *) echo_red "Invalid option '${reply}'. Expected '1', '2', or '3'.";;
    esac
  done
elif [ "$RPI_MODEL" != 3 ] && [ "$RPI_MODEL" != 4 ] && [ "$RPI_MODEL" != 5 ];then
  error "Unknown value for RPI_MODEL. Expected '3' or '4' or '5'."
fi
}

{ #make sure the chosen Windows build can run on the chosen Raspberry Pi
if ! cpu_supports_bid "$BID" ;then
  error "$(get_os_name "$BID") cannot run on a Raspberry Pi ${RPI_MODEL}.
Builds newer than ${ARMV80_MAX_BUILD} require an ARMv8.1 CPU, but the Pi 3 and Pi 4 are ARMv8.0.
Those builds hang immediately after the bootloader hands off to Windows.
Choose Windows 10, or a Windows 11 build such as ${ARMV80_SAFE_BID}."
fi
}

{ #choose output device
if [ -z "$DEVICE" ];then
  while true;do
    echo
    echo "Available devices:"
    list_devs || echo -e "\e[93mNone found - please insert a storage device and press Enter\e[0m"
    read -p "Choose a device to flash the Windows setup files to: " DEVICE
    if ! is_safe_target_device "$DEVICE";then
      echo_red "Device $DEVICE is your current boot drive! You cannot overwrite this drive."
    elif [ -b "$DEVICE" ];then
      break #exit loop
    elif [ -z "$DEVICE" ];then
      true #refresh list if user presses Enter
    else
      echo_red "Device $DEVICE is not a valid block device!"
    fi
  done

elif [ ! -b "$DEVICE" ];then
  error "Invalid value for DEVICE: block device $DEVICE does not exist. Available devices:\n$(list_devs)"
elif ! is_safe_target_device "$DEVICE";then
  error "Refusing to overwrite $DEVICE. Choose an external, physical, writable whole disk that is not the current boot drive."
fi
}

{ #CAN_INSTALL_ON_SAME_DRIVE
device_capability="$(drive_capability "$DEVICE")"
validate_install_mode "$device_capability"

if [ -z "$CAN_INSTALL_ON_SAME_DRIVE" ] && [ "$device_capability" == install ];then
  #Drive is >=25GB, so present the user with the option to make this a recovery drive or a full installation

  while true; do
    echo -ne "\nWould you like to:
\e[96m1\e[0m) Create an installation drive capable of installing Windows to itself
\e[96m2\e[0m) Create a recovery drive to install Windows on other >16 GB drives
Choose the installation mode (\e[96m1\e[0m or \e[96m2\e[0m): "
    read reply
    case $reply in
      1)
        CAN_INSTALL_ON_SAME_DRIVE=1
        break
        ;;
      2)
        CAN_INSTALL_ON_SAME_DRIVE=0
        break
        ;;
      *) echo_red "Invalid option '${reply}'. Expected '1' or '2'.";;
    esac
  done

elif [ -z "$CAN_INSTALL_ON_SAME_DRIVE" ];then
  #Drive is <25GB, so user's only choice is to make this a recovery drive
  status "Drive $DEVICE is too small to install Windows to itself. Using recovery-drive mode to install Windows on another larger device."
  CAN_INSTALL_ON_SAME_DRIVE=0
fi
}


#fail fast, before any downloads, if macOS partitioning can't proceed later
is_macos && { command -v sgdisk >/dev/null || error "sgdisk is required to partition $DEVICE correctly. Install it with 'brew install gptfdisk', then run this script again."; }

#the GUI already picked one and exported it; a CLI run gets the same shipped template, so both write identical media
set_default_config_txt

printf '\n\033[96m%s\033[0m - starting installation\n' "$WOR_APP_TITLE"
settings_summary_plain '  %-24s %s\n'
echo

STEP_NUM=0
[ "$RPI_MODEL" == 5 ] && STEP_TOTAL=7 || STEP_TOTAL=8

phase "Preparing the WoR PE-based installer"
if [ "$USE_CACHE" == 2 ] && [ -d "$PWD/peinstaller" ];then
  echo "Not downloading $PWD/peinstaller - using cache without checking for updates"
else
  #from: https://worproject.com/downloads#windows-on-raspberry-pe-based-installer
  URL=''
  EXPECTED_SHA256=''
  if [ "$PE_USE_LATEST" == 1 ];then
    EXPECTED_SHA256="$(wget -qO- https://worproject.com/dldserv/worpe/gethashlatest.php 2>/dev/null | cut -d ':' -f2)"
    if [ -z "$EXPECTED_SHA256" ];then
      echo_red "Failed to query the latest PE installer hash. Falling back to the pinned package."
    else
      URL='https://worproject.com/dldserv/worpe/downloadlatest.php'
    fi
  fi
  if [ -z "$URL" ];then
    URL="$PE_INSTALLER_URL"
    EXPECTED_SHA256="$PE_INSTALLER_SHA256"
  fi

  #the download URL is a redirector that never changes, so the hash identifies the version
  if cache_is_current "$PWD/peinstaller" "$EXPECTED_SHA256" ;then
    echo "Not downloading $PWD/peinstaller - cached copy is up to date"
  else
    status "Downloading WoR PE-based installer: $URL"
    wget "$URL" -O "$PWD/WoR-PE_Package.zip" || error "Failed to download the WoR PE-based installer.\nURL: $URL"

    if [ "$EXPECTED_SHA256" != "$(sha256_file "$PWD/WoR-PE_Package.zip" | tr '[a-z]' '[A-Z]')" ];then
      error "Downloaded PE-based installer does not match expected file"
    fi

    rm -rf "$PWD/peinstaller"
    unzip -q "$PWD/WoR-PE_Package.zip" -d "$PWD/peinstaller"
    if [ $? != 0 ];then
      rm -rf "$PWD/peinstaller"
      error "The unzip command failed to extract $PWD/WoR-PE_Package.zip"
    fi
    rm -f "$PWD/WoR-PE_Package.zip"
    mark_cache "$PWD/peinstaller" "$EXPECTED_SHA256" || error "Failed to record PE installer cache integrity."
    echo
  fi
fi

if [ "$RPI_MODEL" != 5 ];then
  phase "Preparing ARM64 drivers"
  if [ "$USE_CACHE" == 2 ] && [ -d "$PWD/driverpackage" ];then
    echo "Not downloading $PWD/driverpackage - using cache without checking for updates"
  else
    #from: https://github.com/worproject/RPi-Windows-Drivers/releases
    URL=''
    if [ "$DRIVERS_USE_LATEST" == 1 ];then
      URL="$(wget -qO- https://api.github.com/repos/worproject/RPi-Windows-Drivers/releases/latest 2>/dev/null | grep '"browser_download_url":'".*RPi${RPI_MODEL}_Windows_ARM64_Drivers_.*\.zip" | sed 's/^.*browser_download_url": "//g' | sed 's/"$//g')"
      [ -z "$URL" ] && echo_red "Failed to query the latest driver release. Falling back to pinned version ${DRIVER_VER}."
    fi
    [ -z "$URL" ] && URL="https://github.com/worproject/RPi-Windows-Drivers/releases/download/${DRIVER_VER}/RPi${RPI_MODEL}_Windows_ARM64_Drivers_${DRIVER_VER}.zip"

    if cache_is_current "$PWD/driverpackage" "$URL" ;then
      echo "Not downloading $PWD/driverpackage - cached copy is up to date"
    else
      status "Downloading ARM64 drivers: $URL"
      wget -O "$PWD/RPi${RPI_MODEL}_Windows_ARM64_Drivers.zip" "$URL" || error "Failed to download driver package"

      rm -rf "$PWD/driverpackage"
      unzip -q "$PWD/RPi${RPI_MODEL}_Windows_ARM64_Drivers.zip" -d "$PWD/driverpackage"
      if [ $? != 0 ];then
        rm -rf "$PWD/driverpackage"
        error "The unzip command failed to extract $PWD/RPi${RPI_MODEL}_Windows_ARM64_Drivers.zip"
      fi

      rm -f "$PWD/RPi${RPI_MODEL}_Windows_ARM64_Drivers.zip"
      mark_cache "$PWD/driverpackage" "$URL" || error "Failed to record driver cache integrity."
      echo
    fi
  fi

  if [ "$RPI_MODEL" == 4 ];then
    for driver_file in \
      bcmgenet/bcmgenet.inf \
      bcmgenet/bcmgenet.cat \
      mcci_dwchsotg/mcci_dwchsotg_hcd.inf \
      mcci_dwchsotg/mcci_dwchsotg_hcd.cat \
      mcci_dwchsotg/mcci_dwchsotg_hub.inf \
      mcci_dwchsotg/mcci_dwchsotg_hub.cat \
      mcci_dwchsotg/arm64/mcci_dwchsotg_hcd.sys \
      mcci_dwchsotg/arm64/mcci_dwchsotg_hub.sys \
      rpiuxflt/rpiuxflt.inf \
      rpiuxflt/rpiuxflt.cat \
      rpiuxflt/rpiuxflt.sys
    do
      [ -s "$PWD/driverpackage/$driver_file" ] || error "Pi 4 driver package is incomplete: $driver_file is missing or empty. Clear the driver cache and run WoR-Flasher again."
    done
  fi
fi

phase "Preparing Pi${RPI_MODEL} UEFI firmware"
if [ "$USE_CACHE" == 2 ] && [ -d "$PWD/pi${RPI_MODEL}-uefipackage" ];then
  echo "Not downloading $PWD/pi${RPI_MODEL}-uefipackage - using cache without checking for updates"
else
  #from: https://github.com/pftf/RPi4/releases
  case "$RPI_MODEL" in
    5)
      UEFI_REPO='worproject/rpi5-uefi'
      UEFI_VER="$UEFI_VER_PI5"
      PINNED_URL="https://github.com/${UEFI_REPO}/releases/download/${UEFI_VER}/RPi5_UEFI_Release_${UEFI_VER}.zip"
      ;;
    4)
      UEFI_REPO='pftf/RPi4'
      UEFI_VER="$UEFI_VER_PI4"
      PINNED_URL="https://github.com/${UEFI_REPO}/releases/download/${UEFI_VER}/RPi4_UEFI_Firmware_${UEFI_VER}.zip"
      ;;
    3)
      UEFI_REPO='pftf/RPi3'
      UEFI_VER="$UEFI_VER_PI3"
      PINNED_URL="https://github.com/${UEFI_REPO}/releases/download/${UEFI_VER}/RPi3_UEFI_Firmware_${UEFI_VER}.zip"
      ;;
  esac

  URL=''
  if [ "$UEFI_USE_LATEST" == 1 ];then
    #the 'latest' endpoint skips pre-releases, which upstream uses to mark known-bad builds
    URL="$(wget -qO- "https://api.github.com/repos/${UEFI_REPO}/releases/latest" 2>/dev/null | grep '"browser_download_url":' | grep -o 'https://[^"]*\.zip' | head -n1)"
    [ -z "$URL" ] && echo_red "Failed to query the latest UEFI release for ${UEFI_REPO}. Falling back to pinned version ${UEFI_VER}."
  fi
  [ -z "$URL" ] && URL="$PINNED_URL"

  if cache_is_current "$PWD/pi${RPI_MODEL}-uefipackage" "$URL" ;then
    echo "Not downloading $PWD/pi${RPI_MODEL}-uefipackage - cached copy is up to date"
  else
    status "Downloading Pi${RPI_MODEL} UEFI firmware: $URL"
    rm -rf "$PWD/pi${RPI_MODEL}-uefipackage" "$PWD/uefipackage" "$PWD/RPi${RPI_MODEL}_UEFI_Firmware.zip"

    wget -O "$PWD/RPi${RPI_MODEL}_UEFI_Firmware.zip" "$URL" || error "Failed to download UEFI package"

    unzip -q "$PWD/RPi${RPI_MODEL}_UEFI_Firmware.zip" -d "$PWD/pi${RPI_MODEL}-uefipackage"
    if [ $? != 0 ];then
      rm -rf "$PWD/pi${RPI_MODEL}-uefipackage"
      error "The unzip command failed to extract $PWD/RPi${RPI_MODEL}_UEFI_Firmware.zip"
    fi

    rm -f "$PWD/RPi${RPI_MODEL}_UEFI_Firmware.zip"
    mark_cache "$PWD/pi${RPI_MODEL}-uefipackage" "$URL" || error "Failed to record UEFI cache integrity."
    echo
  fi
fi

{ #Download Windows ESD if an ISO was not provided and one has not already been extracted

phase "Preparing the Windows image"
if [ ! -z "$SOURCE_FILE" ];then
  echo "Not downloading ESD image - using your ISO instead"

  #set folder name to store files from the ISO
  #files are stored in a folder specific to the OS version and language
  winfiles="winfiles_from_iso_${BID}_${WIN_LANG}"
  mkdir -p "$PWD/$winfiles"

elif [ -f "$PWD/winfiles_from_iso_${BID}_${WIN_LANG}/alldone" ];then
  echo "Not downloading ESD image - using a previously extracted ISO instead"
  winfiles="winfiles_from_iso_${BID}_${WIN_LANG}"

elif [ -f "$PWD/winfiles_${BID}_${WIN_LANG}/alldone" ];then
  echo "Not downloading ESD image - already extracted"
  winfiles="winfiles_${BID}_${WIN_LANG}"

else #Download and extract ESD

  #Get list of all Windows ESD releases for this Build ID from worproject.com
  if [ "$using_esd" == true ];then
    #Only do it if ESD download is required
    catalog="$(cache_downloader "https://worproject.com/dldserv/esd/getcatalog.php?build=$BID&arch=ARM64&edition=Professional")" || exit 1
  fi

  #Shorten catalog to only show the ESD for this language
  catalog="$(get_esd_catalog_entry "$(echo "$catalog" | sed 's/></>\n</g')" "$WIN_LANG")"

  #Get download link, size, and SHA1 hash for ESD
  URL="$(echo "$catalog" | grep '<FilePath>' -m 1 | sed 's/<FilePath>//g' | sed 's/<\/FilePath>//g')"
  SIZE="$(echo "$catalog" | grep '<Size>' -m 1 | sed 's/<Size>//g' | sed 's/<\/Size>//g')"
  SHA1="$(echo "$catalog" | grep '<Sha1>' -m 1 | sed 's/<Sha1>//g' | sed 's/<\/Sha1>//g')"
  SHA256="$(echo "$catalog" | grep '<Sha256>' -m 1 | sed 's/<Sha256>//g' | sed 's/<\/Sha256>//g')"

  #DL_DIR could be on a FAT partition, which is only OK if no files are larger than 4GB.
  #Make sure that the ESD is smaller than 4GB if DL_DIR is on FAT-type partition
  if [ "$SIZE" -ge $((4*1024*1024*1024)) ] && df -T "$DL_DIR" 2>/dev/null | grep -q 'fat' ;then
    error "The $DL_DIR directory is on a FAT32/FAT16/vfat partition. This type of partition cannot contain files larger than 4GB, however the Windows ESD image will be larger than that.\nPlease format the drive with an Ext4 partition, or use another drive."
  fi

  #set folder name to store files from the ESD
  #files are stored in a folder specific to the OS version and language
  winfiles="winfiles_${BID}_${WIN_LANG}"
  mkdir -p "$PWD/$winfiles"

  SOURCE_FILE="$PWD/$winfiles/image.esd"

  if [ -z "$URL" ] || [ -z "$SIZE" ] || ([ -z "$SHA1" ] && [ -z "$SHA256" ]);then
    error "One of URL, SIZE, or SHA1/SHA256 variables is empty!\nURL: $URL\nSIZE: $SIZE\nSHA1: $SHA1\nSHA256: $SHA256\nHere's the full catalog output: '$catalog'"
  fi

  #The ESD is usually the largest single download in the set, so do the exact free-space check here.
  require_free_space $((SIZE + 768 * 1024 * 1024)) "$DL_DIR"

  if [ -f "$SOURCE_FILE" ] && [ ! -z "$SHA1" ] && [ "$SHA1" == "$(sha1_file_with_progress cached-esd "$SOURCE_FILE")" ];then
    echo "Not downloading $SOURCE_FILE - file exists"
  elif [ -f "$SOURCE_FILE" ] && [ ! -z "$SHA256" ] && [ "$SHA256" == "$(sha256_file_with_progress cached-esd "$SOURCE_FILE")" ];then
    echo "Not downloading $SOURCE_FILE - file exists"
  else
    status "Downloading Windows ESD image"
    wget "$URL" -O "$PWD/$winfiles/image.esd" || error "Failed to download ESD image"
    status "Verifying downloaded image"
    if [ ! -z "$SHA1" ];then
      LOCAL_SHA1="$(sha1_file_with_progress downloaded-esd "$SOURCE_FILE")"
      if [ "$SHA1" != "$LOCAL_SHA1" ];then
        rm -f "$SOURCE_FILE"
        error "\nSuccessfully downloaded ESD image $SOURCE_FILE, but it appears to be corrupted. Please run this script again.\n(Expected SHA1 hash is $SHA1, but downloaded file has SHA1 hash $LOCAL_SHA1"
      fi
    elif [ ! -z "$SHA256" ];then
      LOCAL_SHA256="$(sha256_file_with_progress downloaded-esd "$SOURCE_FILE")"
      if [ "$SHA256" != "$LOCAL_SHA256" ];then
        rm -f "$SOURCE_FILE"
        error "\nSuccessfully downloaded ESD image $SOURCE_FILE, but it appears to be corrupted. Please run this script again.\n(Expected SHA256 hash is $SHA256, but downloaded file has SHA256 hash $LOCAL_SHA256"
      fi
    fi
    echo_green "Download verified"
  fi
fi
}

#Extract ESD or ISO to standardized locations in $DL_DIR
if [[ "$SOURCE_FILE" == *'.ESD' ]] || [[ "$SOURCE_FILE" == *'.esd' ]];then
  cd "$PWD/$winfiles" || error "Failed to access $PWD/$winfiles folder"

  status "Extracting $(basename "$SOURCE_FILE") to $PWD"
  #Extract first volume containing boot files
  wimextract "$SOURCE_FILE" 1 boot efi --dest-dir="$PWD/bootpart" || error "Failed to extract first partition of $SOURCE_FILE"

  #Create boot.wim file
  mkdir "$PWD/bootpart/sources"
  #Export WinPE & Setup editions to non-solid boot.wim
  wimexport "$SOURCE_FILE" 2 "$PWD/bootpart/sources/boot.wim" --compress=LZX || error "Failed to export WinPE edition to $PWD/bootpart/sources/boot.wim"
  wimexport "$SOURCE_FILE" 3 "$PWD/bootpart/sources/boot.wim" --compress=LZX --boot || error "Failed to export Setup edition to $PWD/bootpart/sources/boot.wim"

  #If using an external ESD file, make a copy before modifying it
  if [ "$SOURCE_FILE" != "$PWD/image.esd" ];then
    copy_local_file_with_progress image.esd "$SOURCE_FILE" "$PWD/image.esd" || error "Failed to copy the ESD to $PWD/image.esd"
    SOURCE_FILE="$PWD/image.esd"
  fi
  #Remove first 3 partitions from ESD file
  wimdelete "$SOURCE_FILE" 1 --soft || error "Failed to remove a partition from $SOURCE_FILE"
  wimdelete "$SOURCE_FILE" 1 --soft || error "Failed to remove a partition from $SOURCE_FILE"
  wimdelete "$SOURCE_FILE" 1 --soft || error "Failed to remove a partition from $SOURCE_FILE" #remove --soft for this last one to minimize filesize
  mv -f "$SOURCE_FILE" "$PWD/install.wim" || error "Failed to rename $SOURCE_FILE to install.wim"

  touch "$PWD/alldone" #mark this folder of microsoft stuff as complete

  #Change working directory back to $DL_DIR
  cd ..

elif [[ "$SOURCE_FILE" == *'.ISO' ]] || [[ "$SOURCE_FILE" == *'.iso' ]];then
  cd "$PWD/$winfiles" || error "Failed to access $PWD/$winfiles folder"

  status "Mounting $(basename "$SOURCE_FILE")"
  isomount="$PWD/isomount"
  if is_macos ;then
    darwin_mount_iso "$SOURCE_FILE" || error "Failed to mount ISO file $SOURCE_FILE with hdiutil."
    isomount="$ISO_MOUNTPOINT"
    register_device_cleanup "$ISO_DEVICE"
  else
    mkdir -p "$isomount" || error "Failed to make $isomount folder"
    sudo umount "$isomount" 2>/dev/null
    sudo mount "$SOURCE_FILE" "$isomount" 2>/dev/null
    if [ $? != 0 ];then
      status "Failed to mount the ISO file. Trying again after loading the 'udf' kernel module."
      sudo modprobe udf

      if [ $? != 0 ];then
        modprobe_failed=1
      else
        modprobe_failed=0
      fi

      sudo mount "$SOURCE_FILE" "$isomount"
      if [ $? != 0 ];then
        if [ "$modprobe_failed" == 1 ] && [ ! -d "/lib/modules/$(uname -r)" ];then
          error "The 'udf' kernel module is required to mount $SOURCE_FILE, but all kernel modules are missing. Most likely, you upgraded kernel packages and have not rebooted yet. Try rebooting."
        else
          error "Failed to mount ISO file $SOURCE_FILE to $isomount"
        fi
      fi
    fi
    register_mount_cleanup "$isomount"
  fi

  mkdir -p "$PWD"/bootpart
  status "Copying files from ISO file to $PWD:"
  echo "  - Startup environment"
  copy_startup_environment_with_progress "$isomount" "$PWD/bootpart" local || error "Failed to copy the startup environment from $isomount"
  if [ -f "$PWD/isomount/sources/install.wim" ];then
    install_image="$PWD/isomount/sources/install.wim"
  elif [ -f "$PWD/isomount/sources/install.esd" ];then
    install_image="$PWD/isomount/sources/install.esd"
  else
    error "The ISO file does not contain sources/install.wim or sources/install.esd. Use an official Windows ARM64 ISO."
  fi
  echo "  - $(basename "$install_image")"
  copy_local_file_with_progress "$(basename "$install_image")" "$install_image" "$PWD/install.wim" || error "Failed to copy $install_image to $PWD/install.wim"

  touch "$PWD/alldone" #mark this folder of microsoft stuff as complete

  echo "All necessary files have been copied out. Your ISO file will not be needed for future flashes."

  status "Unmounting ISO file"
  if is_macos ;then
    sudo hdiutil detach "$ISO_DEVICE" || echo_red "Warning: failed to detach $ISO_DEVICE"
  else
    sudo umount "$isomount" || echo_red "Warning: failed to unmount $isomount" #failure is non-fatal
    rmdir "$isomount" #remove mountpoint
  fi

  #Change working directory back to $DL_DIR
  cd ..
fi

if [ "$DRY_RUN" == 1 ];then
  status "Exiting the $WOR_APP_TITLE script now because DRY_RUN=1 was set."
  cli_pause
  exit 0
fi

#now that downloads are complete, check again if destination storage is accessible
if [ ! -b "$DEVICE" ];then
  error "Device $DEVICE is not a valid block device! Available devices:\n$(list_devs)"
fi

if is_macos ;then
  darwin_flash_device
  exit $?
fi

echo
phase "Partitioning and formatting $DEVICE"
printf '  There is no turning back now.\n' 1>&2
sync
partitions=()
while IFS= read -r partition ;do
  [ -n "$partition" ] && partitions+=("$partition")
done < <(get_partition "$DEVICE" all)
[ "${#partitions[@]}" -eq 0 ] || sudo umount -ql "${partitions[@]}"
sync
status "Creating partition table"
sudo parted -s "$DEVICE" mklabel gpt || error "Failed to make GPT partition table on ${DEVICE}!"
sync
status "Generating partitions"
sudo parted -s "$DEVICE" mkpart primary 1MB 1000MB || error "Failed to make 1GB primary partition 1 on ${DEVICE}!"
sudo parted -s "$DEVICE" set 1 esp on || error "Failed to enable the EFI System Partition flag on $DEVICE partition 1"
sync
if [ $CAN_INSTALL_ON_SAME_DRIVE == 1 ];then
  sudo parted -s "$DEVICE" mkpart primary 1000MB 19000MB || error "Failed to make 19GB primary partition 2 on ${DEVICE}!"
else
  sudo parted -s "$DEVICE" mkpart primary 1000MB 7000MB || error "Failed to make 7GB primary partition 2 on ${DEVICE}!"
fi
sudo parted -s "$DEVICE" set 2 msftdata on || error "Failed to enable msftdata flag on $DEVICE partition 2"
sync

status "Generating filesystems"
PART1="$(get_partition "$DEVICE" 1)"
PART2="$(get_partition "$DEVICE" 2)"
echo "Partition 1: $PART1, Partition 2: $PART2"

errors="$(sudo mkfs.fat -F 32 -n WOR_BOOT "$PART1" 2>&1)" || error "Failed to create FAT partition on $PART1\nErrors:\n$errors"
errors="$(sudo mkfs.exfat -n WOR_INSTALL "$PART2" 2>&1)" || error "Failed to create EXFAT partition on $PART2\nErrors:\n$errors"

mntpnt="/media/$USER/wor-flasher"
status "Mounting ${DEVICE} device to $mntpnt"
sudo mkdir -p "$mntpnt"/bootpart || error "Failed to create mountpoint: $mntpnt/bootpart"
sudo mkdir -p "$mntpnt"/winpart || error "Failed to create mountpoint: $mntpnt/winpart"
sudo umount -q "$mntpnt"/bootpart
sudo umount -q "$mntpnt"/winpart
sudo mount "$PART1" "$mntpnt"/bootpart || error "Failed to mount $PART1 to $mntpnt/bootpart"
sudo mount.exfat-fuse "$PART2" "$mntpnt"/winpart
if [ $? != 0 ];then
  status "Failed to mount $PART2. Trying again after loading the 'fuse' kernel module."
  sudo modprobe fuse

  if [ $? != 0 ];then
    modprobe_failed=1
  else
    modprobe_failed=0
  fi

  sudo mount.exfat-fuse "$PART2" "$mntpnt"/winpart
  if [ $? != 0 ];then
    if [ "$modprobe_failed" == 1 ] && [ ! -d "/lib/modules/$(uname -r)" ];then
      error "The 'fuse' kernel module is required to mount $PART2 to $mntpnt/winpart, but all kernel modules are missing! Most likely, you upgraded kernel packages and have not rebooted yet. Try rebooting."
    else
      error "Failed to mount $PART2 to $mntpnt/winpart"
    fi
  fi
fi
#unmount device partitions on exit
register_mount_cleanup "$mntpnt/bootpart"
register_mount_cleanup "$mntpnt/winpart"

phase "Copying files to $DEVICE:"
echo "  - Startup environment"
copy_startup_environment_with_progress "$PWD/$winfiles/bootpart" "$mntpnt/bootpart" || error "Failed to copy $PWD/$winfiles/bootpart to $mntpnt/bootpart"
echo "  - Installation files"
copy_file_with_progress install.wim "$PWD/$winfiles/install.wim" "$mntpnt/winpart/install.wim" || error "Failed to copy $PWD/$winfiles/install.wim to $mntpnt/winpart"
echo "  - EFI files"
sudo cp -r "$PWD/peinstaller/efi" "$mntpnt"/bootpart || error "Failed to copy $PWD/peinstaller/efi to $mntpnt/bootpart"

echo "  - PE installer"
configure_pe_settings_ini
configure_pe_prefinalize
sudo wimupdate "$mntpnt"/bootpart/sources/boot.wim 2 --command="add peinstaller/winpe/2 /" || error "The wimupdate command failed to add $PWD/peinstaller to boot.wim"

if [ "$RPI_MODEL" == 5 ];then
  #no wor drivers available for pi5, so make a dummy file to allow boot
  echo "  - ARM64 drivers"
  echo -n > "$PWD/critical"
  sudo wimupdate "$mntpnt"/bootpart/sources/boot.wim 2 --command="add critical /drivers/critical" || error "The wimupdate command failed to add $PWD/critical to boot.wim"
  rm "$PWD/critical"
else
  echo "  - ARM64 drivers"
  sudo wimupdate "$mntpnt"/bootpart/sources/boot.wim 2 --command="add driverpackage /drivers" || error "The wimupdate command failed to add $PWD/driverpackage to boot.wim"
fi

echo "  - Windows Setup configuration"
install_windows_setup_configuration "$mntpnt/bootpart" "$mntpnt/winpart" || error "Failed to install the Windows Setup configuration."

echo "  - UEFI firmware"
sudo cp -r "$PWD/pi${RPI_MODEL}-uefipackage"/* "$mntpnt"/bootpart || error "Failed to copy $PWD/pi${RPI_MODEL}-uefipackage to $mntpnt/bootpart"

if [ ! -z "$CONFIG_TXT" ] && [ "$APPLY_CUSTOM_CONFIG_TXT" == 1 ];then
  status "Customizing config.txt according to the CONFIG_TXT variable"
  echo "$CONFIG_TXT" | sudo tee "$mntpnt"/bootpart/config.txt >/dev/null
fi

if [ $RPI_MODEL == 3 ];then
  status "Applying GPT partition-table fix for the Pi3/Pi2"
  #According to @mariob, this patches the first sector of the disk to guide the bootloader into finding the fat32 partition
  #there's no other way of doing it on the pi 3 - hardware limitation
  sudo dd if=$PWD/peinstaller/pi3/gptpatch.img of="$DEVICE" conv=fsync || error "The 'dd' command failed to flash $PWD/peinstaller/pi3/gptpatch.img to $DEVICE"
fi

if [ "$SKIP_IMAGE_VERIFICATION" == 1 ];then
  echo_red "Skipping written-image verification (SKIP_IMAGE_VERIFICATION=1). This is not recommended."
else
  verify_written_image "$DEVICE" "$PART1" "$PART2" "$mntpnt/bootpart" "$mntpnt/winpart" "$PWD/$winfiles/install.wim"
fi

status "Ejecting drive $DEVICE"
sudo umount "$PART1" || echo_red "Warning: the umount command failed to unmount all partitions within $DEVICE"
sudo umount "$PART2" || echo_red "Warning: the umount command failed to unmount all partitions within $DEVICE"
sudo umount -q "$mntpnt"/bootpart &>/dev/null
sudo umount -q "$mntpnt"/winpart &>/dev/null
sudo eject "$DEVICE" &>/dev/null
sudo rmdir "$mntpnt"/bootpart "$mntpnt"/winpart || echo_red "Warning: Failed to remove the mountpoint folder: $mntpnt"
phase "$WOR_APP_TITLE script has completed."
cli_pause
