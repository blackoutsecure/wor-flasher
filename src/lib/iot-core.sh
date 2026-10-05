#!/bin/bash

#The engine sources these functions; neither GUI implements a second FFU deployment path.
is_iot_core() {
  [ "${WOR_IMAGE_FAMILY:-desktop}" == iot-core ]
}

validate_image_family() {
  case "${WOR_IOT_DOWNLOAD:-0}" in
    0 | 1) ;;
    *) warning "WOR_IOT_DOWNLOAD must be 0 or 1."; return 1 ;;
  esac
  case "${WOR_IMAGE_FAMILY:-desktop}:${WOR_IMAGE_ARCH:-}" in
    desktop: | desktop:arm64)
      [ "${WOR_IOT_DOWNLOAD:-0}" == 0 ] \
        || { warning "IoT image downloading requires the iot-core profile, not desktop ARM64."; return 1; }
      case "${SOURCE_FILE:-}" in
        *.[fF][fF][uU]) warning "FFU media cannot use the desktop ARM64 workflow. Select --iot-core for a supported Raspberry Pi IoT Core FFU."; return 1 ;;
      esac
      case "${WOR_TARGET_BOARD:-}" in
        pi2-v1.1) warning "Pi 2 v1.1 cannot run desktop ARM64 Windows. Select the ARM32 IoT Core workflow."; return 1 ;;
      esac
      return 0
      ;;
    iot-core: | iot-core:arm32) return 0 ;;
    iot-core:*)
      warning "Raspberry Pi Windows 10 IoT Core uses ARM32 images, including on Pi 3. ARM64 IoT Core is not a supported image profile."
      ;;
    *)
      warning "Unsupported image family/architecture: ${WOR_IMAGE_FAMILY:-desktop}/${WOR_IMAGE_ARCH:-auto}. Use desktop/arm64 or iot-core/arm32."
      ;;
  esac
  return 1
}

select_windows_family() { #Input: GUI Windows selection. Does not retain an image from another family.
  local family
  case "$1" in
    'Windows 10 IoT Core (ARM32, legacy)') family=iot-core ;;
    'Windows 10' | 'Windows 11' | 'More options') family=desktop ;;
    *) warning "Unrecognized Windows selection: $1"; return 1 ;;
  esac
  if [ "$family" != "${WOR_IMAGE_FAMILY:-desktop}" ];then
    SOURCE_FILE=''
    IOT_CORE_BUILD=''
    IOT_CORE_MINIMUM_BYTES=''
    IOT_CORE_SHA256=''
    IOT_CORE_PROFILE_JSON=''
    IOT_CORE_ACQUISITION=''
    IOT_CORE_INSPECTION_JSON=''
    WOR_IMAGE_ARCH=''
    WOR_IOT_DOWNLOAD=0
    GUI_IOT_SOURCE_MODE=''
    GUI_IOT_LOCAL_SOURCE=''
    iot_core_clear_target_approval
  fi
  WOR_IMAGE_FAMILY="$family"
  if is_iot_core;then
    CAN_INSTALL_ON_SAME_DRIVE=1
    SKIP_IMAGE_VERIFICATION=0
    WINDOWS_ACCOUNT_SETUP=0
    WINDOWS_ACCOUNT_PASSWORD=''
  fi
}

iot_core_guidance() {
  printf '%s\n' \
    'Windows 10 IoT Core (ARM32, legacy)' \
    'Choose the board and drive, review settings, then click Flash to download and verify the official Microsoft image. No image is downloaded or imported while choosing settings. Optional local ISO, MSI or FFU imports are in Advanced Options.' \
    'Supported boards: Pi 2 v1.1, Pi 2 v1.2 and Pi 3 Model B. Pi 3B+, Pi 4/400 and Pi 5 are not supported by this image profile.' \
    'This is an IoT/UWP system, not the Windows desktop. No ARM64 Windows binaries or desktop x86 emulation are installed.' \
    'The complete FFU supplies firmware, drivers and partitions. WoR-PE and desktop customization options do not apply.' \
    'Flash starts source preparation; disk writing waits for full verification and administrator approval. CLI downloads remain explicit. Microsoft license terms apply; the image is not redistributed with WoR-Flasher.' \
    'Use archived images only for isolated development. Hardware boot validation is still required.' \
    'Guide: https://learn.microsoft.com/previous-versions/windows/iot-core/tutorials/rpi'
}

iot_core_language() { #Explicit IoT language wins; otherwise reuse the desktop host locale preference.
  printf '%s\n' "${IOT_CORE_LANGUAGE:-${WINDOWS_LOCALE:-$(default_windows_locale)}}"
}

iot_core_language_options() { #Use the same offline language catalog/order as desktop Windows.
  list_langs_preferred | awk -F: '{print $1 "\t" $2}' \
    | windows_locale_from_language_code | awk -F'\t' '{print $1 "\t" $2 " (" $1 ")"}'
}

iot_core_personalization_requested() {
  [ "${IOT_CORE_ACCOUNT_SETUP:-0}" == 1 ] || [ "${IOT_CORE_LANGUAGE_SETUP:-1}" == 1 ]
}

iot_core_next_steps() {
  if [ "${DRY_RUN:-0}" == 1 ];then
    printf 'The FFU passed inspection only. No drive was written. Turn off dry run to apply and verify the image.'
  else
    printf 'Insert the microSD into %s, connect HDMI and wired Ethernet, then power on. Expect the IoT Core default app, not Windows desktop setup. Configure credentials and applications using the IoT Core tools; keep this legacy system on an isolated development network.' "$(rpi_board_label)"
    if iot_core_personalization_requested;then
      printf '\n\nIoT personalization is Pending after first boot. Requested language: %s%s. Finish the SSH personalization step; flash verification does not verify account or language configuration.' \
        "$(iot_core_language)" "$([ "${IOT_CORE_ACCOUNT_SETUP:-0}" == 1 ] && printf '; administrator change requested')"
      printf '\nFor a CLI run, use: python3 "%s/src/lib/iot-account.py" configure --interactive' "$DIRECTORY"
      [ "${IOT_CORE_LANGUAGE_SETUP:-1}" != 1 ] || printf ' --language %s' "$(iot_core_language)"
      [ "${IOT_CORE_ACCOUNT_SETUP:-0}" == 1 ] || printf ' --language-only'
    fi
  fi
}

iot_core_resolve_board() { #The explicit physical board owns the legacy image route, not desktop RPI_MODEL.
  case "${WOR_TARGET_BOARD:-}" in
    pi2-v1.1) RPI_MODEL=2 ;;
    pi2-v1.2 | pi3-b) RPI_MODEL=3 ;;
    *) warning "IoT Core requires target.board (WOR_TARGET_BOARD): pi2-v1.1, pi2-v1.2 or pi3-b. Other Pi models and revisions are not supported."; return 1 ;;
  esac
}

iot_core_load_profile() { #Reads only the pinned catalog; it does not inspect, import or download an image.
  local information
  iot_core_resolve_board || return 1
  information="$(python3 "$DIRECTORY/src/lib/iot-ffu.py" profile --board "$WOR_TARGET_BOARD" --json)" \
    || { warning "Cannot read the reviewed IoT Core board profile. No image or drive was modified."; return 1; }
  if ! jq -e --arg board "$WOR_TARGET_BOARD" '
    .architecture == "arm32" and .profile == "iot-core" and .format == "ffu-v1"
    and .trust == "pinned-bundled-profile" and .source_verification == "not-assessed"
    and (.recommended_boards | type == "array" and index($board) != null)
    and (.expected_build | type == "string" and length > 0)
    and (.expected_sha256 | type == "string" and test("^[a-f0-9]{64}$"))
    and (.minimum_disk_bytes | type == "number" and . > 0 and . == floor)
  ' >/dev/null <<<"$information";then
    warning "The reviewed IoT Core profile is incomplete or incompatible."
    return 1
  fi
  IOT_CORE_PROFILE_JSON="$information"
}

iot_core_minimum_bytes() { #Uses verified image geometry, or the reviewed expectation for a pre-download preview.
  local minimum="${IOT_CORE_MINIMUM_BYTES:-}"
  if [ -z "$minimum" ];then
    [ -n "${IOT_CORE_PROFILE_JSON:-}" ] || iot_core_load_profile || return 1
    minimum="$(jq -er '.minimum_disk_bytes' <<<"$IOT_CORE_PROFILE_JSON")" || return 1
  fi
  [[ "$minimum" =~ ^[0-9]+$ ]] && [ "$minimum" -gt 0 ] \
    || { warning "Cannot determine the IoT Core image's minimum capacity."; return 1; }
  printf '%s\n' "$minimum"
}

iot_core_accept_source_info() { #Input: validated worker metadata. Checks the profile and prior image approval.
  local information digest acquisition=local-ffu expected="${IOT_CORE_SHA256:-}"
  information="$1"
  if ! jq -e --arg board "$WOR_TARGET_BOARD" '
    .architecture == "arm32" and .profile == "iot-core" and .format == "ffu-v1"
    and .sector_size == 512 and .trust == "pinned-official-sha256"
    and (.recommended_boards | type == "array" and index($board) != null)
    and (.build | type == "string" and length > 0)
    and (.minimum_disk_bytes | type == "number" and . > 0 and . == floor)
    and (.sha256 | type == "string" and test("^[a-f0-9]{64}$"))
    and (.source_file | type == "string" and startswith("/"))
  ' >/dev/null <<<"$information";then
    warning "The IoT Core inspector returned incomplete or incompatible image metadata."
    return 1
  fi
  digest="$(jq -r .sha256 <<<"$information")"
  if [ -n "$expected" ] && [ "$digest" != "$expected" ];then
    warning "The selected IoT Core image changed after inspection. Re-select it and review the summary before flashing."
    return 1
  fi
  IOT_CORE_BUILD="$(jq -r .build <<<"$information")"
  IOT_CORE_MINIMUM_BYTES="$(jq -r .minimum_disk_bytes <<<"$information")"
  IOT_CORE_SHA256="$digest"
  SOURCE_FILE="$(jq -r .source_file <<<"$information")"
  [ -z "$expected" ] || acquisition="${IOT_CORE_ACQUISITION:-local-ffu}"
  IOT_CORE_ACQUISITION="$(jq -r --arg fallback "$acquisition" '.acquisition // $fallback' <<<"$information")"
  IOT_CORE_INSPECTION_JSON="$information"
  WOR_IOT_DOWNLOAD=0
  BID="$IOT_CORE_BUILD"
}

iot_core_inspect_source() { #Validates/imports the real image without opening any target.
  local information
  validate_image_family || return 1
  iot_core_resolve_board || return 1
  command -v python3 >/dev/null || { warning "Python 3 is required to validate IoT Core images."; return 1; }
  command -v jq >/dev/null || { warning "jq is required to read IoT Core image metadata."; return 1; }
  case "${WOR_IOT_DOWNLOAD:-0}" in
    0) ;;
    1)
      [ -z "$SOURCE_FILE" ] || { warning "Choose either a local IoT image or an official download, not both."; return 1; }
      status "Downloading or reusing and verifying the official Windows 10 IoT Core image"
      information="$(with_progress_capture python3 "$DIRECTORY/src/lib/iot-media.py" download --cache-dir "$DL_DIR/iot-core" --json)" \
        || { warning "IoT Core download or validation failed. No target was modified."; return 1; }
      iot_core_accept_source_info "$information"
      return
      ;;
    *) warning "WOR_IOT_DOWNLOAD must be 0 or 1."; return 1 ;;
  esac
  [ -f "$SOURCE_FILE" ] && [ -r "$SOURCE_FILE" ] \
    || { warning "IoT Core source does not exist or is unreadable: $SOURCE_FILE"; return 1; }
  SOURCE_FILE="$(resolve_path "$SOURCE_FILE")" || return 1
  case "$SOURCE_FILE" in
    *.[fF][fF][uU])
      printf 'Validating complete approved FFU: 0%%\n' >&2
      report_verification_task 0 'Verifying Windows 10 IoT Core FFU'
      information="$(python3 "$DIRECTORY/src/lib/iot-ffu.py" inspect "$SOURCE_FILE" --json)" \
        || { warning "IoT Core FFU validation failed. The target has not been modified."; return 1; }
      information="$(jq --arg source "$SOURCE_FILE" '. + {source_file: $source}' <<<"$information")" || return 1
      printf 'Validating complete approved FFU: 100%%\n' >&2
      report_verification_task 100 'Verified Windows 10 IoT Core FFU'
      ;;
    *.[iI][sS][oO] | *.[mM][sS][iI])
      status "Importing and verifying the official Windows 10 IoT Core package"
      information="$(with_progress_capture python3 "$DIRECTORY/src/lib/iot-media.py" prepare "$SOURCE_FILE" --cache-dir "$DL_DIR/iot-core" --json)" \
        || { warning "The IoT delivery package could not be validated or extracted. No target was modified."; return 1; }
      ;;
    *) warning "IoT Core accepts only its reviewed local FFU, ISO or MSI package, not desktop WIM/ESD or raw IMG files."; return 1 ;;
  esac
  iot_core_accept_source_info "$information"
}

iot_core_validate_options() {
  validate_image_family || return 1
  iot_core_resolve_board || return 1
  if [ "${CAN_INSTALL_ON_SAME_DRIVE:-1}" != 1 ];then
    warning "IoT Core applies a complete FFU to this drive; WoR recovery-drive mode is not supported."
    return 1
  fi
  if [ "${SKIP_IMAGE_VERIFICATION:-0}" != 0 ];then
    warning "IoT Core FFU validation and written-image verification cannot be skipped."
    return 1
  fi
  if [ "${WINDOWS_ACCOUNT_SETUP:-0}" == 1 ];then
    warning "Desktop account customization does not apply to IoT Core. Disable WINDOWS_ACCOUNT_SETUP and provision the IoT image separately."
    return 1
  fi
  case "${IOT_CORE_ACCOUNT_SETUP:-0}" in
    0 | 1) ;;
    *) warning "IOT_CORE_ACCOUNT_SETUP must be 0 or 1."; return 1 ;;
  esac
  case "${IOT_CORE_LANGUAGE_SETUP:-1}" in
    0 | 1) ;;
    *) warning "IOT_CORE_LANGUAGE_SETUP must be 0 or 1."; return 1 ;;
  esac
  local language
  language="$(iot_core_language)"
  iot_core_language_options | cut -f1 | grep -qiFx "$language" \
    || { warning "Choose a language from the shared Windows language catalog."; return 1; }
  if [ "${IOT_CORE_ACCOUNT_SETUP:-0}" == 1 ];then
    jq -n --arg username "${IOT_CORE_ACCOUNT_USERNAME:-Administrator}" \
      '{accountSetup: true, accountUsername: $username, accountPassword: ""}' \
      | python3 "$DIRECTORY/src/lib/iot-account.py" preferences --metadata-only >/dev/null \
      || { warning "Invalid post-boot IoT administrator preferences."; return 1; }
  fi
  iot_core_hdmi_state >/dev/null || return 1
  CAN_INSTALL_ON_SAME_DRIVE=1
}

iot_core_hdmi_mode() { #Explicit preferences win; 720p60 is the shared compatibility default.
  printf '%s\n' "${IOT_CORE_HDMI_MODE:-720p60}"
}

iot_core_hdmi_state() { #Input: optional mode/custom settings. Metadata-only validation shared by both GUIs.
  python3 "$DIRECTORY/src/lib/iot-ffu.py" hdmi-options \
    --hdmi-mode "${1:-$(iot_core_hdmi_mode)}" --hdmi-config "${2-${IOT_CORE_HDMI_CONFIG:-}}" --json \
    || { warning "Invalid IoT HDMI preferences. No image or drive was modified."; return 1; }
}

iot_core_hdmi_preflight() { #Validate the actual boot-file allocation before authorization or any target write.
  local mode
  mode="$(iot_core_hdmi_mode)"
  [ "$mode" != official ] || return 0
  status "Checking selected IoT HDMI settings against the verified boot configuration"
  python3 "$DIRECTORY/src/lib/iot-ffu.py" inspect "$SOURCE_FILE" \
    --hdmi-mode "$mode" --hdmi-config "${IOT_CORE_HDMI_CONFIG:-}" \
    || { warning "IoT HDMI settings cannot be applied safely to this boot configuration. No drive was written."; return 1; }
}

iot_core_backing_disks() { #Input: existing path. Output: its disks, including APFS/LVM ancestors.
  local root stores store parent kind path="${1:-/}" filesystem
  if is_macos;then
    if [ "$path" == / ];then
      filesystem=/
    else
      filesystem="$(stat -f '%Sd' "$path")" || return 1
      [[ "$filesystem" =~ ^disk[0-9]+(s[0-9]+)*$ ]] || return 1
      filesystem="/dev/$filesystem"
    fi
    root="$(darwin_device_value "$filesystem" '.ParentWholeDisk // .DeviceIdentifier')" || return 1
    printf '/dev/%s\n' "$root"
    stores="$(darwin_plist_json diskutil apfs list -plist)" || return 1
    stores="$(jq -er --arg root "$root" '[.Containers[]? | select(.ContainerReference == $root) | .PhysicalStores[]?.DeviceIdentifier] | join("\n")' <<<"$stores")" || return 1
    kind="$(darwin_device_value "/dev/$root" '.VirtualOrPhysical')" || return 1
    if [ "$kind" != Physical ] && [ -z "$stores" ];then
      return 1
    fi
    while IFS= read -r store;do
      [ -n "$store" ] || continue
      parent="$(darwin_device_value "/dev/$store" '.ParentWholeDisk // .DeviceIdentifier')" || return 1
      printf '/dev/%s\n' "$parent"
    done <<<"$stores"
  else
    root="$(findmnt -nro SOURCE --target "$path")" || return 1
    root="${root%%\[*}"
    lsblk -slnpo PATH "$root"
  fi
}

iot_core_boot_disks() {
  iot_core_backing_disks /
}

iot_core_protect_source_disks() {
  local path disks log_path parent paths=("$SOURCE_FILE" "$DIRECTORY/install-wor.sh")
  log_path="$(wor_log_file)"
  if [ "${1:-verified}" == preview ];then
    paths=("$DIRECTORY/install-wor.sh")
    [ -z "$SOURCE_FILE" ] || paths+=("$SOURCE_FILE")
    for parent in "$DL_DIR/iot-core" "$log_path";do
      while [ ! -e "$parent" ];do
        [ "$parent" != / ] || { warning "Cannot resolve the planned IoT cache/log storage."; return 1; }
        parent="$(dirname "$parent")"
      done
      paths+=("$parent")
    done
  else
    [ ! -f "$log_path" ] || paths+=("$log_path")
  fi
  for path in "${paths[@]}";do
    disks="$(iot_core_backing_disks "$path")" \
      || { warning "Cannot determine the disks backing $path; refusing to unmount or write the target."; return 1; }
    [ -n "$disks" ] || { warning "Disk ancestry for $path is empty; refusing to write."; return 1; }
    if grep -Fxq "$DEVICE" <<<"$disks";then
      warning "Refusing to erase $DEVICE because it contains the source image or running installer/log/cache: $path"
      return 1
    fi
  done
}

iot_core_clear_target_approval() {
  IOT_CORE_TARGET_ID=''
  IOT_CORE_TARGET_BYTES=''
  IOT_CORE_TARGET_LAYOUT=''
  WOR_IOT_CONFIRM_ERASE=0
}

iot_core_validate_device() {
  local bytes boot_disks sector_size layout identity minimum stage="${1:-verified}" target="$DEVICE"
  case "$stage" in preview | verified) ;; *) warning "Invalid IoT target validation stage: $stage"; return 1 ;; esac
  DEVICE="$(resolve_path "$DEVICE")" || { warning "Could not resolve the IoT Core target device."; return 1; }
  case "$DEVICE" in
    /dev/*) ;;
    *) warning "IoT Core requires a /dev whole-disk device, not a file path."; return 1 ;;
  esac
  boot_disks="$(iot_core_boot_disks)" \
    || { warning "Could not determine the boot volume backing disks; refusing to write."; return 1; }
  [ -n "$boot_disks" ] || { warning "The boot-disk safety check returned no devices; refusing to write."; return 1; }
  if grep -Fxq "$DEVICE" <<<"$boot_disks";then
    warning "Refusing to overwrite $DEVICE because it backs the current boot volume."
    return 1
  fi
  [ -b "$DEVICE" ] && is_safe_target_device "$DEVICE" \
    || { warning "Refusing IoT Core deployment to $DEVICE: select a safe, writable whole disk, not the host boot drive."; return 1; }
  if ! is_macos;then
    [ "$(lsblk -dnro TYPE "$DEVICE")" == disk ] && [ "$(lsblk -dnro RO "$DEVICE")" == 0 ] \
      || { warning "IoT Core target must be a writable whole disk, not a partition or virtual device."; return 1; }
    sector_size="$(lsblk -dnbo LOG-SEC "$DEVICE")" || return 1
    layout="$(lsblk -dnro PTTYPE "$DEVICE")" || { warning "Could not read the target partition-table type."; return 1; }
  else
    sector_size="$(darwin_device_value "$DEVICE" '.DeviceBlockSize')" || return 1
    layout="$(darwin_device_value "$DEVICE" '.Content // ""')" || { warning "Could not read the target partition-table type."; return 1; }
  fi
  [ "$sector_size" == 512 ] \
    || { warning "This IoT Core FFU requires a target with 512-byte logical sectors; detected $sector_size."; return 1; }
  bytes="$(get_size_raw "$DEVICE")" || return 1
  minimum="$(iot_core_minimum_bytes)" || return 1
  [[ "$bytes" =~ ^[0-9]+$ ]] \
    || { warning "Cannot determine IoT Core image or target-drive geometry."; return 1; }
  [ "$bytes" -ge "$minimum" ] && [ "$bytes" -le 2199023255552 ] \
    || { warning "IoT Core requires $minimum to 2199023255552 bytes; $DEVICE has $bytes bytes."; return 1; }
  iot_core_protect_source_disks "$stage" || return 1
  target="$DEVICE"
  is_macos && target="/dev/r${DEVICE#/dev/}"
  identity="$(python3 "$DIRECTORY/src/lib/iot-ffu.py" identify-target "$target" --json)" \
    || { warning "Could not obtain a stable identity for the target device."; return 1; }
  identity="$(jq -er '.target_id | select(type == "string" and test("^[0-9]+:[0-9]+:[0-9]+$"))' <<<"$identity")" \
    || { warning "Target identity is missing or malformed."; return 1; }
  if [ -n "${IOT_CORE_TARGET_ID:-}" ] && { [ "$identity" != "$IOT_CORE_TARGET_ID" ] || [ "$bytes" != "$IOT_CORE_TARGET_BYTES" ]; };then
    warning "The target device changed since it was selected. Re-select it and review the erase confirmation."
    return 1
  fi
  if [ -n "${IOT_CORE_TARGET_LAYOUT:-}" ] && [ "$layout" != "$IOT_CORE_TARGET_LAYOUT" ];then
    warning "The target partition layout changed since selection. Re-select the drive and review the erase confirmation."
    return 1
  fi
  IOT_CORE_TARGET_ID="$identity"
  IOT_CORE_TARGET_BYTES="$bytes"
  IOT_CORE_TARGET_LAYOUT="$layout"
}

iot_core_summary() {
  local expected_build='Not selected' minimum='Not Assessed' hdmi
  hdmi="$(iot_core_hdmi_state)" || return 1
  if [ -n "${IOT_CORE_PROFILE_JSON:-}" ];then
    expected_build="$(jq -r .expected_build <<<"$IOT_CORE_PROFILE_JSON")"
    minimum="$(jq -r .minimum_disk_bytes <<<"$IOT_CORE_PROFILE_JSON")"
  fi
  printf '%s version\t%s\n' "$WOR_FLASHER_NAME" "$WOR_FLASHER_VERSION"
  if [ -n "$DEVICE" ];then
    printf 'Target drive\t%s\n' "$(describe_device "$DEVICE")"
  else
    printf 'Target drive\tNot selected (image inspection only)\n'
  fi
  printf 'Target hardware\t%s\n' "$(rpi_board_label)"
  printf 'Operating system\tWindows 10 IoT Core ARM32 (legacy), build %s\n' "${IOT_CORE_BUILD:-$expected_build (planned)}"
  printf 'Installation mode\tApply complete FFU to this drive\n'
  printf 'Windows source\t%s\n' "${SOURCE_FILE:-Official Microsoft image (download after Flash)}"
  if [ -n "${IOT_CORE_SHA256:-}" ];then
    printf 'Image acquisition\t%s\n' "${IOT_CORE_ACQUISITION:-local-ffu}"
  else
    printf 'Image acquisition\t%s\n' "$([ "${WOR_IOT_DOWNLOAD:-0}" == 1 ] && printf 'Official download/cache after Flash' || printf 'Local import after Flash')"
  fi
  printf 'Download folder\t%s\n' "$DL_DIR"
  printf 'Image SHA-256\t%s\n' "${IOT_CORE_SHA256:-Not Assessed}"
  printf 'Minimum drive size\t%s bytes%s\n' "${IOT_CORE_MINIMUM_BYTES:-$minimum}" "$([ -n "${IOT_CORE_MINIMUM_BYTES:-}" ] || printf ' (planned profile; image verification pending)')"
  printf 'Current partition scheme\t%s\n' "${IOT_CORE_TARGET_LAYOUT:-Not selected}"
  printf 'Target layout\tFixed MBR/EBR from the FFU; 512-byte sectors, at most 2 TiB\n'
  printf 'Partition preparation\tAfter verified Flash: replace old GPT/MBR; erase existing data\n'
  printf 'Partition sizes\tPreserved from the FFU; larger cards have unused space\n'
  printf 'Firmware and drivers\tIncluded in the FFU; no ARM64 packages or WoR-PE\n'
  printf 'HDMI display\t%s\n' "$(jq -r '.label + (if .recommended then " (Recommended)" else "" end)' <<<"$hdmi")"
  printf 'Customization\t%s; no desktop answer file or recovery mode\n' \
    "$([ "$(iot_core_hdmi_mode)" == official ] && printf 'Image defaults' || printf 'Display-only boot configuration override')"
  printf 'IoT administrator setup\t%s\n' "$([ "${IOT_CORE_ACCOUNT_SETUP:-0}" == 1 ] \
    && printf 'Pending after first boot (%s); SSH identity and new login must be verified' "${IOT_CORE_ACCOUNT_USERNAME:-Administrator}" \
    || printf 'Image default; no account changes requested')"
  printf 'IoT language\t%s\n' "$([ "${IOT_CORE_LANGUAGE_SETUP:-1}" == 1 ] \
    && printf '%s (Pending after first boot; installed language must be verified)' "$(iot_core_language)" \
    || printf 'Image default; no language change requested')"
  if [ -n "${IOT_CORE_SHA256:-}" ];then
    printf 'Source validation\tVerified approved image; rechecked before writing\n'
  else
    printf 'Source validation\tNot Assessed; full verification after Flash, before writing\n'
  fi
  printf 'Verify written image\tRequired (cannot be skipped)\n'
  printf 'Play completion sound\t%s\n' "$([ "${PLAY_SOUND:-1}" == 1 ] && echo 'Yes' || echo 'No')"
  [ "${PLAY_SOUND:-1}" != 1 ] || printf 'Completion sound\t%s\n' "$(wor_completion_sound)"
  printf 'Show completion notification\t%s\n' "$([ "${SHOW_NOTIFICATION:-1}" == 1 ] && echo 'Yes' || echo 'No')"
  printf 'Recommendation\tOfficial Pi2/3 FFU on microSD; isolated development only\n'
  printf 'Dry run\t%s\n' "$([ "$DRY_RUN" == 1 ] && echo 'Yes (inspect only; no writes)' || echo 'No')"
  printf 'Log file\t%s\n' "$(wor_log_file)"
}

iot_core_apply() {
  local target="$DEVICE"
  [ "${WOR_IOT_CONFIRM_ERASE:-0}" == 1 ] && [ -n "${IOT_CORE_SHA256:-}" ] && [ -n "${IOT_CORE_TARGET_ID:-}" ] \
    || { warning "IoT writing and GPT cleanup require explicit erase consent and verified source/target identities."; return 1; }
  is_macos && target="/dev/r${DEVICE#/dev/}"
  sudo python3 "$DIRECTORY/src/lib/iot-ffu.py" apply "$SOURCE_FILE" "$target" \
    --target-size "$IOT_CORE_TARGET_BYTES" --expected-sha256 "$IOT_CORE_SHA256" \
    --expected-target-id "$IOT_CORE_TARGET_ID" --allow-gpt-cleanup \
    --hdmi-mode "$(iot_core_hdmi_mode)" --hdmi-config "${IOT_CORE_HDMI_CONFIG:-}"
}

iot_core_run_logged() {
  local log last temporary statuses=()
  if [ "$RUN_MODE" == gui ];then
    iot_core_run
    return
  fi
  log="$(wor_log_file)"
  mkdir -p "$(dirname "$log")" || error "Cannot create the IoT run-log directory."
  (umask 077; set -o noclobber; : > "$log") \
    || error "Cannot create a new IoT run log at $log. Refusing to overwrite an existing log; choose a new WOR_LOG_FILE."
  iot_core_run 2>&1 | tee "$log"
  statuses=("${PIPESTATUS[@]}")
  last="$(wor_last_log_file)"
  if [ "$last" != "$log" ];then
    if mkdir -p "$(dirname "$last")" && temporary="$(mktemp "$(dirname "$last")/.iot-last-log.XXXXXX")";then
      if ! cat "$log" > "$temporary" || ! mv -f "$temporary" "$last";then
        warning "Could not refresh $last. The IoT run log remains at $log."
        rm -f "$temporary"
      fi
    else
      warning "Could not create the last-run log. The IoT run log remains at $log."
    fi
  fi
  if [ "${statuses[1]}" != 0 ];then
    warning "IoT run-log recording failed; do not treat this run as successful."
    return 1
  fi
  return "${statuses[0]}"
}

iot_core_run() { #Separate full-image workflow; never reaches desktop download, formatting or verification.
  local choice boards=() label index partition partitions selected_status apply_status=0
  require_linux_host
  validate_image_family || error "Invalid IoT Core image profile."
  if [ -z "${WOR_TARGET_BOARD:-}" ] && [ -t 0 ];then
    iot_core_guidance
    while IFS= read -r label;do boards+=("$label");done < <(wor_rpi_board_options iot-core)
    for ((index=0; index<${#boards[@]}; index++));do printf '%d) %s\n' "$((index+1))" "${boards[$index]}";done
    read -r -p 'Choose an IoT Core board: ' choice
    [[ "$choice" =~ ^[1-3]$ ]] || error "Invalid IoT Core board selection."
    select_rpi_board "${boards[$((choice-1))]}" || error "Invalid IoT Core board selection."
  fi
  iot_core_validate_options || error "Invalid IoT Core installation settings."
  if [ -z "$SOURCE_FILE" ] && [ "${WOR_IOT_DOWNLOAD:-0}" == 0 ] && [ -t 0 ];then
    read -r -p 'Path to the official IoT FFU/ISO/MSI, or type download for the reviewed Microsoft image: ' SOURCE_FILE
    if [ "$SOURCE_FILE" == download ];then SOURCE_FILE=''; WOR_IOT_DOWNLOAD=1;fi
  fi
  iot_core_load_profile || error "Cannot use the reviewed IoT Core board profile."
  if [ "$DRY_RUN" != 1 ];then
    is_macos && require_macos_tools
    detect_root_dev
    if [ -z "$DEVICE" ];then
      [ -t 0 ] || error "DEVICE is required for non-interactive IoT Core deployment."
      choose_device
      selected_status=$?
      [ "$selected_status" == 2 ] && return 0
      [ "$selected_status" == 0 ] || return "$selected_status"
      iot_core_clear_target_approval
    fi
    iot_core_validate_device preview || error "The selected drive cannot receive this IoT Core image."
    if [ "$RUN_MODE" != gui ] && [ -t 0 ];then
      confirm_cli_installation
      selected_status=$?
      [ "$selected_status" == 2 ] && return 0
      [ "$selected_status" == 0 ] || return "$selected_status"
      WOR_IOT_CONFIRM_ERASE=1
    fi
    [ "${WOR_IOT_CONFIRM_ERASE:-0}" == 1 ] \
      || error "IoT Core deployment requires explicit erase confirmation. For automation set WOR_IOT_CONFIRM_ERASE=1 after verifying DEVICE; use DRY_RUN=1 to inspect first."
  fi
  STEP_NUM=0
  STEP_TOTAL=3
  phase "Preparing and verifying Windows 10 IoT Core image"
  iot_core_inspect_source || error "Cannot use this IoT Core image. No drive was written."
  iot_core_hdmi_preflight || error "Cannot apply the selected IoT HDMI settings. No drive was written."
  if [ "$DRY_RUN" == 1 ];then
    settings_summary_plain '  %-24s %s\n'
    status "IoT Core FFU inspection passed. DRY_RUN=1: no drive was opened for writing; hardware boot is not assessed."
    return 0
  fi
  iot_core_validate_device || error "The target changed during image preparation or cannot receive the verified image."
  authenticate_flash
  detect_root_dev
  iot_core_validate_device || error "The target changed or is no longer safe to overwrite."
  phase "Applying and verifying Windows 10 IoT Core"
  if is_macos;then
    diskutil unmountDisk "$DEVICE" || error "Could not unmount $DEVICE. Close applications using the target; keep the FFU on another drive."
  else
    partitions="$(lsblk -lnpo PATH "$DEVICE")" \
      || error "Could not enumerate the target partitions; refusing to write."
    [ -n "$partitions" ] || error "The target partition list was empty; refusing to write."
    while IFS= read -r partition;do
      if findmnt -rn -S "$partition" >/dev/null;then
        sudo umount "$partition" || error "Could not unmount $partition. Keep the FFU on another drive."
      fi
    done <<<"$partitions"
  fi
  emit_gui_progress "DISK_WRITE"$'\t'"1"$'\t'"$DEVICE"
  with_progress_capture iot_core_apply || apply_status=$?
  if [ "$apply_status" == 3 ];then
    error "IoT Core target validation failed. GPT cleanup requires valid, consistent headers/tables on the confirmed 512-byte-sector disk. No unvalidated GPT layout is erased. See the log for the exact failure. If writing started, do not boot the incomplete target."
  elif [ "$apply_status" != 0 ];then
    error "IoT Core application or read-back verification failed. The target is not verified bootable; do not treat this run as successful."
  fi
  phase "Ejecting verified IoT Core media"
  sync
  if is_macos;then
    diskutil eject "$DEVICE" || error "IoT Core was written and verified, but ejection failed. Eject the disk safely before removing it."
  else
    sudo eject "$DEVICE" || error "IoT Core was written and verified, but ejection failed. Eject the disk safely before removing it."
  fi
  status "IoT Core FFU written and verified. Boot the selected Pi from microSD; hardware boot remains unassessed."
}
