#!/bin/bash

#Shared mount, device and temporary-file cleanup used by both the CLI and GUI execution paths.
#Every register_* helper re-arms the EXIT trap, so an interrupted run still releases what it claimed.
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
