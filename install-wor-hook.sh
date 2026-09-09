#!/bin/bash

#Stable integration adapter for front-ends and automation.
#It exposes the install-wor.sh engine without owning a GUI or a device workflow.
#When distributed on its own it obtains a complete checkout, because the engine needs repository assets.

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd -P)"
ENGINE="$SCRIPT_DIR/install-wor.sh"

hook_export_assignment() { #Input: NAME=VALUE. Export one engine setting passed as a hook option.
  local assignment="$1" name
  name="${assignment%%=*}"
  case "$assignment" in
    *=*) ;;
    *) printf 'Invalid --set value. Expected NAME=VALUE.\n' >&2; exit 2 ;;
  esac
  case "$name" in
    ''|[!A-Za-z_]*|*[!A-Za-z0-9_]*) printf 'Invalid --set name: %s\n' "$name" >&2; exit 2 ;;
  esac
  export "$assignment"
}

hook_checkout_complete() { #Input: checkout directory. Output: success when every runtime file exists.
  local checkout_dir="$1"
  local required_path
  for required_path in \
    install-wor.sh \
    src/lib/metadata.sh src/lib/dependencies.sh src/lib/paths.sh src/lib/cleanup.sh src/lib/gui.sh \
    src/config/metadata.json src/config/metadata.schema.json \
    config-templates/config.json config-templates/config.schema.json \
    config-templates/pi3.config.txt config-templates/pi4.config.txt config-templates/pi5.config.txt \
    config-templates/pi4-ram-unlock.ps1 config-templates/pi4-ram-unlock-specialize.xml \
    config-templates/oobe-network-bypass.xml config-templates/prefinalize.cmd ;do
    [ -f "$checkout_dir/$required_path" ] || return 1
  done
}

if ! hook_checkout_complete "$SCRIPT_DIR" ;then
  : "${WOR_HOOK_REPOSITORY:=https://github.com/Botspot/wor-flasher.git}"
  #a released hook must clone a durable ref; working branches are deleted after merge
  : "${WOR_HOOK_REF:=main}"
  : "${WOR_HOOK_INSTALL_DIR:=${XDG_CACHE_HOME:-$HOME/.cache}/wor-flasher-hook}"
  ENGINE="$WOR_HOOK_INSTALL_DIR/install-wor.sh"
  if [ ! -f "$ENGINE" ];then
    command -v git >/dev/null || { printf 'git is required to obtain WoR-Flasher.\n' >&2; exit 1; }
    [ ! -e "$WOR_HOOK_INSTALL_DIR" ] || { printf 'WoR-Flasher cache is incomplete: %s\n' "$WOR_HOOK_INSTALL_DIR" >&2; exit 1; }
    mkdir -p "$(dirname "$WOR_HOOK_INSTALL_DIR")" || exit 1
    bootstrap_dir="${WOR_HOOK_INSTALL_DIR}.tmp.$$"
    trap 'rm -rf "$bootstrap_dir"' EXIT
    printf 'Obtaining WoR-Flasher from %s (%s)...\n' "$WOR_HOOK_REPOSITORY" "$WOR_HOOK_REF" >&2
    git clone --quiet --depth 1 --branch "$WOR_HOOK_REF" "$WOR_HOOK_REPOSITORY" "$bootstrap_dir" \
      || { printf 'Failed to obtain WoR-Flasher.\n' >&2; exit 1; }
    hook_checkout_complete "$bootstrap_dir" \
      || { printf 'The obtained WoR-Flasher checkout is incomplete.\n' >&2; exit 1; }
    mv "$bootstrap_dir" "$WOR_HOOK_INSTALL_DIR" || exit 1
    trap - EXIT
  fi
  if ! hook_checkout_complete "$WOR_HOOK_INSTALL_DIR" ;then
    printf 'The obtained WoR-Flasher checkout is incomplete: %s\n' "$WOR_HOOK_INSTALL_DIR" >&2
    exit 1
  fi
fi

while [ $# -gt 0 ]; do
  case "$1" in
    --config=*)
      WOR_CONFIG_FILE="${1#*=}"
      export WOR_CONFIG_FILE
      shift
      ;;
    --config)
      [ -n "${2:-}" ] || { printf 'Usage: %s [--config FILE] [--progress-file FILE] [--set NAME=VALUE] COMMAND\n' "$0" >&2; exit 2; }
      WOR_CONFIG_FILE="$2"
      export WOR_CONFIG_FILE
      shift 2
      ;;
    --progress-file=*)
      WOR_GUI_PROGRESS_FILE="${1#*=}"
      case "$WOR_GUI_PROGRESS_FILE" in
        ''|*$'\n'*|*$'\r'*) printf 'Invalid --progress-file value.\n' >&2; exit 2 ;;
      esac
      export WOR_GUI_PROGRESS_FILE
      shift
      ;;
    --progress-file)
      [ -n "${2:-}" ] || { printf 'Usage: %s [--config FILE] [--progress-file FILE] [--set NAME=VALUE] COMMAND\n' "$0" >&2; exit 2; }
      WOR_GUI_PROGRESS_FILE="$2"
      case "$WOR_GUI_PROGRESS_FILE" in
        ''|*$'\n'*|*$'\r'*) printf 'Invalid --progress-file value.\n' >&2; exit 2 ;;
      esac
      export WOR_GUI_PROGRESS_FILE
      shift 2
      ;;
    --set=*)
      hook_export_assignment "${1#*=}"
      shift
      ;;
    --set)
      [ -n "${2:-}" ] || { printf 'Usage: %s [--config FILE] [--progress-file FILE] [--set NAME=VALUE] COMMAND\n' "$0" >&2; exit 2; }
      hook_export_assignment "$2"
      shift 2
      ;;
    *)
      break
      ;;
  esac
done

if [ "${1:-}" == run ];then
  shift
  exec "$ENGINE" ${WOR_CONFIG_FILE:+--config="$WOR_CONFIG_FILE"} "$@"
fi

# shellcheck disable=SC1090
source "$ENGINE" source
[ -n "${WOR_CONFIG_FILE:-}" ] && load_config_json "$WOR_CONFIG_FILE" || load_config_json

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
Usage: $(basename "$0") [--config FILE] [--progress-file FILE] [--set NAME=VALUE] COMMAND

Commands:
  list-devices              Print safe whole-disk device paths, one per line.
  describe-device DEVICE    Print a human-readable device description.
  summary                   Print the current settings as label<TAB>value lines.
  run [ARGS...]             Run install-wor.sh with environment-provided settings.

Set the same variables documented in README.md before calling run, including
DEVICE, RPI_MODEL, BID, WIN_LANG and CAN_INSTALL_ON_SAME_DRIVE, or pass --config FILE.
Use --progress-file FILE with run to receive STATUS, STEP, SUBSTEP and TASK events.
Use --set NAME=VALUE to pass engine settings as options instead of environment variables.

If install-wor.sh is not next to this adapter, the complete WoR-Flasher checkout
is obtained automatically. Override WOR_HOOK_REPOSITORY, WOR_HOOK_REF or
WOR_HOOK_INSTALL_DIR to use a trusted mirror, ref or installation directory.
USAGE
    exit 2
    ;;
esac
