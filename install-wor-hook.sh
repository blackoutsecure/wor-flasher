#!/bin/bash

# Stable integration adapter for front-ends and automation.
# It exposes the install-wor.sh engine without owning a GUI or device workflow.

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd -P)"
ENGINE="$SCRIPT_DIR/install-wor.sh"

if [ ! -f "$ENGINE" ];then
  printf 'Missing install-wor.sh next to this adapter.\n' >&2
  exit 1
fi

if [ "${1:-}" == run ];then
  shift
  exec "$ENGINE" "$@"
fi

# shellcheck disable=SC1090
source "$ENGINE" source

case "${1:-}" in
  list-devices)
    require_linux_host
    list_dev_paths
    ;;
  describe-device)
    [ -n "${2:-}" ] || { printf 'Usage: %s describe-device DEVICE\n' "$0" >&2; exit 2; }
    describe_device "$2"
    ;;
  summary)
    settings_summary
    ;;
  *)
    cat >&2 <<USAGE
Usage: $(basename "$0") COMMAND

Commands:
  list-devices              Print safe whole-disk device paths, one per line.
  describe-device DEVICE    Print a human-readable device description.
  summary                   Print the current settings as label<TAB>value lines.
  run [ARGS...]             Run install-wor.sh with environment-provided settings.

Set the same variables documented in README.md before calling run, including
DEVICE, RPI_MODEL, BID, WIN_LANG and CAN_INSTALL_ON_SAME_DRIVE.
USAGE
    exit 2
    ;;
esac
