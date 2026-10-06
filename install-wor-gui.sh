#!/bin/bash

#WoR-Flasher graphical front-end for Linux and macOS.
#Presentation only: every decision, validation and device operation belongs to install-wor.sh,
#which this script sources so the two can never disagree about what actually gets flashed.
#Native AppKit/JXA dialogs are used on macOS and yad dialogs on Linux; the two must stay aligned.
#Version, licensing and attribution are defined in src/lib/metadata.sh and install-wor.sh.
#
#Original author: Botspot - https://github.com/Botspot/wor-flasher
#Maintained with Blackout Secure support - https://github.com/Botspot/wor-flasher

: "${WOR_ANNOUNCEMENT_TIMEOUT:=30}"

#Packaged Linux GUI clients pin both values to their matching release.
WOR_GUI_BOOTSTRAP_BASE_URL='https://github.com/blackoutsecure/wor-flasher/releases/latest/download'
WOR_GUI_BOOTSTRAP_SHA256=''

export RUN_MODE=gui #this variable is detected by install-wor.sh to display gui error messages

#Determine the directory that contains this script
[ -z "$DIRECTORY" ] && DIRECTORY="$(cd "$(dirname "$0")" && pwd -P)"

#On Windows, hand off to the packaged Windows executable if present
if [ "${OS:-}" == "Windows_NT" ] || [[ "$(uname -s 2>/dev/null)" =~ MINGW|MSYS|CYGWIN|Windows ]];then
  if [ -x "$DIRECTORY/release/windows/wor-flasher.exe" ];then
    exec "$DIRECTORY/release/windows/wor-flasher.exe" "$@"
  elif [ -x "$DIRECTORY/release/windows/WoR-Flasher.exe" ];then
    exec "$DIRECTORY/release/windows/WoR-Flasher.exe" "$@"
  fi
fi

#The native app owns macOS preflight and dependency setup. Local development runs the working tree
#directly; opt in to app handoff only when intentionally testing the packaged launcher.
if [ "$(uname -s 2>/dev/null)" == Darwin ] && [ "${WOR_NATIVE_APP:-0}" != 1 ] && [ "${WOR_USE_PACKAGED_APP:-0}" == 1 ] && [ -x "$DIRECTORY/release/macos/WoR-Flasher.app/Contents/MacOS/WoR-Flasher" ];then
  exec /usr/bin/open -W "$DIRECTORY/release/macos/WoR-Flasher.app"
fi

repair_missing_checkout_runtime() { #Restore only absent runtime files from local HEAD before shared libraries are sourced.
  local required_path
  local missing=()
  command -v git >/dev/null 2>&1 && git -C "$DIRECTORY" rev-parse --git-dir >/dev/null 2>&1 || return 0
  while IFS= read -r required_path ;do
    [ -e "$DIRECTORY/$required_path" ] || missing+=("$required_path")
  done < <(git -C "$DIRECTORY" ls-tree -r --name-only HEAD -- \
    install-wor.sh install-wor-gui.sh install-wor-hook.sh src/lib src/config src/updater.mjs \
    config-templates assets)
  [ "${#missing[@]}" -gt 0 ] || return 0
  printf 'Repairing missing WoR-Flasher files from local Git HEAD:\n' 1>&2
  printf '  %s\n' "${missing[@]}" 1>&2
  git -C "$DIRECTORY" restore --source=HEAD -- "${missing[@]}" \
    || { printf 'Failed to restore missing WoR-Flasher files from local Git HEAD.\n' 1>&2; return 1; }
}

gui_runtime_complete() { #Input: runtime directory. Do not source a partial engine or mix versions of its dependencies.
  local path
  for path in \
    install-wor.sh \
    src/lib/metadata.sh src/lib/dependencies.sh src/lib/paths.sh src/lib/cleanup.sh src/lib/gui.sh \
    src/lib/node-runtime.mjs src/lib/macos-disk-alerts.js src/updater.mjs \
    src/lib/pi3-boot-refresh/Pi3BootRefresh.exe src/lib/pi3-boot-refresh/manifest.json \
    src/lib/pi3-boot-refresh/GO-LICENSE.txt \
    src/config/metadata.json src/config/metadata.schema.json \
    config-templates/config.json config-templates/config.schema.json \
    config-templates/pi3.config.txt config-templates/pi4.config.txt config-templates/pi5.config.txt \
    config-templates/pi4-ram-unlock.ps1 config-templates/pi4-ram-unlock-specialize.xml \
    config-templates/oobe-network-bypass.xml config-templates/prefinalize.cmd \
    assets/logo-full.png assets/partnership.png assets/overview.png assets/ram.png assets/next-steps.png ;do
    [ -f "$1/$path" ] && [ -r "$1/$path" ] || return 1
  done
}

gui_fetch_runtime_file() { #Input: HTTPS URL and destination. Bootstrap must not weaken transport verification.
  case "$1" in
    https://*) ;;
    *) printf 'Refusing a non-HTTPS GUI runtime download.\n' >&2; return 1 ;;
  esac
  if command -v curl >/dev/null 2>&1;then
    curl --fail --location --silent --show-error --proto '=https' --proto-redir '=https' \
      --connect-timeout 15 --max-time 120 --output "$2" -- "$1"
  elif command -v wget >/dev/null 2>&1;then
    wget --https-only --timeout=30 --tries=2 -O "$2" -- "$1"
  else
    printf 'Missing GUI runtime: curl or wget is required to download it. Alternatively, put the standalone install-wor.sh release file beside this GUI script.\n' >&2
    return 1
  fi
}

gui_bootstrap_client() { #Output: verified standalone client path. Only a private cache is written; local files are preserved.
  (
    set -euo pipefail
    umask 077
    #Subshell-owned state remains available to EXIT cleanup even on Bash 3.2 error exits.
    cache_root='' workspace='' expected='' actual='' cached=''
    hash_command=()
    if command -v sha256sum >/dev/null 2>&1;then
      hash_command=(sha256sum)
    elif command -v shasum >/dev/null 2>&1;then
      hash_command=(shasum -a 256)
    else
      printf 'GUI runtime verification requires sha256sum or shasum.\n' >&2
      exit 1
    fi
    cache_root="${XDG_CACHE_HOME:-${HOME:?HOME is required}/.cache}/wor-flasher/gui-client"
    [[ "$cache_root" == /* ]] && [ ! -L "$cache_root" ] \
      || { printf 'Refusing an unsafe GUI runtime cache: %s\n' "$cache_root" >&2; exit 1; }
    mkdir -p "$cache_root" || { printf 'Could not create GUI runtime cache: %s\n' "$cache_root" >&2; exit 1; }
    [ -O "$cache_root" ] || { printf 'GUI runtime cache is not owned by this user: %s\n' "$cache_root" >&2; exit 1; }
    chmod 700 "$cache_root" || { printf 'Could not secure GUI runtime cache: %s\n' "$cache_root" >&2; exit 1; }
    workspace="$(mktemp -d "$cache_root/.download.XXXXXX")" \
      || { printf 'Could not create GUI download staging.\n' >&2; exit 1; }
    trap 'result=$?; trap - EXIT; rm -rf -- "$workspace" || result=1; exit "$result"' EXIT
    trap 'exit 130' INT
    trap 'exit 143' HUP TERM
    expected="$WOR_GUI_BOOTSTRAP_SHA256"
    if [ -z "$expected" ];then
      gui_fetch_runtime_file "$WOR_GUI_BOOTSTRAP_BASE_URL/SHA256SUMS" "$workspace/SHA256SUMS" \
        || { printf 'Could not obtain GUI runtime checksums from the maintained release.\n' >&2; exit 1; }
      expected="$(awk '$2 == "install-wor.sh" { count++; digest=$1 } END { if (count != 1) exit 1; print digest }' "$workspace/SHA256SUMS")" \
        || { printf 'The release does not identify exactly one standalone install-wor.sh checksum.\n' >&2; exit 1; }
    fi
    [[ "$expected" =~ ^[0-9a-f]{64}$ ]] \
      || { printf 'Invalid standalone runtime SHA-256 checksum.\n' >&2; exit 1; }
    cached="$cache_root/$expected.sh"
    if [ ! -e "$cached" ] && [ ! -L "$cached" ];then
      printf 'Obtaining the complete GUI runtime from %s/install-wor.sh...\n' "$WOR_GUI_BOOTSTRAP_BASE_URL" >&2
      gui_fetch_runtime_file "$WOR_GUI_BOOTSTRAP_BASE_URL/install-wor.sh" "$workspace/install-wor.sh" \
        || { printf 'Could not download the GUI runtime. No local files were replaced.\n' >&2; exit 1; }
      actual="$("${hash_command[@]}" "$workspace/install-wor.sh")" \
        || { printf 'Could not hash the downloaded GUI runtime.\n' >&2; exit 1; }
      [ "${actual%% *}" == "$expected" ] \
        || { printf 'Downloaded GUI runtime checksum mismatch; refusing to run it.\n' >&2; exit 1; }
      grep -qFx '#WOR_STANDALONE_CLIENT' "$workspace/install-wor.sh" \
        || { printf 'The release runtime is not a standalone client; refusing to run a partial script.\n' >&2; exit 1; }
      chmod 700 "$workspace/install-wor.sh" || { printf 'Could not secure the downloaded GUI runtime.\n' >&2; exit 1; }
      mv -n "$workspace/install-wor.sh" "$cached" || { printf 'Could not cache the verified GUI runtime.\n' >&2; exit 1; }
    fi
    [ -f "$cached" ] && [ -O "$cached" ] && [ ! -L "$cached" ] \
      || { printf 'Refusing an unsafe cached GUI runtime: %s\n' "$cached" >&2; exit 1; }
    actual="$("${hash_command[@]}" "$cached")" \
      || { printf 'Could not hash the cached GUI runtime.\n' >&2; exit 1; }
    [ "${actual%% *}" == "$expected" ] \
      || { printf 'Cached GUI runtime failed verification. Move this file aside after closing all runs: %s\n' "$cached" >&2; exit 1; }
    grep -qFx '#WOR_STANDALONE_CLIENT' "$cached" \
      || { printf 'The cached GUI runtime is not a standalone client.\n' >&2; exit 1; }
    printf '%s\n' "$cached"
  )
}

repair_missing_checkout_runtime || exit 1

cli_script="$DIRECTORY/install-wor.sh"
if [ -z "${WOR_CONFIG_FILE:-}" ] && [ -f "$DIRECTORY/config.json" ];then
  export WOR_CONFIG_FILE="$DIRECTORY/config.json"
fi
if [ -f "$cli_script" ] && grep -qFx '#WOR_STANDALONE_CLIENT' "$cli_script";then
  [ "${WOR_GUI_BOOTSTRAPPED:-0}" != 1 ] \
    || { printf 'The standalone GUI runtime is incomplete; refusing a bootstrap loop.\n' >&2; exit 1; }
  export WOR_GUI_BOOTSTRAPPED=1
  exec "$BASH" "$cli_script" --gui "$@"
fi
if ! gui_runtime_complete "$DIRECTORY";then
  if [ "$(uname -s 2>/dev/null)" != Linux ];then
    printf 'The GUI runtime is incomplete. Use the full checkout or the macOS app, or place the standalone install-wor.sh release beside this script.\n' >&2
    exit 1
  fi
  if [ -n "${WSL_DISTRO_NAME:-}" ] || [ -n "${WSLENV:-}" ] || grep -qi 'microsoft\|wsl' /proc/version 2>/dev/null;then
    printf 'WoR-Flasher does not support WSL. No runtime was downloaded.\n' >&2
    exit 1
  fi
  if [ "${WOR_GUI_BOOTSTRAPPED:-0}" == 1 ];then
    printf 'The downloaded GUI runtime is incomplete; refusing a bootstrap loop.\n' >&2
    exit 1
  fi
  printf 'Local GUI runtime files are missing. Using a verified release runtime; existing files are left unchanged.\n' >&2
  cli_script="$(gui_bootstrap_client)" || exit 1
  export WOR_GUI_BOOTSTRAPPED=1
  exec "$BASH" "$cli_script" --gui "$@"
fi

#shellcheck disable=SC1090
source "$cli_script" source #shared engine definitions and a read-only release check

#outside the packaged .app (which sets this from its own bundled .icns), fall back to the same
#logo PNG the rest of the app already uses, so the Dock and minimized-window tile are branded
#instead of showing the generic osascript icon
[ -z "$WOR_ICON_PATH" ] && WOR_ICON_PATH="$WOR_LOGO_PATH"

find_macos_gui_process() { #Input: parent pid. Output: first descendant WoR-Flasher script-host pid.
  local child found process_name
  for child in $(pgrep -P "$1" 2>/dev/null) ;do
    process_name="$(ps -p "$child" -o comm= 2>/dev/null)"
    if [ "${process_name##*/}" == "$WOR_FLASHER_NAME" ];then
      printf '%s\n' "$child"
      return 0
    fi
    found="$(find_macos_gui_process "$child")" && { printf '%s\n' "$found"; return 0; }
  done
  return 1
}

activate_macos_gui() { #Input: owning shell pid. Brings its current JXA window forward.
  local gui_pid
  gui_pid="$(find_macos_gui_process "$1")" || return 1
  wor_osascript -l JavaScript - "$gui_pid" <<'JXA' >/dev/null 2>&1
ObjC.import('AppKit')
const pid = Number(ObjC.unwrap($.NSProcessInfo.processInfo.arguments.objectAtIndex(4)))
const runningApp = $.NSRunningApplication.runningApplicationWithProcessIdentifier(pid)
if (!runningApp) $.exit(1)
runningApp.unhide
if (!runningApp.activateWithOptions($.NSApplicationActivateAllWindows | $.NSApplicationActivateIgnoringOtherApps)) $.exit(1)
JXA
}

release_gui_instance() {
  local owner_pid
  owner_pid="$(cat "$WOR_GUI_INSTANCE_DIR/pid" 2>/dev/null)"
  [ "$owner_pid" == "$$" ] || return 0
  rm -f "$WOR_GUI_INSTANCE_DIR/pid"
  rmdir "$WOR_GUI_INSTANCE_DIR" 2>/dev/null
}

acquire_gui_instance() {
  local owner_command owner_pid
  WOR_GUI_INSTANCE_DIR="${TMPDIR:-/tmp}/wor-flasher-gui-${UID}.lock"
  if ! mkdir "$WOR_GUI_INSTANCE_DIR" 2>/dev/null ;then
    owner_pid="$(cat "$WOR_GUI_INSTANCE_DIR/pid" 2>/dev/null)"
    owner_command="$(ps -p "$owner_pid" -o args= 2>/dev/null)"
    if [ -n "$owner_pid" ] && kill -0 "$owner_pid" 2>/dev/null && [[ "$owner_command" == *install-wor-gui.sh* ]];then
      [ "$(uname -s)" != Darwin ] || activate_macos_gui "$owner_pid"
      return 1
    fi
    rm -f "$WOR_GUI_INSTANCE_DIR/pid"
    rmdir "$WOR_GUI_INSTANCE_DIR" 2>/dev/null
    mkdir "$WOR_GUI_INSTANCE_DIR" 2>/dev/null || return 1
  fi
  printf '%s\n' "$$" > "$WOR_GUI_INSTANCE_DIR/pid"
  export WOR_GUI_INSTANCE_DIR
  trap release_gui_instance EXIT
  trap 'exit 130' INT
  trap 'exit 143' HUP TERM
}

acquire_gui_instance || exit 0

echo "DIRECTORY: $DIRECTORY"
echo "DL_DIR: $DL_DIR"

open_url() { #Input: url
  if command -v x-www-browser >/dev/null ;then
    x-www-browser "$1" &
  elif command -v xdg-open >/dev/null ;then
    xdg-open "$1" &
  elif command -v open >/dev/null ;then
    open "$1" &
  else
    error "Failed to locate a browser opener for $1"
  fi
}

kill_process_tree() { #Input: pid. Stops it and everything it started; most of the flash runs under sudo.
  local pid="$1" child allow_sudo="${2:-1}"
  for child in $(pgrep -P "$pid" 2>/dev/null) ;do
    kill_process_tree "$child" "$allow_sudo"
  done
  kill -TERM "$pid" 2>/dev/null || { [ "$allow_sudo" != 1 ] || command sudo -n kill -TERM "$pid" 2>/dev/null; }
}

gui_start_disk_alert_handler() { #Runs only the optional macOS alert automation, separate from the flash and progress window.
  {
    wor_osascript -l JavaScript "$DIRECTORY/src/lib/macos-disk-alerts.js" \
      "$progress_file" "$done_marker" "$abort_marker" "$installer_pid" "$disk_alert_status" \
      > "$disk_alert_log" 2>&1
    printf '%s\n' "$?" > "$disk_alert_done"
  } &
  disk_alert_pid=$!
}

gui_stop_disk_alert_handler() { #Stops only this run's user-owned helper and preserves its diagnostics after the installer exits.
  local attempt child children='' result='' wait_status=0 forced_stop=0
  [ -n "${disk_alert_pid:-}" ] || return 0
  for attempt in 1 2 3 4 5 6 7 8 9 10 ;do
    if [ -s "$disk_alert_done" ] || ! kill -0 "$disk_alert_pid" 2>/dev/null;then
      break
    fi
    sleep 0.1
  done
  if kill -0 "$disk_alert_pid" 2>/dev/null;then
    children="$(pgrep -P "$disk_alert_pid" 2>/dev/null)"
    for child in $children ;do
      kill -TERM "$child" 2>/dev/null || ! kill -0 "$child" 2>/dev/null \
        || printf 'Warning: could not stop disk-alert helper child %s.\n' "$child" >> "$output_log"
    done
    kill -TERM "$disk_alert_pid" 2>/dev/null || ! kill -0 "$disk_alert_pid" 2>/dev/null \
      || printf 'Warning: could not stop disk-alert helper %s.\n' "$disk_alert_pid" >> "$output_log"
    for attempt in 1 2 3 4 5 6 7 8 9 10 ;do
      kill -0 "$disk_alert_pid" 2>/dev/null || break
      sleep 0.1
    done
    for child in $children "$disk_alert_pid" ;do
      if kill -0 "$child" 2>/dev/null;then
        forced_stop=1
        kill -KILL "$child" 2>/dev/null || ! kill -0 "$child" 2>/dev/null \
          || printf 'Warning: could not terminate disk-alert helper process %s.\n' "$child" >> "$output_log"
      fi
    done
  fi
  wait "$disk_alert_pid" 2>/dev/null || wait_status=$?
  if [ -s "$disk_alert_status" ];then
    disk_alert_warning="$(jq -r 'if .state == "warning" then .message else "" end' "$disk_alert_status")" \
      || disk_alert_warning='Automatic Ignore status could not be read. Choose Ignore manually if the unreadable-disk alert appears.'
  fi
  [ ! -s "$disk_alert_done" ] || result="$(cat "$disk_alert_done")"
  if [ "$forced_stop" == 1 ] || { [ -n "$result" ] && [ "$result" != 0 ]; } || { [ -z "$result" ] && [ "$wait_status" != 0 ] && [ "$wait_status" != 143 ]; };then
    [ -n "$disk_alert_warning" ] || disk_alert_warning='Automatic Ignore stopped unexpectedly. Choose Ignore manually if the unreadable-disk alert appears.'
  fi
  [ ! -s "$disk_alert_log" ] || cat "$disk_alert_log" >> "$output_log"
  [ -z "$disk_alert_warning" ] || printf 'Warning: %s\n' "$disk_alert_warning" >> "$output_log"
  rm -f "$disk_alert_status" "$disk_alert_done" "$disk_alert_log"
  disk_alert_pid=''
}

gui_start_installer() { #Starts install-wor.sh in the background and waits for it to authenticate. Sets error_marker, output_log, progress_file, done_marker, auth_marker and installer_pid.
  error_marker="$(mktemp)" || error "Failed to create a GUI error marker."
  output_log="$(mktemp)" || error "Failed to create an install log."
  progress_file="$(mktemp)" || error "Failed to create a progress file."
  done_marker="$(mktemp -u)"
  auth_marker="$(mktemp -u)"
  disk_alert_pid='' disk_alert_warning=''
  if is_macos;then
    disk_alert_status="$(mktemp)" || error "Failed to create disk-alert status."
    disk_alert_log="$(mktemp)" || error "Failed to create a disk-alert log."
    disk_alert_done="$disk_alert_status.done"
  fi
  #start with a clean marker; the installer creates it only if an error occurs
  rm -f "$error_marker"

  #No separate terminal is spawned, so the installer inherits this process's exported environment directly.
  export_installer_settings
  export WOR_GUI_ERROR_MARKER="$error_marker"
  export WOR_GUI_PROGRESS_FILE="$progress_file"
  export WOR_GUI_AUTH_MARKER="$auth_marker"
  export WOR_GUI_ABORT_MARKER="${abort_marker:-}"
  #this script already ran the update check while sourcing install-wor.sh; a second one would only
  #repeat the network round-trip, and an update applied here would re-exec the installer mid-launch
  export NO_UPDATE=1
  #only a password retry sets this; the installer still verifies the prepared files before trusting it
  export WOR_RESUME_AT_FLASH="${resume_at_flash:-0}"

  #the job records its own status: a subshell cannot wait on a sibling, so waiting there returned 127 at once
  { "$cli_script" > "$output_log" 2>&1; echo $? > "$done_marker"; } &
  installer_pid=$!
  if is_macos;then
    gui_start_disk_alert_handler
  fi

  #macOS opens progress immediately; its askpass dialog activates itself when authentication is needed.
  if [ "${GUI_PROGRESS_EARLY:-0}" != 1 ];then
    while [ ! -e "$auth_marker" ] && [ ! -f "$done_marker" ] ;do
      sleep 0.3
    done
  fi
}

installer_showed_own_error() { #Exit 0 if install-wor.sh already displayed its own native error dialog.
  #gui_error_dialog creates the marker before opening its own native dialog; the log has the details
  [ -e "$error_marker" ]
}

gui_update_last_log() { #Input: saved log. Keep the support shortcut current after dialog diagnostics are appended.
  local last_log
  last_log="$(wor_last_log_file)"
  if [ "$1" != "$last_log" ];then
    mkdir -p "$(dirname "$last_log")" && cp "$1" "$last_log" \
      || warning "Could not refresh $last_log. The full log remains at $1."
  fi
}

gui_save_installer_log() { #Output: where the installer log was kept, even when a result window cannot open.
  local saved_log
  saved_log="$(wor_log_file)"
  if ! mkdir -p "$(dirname "$saved_log")" || ! mv "$output_log" "$saved_log";then
    warning "Could not save the installer log to $saved_log. Keeping $output_log instead."
    saved_log="$output_log"
  fi
  gui_update_last_log "$saved_log"
  echo "Installer log saved to $saved_log" 1>&2
  echo "$saved_log"
}

macos_show_result_dialog() { #Input: message, image, settings URL, log, optional sound/connect action. Sets GUI_RESULT_ACTION.
  local message="$1" image="$2" settings_url="$3" saved_log="$4" sound="${5:-}" allow_connect="${6:-0}"
  local dialog_status dialog_result button=OK extra=()
  GUI_RESULT_ACTION=close
  [ -z "$image" ] || button=Complete
  if [ "$allow_connect" == 1 ] && [ -n "$image" ];then extra=(connect);else allow_connect=0;fi
  if dialog_result="$(wor_osascript -l JavaScript - "$message" "$WOR_ICON_PATH" "$WOR_APP_TITLE" "$image" "$WOR_WINDOW_TITLE" "$sound" "$settings_url" "${extra[@]}" \
    <<<"$completion_jxa" 2>>"$saved_log")";then
    if [ "$allow_connect" == 1 ];then
      case "$dialog_result" in
        __WOR_CONNECT__) GUI_RESULT_ACTION=connect ;;
        '' | __WOR_CLOSE__) ;;
        *) warning "The result dialog returned an invalid action; no Pi connection was started."; return 1 ;;
      esac
    fi
    printf '\nResult dialog closed normally.\n' >> "$saved_log"
    gui_update_last_log "$saved_log"
    return 0
  else
    dialog_status=$?
  fi
  printf '\nCustom result dialog failed (status %s); opening the fallback dialog.\n' "$dialog_status" >> "$saved_log"
  warning "The custom result window failed to open. Using the fallback dialog. Log: $saved_log"
  if dialog_result="$(osascript - "$message" "$WOR_WINDOW_TITLE" "$button" "${extra[@]}" 2>>"$saved_log" <<'APPLESCRIPT'
on run argv
  activate
  if (count argv) > 3 then
    if item 4 of argv is "connect" then
      set resultDialog to display dialog (item 1 of argv) with title (item 2 of argv) buttons {"Connect to Pi", "Complete"} default button "Complete" with icon note
      return button returned of resultDialog
    end if
  end if
  if item 3 of argv is "Complete" then
    display dialog (item 1 of argv) with title (item 2 of argv) buttons {"Complete"} default button "Complete" with icon note
  else
    display dialog (item 1 of argv) with title (item 2 of argv) buttons {"OK"} default button "OK" with icon caution
  end if
end run
APPLESCRIPT
)";then
    if [ "$allow_connect" == 1 ] && [ "$dialog_result" == 'Connect to Pi' ];then GUI_RESULT_ACTION=connect;fi
    printf 'Fallback result dialog closed normally.\n' >> "$saved_log"
    gui_update_last_log "$saved_log"
    return 0
  else
    dialog_status=$?
  fi
  printf 'Fallback result dialog failed (status %s); opening the saved log.\n' "$dialog_status" >> "$saved_log"
  warning "Neither result dialog could be displayed. Opening the full log: $saved_log"
  open -t "$saved_log" 2>>"$saved_log" \
    || { printf 'Could not open the saved log automatically.\n' >> "$saved_log"; warning "Open $saved_log to see the installer result."; }
  gui_update_last_log "$saved_log"
  return 1
}

linux_show_completion_dialog() { #Input: message, image, optional connect action. Sets GUI_RESULT_ACTION.
  local message="$1" image="$2" allow_connect="${3:-0}" response=0 buttons=(--button=Close:0)
  GUI_RESULT_ACTION=close
  [ "$allow_connect" != 1 ] || buttons+=(--button='Connect to Pi':2)
  yad --center --width="$(wor_yad_width 690)" --height="$(wor_yad_height 380)" --window-icon="$WOR_LOGO_PATH" --class="$WOR_ICON_NAME" --title="$WOR_WINDOW_TITLE" \
    --text="$message" \
    --form --align=center --image-on-top --buttons-layout=center --image="$image" \
    --field="It is now safe to remove your USB drive.":LBL '' "${buttons[@]}" >/dev/null || response=$?
  case "$response" in
    0 | 1 | 252) return 0 ;;
    2)
      if [ "$allow_connect" == 1 ];then GUI_RESULT_ACTION=connect;return 0;fi
      ;;
  esac
  warning "The completion dialog failed or returned an invalid action; no Pi connection was started."
  return 1
}

gui_log_tail() { #Input: log path. Output: the last lines, with terminal escapes and carriage returns removed.
  #Installer logs can contain raw tool output; macOS sed rejects an invalid UTF-8 byte under the user locale.
  LC_ALL=C sed 's/\x1b\[[0-9;]*[A-Za-z]//g; s/\r//g' "$1" | tail -n 18
}

macos_password_retry_dialog() { #Input: saved log and progress file. Output: retry/close only for authentication that failed before writing.
  local saved_log="$1" progress_path="$2" password_retry_reason choice
  if [ ! -r "$saved_log" ] || [ ! -r "$progress_path" ];then
    warning "Administrator authentication could not be classified because its diagnostics are unavailable."
    return 1
  fi
  #Any write marker rules out a no-changes claim, even if a later marker says writing stopped.
  awk -F '\t' '
    $1 == "DISK_WRITE" && $2 == "1" { written = 1 }
    $1 == "STEP" { step = $2; total = $3; label = $4; awaiting_password = 0 }
    $1 == "TASK" { awaiting_password = ($2 == "0" && $3 == "Waiting for administrator access...") }
    END {
      valid_step = step ~ /^[1-9][0-9]*$/ && total ~ /^[1-9][0-9]*$/ && step <= total && label != ""
      partition_auth = label ~ /^Partitioning and formatting / &&
        ((total == 8 && step == 5) || (total == 7 && step == 4))
      exit written || !valid_step || !(awaiting_password || partition_auth)
    }
  ' "$progress_path" || return 1
  if ! grep -qF 'Administrator authentication failed or was canceled.' "$saved_log" \
    && ! grep -qF 'Administrator authentication was canceled or unavailable' "$saved_log";then
    return 1
  fi
  if grep -qF '(-128)' "$saved_log";then
    password_retry_reason='Administrator password entry was canceled.'
  elif grep -qF 'incorrect password attempt' "$saved_log" \
    || grep -qF 'Administrator password was not accepted.' "$saved_log";then
    password_retry_reason='The administrator password was not accepted.'
  elif grep -qF 'no password was provided' "$saved_log";then
    password_retry_reason='No administrator password was entered.'
  else
    password_retry_reason='Administrator access was not obtained.'
  fi
  choice="$(macos_choose '' "$password_retry_reason

Flashing has not started. No changes have been made to $DEVICE.

Your prepared downloads have been kept. Try Again rechecks the prepared image and returns to the administrator password step.

Log: $saved_log" retry Close '' '' '' 'Try Again' "$WOR_ICON_PATH" "$WOR_WINDOW_TITLE | Administrator access")" || choice=close
  case "$choice" in
    retry|close) printf '%s\n' "$choice" ;;
    *)
      warning "The administrator retry dialog returned an unexpected response; showing the full error instead."
      return 1
      ;;
  esac
}

loading_dialog() { #display a dialog to say something is loading
  local dialog_pid
  #1, not 0: gtk_window_resize asserts height > 0, and GTK still grows the window to its natural size
  (echo '# ' ; sleep infinity) | yad "${yadflags[@]}" --height=1 \
    --progress --pulsate --title="$1" --text="$1" --no-buttons &
  dialog_pid=$!
  trap 'kill "$dialog_pid" 2>/dev/null' EXIT

  sleep infinity
}

stop_loader() {
  [ -z "${loader_pid:-}" ] || kill "$loader_pid" 2>/dev/null
  loader_pid=''
}

macos_choose() { #Input: choices, prompt, default, cancel label, action label/value, image, next label, icon, title, cancel value, timeout, process title, optional action URL. Output: choice or action value.
  local result choose_jxa
  choose_jxa="$(wor_jxa_window_lib; cat <<'JXA'
ObjC.import('AppKit')
ObjC.import('stdlib')
ObjC.import('Foundation')

const args = $.NSProcessInfo.processInfo.arguments
const rawChoices = ObjC.unwrap(args.objectAtIndex(4))
const choices = rawChoices.length > 0 ? rawChoices.split('\n') : []
const promptText = ObjC.unwrap(args.objectAtIndex(5))
const defaultChoice = ObjC.unwrap(args.objectAtIndex(6))
const cancelLabel = ObjC.unwrap(args.objectAtIndex(7))
const actionLabel = ObjC.unwrap(args.objectAtIndex(8))
const actionValue = ObjC.unwrap(args.objectAtIndex(9))
const imagePath = ObjC.unwrap(args.objectAtIndex(10))
const nextLabel = ObjC.unwrap(args.objectAtIndex(11))
const iconPath = ObjC.unwrap(args.objectAtIndex(12))
const windowTitle = ObjC.unwrap(args.objectAtIndex(13))
const cancelValue = ObjC.unwrap(args.objectAtIndex(14))
const announcementTimeout = Number(ObjC.unwrap(args.objectAtIndex(15) || '0'))
const appTitle = ObjC.unwrap(args.objectAtIndex(16) || windowTitle)
const actionURL = ObjC.unwrap(args.objectAtIndex(17))
//a message screen has no selectable rows; it may still show an image (e.g. the welcome screen)
const isMessageMode = choices.length === 0
const isPartnershipAnnouncement = imagePath.endsWith('/partnership.png')

function writeResult(value) {
  const data = $(value + '\n').dataUsingEncoding($.NSUTF8StringEncoding)
  $.NSFileHandle.fileHandleWithStandardOutput.writeData(data)
}

function cancelAndExit() {
  writeResult('__WOR_CANCEL__')
  $.exit(0)
}

$.NSProcessInfo.processInfo.processName = appTitle
const app = $.NSApplication.sharedApplication
app.setActivationPolicy($.NSApplicationActivationPolicyRegular)
worInstallAppMenu(app, appTitle, windowTitle, iconPath)
worSetAppIcon(app, iconPath)
let tableView
let window
let selectedValue = null
let allowTermination = false
let countdownSeconds = announcementTimeout
let countdownTimer = null
let nextButton
const Controller = ObjC.registerSubclass({
  name: 'WorChooserController',
  superclass: 'NSObject',
  methods: {
    'numberOfRowsInTableView:': {
      types: ['NSInteger', ['id']],
      implementation: function() {
        return choices.length
      }
    },
    'tableView:objectValueForTableColumn:row:': {
      types: ['id', ['id', 'id', 'NSInteger']],
      implementation: function(_tableView, _tableColumn, row) {
        return $(choices[row])
      }
    },
    'nextClicked:': {
      types: ['void', ['id']],
      implementation: function() {
        if (isMessageMode) {
          selectedValue = defaultChoice
          app.stopModalWithCode($.NSOKButton)
          window.orderOut(null)
          return
        }
        const row = tableView.selectedRow
        if (row >= 0 && row < choices.length) {
          selectedValue = choices[row]
          app.stopModalWithCode($.NSOKButton)
          window.orderOut(null)
        }
      }
    },
    'cancelClicked:': {
      types: ['void', ['id']],
      implementation: function() {
        selectedValue = cancelValue.length > 0 ? cancelValue : null
        app.stopModalWithCode($.NSCancelButton)
        window.orderOut(null)
      }
    },
    'countdownTick:': {
      types: ['void', ['id']],
      implementation: function() {
        if (countdownSeconds <= 0) return
        countdownSeconds -= 1
        nextButton.title = nextLabel + ' (' + countdownSeconds + ')'
        if (countdownSeconds === 0) {
          selectedValue = defaultChoice
          app.stopModalWithCode($.NSOKButton)
          window.orderOut(null)
        }
      }
    },
    'textView:clickedOnLink:atIndex:': {
      types: ['BOOL', ['id', 'id', 'NSUInteger']],
      implementation: function(_textView, link, _index) {
        const url = ObjC.unwrap(link)
        $.NSWorkspace.sharedWorkspace.openURL($.NSURL.URLWithString($(url)))
        return true
      }
    },
    'actionClicked:': {
      types: ['void', ['id']],
      implementation: function() {
        if (actionURL.length > 0) {
          //Keep Recheck available while System Settings is open.
          if (!$.NSWorkspace.sharedWorkspace.openURL($.NSURL.URLWithString($(actionURL)))) {
            const alert = $.NSAlert.alloc.init
            alert.messageText = 'Could not open System Settings'
            alert.informativeText = 'Open System Settings > Privacy & Security > Accessibility manually, then return here and choose Recheck.'
            alert.addButtonWithTitle('OK')
            alert.runModal
          }
          return
        }
        selectedValue = actionValue
        app.stopModalWithCode($.NSOKButton)
        window.orderOut(null)
      }
    },
    'windowWillClose:': {
      types: ['void', ['id']],
      implementation: function() {
        selectedValue = null
        app.stopModalWithCode($.NSCancelButton)
      }
    },
    //right-click Quit from the Dock/app-switcher sends terminate: to NSApp; since this window is
    //hosted by osascript rather than a full NSApplicationMain run loop, that does not otherwise exit the process
    'handleQuitEvent:withReplyEvent:': {
      types: ['void', ['id', 'id']],
      implementation: function() {
        cancelAndExit()
      }
    },
    'pumpEvents:': {
      types: ['void', ['id']],
      implementation: function() {
        $.NSRunLoop.currentRunLoop.runModeBeforeDate($.NSDefaultRunLoopMode, $.NSDate.dateWithTimeIntervalSinceNow(0.01))
      }
    },
    //clicking the Dock icon sends aevt/rapp; without a handler a minimised window can never come back
    'handleReopenEvent:withReplyEvent:': {
      types: ['void', ['id', 'id']],
      implementation: function() {
        if (window.isMiniaturized) window.deminiaturize(null)
        window.makeKeyAndOrderFront(null)
        app.activateIgnoringOtherApps(true)
      }
    },
    'applicationShouldTerminate:': {
      types: ['NSUInteger', ['id']],
      implementation: function() {
        if (allowTermination) return $.NSTerminateNow
        cancelAndExit()
      }
    }
  }
})

const controller = $.WorChooserController.alloc.init
app.setDelegate(controller)
const screenFrame = $.NSScreen.mainScreen.visibleFrame
//clamp the desired size to whatever screen real estate is actually available, rather than assuming a full-size display
const maxWidth = Math.min(760, screenFrame.size.width - 40)
//a message screen only hides Cancel for the image-based welcome screen; an explicitly empty cancelLabel hides it everywhere else
const showCancelButton = cancelLabel.length > 0 && !(isMessageMode && imagePath.length > 0)
//the bottom button row is the hard floor on width; fitting the window to its content must never clip it
const minButtonRowWidth = 40 + 180 + (showCancelButton ? 188 : 0) + (actionLabel.length > 0 ? 120 : 0)

function measuredTextWidth(text, font) {
  const measured = $.NSMutableAttributedString.alloc.init
  measured.mutableString.appendString($(text))
  measured.addAttributeValueRange($.NSFontAttributeName, font, $.NSMakeRange(0, text.length))
  const bounds = measured.boundingRectWithSizeOptions($.NSMakeSize(2000, 200), $.NSStringDrawingUsesLineFragmentOrigin | $.NSStringDrawingUsesFontLeading)
  return Math.ceil(bounds.size.width)
}

const measureText = $.NSMutableAttributedString.alloc.init
measureText.mutableString.appendString($(promptText))
const measureRange = $.NSMakeRange(0, promptText.length)
const promptFont = $.NSFont.systemFontOfSizeWeight(14, $.NSFontWeightMedium)
measureText.addAttributeValueRange($.NSFontAttributeName, promptFont, measureRange)
const naturalBounds = measureText.boundingRectWithSizeOptions($.NSMakeSize(1000, 1000), $.NSStringDrawingUsesLineFragmentOrigin | $.NSStringDrawingUsesFontLeading)

const listRowHeight = 24
//cap the visible list so a two-item chooser stays compact and a 190-entry language list still scrolls instead of filling the screen
const visibleListHeight = isMessageMode ? 0 : Math.min(384, Math.max(96, choices.length * listRowHeight + 8))
let requestedWidth
if (isPartnershipAnnouncement || (isMessageMode && imagePath.length > 0)) {
  requestedWidth = maxWidth
} else if (isMessageMode) {
  requestedWidth = Math.max(360, Math.ceil(naturalBounds.size.width) + 40)
} else {
  //fit the widest row plus its inset and the vertical scroller, so rows read fully without a needlessly wide window
  const rowFont = $.NSFont.systemFontOfSize(13)
  let widestChoice = 0
  for (let i = 0; i < choices.length; i++) widestChoice = Math.max(widestChoice, measuredTextWidth(choices[i], rowFont))
  requestedWidth = Math.max(Math.ceil(naturalBounds.size.width), widestChoice + 60) + 40
}
const width = Math.min(maxWidth, Math.max(minButtonRowWidth, requestedWidth))
const measuredBounds = measureText.boundingRectWithSizeOptions($.NSMakeSize(width - 40, 1000), $.NSStringDrawingUsesLineFragmentOrigin | $.NSStringDrawingUsesFontLeading)
const contentHeight = Math.ceil(measuredBounds.size.height) + 150
const requestedHeight = isPartnershipAnnouncement
  ? 600
  : isMessageMode
    ? (imagePath.length > 0 ? 500 : Math.max(220, contentHeight))
    : visibleListHeight + 132
const height = Math.min(requestedHeight, screenFrame.size.height - 60)
//fixed layout: no drag-resize and no zoom/maximize button, only minimize (and restore) via the titlebar
window = worMakeWindow({ width: width, height: height, title: windowTitle, delegate: controller })

const content = window.contentView
content.autoresizingMask = $.NSViewWidthSizable | $.NSViewHeightSizable

const prompt = $.NSTextField.labelWithString(promptText)
prompt.font = $.NSFont.systemFontOfSizeWeight(14, $.NSFontWeightMedium)
prompt.autoresizingMask = $.NSViewWidthSizable | $.NSViewMinYMargin
if (isMessageMode) {
  prompt.setUsesSingleLineMode(false)
  prompt.cell.setWraps(true)
  prompt.cell.setScrollable(false)
  if (imagePath.length > 0) {
    prompt.frame = $.NSMakeRect(20, 76, width - 40, 122)
    const imageHeight = isPartnershipAnnouncement ? Math.min(315, height - 185) : 320
    const imageFrame = isPartnershipAnnouncement
      ? $.NSMakeRect(20, height - imageHeight - 12, width - 40, imageHeight)
      : $.NSMakeRect(20, 140, width - 40, imageHeight)
    const imageView = $.NSImageView.alloc.initWithFrame(imageFrame)
    imageView.image = $.NSImage.alloc.initWithContentsOfFile($(imagePath))
    imageView.imageScaling = $.NSImageScaleProportionallyUpOrDown
    imageView.imageAlignment = $.NSImageAlignCenter
    imageView.autoresizingMask = $.NSViewWidthSizable | $.NSViewMinYMargin
    content.addSubview(imageView)
  } else if (isPartnershipAnnouncement) {
    prompt.frame = $.NSMakeRect(20, 96, width - 40, height - 132)
  } else {
    prompt.frame = $.NSMakeRect(20, 72, width - 40, height - 112)
  }
  if (isPartnershipAnnouncement) {
    const attributedPrompt = $.NSMutableAttributedString.alloc.init
    attributedPrompt.mutableString.appendString($(promptText))
    const fullPromptRange = $.NSMakeRange(0, promptText.length)
    const paragraphStyle = $.NSMutableParagraphStyle.alloc.init
    paragraphStyle.alignment = $.NSTextAlignmentRight
    paragraphStyle.lineBreakMode = $.NSLineBreakByWordWrapping
    paragraphStyle.lineSpacing = 1
    paragraphStyle.paragraphSpacing = 4
    attributedPrompt.addAttributeValueRange($.NSForegroundColorAttributeName, $.NSColor.labelColor, fullPromptRange)
    attributedPrompt.addAttributeValueRange($.NSFontAttributeName, $.NSFont.systemFontOfSizeWeight(14, $.NSFontWeightMedium), fullPromptRange)
    attributedPrompt.addAttributeValueRange($.NSParagraphStyleAttributeName, paragraphStyle, fullPromptRange)
    function addPromptLink(label, url) {
      const index = promptText.indexOf(label)
      if (index < 0) return
      const range = $.NSMakeRange(index, label.length)
      attributedPrompt.addAttributeValueRange($.NSLinkAttributeName, $(url), range)
      attributedPrompt.addAttributeValueRange($.NSForegroundColorAttributeName, $.NSColor.linkColor, range)
      attributedPrompt.addAttributeValueRange($.NSUnderlineStyleAttributeName, $(1), range)
    }
    addPromptLink('Blackout Secure', 'https://blackoutsecure.app/')
    addPromptLink('Botspot', 'https://github.com/Botspot')
    addPromptLink('Windows on R', 'https://worproject.com/')
    addPromptLink('Botspot/wor-flasher', 'https://github.com/Botspot/wor-flasher')
    addPromptLink('sponsoring Botspot', 'https://github.com/sponsors/Botspot')
    addPromptLink('buying Blackout Secure a coffee', 'https://github.com/sponsors/blackoutsecure?frequency=one-time&amount=8')
    const textFrame = isPartnershipAnnouncement
      ? (() => {
          const textWidth = 680
          const textBounds = attributedPrompt.boundingRectWithSizeOptions($.NSMakeSize(textWidth, 1000), $.NSStringDrawingUsesLineFragmentOrigin | $.NSStringDrawingUsesFontLeading)
          const textHeight = Math.ceil(textBounds.size.height) + 12
          return $.NSMakeRect((width - textWidth) / 2, 58, textWidth, textHeight)
        })()
      : imagePath.length > 0
        ? $.NSMakeRect(32, 76, width - 64, 122)
        : $.NSMakeRect(20, 96, width - 40, height - 132)
    const textView = $.NSTextView.alloc.initWithFrame(textFrame)
    textView.editable = false
    textView.selectable = false
    textView.drawsBackground = false
    textView.alignment = $.NSTextAlignmentCenter
    textView.textContainerInset = $.NSMakeSize(0, 0)
    textView.textContainer.lineFragmentPadding = 0
    textView.textContainer.widthTracksTextView = true
    textView.horizontallyResizable = false
    textView.verticallyResizable = false
    textView.autoresizingMask = isPartnershipAnnouncement
      ? $.NSViewMinXMargin | $.NSViewMaxXMargin | $.NSViewMinYMargin
      : $.NSViewWidthSizable | $.NSViewMinYMargin
    textView.textStorage.setAttributedString(attributedPrompt)
    textView.delegate = controller
    content.addSubview(textView)
  } else {
    content.addSubview(prompt)
  }
} else {
  prompt.frame = $.NSMakeRect(20, height - 52, width - 40, 24)
  content.addSubview(prompt)
}

if (!isMessageMode) {
  const scrollView = $.NSScrollView.alloc.initWithFrame($.NSMakeRect(20, 72, width - 40, height - 132))
  scrollView.autoresizingMask = $.NSViewWidthSizable | $.NSViewHeightSizable
  scrollView.borderType = $.NSBezelBorder
  scrollView.hasVerticalScroller = true

  tableView = $.NSTableView.alloc.initWithFrame(scrollView.bounds)
  tableView.setHeaderView(undefined)
  tableView.rowHeight = 24
  tableView.setDelegate(controller)
  tableView.setDataSource(controller)
  tableView.setTarget(controller)
  tableView.setDoubleAction('nextClicked:')
  tableView.setAllowsEmptySelection(false)
  tableView.setUsesAlternatingRowBackgroundColors(true)

  const column = $.NSTableColumn.alloc.initWithIdentifier('choice')
  column.width = scrollView.contentSize.width
  column.resizingMask = $.NSTableColumnAutoresizingMask
  column.editable = false
  tableView.addTableColumn(column)
  scrollView.documentView = tableView
  content.addSubview(scrollView)
}

//a lone button in a message dialog reads better centered; once a Back/Advanced companion button is
//present, centering the primary button made the two overlap, so both use the standard bottom-right row instead
const centerNextButton = isMessageMode && !showCancelButton && actionLabel.length === 0

//buttons are sized to their own text so any label (e.g. "Proceed with WoR-Flasher") fits without truncation
nextButton = $.NSButton.buttonWithTitleTargetAction(nextLabel, controller, 'nextClicked:')
nextButton.setTarget(controller)
nextButton.setAction('nextClicked:')
nextButton.bezelStyle = $.NSBezelStyleRounded
nextButton.keyEquivalent = '\r'
if (announcementTimeout > 0 && isMessageMode) nextButton.title = nextLabel + ' (' + announcementTimeout + ')'
nextButton.sizeToFit
const nextWidth = Math.max(180, nextButton.frame.size.width)
const nextX = centerNextButton ? (width - nextWidth) / 2 : width - 20 - nextWidth
nextButton.frame = $.NSMakeRect(nextX, 22, nextWidth, 32)
nextButton.autoresizingMask = $.NSViewMinXMargin | $.NSViewMaxXMargin | $.NSViewMaxYMargin
content.addSubview(nextButton)

if (announcementTimeout > 0 && isMessageMode) {
  countdownTimer = $.NSTimer.timerWithTimeIntervalTargetSelectorUserInfoRepeats(1, controller, 'countdownTick:', $(), true)
  $.NSRunLoop.currentRunLoop.addTimerForMode(countdownTimer, $.NSModalPanelRunLoopMode)
}

if (showCancelButton) {
  const cancelButton = $.NSButton.buttonWithTitleTargetAction(cancelLabel, controller, 'cancelClicked:')
  cancelButton.bezelStyle = $.NSBezelStyleRounded
  cancelButton.keyEquivalent = '\u001b'
  cancelButton.sizeToFit
  //match the next button width so the pair reads as one row instead of two different sizes
  const cancelWidth = Math.max(nextWidth, cancelButton.frame.size.width)
  //anchor to the next button real x position instead of re-deriving one, so the two never overlap
  cancelButton.frame = $.NSMakeRect(nextButton.frame.origin.x - 8 - cancelWidth, 22, cancelWidth, 32)
  cancelButton.autoresizingMask = $.NSViewMinXMargin | $.NSViewMaxYMargin
  content.addSubview(cancelButton)
}

if (actionLabel.length > 0) {
  const actionButton = $.NSButton.buttonWithTitleTargetAction(actionLabel, controller, 'actionClicked:')
  actionButton.bezelStyle = $.NSBezelStyleRounded
  actionButton.sizeToFit
  const actionWidth = Math.max(112, actionButton.frame.size.width)
  actionButton.frame = $.NSMakeRect(20, 22, actionWidth, 32)
  actionButton.autoresizingMask = $.NSViewMaxXMargin | $.NSViewMaxYMargin
  content.addSubview(actionButton)
}

if (!isMessageMode) {
  const defaultIndex = Math.max(0, choices.indexOf(defaultChoice))
  tableView.selectRowIndexesByExtendingSelection($.NSIndexSet.indexSetWithIndex(defaultIndex), false)
  tableView.scrollRowToVisible(defaultIndex)
}

window.makeKeyAndOrderFront(null)
if (!app.isActive) app.requestUserAttention($.NSInformationalRequest)
app.activateIgnoringOtherApps(true)
worInstallWindowHandlers(controller)
app.runModalForWindow(window)

if (selectedValue === null) {
  writeResult('__WOR_CANCEL__')
} else {
  writeResult(selectedValue)
}
allowTermination = true
app.terminate(null)
JXA
)"
  result="$(wor_osascript -l JavaScript - "$1" "$2" "$3" "${4:-Cancel}" "${5:-}" "${6:-}" "${7:-}" "${8:-Next}" "${9:-$WOR_ICON_PATH}" "${10:-$WOR_WINDOW_TITLE}" "${11:-}" "${12:-0}" "${13:-$WOR_APP_TITLE}" "${14:-}" <<<"$choose_jxa")"
  if [ "$result" == __WOR_CANCEL__ ];then
    return 1
  fi
  printf '%s\n' "$result"
}

macos_check_accessibility() { #Checks the same script host as Automatic Ignore before opening the setup wizard.
  is_macos || return 0
  local permission check_status choice message
  status "Checking Accessibility permission for Automatic Ignore"
  while true ;do
    check_status=0
    permission="$(wor_osascript -l JavaScript "$DIRECTORY/src/lib/macos-disk-alerts.js" --check-accessibility 2>&1)" || check_status=$?
    if [ "$check_status" == 0 ] && [ "$permission" == granted ];then
      status "Automatic Ignore: Accessibility permission is allowed."
      return 0
    fi
    if [ "$check_status" == 0 ] && [ "$permission" == missing ];then
      message="Automatic Ignore needs Accessibility permission."
    else
      message="Automatic Ignore could not check Accessibility permission."
      echo_red "Warning: $message (helper exit $check_status).${permission:+ ($permission)}"
    fi
    message="$message

Open Settings and enable WoR-Flasher in Privacy & Security > Accessibility. If it is not listed, use + to add the app you launched. If macOS lists the script host or launcher instead, enable that entry. Return here and choose Recheck. If the change is not detected, quit and reopen WoR-Flasher.

This permission is only for automatically choosing Ignore on the unreadable-disk alert. You can Continue Manually and choose Ignore yourself during flashing. Never choose Initialize or Eject while a flash is running."
    choice="$(macos_choose '' "$message" recheck 'Continue Manually' 'Open Settings' '' '' Recheck "$WOR_ICON_PATH" "$WOR_WINDOW_TITLE | Accessibility" manual 0 "$WOR_APP_TITLE" 'x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility')" || return 1
    case "$choice" in
      recheck) ;;
      manual)
        echo_red "Warning: continuing without confirmed Accessibility permission. Choose Ignore manually if the unreadable-disk alert appears."
        return 0
        ;;
      *) error "Unexpected Accessibility startup response: $choice" ;;
    esac
  done
}

macos_show_announcement() { #Output: proceed or project partner.
  local announcement_choices announcement_choice announcement_text
  announcement_choices=''
  printf -v announcement_text '%s\n\n%s\n\n%s\n' \
    "Blackout Secure is proud to partner with Botspot and the Windows on R community, carrying WoR-Flasher forward while preserving Botspot's original authorship and project direction." \
    "Report issues, share feedback, or contribute at Botspot/wor-flasher." \
    "Support continued development by sponsoring Botspot or buying Blackout Secure a coffee on GitHub."
  announcement_choice="$(macos_choose "$announcement_choices" "$announcement_text" 'Proceed with WoR-Flasher' Cancel '' '' "$WOR_ASSETS_DIR/partnership.png" 'Proceed with WoR-Flasher' "$WOR_ICON_PATH" "$WOR_WINDOW_TITLE" '' "$WOR_ANNOUNCEMENT_TIMEOUT" "$WOR_APP_TITLE")" || return 1

  case "$announcement_choice" in
    'Proceed with WoR-Flasher') echo "$announcement_choice" ;;
    *) return 1 ;;
  esac
}

macos_choose_device() { #Input: newline-separated detected volume rows. Output: selected row, __REFRESH__, or Back. Fails only on a genuine Quit/close, never on Back.
  if [ -z "$1" ];then
    #cancelValue Back makes clicking Back succeed with a literal value instead of failing like Quit does,
    #so the wizard can tell "go back a step" apart from "the user is quitting the whole thing"
    result="$(macos_choose '' 'No external, physical, writable drive was found. Connect a removable drive, then click Refresh.' __REFRESH__ Back '' '' '' Refresh "$WOR_ICON_PATH" "$WOR_WINDOW_TITLE" Back 0 "$WOR_APP_TITLE")" || return 1
    echo "$result"
  else
    device_choice="$(macos_choose "$1" 'Choose the external drive and volumes to erase' "$(printf '%s\n' "$1" | head -n1)" Back Refresh __REFRESH__ '' '' '' '' Back)" || return 1
    echo "$device_choice"
  fi
}

macos_confirm_flash() { #Output: Flash, Advanced, Back, or failure for Quit/close.
  local result rows
  rows="$(settings_summary)"
  local confirm_jxa
  confirm_jxa="$(wor_jxa_window_lib; cat <<'JXA'
ObjC.import('AppKit')
ObjC.import('Foundation')
ObjC.import('stdlib')

const args = $.NSProcessInfo.processInfo.arguments
const rawRows = ObjC.unwrap(args.objectAtIndex(4))
const targetDevice = ObjC.unwrap(args.objectAtIndex(5))
const iconPath = ObjC.unwrap(args.objectAtIndex(6))
const windowTitle = ObjC.unwrap(args.objectAtIndex(7))
const appTitle = ObjC.unwrap(args.objectAtIndex(8))
const rows = rawRows.split('\n')
  .filter((line) => line.length > 0)
  .map((line) => {
    const parts = line.split('\t')
    return { label: parts[0] || '', value: parts.slice(1).join('\t') || '' }
  })

function writeResult(value) {
  const data = $(value + '\n').dataUsingEncoding($.NSUTF8StringEncoding)
  $.NSFileHandle.fileHandleWithStandardOutput.writeData(data)
}

$.NSProcessInfo.processInfo.processName = appTitle
const app = $.NSApplication.sharedApplication
app.setActivationPolicy($.NSApplicationActivationPolicyRegular)
worInstallAppMenu(app, appTitle, windowTitle, iconPath)
worSetAppIcon(app, iconPath)

let window
let selectedValue = 'Cancel'
let allowTermination = false
const Controller = ObjC.registerSubclass({
  name: 'WorConfirmFlashController',
  superclass: 'NSObject',
  methods: {
    'flashClicked:': {
      types: ['void', ['id']],
      implementation: function() {
        selectedValue = 'Flash'
        app.stopModalWithCode($.NSOKButton)
        window.orderOut(null)
      }
    },
    'advancedClicked:': {
      types: ['void', ['id']],
      implementation: function() {
        selectedValue = 'Advanced'
        app.stopModalWithCode($.NSOKButton)
        window.orderOut(null)
      }
    },
    'backClicked:': {
      types: ['void', ['id']],
      implementation: function() {
        selectedValue = 'Back'
        app.stopModalWithCode($.NSCancelButton)
        window.orderOut(null)
      }
    },
    'windowWillClose:': {
      types: ['void', ['id']],
      implementation: function() {
        selectedValue = 'Cancel'
        app.stopModalWithCode($.NSCancelButton)
      }
    },
    'handleQuitEvent:withReplyEvent:': {
      types: ['void', ['id', 'id']],
      implementation: function() {
        selectedValue = 'Cancel'
        app.stopModalWithCode($.NSCancelButton)
        window.orderOut(null)
      }
    },
    'pumpEvents:': {
      types: ['void', ['id']],
      implementation: function() {
        $.NSRunLoop.currentRunLoop.runModeBeforeDate($.NSDefaultRunLoopMode, $.NSDate.dateWithTimeIntervalSinceNow(0.01))
      }
    },
    //clicking the Dock icon sends aevt/rapp; without a handler a minimised window can never come back
    'handleReopenEvent:withReplyEvent:': {
      types: ['void', ['id', 'id']],
      implementation: function() {
        if (window.isMiniaturized) window.deminiaturize(null)
        window.makeKeyAndOrderFront(null)
        app.activateIgnoringOtherApps(true)
      }
    },
    'applicationShouldTerminate:': {
      types: ['NSUInteger', ['id']],
      implementation: function() {
        if (allowTermination) return $.NSTerminateNow
        selectedValue = 'Cancel'
        app.stopModalWithCode($.NSCancelButton)
        return 0
      }
    }
  }
})
const controller = $.WorConfirmFlashController.alloc.init
app.setDelegate(controller)

const screenFrame = $.NSScreen.mainScreen.visibleFrame
const rowHeight = 24
//the settings list is the only variable-height part, so fit the window to it instead of always opening at full size
const valueFont = $.NSFont.systemFontOfSizeWeight(12, $.NSFontWeightRegular)
let widestValue = 0
for (let i = 0; i < rows.length; i++) {
  const measured = $.NSMutableAttributedString.alloc.init
  measured.mutableString.appendString($(rows[i].value))
  measured.addAttributeValueRange($.NSFontAttributeName, valueFont, $.NSMakeRange(0, rows[i].value.length))
  const bounds = measured.boundingRectWithSizeOptions($.NSMakeSize(2000, 100), $.NSStringDrawingUsesLineFragmentOrigin | $.NSStringDrawingUsesFontLeading)
  widestValue = Math.max(widestValue, Math.ceil(bounds.size.width))
}
const width = Math.min(760, screenFrame.size.width - 40, Math.max(560, 214 + widestValue + 72))
const settingsHeight = rows.length * rowHeight + 16
window = worMakeWindow({ width: width, height: 1, title: windowTitle, delegate: controller })
const chromeHeight = Number(window.frame.size.height) - Number(window.contentView.frame.size.height)
const maximumHeight = Math.max(1, Math.floor(Number(screenFrame.size.height) - chromeHeight))
const scrollView = $.NSScrollView.alloc.initWithFrame($.NSMakeRect(24, 86, width - 48, settingsHeight))
scrollView.borderType = $.NSBezelBorder
scrollView.hasHorizontalScroller = false
scrollView.hasVerticalScroller = false
scrollView.autohidesScrollers = false
scrollView.scrollerStyle = $.NSScrollerStyleLegacy
scrollView.tile
//The panel border reduces its viewport; include it before deciding whether any rows overflow.
const panelBorderHeight = settingsHeight - Number(scrollView.contentSize.height)
const height = Math.min(maximumHeight, Math.max(320, settingsHeight + panelBorderHeight + 220))
window.setContentSize($.NSMakeSize(width, height))
window.center
scrollView.frame = $.NSMakeRect(24, 86, width - 48, height - 220)
scrollView.tile
const needsScrolling = settingsHeight > Number(scrollView.contentSize.height)
scrollView.hasVerticalScroller = needsScrolling
scrollView.tile

const content = window.contentView

const heading = $.NSTextField.labelWithString('Review flash settings')
heading.font = $.NSFont.systemFontOfSizeWeight(17, $.NSFontWeightBold)
heading.frame = $.NSMakeRect(24, height - 54, width - 48, 26)
content.addSubview(heading)

const target = $.NSTextField.labelWithString('Target: ' + targetDevice)
target.font = $.NSFont.systemFontOfSizeWeight(12, $.NSFontWeightMedium)
target.textColor = $.NSColor.secondaryLabelColor
target.frame = $.NSMakeRect(24, height - 80, width - 48, 20)
content.addSubview(target)

const warning = $.NSTextField.labelWithString('All data on the target drive will be erased.')
if (rows.some(function (row) { return row.label === 'Dry run' && row.value.indexOf('Yes') === 0 })) {
  warning.stringValue = 'Inspection only. No target drive will be modified.'
}
warning.font = $.NSFont.systemFontOfSizeWeight(13, $.NSFontWeightSemibold)
warning.textColor = $.NSColor.systemRedColor
warning.frame = $.NSMakeRect(24, height - 114, width - 48, 22)
content.addSubview(warning)

const documentWidth = Number(scrollView.contentSize.width)
const documentHeight = Math.max(settingsHeight, Number(scrollView.contentSize.height))
const documentView = $.NSView.alloc.initWithFrame($.NSMakeRect(0, 0, documentWidth, documentHeight))
scrollView.documentView = documentView
content.addSubview(scrollView)

let y = documentHeight - rowHeight - 8
for (let i = 0; i < rows.length; i++) {
  const label = $.NSTextField.labelWithString(rows[i].label)
  label.font = $.NSFont.systemFontOfSizeWeight(12, $.NSFontWeightMedium)
  label.textColor = $.NSColor.secondaryLabelColor
  label.alignment = $.NSTextAlignmentRight
  label.frame = $.NSMakeRect(8, y, 190, 20)
  documentView.addSubview(label)

  const value = $.NSTextField.labelWithString(rows[i].value)
  value.font = $.NSFont.systemFontOfSizeWeight(12, $.NSFontWeightRegular)
  value.lineBreakMode = $.NSLineBreakByTruncatingMiddle
  value.frame = $.NSMakeRect(214, y, Math.max(1, documentWidth - 222), 20)
  documentView.addSubview(value)
  y -= rowHeight
}
documentView.scrollPoint($.NSMakePoint(0, Math.max(0, documentHeight - Number(scrollView.contentSize.height))))
scrollView.reflectScrolledClipView(scrollView.contentView)

const guidance = $.NSTextField.labelWithString('Flash starts image preparation. Disk writing follows verification and administrator approval.')
guidance.font = $.NSFont.systemFontOfSizeWeight(11, $.NSFontWeightRegular)
guidance.textColor = $.NSColor.secondaryLabelColor
guidance.frame = $.NSMakeRect(24, 58, width - 48, 18)
content.addSubview(guidance)

const flashButton = $.NSButton.buttonWithTitleTargetAction('Flash', controller, 'flashClicked:')
flashButton.bezelStyle = $.NSBezelStyleRounded
flashButton.keyEquivalent = '\r'
flashButton.sizeToFit
const flashWidth = Math.max(132, flashButton.frame.size.width)
flashButton.frame = $.NSMakeRect(width - 24 - flashWidth, 20, flashWidth, 32)
content.addSubview(flashButton)

const backButton = $.NSButton.buttonWithTitleTargetAction('Back', controller, 'backClicked:')
backButton.bezelStyle = $.NSBezelStyleRounded
backButton.keyEquivalent = '\u001b'
backButton.sizeToFit
const backWidth = Math.max(112, backButton.frame.size.width)
backButton.frame = $.NSMakeRect(width - 24 - flashWidth - 8 - backWidth, 20, backWidth, 32)
content.addSubview(backButton)

const advancedButton = $.NSButton.buttonWithTitleTargetAction('Advanced...', controller, 'advancedClicked:')
advancedButton.bezelStyle = $.NSBezelStyleRounded
advancedButton.sizeToFit
const advancedWidth = Math.max(130, advancedButton.frame.size.width)
advancedButton.frame = $.NSMakeRect(24, 20, advancedWidth, 32)
content.addSubview(advancedButton)

window.makeKeyAndOrderFront(null)
if (!app.isActive) app.requestUserAttention($.NSInformationalRequest)
app.activateIgnoringOtherApps(true)
worInstallWindowHandlers(controller)
app.runModalForWindow(window)
allowTermination = true
writeResult(selectedValue)
app.terminate(null)
JXA
)"
  result="$(wor_osascript -l JavaScript - "$rows" "$DEVICE" "$WOR_ICON_PATH" "$WOR_WINDOW_TITLE" "$WOR_APP_TITLE" <<<"$confirm_jxa")" || return 1
  [ "$result" == Cancel ] && return 1
  printf '%s\n' "$result"
}

prepare_release_choices() { #Input: kind, current tag. Sets choices and a visible warning for either GUI.
  local kind="$1" current="$2" versions latest_result tag
  RELEASE_CHOICES='' RELEASE_CHOICES_WARNING='' RELEASE_CHOICES_LATEST='' RELEASE_CHOICES_LABELS=''
  if versions="$(list_release_versions "$kind" 2>&1)";then
    if ! grep -qxF "$current" <<<"$versions";then
      RELEASE_CHOICES_WARNING="The configured version $current is not in the published release list."
    fi
    if [ "$(wor_advanced_recommended "$kind" "$RPI_MODEL")" == 1 ];then
      if latest_result="$(list_release_versions "$kind" latest 2>&1)" && release_version_is_valid "$latest_result";then
        RELEASE_CHOICES_LATEST="$latest_result"
      else
        RELEASE_CHOICES_WARNING="${RELEASE_CHOICES_WARNING:+$RELEASE_CHOICES_WARNING }Could not identify the latest recommended $kind release. $latest_result Keeping the current selection."
        warning "$RELEASE_CHOICES_WARNING"
      fi
    fi
  else
    RELEASE_CHOICES_WARNING="$versions Only the current selection is available; reopen Advanced Options to retry."
    versions=''
    warning "$RELEASE_CHOICES_WARNING"
  fi
  if release_version_is_valid "$current";then
    RELEASE_CHOICES="$(printf '%s\n%s\n%s\n' "$current" "$RELEASE_CHOICES_LATEST" "$versions" | awk 'NF && !seen[$0]++')"
  else
    RELEASE_CHOICES="$versions"
    RELEASE_CHOICES_WARNING="The configured $kind version is invalid. Select an available release."
    warning "$RELEASE_CHOICES_WARNING"
  fi
  RELEASE_CHOICES_DEFAULT="$(release_dropdown_version "$kind" "$current" "$RELEASE_CHOICES_LATEST")"
  if grep -qxF "$RELEASE_CHOICES_DEFAULT" <<<"$RELEASE_CHOICES";then
    RELEASE_CHOICES="$(printf '%s\n%s\n' "$RELEASE_CHOICES_DEFAULT" "$RELEASE_CHOICES" | awk 'NF && !seen[$0]++')"
  fi
  RELEASE_CHOICES_LABELS="$(
    while IFS= read -r tag;do
      [ -n "$tag" ] || continue
      wor_release_version_label "$tag" "$RELEASE_CHOICES_LATEST"
      printf '\n'
    done <<<"$RELEASE_CHOICES"
  )"
  if [ "$USE_CACHE" == 2 ];then
    RELEASE_CHOICES_WARNING="${RELEASE_CHOICES_WARNING:+$RELEASE_CHOICES_WARNING }Trust cache may reuse another version. Use checked cache to fetch this selection."
  fi
}

release_choice_tag() { #Input: display label, available tags, verified latest tag. Validate before removing annotations.
  local tag="${1%% *}"
  if ! release_version_is_valid "$tag" || ! grep -qxF "$tag" <<<"$2" \
    || [ "$1" != "$(wor_release_version_label "$tag" "$3")" ];then
    warning "The selected release is not in the available choices."
    return 1
  fi
  printf '%s' "$tag"
}

save_advanced_preferences() { #Keep automatic Linux form refreshes transactional until OK is clicked.
  ADVANCED_ORIGINAL_VALUES=("$OOBE_NETWORK_BYPASS" "$PI4_AUTO_DISABLE_3GB" "${UEFI_USE_LATEST:-}" "$DRIVERS_USE_LATEST"
    "$UEFI_VER_PI3" "$UEFI_VER_PI4" "$UEFI_VER_PI5" "$DRIVER_VER" "$SKIP_IMAGE_VERIFICATION" "$DRY_RUN"
    "$USE_CACHE" "$WINDOWS_ACCOUNT_SETUP" "$WINDOWS_ACCOUNT_USERNAME" "$WINDOWS_ACCOUNT_PASSWORD"
    "$WINDOWS_LOCALE_SETUP" "$WINDOWS_LOCALE" "$WIN_LANG" "$PLAY_SOUND" "$COMPLETION_SOUND" "$SHOW_NOTIFICATION"
    "$APPLY_CUSTOM_CONFIG_TXT" "$CONFIG_TXT" "$DL_DIR" "${WOR_SELECTED_RELEASES:-}")
}

restore_advanced_preferences() {
  OOBE_NETWORK_BYPASS="${ADVANCED_ORIGINAL_VALUES[0]}" PI4_AUTO_DISABLE_3GB="${ADVANCED_ORIGINAL_VALUES[1]}"
  UEFI_USE_LATEST="${ADVANCED_ORIGINAL_VALUES[2]}" DRIVERS_USE_LATEST="${ADVANCED_ORIGINAL_VALUES[3]}"
  UEFI_VER_PI3="${ADVANCED_ORIGINAL_VALUES[4]}" UEFI_VER_PI4="${ADVANCED_ORIGINAL_VALUES[5]}"
  UEFI_VER_PI5="${ADVANCED_ORIGINAL_VALUES[6]}" DRIVER_VER="${ADVANCED_ORIGINAL_VALUES[7]}"
  SKIP_IMAGE_VERIFICATION="${ADVANCED_ORIGINAL_VALUES[8]}" DRY_RUN="${ADVANCED_ORIGINAL_VALUES[9]}"
  USE_CACHE="${ADVANCED_ORIGINAL_VALUES[10]}" WINDOWS_ACCOUNT_SETUP="${ADVANCED_ORIGINAL_VALUES[11]}"
  WINDOWS_ACCOUNT_USERNAME="${ADVANCED_ORIGINAL_VALUES[12]}" WINDOWS_ACCOUNT_PASSWORD="${ADVANCED_ORIGINAL_VALUES[13]}"
  WINDOWS_LOCALE_SETUP="${ADVANCED_ORIGINAL_VALUES[14]}" WINDOWS_LOCALE="${ADVANCED_ORIGINAL_VALUES[15]}"
  WIN_LANG="${ADVANCED_ORIGINAL_VALUES[16]}" PLAY_SOUND="${ADVANCED_ORIGINAL_VALUES[17]}"
  COMPLETION_SOUND="${ADVANCED_ORIGINAL_VALUES[18]}" SHOW_NOTIFICATION="${ADVANCED_ORIGINAL_VALUES[19]}"
  APPLY_CUSTOM_CONFIG_TXT="${ADVANCED_ORIGINAL_VALUES[20]}" CONFIG_TXT="${ADVANCED_ORIGINAL_VALUES[21]}"
  DL_DIR="${ADVANCED_ORIGINAL_VALUES[22]}"
  WOR_SELECTED_RELEASES="${ADVANCED_ORIGINAL_VALUES[23]}"
}

macos_advanced_options() { #Collects customization and explicit release versions without changing them on Back.
  if is_iot_core;then
    macos_iot_options
    return
  fi
  local advanced_jxa checkbox_spec result status line i uefi_pinned oobe_applicable pi4_applicable drivers_applicable windows_family config_scope lang_spec locale_spec l_code l_name sel_win_lang sound_spec sel_sound LC_ALL
  local uefi_versions driver_versions='' uefi_version_warning driver_version_warning='' selected_uefi selected_driver
  local uefi_dropdown_default driver_dropdown_default="$DRIVER_VER" uefi_version_labels driver_version_labels=''
  uefi_pinned="$(uefi_pinned_version)"
  windows_family="$(windows_version_label)"
  advanced_option_applies oobe && oobe_applicable=1 || oobe_applicable=0
  advanced_option_applies pi4 && pi4_applicable=1 || pi4_applicable=0
  advanced_option_applies drivers && drivers_applicable=1 || drivers_applicable=0
  #in recovery mode this config.txt boots the installer media; WoR-PE writes the target drive's own copy
  config_scope="$(wor_config_scope "$CAN_INSTALL_ON_SAME_DRIVE")"
  #labels and caution flags come from gui.sh so the Linux dialog cannot describe these differently
  checkbox_spec="$(wor_advanced_label oobe "$uefi_pinned" "$DRIVER_VER" "$RPI_MODEL" "$windows_family" "$CAN_INSTALL_ON_SAME_DRIVE")	$OOBE_NETWORK_BYPASS	$oobe_applicable	$(wor_advanced_caution oobe)	0
$(wor_advanced_label pi4 "$uefi_pinned" "$DRIVER_VER" "$RPI_MODEL")	$PI4_AUTO_DISABLE_3GB	$pi4_applicable	$(wor_advanced_caution pi4)	0
$(wor_advanced_label uefi "$uefi_pinned" "$DRIVER_VER" "$RPI_MODEL")	$(uefi_use_latest)	1	$(wor_advanced_caution uefi "$RPI_MODEL")	$(wor_advanced_recommended uefi "$RPI_MODEL")
$(wor_advanced_label drivers "$uefi_pinned" "$DRIVER_VER" "$RPI_MODEL")	$DRIVERS_USE_LATEST	$drivers_applicable	$(wor_advanced_caution drivers)	$(wor_advanced_recommended drivers "$RPI_MODEL")
$(wor_advanced_label verify "$uefi_pinned" "$DRIVER_VER" "$RPI_MODEL" "$windows_family" "$CAN_INSTALL_ON_SAME_DRIVE")	$SKIP_IMAGE_VERIFICATION	1	$(wor_advanced_caution verify)	0
$(wor_advanced_label dryrun "$uefi_pinned" "$DRIVER_VER" "$RPI_MODEL")	$DRY_RUN	1	$(wor_advanced_caution dryrun)	0"

  lang_spec=""
  if advanced_option_applies language;then
    while IFS=: read -r l_code l_name ;do
      [ -z "$l_code" ] && continue
      lang_spec+="${l_code}	${l_name}"$'\n'
    done < <(list_langs_preferred)
  fi

  locale_spec="$(list_windows_locale_options)"
  sound_spec="$(wor_sound_options)"
  prepare_release_choices uefi "$uefi_pinned"
  uefi_versions="$RELEASE_CHOICES" uefi_version_warning="$RELEASE_CHOICES_WARNING"
  uefi_dropdown_default="$RELEASE_CHOICES_DEFAULT" uefi_version_labels="$RELEASE_CHOICES_LABELS"
  if [ "$drivers_applicable" == 1 ];then
    prepare_release_choices drivers "$DRIVER_VER"
    driver_versions="$RELEASE_CHOICES" driver_version_warning="$RELEASE_CHOICES_WARNING"
    driver_dropdown_default="$RELEASE_CHOICES_DEFAULT" driver_version_labels="$RELEASE_CHOICES_LABELS"
  fi

  advanced_jxa="$(wor_jxa_window_lib; cat <<'JXA'
ObjC.import('AppKit')
ObjC.import('Foundation')
ObjC.import('stdlib')
const args = $.NSProcessInfo.processInfo.arguments
const checkboxSpec = ObjC.unwrap(args.objectAtIndex(4))
const configTxtDefault = ObjC.unwrap(args.objectAtIndex(5))
const applyConfigDefault = ObjC.unwrap(args.objectAtIndex(6))
const iconPath = ObjC.unwrap(args.objectAtIndex(7))
const appTitle = ObjC.unwrap(args.objectAtIndex(8))
const applyConfigLabel = ObjC.unwrap(args.objectAtIndex(9))
const cacheModeDefault = ObjC.unwrap(args.objectAtIndex(10))
const accountUsernameDefault = ObjC.unwrap(args.objectAtIndex(11))
const accountPasswordDefault = ObjC.unwrap(args.objectAtIndex(12))
const localeDefault = ObjC.unwrap(args.objectAtIndex(13))
const langSpec = ObjC.unwrap(args.objectAtIndex(14))
const currentLangCode = ObjC.unwrap(args.objectAtIndex(15))
const accountSetupDefault = ObjC.unwrap(args.objectAtIndex(16))
const localeSetupDefault = ObjC.unwrap(args.objectAtIndex(17))
const localeSpec = ObjC.unwrap(args.objectAtIndex(18))
const windowTitle = ObjC.unwrap(args.objectAtIndex(19))
const soundSpec = ObjC.unwrap(args.objectAtIndex(20) || '')
const playSoundDefault = ObjC.unwrap(args.objectAtIndex(21) || '1')
const soundDefault = ObjC.unwrap(args.objectAtIndex(22) || '')
const showNotificationDefault = ObjC.unwrap(args.objectAtIndex(23) || '1')
const contextText = ObjC.unwrap(args.objectAtIndex(24))
const setupTitle = ObjC.unwrap(args.objectAtIndex(25))
const uefiVersions = ObjC.unwrap(args.objectAtIndex(26)).split('\n').filter(Boolean)
const currentUefiVersion = ObjC.unwrap(args.objectAtIndex(27))
const driverVersions = ObjC.unwrap(args.objectAtIndex(28)).split('\n').filter(Boolean)
const currentDriverVersion = ObjC.unwrap(args.objectAtIndex(29))
const uefiVersionWarning = ObjC.unwrap(args.objectAtIndex(30))
const driverVersionWarning = ObjC.unwrap(args.objectAtIndex(31))
const uefiVersionLabels = ObjC.unwrap(args.objectAtIndex(32)).split('\n').filter(Boolean)
const driverVersionLabels = ObjC.unwrap(args.objectAtIndex(33)).split('\n').filter(Boolean)

const soundOptions = []
let initialSoundIdx = 0
if (soundSpec && soundSpec.length > 0) {
  const soundLines = soundSpec.split('\n')
  for (let i = 0; i < soundLines.length; i++) {
    const line = soundLines[i].trim()
    if (!line) continue
    const parts = line.split('\t')
    soundOptions.push({ value: parts[0], label: parts[1] || parts[0] })
    if (parts[0] === soundDefault) initialSoundIdx = soundOptions.length - 1
  }
}

const rows = checkboxSpec.split('\n').map(function(line) {
  const parts = line.split('\t')
  return { label: parts[0], checked: parts[1] === '1', enabled: parts[2] !== '0', caution: parts[3] === '1', recommended: parts[4] === '1' }
})

const langOptions = []
let initialLangIdx = 0
if (langSpec && langSpec.length > 0) {
  const langLines = langSpec.split('\n')
  for (let i = 0; i < langLines.length; i++) {
    const line = langLines[i].trim()
    if (!line) continue
    const parts = line.split('\t')
    const code = parts[0]
    const name = parts[1] || code
    const label = name + ' (' + code + ')'
    langOptions.push({ code: code, label: label })
    if (code === currentLangCode) {
      initialLangIdx = langOptions.length - 1
    }
  }
}

const localeOptions = []
let initialLocaleIdx = 0
if (localeSpec && localeSpec.length > 0) {
  const localeLines = localeSpec.split('\n')
  for (let i = 0; i < localeLines.length; i++) {
    const line = localeLines[i].trim()
    if (!line) continue
    const parts = line.split('\t')
    const locale = parts[0]
    const label = parts[1] || locale
    localeOptions.push({ locale: locale, label: label })
    if (locale.toLowerCase() === localeDefault.toLowerCase()) {
      initialLocaleIdx = localeOptions.length - 1
    }
  }
}

$.NSProcessInfo.processInfo.processName = appTitle
const app = $.NSApplication.sharedApplication
app.setActivationPolicy($.NSApplicationActivationPolicyRegular)
worSetAppIcon(app, iconPath)

//this screen adds an Edit menu on top of the shared one: without it Cmd+C/V/X/A have nothing to route to
const mainMenu = worInstallAppMenu(app, appTitle, windowTitle, iconPath)
const editMenuItem = $.NSMenuItem.alloc.init
mainMenu.addItem(editMenuItem)
const editMenu = $.NSMenu.alloc.initWithTitle('Edit')
editMenuItem.submenu = editMenu
editMenu.addItemWithTitleActionKeyEquivalent('Cut', 'cut:', 'x')
editMenu.addItemWithTitleActionKeyEquivalent('Copy', 'copy:', 'c')
editMenu.addItemWithTitleActionKeyEquivalent('Paste', 'paste:', 'v')
editMenu.addItemWithTitleActionKeyEquivalent('Select All', 'selectAll:', 'a')

let window, applyConfigCheckbox, editConfigButton, cachePopup, winLangPopup, accountCheckbox, accountUsernameField, accountPasswordField, localeCheckbox, localePopup, playSoundCheckbox, soundPopup, soundLabel, notificationCheckbox
let checkboxes = []
let versionRows = []
let confirmed = false
let configTxtValue = configTxtDefault

function updateConfigEditableState() {
  const enabled = applyConfigCheckbox.state == 1
  editConfigButton.enabled = enabled
  editConfigButton.alphaValue = enabled ? 1.0 : 0.5
}

function editConfigTxt() {
  const result = worEditText(configTxtValue, 'View / Edit config.txt', 'These settings control Raspberry Pi firmware and boot behavior.', iconPath)
  if (result.saved) configTxtValue = result.text
}

function updateAccountEditableState() {
  const enabled = accountCheckbox.state == 1
  accountUsernameField.enabled = enabled
  accountPasswordField.enabled = enabled
  accountUsernameField.alphaValue = enabled ? 1.0 : 0.5
  accountPasswordField.alphaValue = enabled ? 1.0 : 0.5
}

function updateLocaleEditableState() {
  const enabled = localeCheckbox.state == 1
  localePopup.enabled = enabled
  localePopup.alphaValue = enabled ? 1.0 : 0.5
}

function updateSoundEnabledState() {
  if (!soundPopup) return
  const enabled = playSoundCheckbox.state == 1
  soundPopup.enabled = enabled
  soundPopup.alphaValue = enabled ? 1.0 : 0.5
  soundLabel.alphaValue = enabled ? 1.0 : 0.5
}

function previewSelectedSound() {
  if (!soundPopup || playSoundCheckbox.state != 1) return
  const idx = soundPopup.indexOfSelectedItem
  if (idx < 0 || idx >= soundOptions.length) return
  //soundNamed hands back a truthy wrapper for a name it cannot find, so isNil is the only guard
  const preview = $.NSSound.soundNamed(soundOptions[idx].value)
  if (!preview.isNil()) preview.play
}

const Controller = ObjC.registerSubclass({
  name: 'WorAdvancedController',
  superclass: 'NSObject',
  methods: {
    'okClicked:': {
      types: ['void', ['id']],
      implementation: function() {
        for (let i = 0; i < versionRows.length; i++) {
          const item = versionRows[i]
          if (checkboxes[item.checkboxIndex].state != 1 && item.popup.indexOfSelectedItem < 0) {
            const alert = $.NSAlert.alloc.init
            alert.messageText = 'Select a released version or enable latest'
            alert.informativeText = 'The release list could not provide a usable selection.'
            alert.runModal
            return
          }
        }
        confirmed = true
        app.stopModalWithCode($.NSOKButton)
        window.orderOut(null)
      }
    },
    'cancelClicked:': {
      types: ['void', ['id']],
      implementation: function() {
        confirmed = false
        app.stopModalWithCode($.NSCancelButton)
        window.orderOut(null)
      }
    },
    'applyConfigToggled:': {
      types: ['void', ['id']],
      implementation: function() {
        updateConfigEditableState()
      }
    },
    'releaseChoiceToggled:': {
      types: ['void', ['id']],
      implementation: function() {
        layoutAdvancedOptions()
      }
    },
    'editConfigClicked:': {
      types: ['void', ['id']],
      implementation: function() {
        editConfigTxt()
      }
    },
    'accountToggled:': {
      types: ['void', ['id']],
      implementation: function() {
        updateAccountEditableState()
      }
    },
    'localeToggled:': {
      types: ['void', ['id']],
      implementation: function() {
        updateLocaleEditableState()
      }
    },
    'playSoundToggled:': {
      types: ['void', ['id']],
      implementation: function() {
        updateSoundEnabledState()
        previewSelectedSound()
      }
    },
    'soundPicked:': {
      types: ['void', ['id']],
      implementation: function() {
        previewSelectedSound()
      }
    },
    'windowWillClose:': {
      types: ['void', ['id']],
      implementation: function() {
        confirmed = false
        app.stopModalWithCode($.NSCancelButton)
      }
    },
    //right-click Quit from the Dock/app-switcher sends terminate: to NSApp; since this window is
    //hosted by osascript rather than a full NSApplicationMain run loop, that does not otherwise exit the process
    'handleQuitEvent:withReplyEvent:': {
      types: ['void', ['id', 'id']],
      implementation: function() {
        $.exit(0)
      }
    },
    'pumpEvents:': {
      types: ['void', ['id']],
      implementation: function() {
        $.NSRunLoop.currentRunLoop.runModeBeforeDate($.NSDefaultRunLoopMode, $.NSDate.dateWithTimeIntervalSinceNow(0.01))
      }
    },
    //clicking the Dock icon sends aevt/rapp; without a handler a minimised window can never come back
    'handleReopenEvent:withReplyEvent:': {
      types: ['void', ['id', 'id']],
      implementation: function() {
        if (window.isMiniaturized) window.deminiaturize(null)
        window.makeKeyAndOrderFront(null)
        app.activateIgnoringOtherApps(true)
      }
    },
    'applicationShouldTerminate:': {
      types: ['NSUInteger', ['id']],
      implementation: function() {
        $.exit(0)
      }
    }
  }
})
const controller = $.WorAdvancedController.alloc.init
app.setDelegate(controller)

const screenFrame = $.NSScreen.mainScreen.visibleFrame
const rowHeight = 26
const desiredWidth = 640
const width = Math.min(desiredWidth, screenFrame.size.width - 40)
//fixed layout: no drag-resize and no zoom/maximize button, only minimize (and restore) via the titlebar
window = worMakeWindow({ width: width, height: 1, title: windowTitle + ' | Advanced Options', delegate: controller })

//Lay out downward from zero, then measure and position the actual controls before showing the window.
const content = $.NSView.alloc.initWithFrame($.NSMakeRect(0, 0, width, 0))
content.autoresizesSubviews = false
let y = 0
const contextLabel = $.NSTextField.wrappingLabelWithString(contextText)
contextLabel.font = $.NSFont.systemFontOfSize(12)
//Measure with room for the overflow scrollbar so wrapped text stays readable in either layout.
const contextWidth = width - 40 - Number($.NSScroller.scrollerWidthForControlSizeScrollerStyle($.NSControlSizeRegular, $.NSScrollerStyleLegacy))
const contextHeight = Math.ceil(Number(contextLabel.cell.cellSizeForBounds($.NSMakeRect(0, 0, contextWidth, 2000)).height))
contextLabel.frame = $.NSMakeRect(20, y - contextHeight, width - 40, contextHeight)
contextLabel.autoresizingMask = $.NSViewWidthSizable | $.NSViewMinYMargin
content.addSubview(contextLabel)
y -= contextHeight + 16
function addSectionHeader(title) {
  y -= 8
  const section = $.NSTextField.labelWithString(title)
  section.font = $.NSFont.systemFontOfSizeWeight(12, $.NSFontWeightSemibold)
  section.textColor = $.NSColor.secondaryLabelColor
  section.frame = $.NSMakeRect(20, y, width - 40, 18)
  section.autoresizingMask = $.NSViewWidthSizable | $.NSViewMinYMargin
  content.addSubview(section)
  y -= 24
}

function addVersionSelector(index) {
  const versions = index === 2 ? uefiVersions : driverVersions
  const labels = index === 2 ? uefiVersionLabels : driverVersionLabels
  const current = index === 2 ? currentUefiVersion : currentDriverVersion
  const warningText = index === 2 ? uefiVersionWarning : driverVersionWarning
  const height = warningText.length > 0 ? 82 : 34
  const view = $.NSView.alloc.initWithFrame($.NSMakeRect(20, y - height + 20, width - 40, height))
  view.autoresizingMask = $.NSViewWidthSizable | $.NSViewMinYMargin
  const label = $.NSTextField.labelWithString(index === 2 ? 'UEFI version:' : 'Driver version:')
  label.frame = $.NSMakeRect(0, height - 26, 120, 20)
  label.autoresizingMask = $.NSViewMaxXMargin | $.NSViewMinYMargin
  view.addSubview(label)
  const popup = $.NSPopUpButton.alloc.initWithFramePullsDown($.NSMakeRect(126, height - 30, width - 166, 26), false)
  for (let i = 0; i < versions.length; i++) popup.addItemWithTitle($(labels[i]))
  popup.selectItemAtIndex(versions.indexOf(current))
  popup.autoresizingMask = $.NSViewWidthSizable | $.NSViewMinYMargin
  view.addSubview(popup)
  if (warningText.length > 0) {
    const note = $.NSTextField.wrappingLabelWithString(warningText)
    note.frame = $.NSMakeRect(0, 0, width - 40, 44)
    note.font = $.NSFont.systemFontOfSize(11)
    note.textColor = $.NSColor.systemOrangeColor
    note.autoresizingMask = $.NSViewWidthSizable | $.NSViewMinYMargin
    view.addSubview(note)
  }
  content.addSubview(view)
  versionRows.push({ view: view, index: Number(content.subviews.count) - 1, height: height, popup: popup, versions: versions, checkboxIndex: index })
  y -= height
}

for (let i = 0; i < rows.length; i++) {
  if (!rows[i].enabled) {
    checkboxes.push(null)
    continue
  }
  if (i < 2 && (i === 0 || !rows[0].enabled)) addSectionHeader(setupTitle)
  if (i === 2) addSectionHeader('Firmware and drivers')
  if (i === 4) addSectionHeader('Validation')
  const checkbox = $.NSButton.checkboxWithTitleTargetAction(rows[i].label, undefined, undefined)
  if (rows[i].caution) worAnnotateCheckbox(checkbox, rows[i].label, 'Not recommended', $.NSColor.systemRedColor)
  else if (rows[i].recommended) worAnnotateCheckbox(checkbox, rows[i].label, 'Recommended', $.NSColor.systemGreenColor)
  checkbox.frame = $.NSMakeRect(20, y, width - 40, 20)
  checkbox.state = rows[i].checked ? 1 : 0
  checkbox.enabled = rows[i].enabled
  if (i === 2 || i === 3) {
    checkbox.target = controller
    checkbox.action = 'releaseChoiceToggled:'
  }
  checkbox.autoresizingMask = $.NSViewWidthSizable | $.NSViewMinYMargin
  content.addSubview(checkbox)
  checkboxes.push(checkbox)
  y -= rowHeight
  if (i === 2 || i === 3) addVersionSelector(i)
}

addSectionHeader('Downloads')
//USE_CACHE has three values, so it needs a menu rather than a checkbox
const cacheLabel = $.NSTextField.labelWithString('Downloaded files:')
cacheLabel.font = $.NSFont.systemFontOfSizeWeight(12, $.NSFontWeightMedium)
cacheLabel.frame = $.NSMakeRect(20, y - 2, 120, 20)
cacheLabel.autoresizingMask = $.NSViewMaxXMargin | $.NSViewMinYMargin
content.addSubview(cacheLabel)
cachePopup = $.NSPopUpButton.alloc.initWithFramePullsDown($.NSMakeRect(146, y - 6, width - 166, 26), false)
cachePopup.addItemWithTitle('Re-download everything, ignoring the cache')
cachePopup.addItemWithTitle('Reuse cached files when they still match (recommended)')
cachePopup.addItemWithTitle('Trust the cache without checking it')
cachePopup.selectItemAtIndex(cacheModeDefault === '0' ? 0 : (cacheModeDefault === '2' ? 2 : 1))
cachePopup.autoresizingMask = $.NSViewWidthSizable | $.NSViewMinYMargin
content.addSubview(cachePopup)
y -= rowHeight + 8

if (langOptions.length > 0) {
  const winLangLabel = $.NSTextField.labelWithString('Choose Windows language:')
  winLangLabel.font = $.NSFont.systemFontOfSizeWeight(12, $.NSFontWeightMedium)
  winLangLabel.frame = $.NSMakeRect(20, y - 2, 170, 20)
  winLangLabel.autoresizingMask = $.NSViewMaxXMargin | $.NSViewMinYMargin
  content.addSubview(winLangLabel)
  winLangPopup = $.NSPopUpButton.alloc.initWithFramePullsDown($.NSMakeRect(196, y - 6, width - 216, 26), false)
  for (let i = 0; i < langOptions.length; i++) {
    winLangPopup.addItemWithTitle($(langOptions[i].label))
  }
  winLangPopup.selectItemAtIndex(initialLangIdx)
  winLangPopup.autoresizingMask = $.NSViewWidthSizable | $.NSViewMinYMargin
  content.addSubview(winLangPopup)
  y -= rowHeight + 8
}

addSectionHeader('Notifications')
if (soundOptions.length > 0) {
  playSoundCheckbox = $.NSButton.checkboxWithTitleTargetAction('Play a sound when the flash finishes', controller, 'playSoundToggled:')
  playSoundCheckbox.frame = $.NSMakeRect(20, y, width - 40, 20)
  playSoundCheckbox.state = playSoundDefault === '0' ? 0 : 1
  playSoundCheckbox.autoresizingMask = $.NSViewWidthSizable | $.NSViewMinYMargin
  content.addSubview(playSoundCheckbox)
  y -= rowHeight

  soundLabel = $.NSTextField.labelWithString('Completion sound:')
  soundLabel.font = $.NSFont.systemFontOfSizeWeight(12, $.NSFontWeightMedium)
  soundLabel.frame = $.NSMakeRect(20, y - 2, 130, 20)
  soundLabel.autoresizingMask = $.NSViewMaxXMargin | $.NSViewMinYMargin
  content.addSubview(soundLabel)
  soundPopup = $.NSPopUpButton.alloc.initWithFramePullsDown($.NSMakeRect(156, y - 6, width - 176, 26), false)
  for (let i = 0; i < soundOptions.length; i++) {
    soundPopup.addItemWithTitle($(soundOptions[i].label))
  }
  soundPopup.selectItemAtIndex(initialSoundIdx)
  soundPopup.autoresizingMask = $.NSViewWidthSizable | $.NSViewMinYMargin
  //hearing the choice is the whole point, so play it as it is picked
  soundPopup.target = controller
  soundPopup.action = 'soundPicked:'
  content.addSubview(soundPopup)
  updateSoundEnabledState()
  y -= rowHeight + 8

}
notificationCheckbox = $.NSButton.checkboxWithTitleTargetAction('Show a completion notification', undefined, undefined)
notificationCheckbox.frame = $.NSMakeRect(20, y, width - 40, 20)
notificationCheckbox.state = showNotificationDefault === '0' ? 0 : 1
notificationCheckbox.autoresizingMask = $.NSViewWidthSizable | $.NSViewMinYMargin
content.addSubview(notificationCheckbox)
y -= rowHeight

addSectionHeader('Windows account')
accountCheckbox = $.NSButton.checkboxWithTitleTargetAction('Create a local Windows administrator account', controller, 'accountToggled:')
accountCheckbox.frame = $.NSMakeRect(20, y, width - 40, 20)
accountCheckbox.state = accountSetupDefault === '1' ? 1 : 0
accountCheckbox.autoresizingMask = $.NSViewWidthSizable | $.NSViewMinYMargin
content.addSubview(accountCheckbox)
y -= rowHeight

function addAdvancedField(label, value, secure) {
  const fieldLabel = $.NSTextField.labelWithString(label)
  fieldLabel.frame = $.NSMakeRect(20, y, 170, 20)
  fieldLabel.autoresizingMask = $.NSViewMaxXMargin | $.NSViewMinYMargin
  content.addSubview(fieldLabel)
  const field = secure ? $.NSSecureTextField.alloc.initWithFrame($.NSMakeRect(196, y - 2, width - 216, 24)) : $.NSTextField.alloc.initWithFrame($.NSMakeRect(196, y - 2, width - 216, 24))
  field.stringValue = $(value)
  field.autoresizingMask = $.NSViewWidthSizable | $.NSViewMinYMargin
  content.addSubview(field)
  y -= rowHeight
  return field
}
accountUsernameField = addAdvancedField('Windows username:', accountUsernameDefault, false)
accountPasswordField = addAdvancedField('Windows password:', accountPasswordDefault, true)
updateAccountEditableState()

addSectionHeader('Regional settings')
localeCheckbox = $.NSButton.checkboxWithTitleTargetAction('Configure Windows keyboard and regional settings', controller, 'localeToggled:')
localeCheckbox.frame = $.NSMakeRect(20, y, width - 40, 20)
localeCheckbox.state = localeSetupDefault === '1' ? 1 : 0
localeCheckbox.autoresizingMask = $.NSViewWidthSizable | $.NSViewMinYMargin
content.addSubview(localeCheckbox)
y -= rowHeight

const localeLabel = $.NSTextField.labelWithString('Windows locale:')
localeLabel.frame = $.NSMakeRect(20, y, 170, 20)
localeLabel.autoresizingMask = $.NSViewMaxXMargin | $.NSViewMinYMargin
content.addSubview(localeLabel)
localePopup = $.NSPopUpButton.alloc.initWithFramePullsDown($.NSMakeRect(196, y - 6, width - 216, 26), false)
for (let i = 0; i < localeOptions.length; i++) {
  localePopup.addItemWithTitle($(localeOptions[i].label))
}
if (localeOptions.length === 0) {
  localeOptions.push({ locale: localeDefault, label: localeDefault })
  localePopup.addItemWithTitle($(localeDefault))
}
localePopup.selectItemAtIndex(initialLocaleIdx)
localePopup.autoresizingMask = $.NSViewWidthSizable | $.NSViewMinYMargin
content.addSubview(localePopup)
y -= rowHeight
updateLocaleEditableState()

addSectionHeader('Raspberry Pi boot config')
applyConfigCheckbox = $.NSButton.checkboxWithTitleTargetAction(applyConfigLabel, controller, 'applyConfigToggled:')
worAnnotateCheckbox(applyConfigCheckbox, applyConfigLabel, 'Recommended', $.NSColor.systemGreenColor)
applyConfigCheckbox.frame = $.NSMakeRect(20, y, width - 200, 20)
applyConfigCheckbox.state = applyConfigDefault === '1' ? 1 : 0
applyConfigCheckbox.autoresizingMask = $.NSViewWidthSizable | $.NSViewMinYMargin
content.addSubview(applyConfigCheckbox)
editConfigButton = $.NSButton.buttonWithTitleTargetAction('View / Edit…', controller, 'editConfigClicked:')
editConfigButton.bezelStyle = $.NSBezelStyleRounded
editConfigButton.frame = $.NSMakeRect(width - 160, y - 6, 140, 28)
editConfigButton.autoresizingMask = $.NSViewMinXMargin | $.NSViewMinYMargin
content.addSubview(editConfigButton)
updateConfigEditableState()

const baselineFrames = []
for (let i = 0; i < Number(content.subviews.count); i++) {
  const view = content.subviews.objectAtIndex(i)
  const frame = view.frame
  baselineFrames.push({ view: view, x: Number(frame.origin.x), y: Number(frame.origin.y), width: Number(frame.size.width), height: Number(frame.size.height) })
}
let scrollView = null
let bodyHeight = 0
let needsScrolling = false
let scrollHeight = 0

function layoutAdvancedOptions() {
const topPadding = 8
content.removeFromSuperview
if (scrollView) {
  scrollView.documentView = $()
  scrollView.removeFromSuperview
  scrollView = null
}
content.autoresizesSubviews = false
content.setFrameSize($.NSMakeSize(width, 0))
let removedHeight = 0
for (let i = 0; i < baselineFrames.length; i++) {
  const entry = baselineFrames[i]
  const selector = versionRows.find(function(item) { return item.index === i })
  if (selector) {
    const hidden = checkboxes[selector.checkboxIndex].state == 1
    entry.view.hidden = hidden
    if (hidden) {
      removedHeight += selector.height
      continue
    }
  }
  entry.view.frame = $.NSMakeRect(entry.x, entry.y + removedHeight, entry.width, entry.height)
}
const contentViews = content.subviews
let minimumY = 0
let maximumY = 0
for (let i = 0; i < Number(contentViews.count); i++) {
  const frame = contentViews.objectAtIndex(i).frame
  if (contentViews.objectAtIndex(i).isHidden) continue
  minimumY = Math.min(minimumY, Number(frame.origin.y))
  maximumY = Math.max(maximumY, Number(frame.origin.y) + Number(frame.size.height))
}
bodyHeight = Math.ceil(maximumY - minimumY) + 20 + topPadding
content.setFrameSize($.NSMakeSize(width, bodyHeight))
for (let i = 0; i < Number(contentViews.count); i++) {
  const view = contentViews.objectAtIndex(i)
  if (view.isHidden) continue
  const frame = view.frame
  view.setFrameOrigin($.NSMakePoint(Number(frame.origin.x), Number(frame.origin.y) - minimumY + 20))
}
content.autoresizesSubviews = true

const chromeHeight = Number(window.frame.size.height) - Number(window.contentView.frame.size.height)
const fixedHeight = 70 + topPadding
const maximumBodyHeight = Math.max(1, Math.floor(Number(screenFrame.size.height) - chromeHeight - fixedHeight))
scrollHeight = Math.min(bodyHeight, maximumBodyHeight)
needsScrolling = bodyHeight > maximumBodyHeight
window.setContentSize($.NSMakeSize(width, scrollHeight + fixedHeight))
window.center

if (needsScrolling) {
  scrollView = $.NSScrollView.alloc.initWithFrame($.NSMakeRect(0, 70, width, scrollHeight))
  scrollView.hasVerticalScroller = true
  scrollView.hasHorizontalScroller = false
  scrollView.autohidesScrollers = false
  //Keep the overflow affordance visible even when macOS normally fades overlay scrollbars.
  scrollView.scrollerStyle = $.NSScrollerStyleLegacy
  window.contentView.addSubview(scrollView)
  scrollView.tile
  content.setFrameSize($.NSMakeSize(Number(scrollView.contentSize.width), bodyHeight))
  scrollView.documentView = content
  content.scrollPoint($.NSMakePoint(0, Math.max(0, bodyHeight - Number(scrollView.contentSize.height))))
} else {
  content.setFrameOrigin($.NSMakePoint(0, 70))
  window.contentView.addSubview(content)
}
}
layoutAdvancedOptions()

const okButton = $.NSButton.buttonWithTitleTargetAction('OK', controller, 'okClicked:')
okButton.bezelStyle = $.NSBezelStyleRounded
okButton.keyEquivalent = '\r'
okButton.sizeToFit
const okWidth = Math.max(96, okButton.frame.size.width)
okButton.frame = $.NSMakeRect(width - 20 - okWidth, 20, okWidth, 32)
okButton.autoresizingMask = $.NSViewMinXMargin | $.NSViewMaxYMargin
window.contentView.addSubview(okButton)

const cancelButton = $.NSButton.buttonWithTitleTargetAction('Back', controller, 'cancelClicked:')
cancelButton.bezelStyle = $.NSBezelStyleRounded
cancelButton.keyEquivalent = '\u001b'
cancelButton.sizeToFit
const cancelWidth = Math.max(96, cancelButton.frame.size.width)
cancelButton.frame = $.NSMakeRect(width - 20 - okWidth - 8 - cancelWidth, 20, cancelWidth, 32)
cancelButton.autoresizingMask = $.NSViewMinXMargin | $.NSViewMaxYMargin
window.contentView.addSubview(cancelButton)

window.makeKeyAndOrderFront(null)
if (!app.isActive) app.requestUserAttention($.NSInformationalRequest)
app.activateIgnoringOtherApps(true)
worInstallWindowHandlers(controller)
app.runModalForWindow(window)

function writeResult(lines) {
  const data = $(lines.join('\n') + '\n').dataUsingEncoding($.NSUTF8StringEncoding)
  $.NSFileHandle.fileHandleWithStandardOutput.writeData(data)
}

if (!confirmed) {
  writeResult(['CANCEL'])
  app.terminate(null)
}

const out = ['OK']
for (let i = 0; i < checkboxes.length; i++) {
  out.push(checkboxes[i] ? (checkboxes[i].state == 1 ? '1' : '0') : (rows[i].checked ? '1' : '0'))
}
out.push(accountCheckbox.state == 1 ? '1' : '0')
out.push(localeCheckbox.state == 1 ? '1' : '0')
out.push(applyConfigCheckbox.state == 1 ? '1' : '0')
out.push(String(cachePopup.indexOfSelectedItem))
out.push(ObjC.unwrap(accountUsernameField.stringValue))
out.push(ObjC.unwrap(accountPasswordField.stringValue))
let selectedLocale = localeDefault
if (localePopup && localeOptions.length > 0) {
  const localeIdx = localePopup.indexOfSelectedItem
  if (localeIdx >= 0 && localeIdx < localeOptions.length) {
    selectedLocale = localeOptions[localeIdx].locale
  }
}
out.push(selectedLocale)
let selectedLang = currentLangCode
if (winLangPopup && langOptions.length > 0) {
  const selIdx = winLangPopup.indexOfSelectedItem
  if (selIdx >= 0 && selIdx < langOptions.length) {
    selectedLang = langOptions[selIdx].code
  }
}
out.push(selectedLang)
//appended after the existing fields so the positional reader above them keeps working
out.push(playSoundCheckbox && playSoundCheckbox.state == 1 ? '1' : (soundOptions.length > 0 ? '0' : playSoundDefault))
let selectedSound = soundDefault
if (soundPopup && soundOptions.length > 0) {
  const soundIdx = soundPopup.indexOfSelectedItem
  if (soundIdx >= 0 && soundIdx < soundOptions.length) {
    selectedSound = soundOptions[soundIdx].value
  }
}
out.push(selectedSound)
out.push(notificationCheckbox.state == 1 ? '1' : '0')
function selectedVersion(index, fallback) {
  const item = versionRows.find(function(row) { return row.checkboxIndex === index })
  if (!item || item.popup.indexOfSelectedItem < 0) return fallback
  return item.versions[Number(item.popup.indexOfSelectedItem)]
}
out.push(selectedVersion(2, currentUefiVersion))
out.push(selectedVersion(3, currentDriverVersion))
out.push('---CONFIG_TXT---')
out.push(configTxtValue)
writeResult(out)
app.terminate(null)
JXA
)"

  result="$(wor_osascript -l JavaScript - "$checkbox_spec" "$CONFIG_TXT" "$APPLY_CUSTOM_CONFIG_TXT" "$WOR_ICON_PATH" "$WOR_APP_TITLE" "$(wor_config_txt_label "$config_scope")" "$USE_CACHE" "$WINDOWS_ACCOUNT_USERNAME" "$WINDOWS_ACCOUNT_PASSWORD" "$WINDOWS_LOCALE" "$lang_spec" "$WIN_LANG" "$WINDOWS_ACCOUNT_SETUP" "$WINDOWS_LOCALE_SETUP" "$locale_spec" "$WOR_WINDOW_TITLE" "$sound_spec" "$PLAY_SOUND" "$(wor_completion_sound)" "$SHOW_NOTIFICATION" "$(wor_advanced_context "$windows_family" "$CAN_INSTALL_ON_SAME_DRIVE")" "$(wor_setup_scope "$windows_family" "$CAN_INSTALL_ON_SAME_DRIVE")" "$uefi_versions" "$uefi_dropdown_default" "$driver_versions" "$driver_dropdown_default" "$uefi_version_warning" "$driver_version_warning" "$uefi_version_labels" "$driver_version_labels" <<<"$advanced_jxa")" \
    || { warning "Could not display Advanced Options. Keeping the current settings."; return 1; }
  #config.txt and account fields are user-editable bytes; BSD sed rejects malformed UTF-8 under the
  #desktop locale, so parse the machine-readable result in byte mode after AppKit has finished.
  LC_ALL=C
  status="$(printf '%s\n' "$result" | sed -n '1p')"
  [ "$status" == OK ] || return 1
  selected_uefi="$(printf '%s\n' "$result" | sed -n '19p')"
  selected_driver="$(printf '%s\n' "$result" | sed -n '20p')"
  release_version_is_valid "$selected_uefi" && grep -qxF "$selected_uefi" <<<"$uefi_versions" \
    || { warning "The selected UEFI release is not in the available choices. Keeping the current settings."; return 1; }
  if [ "$drivers_applicable" == 1 ];then
    release_version_is_valid "$selected_driver" && grep -qxF "$selected_driver" <<<"$driver_versions" \
      || { warning "The selected driver release is not in the available choices. Keeping the current settings."; return 1; }
  fi

  i=0
  while IFS= read -r line;do
    i=$((i+1))
    case "$i" in
      1) [ "$oobe_applicable" == 1 ] && OOBE_NETWORK_BYPASS="$line" ;;
      2) [ "$pi4_applicable" == 1 ] && PI4_AUTO_DISABLE_3GB="$line" ;;
      3) set_uefi_use_latest_choice "$line" ;;
      4) [ "$drivers_applicable" == 1 ] && DRIVERS_USE_LATEST="$line" ;;
      5) SKIP_IMAGE_VERIFICATION="$line" ;;
      6) DRY_RUN="$line" ;;
      7) WINDOWS_ACCOUNT_SETUP="$line" ;;
      8) WINDOWS_LOCALE_SETUP="$line" ;;
      9) APPLY_CUSTOM_CONFIG_TXT="$line" ;;
    esac
  done < <(printf '%s\n' "$result" | sed -n '2,10p')
  case "$(printf '%s\n' "$result" | sed -n '11p')" in
    0 | 1 | 2) USE_CACHE="$(printf '%s\n' "$result" | sed -n '11p')" ;;
  esac
  WINDOWS_ACCOUNT_USERNAME="$(printf '%s\n' "$result" | sed -n '12p')"
  WINDOWS_ACCOUNT_PASSWORD="$(printf '%s\n' "$result" | sed -n '13p')"
  WINDOWS_LOCALE="$(printf '%s\n' "$result" | sed -n '14p')"
  sel_win_lang="$(printf '%s\n' "$result" | sed -n '15p')"
  if advanced_option_applies language && is_known_win_lang "$sel_win_lang" ;then
    WIN_LANG="$sel_win_lang"
  fi
  case "$(printf '%s\n' "$result" | sed -n '16p')" in
    0 | 1) PLAY_SOUND="$(printf '%s\n' "$result" | sed -n '16p')" ;;
  esac
  sel_sound="$(printf '%s\n' "$result" | sed -n '17p')"
  [ -n "$sel_sound" ] && COMPLETION_SOUND="$sel_sound"
  case "$(printf '%s\n' "$result" | sed -n '18p')" in
    0 | 1) SHOW_NOTIFICATION="$(printf '%s\n' "$result" | sed -n '18p')" ;;
  esac
  if [ "$(uefi_use_latest)" != 1 ] || [ "$selected_uefi" != "$uefi_dropdown_default" ];then
    set_selected_release_version uefi "$selected_uefi" || return 1
  fi
  if [ "$drivers_applicable" == 1 ] && { [ "$DRIVERS_USE_LATEST" != 1 ] || [ "$selected_driver" != "$driver_dropdown_default" ]; };then
    set_selected_release_version drivers "$selected_driver" || return 1
  fi

  CONFIG_TXT="$(printf '%s\n' "$result" | sed -n '/^---CONFIG_TXT---$/,$p' | tail -n +2)"
}

macos_choose_target() { #Input: current-run Windows/Pi defaults. Output: selected Windows version and Raspberry Pi model separated by a tab.
  local default_pi_label default_windows_label target_jxa result
  default_windows_label="${1:-Windows 11}"
  case "${2:-}" in
    5 | 'Raspberry Pi 5') default_pi_label='Raspberry Pi 5' ;;
    4 | 'Raspberry Pi 4 / Pi 400') default_pi_label='Raspberry Pi 4 / Pi 400' ;;
    3 | 'Raspberry Pi 3') default_pi_label='Raspberry Pi 3' ;;
    'Raspberry Pi 2 v1.2') default_pi_label='Raspberry Pi 2 v1.2' ;;
    'Raspberry Pi 2 v1.1' | 'Raspberry Pi 3 Model B') default_pi_label="$2" ;;
    *) default_pi_label='Raspberry Pi 5' ;;
  esac
  target_jxa="$(wor_jxa_window_lib; cat <<'JXA'
ObjC.import('AppKit')
ObjC.import('Foundation')
ObjC.import('stdlib')

const args = $.NSProcessInfo.processInfo.arguments
const iconPath = ObjC.unwrap(args.objectAtIndex(4))
const windowTitle = ObjC.unwrap(args.objectAtIndex(5))
const appTitle = ObjC.unwrap(args.objectAtIndex(6))
const defaultWindows = ObjC.unwrap(args.objectAtIndex(7) || 'Windows 11')
const defaultPiModel = ObjC.unwrap(args.objectAtIndex(8) || 'Raspberry Pi 5')
const app = $.NSApplication.sharedApplication
const windows = ['Windows 11', 'Windows 10', 'Windows 10 IoT Core (ARM32, legacy)']
const piModels = ObjC.unwrap(args.objectAtIndex(9)).split('\n').filter(Boolean)
const iotModels = ObjC.unwrap(args.objectAtIndex(10)).split('\n').filter(Boolean)
const defaultWindowsIdx = Math.max(0, windows.indexOf(defaultWindows))
let activeModels = defaultWindowsIdx === 2 ? iotModels : piModels
const defaultPiIdx = Math.max(0, activeModels.indexOf(defaultPiModel))
let window
let selectedValue = windows[defaultWindowsIdx] + '\t' + activeModels[defaultPiIdx]
let allowTermination = false

function writeResult(value) {
  const data = $(value + '\n').dataUsingEncoding($.NSUTF8StringEncoding)
  $.NSFileHandle.fileHandleWithStandardOutput.writeData(data)
}

function cancelAndExit() {
  writeResult('__WOR_CANCEL__')
  $.exit(0)
}

const Controller = ObjC.registerSubclass({
  name: 'WorTargetController',
  superclass: 'NSObject',
  methods: {
    'nextClicked:': {
      types: ['void', ['id']],
      implementation: function() {
        const windowsIdx = windowsPopup.indexOfSelectedItem
        const piIdx = piPopup.indexOfSelectedItem
        selectedValue = windows[windowsIdx] + '\t' + activeModels[piIdx]
        app.stopModalWithCode($.NSOKButton)
        window.orderOut(null)
      }
    },
    'familyChanged:': {
      types: ['void', ['id']],
      implementation: function() {
        const previous = ObjC.unwrap(piPopup.titleOfSelectedItem)
        activeModels = Number(windowsPopup.indexOfSelectedItem) === 2 ? iotModels : piModels
        piPopup.removeAllItems
        for (let i = 0; i < activeModels.length; i++) piPopup.addItemWithTitle($(activeModels[i]))
        piPopup.selectItemAtIndex(Math.max(0, activeModels.indexOf(previous)))
      }
    },
    'cancelClicked:': {
      types: ['void', ['id']],
      implementation: function() {
        cancelAndExit()
      }
    },
    'windowWillClose:': {
      types: ['void', ['id']],
      implementation: function() {
        cancelAndExit()
      }
    },
    'handleQuitEvent:withReplyEvent:': {
      types: ['void', ['id', 'id']],
      implementation: function() {
        cancelAndExit()
      }
    },
    'pumpEvents:': {
      types: ['void', ['id']],
      implementation: function() {
        $.NSRunLoop.currentRunLoop.runModeBeforeDate($.NSDefaultRunLoopMode, $.NSDate.dateWithTimeIntervalSinceNow(0.01))
      }
    },
    'handleReopenEvent:withReplyEvent:': {
      types: ['void', ['id', 'id']],
      implementation: function() {
        if (window.isMiniaturized) window.deminiaturize(null)
        window.makeKeyAndOrderFront(null)
        app.activateIgnoringOtherApps(true)
      }
    },
    'applicationShouldTerminate:': {
      types: ['NSUInteger', ['id']],
      implementation: function() {
        if (allowTermination) return $.NSTerminateNow
        cancelAndExit()
      }
    }
  }
})

const controller = $.WorTargetController.alloc.init
app.setActivationPolicy($.NSApplicationActivationPolicyRegular)
worInstallAppMenu(app, appTitle, windowTitle, iconPath)
worSetAppIcon(app, iconPath)
app.setDelegate(controller)
const screenFrame = $.NSScreen.mainScreen.visibleFrame
const width = Math.min(640, screenFrame.size.width - 40)
const height = Math.min(260, screenFrame.size.height - 60)
window = worMakeWindow({ width: width, height: height, title: windowTitle, delegate: controller })
const content = window.contentView
content.autoresizingMask = $.NSViewWidthSizable | $.NSViewHeightSizable

const heading = $.NSTextField.labelWithString('Choose Windows and Raspberry Pi target')
heading.font = $.NSFont.systemFontOfSizeWeight(18, $.NSFontWeightSemibold)
heading.frame = $.NSMakeRect(24, height - 58, width - 48, 28)
content.addSubview(heading)

const windowsLabel = $.NSTextField.labelWithString('Windows version:')
windowsLabel.frame = $.NSMakeRect(24, height - 112, 170, 24)
content.addSubview(windowsLabel)
const windowsPopup = $.NSPopUpButton.alloc.initWithFrame($.NSMakeRect(190, height - 116, width - 214, 30))
for (let i = 0; i < windows.length; i++) windowsPopup.addItemWithTitle($(windows[i]))
windowsPopup.selectItemAtIndex(defaultWindowsIdx)
windowsPopup.target = controller
windowsPopup.action = 'familyChanged:'
content.addSubview(windowsPopup)

const piLabel = $.NSTextField.labelWithString('Raspberry Pi model:')
piLabel.frame = $.NSMakeRect(24, height - 158, 170, 24)
content.addSubview(piLabel)
const piPopup = $.NSPopUpButton.alloc.initWithFrame($.NSMakeRect(190, height - 162, width - 214, 30))
for (let i = 0; i < activeModels.length; i++) piPopup.addItemWithTitle($(activeModels[i]))
piPopup.selectItemAtIndex(defaultPiIdx)
content.addSubview(piPopup)

const cancelButton = $.NSButton.buttonWithTitleTargetAction('Cancel', controller, 'cancelClicked:')
cancelButton.bezelStyle = $.NSBezelStyleRounded
cancelButton.frame = $.NSMakeRect(width - 220, 20, 92, 32)
content.addSubview(cancelButton)
const nextButton = $.NSButton.buttonWithTitleTargetAction('Next', controller, 'nextClicked:')
nextButton.bezelStyle = $.NSBezelStyleRounded
nextButton.frame = $.NSMakeRect(width - 116, 20, 92, 32)
nextButton.keyEquivalent = '\\r'
content.addSubview(nextButton)

worInstallWindowHandlers(controller)
window.makeKeyAndOrderFront(null)
app.activateIgnoringOtherApps(true)
app.runModalForWindow(window)
if (selectedValue === null || selectedValue.length === 0) cancelAndExit()
writeResult(selectedValue)
allowTermination = true
$.exit(0)
JXA
)"
  result="$(wor_osascript -l JavaScript - "$WOR_ICON_PATH" "$WOR_WINDOW_TITLE" "$WOR_APP_TITLE" "$default_windows_label" "$default_pi_label" "$(wor_rpi_board_options)" "$(wor_rpi_board_options iot-core)" <<<"$target_jxa")"
  result="$(printf '%s\n' "$result" | awk 'index($0, "\t") { print; exit }')"
  result="${result%%__WOR_CANCEL__*}"
  result="$(printf '%s' "$result" | tr -d '\r')"
  [ "$result" != __WOR_CANCEL__ ] || return 1
  [ -n "$result" ] || return 1
  printf '%s\n' "$result"
}

gui_iot_validate_target() { #Keep validation in this shell so successful device binding is preserved.
  local diagnostic_file diagnostic result=0 choice response=0
  diagnostic_file="$(mktemp)" || { warning "Could not create the IoT target-validation diagnostics."; return 1; }
  iot_core_validate_device preview > "$diagnostic_file" 2>&1 || result=$?
  cat "$diagnostic_file" >&2
  if [ "$result" == 0 ];then
    rm -f "$diagnostic_file"
    return 0
  fi
  diagnostic="$(gui_log_tail "$diagnostic_file")"
  rm -f "$diagnostic_file"
  [ -n "$diagnostic" ] || diagnostic="Target validation failed without further diagnostics. Check the app startup log; no drive was modified."
  diagnostic="Cannot use $DEVICE for Windows 10 IoT Core.

$diagnostic

No image was downloaded and no drive was modified. Choose another suitable drive or cancel.

A full wipe is available in Advanced Options only after the drive passes safety and identity checks."
  if is_macos;then
    choice="$(macos_choose '' "$diagnostic" __RETRY__ Cancel '' '' '' 'Choose another drive' "$WOR_ICON_PATH" "$WOR_WINDOW_TITLE" '' 0 "$WOR_APP_TITLE")" || return 1
    [ "$choice" == __RETRY__ ] && return 2
  else
    yad "${yadflags[@]}" --no-markup --width="$(wor_yad_width 700)" \
      --text="$diagnostic" --button=Cancel:1 --button='Choose another drive':0 >/dev/null || response=$?
    [ "$response" != 0 ] || return 2
  fi
  return 1
}

gui_iot_confirm_wipe() { #Separate consent for a full wipe; never offers to erase an unidentified target.
  WOR_IOT_CONFIRM_WIPE=0
  [ "${IOT_CORE_WIPE_DRIVE:-0}" == 1 ] && [ "$DRY_RUN" != 1 ] || return 0
  iot_core_validate_device preview \
    || { warning "Cannot confirm a full wipe until the selected drive passes safety and identity checks."; return 1; }
  local message choice response=0
  message="Permanently erase every addressable sector of this selected drive?

$(describe_device "$DEVICE")
Device: $DEVICE
Capacity: $IOT_CORE_TARGET_BYTES bytes

All existing partitions and data will be lost. After the image is verified and administrator access is granted, the entire drive is zeroed and read back, then Windows IoT Core is written and verified.

This may take a long time. It is not a secure erase or a repair for failing hardware."
  if is_macos;then
    choice="$(macos_choose '' "$message" __WIPE__ Cancel '' '' '' 'Wipe and Flash' "$WOR_ICON_PATH" "$WOR_WINDOW_TITLE | Full drive reset" '' 0 "$WOR_APP_TITLE")" || return 1
    [ "$choice" == __WIPE__ ] || return 1
  else
    yad "${yadflags[@]}" --no-markup --warning --width="$(wor_yad_width 700)" \
      --title="$WOR_WINDOW_TITLE | Full drive reset" --text="$message" \
      --button=Cancel:1 --button='Wipe and Flash':0 >/dev/null || response=$?
    [ "$response" == 0 ] || return 1
  fi
  WOR_IOT_CONFIRM_WIPE=1
}

linux_iot_validate_selection() {
  local validation_status
  while true;do
    validation_status=0
    gui_iot_validate_target || validation_status=$?
    case "$validation_status" in
      0) return 0 ;;
      2)
        DEVICE=''
        linux_choose_flash_device
        iot_core_clear_target_approval
        ;;
      *) return 1 ;;
    esac
  done
}

gui_iot_plan_source() { #Queue the source choice; only the post-Flash installer prepares image bytes.
  iot_core_load_profile || return 1
  if [ -z "$SOURCE_FILE" ];then
    WOR_IOT_DOWNLOAD=1
    GUI_IOT_SOURCE_MODE=official
    GUI_IOT_LOCAL_SOURCE=''
  elif [ "${WOR_IOT_DOWNLOAD:-0}" == 0 ] && [ -z "${GUI_IOT_SOURCE_MODE:-}" ];then
    GUI_IOT_SOURCE_MODE=local
    GUI_IOT_LOCAL_SOURCE="$SOURCE_FILE"
  fi
  return 0
}

gui_iot_connection_state() { #Saved current login, with factory defaults only for unset values; no network access.
  local defaults_json automatic
  defaults_json="$(python3 "$DIRECTORY/src/lib/iot-account.py" login-defaults)" \
    || { warning "Cannot read the IoT factory login defaults."; return 1; }
  automatic="$(iot_core_automatic_address)" || return 1
  WOR_IOT_HOST="${IOT_CORE_HOST:-}" \
    WOR_IOT_AUTOMATIC_ADDRESS="$automatic" WOR_IOT_AUTOMATIC_HOSTNAME="$(jq -r .automaticHostname <<<"$defaults_json")" \
    WOR_IOT_CURRENT_USERNAME="${IOT_CORE_CURRENT_USERNAME-$(jq -r .currentUsername <<<"$defaults_json")}" \
    WOR_IOT_CURRENT_PASSWORD="${IOT_CORE_CURRENT_PASSWORD-$(jq -r .currentPassword <<<"$defaults_json")}" \
    jq -n '{host: env.WOR_IOT_HOST, automaticAddress: (env.WOR_IOT_AUTOMATIC_ADDRESS == "1"),
      automaticHostname: env.WOR_IOT_AUTOMATIC_HOSTNAME,
      currentUsername: env.WOR_IOT_CURRENT_USERNAME, currentPassword: env.WOR_IOT_CURRENT_PASSWORD}'
}

gui_iot_options_state() { #Output: current IoT-only options, without changing a prepared source.
  local mode="${GUI_IOT_SOURCE_MODE:-official}" source='' hdmi connection
  hdmi="$(iot_core_hdmi_state)" || return 1
  connection="$(gui_iot_connection_state)" || return 1
  if [ -z "${GUI_IOT_SOURCE_MODE:-}" ] && [ -n "$SOURCE_FILE" ] && [ "${WOR_IOT_DOWNLOAD:-0}" == 0 ];then mode=local;fi
  [ "$mode" != local ] || source="${GUI_IOT_LOCAL_SOURCE:-$SOURCE_FILE}"
  WOR_IOT_ACCOUNT_PASSWORD="${IOT_CORE_ACCOUNT_PASSWORD:-}" jq --arg mode "$mode" --arg source "$source" --arg downloadDir "$DL_DIR" \
    --arg dryRun "$DRY_RUN" --arg playSound "$PLAY_SOUND" --arg completionSound "$(wor_completion_sound)" \
    --arg showNotification "$SHOW_NOTIFICATION" \
    --arg hdmiMode "$(iot_core_hdmi_mode)" --arg hdmiConfig "${IOT_CORE_HDMI_CONFIG:-}" --argjson hdmi "$hdmi" \
    --argjson sounds "$(wor_sound_options | jq -Rn '[inputs | split("\t") | {value: .[0], label: .[1]}]')" \
    --arg official "$(wor_iot_option_label official)" --arg local "$(wor_iot_option_label local)" \
    --arg recommended "$(wor_iot_option_label recommended)" \
    --arg sourceLabel "$(wor_iot_option_label source)" --arg fileLabel "$(wor_iot_option_label file)" \
    --arg cacheLabel "$(wor_iot_option_label cache)" --arg dryRunLabel "$(wor_iot_option_label dryrun)" \
    --arg playSoundLabel "$(wor_iot_option_label playSound)" --arg soundLabel "$(wor_iot_option_label sound)" \
    --arg notificationLabel "$(wor_iot_option_label notification)" --arg verification "$(wor_iot_option_label verification)" \
    --arg hdmiLabel "$(wor_iot_option_label hdmi)" --arg hdmiEdit "$(wor_iot_option_label editHdmi)" --arg hdmiHelp "$(wor_iot_option_label hdmiHelp)" \
    --arg hdmiEditorHelp "$(wor_iot_option_label hdmiEditorHelp)" \
    --arg accountSetup "${IOT_CORE_ACCOUNT_SETUP:-0}" --arg accountUsername "${IOT_CORE_ACCOUNT_USERNAME:-Administrator}" \
    --arg accountLabel "$(wor_iot_option_label accountSetup)" --arg usernameLabel "$(wor_iot_option_label accountUsername)" \
    --arg passwordLabel "$(wor_iot_option_label accountPassword)" --arg accountHelp "$(wor_iot_option_label accountHelp)" \
    --arg languageSetup "${IOT_CORE_LANGUAGE_SETUP:-1}" --arg language "$(iot_core_language)" \
    --argjson languages "$(iot_core_language_options | jq -Rn '[inputs | split("\t") | {value: .[0], label: .[1]}]')" \
    --arg languageSetupLabel "$(wor_iot_option_label languageSetup)" --arg languageLabel "$(wor_iot_option_label language)" \
    --arg languageHelp "$(wor_iot_option_label languageHelp)" \
    --arg wipeDrive "${IOT_CORE_WIPE_DRIVE:-0}" --arg wipeLabel "$(wor_iot_option_label wipeDrive)" \
    --arg wipeHelp "$(wor_iot_option_label wipeHelp)" \
    --arg connectionTitle "$(wor_iot_option_label connectionTitle)" --arg connectionHost "$(wor_iot_option_label connectionHost)" \
    --arg connectionUsername "$(wor_iot_option_label connectionUsername)" --arg connectionPassword "$(wor_iot_option_label connectionPassword)" \
    --arg connectionHelp "$(wor_iot_option_label connectionHelp)" \
    --arg automaticAddressLabel "$(wor_iot_option_label automaticAddress)" --arg automaticAddressHelp "$(wor_iot_option_label automaticAddressHelp)" \
    '{mode: $mode, source: $source, downloadDir: $downloadDir, dryRun: ($dryRun == "1"),
      playSound: ($playSound == "1"), completionSound: $completionSound, showNotification: ($showNotification == "1"),
      hdmiMode: $hdmiMode, hdmiConfig: $hdmiConfig, hdmi: $hdmi,
      accountSetup: ($accountSetup == "1"), accountUsername: $accountUsername,
      accountPassword: env.WOR_IOT_ACCOUNT_PASSWORD,
      languageSetup: ($languageSetup == "1"), language: $language, languages: $languages,
      wipeDrive: ($wipeDrive == "1"),
      host: .host, automaticAddress: .automaticAddress, automaticHostname: .automaticHostname,
      currentUsername: .currentUsername, currentPassword: .currentPassword,
      sounds: $sounds, labels: {official: $official, recommended: $recommended, local: $local, source: $sourceLabel, file: $fileLabel,
        cache: $cacheLabel, dryRun: $dryRunLabel, playSound: $playSoundLabel, sound: $soundLabel,
        notification: $notificationLabel, verification: $verification, hdmi: $hdmiLabel,
        editHdmi: $hdmiEdit, hdmiHelp: $hdmiHelp, hdmiEditorHelp: $hdmiEditorHelp,
        accountSetup: $accountLabel, accountUsername: $usernameLabel, accountPassword: $passwordLabel,
        accountHelp: $accountHelp, languageSetup: $languageSetupLabel, language: $languageLabel,
        languageHelp: $languageHelp, wipeDrive: $wipeLabel, wipeHelp: $wipeHelp,
        connectionTitle: $connectionTitle, connectionHost: $connectionHost, connectionUsername: $connectionUsername,
        connectionPassword: $connectionPassword, connectionHelp: $connectionHelp,
        automaticAddress: $automaticAddressLabel, automaticAddressHelp: $automaticAddressHelp}}' <<<"$connection"
}

gui_iot_apply_options() { #Input: saved form JSON. Queue source changes without importing or downloading.
  local options="$1" current mode source download_dir sound hdmi_mode hdmi_config changed=0
  if ! jq -e '
    type == "object" and (.mode == "official" or .mode == "local")
    and (.source | type == "string" and (test("[\r\n]") | not))
    and (.downloadDir | type == "string" and startswith("/") and (test("[\r\n]") | not))
    and (.dryRun | type == "boolean") and (.playSound | type == "boolean")
    and (.completionSound | type == "string") and (.showNotification | type == "boolean")
    and (.hdmiMode | type == "string") and (.hdmiConfig | type == "string")
    and (.accountSetup | type == "boolean") and (.accountUsername | type == "string")
    and (.accountPassword | type == "string")
    and (.languageSetup | type == "boolean") and (.language | type == "string")
    and (.wipeDrive | type == "boolean")
    and (.host | type == "string") and (.currentUsername | type == "string") and (.currentPassword | type == "string")
    and (.automaticAddress | type == "boolean")
    and (.mode != "local" or (.source | startswith("/") and length > 1))
  ' >/dev/null <<<"$options";then
    warning "Invalid IoT Advanced Options. No preferences were changed."
    return 1
  fi
  mode="$(jq -r .mode <<<"$options")"
  source="$(jq -r .source <<<"$options")"
  download_dir="$(jq -r .downloadDir <<<"$options")"
  sound="$(jq -r .completionSound <<<"$options")"
  hdmi_mode="$(jq -r .hdmiMode <<<"$options")"
  hdmi_config="$(jq -r .hdmiConfig <<<"$options")"
  printf '%s' "$options" | python3 "$DIRECTORY/src/lib/iot-account.py" preferences >/dev/null || return 1
  iot_core_language_options | cut -f1 | grep -qiFx "$(jq -r .language <<<"$options")" \
    || { warning "Select a language from the shared Windows language catalog. No preferences were changed."; return 1; }
  iot_core_hdmi_state "$hdmi_mode" "$hdmi_config" >/dev/null || return 1
  if ! wor_sound_options | awk -F'\t' -v value="$sound" '$1 == value {found=1} END {exit !found}';then
    warning "The selected IoT completion sound is not available on this host. No preferences were changed."
    return 1
  fi
  if [ "$mode" == local ];then
    case "$source" in
      *.[fF][fF][uU] | *.[iI][sS][oO] | *.[mM][sS][iI]) ;;
      *) warning "Choose the official IoT FFU, ISO or MSI in Advanced Options. No preferences were changed."; return 1 ;;
    esac
  fi
  current="$(gui_iot_options_state)" || return 1
  if [ "$mode" != "$(jq -r .mode <<<"$current")" ] || [ "$download_dir" != "$DL_DIR" ] \
    || { [ "$mode" == local ] && [ "$source" != "$(jq -r .source <<<"$current")" ]; } \
    || [ -z "${IOT_CORE_SHA256:-}" ];then changed=1;fi
  GUI_IOT_SOURCE_MODE="$mode"
  GUI_IOT_LOCAL_SOURCE=''
  [ "$mode" != local ] || GUI_IOT_LOCAL_SOURCE="$source"
  DL_DIR="$download_dir"
  DRY_RUN="$(jq -r 'if .dryRun then "1" else "0" end' <<<"$options")"
  PLAY_SOUND="$(jq -r 'if .playSound then "1" else "0" end' <<<"$options")"
  COMPLETION_SOUND="$sound"
  SHOW_NOTIFICATION="$(jq -r 'if .showNotification then "1" else "0" end' <<<"$options")"
  IOT_CORE_HDMI_MODE="$hdmi_mode"
  IOT_CORE_HDMI_CONFIG="$hdmi_config"
  IOT_CORE_ACCOUNT_SETUP="$(jq -r 'if .accountSetup then "1" else "0" end' <<<"$options")"
  IOT_CORE_ACCOUNT_USERNAME="$(jq -r .accountUsername <<<"$options")"
  IOT_CORE_ACCOUNT_PASSWORD=''
  [ "$IOT_CORE_ACCOUNT_SETUP" != 1 ] || IOT_CORE_ACCOUNT_PASSWORD="$(jq -r .accountPassword <<<"$options")"
  IOT_CORE_LANGUAGE_SETUP="$(jq -r 'if .languageSetup then "1" else "0" end' <<<"$options")"
  IOT_CORE_LANGUAGE="$(windows_locale_from_language_code "$(jq -r .language <<<"$options")")"
  IOT_CORE_WIPE_DRIVE="$(jq -r 'if .wipeDrive then "1" else "0" end' <<<"$options")"
  IOT_CORE_HOST="$(jq -r .host <<<"$options")"
  IOT_CORE_AUTOMATIC_ADDRESS="$(jq -r 'if .automaticAddress then "1" else "0" end' <<<"$options")"
  IOT_CORE_CURRENT_USERNAME="$(jq -r .currentUsername <<<"$options")"
  IOT_CORE_CURRENT_PASSWORD="$(jq -r .currentPassword <<<"$options")"
  export -n IOT_CORE_CURRENT_PASSWORD
  WOR_IOT_CONFIRM_ERASE=0
  WOR_IOT_CONFIRM_WIPE=0
  if [ "$changed" == 1 ];then
    IOT_CORE_BUILD='' IOT_CORE_MINIMUM_BYTES='' IOT_CORE_SHA256='' IOT_CORE_ACQUISITION='' IOT_CORE_INSPECTION_JSON=''
    SOURCE_FILE="$GUI_IOT_LOCAL_SOURCE" WOR_IOT_DOWNLOAD=0
    [ "$mode" != official ] || WOR_IOT_DOWNLOAD=1
    BID=''
  fi
  return 0
}

gui_iot_private_json() { #Input: JSON and caller variable name. Secrets stay out of native process arguments.
  local private_file
  private_file="$(umask 077; mktemp)" || { warning "Cannot create private IoT dialog state."; return 1; }
  chmod 600 "$private_file" || { rm -f "$private_file"; warning "Cannot protect private IoT dialog state."; return 1; }
  register_file_cleanup "$private_file"
  printf '%s' "$1" > "$private_file" || { rm -f "$private_file"; warning "Cannot write private IoT dialog state."; return 1; }
  printf -v "$2" '%s' "$private_file"
}

gui_iot_edit_again() { #Input: message, title, optional edit label. Return 1 to edit or 2 to cancel.
  local message="$1" title="$2" edit_label="${3:-Edit Again}" choice response=0
  warning "$message"
  if is_macos;then
    choice="$(macos_choose '' "$message" edit Back '' '' '' "$edit_label" "$WOR_ICON_PATH" "$title" '' 0 "$WOR_APP_TITLE")" || return 2
    [ "$choice" == edit ] && return 1
    return 2
  fi
  yad "${yadflags[@]}" --warning --width="$(wor_yad_width 620)" --height="$(wor_yad_height 240)" \
    --title="$title" --text="$message" --button=Back:1 --button="$edit_label":0 || response=$?
  [ "$response" != 0 ] || return 1
  return 2
}

gui_iot_account_setup() { #Input: optional result log. Use saved Advanced Options only after Connect to Pi.
  is_iot_core && iot_core_personalization_requested && [ "$DRY_RUN" != 1 ] || return 0
  local connection pairing request result diagnostic message choice response=0 result_log="${1:-}"
  IOT_CORE_ACCOUNT_STATUS=pending
  IOT_CORE_LANGUAGE_STATUS=pending
  diagnostic="$(umask 077; mktemp)" || { warning "Cannot create a private IoT account diagnostic."; return 1; }
  register_file_cleanup "$diagnostic"
  [ -n "$result_log" ] || result_log="$diagnostic"
  while true;do
    connection="$(gui_iot_connection_state)" || {
      IOT_CORE_ACCOUNT_PASSWORD='' IOT_CORE_CURRENT_PASSWORD=''
      rm -f "$diagnostic"
      return 1
    }
    if ! printf '%s' "$connection" | python3 "$DIRECTORY/src/lib/iot-account.py" preferences --metadata-only >/dev/null 2>"$diagnostic";then
      message="The saved IoT connection settings are invalid.

$(gui_log_tail "$diagnostic")

No connection was made. Correct them in Advanced Options."
    elif ! pairing="$(printf '%s' "$connection" | jq '{host: .host, automaticAddress: .automaticAddress}' \
      | python3 "$DIRECTORY/src/lib/iot-account.py" probe 2>"$diagnostic")";then
      message="IoT personalization remains Pending.

$(gui_log_tail "$diagnostic")

The verified media was not changed. No credentials were sent. Boot the Pi and check its address in Advanced Options, then save to retry."
    else
      break
    fi
    choice=0
    gui_iot_edit_again "$message" "$WOR_WINDOW_TITLE | Saved IoT settings" 'Advanced Options' || choice=$?
    if [ "$choice" == 1 ];then
      response=0 GUI_IOT_OPTIONS_SAVED=0
      if is_macos;then macos_iot_options || response=$?;else linux_iot_options || response=$?;fi
      if [ "$response" != 0 ];then
        IOT_CORE_ACCOUNT_PASSWORD='' IOT_CORE_CURRENT_PASSWORD=''
        rm -f "$diagnostic"
        warning "IoT personalization remains Pending; Advanced Options did not complete."
        return 1
      fi
      if [ "$GUI_IOT_OPTIONS_SAVED" == 1 ] && [ "$DRY_RUN" != 1 ] && iot_core_personalization_requested;then continue;fi
    fi
    IOT_CORE_ACCOUNT_PASSWORD='' IOT_CORE_CURRENT_PASSWORD=''
    rm -f "$diagnostic"
    warning "IoT personalization remains Pending; no device identity was confirmed."
    return 0
  done
  message="Confirm this SSH identity belongs to your Raspberry Pi before continuing.

Device: $(jq -r .address <<<"$pairing")
Fingerprint: $(jq -r .fingerprint <<<"$pairing")

Administrator change: $([ "${IOT_CORE_ACCOUNT_SETUP:-0}" == 1 ] && printf '%s' "${IOT_CORE_ACCOUNT_USERNAME:-Administrator}" || printf 'Not requested')
UI language: $([ "${IOT_CORE_LANGUAGE_SETUP:-1}" == 1 ] && iot_core_language || printf 'Not requested')

Only confirm for your own Pi on an isolated local network. The app checks installed language support under DefaultAccount before applying language preferences, and verifies a fresh login if administrator changes were requested."
    if is_macos;then
      choice="$(macos_choose '' "$message" apply Cancel '' '' '' 'Confirm and Apply' "$WOR_ICON_PATH" "$WOR_WINDOW_TITLE | IoT SSH identity" '' 0 "$WOR_APP_TITLE")" || choice=cancel
    else
      choice=apply
      yad "${yadflags[@]}" --question --width="$(wor_yad_width 690)" --height="$(wor_yad_height 440)" \
        --title="$WOR_WINDOW_TITLE | IoT SSH identity" --text="$message" --button=Cancel:1 --button='Confirm and Apply':0 || choice=cancel
    fi
    if [ "$choice" != apply ];then
      IOT_CORE_ACCOUNT_PASSWORD=''
      IOT_CORE_CURRENT_PASSWORD=''
      rm -f "$diagnostic"
      warning "IoT SSH identity was not confirmed. Personalization remains Pending."
      return 0
    fi
    request="$(WOR_IOT_ACCOUNT_PASSWORD="${IOT_CORE_ACCOUNT_PASSWORD:-}" jq --argjson pairing "$pairing" \
      --arg accountUsername "${IOT_CORE_ACCOUNT_USERNAME:-Administrator}" \
      --arg accountSetup "${IOT_CORE_ACCOUNT_SETUP:-0}" --arg languageSetup "${IOT_CORE_LANGUAGE_SETUP:-1}" \
      --arg language "$(iot_core_language)" \
      '. + {pairing: $pairing, confirmIdentity: true, accountSetup: ($accountSetup == "1"),
        accountUsername: $accountUsername, accountPassword: env.WOR_IOT_ACCOUNT_PASSWORD,
        languageSetup: ($languageSetup == "1"), language: $language}' <<<"$connection")" || return 1
    status "Applying and verifying IoT language/account preferences"
    if result="$(printf '%s' "$request" | python3 "$DIRECTORY/src/lib/iot-account.py" configure 2>"$diagnostic")";then
      if jq -e --arg username "${IOT_CORE_ACCOUNT_USERNAME:-Administrator}" \
        --arg accountSetup "${IOT_CORE_ACCOUNT_SETUP:-0}" --arg languageSetup "${IOT_CORE_LANGUAGE_SETUP:-1}" \
        --arg language "$(iot_core_language)" '
          (.state == "verified" or .state == "pending-reboot")
          and (if $accountSetup == "1" then .accountState == "verified" and .username == $username
            and (.sid | type == "string" and endswith("-500")) else .accountState == "not-assessed" end)
          and (if $languageSetup == "1" then .language == $language
            and (.languageState == "verified" or .languageState == "pending-reboot") else .languageState == "not-assessed" end)
        ' >/dev/null <<<"$result";then
        IOT_CORE_ACCOUNT_STATUS="$(jq -r .accountState <<<"$result")"
        IOT_CORE_LANGUAGE_STATUS="$(jq -r .languageState <<<"$result")"
        message="IoT personalization result:

Administrator: $IOT_CORE_ACCOUNT_STATUS$([ "$IOT_CORE_ACCOUNT_STATUS" == verified ] && printf ' (%s); fresh SSH login verified' "${IOT_CORE_ACCOUNT_USERNAME:-Administrator}")
UI language: $IOT_CORE_LANGUAGE_STATUS$([ "$IOT_CORE_LANGUAGE_STATUS" != not-assessed ] && printf ' (%s)' "$(iot_core_language)")

$([ "$IOT_CORE_LANGUAGE_STATUS" == pending-reboot ] && printf 'The language change was accepted but is not yet active. Restart the Pi and verify its displayed language; do not treat it as complete yet.')

Passwords are not displayed or saved in the log. The original FFU and service accounts were not changed."
      else
        message="The helper did not return verified personalization results. Language or credentials may have changed; verify the device before retrying."
        response=1
      fi
    else
      message="IoT personalization was not verified.

$(gui_log_tail "$diagnostic")

The media remains verified, but account credentials may have changed. Check the old/new login before retrying."
      response=1
    fi
  IOT_CORE_ACCOUNT_PASSWORD=''
  IOT_CORE_CURRENT_PASSWORD=''
  connection='' request=''
  [ "$response" == 0 ] || { IOT_CORE_ACCOUNT_STATUS=failed; IOT_CORE_LANGUAGE_STATUS=failed; }
  [ "$response" != 0 ] && warning "$message" || status "IoT personalization result: account=$IOT_CORE_ACCOUNT_STATUS; language=$IOT_CORE_LANGUAGE_STATUS."
  if is_macos;then
    macos_show_result_dialog "$message" '' '' "$result_log" || response=1
  else
    yad "${yadflags[@]}" --info --width="$(wor_yad_width 690)" --height="$(wor_yad_height 380)" \
      --title="$WOR_WINDOW_TITLE | IoT personalization" --text="$message" || response=1
  fi
  rm -f "$diagnostic"
  return "$response"
}

gui_iot_save_options() { #Apply in this shell; invalid edited settings stay pending until corrected or canceled.
  local diagnostic message
  diagnostic="$(mktemp)" || { warning "Cannot create the Advanced Options validation diagnostic."; return 2; }
  if gui_iot_apply_options "$1" 2>"$diagnostic";then
    rm -f "$diagnostic"
    return 0
  fi
  message="$(gui_log_tail "$diagnostic")"
  rm -f "$diagnostic"
  message="$message

No preferences were changed. Correct the settings or go back."
  gui_iot_edit_again "$message" "$WOR_WINDOW_TITLE | Advanced Options"
}

macos_iot_options() {
  local state options options_jxa saved state_file
  GUI_IOT_OPTIONS_SAVED=0
  state="$(gui_iot_options_state)" || return 1
  options_jxa="$(wor_jxa_window_lib; cat <<'JXA'
ObjC.import('AppKit')
ObjC.import('Foundation')
ObjC.import('stdlib')
const args = $.NSProcessInfo.processInfo.arguments
const statePath = ObjC.unwrap(args.objectAtIndex(4))
const stateText = $.NSString.stringWithContentsOfFileEncodingError($(statePath), $.NSUTF8StringEncoding, null)
if (stateText.isNil()) throw new Error('Cannot read private IoT options state')
const state = JSON.parse(ObjC.unwrap(stateText))
if (!$.NSFileManager.defaultManager.removeItemAtPathError($(statePath), null)) throw new Error('Cannot discard private IoT options state')
const iconPath = ObjC.unwrap(args.objectAtIndex(5))
const windowTitle = ObjC.unwrap(args.objectAtIndex(6))
const appTitle = ObjC.unwrap(args.objectAtIndex(7))
const app = $.NSApplication.sharedApplication
let window, sourcePopup, sourceField, browseSourceButton, directoryField, dryRunCheckbox, soundCheckbox, soundPopup, notificationCheckbox
let sourceLabel, customLabel, recommendedBadge
let hdmiPopup, hdmiLabel, editHdmiButton, hdmiRecommendedBadge
let accountCheckbox, accountUsernameField, accountPasswordField, accountUsernameLabel, accountPasswordLabel, accountHelp
let accountRows = []
let languageCheckbox, languagePopup, languageLabel, languageHelp
let wipeCheckbox
let connectionTitle, connectionHostLabel, connectionHostField, currentUsernameLabel, currentUsernameField, currentPasswordLabel, currentPasswordField, connectionHelp
let automaticAddressCheckbox
let hdmiConfig = state.hdmiConfig || state.hdmi.custom
let hdmiEdited = state.hdmiConfig.length > 0
let previousHdmi = state.hdmiMode
let confirmed = false
function closeOptions() {
  app.stopModalWithCode($.NSCancelButton)
  window.orderOut(null)
}
function updateSourceControls() {
  const local = Number(sourcePopup.indexOfSelectedItem) === 1
  sourceField.enabled = local
  browseSourceButton.enabled = local
  sourceField.hidden = !local
  browseSourceButton.hidden = !local
  customLabel.hidden = !local
  recommendedBadge.hidden = local
  layoutSourceRows(local)
}
function layoutSourceRows(local) {
  const accountHeight = Number(accountCheckbox.state) === 1 ? 220 : 128
  const languageHeight = 154
  const connectionHeight = 274
  const documentHeight = (local ? 536 : 490) + accountHeight + languageHeight + connectionHeight
  const height = Math.min(documentHeight + 130, screen.size.height - 60)
  const previousFrame = window.frame
  window.setContentSize($.NSMakeSize(width, height))
  const resizedFrame = window.frame
  window.setFrameOrigin($.NSMakePoint(previousFrame.origin.x, previousFrame.origin.y + previousFrame.size.height - resizedFrame.size.height))
  heading.frame = $.NSMakeRect(24, height - 48, width - 48, 28)
  const viewportHeight = height - 130
  scroll.frame = $.NSMakeRect(24, 72, width - 48, viewportHeight)
  scroll.hasVerticalScroller = documentHeight > viewportHeight
  innerWidth = scroll.contentSize.width
  content.frame = $.NSMakeRect(0, 0, innerWidth, documentHeight)
  for (const row of accountRows) row.view.setFrameOrigin($.NSMakePoint(row.view.frame.origin.x, row.y + accountHeight + languageHeight + connectionHeight))
  const sourceY = documentHeight - 34
  sourceLabel.frame = $.NSMakeRect(0, sourceY, 188, 26)
  const badgeWidth = 112
  sourcePopup.frame = $.NSMakeRect(198, sourceY - 4, innerWidth - 198 - (local ? 0 : badgeWidth + 8), 32)
  recommendedBadge.frame = $.NSMakeRect(innerWidth - badgeWidth, sourceY + 1, badgeWidth, 22)
  accountCheckbox.frame = $.NSMakeRect(0, accountHeight - 36 + languageHeight, innerWidth, 28)
  accountUsernameLabel.frame = $.NSMakeRect(0, 136 + languageHeight, 188, 26)
  accountUsernameField.frame = $.NSMakeRect(198, 136 + languageHeight, innerWidth - 198, 26)
  accountPasswordLabel.frame = $.NSMakeRect(0, 90 + languageHeight, 188, 26)
  accountPasswordField.frame = $.NSMakeRect(198, 90 + languageHeight, innerWidth - 198, 26)
  accountHelp.frame = $.NSMakeRect(0, 10 + languageHeight, innerWidth, 64)
  languageCheckbox.frame = $.NSMakeRect(0, 116, innerWidth, 28)
  languageLabel.frame = $.NSMakeRect(0, 78, 188, 26)
  languagePopup.frame = $.NSMakeRect(198, 74, innerWidth - 198, 32)
  languageHelp.frame = $.NSMakeRect(0, 8, innerWidth, 58)
  const connectionY = accountHeight + languageHeight
  connectionTitle.frame = $.NSMakeRect(0, connectionY + 246, innerWidth, 24)
  automaticAddressCheckbox.frame = $.NSMakeRect(0, connectionY + 200, innerWidth, 28)
  connectionHostLabel.frame = $.NSMakeRect(0, connectionY + 158, 188, 26)
  connectionHostField.frame = $.NSMakeRect(198, connectionY + 158, innerWidth - 198, 26)
  currentUsernameLabel.frame = $.NSMakeRect(0, connectionY + 114, 188, 26)
  currentUsernameField.frame = $.NSMakeRect(198, connectionY + 114, innerWidth - 198, 26)
  currentPasswordLabel.frame = $.NSMakeRect(0, connectionY + 70, 188, 26)
  currentPasswordField.frame = $.NSMakeRect(198, connectionY + 70, innerWidth - 198, 26)
  connectionHelp.frame = $.NSMakeRect(0, connectionY + 4, innerWidth, 58)
  updateHdmiControls()
  content.scrollPoint($.NSMakePoint(0, Math.max(0, documentHeight - viewportHeight)))
  scroll.reflectScrolledClipView(scroll.contentView)
}
function updateHdmiControls() {
  const selected = state.hdmi.choices[Number(hdmiPopup.indexOfSelectedItem)]
  const custom = selected.value === 'custom'
  const recommended = selected.recommended === true
  const editWidth = 140
  const badgeWidth = 112
  const accessoryWidth = custom ? editWidth + 8 : (recommended ? badgeWidth + 8 : 0)
  const popupX = Math.max(124, Math.min(198, innerWidth - accessoryWidth - Math.ceil(hdmiPopup.fittingSize.width)))
  const offset = (Number(accountCheckbox.state) === 1 ? 220 : 128) + 154 + 274
  hdmiLabel.frame = $.NSMakeRect(0, 358 + offset, popupX - 10, 26)
  hdmiPopup.frame = $.NSMakeRect(popupX, 354 + offset, innerWidth - popupX - accessoryWidth, 32)
  editHdmiButton.frame = $.NSMakeRect(innerWidth - editWidth, 354 + offset, editWidth, 32)
  editHdmiButton.hidden = !custom
  editHdmiButton.enabled = custom
  hdmiRecommendedBadge.frame = $.NSMakeRect(innerWidth - badgeWidth, 359 + offset, badgeWidth, 22)
  hdmiRecommendedBadge.hidden = !recommended
}
function updateAccountControls() {
  const enabled = Number(accountCheckbox.state) === 1
  accountUsernameField.hidden = !enabled
  accountPasswordField.hidden = !enabled
  accountUsernameLabel.hidden = !enabled
  accountPasswordLabel.hidden = !enabled
  layoutSourceRows(Number(sourcePopup.indexOfSelectedItem) === 1)
}
function updateLanguageControls() {
  languagePopup.enabled = Number(languageCheckbox.state) === 1
}
function updateConnectionAddress() {
  connectionHostField.enabled = Number(automaticAddressCheckbox.state) !== 1
}
function hdmiChanged() {
  const selected = state.hdmi.choices[Number(hdmiPopup.indexOfSelectedItem)].value
  if (selected === 'custom' && !hdmiEdited) {
    const previous = state.hdmi.choices.find(function (choice) { return choice.value === previousHdmi })
    hdmiConfig = previous.settings || state.hdmi.custom
    hdmiEdited = true
  }
  previousHdmi = selected
  updateHdmiControls()
}
function editHdmi() {
  const defaults = state.hdmi.choices.find(function (choice) { return choice.value === '720p60' }).settings
  const result = worEditText(hdmiConfig, state.labels.editHdmi, state.labels.hdmiEditorHelp, iconPath, defaults)
  if (result.saved) {
    hdmiConfig = result.text
    hdmiEdited = true
  }
}
function choosePath(directory) {
  const panel = $.NSOpenPanel.openPanel
  panel.canChooseDirectories = directory
  panel.canChooseFiles = !directory
  panel.allowsMultipleSelection = false
  if (!directory) panel.allowedFileTypes = $(['ffu', 'iso', 'msi'])
  if (panel.runModal === $.NSModalResponseOK) {
    const field = directory ? directoryField : sourceField
    field.stringValue = panel.URL.path
  }
}
const Controller = ObjC.registerSubclass({
  name: 'WorIotOptionsController',
  superclass: 'NSObject',
  methods: {
    'saveClicked:': {
      types: ['void', ['id']],
      implementation: function() {
        const local = Number(sourcePopup.indexOfSelectedItem) === 1
        if ((local && ObjC.unwrap(sourceField.stringValue).length === 0) || ObjC.unwrap(directoryField.stringValue).length === 0) {
          const alert = $.NSAlert.alloc.init
          alert.messageText = 'Complete the image settings'
          alert.informativeText = 'Choose an official FFU, ISO or MSI when Custom Image is selected, and a download folder.'
          alert.addButtonWithTitle('OK')
          alert.runModal
          return
        }
        confirmed = true
        closeOptions()
      }
    },
    'cancelClicked:': { types: ['void', ['id']], implementation: closeOptions },
    'windowWillClose:': { types: ['void', ['id']], implementation: closeOptions },
    'sourceChanged:': { types: ['void', ['id']], implementation: updateSourceControls },
    'hdmiChanged:': { types: ['void', ['id']], implementation: hdmiChanged },
    'editHdmi:': { types: ['void', ['id']], implementation: editHdmi },
    'accountChanged:': { types: ['void', ['id']], implementation: updateAccountControls },
    'languageChanged:': { types: ['void', ['id']], implementation: updateLanguageControls },
    'connectionAddressChanged:': { types: ['void', ['id']], implementation: updateConnectionAddress },
    'browseSource:': { types: ['void', ['id']], implementation: function() { choosePath(false) } },
    'browseDirectory:': { types: ['void', ['id']], implementation: function() { choosePath(true) } },
    'soundChanged:': { types: ['void', ['id']], implementation: function() { soundPopup.enabled = Number(soundCheckbox.state) === 1 } },
    'handleQuitEvent:withReplyEvent:': { types: ['void', ['id', 'id']], implementation: closeOptions },
    'handleReopenEvent:withReplyEvent:': {
      types: ['void', ['id', 'id']],
      implementation: function() {
        if (window.isMiniaturized) window.deminiaturize(null)
        window.makeKeyAndOrderFront(null)
        app.activateIgnoringOtherApps(true)
      }
    },
    'pumpEvents:': {
      types: ['void', ['id']],
      implementation: function() {
        $.NSRunLoop.currentRunLoop.runModeBeforeDate($.NSDefaultRunLoopMode, $.NSDate.dateWithTimeIntervalSinceNow(0.01))
      }
    },
    'applicationShouldTerminate:': { types: ['NSUInteger', ['id']], implementation: function() { closeOptions(); return $.NSTerminateCancel } }
  }
})
const controller = $.WorIotOptionsController.alloc.init
app.setActivationPolicy($.NSApplicationActivationPolicyRegular)
app.setDelegate(controller)
const mainMenu = worInstallAppMenu(app, appTitle, windowTitle, iconPath)
const editMenuItem = $.NSMenuItem.alloc.init
mainMenu.addItem(editMenuItem)
const editMenu = $.NSMenu.alloc.initWithTitle('Edit')
editMenuItem.submenu = editMenu
editMenu.addItemWithTitleActionKeyEquivalent('Cut', 'cut:', 'x')
editMenu.addItemWithTitleActionKeyEquivalent('Copy', 'copy:', 'c')
editMenu.addItemWithTitleActionKeyEquivalent('Paste', 'paste:', 'v')
editMenu.addItemWithTitleActionKeyEquivalent('Select All', 'selectAll:', 'a')
worSetAppIcon(app, iconPath)
const screen = $.NSScreen.mainScreen.visibleFrame
const width = Math.min(760, screen.size.width - 40)
const height = Math.min(886, screen.size.height - 60)
window = worMakeWindow({ width: width, height: height, title: windowTitle + ' | IoT Core Advanced Options', delegate: controller })
const heading = $.NSTextField.labelWithString('Windows 10 IoT Core Advanced Options')
heading.font = $.NSFont.systemFontOfSizeWeight(18, $.NSFontWeightSemibold)
heading.frame = $.NSMakeRect(24, height - 48, width - 48, 28)
window.contentView.addSubview(heading)
const scroll = $.NSScrollView.alloc.initWithFrame($.NSMakeRect(24, 72, width - 48, height - 130))
scroll.hasVerticalScroller = false
scroll.autohidesScrollers = false
scroll.scrollerStyle = $.NSScrollerStyleLegacy
let innerWidth = scroll.contentSize.width
const content = $.NSView.alloc.initWithFrame($.NSMakeRect(0, 0, innerWidth, 536))
scroll.documentView = content
window.contentView.addSubview(scroll)
function addLabel(text, y) {
  const label = $.NSTextField.labelWithString($(text))
  label.frame = $.NSMakeRect(0, y, 188, 26)
  content.addSubview(label)
  return label
}
function addRecommendedBadge() {
  const badge = $.NSTextField.labelWithString($(state.labels.recommended))
  badge.font = $.NSFont.systemFontOfSizeWeight(11, $.NSFontWeightSemibold)
  badge.textColor = $.NSColor.systemGreenColor
  content.addSubview(badge)
  return badge
}
function addPathField(text, y, action) {
  const field = $.NSTextField.alloc.initWithFrame($.NSMakeRect(198, y, innerWidth - 300, 26))
  field.stringValue = $(text)
  field.autoresizingMask = $.NSViewWidthSizable
  content.addSubview(field)
  const button = $.NSButton.buttonWithTitleTargetAction('Browse...', controller, action)
  button.bezelStyle = $.NSBezelStyleRounded
  button.frame = $.NSMakeRect(innerWidth - 94, y - 2, 94, 30)
  button.autoresizingMask = $.NSViewMinXMargin
  content.addSubview(button)
  return {field: field, button: button}
}
function addCheckbox(label, value, y, action) {
  const checkbox = $.NSButton.checkboxWithTitleTargetAction($(label), controller, action || $())
  checkbox.frame = $.NSMakeRect(0, y, innerWidth, 28)
  checkbox.state = value ? 1 : 0
  checkbox.autoresizingMask = $.NSViewWidthSizable
  content.addSubview(checkbox)
  return checkbox
}
function addOptionTextField(value, y, secure) {
  const frame = $.NSMakeRect(198, y, innerWidth - 198, 26)
  const field = secure ? $.NSSecureTextField.alloc.initWithFrame(frame) : $.NSTextField.alloc.initWithFrame(frame)
  field.stringValue = $(value)
  content.addSubview(field)
  return field
}
sourceLabel = addLabel(state.labels.source, 502)
sourcePopup = $.NSPopUpButton.alloc.initWithFrame($.NSMakeRect(198, 498, innerWidth - 198, 32))
sourcePopup.addItemWithTitle($(state.labels.official))
sourcePopup.addItemWithTitle($(state.labels.local))
sourcePopup.selectItemAtIndex(state.mode === 'local' ? 1 : 0)
sourcePopup.target = controller
sourcePopup.action = 'sourceChanged:'
content.addSubview(sourcePopup)
recommendedBadge = addRecommendedBadge()
customLabel = addLabel(state.labels.file, 456)
const sourceControls = addPathField(state.source, 456, 'browseSource:')
sourceField = sourceControls.field
sourceField.placeholderString = 'Official FFU, ISO or MSI'
browseSourceButton = sourceControls.button
addLabel(state.labels.cache, 410)
directoryField = addPathField(state.downloadDir, 410, 'browseDirectory:').field
hdmiLabel = addLabel(state.labels.hdmi, 358)
hdmiPopup = $.NSPopUpButton.alloc.initWithFrame($.NSMakeRect(198, 354, innerWidth - 198, 32))
for (const choice of state.hdmi.choices) hdmiPopup.addItemWithTitle($(choice.label))
hdmiPopup.selectItemAtIndex(state.hdmi.choices.findIndex(function (choice) { return choice.value === state.hdmiMode }))
hdmiPopup.target = controller
hdmiPopup.action = 'hdmiChanged:'
content.addSubview(hdmiPopup)
hdmiRecommendedBadge = addRecommendedBadge()
editHdmiButton = $.NSButton.buttonWithTitleTargetAction('View / Edit…', controller, 'editHdmi:')
editHdmiButton.bezelStyle = $.NSBezelStyleRounded
content.addSubview(editHdmiButton)
const hdmiHelp = $.NSTextField.wrappingLabelWithString($(state.labels.hdmiHelp))
hdmiHelp.frame = $.NSMakeRect(0, 292, innerWidth, 54)
hdmiHelp.textColor = $.NSColor.secondaryLabelColor
hdmiHelp.autoresizingMask = $.NSViewWidthSizable
content.addSubview(hdmiHelp)
dryRunCheckbox = addCheckbox(state.labels.dryRun, state.dryRun, 264)
const verification = $.NSTextField.wrappingLabelWithString($(state.labels.verification))
verification.textColor = $.NSColor.secondaryLabelColor
verification.frame = $.NSMakeRect(0, 184, innerWidth, 64)
verification.autoresizingMask = $.NSViewWidthSizable
content.addSubview(verification)
soundCheckbox = addCheckbox(state.labels.playSound, state.playSound, 132, 'soundChanged:')
addLabel(state.labels.sound, 94)
soundPopup = $.NSPopUpButton.alloc.initWithFrame($.NSMakeRect(198, 90, innerWidth - 198, 32))
for (let i = 0; i < state.sounds.length; i++) soundPopup.addItemWithTitle($(state.sounds[i].label))
soundPopup.selectItemAtIndex(Math.max(0, state.sounds.findIndex(function (sound) { return sound.value === state.completionSound })))
soundPopup.enabled = state.playSound
soundPopup.autoresizingMask = $.NSViewWidthSizable
content.addSubview(soundPopup)
notificationCheckbox = addCheckbox(state.labels.notification, state.showNotification, 48)
wipeCheckbox = addCheckbox(state.labels.wipeDrive, state.wipeDrive, 8)
wipeCheckbox.toolTip = $(state.labels.wipeHelp)
for (let i = 0; i < content.subviews.count; i++) {
  const view = content.subviews.objectAtIndex(i)
  const frame = view.frame
  accountRows.push({view: view, x: frame.origin.x, y: frame.origin.y, width: frame.size.width, height: frame.size.height})
}
accountCheckbox = addCheckbox(state.labels.accountSetup, state.accountSetup, 184, 'accountChanged:')
accountUsernameLabel = addLabel(state.labels.accountUsername, 136)
accountUsernameField = addOptionTextField(state.accountUsername, 136, false)
accountPasswordLabel = addLabel(state.labels.accountPassword, 90)
accountPasswordField = addOptionTextField(state.accountPassword, 90, true)
accountHelp = $.NSTextField.wrappingLabelWithString($(state.labels.accountHelp))
accountHelp.textColor = $.NSColor.secondaryLabelColor
content.addSubview(accountHelp)
languageCheckbox = addCheckbox(state.labels.languageSetup, state.languageSetup, 116, 'languageChanged:')
languageLabel = addLabel(state.labels.language, 78)
languagePopup = $.NSPopUpButton.alloc.initWithFrame($.NSMakeRect(198, 74, innerWidth - 198, 32))
for (const language of state.languages) languagePopup.addItemWithTitle($(language.label))
languagePopup.selectItemAtIndex(state.languages.findIndex(function (language) { return language.value.toLowerCase() === state.language.toLowerCase() }))
content.addSubview(languagePopup)
languageHelp = $.NSTextField.wrappingLabelWithString($(state.labels.languageHelp))
languageHelp.textColor = $.NSColor.secondaryLabelColor
content.addSubview(languageHelp)
connectionTitle = addLabel(state.labels.connectionTitle, 202)
connectionTitle.font = $.NSFont.systemFontOfSizeWeight(13, $.NSFontWeightSemibold)
automaticAddressCheckbox = addCheckbox(state.labels.automaticAddress, state.automaticAddress, 200, 'connectionAddressChanged:')
worAnnotateCheckbox(automaticAddressCheckbox, state.labels.automaticAddress, state.labels.recommended, $.NSColor.systemGreenColor)
automaticAddressCheckbox.toolTip = $(state.labels.automaticAddressHelp)
connectionHostLabel = addLabel(state.labels.connectionHost, 158)
connectionHostField = addOptionTextField(state.host, 158, false)
connectionHostField.placeholderString = $(state.automaticHostname + ' (automatic)')
updateConnectionAddress()
currentUsernameLabel = addLabel(state.labels.connectionUsername, 114)
currentUsernameField = addOptionTextField(state.currentUsername, 114, false)
currentUsernameField.toolTip = $(state.labels.connectionHelp)
currentPasswordLabel = addLabel(state.labels.connectionPassword, 70)
currentPasswordField = addOptionTextField(state.currentPassword, 70, true)
currentPasswordField.toolTip = $(state.labels.connectionHelp)
connectionHelp = $.NSTextField.wrappingLabelWithString($(state.labels.connectionHelp))
connectionHelp.textColor = $.NSColor.secondaryLabelColor
content.addSubview(connectionHelp)
updateLanguageControls()
const back = $.NSButton.buttonWithTitleTargetAction('Back', controller, 'cancelClicked:')
back.bezelStyle = $.NSBezelStyleRounded
back.keyEquivalent = '\u001b'
back.frame = $.NSMakeRect(width - 224, 22, 92, 32)
window.contentView.addSubview(back)
const save = $.NSButton.buttonWithTitleTargetAction('Save', controller, 'saveClicked:')
save.bezelStyle = $.NSBezelStyleRounded
save.keyEquivalent = '\r'
save.frame = $.NSMakeRect(width - 116, 22, 92, 32)
window.contentView.addSubview(save)
updateAccountControls()
updateSourceControls()
const pumpTimer = worInstallWindowHandlers(controller)
window.makeKeyAndOrderFront(null)
app.activateIgnoringOtherApps(true)
app.runModalForWindow(window)
pumpTimer.invalidate
const result = confirmed ? JSON.stringify({
  mode: Number(sourcePopup.indexOfSelectedItem) === 1 ? 'local' : 'official',
  source: ObjC.unwrap(sourceField.stringValue), downloadDir: ObjC.unwrap(directoryField.stringValue),
  dryRun: Number(dryRunCheckbox.state) === 1, playSound: Number(soundCheckbox.state) === 1,
  completionSound: state.sounds[Number(soundPopup.indexOfSelectedItem)].value,
  showNotification: Number(notificationCheckbox.state) === 1,
  hdmiMode: state.hdmi.choices[Number(hdmiPopup.indexOfSelectedItem)].value,
  hdmiConfig: hdmiEdited || previousHdmi === 'custom' ? hdmiConfig : state.hdmiConfig,
  accountSetup: Number(accountCheckbox.state) === 1,
  accountUsername: ObjC.unwrap(accountUsernameField.stringValue),
  accountPassword: ObjC.unwrap(accountPasswordField.stringValue),
  languageSetup: Number(languageCheckbox.state) === 1,
  language: state.languages[Number(languagePopup.indexOfSelectedItem)].value,
  wipeDrive: Number(wipeCheckbox.state) === 1,
  host: ObjC.unwrap(connectionHostField.stringValue),
  automaticAddress: Number(automaticAddressCheckbox.state) === 1,
  currentUsername: ObjC.unwrap(currentUsernameField.stringValue),
  currentPassword: ObjC.unwrap(currentPasswordField.stringValue)
}) : '__WOR_CANCEL__'
$.NSFileHandle.fileHandleWithStandardOutput.writeData($(result + '\n').dataUsingEncoding($.NSUTF8StringEncoding))
$.exit(0)
JXA
)"
  while true;do
    gui_iot_private_json "$state" state_file || return 1
    options="$(wor_osascript -l JavaScript - "$state_file" "$WOR_ICON_PATH" "$WOR_WINDOW_TITLE" "$WOR_APP_TITLE" <<<"$options_jxa")" || { rm -f "$state_file"; return 1; }
    rm -f "$state_file"
    [ "$options" != __WOR_CANCEL__ ] || return 0
    saved=0
    gui_iot_save_options "$options" || saved=$?
    [ "$saved" != 0 ] || { GUI_IOT_OPTIONS_SAVED=1; return 0; }
    [ "$saved" != 2 ] || return 0
    state="$(printf '%s\n%s\n' "$state" "$options" | jq -s '.[0] + .[1]')" || return 1
  done
}

linux_iot_options() {
  local state mode source_choices selected_label official_label selected_sound sound_choices options
  local source download_dir dry_run play_sound sound notification settings refresh_file refresh_requested response changed_action height
  local source_field folder_field dryrun_field play_field sound_field notification_field hdmi_field
  local hdmi_mode hdmi_label hdmi_choices hdmi_config hdmi_edit_file hdmi_editor hdmi_defaults saved
  local account_field account_username_field account_password_field account_setup account_username account_password
  local language_setup_field language_field language_setup language language_label language_choices
  local wipe_field wipe_drive
  local connection_host_field current_username_field current_password_field connection_host current_username current_password field_index
  local automatic_address_field automatic_address
  local fields=() scrolling=() form_fields=()
  GUI_IOT_OPTIONS_SAVED=0
  state="$(gui_iot_options_state)" || return 1
  official_label="$(wor_iot_option_label official) ($(wor_iot_option_label recommended))"
  while true;do
    mode="$(jq -r .mode <<<"$state")"
    selected_label="$official_label"
    [ "$mode" != local ] || selected_label="$(wor_iot_option_label local)"
    source_choices="$official_label!$(wor_iot_option_label local)"
    [ "$mode" != local ] || source_choices="$(wor_iot_option_label local)!$official_label"
    selected_sound="$(jq -r '.completionSound as $selected | .sounds[] | select(.value == $selected) | .label' <<<"$state")"
    sound_choices="$(printf '%s\n' "$selected_sound"; jq -r '.sounds[].label' <<<"$state")"
    sound_choices="$(awk 'NF && !seen[$0]++' <<<"$sound_choices" | paste -sd '!' -)"
    fields=("--field=$(wor_iot_option_label source):CB" "$source_choices")
    source_field=0
    source="$(jq -r .source <<<"$state")"
    height=540
    if [ "$mode" == local ];then
      source_field=$((${#fields[@]} / 2 + 1))
      fields+=("--field=$(wor_iot_option_label file):FL" "$source")
      height=586
    fi
    folder_field=$((${#fields[@]} / 2 + 1))
    fields+=("--field=$(wor_iot_option_label cache):DIR" "$(jq -r .downloadDir <<<"$state")")
    hdmi_mode="$(jq -r .hdmiMode <<<"$state")"
    hdmi_config="$(jq -r .hdmiConfig <<<"$state")"
    hdmi_label="$(jq -r --arg mode "$hdmi_mode" '.hdmi.choices[] | select(.value == $mode) | .menu_label' <<<"$state")"
    hdmi_choices="$(jq -r --arg mode "$hdmi_mode" \
      '[.hdmi.choices[] | select(.value == $mode)] + [.hdmi.choices[] | select(.value != $mode)] | map(.menu_label) | join("!")' <<<"$state")"
    hdmi_field=$((${#fields[@]} / 2 + 1))
    fields+=("--field=$(wor_iot_option_label hdmi):CB" "$hdmi_choices")
    hdmi_edit_file=''
    if [ "$hdmi_mode" == custom ];then
      hdmi_edit_file="$(mktemp)" || { warning "Cannot create the pending HDMI editor file."; return 1; }
      register_file_cleanup "$hdmi_edit_file"
      printf '%s' "$hdmi_config" > "$hdmi_edit_file" \
        || { rm -f "$hdmi_edit_file"; warning "Cannot initialize the pending HDMI editor."; return 1; }
      hdmi_editor="@yad --center --no-markup --width=$(wor_yad_width 700) --height=$(wor_yad_height 520) --title=$(printf '%q' "$WOR_WINDOW_TITLE | $(wor_iot_option_label editHdmi)") --window-icon=$(printf '%q' "$WOR_LOGO_PATH") --class=$(printf '%q' "$WOR_ICON_NAME") --text=$(printf '%q' "$(wor_iot_option_label hdmiEditorHelp)") --text-info --editable --in-place --confirm-save='Save video settings?' --filename=$(printf '%q' "$hdmi_edit_file") --button=Close:0 >/dev/null"
      fields+=("--field=$(wor_iot_option_label editHdmi):BT" "$hdmi_editor")
      hdmi_defaults="$(jq -r '.hdmi.choices[] | select(.value == "720p60") | .settings' <<<"$state")"
      fields+=("--field=Reset to 720p defaults:BT" "@bash -c $(printf '%q' "printf '%s\n' $(printf '%q' "$hdmi_defaults") > $(printf '%q' "$hdmi_edit_file")")")
      height=$((height+70))
    fi
    dryrun_field=$((${#fields[@]} / 2 + 1))
    fields+=("--field=$(wor_iot_option_label dryrun):CHK" "$(jq -r 'if .dryRun then "TRUE" else "FALSE" end' <<<"$state")")
    play_field=$((${#fields[@]} / 2 + 1))
    fields+=("--field=$(wor_iot_option_label playSound):CHK" "$(jq -r 'if .playSound then "TRUE" else "FALSE" end' <<<"$state")")
    sound_field=$((${#fields[@]} / 2 + 1))
    fields+=("--field=$(wor_iot_option_label sound):CB" "$sound_choices")
    notification_field=$((${#fields[@]} / 2 + 1))
    fields+=("--field=$(wor_iot_option_label notification):CHK" "$(jq -r 'if .showNotification then "TRUE" else "FALSE" end' <<<"$state")")
    account_setup="$(jq -r 'if .accountSetup then "TRUE" else "FALSE" end' <<<"$state")"
    account_username="$(jq -r .accountUsername <<<"$state")"
    account_password="$(jq -r .accountPassword <<<"$state")"
    account_field=$((${#fields[@]} / 2 + 1))
    fields+=("--field=$(wor_iot_option_label accountSetup):CHK" "$account_setup")
    account_username_field=$((${#fields[@]} / 2 + 1))
    [ "$account_setup" == TRUE ] && fields+=("--field=$(wor_iot_option_label accountUsername):TXT" "$account_username") \
      || fields+=("--field=$(wor_iot_option_label accountUsername):TXT" '@disabled@')
    account_password_field=$((${#fields[@]} / 2 + 1))
    [ "$account_setup" == TRUE ] && fields+=("--field=$(wor_iot_option_label accountPassword):H" "$account_password") \
      || fields+=("--field=$(wor_iot_option_label accountPassword):H" '@disabled@')
    height=$((height+132))
    language_setup="$(jq -r 'if .languageSetup then "TRUE" else "FALSE" end' <<<"$state")"
    language="$(jq -r .language <<<"$state")"
    language_label="$(jq -r --arg language "$language" '.languages[] | select((.value | ascii_downcase) == ($language | ascii_downcase)) | .label' <<<"$state")"
    language_choices="$(jq -r --arg language "$language" \
      '[.languages[] | select((.value | ascii_downcase) == ($language | ascii_downcase))] + [.languages[] | select((.value | ascii_downcase) != ($language | ascii_downcase))] | map(.label) | join("!")' <<<"$state")"
    language_setup_field=$((${#fields[@]} / 2 + 1))
    fields+=("--field=$(wor_iot_option_label languageSetup):CHK" "$language_setup")
    language_field=$((${#fields[@]} / 2 + 1))
    fields+=("--field=$(wor_iot_option_label language):CB" "$language_choices")
    height=$((height+78))
    wipe_field=$((${#fields[@]} / 2 + 1))
    fields+=("--field=$(wor_iot_option_label wipeDrive):CHK" "$(jq -r 'if .wipeDrive then "TRUE" else "FALSE" end' <<<"$state")")
    height=$((height+36))
    connection_host="$(jq -r .host <<<"$state")"
    automatic_address="$(jq -r 'if .automaticAddress then "TRUE" else "FALSE" end' <<<"$state")"
    current_username="$(jq -r .currentUsername <<<"$state")"
    current_password="$(jq -r .currentPassword <<<"$state")"
    automatic_address_field=$((${#fields[@]} / 2 + 1))
    fields+=("--field=$(wor_iot_option_label automaticAddress) ($(wor_iot_option_label recommended)):CHK" "$automatic_address")
    connection_host_field=$((${#fields[@]} / 2 + 1))
    [ "$automatic_address" == TRUE ] && fields+=("--field=$(wor_iot_option_label connectionHost):TXT" '@disabled@') \
      || fields+=("--field=$(wor_iot_option_label connectionHost):TXT" "$connection_host")
    current_username_field=$((${#fields[@]} / 2 + 1))
    fields+=("--field=$(wor_iot_option_label connectionUsername):TXT" "$current_username")
    current_password_field=$((${#fields[@]} / 2 + 1))
    fields+=("--field=$(wor_iot_option_label connectionPassword):H" "$current_password")
    height=$((height+168))
    form_fields=()
    for ((field_index=0;field_index<${#fields[@]};field_index+=2));do
      form_fields+=("${fields[$field_index]}")
      case "${fields[$((field_index+1))]}" in
        *$'\r'* | *$'\n'*) warning "An IoT form value contains a line break. Correct the configuration before opening Advanced Options."; return 1 ;;
      esac
    done
    scrolling=()
    [ "$(wor_yad_height "$height")" -ge "$height" ] || scrolling=(--scroll)
    refresh_file="$(mktemp)" || { warning "Could not create the IoT options refresh marker."; return 1; }
    changed_action="if { [ \"\$1\" == 1 ] && [ \"\$2\" != $(printf '%q' "$selected_label") ]; } \
      || { [ \"\$1\" == $hdmi_field ] && [ \"\$2\" != $(printf '%q' "$hdmi_label") ]; } \
      || { [ \"\$1\" == $account_field ] && [ \"\$2\" != $(printf '%q' "$account_setup") ]; } \
      || { [ \"\$1\" == $automatic_address_field ] && [ \"\$2\" != $automatic_address ]; };then
      printf 'refresh\n' > $(printf '%q' "$refresh_file")
      kill -USR1 \"\$YAD_PID\"
    fi"
    response=0
    options="$(for ((field_index=0;field_index<${#fields[@]};field_index+=2));do
      printf '%s\n' "${fields[$((field_index+1))]}"
    done | yad "${yadflags[@]}" --response=0 --changed-action="$changed_action" \
      --width="$(wor_yad_width 740)" --height="$(wor_yad_height "$height")" \
      --title="$WOR_WINDOW_TITLE | IoT Core Advanced Options" --form "${scrolling[@]}" \
      --text="$(wor_iot_option_label verification)

$(wor_iot_option_label hdmiHelp)

$(wor_iot_option_label accountHelp)

$(wor_iot_option_label languageHelp)

$(wor_iot_option_label connectionHelp)

$(wor_iot_option_label automaticAddressHelp)" "${form_fields[@]}" \
      --button=Back:1 --button=Save:0)" || response=$?
    refresh_requested=0
    [ ! -s "$refresh_file" ] || refresh_requested=1
    rm -f "$refresh_file"
    if [ -n "$hdmi_edit_file" ];then
      hdmi_config="$(cat "$hdmi_edit_file")" \
        || { rm -f "$hdmi_edit_file"; warning "Cannot read the pending HDMI editor."; return 1; }
      rm -f "$hdmi_edit_file"
    fi
    [ "$response" == 0 ] || return 0
    mode="$(sed -n '1p' <<<"$options")"
    case "$mode" in
      "$official_label") mode=official ;;
      "$(wor_iot_option_label local)") mode=local ;;
      *) warning "Invalid IoT image-source selection. No preferences were changed."; return 1 ;;
    esac
    [ "$source_field" == 0 ] || source="$(sed -n "${source_field}p" <<<"$options")"
    download_dir="$(sed -n "${folder_field}p" <<<"$options")"
    dry_run="$(sed -n "${dryrun_field}p" <<<"$options")"
    play_sound="$(sed -n "${play_field}p" <<<"$options")"
    sound="$(sed -n "${sound_field}p" <<<"$options")"
    notification="$(sed -n "${notification_field}p" <<<"$options")"
    account_setup="$(sed -n "${account_field}p" <<<"$options")"
    language_setup="$(sed -n "${language_setup_field}p" <<<"$options")"
    wipe_drive="$(sed -n "${wipe_field}p" <<<"$options")"
    [ "$automatic_address" == TRUE ] || connection_host="$(sed -n "${connection_host_field}p" <<<"$options")"
    automatic_address="$(sed -n "${automatic_address_field}p" <<<"$options")"
    current_username="$(sed -n "${current_username_field}p" <<<"$options")"
    current_password="$(sed -n "${current_password_field}p" <<<"$options")"
    language_label="$(sed -n "${language_field}p" <<<"$options")"
    language="$(jq -er --arg label "$language_label" '.languages[] | select(.label == $label) | .value' <<<"$state")" \
      || { warning "Invalid IoT language selection. No preferences were changed."; return 1; }
    if [ "$(jq -r .accountSetup <<<"$state")" == true ];then
      account_username="$(sed -n "${account_username_field}p" <<<"$options")"
      account_password="$(sed -n "${account_password_field}p" <<<"$options")"
    fi
    [ "$account_setup" == TRUE ] || account_password=''
    hdmi_label="$(sed -n "${hdmi_field}p" <<<"$options")"
    hdmi_mode="$(jq -er --arg label "$hdmi_label" '.hdmi.choices[] | select(.menu_label == $label) | .value' <<<"$state")" \
      || { warning "Invalid IoT HDMI mode. No preferences were changed."; return 1; }
    if [ "$hdmi_mode" == custom ] && [ -z "$hdmi_config" ] && [ "$(jq -r .hdmiMode <<<"$state")" != custom ];then
      hdmi_config="$(jq -r '.hdmiMode as $mode | .hdmi.choices[] | select(.value == $mode) | .settings' <<<"$state")"
      [ -n "$hdmi_config" ] || hdmi_config="$(jq -r .hdmi.custom <<<"$state")"
    fi
    for settings in "$dry_run" "$play_sound" "$notification" "$account_setup" "$language_setup" "$wipe_drive" "$automatic_address";do
      case "$settings" in TRUE | FALSE) ;; *) warning "Invalid IoT checkbox value. No preferences were changed."; return 1 ;; esac
    done
    sound="$(jq -er --arg label "$sound" '.sounds[] | select(.label == $label) | .value' <<<"$state")" \
      || { warning "Invalid IoT completion sound. No preferences were changed."; return 1; }
    settings="$(WOR_IOT_ACCOUNT_PASSWORD="$account_password" WOR_IOT_CURRENT_PASSWORD="$current_password" jq -n --arg mode "$mode" --arg source "$source" --arg downloadDir "$download_dir" \
      --arg dryRun "$dry_run" --arg playSound "$play_sound" --arg completionSound "$sound" --arg showNotification "$notification" \
      --arg hdmiMode "$hdmi_mode" --arg hdmiConfig "$hdmi_config" \
      --arg accountSetup "$account_setup" --arg accountUsername "$account_username" \
      --arg languageSetup "$language_setup" --arg language "$language" \
      --arg wipeDrive "$wipe_drive" \
      --arg host "$connection_host" --arg currentUsername "$current_username" \
      --arg automaticAddress "$automatic_address" \
      '{mode: $mode, source: $source, downloadDir: $downloadDir, dryRun: ($dryRun == "TRUE"),
        playSound: ($playSound == "TRUE"), completionSound: $completionSound, showNotification: ($showNotification == "TRUE"),
        hdmiMode: $hdmiMode, hdmiConfig: $hdmiConfig, accountSetup: ($accountSetup == "TRUE"),
        accountUsername: $accountUsername, accountPassword: env.WOR_IOT_ACCOUNT_PASSWORD,
        languageSetup: ($languageSetup == "TRUE"), language: $language,
        wipeDrive: ($wipeDrive == "TRUE"), host: $host, automaticAddress: ($automaticAddress == "TRUE"),
        currentUsername: $currentUsername,
        currentPassword: env.WOR_IOT_CURRENT_PASSWORD}')" || return 1
    if [ "$refresh_requested" == 1 ] || [ "$mode" != "$(jq -r .mode <<<"$state")" ] \
      || [ "$hdmi_mode" != "$(jq -r .hdmiMode <<<"$state")" ] \
      || [ "$account_setup" != "$(jq -r 'if .accountSetup then "TRUE" else "FALSE" end' <<<"$state")" ] \
      || [ "$automatic_address" != "$(jq -r 'if .automaticAddress then "TRUE" else "FALSE" end' <<<"$state")" ];then
      state="$(printf '%s\n%s\n' "$state" "$settings" | jq -s '.[0] + .[1]')" || return 1
      continue
    fi
    saved=0
    gui_iot_save_options "$settings" || saved=$?
    [ "$saved" != 0 ] || { GUI_IOT_OPTIONS_SAVED=1; return 0; }
    [ "$saved" != 2 ] || return 0
    state="$(printf '%s\n%s\n' "$state" "$settings" | jq -s '.[0] + .[1]')" || return 1
  done
}

macos_start_cli() {
  local completion_jxa confirm_summary confirmation current_rpi_model current_windows_ver default_language device_choices device_capability device_choice done_marker abort_marker auth_marker error_marker install_mode installer_pid installer_status language_choices mode_choices output_log password_retry_choice privacy_guidance privacy_settings_url progress_file progress_jxa progress_status progress_diagnostics progress_failed resume_at_flash saved_log step target_choice disk_alert_pid disk_alert_status disk_alert_done disk_alert_log disk_alert_warning validation_status
  local completion_connect

  current_windows_ver='Windows 11'
  current_rpi_model=''
  if is_iot_core;then
    current_windows_ver='Windows 10 IoT Core (ARM32, legacy)'
    [ -z "${WOR_TARGET_BOARD:-}" ] || current_rpi_model="$(rpi_board_label)"
  fi
  step=target
  while true; do
    case "$step" in
      target)
        target_choice="$(macos_choose_target "$current_windows_ver" "$current_rpi_model")" || exit 0
        WINDOWS_VER="${target_choice%%$'\t'*}"
        select_windows_family "$WINDOWS_VER" || error "Unrecognized Windows image family."
        select_rpi_board "${target_choice#*$'\t'}" || error "Unrecognized Raspberry Pi selection."
        current_windows_ver="$WINDOWS_VER"
        current_rpi_model="${target_choice#*$'\t'}"
        if is_iot_core;then
          CAN_INSTALL_ON_SAME_DRIVE=1
          gui_iot_plan_source || error "Cannot read the reviewed IoT Core source settings."
        else
          list_bids 10 >/dev/null || error "Failed to retrieve available Windows versions."
          [ "$WINDOWS_VER" == 'Windows 11' ] && BID="$(get_bid 11)" || BID="$(get_bid 10)"
          [ -n "$BID" ] || error "No compatible Windows build is available for $(rpi_board_label)."
          set_default_config_txt
          [ -z "$WIN_LANG" ] && WIN_LANG="$(default_win_lang)"
        fi
        if is_iot_core && [ "$DRY_RUN" == 1 ];then
          DEVICE=''
          step=confirm
          continue
        fi
        step=device
        ;;
      language)
        [ -z "$WIN_LANG" ] && WIN_LANG="$(default_win_lang)"
        step=device
        ;;
      device)
        device_choices="$(darwin_list_device_choices)"
        device_choice="$(macos_choose_device "$device_choices")" || exit 0
        if [ "$device_choice" == Back ];then
          step=target
          continue
        fi
        [ "$device_choice" == __REFRESH__ ] && continue
        DEVICE="${device_choice%%$'\t'*}"
        is_iot_core && iot_core_clear_target_approval
        is_safe_target_device "$DEVICE" || error "Refusing to overwrite $DEVICE. Choose an external, physical, writable whole disk."
        if ! is_iot_core;then
          device_capability="$(drive_capability "$DEVICE")"
          validate_install_mode "$device_capability"
        fi
        step=mode
        ;;
      mode)
        if is_iot_core;then
          CAN_INSTALL_ON_SAME_DRIVE=1
          validation_status=0
          gui_iot_validate_target || validation_status=$?
          if [ "$validation_status" == 2 ];then step=device;continue;fi
          [ "$validation_status" == 0 ] || exit 0
          step=confirm
          continue
        fi
        if [ "$device_capability" == recovery ];then
          CAN_INSTALL_ON_SAME_DRIVE=0
        else
          mode_choices=$'Install Windows onto this drive\nCreate a recovery drive'
          install_mode="$(macos_choose "$mode_choices" 'Choose installation mode' 'Install Windows onto this drive' Back '' '' '' '' '' '' Back)" || exit 0
          if [ "$install_mode" == Back ];then
            step=device
            continue
          fi
          [ "$install_mode" == 'Install Windows onto this drive' ] && CAN_INSTALL_ON_SAME_DRIVE=1 || CAN_INSTALL_ON_SAME_DRIVE=0
        fi
        step=confirm
        ;;
      confirm)
        confirmation="$(macos_confirm_flash)" || confirmation=Cancel
        [ "$confirmation" == Cancel ] && exit 0
        if [ "$confirmation" == Advanced ];then
          macos_advanced_options
          if is_iot_core && [ "$DRY_RUN" != 1 ] && [ -z "$DEVICE" ];then step=device;fi
          continue
        fi
        if [ "$confirmation" == Flash ];then
          if is_iot_core;then
            gui_iot_confirm_wipe || continue
            WOR_IOT_CONFIRM_ERASE=1
          fi
          break
        fi
        if is_iot_core;then step=device;else step=mode;fi
        ;;
    esac
  done

  completion_jxa="$(wor_jxa_window_lib; cat <<'JXA'
ObjC.import('AppKit')
ObjC.import('stdlib')
const args = $.NSProcessInfo.processInfo.arguments
function optionalArgument(index, fallback) {
  return Number(args.count) > index ? ObjC.unwrap(args.objectAtIndex(index)) : fallback
}
const message = ObjC.unwrap(args.objectAtIndex(4))
const iconPath = ObjC.unwrap(args.objectAtIndex(5))
const appTitle = ObjC.unwrap(args.objectAtIndex(6))
const imagePath = optionalArgument(7, '')
const windowTitle = optionalArgument(8, appTitle)
const successSound = optionalArgument(9, '')
const settingsUrl = optionalArgument(10, '')
const connectAllowed = optionalArgument(11, '') === 'connect' && imagePath.length > 0

$.NSProcessInfo.processInfo.processName = appTitle
const app = $.NSApplication.sharedApplication
app.setActivationPolicy($.NSApplicationActivationPolicyRegular)
worInstallAppMenu(app, appTitle, windowTitle, iconPath)
worSetAppIcon(app, iconPath)

let window
let completionAction = '__WOR_CLOSE__'
const Controller = ObjC.registerSubclass({
  name: 'WorCompletionController',
  superclass: 'NSObject',
  methods: {
    'openLogClicked:': {
      types: ['void', ['id']],
      implementation: function() {
        if (logPath.length > 0) {
          $.NSWorkspace.sharedWorkspace.openFileWithApplication($(logPath), $())
        }
      }
    },
    'copyLogClicked:': {
      types: ['void', ['id']],
      implementation: function() {
        if (logPath.length > 0) {
          const pb = $.NSPasteboard.generalPasteboard
          pb.clearContents
          pb.setStringForType($(logPath), $.NSPasteboardTypeString)
        }
      }
    },
    'openSettingsClicked:': {
      types: ['void', ['id']],
      implementation: function() {
        if (settingsUrl.length > 0) {
          $.NSWorkspace.sharedWorkspace.openURL($.NSURL.URLWithString($(settingsUrl)))
        }
      }
    },
    'okClicked:': {
      types: ['void', ['id']],
      implementation: function() {
        app.stopModalWithCode($.NSOKButton)
        window.orderOut(null)
      }
    },
    'connectClicked:': {
      types: ['void', ['id']],
      implementation: function() {
        if (!connectAllowed) return
        completionAction = '__WOR_CONNECT__'
        app.stopModalWithCode($.NSOKButton)
        window.orderOut(null)
      }
    },
    'windowWillClose:': {
      types: ['void', ['id']],
      implementation: function() {
        app.stopModalWithCode($.NSOKButton)
      }
    },
    //right-click Quit from the Dock/app-switcher sends terminate: to NSApp; since this window is
    //hosted by osascript rather than a full NSApplicationMain run loop, that does not otherwise exit the process
    'handleQuitEvent:withReplyEvent:': {
      types: ['void', ['id', 'id']],
      implementation: function() {
        $.exit(0)
      }
    },
    'pumpEvents:': {
      types: ['void', ['id']],
      implementation: function() {
        $.NSRunLoop.currentRunLoop.runModeBeforeDate($.NSDefaultRunLoopMode, $.NSDate.dateWithTimeIntervalSinceNow(0.01))
      }
    },
    //clicking the Dock icon sends aevt/rapp; without a handler a minimised window can never come back
    'handleReopenEvent:withReplyEvent:': {
      types: ['void', ['id', 'id']],
      implementation: function() {
        if (window.isMiniaturized) window.deminiaturize(null)
        window.makeKeyAndOrderFront(null)
        app.activateIgnoringOtherApps(true)
      }
    },
    'applicationShouldTerminate:': {
      types: ['NSUInteger', ['id']],
      implementation: function() {
        $.exit(0)
      }
    }
  }
})
const controller = $.WorCompletionController.alloc.init
app.setDelegate(controller)

const contentMargin = 20
const buttonY = 22
const buttonHeight = 32
let width = 600
let height = 360
let bannerImage = null
let bannerWidth = 0
let bannerHeight = 0

if (imagePath.length > 0) {
  bannerImage = $.NSImage.alloc.initWithContentsOfFile($(imagePath))
  //NSImage.size is DPI-scaled, so a 96dpi PNG reports three quarters of its pixels; measure the bitmap.
  //Number() is required: JXA hands these back as strings, and '650' + 40 would concatenate into a bogus frame.
  const reps = bannerImage.representations
  const rep = Number(reps.count) > 0 ? reps.objectAtIndex(0) : null
  bannerWidth = rep ? Number(rep.pixelsWide) : Number(bannerImage.size.width)
  bannerHeight = rep ? Number(rep.pixelsHigh) : Number(bannerImage.size.height)
  const maxBannerWidth = 760
  if (bannerWidth > maxBannerWidth) {
    bannerHeight = Math.round(bannerHeight * (maxBannerWidth / bannerWidth))
    bannerWidth = maxBannerWidth
  }
  width = Math.max(600, bannerWidth + contentMargin * 2)
}

//measure at the final width so neither layout can clip the message
const measured = $.NSMutableAttributedString.alloc.init
measured.mutableString.appendString($(message))
measured.addAttributeValueRange($.NSFontAttributeName, $.NSFont.systemFontOfSizeWeight(14, $.NSFontWeightMedium), $.NSMakeRange(0, message.length))
const measuredBounds = measured.boundingRectWithSizeOptions($.NSMakeSize(width - contentMargin * 2, 2000), $.NSStringDrawingUsesLineFragmentOrigin | $.NSStringDrawingUsesFontLeading)
const textHeight = Math.ceil(Number(measuredBounds.size.height))

if (imagePath.length > 0) {
  height = contentMargin + bannerHeight + 24 + textHeight + 20 + buttonHeight + buttonY
} else {
  //100pt of chrome: the button row below the text and the gap under the title bar
  height = Math.min(560, Math.max(200, textHeight + 100))
}
//fixed layout: no drag-resize, no zoom/maximize, and no minimize - this is a terminal screen, so
//the close box is the only titlebar control and behaves the same as clicking OK (see windowWillClose:)
window = worMakeWindow({ width: width, height: height, title: windowTitle, delegate: controller, miniaturizable: false })

const content = window.contentView

const textY = imagePath.length > 0 ? buttonY + buttonHeight + 20 : 70
const label = $.NSTextField.labelWithString(message)
label.font = $.NSFont.systemFontOfSizeWeight(14, $.NSFontWeightMedium)
label.setUsesSingleLineMode(false)
label.cell.setWraps(true)
label.cell.setScrollable(false)
label.frame = $.NSMakeRect(contentMargin, textY, width - contentMargin * 2, imagePath.length > 0 ? textHeight : height - 100)
//1, not $.NSTextAlignmentCenter: that constant bridges as 2, which this runtime draws right-aligned
if (imagePath.length > 0) label.setAlignment(1)
content.addSubview(label)

if (imagePath.length > 0) {
  const imageView = $.NSImageView.alloc.initWithFrame($.NSMakeRect(Math.round((width - bannerWidth) / 2), textY + textHeight + 24, bannerWidth, bannerHeight))
  imageView.image = bannerImage
  imageView.imageScaling = $.NSImageScaleProportionallyUpOrDown
  content.addSubview(imageView)
}

//extract the log file path if present (after "Full log: ") and create clickable buttons
let logPath = ''
const logMatch = message.match(/Full log: (.+)$/)
if (logMatch) {
  logPath = logMatch[1].trim()
}

let okButton, openButton, copyButton, settingsButton
const buttonGap = 8
let nextButtonX = 20
//"Complete" only on the success screen; a failure dialog reached through Open Log/Copy is not a completion
const primaryLabel = imagePath.length > 0 ? 'Complete' : 'OK'
if (logPath.length > 0) {
  openButton = $.NSButton.buttonWithTitleTargetAction('Open Log', controller, 'openLogClicked:')
  openButton.bezelStyle = $.NSBezelStyleRounded
  openButton.sizeToFit
  const openWidth = Math.max(96, openButton.frame.size.width)
  openButton.frame = $.NSMakeRect(nextButtonX, buttonY, openWidth, buttonHeight)
  content.addSubview(openButton)
  nextButtonX += openWidth + buttonGap

  copyButton = $.NSButton.buttonWithTitleTargetAction('Copy', controller, 'copyLogClicked:')
  copyButton.bezelStyle = $.NSBezelStyleRounded
  copyButton.sizeToFit
  const copyWidth = Math.max(80, copyButton.frame.size.width)
  copyButton.frame = $.NSMakeRect(nextButtonX, buttonY, copyWidth, buttonHeight)
  content.addSubview(copyButton)
  nextButtonX += copyWidth + buttonGap

  if (settingsUrl.length > 0) {
    settingsButton = $.NSButton.buttonWithTitleTargetAction('Open Settings', controller, 'openSettingsClicked:')
    settingsButton.bezelStyle = $.NSBezelStyleRounded
    settingsButton.sizeToFit
    const settingsWidth = Math.max(120, settingsButton.frame.size.width)
    settingsButton.frame = $.NSMakeRect(nextButtonX, buttonY, settingsWidth, buttonHeight)
    content.addSubview(settingsButton)
  }

  okButton = $.NSButton.buttonWithTitleTargetAction(primaryLabel, controller, 'okClicked:')
  okButton.bezelStyle = $.NSBezelStyleRounded
  okButton.keyEquivalent = '\r'
  okButton.sizeToFit
  const okWidth = Math.max(96, okButton.frame.size.width)
  okButton.frame = $.NSMakeRect(width - 20 - okWidth, buttonY, okWidth, buttonHeight)
  content.addSubview(okButton)
} else {
  okButton = $.NSButton.buttonWithTitleTargetAction(primaryLabel, controller, 'okClicked:')
  okButton.bezelStyle = $.NSBezelStyleRounded
  okButton.keyEquivalent = '\r'
  okButton.sizeToFit
  const okWidth = Math.max(96, okButton.frame.size.width)
  //centred under the message when it is the only button, right-aligned otherwise
  const okX = imagePath.length > 0 ? Math.round((width - okWidth) / 2) : width - 20 - okWidth
  okButton.frame = $.NSMakeRect(okX, buttonY, okWidth, buttonHeight)
  content.addSubview(okButton)
}

//a flash runs long enough to walk away from, so announce the result; these ship with macOS, so there
//is no audio asset to bundle, and they follow the system alert volume and Do Not Disturb settings.
//soundNamed returns a truthy wrapper for a missing name, so isNil is the only guard that works.
//An empty successSound means the user turned the sound off; a failure keeps the standard error sound.
if (connectAllowed) {
  const connectButton = $.NSButton.buttonWithTitleTargetAction('Connect to Pi', controller, 'connectClicked:')
  connectButton.bezelStyle = $.NSBezelStyleRounded
  connectButton.sizeToFit
  const connectWidth = Math.max(132, connectButton.frame.size.width)
  const okWidth = okButton.frame.size.width
  const connectX = logPath.length > 0 ? okButton.frame.origin.x - buttonGap - connectWidth
    : Math.round((width - connectWidth - buttonGap - okWidth) / 2)
  if (logPath.length === 0) okButton.frame = $.NSMakeRect(connectX + connectWidth + buttonGap, buttonY, okWidth, buttonHeight)
  connectButton.frame = $.NSMakeRect(connectX, buttonY, connectWidth, buttonHeight)
  content.addSubview(connectButton)
}
const soundName = imagePath.length > 0 ? successSound : 'Basso'
if (soundName.length > 0) {
  const completionSound = $.NSSound.soundNamed(soundName)
  if (!completionSound.isNil()) completionSound.play
}

window.makeKeyAndOrderFront(null)
if (!app.isActive) app.requestUserAttention($.NSInformationalRequest)
app.activateIgnoringOtherApps(true)
worInstallWindowHandlers(controller)
app.runModalForWindow(window)
if (connectAllowed) $.NSFileHandle.fileHandleWithStandardOutput.writeData($(completionAction + '\n').dataUsingEncoding($.NSUTF8StringEncoding))
app.terminate(null)
JXA
)"

  progress_jxa="$(wor_jxa_window_lib; cat <<'JXA'
ObjC.import('AppKit')
ObjC.import('Foundation')
ObjC.import('stdlib')
const args = $.NSProcessInfo.processInfo.arguments
const progressFile = ObjC.unwrap(args.objectAtIndex(4))
const doneMarker = ObjC.unwrap(args.objectAtIndex(5))
const iconPath = ObjC.unwrap(args.objectAtIndex(6))
const appTitle = ObjC.unwrap(args.objectAtIndex(7))
const abortMarker = ObjC.unwrap(args.objectAtIndex(8))
const windowTitle = ObjC.unwrap(args.objectAtIndex(9) || appTitle)
const diskAlertStatus = ObjC.unwrap(args.objectAtIndex(10))
const diskAlertDone = ObjC.unwrap(args.objectAtIndex(11))

$.NSProcessInfo.processInfo.processName = appTitle
const app = $.NSApplication.sharedApplication
app.setActivationPolicy($.NSApplicationActivationPolicyRegular)
worInstallAppMenu(app, appTitle, windowTitle, iconPath)
worSetAppIcon(app, iconPath)

const fm = $.NSFileManager.defaultManager

function readFile(path) {
  if (!fm.fileExistsAtPath($(path))) return ''
  try {
    const data = $.NSString.stringWithContentsOfFileEncodingError($(path), $.NSUTF8StringEncoding, undefined)
    return ObjC.unwrap(data) || ''
  } catch (e) {
    return ''
  }
}

function lastMatch(lines, prefix) {
  for (let i = lines.length - 1; i >= 0; i--) {
    if (lines[i].indexOf(prefix) === 0) return lines[i]
  }
  return ''
}

let window, bar, taskBar, phaseLabel, detailLabel, stepLabel, stepPercentLabel, taskPercentLabel, diskAlertLabel

function updateDiskAlertStatus() {
  try {
    const text = readFile(diskAlertStatus)
    if (text.length > 0) {
      const status = JSON.parse(text)
      diskAlertLabel.stringValue = status.state === 'waiting' || status.state === 'watching' ? '' : status.message
      diskAlertLabel.textColor = status.state === 'warning' ? $.NSColor.systemOrangeColor : $.NSColor.secondaryLabelColor
    }
    const result = readFile(diskAlertDone).trim()
    if (result.length > 0 && result !== '0') {
      diskAlertLabel.stringValue = 'Automatic Ignore is unavailable. Allow WoR-Flasher in Accessibility and Automation, or choose Ignore manually.'
      diskAlertLabel.textColor = $.NSColor.systemOrangeColor
    }
  } catch (error) {
    diskAlertLabel.stringValue = 'Automatic Ignore status is unavailable. Choose Ignore manually if the unreadable-disk alert appears.'
    diskAlertLabel.textColor = $.NSColor.systemOrangeColor
  }
}

//stopping part-way leaves an unbootable drive, so make the user confirm and record why we stopped
function confirmAbort() {
  const alert = $.NSAlert.alloc.init
  alert.messageText = 'Stop flashing this drive?'
  alert.informativeText = 'Preparation will stop. If disk writing has begun, the drive may be incomplete and must not be booted.'
  alert.alertStyle = $.NSAlertStyleCritical
  alert.addButtonWithTitle('Stop flashing')
  alert.addButtonWithTitle('Keep going')
  if (alert.runModal !== $.NSAlertFirstButtonReturn) return false
  $('').writeToFileAtomicallyEncodingError(abortMarker, true, $.NSUTF8StringEncoding, null)
  return true
}

const Controller = ObjC.registerSubclass({
  name: 'WorProgressController',
  superclass: 'NSObject',
  methods: {
    'tick:': {
      types: ['void', ['id']],
      implementation: function() {
        try {
          if (fm.fileExistsAtPath($(doneMarker))) {
            app.stopModalWithCode($.NSOKButton)
            return
          }
          updateDiskAlertStatus()
          const content = readFile(progressFile)
          if (content.length === 0) return
          const lines = content.split('\n')
          const stepLine = lastMatch(lines, 'STEP\t')
          const subLine = lastMatch(lines, 'SUBSTEP\t')
          const taskLine = lastMatch(lines, 'TASK\t')
          const statusLine = lastMatch(lines, 'STATUS\t')
          let taskText = ''
          let taskPercentText = ''
          let currentPercent = -1
          if (taskLine.length > 0) {
            const taskParts = taskLine.split('\t')
            const taskPct = parseInt(taskParts[1], 10)
            const taskLabel = taskParts.slice(2).join('\t')
            if (taskLabel.length > 0 && !isNaN(taskPct)) {
              currentPercent = Math.max(0, Math.min(100, taskPct))
              taskText = taskLabel
              taskPercentText = 'Sub-progress: ' + currentPercent + '%'
            }
          }
          if (stepLine.length > 0) {
            const parts = stepLine.split('\t')
            const stepNum = parseInt(parts[1], 10)
            const stepTotal = parseInt(parts[2], 10)
            if (!isNaN(stepNum) && !isNaN(stepTotal) && stepTotal > 0) {
              let within = 0
              if (subLine.length > 0) {
                const parsed = parseInt(subLine.split('\t')[1], 10)
                if (!isNaN(parsed)) within = Math.max(0, Math.min(100, parsed))
              }
              if (currentPercent < 0) currentPercent = within
              if (taskPercentText.length === 0 && currentPercent > 0) taskPercentText = 'Sub-progress: ' + currentPercent + '%'
              //count in hundredths of a step so progress inside a step is visible too
              bar.indeterminate = false
              bar.minValue = 0
              bar.maxValue = stepTotal * 100
              bar.doubleValue = (stepNum - 1) * 100 + within
              stepPercentLabel.stringValue = 'Overall: ' + Math.round(bar.doubleValue / bar.maxValue * 100) + '%'
              taskBar.indeterminate = false
              taskBar.minValue = 0
              taskBar.maxValue = 100
              taskBar.doubleValue = Math.max(0, Math.min(100, currentPercent))

              let stepStr = 'Step ' + stepNum + ' of ' + stepTotal
              stepLabel.stringValue = stepStr
            }
            phaseLabel.stringValue = parts.slice(3).join('\t')
          } else if (subLine.length > 0) {
            //work before the first numbered step, such as clearing the cache, still has its own percentage
            const parsed = parseInt(subLine.split('\t')[1], 10)
            if (!isNaN(parsed)) {
              bar.indeterminate = false
              bar.minValue = 0
              bar.maxValue = 100
              bar.doubleValue = Math.max(0, Math.min(100, parsed))
              stepPercentLabel.stringValue = 'Overall: ' + parsed + '%'
              taskBar.indeterminate = false
              taskBar.minValue = 0
              taskBar.maxValue = 100
              taskBar.doubleValue = Math.max(0, Math.min(100, parsed))
              stepLabel.stringValue = parsed + '%'
              taskPercentLabel.stringValue = 'Sub-progress: ' + parsed + '%'
            }
          }
          if (taskText.length > 0) {
            detailLabel.stringValue = taskText
          } else if (statusLine.length > 0) {
            detailLabel.stringValue = statusLine.split('\t').slice(1).join('\t')
          }
          taskPercentLabel.stringValue = taskPercentText
        } catch (e) {}
      }
    },
    //right-click Quit from the Dock/app-switcher sends terminate: to NSApp; since this window is
    //hosted by osascript rather than a full NSApplicationMain run loop, that does not otherwise exit the process
    'handleQuitEvent:withReplyEvent:': {
      types: ['void', ['id', 'id']],
      implementation: function() {
        //quitting mid-flash must go through the same confirmation and let the shell stop the installer
        if (confirmAbort()) app.stopModalWithCode($.NSCancelButton)
      }
    },
    'pumpEvents:': {
      types: ['void', ['id']],
      implementation: function() {
        $.NSRunLoop.currentRunLoop.runModeBeforeDate($.NSDefaultRunLoopMode, $.NSDate.dateWithTimeIntervalSinceNow(0.01))
      }
    },
    'abortClicked:': {
      types: ['void', ['id']],
      implementation: function() {
        if (confirmAbort()) app.stopModalWithCode($.NSCancelButton)
      }
    },
    //the close box must go through the same confirmation, so veto it unless the user means it
    'windowShouldClose:': {
      types: ['bool', ['id']],
      implementation: function() {
        if (!confirmAbort()) return false
        app.stopModalWithCode($.NSCancelButton)
        return true
      }
    },
    //clicking the Dock icon sends aevt/rapp; without a handler a minimised window can never come back
    'handleReopenEvent:withReplyEvent:': {
      types: ['void', ['id', 'id']],
      implementation: function() {
        if (window.isMiniaturized) window.deminiaturize(null)
        window.makeKeyAndOrderFront(null)
        app.activateIgnoringOtherApps(true)
      }
    },
    'applicationShouldTerminate:': {
      types: ['NSUInteger', ['id']],
      implementation: function() {
        if (confirmAbort()) app.stopModalWithCode($.NSCancelButton)
        //NSTerminateCancel: never let AppKit kill this process while a flash is running
        return 0
      }
    }
  }
})
const controller = $.WorProgressController.alloc.init
app.setDelegate(controller)

const width = 680
const height = 330
window = worMakeWindow({ width: width, height: height, title: windowTitle, delegate: controller })

const content = window.contentView
window.contentView = content

const logoWidth = 56
const logoHeight = 173
const progressX = 96
const progressWidth = width - progressX - 20
const logoImage = $.NSImage.alloc.initWithContentsOfFile($(iconPath))
if (!logoImage.isNil()) {
  const logoView = $.NSImageView.alloc.initWithFrame($.NSMakeRect(20, height - 20 - logoHeight, logoWidth, logoHeight))
  logoView.image = logoImage
  logoView.imageScaling = $.NSImageScaleProportionallyUpOrDown
  content.addSubview(logoView)
}

phaseLabel = $.NSTextField.labelWithString('Starting...')
phaseLabel.frame = $.NSMakeRect(progressX, 260, width - progressX - 140, 24)
phaseLabel.font = $.NSFont.systemFontOfSizeWeight(14, $.NSFontWeightBold)
content.addSubview(phaseLabel)

stepLabel = $.NSTextField.labelWithString('')
stepLabel.frame = $.NSMakeRect(width - 120, 260, 100, 24)
stepLabel.font = $.NSFont.systemFontOfSizeWeight(12, $.NSFontWeightRegular)
stepLabel.alignment = $.NSTextAlignmentRight
content.addSubview(stepLabel)

stepPercentLabel = $.NSTextField.labelWithString('')
stepPercentLabel.frame = $.NSMakeRect(width - 140, 207, 120, 16)
stepPercentLabel.font = $.NSFont.monospacedDigitSystemFontOfSizeWeight(11, $.NSFontWeightMedium)
stepPercentLabel.alignment = $.NSTextAlignmentRight
content.addSubview(stepPercentLabel)

detailLabel = $.NSTextField.labelWithString('')
detailLabel.frame = $.NSMakeRect(progressX, 163, progressWidth - 170, 16)
detailLabel.font = $.NSFont.systemFontOfSizeWeight(11, $.NSFontWeightRegular)
content.addSubview(detailLabel)

taskPercentLabel = $.NSTextField.labelWithString('')
taskPercentLabel.frame = $.NSMakeRect(width - 170, 163, 150, 16)
taskPercentLabel.font = $.NSFont.monospacedDigitSystemFontOfSizeWeight(11, $.NSFontWeightMedium)
taskPercentLabel.alignment = $.NSTextAlignmentRight
content.addSubview(taskPercentLabel)

bar = $.NSProgressIndicator.alloc.initWithFrame($.NSMakeRect(progressX, 185, progressWidth, 20))
bar.indeterminate = true
bar.startAnimation(null)
content.addSubview(bar)

taskBar = $.NSProgressIndicator.alloc.initWithFrame($.NSMakeRect(progressX, 142, progressWidth, 10))
taskBar.indeterminate = false
taskBar.minValue = 0
taskBar.maxValue = 100
taskBar.doubleValue = 0
content.addSubview(taskBar)

diskAlertLabel = $.NSTextField.wrappingLabelWithString('')
diskAlertLabel.frame = $.NSMakeRect(20, 106, width - 40, 30)
diskAlertLabel.font = $.NSFont.systemFontOfSizeWeight(10, $.NSFontWeightRegular)
diskAlertLabel.textColor = $.NSColor.secondaryLabelColor
content.addSubview(diskAlertLabel)

const noteLabel = $.NSTextField.labelWithString('This window will close automatically when the process finishes.')
noteLabel.frame = $.NSMakeRect(20, 82, width - 40, 20)
noteLabel.font = $.NSFont.systemFontOfSizeWeight(11, $.NSFontWeightRegular)
//1, not $.NSTextAlignmentCenter: that constant bridges as 2, which this runtime draws right-aligned
noteLabel.alignment = 1
content.addSubview(noteLabel)

const abortButton = $.NSButton.buttonWithTitleTargetAction('Abort', controller, 'abortClicked:')
abortButton.bezelStyle = $.NSBezelStyleRounded
abortButton.sizeToFit
const abortWidth = Math.max(96, abortButton.frame.size.width)
abortButton.frame = $.NSMakeRect(Math.round((width - abortWidth) / 2), 22, abortWidth, 32)
content.addSubview(abortButton)

window.makeKeyAndOrderFront(null)
app.activateIgnoringOtherApps(true)

const timer = $.NSTimer.scheduledTimerWithTimeIntervalTargetSelectorUserInfoRepeats(0.4, controller, 'tick:', undefined, true)
worInstallWindowHandlers(controller)
app.runModalForWindow(window)
timer.invalidate
window.orderOut(null)
JXA
)"

  resume_at_flash=0
  while true; do
    abort_marker="$(mktemp -u)"

    gui_start_installer

    progress_status=0
    progress_failed=0
    progress_diagnostics="$(wor_osascript -l JavaScript - "$progress_file" "$done_marker" "$WOR_ICON_PATH" "$WOR_APP_TITLE" "$abort_marker" "$WOR_WINDOW_TITLE" "$disk_alert_status" "$disk_alert_done" <<<"$progress_jxa" 2>&1)" \
      || progress_status=$?

    #Command-Q or a crashed/killed osascript can bypass the JXA abort handler. Never leave the flash unattended.
    if [ ! -f "$done_marker" ] && [ ! -e "$abort_marker" ];then
      progress_failed=1
      touch "$abort_marker"
    fi

    if [ -e "$abort_marker" ];then
      if [ "$progress_failed" == 1 ];then
        status "The progress window closed unexpectedly; stopping the installer"
      else
        status "Aborting at your request"
      fi
      #most of the work runs under sudo, so the tree has to come down with the credential we already hold
      kill_process_tree "$installer_pid"
    fi
    wait "$installer_pid" 2>/dev/null
    gui_stop_disk_alert_handler
    printf '\nProgress window exit status: %s\n' "$progress_status" >> "$output_log"
    [ -z "$progress_diagnostics" ] || printf '%s\n' "$progress_diagnostics" >> "$output_log"

    if [ -e "$abort_marker" ];then
      completion_text="Flashing was stopped before it finished."
      if [ "$progress_failed" == 1 ];then
        completion_text="The progress window closed unexpectedly. The installer was stopped before it finished."
      fi
      printf '%s\nInstaller exit status: 1 (interrupted).\n' "$completion_text" >> "$output_log"
      rm -f "$progress_file" "$done_marker" "$abort_marker" "$auth_marker" "$error_marker"
      saved_log="$(gui_save_installer_log)"
      wor_show_result_notification failure "$(windows_version_label)"
      macos_show_result_dialog "$completion_text

$(windows_version_label) media was not completed.

If disk writing had begun, $DEVICE may be incomplete. Do not boot an unverified image.

Full log: $saved_log" '' '' "$saved_log"
      exit 1
    fi

    installer_status="$(cat "$done_marker" 2>/dev/null)"
    if ! [[ "$installer_status" =~ ^[0-9]+$ ]];then
      printf 'The installer did not leave a valid exit status; success cannot be confirmed.\n' >> "$output_log"
      installer_status=1
    fi
    printf 'Installer exit status: %s\n' "$installer_status" >> "$output_log"
    saved_log="$(gui_save_installer_log)"
    password_retry_choice=''
    if [ "$installer_status" != 0 ];then
      if ! installer_showed_own_error;then
        password_retry_choice="$(macos_password_retry_dialog "$saved_log" "$progress_file")" || password_retry_choice=''
      fi
    fi
    rm -f "$progress_file" "$done_marker" "$abort_marker" "$auth_marker"

    privacy_guidance=''
    privacy_settings_url=''
    if [ "$installer_status" == 0 ];then
      rm -f "$error_marker"
      completion_text="Process completed successfully.

    $(windows_version_label) media preparation is complete.

    It is now safe to remove your USB drive."
      if is_iot_core;then
        completion_text="Process completed successfully.

$(iot_core_next_steps)"
      fi
      [ -z "$disk_alert_warning" ] || completion_text="$completion_text"$'\n\n'"$disk_alert_warning"
      completion_text="$completion_text"$'\n\n'"Full log: $saved_log"
    else
      #installer writes the error_marker before showing its own error dialog; if it exists, skip the completion dialog
      if installer_showed_own_error ;then
        rm -f "$error_marker"
        exit "$installer_status"
      fi
      if [ "$password_retry_choice" == retry ];then
        resume_at_flash=1
        continue
      elif [ "$password_retry_choice" == close ];then
        exit "$installer_status"
      fi
      if grep -qF 'macOS denied removable-volume access' "$saved_log" 2>/dev/null ;then
        privacy_settings_url='x-apple.systempreferences:com.apple.preference.security?Privacy_FilesAndFolders'
        privacy_guidance="Click Open Settings, then in Files and Folders enable Removable Volumes for bash when it is listed. If macOS does not offer that narrower permission, open Full Disk Access, click +, press Shift-Command-G, enter /bin/bash, click Open, and enable its toggle. Then quit WoR-Flasher completely and try again."
      elif grep -qF 'Full Disk Access' "$saved_log" 2>/dev/null || grep -qF 'Operation not permitted' "$saved_log" 2>/dev/null ;then
        privacy_settings_url='x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles'
        privacy_guidance="Click Open Settings, then enable Full Disk Access for the exact WoR-Flasher.app copy you launched. If it is not listed, click + and add that app. If this still fails, also enable the launcher app you used, such as Visual Studio Code.app, Terminal.app, or iTerm.app. Then quit WoR-Flasher completely and try again."
      fi
      [ -z "$privacy_guidance" ] || privacy_guidance="$privacy_guidance

    "
      completion_text="The $(windows_version_label) media preparation stopped unexpectedly (exit code $installer_status).

    $privacy_guidance$(gui_log_tail "$saved_log")

Full log: $saved_log"
    fi
    completion_image=''
    [ "$installer_status" == 0 ] && completion_image="$WOR_ASSETS_DIR/next-steps.png"
    is_iot_core && [ "$installer_status" == 0 ] && completion_image="$WOR_LOGO_PATH"
    #posted before the window opens, so it lands while the app is still in the background
    [ "$installer_status" == 0 ] && wor_show_result_notification success "$(windows_version_label)" || wor_show_result_notification failure "$(windows_version_label)"
    completion_connect=0
    if [ "$installer_status" == 0 ] && is_iot_core && iot_core_personalization_requested && [ "$DRY_RUN" != 1 ];then
      completion_connect=1
    fi
    if macos_show_result_dialog "$completion_text" "$completion_image" "$privacy_settings_url" "$saved_log" "$([ "${PLAY_SOUND:-1}" == 1 ] && wor_completion_sound)" "$completion_connect";then
      if [ "$completion_connect" == 1 ] && [ "$GUI_RESULT_ACTION" == connect ];then
        gui_iot_account_setup "$saved_log" || exit 1
      fi
    fi
    exit "$installer_status"
  done
}

if is_macos ;then
  command -v osascript >/dev/null 2>&1 || error "Cannot present graphical interface: osascript is unavailable on this macOS host. Cannot continue."
  macos_check_accessibility || exit 0
  setup gui || exit 1
  announcement_choice="$(macos_show_announcement)" || exit 0
  macos_start_cli
  exit $?
fi

if is_wsl ;then
  error "Cannot present graphical interface: WoR-Flasher does not support WSL.
WSL cannot access USB drives directly, and the drives it does list are WSL's own virtual disks.
On Windows, use the official Windows on Raspberry Imager instead: https://worproject.com/downloads"
fi

if [ "$HOST_OS" != Linux ];then
  if [ "${OS:-}" == "Windows_NT" ] || [[ "$HOST_OS" =~ MINGW|MSYS|CYGWIN|Windows ]];then
    error "Cannot present graphical interface: WoR-Flasher does not support Windows hosts. On Windows, use the official Windows on Raspberry Imager instead: https://worproject.com/downloads"
  else
    error "Cannot present graphical interface: WoR-Flasher supports Linux and macOS hosts only. This host is $HOST_OS."
  fi
fi

if [ -z "${DISPLAY:-}" ] && [ -z "${WAYLAND_DISPLAY:-}" ];then
  error "Cannot present graphical interface: No active graphical display session (DISPLAY or WAYLAND_DISPLAY is unset). Cannot continue."
fi

#run safety checks and install packages
setup gui || exit 1

if ! command -v yad >/dev/null 2>&1 ;then
  error "Cannot present graphical interface: 'yad' is missing and could not be installed. Cannot continue."
fi

ensure_linux_desktop_identity() { #Install a desktop identity so GNOME maps yad windows to the WoR-Flasher icon.
  is_macos && return 0
  [ -n "${HOME:-}" ] || return 0
  [ -f "$WOR_LOGO_PATH" ] || return 0
  local app_dir desktop_file exec_path icon_path
  app_dir="$HOME/.local/share/applications"
  desktop_file="$app_dir/wor-flasher.desktop"
  mkdir -p "$app_dir" 2>/dev/null || return 0
  exec_path="$(printf '%s' "$DIRECTORY/install-wor-gui.sh" | sed 's/\\/\\\\/g; s/"/\\"/g')"
  icon_path="$(printf '%s' "$WOR_LOGO_PATH" | sed 's/\\/\\\\/g; s/"/\\"/g')"
  cat > "$desktop_file" <<DESKTOP 2>/dev/null || return 0
[Desktop Entry]
Type=Application
Name=$WOR_APP_TITLE
Exec="$exec_path"
Icon=$icon_path
StartupWMClass=$WOR_ICON_NAME
Terminal=false
Categories=Utility;
DESKTOP
}

linux_no_device_message() { #Output: explanation when lsblk sees devices but no safe target drive.
  printf '%s\n\n%s\n%s\n%s' \
    'No external writable target drive was found.' \
    'WoR-Flasher hides loop/snap devices, the current boot disk, and anything that is not a writable whole disk.' \
    'Connect a USB drive or SD card reader, then click Refresh.' \
    'If you expected a drive to appear, run lsblk and confirm it is not the system disk.'
}

linux_choose_one() { #Input: newline choices, prompt, default, optional Back label. Output: choice or Back.
  local choices="$1" prompt="$2" default_choice="$3" back_label="${4:-}" choice result button rows=''
  local buttons=(--button='<b>Next</b>':0)
  [ -z "$back_label" ] || buttons=(--button="<b>$back_label</b>":1 "${buttons[@]}")
  while IFS= read -r choice ;do
    [ -n "$choice" ] || continue
    if [ "$choice" == "$default_choice" ];then
      rows+="TRUE\n${choice}\n"
    else
      rows+="FALSE\n${choice}\n"
    fi
  done <<< "$choices"
  result="$(printf '%b' "$rows" | yad "${yadflags[@]}" --width="$(wor_yad_width 560)" --height="$(wor_yad_height 420)" \
    --list --radiolist --no-selection --no-headers --column=selected:CHK --column=choice \
    --print-column=2 --text="<big><b>$prompt</b></big>" "${buttons[@]}")"
  button=$?
  if [ "$button" == 0 ];then
    [ -n "$result" ] || return 1
    printf '%s\n' "$result"
  elif [ "$button" == 1 ] && [ -n "$back_label" ];then
    printf 'Back\n'
  else
    return 1
  fi
}

linux_choose_target() { #Collects Windows and a compatible board on one screen, matching macOS.
  local windows="${1:-Windows 11}" desktop_board="${2:-}" iot_board="${2:-}" family boards board windows_choices
  local result response refresh_file refresh_requested changed_action previous_windows previous_family
  local all_windows=$'Windows 11\nWindows 10\nWindows 10 IoT Core (ARM32, legacy)\nMore options'
  while true;do
    family=desktop
    [ "$windows" != 'Windows 10 IoT Core (ARM32, legacy)' ] || family=iot-core
    boards="$(wor_rpi_board_options "$family")"
    [ "$family" == desktop ] && board="$desktop_board" || board="$iot_board"
    if ! grep -qxF "$board" <<<"$boards";then board="$(head -n1 <<<"$boards")";fi
    boards="$(printf '%s\n%s\n' "$board" "$boards" | awk 'NF && !seen[$0]++' | paste -sd '!' -)"
    windows_choices="$(printf '%s\n%s\n' "$windows" "$all_windows" | awk 'NF && !seen[$0]++' | paste -sd '!' -)"
    refresh_file="$(mktemp)" || { warning "Could not create the target-picker refresh marker."; return 1; }
    previous_windows="$windows" previous_family="$family"
    #Yad cannot replace a live combo catalog; refresh the same form and retain both families' choices.
    changed_action="if [ \"\$1\" == 1 ] && [ \"\$2\" != $(printf '%q' "$windows") ];then
      printf 'refresh\n' > $(printf '%q' "$refresh_file")
      kill -USR1 \"\$YAD_PID\"
    fi"
    response=0
    result="$(yad "${yadflags[@]}" --response=0 --changed-action="$changed_action" \
      --width="$(wor_yad_width 680)" --height="$(wor_yad_height 260)" \
      --text='<big><b>Choose Windows and Raspberry Pi target</b></big>' \
      --form --align=center --buttons-layout=center \
      --field='Windows version:CB' "$windows_choices" \
      --field='Raspberry Pi model:CB' "$boards" \
      --button='<b>Cancel</b>':1 --button='<b>Next</b>':0)" || response=$?
    refresh_requested=0
    [ ! -s "$refresh_file" ] || refresh_requested=1
    rm -f "$refresh_file"
    [ "$response" == 0 ] || return 1
    windows="$(sed -n '1p' <<<"$result")"
    board="$(sed -n '2p' <<<"$result")"
    if ! grep -qxF "$windows" <<<"$all_windows";then
      warning "Unrecognized Windows image selection. No image was prepared."
      return 1
    fi
    if grep -qxF "$board" <<<"$(wor_rpi_board_options "$previous_family")";then
      [ "$previous_family" == desktop ] && desktop_board="$board" || iot_board="$board"
    fi
    if [ "$refresh_requested" == 1 ] || [ "$windows" != "$previous_windows" ];then continue;fi
    if ! grep -qxF "$board" <<<"$(wor_rpi_board_options "$family")";then
      warning "Choose a board supported by the selected Windows image. No image was prepared."
      return 1
    fi
    printf '%s\t%s\n' "$windows" "$board"
    return 0
  done
}

ensure_linux_desktop_identity

#this array stores flags that are used in all yad windows - saves on the typing and makes it easy to change an attribute on all dialogs from one place.
#--class sets the window's WM_CLASS so the taskbar/Alt-Tab switcher shows WoR-Flasher instead of
#the generic "yad" process name
wor_init_yad_flags
announcement_image="$(wor_yad_image_for_screen "$WOR_ASSETS_DIR/partnership.png" "$WOR_LOGO_PATH" 880 740)"

#display partnership announcement
#match the macOS announcement composition: the full banner sits above readable centered copy rather
#than consuming almost the whole width and squeezing the text into a one-word column beside it.
#--image must precede --form (as it does for the overview.png dialog below) or yad packs the image
#beside the field column instead of above it, regardless of --image-on-top.
announcement_status=0
yad "${yadflags[@]}" --width="$(wor_yad_width 840)" --height="$(wor_yad_height 720)" --center \
  --image="$announcement_image" --image-on-top --text-align=center \
  --form --align=center --buttons-layout=center --timeout="$WOR_ANNOUNCEMENT_TIMEOUT" --timeout-indicator=bottom \
  --field=$'<a href="https://blackoutsecure.app/">Blackout Secure</a> is proud to partner with <a href="https://github.com/Botspot">Botspot</a> and the <a href="https://worproject.com/">Windows on R</a> community, carrying WoR-Flasher forward while preserving Botspot\'s original authorship and project direction.\n\nReport issues, share feedback, or contribute at <a href="https://github.com/Botspot/wor-flasher">Botspot/wor-flasher</a>.\n\nSupport continued development by <a href="https://github.com/sponsors/Botspot">sponsoring Botspot</a> or <a href="https://github.com/sponsors/blackoutsecure?frequency=one-time&amp;amount=8">buying Blackout Secure a coffee</a> on GitHub.':LBL '' \
  --button='<b>Proceed with WoR-Flasher</b>':0 >/dev/null || announcement_status=$?
#yad returns 70 when its timeout expires; that is the automatic Proceed action, not a quit.
[ "$announcement_status" == 0 ] || [ "$announcement_status" == 70 ] || exit 0

{ #choose destination RPi model and windows build ID
RPI_MODEL=''
BID=''
WINDOWS_VER='Windows 11'
is_iot_core && WINDOWS_VER='Windows 10 IoT Core (ARM32, legacy)'
target_choice="$(linux_choose_target "$WINDOWS_VER" "$(rpi_board_label)")" || exit 0
WINDOWS_VER="${target_choice%%$'\t'*}"
rpi_choice="${target_choice#*$'\t'}"
select_windows_family "$WINDOWS_VER" || error "Unrecognized Windows image family."
select_rpi_board "$rpi_choice" || error "Unrecognized Raspberry Pi selection."

case "$WINDOWS_VER" in
    'Windows 10 IoT Core (ARM32, legacy)')
      CAN_INSTALL_ON_SAME_DRIVE=1
      gui_iot_plan_source || error "Cannot read the reviewed IoT Core source settings."
      if [ "$DRY_RUN" == 1 ];then
        DEVICE=''
      fi
      ;;
    'Windows 11' | 'Windows 10')
      loading_dialog "Finding best $WINDOWS_VER image version..." &
      loader_pid=$!
      trap stop_loader EXIT

      list_bids 10 >/dev/null #set $versions globally so it is not downloaded twice
      if [ "$WINDOWS_VER" == 'Windows 11' ];then
        BID="$(get_bid 11)" || exit 1
      elif [ "$WINDOWS_VER" == 'Windows 10' ];then
        BID="$(get_bid 10)" || exit 1
      fi

      stop_loader
      ;;

    'More options')
      #display more options for OS choice to user: enter exact version, use ISO, use pre-extracted ISO

      BID=''
      while [ -z "$BID" ];do
        reply="$(echo -e "FALSE\nChoose an exact Windows version to download\nenter exact
FALSE\nUse a Windows ISO file\nuse iso
FALSE\nUse a cached version of Windows from a previous run\nuse cached" | yad "${yadflags[@]}" --width="$(wor_yad_width 420)" \
          --list --radiolist --column=chk:CHK --column=human --column=script:HD --no-headers --print-column=3 --no-selection \
          --text=$'<big><b>More options</b></big>' \
          --button='<b>Next</b>':0)"
        button=$?
        [ $button != 0 ] && exit 0

        case "$reply" in
          'enter exact')
            list_bids 10 >/dev/null #set $versions globally so it is not downloaded twice
            while [ -z "$BID" ];do
              BID="$(echo -n "$(list_bids_supported 11 | sed 's/^/Windows 11 /g'
              list_bids_supported 10 | sed 's/^/Windows 10 /g')" | sed 's/^/FALSE\n/g' | yad "${yadflags[@]}" --width="$(wor_yad_width 420)" \
                --list --radiolist --column=chk:CHK --column=human --no-headers --print-column=2 --no-selection \
                --text=$'Choose version of Windows:' \
                --button='<b>Next</b>':0)"
              button=$?
              [ $button != 0 ] && exit 0

              #Isolate build number from selection
              BID="$(echo "$BID" | awk '{print $3}')"
            done
            break
            ;;
          'use iso')
            SOURCE_FILE="$(yad "${yadflags[@]}" --width="$(wor_yad_width 420)" \
              --file --file-filter "ISO disk images | *.ISO *.iso" \
              --text=$'<big><b>Import ISO file</b></big>\nMust be an ARM64 version of Windows from <a href="https://uupdump.net">uupdump.net</a>' \
              --button="<b>Cancel</b>":1 --button="<b>OK</b>":0)"

            #verify ISO file
            if [ -z "$SOURCE_FILE" ];then
              break #exit ISO file menu
            elif ! iso_problem="$(validate_iso_file "$SOURCE_FILE")" ;then
              yad "${yadflags[@]}" --text="$iso_problem"
              SOURCE_FILE=''
            else #ISO file looks good
              #Infer Build ID based on filename of ISO
              BID="$(bid_from_iso_name "$SOURCE_FILE")"
              if [ -z "$BID" ];then
                BID="$(yad --form --field= '' "${yadflags[@]}" \
                  --text='To store files from this ISO, this script needs to know the Windows build number of this ISO.\nPlease enter it now: (example: '"$EXAMPLE_BID"')' \
                  --button="<b>OK</b>":0)"
                [ -z "$BID" ] && error "Cannot proceed without a build number for your ISO file."
              fi
              #Infer language based on filename of ISO
              WIN_LANG="$(lang_from_iso_name "$SOURCE_FILE")"
              if [ -z "$WIN_LANG" ];then
                WIN_LANG="$(yad --form --field= '' "${yadflags[@]}" \
                  --text='To store files from this ISO, this script needs to know the language of this Windows ISO.\nPlease enter it now: (example: en-us)' \
                  --button="<b>OK</b>":0)"
                if [ -z "$WIN_LANG" ];then
                  error "Cannot proceed without a language for your ISO file."
                elif ! is_known_win_lang "$WIN_LANG" ;then
                  error "Language code was not found in the list!\n$(list_langs | awk '{print $1}' | tr '\n' ' ')"
                fi
              fi
              break
            fi
            ;;
          'use cached')
            #Discover past extracted ISO files in this DL_DIR so user does not need to keep ISO
            #folders in DL_DIR named winfiles_from_iso_<BID>_<WIN_LANG>
            while true;do
              list=''
              existing_winfiles="$(list_cached_winfiles)"

              echo "$existing_winfiles"

              for folder in $existing_winfiles ;do
                BID="$(bid_from_winfiles_dir "$folder")"
                WIN_LANG="$(lang_from_winfiles_dir "$folder")"

                list+="FALSE\n$(get_os_name "$BID") $WIN_LANG\n${folder}\n"
                num_opts=$((num_opts+1))
              done
              unset BID WIN_LANG #Avoid leaving these variables set from the loop

              folder="$(echo -ne "$list" | yad "${yadflags[@]}" --height="$(wor_yad_height 320)" \
                --list --radiolist --column=chk:CHK --column=human --column=script:HD --no-headers --print-column=3 --no-selection \
                --text=$'<big><b>Choose Cached Windows Files</b></big>\nIf the list is empty, choose the download folder used by the previous run.\nCurrent download folder: <b><u>'"$DL_DIR"'</u></b>' \
                --button='<b>Change Download Folder</b>':2 \
                --button='<b>Next</b>':0)"
              button=$?

              case $button in
                0) #Next
                  if [ ! -z "$folder" ];then
                    #A cached version of windows (winfiles folder) was selected; infer BID and WIN_LANG from it
                    BID="$(bid_from_winfiles_dir "$folder")"
                    WIN_LANG="$(lang_from_winfiles_dir "$folder")"

                    #DL_DIR cannot be changed later on - it is being relied upon for winfiles
                    break
                  else
                    #nothing selected; present the window again
                    true
                  fi
                  ;;
                2) #change DL_DIR
                  DL_DIR="$(yad "${yadflags[@]}" --file --directory --mime-filter="Directories | inode/directory" \
                    --width="$(wor_yad_width 500)" --height="$(wor_yad_height 400)" --title="Choose Download Folder" \
                    --text=$'Choose where WoR-Flasher stores downloaded and extracted Windows files.\nFor cached Windows files, select the folder used by the previous run.' \
                    --button="<b>Cancel</b>":1 --button="<b>OK</b>":0 \
                    || echo "$DL_DIR")"
                    #This ^^^^^^^^^^^ preserves the current value of DL_DIR if anything other than OK is clicked
                  ;;
                *)
                  exit 0 #user wishes to exit the list of previously extracted winfiles
                  ;;
              esac
            done
            ;;
        esac
      done
      ;;
    *)
      error "Unrecognized user-selected WINDOWS_VER '$WINDOWS_VER'"
      ;;
  esac
echo "BID: $BID
RPI_MODEL: $RPI_MODEL"
}

{ #choose language
if ! is_iot_core && [ -z "$WIN_LANG" ];then
  WIN_LANG="$(default_win_lang)"
fi
echo "WIN_LANG: $WIN_LANG"
}

linux_choose_flash_device() { #Collects a device using the shared enumeration and safety helpers.
if [ -z "$DEVICE" ];then
  while [ -z "$DEVICE" ] || [ ! -b "$DEVICE" ];do
    IFS=$'\n'
    DEV_LIST=''
    device_buttons=(--button="<b>Refresh</b>!!Reload the list of connected drives to detect new ones":2 --button='<b>Next</b>':0)
    for device in $(list_dev_paths) ;do
      [ "$(get_size_raw "$device")" -le 0 ] && continue
      DEV_LIST="FALSE
${device}
<b>${device}</b>
$(lsblk -dno SIZE "$device")B
$(get_device_name "$device")
$DEV_LIST"
    done

    if [ -z "$DEV_LIST" ];then
      device_prompt="$(linux_no_device_message)"
      device_buttons=(--button='<b>Cancel</b>':1 --button="<b>Refresh</b>!!Reload the list of connected drives to detect new ones":2)
    else
      device_prompt='Choose device to flash:'
    fi

    DEVICE="$(echo -n "$DEV_LIST" | sed -e '0,/FALSE/ s/FALSE/TRUE/' | yad "${yadflags[@]}" --text="$device_prompt" --width="$(wor_yad_width 520)" \
      --list --radiolist --no-selection --no-headers --column=chk:CHK --column=echoname:HD --column=name --column=size --column=pretty-name \
      --print-column=2 --tooltip-column=3 \
      "${device_buttons[@]}")"
    button=$?
    if [ $button == 0 ];then
      #OK
      true #do nothing and while loop will exit if $DEVICE is valid
    elif [ $button == 2 ];then
      #Refresh
      DEVICE=''
    else
      #Cancel, or unknown button
      exit 1
    fi
  done
elif [ -z "$(lsblk -no PATH "$DEVICE")" ];then
  error "Invalid value for DEVICE: $DEVICE is not a valid drive!"
fi
#same guard the macOS wizard applies, so a caller-supplied DEVICE cannot be the host's own boot disk
is_safe_target_device "$DEVICE" || error "Refusing to overwrite $DEVICE, which is this system's boot drive."
echo "DEVICE: $DEVICE"
}

if ! is_iot_core || [ "$DRY_RUN" != 1 ];then linux_choose_flash_device;fi

{ #choose installation mode from the detected drive capacity
if ! is_iot_core || [ "$DRY_RUN" != 1 ];then
if is_iot_core;then
  iot_core_clear_target_approval
  linux_iot_validate_selection || exit 0
fi
device_capability="$(drive_capability "$DEVICE")"
validate_install_mode "$device_capability"

if [ -z "$CAN_INSTALL_ON_SAME_DRIVE" ] && [ "$device_capability" == recovery ];then
  echo "Drive $DEVICE is too small to install Windows to itself. Using recovery-drive mode to install Windows on another larger device."
  CAN_INSTALL_ON_SAME_DRIVE=0
elif [ -z "$CAN_INSTALL_ON_SAME_DRIVE" ];then
  while [ -z "$CAN_INSTALL_ON_SAME_DRIVE" ];do
    install_mode="$(echo -e "TRUE\ninstall\nInstallation drive\nInstall Windows onto this 25 GB+ drive\nFALSE\nrecovery\nRecovery drive\nInstall Windows onto another >16 GB drive" | yad "${yadflags[@]}" --width="$(wor_yad_width 520)" \
      --list --radiolist --column=chk:CHK --column=value:HD --column=Mode --column=Description --no-headers --print-column=2 --no-selection \
      --text=$'<big><b>Installation mode</b></big>\nThis drive is large enough for either mode. Choose what you want it to do:' \
      --button='<b>Next</b>':0)"
    button=$?
    [ $button != 0 ] && exit 1

    case "$install_mode" in
      install)
        CAN_INSTALL_ON_SAME_DRIVE=1
        ;;
      recovery)
        CAN_INSTALL_ON_SAME_DRIVE=0
        ;;
    esac
  done
fi
echo "CAN_INSTALL_ON_SAME_DRIVE: $CAN_INSTALL_ON_SAME_DRIVE"
fi
}

{ #Offer to use ZRAM DL_DIR if appropriate
#if a windows ESD file will be downloaded (no point in using ram if windows is already in DL_DIR), an ISO will not be used, and DL_DIR has not already been customized
if ! is_iot_core && [ ! -f "${DL_DIR}/winfiles_from_iso_${BID}_${WIN_LANG}/alldone" ] && [ ! -f "${DL_DIR}/winfiles_${BID}_${WIN_LANG}/alldone" ] && [ -z "$SOURCE_FILE" ] && [ "$DL_DIR" == "$HOME/wor-flasher-files" ];then
  #if total usable RAM is >= 5GB
  if [ "$(awk '/MemTotal/ {print $2}' /proc/meminfo)" -ge $((5*1024*1024)) ];then
    #if kernel modules are available
    if [ -d "/lib/modules/$(uname -r)" ];then
      #tooltip text of 'Use RAM' button will explain that More RAM app from Pi-Apps will be installed, assuming it is not already installed.
      if [ -f /usr/bin/zram.sh ] && [ -d /zram ];then
        tooltip='Will use the /zram folder that was set up when you installed <b>More RAM</b> from Pi-Apps.'
      elif [ -f /usr/local/bin/pi-apps ];then
        tooltip='Will install <b>More RAM</b> from Pi-Apps and then use the new ramdisk at <u>/zram</u>.'
      else
        tooltip='Will set up a RAM-compression tool from Pi-Apps and then use the new ramdisk at <u>/zram</u>. Please note that Pi-Apps itself will not be installed.'
      fi

      yad "${yadflags[@]}" --width="$(wor_yad_width 500)" --form --field="About 4.2GB of files need to be downloaded to system storage before flashing can begin.
But your system has $(echo "scale=1 ; $( awk '/MemTotal/ {print $2}' /proc/meminfo ) / 1048576 " | bc )GB of RAM. Everything can be downloaded to RAM if you prefer.
Choose this if:
- You don't have enough space in $HOME
- You want your system storage to last as long as possible
- You don't plan to use WoR-Flasher often:LBL" \
        --image="$WOR_ASSETS_DIR/ram.png" --image-on-top \
        --button="<b>Use download folder</b>!!${DL_DIR}":2 \
        --button="<b>Use RAM</b>!!${tooltip}":0
      button=$?

      if [ "$button" == 0 ];then
        status "User chose to download everything to RAM."
        echo "For best results, please close all other programs. (especially web browsers and games)"
        yad "${yadflags[@]}" --width="$(wor_yad_width 500)" --image="$WOR_ASSETS_DIR/ram.png" --image-on-top \
          --form --field="OK! Will download everything to RAM. For best results, please close all other programs. (especially web browsers and games):LBL" \
          --button='<b>OK</b>':0 >/dev/null

        #install zram if necessary
        if [ ! -f /usr/bin/zram.sh ];then
          #install More RAM
          loading_dialog "Setting up RAM..." &
          loader_pid=$!
          trap stop_loader EXIT

          if [ -f "$HOME/pi-apps/manage" ];then
            #if Pi-Apps installed to default location, install More RAM from there
            "$HOME/pi-apps/manage" install 'More RAM'
            exitcode=$?
          elif [ -f /usr/local/bin/pi-apps ] && [ -f "$(dirname "$(cat /usr/local/bin/pi-apps | sed -n 2p)")/manage" ];then
            #if Pi-Apps installed to another folder, install More RAM from there
            "$(dirname "$(cat /usr/local/bin/pi-apps | sed -n 2p)")/manage" install 'More RAM'
            exitcode=$?
          else
            #Pi-Apps is not installed, so run More RAM script straight from the pi-apps github repo
            wget -qO- 'https://raw.githubusercontent.com/Botspot/pi-apps/master/apps/More%20RAM/install' | bash
            #either wget or bash could have failed, so check them both
            if [ ${PIPESTATUS[0]} == 0 ] && [ ${PIPESTATUS[1]} == 0 ];then
              exitcode=0
            else
              exitcode=1
            fi
          fi
          #installation complete, so close pulsating progress bar dialog
          stop_loader

          #edge case: if user had installed More RAM before and disabled the /zram folder, enable it now
          if [ "$exitcode" == 0 ] && [ ! -d /zram ];then
            sudo zram.sh storage-on
            if [ ! -d /zram ];then
              echo_red "zram.sh failed to create /zram ramdisk."
              exitcode=1
            fi
          fi

          #display warning dialog if installing More RAM failed
          if [ "$exitcode" == 0 ];then
            DL_DIR='/zram'
          else
            yad "${yadflags[@]}" --text="Failed to install 'More RAM' app from Pi-Apps.\nWoR-Flasher will not download files to RAM."
          fi
        else
          #zram already installed; now make sure /zram exists
          if [ -d /zram ];then
            DL_DIR='/zram'
          else
            sudo zram.sh storage-on
            if [ -d /zram ];then
              DL_DIR='/zram'
            else
              yad "${yadflags[@]}" --text="Failed to set up ZRAM ramdisk.\nWoR-Flasher will not download files to RAM."
            fi
          fi
        fi
      fi
    fi
  fi
fi
}

#if no user-supplied CONFIG_TXT variable, set it to initial value for yad to change later
set_default_config_txt

{ #confirmation dialog and edit config.txt

#by default, if a windows image exists, don't delete it to rebuild it
rm_img=FALSE
existing_img_chk=()

while true;do #repeat the Installation Overview window until Flash button clicked
  window_text="$(settings_summary_markup)

To continue, click Flash. To review or change these settings, click Advanced. To cancel, close this window."

  if [ "$DRY_RUN" == 1 ];then
    deletion_warning="DRY_RUN=1, so the target drive will not be modified."
    deletion_warning_2="$deletion_warning"
  else
    deletion_warning="<b>Warning!</b> All data on the target drive will be deleted!"
    deletion_warning_2="$deletion_warning Backup any files before it's too late!"
  fi

  yad "${yadflags[@]}" --width="$(wor_yad_width 720)" --height="$(wor_yad_height 700)" --image="$WOR_ASSETS_DIR/overview.png" --image-on-top \
    --form --scroll --field="$window_text":LBL '' \
    "${existing_img_chk[@]}" \
    --field="$deletion_warning":LBL '' \
    --button='<b>Advanced...</b>'!!"More settings, intended for the advanced user or for troubleshooting":2 \
    --button='<b>Flash</b>'!!"$deletion_warning_2":0 >/dev/null
  button=$?

  if [ $button == 0 ];then
    #button: Flash
    if is_iot_core;then
      gui_iot_confirm_wipe || continue
      WOR_IOT_CONFIRM_ERASE=1
    fi
    break
  elif [ $button == 2 ];then
    #button: Advanced options
      if is_iot_core;then
        linux_iot_options
        if [ "$DRY_RUN" != 1 ] && [ -z "$DEVICE" ];then
          linux_choose_flash_device
          iot_core_clear_target_approval
          linux_iot_validate_selection || exit 0
        fi
        continue
      fi

    refresh_prompt=() #this variable is populated if the Advanced Options window is repeated, to let the user know why
    save_advanced_preferences

    while true;do #repeat the advanced options window until the DL_DIR is not changed, or until Cancel is clicked
      fields=()
      release_refresh_file="$(mktemp)" || error "Could not create a release-selector refresh marker."
      release_refresh_requested=0
      config_edit_file="$(mktemp)"
      printf '%s' "$CONFIG_TXT" > "$config_edit_file"
      config_editor_title="$WOR_WINDOW_TITLE | config.txt"
      config_editor_action="@yad --center --no-markup --width=$(wor_yad_width 700) --height=$(wor_yad_height 520) --title=$(printf '%q' "$config_editor_title") --window-icon=$(printf '%q' "$WOR_LOGO_PATH") --class=$(printf '%q' "$WOR_ICON_NAME") --text-info --editable --in-place --confirm-save='Save config.txt changes?' --filename=$(printf '%q' "$config_edit_file") --button=Close:0 >/dev/null"
      uefi_pinned="$(uefi_pinned_version)"
      prepare_release_choices uefi "$uefi_pinned"
      uefi_versions="$RELEASE_CHOICES" uefi_version_warning="$RELEASE_CHOICES_WARNING"
      uefi_dropdown_default="$RELEASE_CHOICES_DEFAULT" uefi_version_labels="$RELEASE_CHOICES_LABELS" uefi_latest_tag="$RELEASE_CHOICES_LATEST"
      driver_versions='' driver_version_warning='' driver_version_labels='' driver_latest_tag='' driver_dropdown_default="$DRIVER_VER"
      if advanced_option_applies drivers;then
        prepare_release_choices drivers "$DRIVER_VER"
        driver_versions="$RELEASE_CHOICES" driver_version_warning="$RELEASE_CHOICES_WARNING"
        driver_dropdown_default="$RELEASE_CHOICES_DEFAULT" driver_version_labels="$RELEASE_CHOICES_LABELS" driver_latest_tag="$RELEASE_CHOICES_LATEST"
      fi
      windows_family="$(windows_version_label)"
      #make entries for the customization toggles
      oobe_field='' pi4_field='' drivers_field='' language_field='' uefi_version_field='' driver_version_field=''
      if advanced_option_applies oobe || advanced_option_applies pi4;then
        fields+=("--field=$(wor_setup_scope "$windows_family" "$CAN_INSTALL_ON_SAME_DRIVE"):LBL" '')
      fi
      if advanced_option_applies oobe;then
        oobe_field=$((${#fields[@]} / 2 + 1))
        fields+=("--field=$(wor_yad_label "$(wor_advanced_label oobe "$uefi_pinned" "$DRIVER_VER" "$RPI_MODEL" "$windows_family" "$CAN_INSTALL_ON_SAME_DRIVE")" "$(wor_advanced_caution oobe)")":CHK "$(wor_yad_bool "$OOBE_NETWORK_BYPASS")")
      fi
      if advanced_option_applies pi4;then
        pi4_field=$((${#fields[@]} / 2 + 1))
        fields+=("--field=$(wor_pi4_label):CHK" "$(wor_yad_bool "$PI4_AUTO_DISABLE_3GB")")
      fi
      fields+=("--field=Firmware and drivers:LBL" '')
      uefi_field=$((${#fields[@]} / 2 + 1))
      fields+=("--field=$(wor_yad_label "$(wor_advanced_label uefi "$uefi_pinned" "$DRIVER_VER" "$RPI_MODEL")" "$(wor_advanced_caution uefi "$RPI_MODEL")" "$(wor_advanced_recommended uefi "$RPI_MODEL")")":CHK "$(wor_yad_bool "$(uefi_use_latest)")")
      if [ "$(uefi_use_latest)" != 1 ];then
        uefi_version_field=$((${#fields[@]} / 2 + 1))
        fields+=("--field=UEFI version:CB" "$(printf '%s\n' "$uefi_version_labels" | awk 'NF {v=v ? v "!" $0 : $0} END {print v}')")
        [ -z "$uefi_version_warning" ] || fields+=("--field=$uefi_version_warning:LBL" '')
      fi
      if advanced_option_applies drivers;then
        drivers_field=$((${#fields[@]} / 2 + 1))
        fields+=("--field=$(wor_yad_label "$(wor_advanced_label drivers "$uefi_pinned" "$DRIVER_VER" "$RPI_MODEL")" "$(wor_advanced_caution drivers)" "$(wor_advanced_recommended drivers "$RPI_MODEL")")":CHK "$(wor_yad_bool "$DRIVERS_USE_LATEST")")
        if [ "$DRIVERS_USE_LATEST" != 1 ];then
          driver_version_field=$((${#fields[@]} / 2 + 1))
          fields+=("--field=Driver version:CB" "$(printf '%s\n' "$driver_version_labels" | awk 'NF {v=v ? v "!" $0 : $0} END {print v}')")
          [ -z "$driver_version_warning" ] || fields+=("--field=$driver_version_warning:LBL" '')
        fi
      fi
      fields+=("--field=Validation:LBL" '')
      verify_field=$((${#fields[@]} / 2 + 1))
      fields+=("--field=$(wor_yad_label "$(wor_advanced_label verify "$uefi_pinned" "$DRIVER_VER" "$RPI_MODEL" "$windows_family" "$CAN_INSTALL_ON_SAME_DRIVE")" "$(wor_advanced_caution verify)")":CHK "$(wor_yad_bool "$SKIP_IMAGE_VERIFICATION")")
      dryrun_field=$((${#fields[@]} / 2 + 1))
      fields+=("--field=$(wor_advanced_label dryrun "$uefi_pinned" "$DRIVER_VER" "$RPI_MODEL"):CHK" "$(wor_yad_bool "$DRY_RUN")")
      #in recovery mode this config.txt boots the installer media; WoR-PE writes the target drive's own copy
      config_scope="$(wor_config_scope "$CAN_INSTALL_ON_SAME_DRIVE")"
      fields+=("--field=Downloads:LBL" '')
      #make entry to change DL_DIR
      if [ -f "${DL_DIR}/winfiles_from_iso_${BID}_${WIN_LANG}/alldone" ];then
        working_dir_field=$((${#fields[@]} / 2 + 1))
        fields+=("--field=Download folder:RO" 'Cannot be changed')
      else
        working_dir_field=$((${#fields[@]} / 2 + 1))
        fields+=("--field=Download folder:DIR" "$DL_DIR")
      fi
      if [ -f "${DL_DIR}/winfiles_${BID}_${WIN_LANG}/alldone" ];then
        windows_files_status="$windows_family image already extracted and ready to use"
      elif [ -f "${DL_DIR}/winfiles_from_iso_${BID}_${WIN_LANG}/alldone" ];then
        windows_files_status="$windows_family ISO files already extracted and ready to use"
      elif [ -n "$SOURCE_FILE" ];then
        windows_files_status="$windows_family files will be extracted from the selected ISO"
      else
        windows_files_status="Will download and extract the $windows_family image"
      fi
      windows_files_field=$((${#fields[@]} / 2 + 1))
      fields+=("--field=Windows files:RO" "$windows_files_status")
      #USE_CACHE has three values, so it needs a combo rather than a check box; the selected item comes first
      case "$USE_CACHE" in
        0) cache_items='Re-download everything, ignoring the cache!Reuse cached files when they still match (recommended)!Trust the cache without checking it' ;;
        2) cache_items='Trust the cache without checking it!Re-download everything, ignoring the cache!Reuse cached files when they still match (recommended)' ;;
        *) cache_items='Reuse cached files when they still match (recommended)!Re-download everything, ignoring the cache!Trust the cache without checking it' ;;
      esac
      cache_field=$((${#fields[@]} / 2 + 1))
      fields+=("--field=Downloaded files":CB "$cache_items")
      fields+=("--field=Notifications:LBL" '')
      sound_items=""
      curr_sound_item=""
      other_sound_items=""
      sel_sound="$(wor_completion_sound)"
      while IFS=$'\t' read -r sound_value sound_label ;do
        [ -z "$sound_value" ] && continue
        if [ "$sound_value" == "$sel_sound" ];then curr_sound_item="$sound_label"; else [ -n "$other_sound_items" ] && other_sound_items+="!${sound_label}" || other_sound_items="${sound_label}"; fi
      done < <(wor_sound_options)
      if [ -n "$curr_sound_item" ] || [ -n "$other_sound_items" ];then
        [ -n "$curr_sound_item" ] && sound_items="${curr_sound_item}!${other_sound_items}" || sound_items="${other_sound_items}"
        play_sound_field=$((${#fields[@]} / 2 + 1))
        fields+=("--field=Play a sound when the flash finishes":CHK "$(wor_yad_bool "${PLAY_SOUND:-1}")")
        completion_sound_field=$((${#fields[@]} / 2 + 1))
        fields+=("--field=Completion sound":CB "$sound_items")
      fi
      notification_field=$((${#fields[@]} / 2 + 1))
      fields+=("--field=Show a completion notification":CHK "$(wor_yad_bool "${SHOW_NOTIFICATION:-1}")")
      fields+=("--field=Windows account:LBL" '')
      account_checkbox_field=$((${#fields[@]} / 2 + 1))
      fields+=("--field=Create an optional local Windows administrator account":CHK "$(wor_yad_bool "$WINDOWS_ACCOUNT_SETUP")")
      account_username_field=$((${#fields[@]} / 2 + 1))
      account_username_value="$WINDOWS_ACCOUNT_USERNAME"
      account_password_value="$WINDOWS_ACCOUNT_PASSWORD"
      [ "$WINDOWS_ACCOUNT_SETUP" == 1 ] || account_username_value='@disabled@'
      [ "$WINDOWS_ACCOUNT_SETUP" == 1 ] || account_password_value='@disabled@'
      fields+=("--field=Windows username":TXT "$account_username_value")
      account_password_field=$((${#fields[@]} / 2 + 1))
      fields+=("--field=Windows password":H "$account_password_value")
      fields+=("--field=Regional settings:LBL" '')
      locale_checkbox_field=$((${#fields[@]} / 2 + 1))
      fields+=("--field=Configure Windows keyboard and regional settings":CHK "$(wor_yad_bool "$WINDOWS_LOCALE_SETUP")")
      locale_items=""
      curr_locale_item=""
      other_locale_items=""
      while IFS=$'\t' read -r locale_code locale_label ;do
        [ -z "$locale_code" ] && continue
        entry="${locale_code}: ${locale_label:-$locale_code}"
        if [ "$(printf '%s' "$locale_code" | tr '[:upper:]' '[:lower:]')" == "$(printf '%s' "$WINDOWS_LOCALE" | tr '[:upper:]' '[:lower:]')" ];then
          curr_locale_item="$entry"
        else
          [ -n "$other_locale_items" ] && other_locale_items+="!${entry}" || other_locale_items="${entry}"
        fi
      done < <(list_windows_locale_options)
      [ -n "$curr_locale_item" ] && locale_items="${curr_locale_item}!${other_locale_items}" || locale_items="${WINDOWS_LOCALE}: ${WINDOWS_LOCALE}!${other_locale_items}"
      locale_value="$locale_items"
      [ "$WINDOWS_LOCALE_SETUP" == 1 ] || locale_value='@disabled@'
      locale_field=$((${#fields[@]} / 2 + 1))
      fields+=("--field=Windows locale":CB "$locale_value")
      if advanced_option_applies language;then
        lang_items=""
        curr_item=""
        other_items=""
        while IFS=: read -r l_code l_name ;do
          [ -z "$l_code" ] && continue
          entry="${l_code}: ${l_name}"
          if [ "$l_code" == "$WIN_LANG" ];then
            curr_item="$entry"
          else
            [ -n "$other_items" ] && other_items+="!${entry}" || other_items="${entry}"
          fi
        done < <(list_langs_preferred)
        [ -n "$curr_item" ] && lang_items="${curr_item}!${other_items}" || lang_items="${other_items}"
        language_field=$((${#fields[@]} / 2 + 1))
        fields+=("--field=Choose Windows language":CB "$lang_items")
      fi
      fields+=("--field=Raspberry Pi boot config:LBL" '')
      config_button_value="$config_editor_action"
      [ "$APPLY_CUSTOM_CONFIG_TXT" == 1 ] || config_button_value='@disabled@'
      account_username_update="${account_username_field}:$(printf '%q' "$WINDOWS_ACCOUNT_USERNAME")"
      account_password_update="${account_password_field}:$(printf '%q' "$WINDOWS_ACCOUNT_PASSWORD")"
      locale_update="${locale_field}:$(printf '%q' "$locale_items")"
      config_checkbox_field=$((${#fields[@]} / 2 + 1))
      fields+=("--field=$(wor_config_txt_label "$config_scope") (recommended):CHK" "$(wor_yad_bool "$APPLY_CUSTOM_CONFIG_TXT")")
      config_button_field=$((${#fields[@]} / 2 + 1))
      fields+=("--field=<b>View / Edit config.txt</b>:BTN" "$config_button_value")
      release_toggle_fields="$uefi_field${drivers_field:+|$drivers_field}"
      changed_action="case \"\$1\" in
        ${release_toggle_fields}) printf 'refresh\n' > $(printf '%q' "$release_refresh_file"); kill -USR1 \"\$YAD_PID\";;
        ${account_checkbox_field}) if [ \"\$2\" == TRUE ];then printf '%s\\n' \"${account_username_update}\" \"${account_password_update}\"; else printf \"${account_username_field}:@disabled@\\n${account_password_field}:@disabled@\\n\"; fi;;
        ${locale_checkbox_field}) if [ \"\$2\" == TRUE ];then printf '%s\\n' \"${locale_update}\"; else printf \"${locale_field}:@disabled@\\n\"; fi;;
        ${config_checkbox_field}) if [ \"\$2\" == TRUE ];then printf '%s\\n' \"${config_button_field}:${config_editor_action}\"; else printf '${config_button_field}:@disabled@\\n'; fi;;
      esac"

      advanced_text=$'<big><b>Advanced Options</b></big>\n'"$(wor_advanced_context "$windows_family" "$CAN_INSTALL_ON_SAME_DRIVE")"
      [ "${#refresh_prompt[@]}" == 0 ] || advanced_text="${refresh_prompt[0]#--text=}"$'\n\n'"$advanced_text"
      output="$(GTK_OVERLAY_SCROLLING=0 yad "${yadflags[@]}" --response=0 --use-markup --changed-action="$changed_action" --width="$(wor_yad_width 720)" --height="$(wor_yad_height "$WOR_YAD_SCREEN_HEIGHT")" --image-on-top \
        --text="$advanced_text" \
        --form --scroll --vscroll-policy=auto \
        "${fields[@]}" \
        --button="<b>Back</b>":1 --button="<b>OK</b>":0
      )"
      button=$?
      [ ! -s "$release_refresh_file" ] || release_refresh_requested=1
      rm -f "$release_refresh_file"
      [ -f "$config_edit_file" ] && CONFIG_TXT="$(cat "$config_edit_file")"
      rm -f "$config_edit_file"

      if [ "$button" == 0 ];then #everything in this if statement is skipped if Cancel is clicked
        yad_field_value() { printf '%s\n' "$output" | sed -n "${1}p"; }
        selected_uefi="$uefi_dropdown_default" selected_driver="$driver_dropdown_default"
        if { [ -n "$uefi_version_field" ] && ! selected_uefi="$(release_choice_tag "$(yad_field_value "$uefi_version_field")" "$uefi_versions" "$uefi_latest_tag")"; } \
          || { [ -n "$driver_version_field" ] && ! selected_driver="$(release_choice_tag "$(yad_field_value "$driver_version_field")" "$driver_versions" "$driver_latest_tag")"; };then
          warning "Select a version from the published choices before continuing."
          refresh_prompt=("--text=Select an available UEFI/driver version or enable latest.")
          continue
        fi
        if [ ! -f "${DL_DIR}/winfiles_from_iso_${BID}_${WIN_LANG}/alldone" ] && [ "$DL_DIR" != "$(yad_field_value "$working_dir_field")" ];then
          #DL_DIR was changed - only honor the value if it is allowed to be changed
          DL_DIR="$(yad_field_value "$working_dir_field")"
          echo "In the Advanced Options window, user changed DL_DIR to $DL_DIR"

          #explain to user why the Advanced Options window was refreshed when they clicked OK
          refresh_prompt=("--text=<b>Note:</b> As you changed the working directory, this window has refreshed."$'\n'"Any previous checkbox values have been ignored.")

          #skipping the 'break' command to repeat the Advanced Options window

        else #if DL_DIR was not changed, then review the subsequent check-box values
          #peinstaller
          #DRY_RUN
          if [ "$(yad_field_value "$dryrun_field")" == TRUE ] && [ "$DRY_RUN" == 0 ];then
            echo "User checked the box to set DRY_RUN=1"
            DRY_RUN=1
          elif [ "$(yad_field_value "$dryrun_field")" == FALSE ] && [ "$DRY_RUN" == 1 ];then
            echo "User checked the box to set DRY_RUN=0"
            DRY_RUN=0
          fi
          #customization toggles
          if [ -n "$oobe_field" ];then
            [ "$(yad_field_value "$oobe_field")" == TRUE ] && OOBE_NETWORK_BYPASS=1 || OOBE_NETWORK_BYPASS=0
          fi
          #keep the existing preference when the toggle wasn't applicable, so switching back to a Pi 4 doesn't lose it
          if [ -n "$pi4_field" ];then
            [ "$(yad_field_value "$pi4_field")" == TRUE ] && PI4_AUTO_DISABLE_3GB=1 || PI4_AUTO_DISABLE_3GB=0
          fi
          if [ "$(yad_field_value "$uefi_field")" == TRUE ];then
            set_uefi_use_latest_choice 1
          else
            set_uefi_use_latest_choice 0
          fi
          if [ -n "$drivers_field" ];then
            [ "$(yad_field_value "$drivers_field")" == TRUE ] && DRIVERS_USE_LATEST=1 || DRIVERS_USE_LATEST=0
          fi
          if [ -n "$uefi_version_field" ] || [ "$(uefi_use_latest)" != 1 ];then
            set_selected_release_version uefi "$selected_uefi" || error "Invalid UEFI release selection."
          fi
          if [ -n "$drivers_field" ] && { [ -n "$driver_version_field" ] || [ "$DRIVERS_USE_LATEST" != 1 ]; };then
            set_selected_release_version drivers "$selected_driver" || error "Invalid driver release selection."
          fi
          [ "$(yad_field_value "$verify_field")" == TRUE ] && SKIP_IMAGE_VERIFICATION=1 || SKIP_IMAGE_VERIFICATION=0
          [ "$(yad_field_value "$config_checkbox_field")" == TRUE ] && APPLY_CUSTOM_CONFIG_TXT=1 || APPLY_CUSTOM_CONFIG_TXT=0
          case "$(yad_field_value "$cache_field")" in
            'Re-download everything'*) USE_CACHE=0 ;;
            'Trust the cache'*) USE_CACHE=2 ;;
            'Reuse cached files'*) USE_CACHE=1 ;;
          esac
          [ "$(yad_field_value "$account_checkbox_field")" == TRUE ] && WINDOWS_ACCOUNT_SETUP=1 || WINDOWS_ACCOUNT_SETUP=0
          if [ "$WINDOWS_ACCOUNT_SETUP" == 1 ];then
            WINDOWS_ACCOUNT_USERNAME="$(yad_field_value "$account_username_field")"
            WINDOWS_ACCOUNT_PASSWORD="$(yad_field_value "$account_password_field")"
          fi
          [ "$(yad_field_value "$locale_checkbox_field")" == TRUE ] && WINDOWS_LOCALE_SETUP=1 || WINDOWS_LOCALE_SETUP=0
          [ "$WINDOWS_LOCALE_SETUP" != 1 ] || WINDOWS_LOCALE="$(yad_field_value "$locale_field" | awk -F': ' '{print $1}')"
          sel_lang=''
          [ -z "$language_field" ] || sel_lang="$(yad_field_value "$language_field")"
          sel_code="${sel_lang%%:*}"
          if [ -n "$language_field" ] && is_known_win_lang "$sel_code" ;then
            WIN_LANG="$sel_code"
          fi
          if [ -n "$sound_items" ];then
            [ "$(yad_field_value "$play_sound_field")" == TRUE ] && PLAY_SOUND=1 || PLAY_SOUND=0
            #the combo shows labels, so map the chosen one back to the value the player needs
            sel_sound_label="$(yad_field_value "$completion_sound_field")"
            sel_sound="$(wor_sound_options | awk -F'\t' -v l="$sel_sound_label" '$2 == l {print $1; exit}')"
            [ -n "$sel_sound" ] && COMPLETION_SOUND="$sel_sound"
          fi
          [ "$(yad_field_value "$notification_field")" == TRUE ] && SHOW_NOTIFICATION=1 || SHOW_NOTIFICATION=0
          #end of parsing check-box values for advanced options window

          if [ "$release_refresh_requested" == 1 ];then
            refresh_prompt=()
            continue
          fi
          break #as the DL_DIR value was not changed, go back to the Installation Overview window
        fi

      else #button != OK
        restore_advanced_preferences
        break #Don't save and go back to Installation Overview
      fi
    done #end of repeating the advanced options window
    unset ADVANCED_ORIGINAL_VALUES

  else
    #User exited when reviewing information and customizing config.txt
    exit 1
  fi

done #end of repeating the Installation Overview window

#if user checked the box to rebuild the image, delete the image now
if [ "$rm_img" == TRUE ];then
  echo "User checked the box to delete the pre-existing windows image."
  rm -f "$DL_DIR/uupdump"/*ARM64*.ISO
fi

#display multi-line CONFIG_TXT variable
echo -e "CONFIG_TXT: ⤵\n$(echo "$CONFIG_TXT" | sed 's/^/  > /g')\nCONFIG_TXT: ⤴\n"
}

echo "Running install-wor.sh"

abort_marker="$(mktemp -u)"
#Ubuntu must show progress immediately: package-manager and host preflight work can occur before
#the child reaches its authentication marker, and waiting there makes the GUI look hung at 0%.
if ! is_macos;then
  export GUI_PROGRESS_EARLY=1
fi
gui_start_installer

progress_fifo="$(mktemp -u)"
mkfifo "$progress_fifo"
tail -n +1 -F "$progress_file" > "$progress_fifo" 2>/dev/null &
tail_pid=$!
# LINUX_PROGRESS_AWK_BEGIN
awk -F'\t' '
  #pct, not sub: sub() is a built-in awk function and cannot be used as a variable
  #before the first STEP (e.g. while clearing the cache) the percentage stands on its own
  function overall() {
    msg=title;
    if (pct > 0 && task_title != "") msg=task_title;
    if (total+0 > 0) {
      printf("1:%d\n", ((step-1)*100 + pct) / (total*100) * 100);
      if (pct > 0) {
        printf("1:# Overall - Step %d/%d: %s (%d%%)\n", step, total, msg, pct);
      } else {
        printf("1:# Overall - Step %d/%d: %s\n", step, total, title);
      }
    } else {
      printf("1:%d\n", pct+0);
      printf("1:# Overall: %s (%d%%)\n", status_msg, pct);
    }
    printf("2:%d\n", pct+0);
    printf("2:# Sub-progress: %s (%d%%)\n", msg, pct);
  }
  /^STEP/    { step=$2+0; total=$3+0; title=$4; pct=0; task_title=""; overall(); fflush() }
  /^SUBSTEP/ { pct=$2+0; if (pct<0) pct=0; if (pct>100) pct=100; overall(); fflush() }
  /^TASK/    { pct=$2+0; if (pct<0) pct=0; if (pct>100) pct=100; task_title=$3; overall(); fflush() }
  /^STATUS/  { status_msg=$2; if (step+0 == 0) overall(); else printf("1:# %s\n", status_msg); fflush() }
# LINUX_PROGRESS_AWK_END
' < "$progress_fifo" |
  yad "${yadflags[@]}" --width="$(wor_yad_width 680)" --height="$(wor_yad_height 330)" \
  --progress --image="$WOR_LOGO_PATH" --text=$'<big><b>Preparing flash...</b></big>\nRunning setup and preflight checks. The bars will move when WoR-Flasher reaches a measured step.' --bar="Overall:NORM" --bar="Sub-progress:NORM" --button='<b>Abort</b>':1 &
yad_pid=$!

progress_aborted=0
while [ ! -f "$done_marker" ];do
  if ! kill -0 "$yad_pid" 2>/dev/null ;then
    progress_aborted=1
    break
  fi
  sleep 0.3
done

if [ "$progress_aborted" == 1 ];then
  touch "$abort_marker"
  status "Aborting at your request"
  kill_process_tree "$installer_pid"
  wait "$installer_pid" 2>/dev/null
  kill "$tail_pid" 2>/dev/null
  wait "$tail_pid" 2>/dev/null
  rm -f "$progress_fifo" "$progress_file" "$done_marker" "$auth_marker" "$abort_marker" "$error_marker"
  saved_log="$(gui_save_installer_log)"
  wor_play_result_sound failure
  wor_show_result_notification failure "$(windows_version_label)"
  yad "${yadflags[@]}" --text="Preparation or flashing was stopped before it finished.\n\n$(windows_version_label) media was not completed.\n\nIf disk writing had begun, $DEVICE may be incomplete. Do not boot an unverified image.\n\nFull log: $saved_log"
  exit 1
fi

exitcode="$(cat "$done_marker" 2>/dev/null)"
[ -z "$exitcode" ] && exitcode=1

kill "$tail_pid" "$yad_pid" 2>/dev/null
wait "$tail_pid" "$yad_pid" 2>/dev/null
rm -f "$progress_fifo" "$progress_file" "$done_marker" "$auth_marker" "$abort_marker"

#clear zram - avoid leaving files occupying space in /zram
if [ "$DL_DIR" == /zram ];then
  sudo zram.sh &>/dev/null
fi

if [ "$exitcode" == 0 ];then
  rm -f "$output_log" "$error_marker"
  wor_play_result_sound success
  wor_show_result_notification success "$(windows_version_label)"
  #display "next steps" window
  linux_completion_image="$(wor_yad_image_for_screen "$WOR_ASSETS_DIR/next-steps.png" "$WOR_LOGO_PATH" 730 440)"
  linux_completion_text="$(windows_version_label) media preparation is complete."
  if is_iot_core;then
    linux_completion_image="$WOR_LOGO_PATH"
    linux_completion_text="$(iot_core_next_steps)"
  fi
  linux_completion_connect=0
  if is_iot_core && iot_core_personalization_requested && [ "$DRY_RUN" != 1 ];then
    linux_completion_connect=1
  fi
  if linux_show_completion_dialog "$linux_completion_text" "$linux_completion_image" "$linux_completion_connect";then
    if [ "$linux_completion_connect" == 1 ] && [ "$GUI_RESULT_ACTION" == connect ];then
      gui_iot_account_setup || exit 1
    fi
  fi
else
  #keep the log on failure; the dialog only shows a tail, and the GUI has no terminal to fall back on
  saved_log="$(gui_save_installer_log)"
  wor_play_result_sound failure
  wor_show_result_notification failure "$(windows_version_label)"
  if installer_showed_own_error ;then
    : #install-wor.sh already displayed its own native error dialog.
  else
    yad "${yadflags[@]}" --text="The $(windows_version_label) media preparation stopped unexpectedly (exit code $exitcode).\n\n$(gui_log_tail "$saved_log")\n\nFull log: $saved_log"
  fi
  rm -f "$error_marker"
fi

echo "install-wor.sh has finished."

#if downloading to ram, empty it now
if [ "$DL_DIR" == /zram ] && [ -d /zram/peinstaller ];then
  sudo zram.sh &>/dev/null
fi
