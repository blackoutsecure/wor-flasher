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

#Source-checkout updates are enabled by default. Disable them explicitly for every test
#invocation, including sourced library functions. The self-updater test further down
#deliberately re-enables updates against a disposable clone, never against this working tree.
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
  for f in src/lib/metadata.sh src/lib/dependencies.sh src/lib/paths.sh src/lib/cleanup.sh src/lib/gui.sh install-wor.sh install-wor-gui.sh install-wor-hook.sh 'src/macos-app/Contents/MacOS/WoR-Flasher' ;do
    bash -n "$REPO_DIR/$f" 2>/dev/null && pass "$f parses" || fail "$f has a syntax error"
  done
  jq empty "$REPO_DIR/src/config/metadata.json" >/dev/null 2>&1 \
    && jq empty "$REPO_DIR/src/config/metadata.schema.json" >/dev/null 2>&1 \
    && pass "project metadata JSON and schema parse" \
    || fail "project metadata JSON or schema is invalid"
  grep -qF 'readRuntimePaths' "$REPO_DIR/src/build-release.mjs" \
    && grep -qF '"runtimePaths"' "$REPO_DIR/src/config/metadata.json" \
    && grep -qF '"pe:check": "node src/check-pe-installer.mjs --check"' "$REPO_DIR/package.json" \
    && grep -qF '"pe:update": "node src/check-pe-installer.mjs --write-latest"' "$REPO_DIR/package.json" \
    && grep -qF 'npm run package:all' "$REPO_DIR/.github/workflows/release.yml" \
    && grep -qF 'release/linux/wor-flasher' "$REPO_DIR/.github/workflows/release.yml" \
    && grep -qF 'release/macos/WoR-Flasher.app' "$REPO_DIR/.github/workflows/release.yml" \
    && pass "release tooling shares the project metadata runtime path manifest" \
    || fail "release tooling duplicates the runtime path list"
  release_package_line="$(grep -nF '      - name: Package release artifacts' "$REPO_DIR/.github/workflows/release.yml" | cut -d: -f1)"
  release_tag_line="$(grep -nF '      - name: Create immutable release tag' "$REPO_DIR/.github/workflows/release.yml" | cut -d: -f1)"
  release_publish_line="$(grep -nF '      - name: Publish GitHub Release' "$REPO_DIR/.github/workflows/release.yml" | cut -d: -f1)"
  if [ -n "$release_package_line" ] && [ -n "$release_tag_line" ] && [ -n "$release_publish_line" ] \
    && [ "$release_package_line" -lt "$release_tag_line" ] && [ "$release_tag_line" -lt "$release_publish_line" ] ;then
    pass "release artifacts are packaged before the immutable tag is pushed"
  else
    fail "release workflow mutates the repository before packaging succeeds"
  fi
  if command -v node >/dev/null ;then
    node --check "$REPO_DIR/src/build-release.mjs" >/dev/null 2>&1 \
    && node --check "$REPO_DIR/src/check-pe-installer.mjs" >/dev/null 2>&1 \
    && node --check "$REPO_DIR/src/sync-package-metadata.mjs" >/dev/null 2>&1 \
    && node --check "$REPO_DIR/src/updater.mjs" >/dev/null 2>&1 \
    && node --check "$REPO_DIR/src/package-macos-app.mjs" >/dev/null 2>&1 \
    && node --check "$REPO_DIR/src/set-version.mjs" >/dev/null 2>&1 \
    && node --check "$REPO_DIR/src/lib/node-runtime.mjs" >/dev/null 2>&1 \
    && pass "src/*.mjs and shared Node library scripts parse cleanly" || fail "Node tooling scripts have syntax errors"
    if node --test "$REPO_DIR/tests/node-tools.test.mjs" >/dev/null 2>&1 ;then
      pass "Node.js unit test suite passed (tests/node-tools.test.mjs)"
    else
      fail "Node.js unit test suite failed"
    fi
    release_check_tmp="$(mktemp -d "${TMPDIR:-/tmp}/wor-release-check-test.XXXXXX")"
    if TMPDIR="$release_check_tmp" node "$REPO_DIR/src/build-release.mjs" --check --platform=invalid >/dev/null 2>&1 ;then
      fail "release tooling accepted an unsupported platform"
    elif find "$release_check_tmp" -mindepth 1 -print -quit | grep -q . ;then
      fail "failed release check left temporary staging files behind"
    else
      pass "failed release checks reject invalid input and clean temporary staging"
    fi
    rm -rf "$release_check_tmp"
  else
    skip "node is not installed; skipping release-tool syntax check"
  fi

  if command -v shellcheck >/dev/null ;then
    shellcheck --severity=error "$REPO_DIR"/src/lib/metadata.sh "$REPO_DIR"/src/lib/dependencies.sh "$REPO_DIR"/src/lib/paths.sh "$REPO_DIR"/src/lib/cleanup.sh "$REPO_DIR"/src/lib/gui.sh "$REPO_DIR"/install-wor.sh "$REPO_DIR"/install-wor-gui.sh "$REPO_DIR"/install-wor-hook.sh "$REPO_DIR"/src/macos-app/Contents/MacOS/WoR-Flasher >/dev/null 2>&1 \
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
    && grep -qF 'diskutil unmount force "$mountpoint"' "$REPO_DIR/src/lib/cleanup.sh" \
    && ! grep -qF 'sudo diskutil unmount force "$mountpoint"' "$REPO_DIR/src/lib/cleanup.sh" \
    && pass "all temporary mounts use the shared cleanup handler" \
    || fail "a temporary mount bypasses the shared cleanup handler"

  grep -qF 'if [ "$RUN_MODE" != gui ] && ! command sudo -n -v >/dev/null 2>&1 && ! sudo -v >/dev/null 2>&1;then' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'Administrator authentication failed or was canceled.' "$REPO_DIR/install-wor.sh" \
    && pass "macOS authenticates administrator access before partitioning" \
    || fail "macOS does not authenticate administrator access before partitioning"

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

  grep -qF 'WOR_MACOS_BREW_FORMULAE=(aria2 cabextract jq wget wimlib gptfdisk pv)' "$REPO_DIR/src/lib/dependencies.sh" \
    && grep -qF 'WOR_LINUX_PACKAGES=(yad aria2 cabextract wimtools chntpw genisoimage exfat-fuse wget udftools bc parted dosfstools unzip git pv)' "$REPO_DIR/src/lib/dependencies.sh" \
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

  esd_delete_block="$(sed -n '/#Remove first 3 partitions from ESD file/,/mv -f .*install.wim/p' "$REPO_DIR/install-wor.sh")"
  [ "$(printf '%s\n' "$esd_delete_block" | grep -cF 'wimdelete "$SOURCE_FILE" 1 --soft')" == 2 ] \
    && printf '%s\n' "$esd_delete_block" | grep -qF 'wimdelete "$SOURCE_FILE" 1 || error' \
    && pass "ESD conversion compacts the image after two soft index deletions" \
    || fail "ESD conversion does not compact the image on its final index deletion"

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
    && grep -qF 'To continue, WoR-Flasher needs administrator access for disk preparation, formatting, and flashing.' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'Target: " & targetDevice' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'Only continue if this is the drive you intend to erase.' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'WOR_FLASH_TARGET="$DEVICE" WOR_METADATA_FILE="$WOR_METADATA_FILE" WOR_APP_TITLE="$WOR_APP_TITLE" WOR_WINDOW_TITLE="$WOR_WINDOW_TITLE" SUDO_ASKPASS="$MACOS_ASKPASS" command sudo -A "$@"' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'SUDO_ASKPASS="$MACOS_ASKPASS" command sudo -A "$@"' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'wor_osascript - "$WOR_WINDOW_TITLE" "$WOR_FLASH_TARGET"' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'with title windowTitle' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'export RUN_MODE=gui' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'exec /usr/bin/open -W "$DIRECTORY/release/macos/WoR-Flasher.app"' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF '[ "${WOR_USE_PACKAGED_APP:-0}" == 1 ]' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF '[ "${WOR_NATIVE_APP:-0}" != 1 ]' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'export WOR_NATIVE_APP=1' "$REPO_DIR/src/macos-app/Contents/MacOS/WoR-Flasher" \
    && grep -qF "name: 'WorErrorController'" "$REPO_DIR/install-wor.sh" \
    && grep -qF 'app.requestUserAttention($.NSInformationalRequest)' "$REPO_DIR/install-wor.sh" \
    && ! grep -qF 'display alert' "$REPO_DIR/install-wor.sh" \
    && pass "macOS GUI shows a native password dialog instead of a terminal prompt" \
    || fail "macOS GUI does not use a native password dialog"

  cli_intro_block="$(sed -n '/^cli_intro() {/,/^}/p' "$REPO_DIR/install-wor.sh")"
  grep -qF '"$WOR_FLASHER_NAME"' <<< "$cli_intro_block" \
    && (cd "$REPO_DIR" && ./install-wor.sh --help | head -n1 | grep -qF 'WoR-Flasher') \
    && ! grep -qiE 'web.?watcher' <<< "$cli_intro_block" \
    && [ "$(sed -n "/<<'BANNER'/,/^BANNER$/p" <<< "$cli_intro_block" | awk '{ if (length > max) max = length } END { print max }')" -le 79 ] \
    && pass "CLI ASCII banner names WoR-Flasher" \
    || fail "CLI ASCII banner does not name WoR-Flasher"

  erase_line="$(grep -anF '/usr/sbin/diskutil eraseVolume MS-DOS WOR_BOOT "$part1"' "$REPO_DIR/install-wor.sh" | tail -n1 | cut -d: -f1)"
  type_line="$(grep -anF '"$sgdisk_bin" -t 1:ef00 -c 1:WOR_BOOT -t 2:0700 -c 2:WOR_INSTALL "$raw_device"' "$REPO_DIR/install-wor.sh" | tail -n1 | cut -d: -f1)"
  verify_line="$(grep -anF 'verify_written_image "$DEVICE" "$PART1" "$PART2" "$boot_mount" "$win_mount"' "$REPO_DIR/install-wor.sh" | tail -n1 | cut -d: -f1)"
  finalize_call_line="$(grep -anF 'darwin_finalize_partition_types_or_die "$DEVICE" "$sgdisk_bin"' "$REPO_DIR/install-wor.sh" | tail -n1 | cut -d: -f1)"
  final_verify_line="$(grep -anF 'darwin_verify_final_partition_types_or_die "$PART1" "$PART2"' "$REPO_DIR/install-wor.sh" | tail -n1 | cut -d: -f1)"
  [ -n "$erase_line" ] && [ -n "$type_line" ] && [ -n "$verify_line" ] && [ -n "$finalize_call_line" ] && [ -n "$final_verify_line" ] \
    && [ "$erase_line" -lt "$type_line" ] && [ "$verify_line" -lt "$finalize_call_line" ] && [ "$finalize_call_line" -lt "$final_verify_line" ] \
    && pass "macOS restores the EFI GPT type after copying and verifying the mounted files" \
    || fail "macOS retags WOR_BOOT too early, or leaves it as Microsoft Basic Data"

  #shellcheck disable=SC1091
  source "$TEST_SCRIPT_DIR/run-tests-macos.sh"
  if [ "$(uname -s)" == Darwin ];then
    macos_app_and_launcher_checks
  else
    for macos_only_name in "${MACOS_ONLY_TEST_NAMES[@]}" ;do
      skip "$macos_only_name (requires macOS; detected host is $(uname -s))"
    done
  fi

  grep -qF 'macos_start_cli()' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'WOR_GUI_INSTANCE_DIR="${TMPDIR:-/tmp}/wor-flasher-gui-${UID}.lock"' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'kill -0 "$owner_pid"' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF '[[ "$owner_command" == *install-wor-gui.sh* ]]' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'activate_macos_gui "$owner_pid"' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF '$.NSRunningApplication.runningApplicationWithProcessIdentifier(pid)' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'if (!runningApp.activateWithOptions($.NSApplicationActivateAllWindows | $.NSApplicationActivateIgnoringOtherApps)) $.exit(1)' "$REPO_DIR/install-wor-gui.sh" \
    && pass "the macOS GUI enforces a single instance and refocuses the running one" \
    || fail "macOS GUI single-instance handling is missing or does not refocus the running app"

  grep -qF 'macos_show_announcement()' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'partnership.png' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'Proceed with WoR-Flasher' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF "textView:clickedOnLink:atIndex:" "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF "addPromptLink('Blackout Secure', 'https://blackoutsecure.app/')" "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF "addPromptLink('Botspot', 'https://github.com/Botspot')" "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF "addPromptLink('Windows on R', 'https://worproject.com/')" "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF "addPromptLink('Botspot/wor-flasher', 'https://github.com/Botspot/wor-flasher')" "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF "addPromptLink('sponsoring Botspot', 'https://github.com/sponsors/Botspot')" "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF "addPromptLink('buying Blackout Secure a coffee', 'https://github.com/sponsors/blackoutsecure?frequency=one-time&amount=8')" "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF '$.NSForegroundColorAttributeName, $.NSColor.linkColor, range' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'Blackout Secure is proud to partner with Botspot' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF "printf -v announcement_text '%s\\n\\n%s\\n\\n%s\\n'" "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'yad "${yadflags[@]}" --width="$(wor_yad_width 840)" --height="$(wor_yad_height 720)" --center' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF -- '--image="$announcement_image" --image-on-top --text-align=center' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF -- '--form --align=center --buttons-layout=center --timeout="$WOR_ANNOUNCEMENT_TIMEOUT"' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'announcement_image="$(wor_yad_image_for_screen' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF -- "--field=\$'<a href=\"https://blackoutsecure.app/\">Blackout Secure</a>" "$REPO_DIR/install-wor-gui.sh" \
    && pass "the partnership announcement renders with its attribution links" \
    || fail "the partnership announcement, its copy, or an attribution link is missing"

  #yad draws --image at its native size, so an oversized banner grows the dialog past the screen
  png_dimension() { #Input: png path and 0 for width or 4 for height. Output: pixel count from the IHDR chunk.
    od -An -tu1 -j "$((16 + $2))" -N4 "$1" | awk 'NF{print $1 * 16777216 + $2 * 65536 + $3 * 256 + $4; exit}'
  }
  banner_width="$(png_dimension "$REPO_DIR/assets/partnership.png" 0)"
  banner_height="$(png_dimension "$REPO_DIR/assets/partnership.png" 4)"
  [ -n "$banner_width" ] && [ -n "$banner_height" ] && [ "$banner_width" -le 1000 ] && [ "$banner_height" -le 700 ] \
    && pass "the partnership banner fits a yad dialog at its native size" \
    || fail "assets/partnership.png is ${banner_width}x${banner_height}; yad cannot scale it down"

  responsive_yad="$(run_in_engine 'WOR_YAD_SCREEN_WIDTH=800; WOR_YAD_SCREEN_HEIGHT=600; wor_init_yad_flags; printf "%s|%s|%s|%s" "$(wor_yad_width 840)" "$(wor_yad_height 720)" "$(wor_yad_image_for_screen preferred fallback 880 740)" "$(WOR_YAD_SCREEN_WIDTH=1024 WOR_YAD_SCREEN_HEIGHT=768 wor_yad_image_for_screen preferred fallback 880 740)"')"
  [ "$responsive_yad" == '760|540|fallback|preferred' ] \
    && grep -qF 'wor_detect_yad_screen() {' "$REPO_DIR/src/lib/gui.sh" \
    && grep -qF 'command -v xrandr' "$REPO_DIR/src/lib/gui.sh" \
    && grep -qF 'command -v xdpyinfo' "$REPO_DIR/src/lib/gui.sh" \
    && grep -qF 'command -v xwininfo' "$REPO_DIR/src/lib/gui.sh" \
    && [ "$(grep -cE -- '--(width|height)=[0-9]+' "$REPO_DIR/install-wor-gui.sh")" == 1 ] \
    && pass "Linux dialogs clamp to the detected screen and use smaller artwork when needed" \
    || fail "Linux dialog sizing is fixed or can place content outside the screen: '$responsive_yad'"

  #gtk_window_resize asserts height > 0, so a zero height logs a Gtk-CRITICAL on every dialog
  ! grep -qF -- '--height=0' "$REPO_DIR/install-wor-gui.sh" \
    && pass "no yad dialog asks GTK for a zero height" \
    || fail "a yad dialog uses --height=0, which trips a gtk_window_resize assertion"

  grep -qF 'WOR_WINDOW_TITLE' "$REPO_DIR/src/lib/metadata.sh" \
    && grep -qF '"$WOR_WINDOW_TITLE"' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF '"${10:-$WOR_WINDOW_TITLE}"' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF '"${13:-$WOR_APP_TITLE}"' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF '"$WOR_WINDOW_TITLE" Back 0 "$WOR_APP_TITLE"' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF -- '--title="$WOR_WINDOW_TITLE"' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF -- '--class="$WOR_ICON_NAME"' "$REPO_DIR/src/lib/gui.sh" \
    && grep -qF 'ensure_linux_desktop_identity()' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'StartupWMClass=$WOR_ICON_NAME' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'Icon=$icon_path' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'const isMessageMode = choices.length === 0' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'const iconPath = ObjC.unwrap(args.objectAtIndex(12))' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'const cancelValue = ObjC.unwrap(args.objectAtIndex(14))' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF "function cancelAndExit()" "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'attributedPrompt.mutableString.appendString($(promptText))' "$REPO_DIR/install-wor-gui.sh" \
    && ! grep -qF 'NSMutableAttributedString.alloc.initWithString' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF '"$WOR_LOGO_PATH"' "$REPO_DIR/install-wor-gui.sh" \
    && [ "$(run_in_engine 'printf %s "$WOR_LOGO_FILENAME"')" == 'logo-full.png' ] \
    && [ "$(run_in_engine 'printf %s "$WOR_ASSETS_DIRNAME"')" == 'assets' ] \
    && grep -qF 'WOR_LOGO_PATH="$WOR_ASSETS_DIR/$WOR_LOGO_FILENAME"' "$REPO_DIR/install-wor.sh" \
    && grep -qF ': "${WOR_ANNOUNCEMENT_TIMEOUT:=30}"' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF "'countdownTick:'" "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF "nextButton.title = nextLabel + ' (' + countdownSeconds + ')'" "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF "selectedValue = defaultChoice" "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF "addTimerForMode(countdownTimer, $.NSModalPanelRunLoopMode)" "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF -- '--timeout="$WOR_ANNOUNCEMENT_TIMEOUT"' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'function writeResult(value)' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'fileHandleWithStandardOutput.writeData(data)' "$REPO_DIR/install-wor-gui.sh" \
    && ! grep -qF 'echo -e "\\\\e[91m' "$REPO_DIR/install-wor-gui.sh" \
    && ! grep -qF 'console.log(selectedValue)' "$REPO_DIR/install-wor-gui.sh" \
    && pass "the shared macos_choose dialog returns its result on stdout and honours the countdown" \
    || fail "the macos_choose dialog contract, its countdown, or its stdout result protocol is broken"

  grep -qF 'Choose Windows and Raspberry Pi target' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF "'Windows 11'" "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'Raspberry Pi model:' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'macos_choose_target()' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'Choose Windows and Raspberry Pi target' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF "selectedValue = windowsPopup.titleOfSelectedItem + '\\t' + piPopup.titleOfSelectedItem" "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'windowWillClose:' "$REPO_DIR/install-wor-gui.sh" \
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
    && grep -qF 'macos_confirm_flash() {' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'const heading = $.NSTextField.labelWithString' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF "const warning = $.NSTextField.labelWithString('All data on the target drive will be erased.')" "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF "const flashButton = $.NSButton.buttonWithTitleTargetAction('Flash'" "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'confirmation="$(macos_confirm_flash)"' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF '[ "$confirmation" == Cancel ] && exit 0' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'macos_advanced_options' "$REPO_DIR/install-wor-gui.sh" \
    && ! grep -qF 'display alert' "$REPO_DIR/install-wor-gui.sh" \
    && ! grep -qF 'display dialog' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'All data on the target drive will be erased.' "$REPO_DIR/install-wor-gui.sh" \
    && pass "the macOS wizard collects every choice and confirms before flashing" \
    || fail "a macOS wizard step, the drive refresh, or the flash confirmation is missing"

  chooser_function="$(sed -n '/^macos_choose_device() {/,/^}$/p' "$REPO_DIR/install-wor-gui.sh")"
  chooser_called="$(mktemp)"
  if [ -z "$chooser_function" ];then
    fail "the macOS device chooser function could not be loaded for behavioral testing"
  elif CHOOSER_CALLED="$chooser_called" bash -c '
    eval "$1"
    #a real Next/Refresh click in message mode returns the default (3rd arg); a real Next click
    #in list mode returns the selected row, simulated here as the last row in the list
    macos_choose() {
      printf "%s\n" "$*" > "$CHOOSER_CALLED"
      if [ -z "$1" ];then printf "%s\n" "$3"; else printf "%s\n" "$1" | tail -n1; fi
    }
    [ "$(macos_choose_device "")" == __REFRESH__ ] || exit 1
    grep -qF "No external, physical, writable drive was found." "$CHOOSER_CALLED" || exit 1
    choices="disk4 First drive
disk5 Second drive"
    [ "$(macos_choose_device "$choices")" == "disk5 Second drive" ]
  ' _ "$chooser_function" 2>/dev/null;then
    pass "the macOS device chooser refreshes an empty list and returns a selected drive"
  else
    fail "the macOS device chooser mishandles an empty list or selected drive"
  fi

  #a genuine Quit/close-box still must propagate as a failure from macos_choose_device, not be
  #swallowed and reported as if Refresh was clicked
  if [ -z "$chooser_function" ];then
    fail "the macOS device chooser cancel path could not be loaded for behavioral testing"
  elif bash -c '
    eval "$1"
    macos_choose() { return 1; }
    ! macos_choose_device ""
  ' _ "$chooser_function" 2>/dev/null;then
    pass "Quit on the no-drive screen is treated as a cancel, not a Refresh"
  else
    fail "Quit on the no-drive screen is swallowed and reported as Refresh"
  fi

  #clicking Back must succeed with the literal cancelValue instead of failing like Quit does,
  #so the wizard step machine can go back a step instead of exiting the whole program
  if [ -z "$chooser_function" ];then
    fail "the macOS device chooser Back path could not be loaded for behavioral testing"
  elif bash -c '
    eval "$1"
    macos_choose() { shift 10; printf "%s\n" "$1"; } #echo the 11th arg: cancelValue
    [ "$(macos_choose_device "")" == Back ] || exit 1
    choices="disk4 First drive
disk5 Second drive"
    [ "$(macos_choose_device "$choices")" == Back ]
  ' _ "$chooser_function" 2>/dev/null;then
    pass "Back on the device chooser screens succeeds with a literal value, unlike Quit"
  else
    fail "Back on a device chooser screen does not carry a distinct cancelValue"
  fi

  rm -f "$chooser_called"

  grep -qF "name: 'WorCompletionController'" "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'wor_osascript -l JavaScript - "$completion_text"' "$REPO_DIR/install-wor-gui.sh" \
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
    && pass "the macOS progress and completion windows report installer status natively" \
    || fail "a macOS progress or completion window is missing, or a spawned-terminal fallback returned"

  grep -qF 'device_tree_address=0x3e0000' "$REPO_DIR/config-templates/pi4.config.txt" \
    && grep -qF 'device_tree_end=0x400000' "$REPO_DIR/config-templates/pi4.config.txt" \
    && grep -qF 'read_config_template "pi$1.config.txt"' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'set_default_config_txt' "$REPO_DIR/install-wor-gui.sh" \
    && pass "Pi 4 GUI config matches the current UEFI memory range" \
    || fail "Pi 4 GUI config uses stale device-tree addresses"

  #stepping back to change the model must not carry the previous model's boot config onto the drive,
  #while a config.txt the user has edited stays theirs
  run_in_engine '
    RPI_MODEL=5; CONFIG_TXT=""; CONFIG_TXT_MODEL=""
    set_default_config_txt
    pi5="$CONFIG_TXT"
    RPI_MODEL=4
    set_default_config_txt
    [ "$CONFIG_TXT" == "$(default_config_txt 4)" ] || exit 1
    [ "$CONFIG_TXT" != "$pi5" ] || exit 1
    CONFIG_TXT="$CONFIG_TXT
#edited by the user"
    edited="$CONFIG_TXT"
    RPI_MODEL=3
    set_default_config_txt
    [ "$CONFIG_TXT" == "$edited" ] || exit 1
  ' \
    && pass "changing the Pi model reloads that model config.txt without discarding user edits" \
    || fail "changing the Pi model keeps the previous model config.txt, or overwrites a user edit"
  #v1.51/v1.52 report a zero MAC (pftf/RPi4#283); v1.52/v1.53 do not boot from microSD (pftf/RPi4#285)
  [ "$(run_in_engine 'printf %s "$WOR_DEFAULT_UEFI_VER_PI4"')" == 'v1.50' ] \
    && ! grep -qF '"uefiVerPi4": "v1.51"' "$REPO_DIR/src/config/metadata.json" \
    && ! grep -qF '"uefiVerPi4": "v1.52"' "$REPO_DIR/src/config/metadata.json" \
    && ! grep -qF '"uefiVerPi4": "v1.53"' "$REPO_DIR/src/config/metadata.json" \
    && pass "Pi 4 pins the only UEFI release with a working MAC address and microSD boot" \
    || fail "Pi 4 pins a UEFI release with a zero Ethernet MAC or a microSD boot regression"

  grep -qF '#Raspberry Pi 4 only; this setting is ignored for every other model.' "$REPO_DIR/install-wor.sh" \
    && grep -qF '[ -z "$PI4_AUTO_DISABLE_3GB" ] && PI4_AUTO_DISABLE_3GB=1' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'SetFirmwareEnvironmentVariableEx("RamLimitTo3GB", "{CD7CC258-31DB-22E6-9F22-63B0B8EED6B5}"' "$REPO_DIR/config-templates/pi4-ram-unlock.ps1" \
    && grep -qF '<settings pass="specialize">' "$REPO_DIR/config-templates/pi4-ram-unlock-specialize.xml" \
    && grep -qF '<Path>powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%WINDIR%\Setup\Scripts\Pi4Disable3GB.ps1"</Path>' "$REPO_DIR/config-templates/pi4-ram-unlock-specialize.xml" \
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
    && grep -qF 'WINDOWS_ACCOUNT_SETUP' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'WINDOWS_LOCALE_SETUP' "$REPO_DIR/install-wor.sh" \
    && grep -qF '<LocalAccount wcm:action="add">' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'Microsoft-Windows-International-Core' "$REPO_DIR/install-wor.sh" \
    && [ "$(grep -cF 'install_windows_setup_configuration ' "$REPO_DIR/install-wor.sh")" == 2 ] \
    && grep -qF 'cmp -s "$boot_mount/Autounattend.xml" "$install_mount/Autounattend.xml"' "$REPO_DIR/install-wor.sh" \
    && pass "Windows OOBE network bypass is default-on, configurable, written to both partitions, and verified" \
    || fail "Windows OOBE network bypass is incomplete"

  account_xml="$(run_in_engine 'WINDOWS_ACCOUNT_SETUP=1 WINDOWS_ACCOUNT_USERNAME="Test&User" WINDOWS_ACCOUNT_PASSWORD="safe<password>" WINDOWS_LOCALE_SETUP=1 WINDOWS_LOCALE=en-GB unattend_xml')"
  printf '%s' "$account_xml" | grep -qF '<Name>Test&amp;User</Name>' \
    && printf '%s' "$account_xml" | grep -qF '<Value>safe&lt;password&gt;</Value>' \
    && printf '%s' "$account_xml" | grep -qF '<InputLocale>en-GB</InputLocale>' \
    && ! printf '%s' "$account_xml" | grep -qF 'WINDOWS_ACCOUNT_PASSWORD' \
    && pass "optional Windows account and locale settings render safely in unattended XML" \
    || fail "optional Windows account or locale settings render incorrectly"

  [ -f "$REPO_DIR/config-templates/pi3.config.txt" ] \
    && [ -f "$REPO_DIR/config-templates/pi4.config.txt" ] \
    && [ -f "$REPO_DIR/config-templates/pi5.config.txt" ] \
    && [ -f "$REPO_DIR/config-templates/pi4-ram-unlock.ps1" ] \
    && [ -f "$REPO_DIR/config-templates/pi4-ram-unlock-specialize.xml" ] \
    && grep -qF 'PI4_UEFI_SHELL_UNLOCK' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'ShellBinPkg/UefiShell/AArch64/Shell.efi' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'BOOTAA64.WOR' "$REPO_DIR/config-templates/prefinalize.cmd" \
    && grep -qF 'setvar RamLimitTo3GB' "$REPO_DIR/config-templates/prefinalize.cmd" \
    && grep -qF 'rm \EFI\BOOT\BOOTAA64.EFI' "$REPO_DIR/config-templates/prefinalize.cmd" \
    && [ -f "$REPO_DIR/config-templates/oobe-network-bypass.xml" ] \
    && [ -f "$REPO_DIR/config-templates/prefinalize.cmd" ] \
    && [ -f "$REPO_DIR/config-templates/config.json" ] \
    && [ -f "$REPO_DIR/config-templates/config.schema.json" ] \
    && grep -qF 'read_config_template() {' "$REPO_DIR/install-wor.sh" \
    && ! grep -qF 'sync_repo_template' "$REPO_DIR/install-wor.sh" \
    && ! grep -qF 'raw.githubusercontent.com' "$REPO_DIR/install-wor.sh" \
    && pass "config-templates/ files exist as static, locally-editable files with no redundant per-file repo sync" \
    || fail "config-templates/ files are missing, or the removed per-file sync mechanism is still present"

  [ "$(jq -r 'has("system")' "$REPO_DIR/config-templates/config.json" 2>/dev/null)" == false ] \
    && grep -qF '"peInstallerUrl"' "$REPO_DIR/src/config/metadata.json" \
    && grep -qF '"peInstallerSha256"' "$REPO_DIR/src/config/metadata.json" \
    && grep -qF '"uefiVerPi4"' "$REPO_DIR/src/config/metadata.json" \
    && grep -qF '"driverVer"' "$REPO_DIR/src/config/metadata.json" \
    && grep -qF '"repoSlug"' "$REPO_DIR/src/config/metadata.json" \
    && pass "default config omits project pins; runtime metadata supplies PE, firmware, driver and update defaults" \
    || fail "default config still carries project pins, or metadata defaults are missing"

  #these must be committed: a fresh clone without them silently writes a blank config.txt and the Pi will not boot
  if command -v git >/dev/null && git -C "$REPO_DIR" rev-parse --git-dir >/dev/null 2>&1 ;then
    [ "$(git -C "$REPO_DIR" ls-files config-templates/ | wc -l | tr -d ' ')" == 9 ] \
      && grep -qF 'This file ships with WoR-Flasher and is required to write a bootable drive.' "$REPO_DIR/install-wor.sh" \
      && pass "config-templates/ files are tracked by git and a missing one aborts instead of writing a blank config.txt" \
      || fail "config-templates/ files are untracked, or a missing template does not abort"
  else
    skip "git is unavailable; cannot verify that config-templates/ is tracked"
  fi

  grep -qF 'macos_advanced_options() {' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF "name: 'WorAdvancedController'" "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'NSButton.checkboxWithTitleTargetAction' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'Automatically disable the Pi 4 3 GB RAM limit after install' "$REPO_DIR/src/lib/gui.sh" \
    && grep -qF 'Skip flashing the device (dry run)' "$REPO_DIR/src/lib/gui.sh" \
    && grep -qF 'function addSectionHeader(title)' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF "addSectionHeader('Windows setup')" "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF "addSectionHeader('Firmware and drivers')" "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF "addSectionHeader('Validation')" "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF "addSectionHeader('Downloads')" "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF "addSectionHeader('Windows account')" "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF "addSectionHeader('Regional settings')" "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF "addSectionHeader('Raspberry Pi boot config')" "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF "accountCheckbox = $.NSButton.checkboxWithTitleTargetAction('Create a local Windows administrator account'" "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'accountUsernameField.enabled = enabled' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'accountPasswordField.enabled = enabled' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'updateAccountEditableState()' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF "localeCheckbox = $.NSButton.checkboxWithTitleTargetAction('Configure Windows keyboard and regional settings'" "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'localePopup.enabled = enabled' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'localePopup.addItemWithTitle' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'const localeSpec = ObjC.unwrap(args.objectAtIndex(18))' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'const windowTitle = ObjC.unwrap(args.objectAtIndex(19))' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF "title: windowTitle + ' | Advanced Options'" "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF '"$WIN_LANG" "$WINDOWS_ACCOUNT_SETUP" "$WINDOWS_LOCALE_SETUP" "$locale_spec" "$WOR_WINDOW_TITLE"' "$REPO_DIR/install-wor-gui.sh" \
    && [ "$(grep -cF 'export_installer_settings' "$REPO_DIR/install-wor-gui.sh")" == 1 ] \
    && grep -qF 'export "${WOR_INSTALLER_SETTINGS[@]}"' "$REPO_DIR/install-wor.sh" \
    && grep -qF "editMenu.addItemWithTitleActionKeyEquivalent('Copy', 'copy:', 'c')" "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'app.mainMenu = mainMenu' "$REPO_DIR/src/lib/gui.sh" \
    && pass "macOS and Linux GUIs expose an Advanced Options window for site-documented customizations" \
    || fail "Advanced Options window is missing or incomplete"

  grep -qF 'linux_choose_one() {' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF -- '--use-markup' "$REPO_DIR/install-wor-gui.sh" \
    && [ "$(grep -anF -- '--field=Windows setup:LBL' "$REPO_DIR/install-wor-gui.sh" | cut -d: -f1)" -lt "$(grep -anF -- '--field=Firmware and drivers:LBL' "$REPO_DIR/install-wor-gui.sh" | cut -d: -f1)" ] \
    && [ "$(grep -anF -- '--field=Firmware and drivers:LBL' "$REPO_DIR/install-wor-gui.sh" | cut -d: -f1)" -lt "$(grep -anF -- '--field=Validation:LBL' "$REPO_DIR/install-wor-gui.sh" | cut -d: -f1)" ] \
    && [ "$(grep -anF -- '--field=Validation:LBL' "$REPO_DIR/install-wor-gui.sh" | cut -d: -f1)" -lt "$(grep -anF -- '--field=Downloads:LBL' "$REPO_DIR/install-wor-gui.sh" | cut -d: -f1)" ] \
    && grep -qF -- "--field='Windows version:CB' 'Windows 11!Windows 10!More options'" "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF -- "--field='Raspberry Pi model:CB' 'Raspberry Pi 5!Raspberry Pi 4 / Pi 400!Raspberry Pi 3 / Pi 2 v1.2'" "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF "RPI_MODEL=''" "$REPO_DIR/install-wor-gui.sh" \
    && ! grep -qF -- '--form --columns=2' "$REPO_DIR/install-wor-gui.sh" \
    && ! grep -qF -- "--button='<b>View / Edit config.txt...</b>':3" "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF -- '--field=<b>View / Edit config.txt</b>:BTN' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF -- '--changed-action="$changed_action"' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'account_username_value=' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF "account_username_value='@disabled@'" "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF "account_password_value='@disabled@'" "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF "locale_value='@disabled@'" "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF "config_button_value='@disabled@'" "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'account_checkbox_field=$((${#fields[@]} / 2 + 1))' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'locale_checkbox_field=$((${#fields[@]} / 2 + 1))' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'config_checkbox_field=$((${#fields[@]} / 2 + 1))' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'account_username_field=$((${#fields[@]} / 2 + 1))' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'config_button_field=$((${#fields[@]} / 2 + 1))' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'yad_field_value()' "$REPO_DIR/install-wor-gui.sh" \
    && ! grep -qF '$(echo "$output" | sed -n 7p)' "$REPO_DIR/install-wor-gui.sh" \
    && ! grep -qF '$(echo "$output" | sed -n 18p)' "$REPO_DIR/install-wor-gui.sh" \
    && [ "$(grep -cF -- '--form --scroll' "$REPO_DIR/install-wor-gui.sh")" -ge 2 ] \
    && grep -qF 'yadflags=(--center --fixed --buttons-layout=center' "$REPO_DIR/src/lib/gui.sh" \
    && grep -qF -- "--button='<b>Abort</b>':1" "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'if ! kill -0 "$yad_pid"' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'kill_process_tree "$installer_pid"' "$REPO_DIR/install-wor-gui.sh" \
    && pass "Linux follows the macOS screen route, bounded forms, fixed windows, and safe progress close" \
    || fail "Linux still uses its old combined route, embedded editor, resizable windows, or unsafe progress close"

  linux_repair_dir="$(mktemp -d)"
  mkdir -p "$linux_repair_dir/assets"
  cp "$REPO_DIR/install-wor-gui.sh" "$linux_repair_dir/install-wor-gui.sh"
  printf 'original script\n' > "$linux_repair_dir/install-wor.sh"
  printf 'next steps\n' > "$linux_repair_dir/assets/next-steps.png"
  git -C "$linux_repair_dir" init -q
  git -C "$linux_repair_dir" add install-wor-gui.sh install-wor.sh assets/next-steps.png
  git -C "$linux_repair_dir" -c user.name=Test -c user.email=test@example.com commit -qm fixture
  printf 'modified script\n' > "$linux_repair_dir/install-wor.sh"
  rm "$linux_repair_dir/assets/next-steps.png"
  awk '{print} /repair_missing_checkout_runtime \|\| exit 1/{exit}' "$linux_repair_dir/install-wor-gui.sh" > "$linux_repair_dir/repair-probe.sh"
  chmod +x "$linux_repair_dir/repair-probe.sh"
  if DIRECTORY="$linux_repair_dir" bash "$linux_repair_dir/repair-probe.sh" >/dev/null 2>&1 \
    && [ "$(cat "$linux_repair_dir/assets/next-steps.png")" == 'next steps' ] \
    && [ "$(cat "$linux_repair_dir/install-wor.sh")" == 'modified script' ];then
    pass "Linux startup repairs missing tracked runtime files before loading shared libraries"
  else
    fail "Linux startup repair is too late, misses assets, or overwrites modified files"
  fi
  rm -rf "$linux_repair_dir"

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
  [ "$(grep -cF "'handleQuitEvent:withReplyEvent:': {" "$REPO_DIR/install-wor-gui.sh")" == 6 ] \
    && [ "$(grep -cF "'pumpEvents:': {" "$REPO_DIR/install-wor-gui.sh")" == 6 ] \
    && [ "$(grep -cF 'worInstallWindowHandlers(controller)' "$REPO_DIR/install-wor-gui.sh")" == 6 ] \
    && grep -qF '0x61657674, 0x71756974' "$REPO_DIR/src/lib/gui.sh" \
    && grep -qF 'addTimerForMode(pumpTimer' "$REPO_DIR/src/lib/gui.sh" \
    && [ "$(grep -cF 'app.runModalForWindow(window)' "$REPO_DIR/install-wor-gui.sh")" == 6 ] \
    && pass "every macOS window responds to the Dock's Quit menu item" \
    || fail "a macOS window cannot receive the Dock's quit Apple Event"

  #these heredocs sit inside "$( ... )", so bash still tracks quote state through the body:
  #a lone apostrophe (e.g. "doesn't") silently swallows every function defined after it
  [ -z "$(awk '/<<.JXA.$/{inh=1; next} inh && /^JXA$/{inh=0; next} inh{n=gsub(/'"'"'/,""); if(n%2==1) print NR}' "$REPO_DIR/install-wor-gui.sh")" ] \
    && pass "JXA heredoc bodies contain no unpaired apostrophes" \
    || fail "an unpaired apostrophe in a JXA heredoc will corrupt shell parsing"

  #guards against the same class of breakage from any cause: run the real script far enough to
  #register its function definitions, then confirm every macos_* helper actually became a function
  macos_fn_expected="$(grep -ac '^macos_[a-z_]*() {' "$REPO_DIR/install-wor-gui.sh")"
  macos_fn_probe="$(mktemp)"
  awk -v line="$(grep -an '^macos_start_cli() {' "$REPO_DIR/install-wor-gui.sh" | cut -d: -f1)" \
    'NR==line{print "declare -F | grep -c \"^declare -f macos_\"; exit 0"} {print}' \
    "$REPO_DIR/install-wor-gui.sh" > "$macos_fn_probe"
  #macos_start_cli itself is not defined yet at the probe point.
  #DIRECTORY is supplied because the probe is a copy: the GUI resolves install-wor.sh relative to its own path.
  #TMPDIR is isolated so the single-instance lock is free: a GUI already open would otherwise
  #make the probe hand off to it and exit before reaching the marker.
  macos_fn_tmpdir="$(mktemp -d)"
  [ "$(cd "$REPO_DIR" && DIRECTORY="$REPO_DIR" WOR_NATIVE_APP=1 TMPDIR="$macos_fn_tmpdir" bash "$macos_fn_probe" 2>/dev/null | tail -n1)" == "$((macos_fn_expected - 1))" ] \
    && pass "all macOS helper functions parse as separate top-level definitions" \
    || fail "a macOS function definition is being swallowed by a preceding heredoc"
  rm -rf "$macos_fn_probe" "$macos_fn_tmpdir"

  #the engine ignores PI4_AUTO_DISABLE_3GB unless RPI_MODEL is 4, so the GUIs must not offer it as a live choice
  grep -qF 'checked: parts[1]' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF "enabled: parts[2] !== '0'" "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'checkbox.enabled = rows[i].enabled' "$REPO_DIR/install-wor-gui.sh" \
    && [ "$(grep -cF 'pi4_applicable=1 || pi4_applicable=0' "$REPO_DIR/install-wor-gui.sh")" == 2 ] \
    && grep -qF 'not applicable to the Pi %s' "$REPO_DIR/src/lib/gui.sh" \
    && grep -qF '2) [ "$pi4_applicable" == 1 ] && PI4_AUTO_DISABLE_3GB="$line" ;;' "$REPO_DIR/install-wor-gui.sh" \
    && pass "the Pi 4 RAM-unlock toggle is greyed out and ignored on other Pi models" \
    || fail "the Pi 4 RAM-unlock toggle is not gated on the selected Pi model"

  #in recovery mode the custom config.txt only boots the installer media
  grep -qF "printf 'boot partition'" "$REPO_DIR/src/lib/gui.sh" \
    && grep -qF 'not the Windows drive' "$REPO_DIR/src/lib/gui.sh" \
    && grep -qF "printf 'Apply the customized config.txt to the %s'" "$REPO_DIR/src/lib/gui.sh" \
    && pass "the config.txt editor states its scope for the selected installation mode" \
    || fail "the config.txt editor does not state its scope per installation mode"

  grep -qF '[ -z "$SKIP_IMAGE_VERIFICATION" ] && SKIP_IMAGE_VERIFICATION=0' "$REPO_DIR/install-wor.sh" \
    && [ "$(grep -cF 'if [ "$SKIP_IMAGE_VERIFICATION" == 1 ];then' "$REPO_DIR/install-wor.sh")" == 2 ] \
    && [ "$(grep -cF 'verify_written_image "$DEVICE" "$PART1" "$PART2"' "$REPO_DIR/install-wor.sh")" == 2 ] \
    && grep -qF 'Use the latest UEFI firmware instead of the tested pinned version (%s)' "$REPO_DIR/src/lib/gui.sh" \
    && grep -qF 'Use the latest Windows ARM64 drivers instead of the pinned version (%s)' "$REPO_DIR/src/lib/gui.sh" \
    && grep -qF 'Skip verifying the written image after flashing' "$REPO_DIR/src/lib/gui.sh" \
    && grep -qF 'Flash begins immediately after administrator approval. Use Advanced to change these settings.' "$REPO_DIR/install-wor-gui.sh" \
    && pass "Skip-verification option defaults off, wraps both verify_written_image calls, and confirm screens show pinned versions and guidance" \
    || fail "Skip-verification option or confirm-screen guidance is missing or incomplete"

  grep -qF '[ -z "$APPLY_CUSTOM_CONFIG_TXT" ] && APPLY_CUSTOM_CONFIG_TXT=1' "$REPO_DIR/install-wor.sh" \
    && grep -qF '[ -z "$CONFIG_TXT" ] || [ "$APPLY_CUSTOM_CONFIG_TXT" != 1 ] || printf' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'mkdir -p "$boot_mount/efi"' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'cp -R "$PWD/peinstaller/efi/." "$boot_mount/efi"' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'cp -RX "$PWD/pi${RPI_MODEL}-uefipackage"/* "$boot_mount"' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'if is_macos;then' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'copy_mounted_file_with_progress()' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'mkdir -p "$destination/boot" "$destination/efi"' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'cp -R "$source/boot/." "$destination/boot"' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'cp -R "$source/efi/." "$destination/efi"' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'sudo cp -R "$source/boot" "$source/efi" "$destination"' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'mounted_wimverify()' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'mounted_wimdir "$boot_mount/sources/boot.wim"' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'if [ ! -z "$CONFIG_TXT" ] && [ "$APPLY_CUSTOM_CONFIG_TXT" == 1 ];then' "$REPO_DIR/install-wor.sh" \
    && grep -qF "applyConfigCheckbox = \$.NSButton.checkboxWithTitleTargetAction(applyConfigLabel, controller, 'applyConfigToggled:')" "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'function updateConfigEditableState() {' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF "editConfigButton = \$.NSButton.buttonWithTitleTargetAction('View / Edit…', controller, 'editConfigClicked:')" "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'editConfigButton.enabled = enabled' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF "dialog.messageText = \$('View / Edit config.txt')" "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'configTxtValue = ObjC.unwrap(editor.string)' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'out.push(configTxtValue)' "$REPO_DIR/install-wor-gui.sh" \
    && pass "Applying the customized config.txt is a togglable checkbox that enables its separate editor on macOS" \
    || fail "Apply-customized-config.txt toggle is missing or incomplete"
  apply_config_line="$(grep -anF "applyConfigCheckbox = $.NSButton.checkboxWithTitleTargetAction(applyConfigLabel," "$REPO_DIR/install-wor-gui.sh" | cut -d: -f1)"
  config_editor_line="$(grep -anF "editConfigButton = $.NSButton.buttonWithTitleTargetAction('View / Edit…'" "$REPO_DIR/install-wor-gui.sh" | cut -d: -f1)"
  [ -n "$apply_config_line" ] && [ -n "$config_editor_line" ] && [ "$apply_config_line" -lt "$config_editor_line" ] \
    && [ "$((config_editor_line - apply_config_line))" -lt 10 ] \
    && pass "Use customized config.txt and its editor button share one settings row" \
    || fail "Use customized config.txt is not grouped with its editor button"

  grep -qF '[ -z "$HIDE_EMPTY_DRIVES" ] && HIDE_EMPTY_DRIVES=1' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'configure_pe_settings_ini() {' "$REPO_DIR/install-wor.sh" \
    && grep -qF "settings_ini=\"\$PWD/peinstaller/winpe/2/settings.ini\"" "$REPO_DIR/install-wor.sh" \
    && [ "$(grep -cF 'configure_pe_settings_ini' "$REPO_DIR/install-wor.sh")" == 3 ] \
    && pass "HideEmptyDrives is written into the cached PE settings.ini before boot.wim assembly" \
    || fail "HideEmptyDrives support is missing or incomplete"

  grep -qF 'Allow Windows setup to continue without a network connection' "$REPO_DIR/src/lib/gui.sh" \
    && ! grep -qF "step=oobe" "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'OOBE_NETWORK_BYPASS' "$REPO_DIR/install-wor.sh" \
    && pass "macOS and Linux GUIs expose the OOBE network choice only in Advanced Options" \
    || fail "GUI OOBE network choice is incomplete"

  grep -qF 'emit_gui_progress() { #Input: line.' "$REPO_DIR/install-wor.sh" \
    && grep -qF $'STEP\t$STEP_NUM\t$STEP_TOTAL\t$1' "$REPO_DIR/install-wor.sh" \
    && grep -qF $'STATUS\t$1' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'LINUX_ASKPASS="$(mktemp)"' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'WOR_FLASH_TARGET="$DEVICE" WOR_ICON_PATH="$WOR_LOGO_PATH" SUDO_ASKPASS="$LINUX_ASKPASS" command sudo -A "$@"' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'sudo parted -ms "$device" unit B print' "$REPO_DIR/install-wor.sh" \
    && grep -qF -- '--progress --image="$WOR_LOGO_PATH" --text="Starting..."' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF -- '--bar="Overall:NORM" --bar="Sub-progress:NORM"' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF '1:# Overall' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF '2:# Sub-progress' "$REPO_DIR/install-wor-gui.sh" \
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

  #After Flash, the GUI opens progress immediately and defers the single sudo prompt until the
  #destructive disk step, so long setup/download/prep work cannot expire it before use.
  gui_auth_wait_line="$(grep -an 'while \[ ! -e "\$auth_marker" \] && \[ ! -f "\$done_marker" \] ;do' "$REPO_DIR/install-wor-gui.sh" | cut -d: -f1)"
  macos_progress_line="$(grep -an '<<<"\$progress_jxa"' "$REPO_DIR/install-wor-gui.sh" | head -n1 | cut -d: -f1)"
  preauth_line="$(grep -anF '[ "$RUN_MODE" == gui ] && gui_preauthenticate' "$REPO_DIR/install-wor.sh" | cut -d: -f1)"
  setup_line="$(grep -anF 'setup || exit 1' "$REPO_DIR/install-wor.sh" | cut -d: -f1)"
  [ -n "$gui_auth_wait_line" ] && [ -n "$macos_progress_line" ] \
    && [ -n "$preauth_line" ] && [ -n "$setup_line" ] && [ "$preauth_line" -lt "$setup_line" ] \
    && grep -aqF 'export WOR_GUI_AUTH_MARKER="$auth_marker"' "$REPO_DIR/install-wor-gui.sh" \
    && grep -aqF 'export GUI_PROGRESS_EARLY=1' "$REPO_DIR/install-wor-gui.sh" \
    && grep -aqF 'gui_preauthenticate() {' "$REPO_DIR/install-wor.sh" \
    && grep -aqF 'deferred until Step 5' "$REPO_DIR/install-wor.sh" \
    && ! grep -aqF 'WOR_GUI_SUDO_PREAUTH_DONE=1' "$REPO_DIR/install-wor.sh" \
    && grep -aqF 'command sudo -n -v >/dev/null 2>&1 || true; sleep 30' "$REPO_DIR/install-wor.sh" \
    && pass "GUI defers the single sudo prompt until the destructive disk step" \
    || fail "GUI still authenticates too early or can hide a sudo prompt behind progress"

  #under `set -e` an unguarded refresh ends the keepalive on its first failure, and the session then
  #lapses mid-flash into the refusal to prompt again
  keepalive_iterations="$(
    set -e
    ( iterations=0
      while [ "$iterations" -lt 3 ] ;do
        iterations=$((iterations + 1))
        false >/dev/null 2>&1 || true
        printf '%s\n' "$iterations"
      done ) | tail -n1
  )"
  [ "$keepalive_iterations" == 3 ] \
    && grep -aqF '[ "${BASH_SUBSHELL:-0}" -eq 0 ] || return 0' "$REPO_DIR/install-wor.sh" \
    && grep -aqF 'sleep 30; done ) >/dev/null 2>&1 </dev/null &' "$REPO_DIR/install-wor.sh" \
    && grep -aqF ') >/dev/null 2>&1 </dev/null &' "$REPO_DIR/install-wor.sh" \
    && ! grep -aqE 'command sudo -n -v >/dev/null 2>&1; sleep 30' "$REPO_DIR/install-wor.sh" \
    && pass "the sudo keepalive survives a failed refresh instead of ending the session" \
    || fail "a failed sudo refresh can still end the keepalive and strand the flash"

  keepalive_hang_dir="$(mktemp -d)"
  cat > "$keepalive_hang_dir/sudo" <<'SH'
#!/bin/bash
printf '%s\n' "$*" >> "$KEEPALIVE_HANG_LOG"
case "$*" in
  '-n -v') exit 1 ;;
  '-A echo ok') exit 0 ;;
esac
exit 0
SH
  chmod +x "$keepalive_hang_dir/sudo"
  keepalive_hang_log="$keepalive_hang_dir/log"
  keepalive_hang_status=0
  PATH="$keepalive_hang_dir:$PATH" KEEPALIVE_HANG_LOG="$keepalive_hang_log" run_in_engine 'RUN_MODE=gui; MACOS_ASKPASS=/tmp/wor-test-askpass; captured="$(sudo echo ok 2>&1)"; printf "%s\n" "$captured"' >/dev/null 2>&1 || keepalive_hang_status=$?
  [ "$keepalive_hang_status" == 0 ] \
    && grep -qF -- '-A echo ok' "$keepalive_hang_log" \
    && pass "GUI sudo does not start a keepalive inside command substitution" \
    || fail "GUI sudo can hang when first used inside command substitution"
  rm -rf "$keepalive_hang_dir"

  #exactly one place may prompt: the first destructive sudo call in the installer subprocess
  [ "$(grep -cE '(^|[^n]) *sudo -v' "$REPO_DIR/install-wor-gui.sh")" == 0 ] \
    && [ "$(grep -cF 'sudo -v ||' "$REPO_DIR/install-wor.sh")" == 0 ] \
    && grep -qF 'Administrator access: requesting macOS password with the native WoR-Flasher dialog.' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'if command sudo -n -v >/dev/null 2>&1;then' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'export WOR_GUI_SUDO_PROMPTED=1' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'command sudo -n "$@"' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'refusing to prompt in the console.' "$REPO_DIR/install-wor.sh" \
    && pass "the flash asks for the password once, in the process that uses it" \
    || fail "a credential is collected in more than one place, so the user is asked twice"

  #a frozen "Creating WOR_BOOT..." line while the password dialog is up reads as a hang; say what it's waiting on
  grep -qF "emit_gui_task_progress 0 'Waiting for administrator access...'" "$REPO_DIR/install-wor.sh" \
    && [ "$(grep -cF "emit_gui_task_progress 0 'Waiting for administrator access...'" "$REPO_DIR/install-wor.sh")" == 2 ] \
    && pass "the GUI progress window explains the administrator password dialog instead of appearing frozen" \
    || fail "the GUI progress window gives no indication it is waiting on the password dialog"

  sudo_retry_dir="$(mktemp -d)"
  cat > "$sudo_retry_dir/sudo" <<'SH'
#!/bin/bash
printf '%s\n' "$*" >> "$SUDO_RETRY_LOG"
case "$*" in
  '-n -v') exit 0 ;;
  '-n failing-command') exit 42 ;;
  '-A failing-command') exit 99 ;;
esac
exit 0
SH
  chmod +x "$sudo_retry_dir/sudo"
  sudo_retry_log="$sudo_retry_dir/log"
  sudo_retry_status=0
  PATH="$sudo_retry_dir:$PATH" SUDO_RETRY_LOG="$sudo_retry_log" run_in_engine 'RUN_MODE=gui MACOS_ASKPASS=/tmp/wor-test-askpass sudo failing-command' >/dev/null 2>&1 || sudo_retry_status=$?
  [ "$sudo_retry_status" == 42 ] \
    && grep -qF -- '-n -v' "$sudo_retry_log" \
    && grep -qF -- '-n failing-command' "$sudo_retry_log" \
    && ! grep -qF -- '-A failing-command' "$sudo_retry_log" \
    && pass "GUI sudo does not ask again when an authenticated command fails" \
    || fail "GUI sudo retries with askpass after a real command failure"
  rm -rf "$sudo_retry_dir"

  #after the first GUI sudo prompt, never ask again behind/near the progress window: either sudo -n works or the run fails
  reauth_dir="$(mktemp -d)"
  cat > "$reauth_dir/sudo" <<'SH'
#!/bin/bash
printf '%s\n' "$*" >> "$REAUTH_LOG"
case "$*" in
  '-n -v') exit 1 ;;
  '-A -v') exit 99 ;;
  '-n failing-command') exit 42 ;;
esac
exit 1
SH
  chmod +x "$reauth_dir/sudo"
  reauth_log="$reauth_dir/log"
  reauth_status=0
  PATH="$reauth_dir:$PATH" REAUTH_LOG="$reauth_log" run_in_engine 'RUN_MODE=gui WOR_GUI_SUDO_PROMPTED=1 MACOS_ASKPASS=/tmp/wor-test-askpass sudo failing-command' >/dev/null 2>&1 || reauth_status=$?
  [ "$reauth_status" == 1 ] \
    && ! grep -qF -- '-A -v' "$reauth_log" \
    && ! grep -qF -- '-A failing-command' "$reauth_log" \
    && grep -qF 'refusing to show a second password dialog during the flash' "$REPO_DIR/install-wor.sh" \
    && pass "GUI sudo refuses a second password prompt after first use" \
    || fail "GUI sudo can still ask for a second password after preauth"
  rm -rf "$reauth_dir"

  #a failed flash must leave the log behind; the GUI has no terminal to fall back on
  grep -qF 'saved_log="$(wor_log_file)"' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'Installer log saved to $saved_log' "$REPO_DIR/install-wor-gui.sh" \
    && [ "$(grep -cF 'saved_log="$(gui_save_failure_log)"' "$REPO_DIR/install-wor-gui.sh")" == 4 ] \
    && pass "a failed run keeps its installer log for diagnosis" \
    || fail "a failed run deletes the only record of what went wrong"

  #canceling the password dialog fails before anything destructive runs; the completion screen must say so
  #plainly instead of the generic "stopped unexpectedly" wording, which reads like a real crash
  grep -qF "grep -qF 'Administrator authentication was canceled or unavailable' \"\$saved_log\"" "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'No changes have been made to $DEVICE yet.' "$REPO_DIR/install-wor-gui.sh" \
    && pass "canceling the administrator password dialog is not reported as a script crash" \
    || fail "canceling the administrator password dialog is reported as if the script crashed"

  #mistyping the password 3 times gets the same friendly treatment, and both cases offer a retry
  #instead of forcing a full app restart when nothing has been written to disk yet
  grep -qF "grep -qF 'incorrect password attempts' \"\$saved_log\"" "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF "macos_choose '' \"\$password_retry_reason" "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF "retry Abort '' '' '' 'Try Again'" "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'if [ "$password_retry_choice" == retry ];then' "$REPO_DIR/install-wor-gui.sh" \
    && pass "a failed password attempt offers to try again instead of only an OK button" \
    || fail "a failed password attempt does not offer to try again"

  #the downloads and extraction all finish before the first sudo call, so a retry must resume at the
  #password step instead of repeating an entire run's worth of work for one mistyped password
  grep -qF 'resume_at_flash=1' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'export WOR_RESUME_AT_FLASH="${resume_at_flash:-0}"' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'if [ "${WOR_RESUME_AT_FLASH:-0}" == 1 ] && flash_files_already_prepared ;then' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'if [ "$RESUME_AT_FLASH" != 1 ];then' "$REPO_DIR/install-wor.sh" \
    && pass "retrying a password does not repeat the downloads and extraction" \
    || fail "retrying a password starts the whole run over"

  #the GUI sources install-wor.sh, which runs the update check; the installer subprocess must not
  #repeat it, or every run (and every password retry) pays for a second network round-trip
  grep -qF 'source "$cli_script" source' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'export NO_UPDATE=1' "$REPO_DIR/install-wor-gui.sh" \
    && pass "the GUI checks for updates once, not again in the installer subprocess" \
    || fail "the GUI checks for updates twice per run"

  #the success screen stacks banner, message and button down the middle at the image's own size.
  #Number() and the literal 1 are load-bearing: JXA returns these as strings, so '650' + 40 built a
  #bogus window frame, and the bridged NSTextAlignmentCenter constant (2) draws right-aligned here.
  grep -qF "const primaryLabel = imagePath.length > 0 ? 'Complete' : 'OK'" "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'bannerWidth = rep ? Number(rep.pixelsWide) : Number(bannerImage.size.width)' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'width = Math.max(600, bannerWidth + contentMargin * 2)' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'if (imagePath.length > 0) label.setAlignment(1)' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'Math.round((width - bannerWidth) / 2), textY + textHeight + 24' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'const okX = imagePath.length > 0 ? Math.round((width - okWidth) / 2) : width - 20 - okWidth' "$REPO_DIR/install-wor-gui.sh" \
    && pass "the completion screen centres the banner, message and button in one column" \
    || fail "the completion screen no longer centres its banner, message and button"

  [ "$(grep -cF 'It is now safe to remove your USB drive.' "$REPO_DIR/install-wor-gui.sh")" == 2 ] \
    && grep -qF 'completion_text="Process completed successfully.' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'linux_completion_image="$(wor_yad_image_for_screen' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF -- '--form --align=center --image-on-top --buttons-layout=center --image="$linux_completion_image"' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF -- '--field="It is now safe to remove your USB drive.":LBL' "$REPO_DIR/install-wor-gui.sh" \
    && pass "both success screens say when the USB drive is safe to remove" \
    || fail "a success screen does not tell the user the USB drive is safe to remove"

  #macOS can drop a freshly formatted FAT/exFAT volume part-way through the copy, so a mount point
  #resolved once at the start goes stale and the next write dies with "No such file or directory"
  grep -qF 'darwin_mount_point_or_die() {' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'darwin_mount_partition_or_die "$partition"' "$REPO_DIR/install-wor.sh" \
    && [ "$(grep -cF 'darwin_mount_point_or_die "$PART1"' "$REPO_DIR/install-wor.sh")" -ge 4 ] \
    && [ "$(grep -cF 'darwin_mount_point_or_die "$PART2"' "$REPO_DIR/install-wor.sh")" -ge 3 ] \
    && pass "each copy step re-checks its mount instead of trusting a stale path" \
    || fail "a dropped volume mid-copy still fails the whole flash"

  #a flash runs long enough to walk away from, so both front-ends announce the result. soundNamed
  #hands back a truthy wrapper for a missing name, so isNil is the only guard that actually works.
  grep -qF 'const completionSound = $.NSSound.soundNamed(soundName)' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'if (!completionSound.isNil()) completionSound.play' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'wor_play_result_sound() {' "$REPO_DIR/src/lib/gui.sh" \
    && grep -qF 'wor_play_result_sound success' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'wor_play_result_sound failure' "$REPO_DIR/install-wor-gui.sh" \
    && [ "$(bash -c "source '$REPO_DIR/src/lib/gui.sh'; PATH=/nonexistent wor_play_result_sound success >/dev/null 2>&1; echo \$?")" == 0 ] \
    && [ "$(bash -c "source '$REPO_DIR/src/lib/gui.sh'; PLAY_SOUND=0 wor_play_result_sound success >/dev/null 2>&1; echo \$?")" == 0 ] \
    && pass "finishing a flash plays a result sound without being able to fail the run" \
    || fail "the completion screen is silent, or its sound can fail the run"

  #the completion sound is configurable per platform, so one config file suits a Mac and a Linux box
  sound_cfg_ok=0
  if command -v jq >/dev/null ;then
    [ "$(jq -r '.notifications.playSound' "$REPO_DIR/config-templates/config.json")" == true ] \
      && [ "$(jq -r '.notifications.sounds.macos' "$REPO_DIR/config-templates/config.json")" == Glass ] \
      && [ "$(jq -r '.notifications.sounds.linux' "$REPO_DIR/config-templates/config.json")" == complete ] \
      && [ "$(jq -r '.notifications.sounds | has("windows")' "$REPO_DIR/config-templates/config.json")" == true ] \
      && [ "$(jq -r '.properties.notifications.properties.sounds.properties | keys | join(",")' "$REPO_DIR/config-templates/config.schema.json")" == 'linux,macos,windows' ] \
      && sound_cfg_ok=1
  else
    sound_cfg_ok=1 #jq is what reads the config at runtime; without it there is nothing to assert
  fi
  #an unplayable name must never be handed to the player, and the catalogue must match the platform
  sound_fallback="$(bash -c "source '$REPO_DIR/src/lib/gui.sh'; COMPLETION_SOUND=NotARealSound wor_completion_sound")"
  sound_explicit="$(bash -c "source '$REPO_DIR/src/lib/gui.sh'; COMPLETION_SOUND=\"\$(wor_sound_options | sed -n 2p | cut -f1)\" wor_completion_sound")"
  sound_platform_default="$(bash -c "source '$REPO_DIR/src/lib/gui.sh'; wor_sound_default")"
  [ "$sound_cfg_ok" == 1 ] \
    && [ -n "$sound_platform_default" ] \
    && [ "$sound_fallback" == "$sound_platform_default" ] \
    && [ "$sound_explicit" != "$sound_platform_default" ] \
    && grep -qF "set_bool_if_unset \"PLAY_SOUND\"" "$REPO_DIR/install-wor.sh" \
    && grep -qF ".notifications.sounds.macos" "$REPO_DIR/install-wor.sh" \
    && grep -qF ".notifications.sounds.linux" "$REPO_DIR/install-wor.sh" \
    && pass "the completion sound is configurable per platform and rejects an unplayable name" \
    || fail "the completion sound is not configurable, or an unplayable name reaches the player"

  #both front-ends must offer the same on/off switch and the same platform-appropriate menu
  grep -qF "playSoundCheckbox = \$.NSButton.checkboxWithTitleTargetAction('Play a sound when the flash finishes'" "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'soundPopup = $.NSPopUpButton.alloc.initWithFramePullsDown' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF "fields+=(\"--field=Play a sound when the flash finishes\":CHK" "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'fields+=("--field=Completion sound":CB "$sound_items")' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'const soundName = imagePath.length > 0 ? successSound : ' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'if (soundName.length > 0) {' "$REPO_DIR/install-wor-gui.sh" \
    && pass "both front-ends can turn the completion sound off and choose which one plays" \
    || fail "the completion sound cannot be turned off or chosen in both front-ends"

  #a flash is long enough to walk away from, so the desktop reports the result even when the app is
  #buried. A denied permission or a missing notifier must never turn a finished flash into a failure.
  notify_ok=0
  if command -v jq >/dev/null ;then
    [ "$(jq -r '.notifications.showNotification' "$REPO_DIR/config-templates/config.json")" == true ] \
      && [ "$(jq -r '.properties.notifications.properties | has("showNotification")' "$REPO_DIR/config-templates/config.schema.json")" == true ] \
      && notify_ok=1
  else
    notify_ok=1
  fi
  [ "$notify_ok" == 1 ] \
    && grep -qF 'wor_show_result_notification() {' "$REPO_DIR/src/lib/gui.sh" \
    && grep -qF "set_bool_if_unset \"SHOW_NOTIFICATION\"" "$REPO_DIR/install-wor.sh" \
    && grep -qF 'wor_show_result_notification success' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'wor_show_result_notification failure' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF "notificationCheckbox = \$.NSButton.checkboxWithTitleTargetAction('Show a notification when the flash finishes'" "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF "fields+=(\"--field=Show a notification when the flash finishes\":CHK" "$REPO_DIR/install-wor-gui.sh" \
    && [ "$(bash -c "source '$REPO_DIR/src/lib/metadata.sh'; source '$REPO_DIR/src/lib/gui.sh'; SHOW_NOTIFICATION=0 wor_show_result_notification success >/dev/null 2>&1; echo \$?")" == 0 ] \
    && [ "$(bash -c "source '$REPO_DIR/src/lib/metadata.sh'; source '$REPO_DIR/src/lib/gui.sh'; PATH=/nonexistent wor_show_result_notification failure >/dev/null 2>&1; echo \$?")" == 0 ] \
    && pass "finishing a flash posts a desktop notification without being able to fail the run" \
    || fail "the flash result is not announced, or its notification can fail the run"

  #a device path reaching AppleScript as source rather than argv would let it close the string and run
  grep -qF 'display notification (item 2 of argv) with title (item 1 of argv)' "$REPO_DIR/src/lib/gui.sh" \
    && ! grep -qE 'display notification "\$' "$REPO_DIR/src/lib/gui.sh" \
    && pass "the notification passes its text as argv, never as AppleScript source" \
    || fail "the notification interpolates text into AppleScript source"

  #a resume must never be taken on trust: the files the disk step reads have to be there, and the
  #step counter has to continue rather than restarting the progress bar at step 1
  resume_dir="$(mktemp -d)"
  mkdir -p "$resume_dir/winfiles_22631.2861_en-us/bootpart" "$resume_dir/peinstaller/efi" \
    "$resume_dir/peinstaller/winpe/2" "$resume_dir/pi4-uefipackage" "$resume_dir/driverpackage"
  touch "$resume_dir/winfiles_22631.2861_en-us/alldone"
  echo data > "$resume_dir/winfiles_22631.2861_en-us/install.wim"
  resume_with_files="$(run_in_engine 'cd "'"$resume_dir"'"; flash_files_already_prepared && printf %s "ready:$winfiles" || printf missing')"
  rm -rf "$resume_dir/driverpackage"
  resume_without_drivers="$(run_in_engine 'cd "'"$resume_dir"'"; flash_files_already_prepared && printf ready || printf missing')"
  resume_pi5_no_drivers="$(run_in_engine 'cd "'"$resume_dir"'"; RPI_MODEL=5; mkdir -p pi5-uefipackage; flash_files_already_prepared && printf ready || printf missing')"
  [ "$resume_with_files" == 'ready:winfiles_22631.2861_en-us' ] \
    && [ "$resume_without_drivers" == missing ] \
    && [ "$resume_pi5_no_drivers" == ready ] \
    && grep -qF '[ "$RPI_MODEL" == 5 ] && STEP_NUM=3 || STEP_NUM=4' "$REPO_DIR/install-wor.sh" \
    && pass "a resume is refused unless every prepared file is still present" \
    || fail "a resume can skip preparation when files are missing"
  rm -rf "$resume_dir"

  #Tool output can include raw bytes; macOS sed must not reject the failure log before its dialog appears.
  grep -qF "LC_ALL=C sed 's/\\x1b\\[[0-9;]*[A-Za-z]//g; s/\\r//g' \"\$1\"" "$REPO_DIR/install-wor-gui.sh" \
    && pass "GUI failure-log sanitization tolerates non-UTF-8 tool output" \
    || fail "GUI failure-log sanitization can fail on non-UTF-8 tool output"

  #Advanced Options returns user-editable config and account fields; parse those bytes after AppKit
  #returns so malformed input cannot make BSD sed abort and close the settings flow.
  advanced_result_line="$(grep -anF 'result="$(wor_osascript -l JavaScript - "$checkbox_spec"' "$REPO_DIR/install-wor-gui.sh" | cut -d: -f1)"
  advanced_byte_mode_line="$(grep -anF '  LC_ALL=C' "$REPO_DIR/install-wor-gui.sh" | awk -F: -v start="$advanced_result_line" '$1 > start {print $1; exit}')"
  advanced_status_line="$(grep -anF 'status="$(printf' "$REPO_DIR/install-wor-gui.sh" | cut -d: -f1)"
  [ -n "$advanced_result_line" ] && [ -n "$advanced_byte_mode_line" ] && [ -n "$advanced_status_line" ] \
    && [ "$advanced_result_line" -lt "$advanced_byte_mode_line" ] && [ "$advanced_byte_mode_line" -lt "$advanced_status_line" ] \
    && pass "Advanced Options parses user-editable result bytes without locale-dependent sed failures" \
    || fail "Advanced Options can exit when BSD sed rejects a user-editable byte"

  #the bar should advance inside a step, not jump once per step
  grep -qF 'emit_gui_substep() {' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'emit_gui_task_progress() {' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'emit_gui_progress "TASK' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'with_progress_capture pv -f -N' "$REPO_DIR/install-wor.sh" \
    && [ "$(grep -cF 'with_progress_capture pv -f -N' "$REPO_DIR/install-wor.sh")" == 3 ] \
    && grep -qF 'bar.maxValue = stepTotal * 100' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'bar.doubleValue = (stepNum - 1) * 100 + within' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'taskBar = $.NSProgressIndicator.alloc.initWithFrame' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'taskBar.maxValue = 100' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'taskBar.doubleValue = Math.max(0, Math.min(100, currentPercent))' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'taskPercentLabel.stringValue = taskPercentText' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF "const taskLine = lastMatch(lines, 'TASK\\t')" "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'taskText = taskLabel' "$REPO_DIR/install-wor-gui.sh" \
    && pass "progress is captured within each step and carried into both progress bars" \
    || fail "progress still jumps a whole step at a time"

  copy_error_message="$(run_in_engine 'copy_local_file_with_progress test /tmp/does-not-exist /tmp/wor-flasher-copy-test; darwin_report_copy_failure /tmp "copy test" user' 2>&1)"
  printf '%s' "$copy_error_message" | grep -qF 'Last copy error: source file does not exist: /tmp/does-not-exist' \
    && pass "copy failure diagnostics keep the real copy error in the GUI message" \
    || fail "copy failure diagnostics hide the real copy error: $copy_error_message"
  rm -f /tmp/wor-flasher-copy-test /tmp/.wor-flasher-write-probe

  copy_io_message="$(run_in_engine 'COPY_WITH_PROGRESS_ERROR="target reported Input/output error while copying a to b"; darwin_report_copy_failure /tmp "copy test" user' 2>&1)"
  printf '%s' "$copy_io_message" | grep -qF 'target volume reported an Input/output error while writing' \
    && pass "copy failure diagnostics identify target media I/O errors" \
    || fail "copy failure diagnostics misclassify target media I/O errors: $copy_io_message"

  #sub() is a built-in awk function, so using it as a variable is a syntax error
  linux_awk="$(sed -n '/# LINUX_PROGRESS_AWK_BEGIN/,/# LINUX_PROGRESS_AWK_END/p' "$REPO_DIR/install-wor-gui.sh" | sed '1d; /^awk -F/d; $d')"
  if [ -n "$linux_awk" ] && printf 'STEP\t3\t8\tThird\nSUBSTEP\t50\nTASK\t50\tinstall.wim\n' | awk -F'\t' "$linux_awk" >/dev/null 2>&1 ;then
    [ "$(printf 'STEP\t3\t8\tThird\nSUBSTEP\t50\nTASK\t50\tinstall.wim\n' | awk -F'\t' "$linux_awk" | grep -E '^1:' | grep -v '#' | tail -n1 | cut -d: -f2)" == 31 ] \
    && printf 'STEP\t3\t8\tThird\nSUBSTEP\t50\nTASK\t50\tinstall.wim\n' | awk -F'\t' "$linux_awk" | grep -qF '1:# Overall - Step 3/8: install.wim (50%)' \
    && printf 'STEP\t3\t8\tThird\nSUBSTEP\t50\nTASK\t50\tinstall.wim\n' | awk -F'\t' "$linux_awk" | grep -qF '2:# Sub-progress: install.wim (50%)' \
      && pass "the Linux progress program runs and maps a mid-step percentage correctly" \
      || fail "the Linux progress program computes the wrong overall percentage"
  else
    fail "the Linux progress awk program has a syntax error"
  fi

  #wimverify does not consistently report byte progress, so verification needs deterministic milestones.
  grep -qF 'report_verification_task() {' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'report_verification_task 45 "Verifying boot.wim integrity"' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'report_verification_task 60 "Verifying install.wim integrity"' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'report_verification_task 100 "Written image verified"' "$REPO_DIR/install-wor.sh" \
    && pass "verification reports milestones when wimverify is silent" \
    || fail "verification can stall the GUI progress bar while wimverify is silent"

  #USE_CACHE has three values, so both GUIs need a menu, and it has to reach the installer
  grep -qF 'cachePopup.addItemWithTitle' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF "out.push(String(cachePopup.indexOfSelectedItem))" "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'case "$(printf' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF '"--field=Downloaded files":CB "$cache_items"' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF '"--field=Create an optional local Windows administrator account":CHK' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF '"--field=Windows password":H' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF '"--field=Windows locale":CB "$locale_value"' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF "awk -F': ' '{print \$1}'" "$REPO_DIR/install-wor-gui.sh" \
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
    && grep -qF 'let style = $.NSWindowStyleMaskTitled' "$REPO_DIR/src/lib/gui.sh" \
    && grep -qF 'if (options.miniaturizable !== false) style = style | $.NSWindowStyleMaskMiniaturizable' "$REPO_DIR/src/lib/gui.sh" \
    && grep -qF 'stepLabel.stringValue = stepStr' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'abort_marker="$(mktemp -u)"' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'kill_process_tree "$installer_pid"' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'Stop flashing this drive?' "$REPO_DIR/install-wor-gui.sh" \
    && pass "the progress window can be aborted, shows step x of y, and has close and minimise" \
    || fail "the progress window cannot be aborted or lacks its window controls"

  grep -qF 'const width = 680' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'const height = 330' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'const logoWidth = 56' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'const logoHeight = 173' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'const progressX = 96' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'logoView.imageScaling = $.NSImageScaleProportionallyUpOrDown' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'bar = $.NSProgressIndicator.alloc.initWithFrame($.NSMakeRect(progressX' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'noteLabel.alignment = 1' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'abortButton.frame = $.NSMakeRect(Math.round((width - abortWidth) / 2)' "$REPO_DIR/install-wor-gui.sh" \
    && pass "the macOS progress window is larger, branded, and has a centered footer" \
    || fail "the macOS progress window lost its larger branded layout"

  #clicking the Dock icon sends aevt/rapp; without a handler a minimised window can never be restored
  gui_windows="$(grep -cF 'window = worMakeWindow({' "$REPO_DIR/install-wor-gui.sh")"
  [ "$(grep -cF "'handleReopenEvent:withReplyEvent:': {" "$REPO_DIR/install-wor-gui.sh")" == "$gui_windows" ] \
    && [ "$(grep -cF 'worInstallWindowHandlers(controller)' "$REPO_DIR/install-wor-gui.sh")" == "$gui_windows" ] \
    && grep -qF '0x61657674, 0x72617070' "$REPO_DIR/src/lib/gui.sh" \
    && grep -qF 'if (window.isMiniaturized) window.deminiaturize(null)' "$REPO_DIR/install-wor-gui.sh" \
    && pass "clicking the Dock icon restores a minimised window" \
    || fail "a minimised window cannot be restored from the Dock"

  #every screen must build its window through the one shared helper, or their title bars drift apart again
  [ "$gui_windows" == 6 ] \
    && grep -qF 'window = worMakeWindow({' "$REPO_DIR/install-wor.sh" \
    && ! grep -qF 'NSWindow.alloc.initWithContentRectStyleMaskBackingDefer' "$REPO_DIR/install-wor-gui.sh" \
    && ! grep -qF 'NSWindow.alloc.initWithContentRectStyleMaskBackingDefer' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'wor_jxa_window_lib() {' "$REPO_DIR/src/lib/gui.sh" \
    && grep -qF 'function worMakeWindow(options) {' "$REPO_DIR/src/lib/gui.sh" \
    && grep -qF 'function worInstallAppMenu(app, appTitle, windowTitle, iconPath) {' "$REPO_DIR/src/lib/gui.sh" \
    && [ "$(grep -cF 'worInstallAppMenu(app, appTitle, windowTitle, iconPath)' "$REPO_DIR/install-wor-gui.sh")" == "$gui_windows" ] \
    && [ "$(grep -cF 'wor_jxa_window_lib' "$REPO_DIR/install-wor-gui.sh")" == "$gui_windows" ] \
    && pass "every window is built by the shared window helper" \
    || fail "a window is built by hand instead of the shared helper"

  #macOS does not deliver app-menu action dispatch to ANY item - custom or native, including Quit -
  #while a screen's own app.runModalForWindow session is active, and every screen sharing this menu
  #is modal; a clickable-looking item that silently does nothing is worse than an informational one
  ! grep -qF "'showAbout:'" "$REPO_DIR/src/lib/gui.sh" \
    && ! grep -qF "addItemWithTitleActionKeyEquivalent('Hide " "$REPO_DIR/src/lib/gui.sh" \
    && ! grep -qF "'unhideAllApplications:'" "$REPO_DIR/src/lib/gui.sh" \
    && ! grep -qF "addItemWithTitleActionKeyEquivalent('Quit " "$REPO_DIR/src/lib/gui.sh" \
    && grep -qF 'infoItem.enabled = false' "$REPO_DIR/src/lib/gui.sh" \
    && pass "the shared app menu shows product info instead of non-functional items during a modal screen" \
    || fail "the shared app menu still offers items that cannot respond while a modal screen is showing"

  #the launcher's own startup window uses a plain app.run() loop, not app.runModalForWindow, so its
  #app-menu items are not subject to the same modal-session dispatch restriction and can stay wired
  #to nil-target native actions the normal way
  launcher_file="$REPO_DIR/src/macos-app/Contents/MacOS/WoR-Flasher"
  grep -qF 'hideItem.enabled = true' "$launcher_file" \
    && grep -qF 'hideOthersItem.enabled = true' "$launcher_file" \
    && grep -qF 'showAllItem.enabled = true' "$launcher_file" \
    && grep -qF 'quitItem.enabled = true' "$launcher_file" \
    && ! grep -qF 'hideItem.target' "$launcher_file" \
    && pass "the launcher's own app menu explicitly enables its native app-menu actions" \
    || fail "the launcher's app menu has an action left disabled, so it silently does nothing"

  #the launcher runs before any runtime is verified, so it cannot load the helper; keep it in step by hand
  grep -qF 'window.title = windowTitle' "$REPO_DIR/src/macos-app/Contents/MacOS/WoR-Flasher" \
    && grep -qF 'NSWindowStyleMaskTitled | $.NSWindowStyleMaskMiniaturizable' "$REPO_DIR/src/macos-app/Contents/MacOS/WoR-Flasher" \
    && grep -qF 'if (zoomButton) zoomButton.hidden = true' "$REPO_DIR/src/macos-app/Contents/MacOS/WoR-Flasher" \
    && grep -qF "appMenu.addItemWithTitleActionKeyEquivalent('About ' + appTitle, 'showAbout:', '')" "$REPO_DIR/src/macos-app/Contents/MacOS/WoR-Flasher" \
    && grep -qF 'WorLauncherAboutController' "$REPO_DIR/src/macos-app/Contents/MacOS/WoR-Flasher" \
    && grep -qF 'appMenu.autoenablesItems = false' "$REPO_DIR/src/macos-app/Contents/MacOS/WoR-Flasher" \
    && grep -qF 'aboutItem.enabled = true' "$REPO_DIR/src/macos-app/Contents/MacOS/WoR-Flasher" \
    && ! grep -qF 'orderFrontStandardAboutPanel:' "$REPO_DIR/src/macos-app/Contents/MacOS/WoR-Flasher" \
    && grep -qF "appMenu.addItemWithTitleActionKeyEquivalent('Quit ' + appTitle, 'terminate:', 'q')" "$REPO_DIR/src/macos-app/Contents/MacOS/WoR-Flasher" \
    && pass "the launcher startup window matches the rest of the app" \
    || fail "the launcher startup window has drifted from the other windows"

  #the close box quits the launcher outright instead of hiding it while verification keeps running unattended
  grep -qF '$.NSWindowStyleMaskMiniaturizable | $.NSWindowStyleMaskClosable' "$REPO_DIR/src/macos-app/Contents/MacOS/WoR-Flasher" \
    && grep -qF "'windowShouldClose:': {" "$REPO_DIR/src/macos-app/Contents/MacOS/WoR-Flasher" \
    && grep -qF 'window.setDelegate(controller)' "$REPO_DIR/src/macos-app/Contents/MacOS/WoR-Flasher" \
    && pass "the launcher's close box quits instead of only hiding the window" \
    || fail "the launcher's close box is disabled or silently hides the window"

  #an option that departs from, or matches, the tested defaults must say so rather than look like every other checkbox
  grep -qF "caution: parts[3] === '1'" "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'function worAnnotateCheckbox(checkbox, label, note, color)' "$REPO_DIR/src/lib/gui.sh" \
    && grep -qF "worAnnotateCheckbox(checkbox, rows[i].label, 'Not recommended', \$.NSColor.systemRedColor)" "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF "'Recommended', \$.NSColor.systemGreenColor)" "$REPO_DIR/install-wor-gui.sh" \
    && [ "$(bash -c "source '$REPO_DIR/src/lib/gui.sh'; wor_advanced_caution uefi")" == 1 ] \
    && [ "$(bash -c "source '$REPO_DIR/src/lib/gui.sh'; wor_advanced_caution verify")" == 1 ] \
    && [ "$(bash -c "source '$REPO_DIR/src/lib/gui.sh'; wor_advanced_caution drivers")" == 0 ] \
    && pass "advanced options are marked recommended or not recommended" \
    || fail "an option that departs from the tested defaults is not flagged"

  #Quit during a flash used to exit on the spot: a bogus failure dialog, with the flash left running
  progress_block="$(sed -n "/^  progress_jxa=\"\$(wor_jxa_window_lib; cat <<'JXA'\$/,/^JXA\$/p" "$REPO_DIR/install-wor-gui.sh")"
  ! printf '%s' "$progress_block" | grep -qF '$.exit(0)' \
    && printf '%s' "$progress_block" | grep -qF 'if (confirmAbort()) app.stopModalWithCode($.NSCancelButton)' \
    && [ "$(grep -c '        \$.exit(0)' "$REPO_DIR/install-wor-gui.sh")" == 4 ] \
    && grep -qF '[ -f "$done_marker" ] || touch "$abort_marker"' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'wait "$installer_pid" 2>/dev/null' "$REPO_DIR/install-wor-gui.sh" \
    && pass "quitting mid-flash confirms first and any unexpected window exit stops the installer" \
    || fail "quitting mid-flash reports a bogus failure or leaves the installer running"

  #the completion dialog has an "Open Log" button that extracts the log path and opens it in the default editor
  completion_block="$(sed -n "/^  completion_jxa=\"\$(wor_jxa_window_lib; cat <<'JXA'\$/,/^JXA\$/p" "$REPO_DIR/install-wor-gui.sh")"
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
    && printf '%s' "$completion_block" | grep -qF 'pb.setStringForType($(logPath)' \
    && pass "completion dialog can copy the log path to the clipboard" \
    || fail "completion dialog cannot copy the log path"

  #The banner carries alpha; do not put a full white panel behind it, and never bridge Core Animation CGColor through JXA.
  printf '%s' "$completion_block" | grep -qF 'const imageView = $.NSImageView.alloc.initWithFrame' \
    && ! printf '%s' "$completion_block" | grep -qF 'imageBacking' \
    && ! printf '%s' "$completion_block" | grep -qF '.CGColor' \
    && pass "completion banner preserves its alpha without a crash-prone backing layer" \
    || fail "completion banner has an opaque or crash-prone backing layer"

  #when an error occurs in the installer, gui_error_dialog writes to WOR_GUI_ERROR_MARKER with touch+sync before showing its dialog
  gui_error_block="$(sed -n "/^gui_error_dialog() {/,/^}/p" "$REPO_DIR/install-wor.sh" | head -n 20)"
  printf '%s' "$gui_error_block" | grep -qF 'mkdir -p "$(dirname "$WOR_GUI_ERROR_MARKER")' \
    && printf '%s' "$gui_error_block" | grep -qF 'touch "$WOR_GUI_ERROR_MARKER"' \
    && printf '%s' "$gui_error_block" | grep -qF 'sync' \
    && pass "gui_error_dialog reliably creates error_marker with touch+sync before dialog" \
    || fail "gui_error_dialog does not reliably create error_marker"

  #the error marker is a synchronization signal; gui_error_dialog creates it before opening its own native dialog
  marker_check_fn="$(sed -n '/^installer_showed_own_error() {/,/^}/p' "$REPO_DIR/install-wor-gui.sh")"
  printf '%s' "$marker_check_fn" | grep -qF '[ -e "$error_marker" ]' \
    && ! printf '%s' "$marker_check_fn" | grep -qF '[ -s "$error_marker" ]' \
    && [ "$(grep -cF 'if installer_showed_own_error ;then' "$REPO_DIR/install-wor-gui.sh")" == 2 ] \
    && pass "both GUIs trust the native error marker before skipping the completion dialog" \
    || fail "GUI ignores the native error marker and can show a duplicate failure dialog"

  error_fn="$(sed -n '/^error() {/,/^}/p' "$REPO_DIR/install-wor.sh")"
  duplicate_dialog_count="$(run_in_engine 'RUN_MODE=gui; WOR_GUI_PROGRESS_FILE=/tmp/progress; gui_error_dialog() { echo duplicate-dialog; }; error broken' 2>&1 | grep -c duplicate-dialog || true)"
  printf '%s' "$error_fn" | grep -qF '[ -z "${WOR_GUI_PROGRESS_FILE:-}" ]' \
    && [ "$duplicate_dialog_count" == 0 ] \
    && pass "progress-owned GUI failures do not stack an engine modal over the progress window" \
    || fail "an installer failure can still leave a duplicate modal over the progress window"

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

  abort_marker_dir="$(mktemp -d)"
  abort_marker_path="$abort_marker_dir/abort"
  : > "$abort_marker_path"
  abort_signal_out="$(run_in_engine 'RUN_MODE=gui WOR_GUI_ABORT_MARKER="'"$abort_marker_path"'"; gui_error_dialog() { echo unexpected-error-dialog; }; handle_interrupt' 2>&1; echo "rc=$?")"
  grep -qF 'export WOR_GUI_ABORT_MARKER="${abort_marker:-}"' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'Interrupted at GUI request.' <<< "$abort_signal_out" \
    && ! grep -qF 'unexpected-error-dialog' <<< "$abort_signal_out" \
    && [ "$(tail -n1 <<< "$abort_signal_out")" == 'rc=130' ] \
    && pass "GUI abort does not stack a generic Interrupted error dialog" \
    || fail "GUI abort still shows the generic Interrupted error dialog"
  rm -rf "$abort_marker_dir"

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
  [ "$summary_labels" == 'WoR-Flasher version,Target drive,Target hardware,Operating system,Installation mode,Offline OOBE,Windows local account,Windows keyboard and regional settings,Pi 4 RAM unlock,UEFI firmware,Windows ARM64 drivers,Custom config.txt,Hide empty drives,Verify written image,Downloaded files,Dry run,Download directory,Log file,' ] \
    && [ "$(run_in_engine 'settings_summary | awk -F"\t" "NF != 2" | wc -l | tr -d " "')" == 0 ] \
    && pass "settings_summary emits one tab-separated label/value pair per setting" \
    || fail "settings_summary is missing settings or emits malformed lines: $summary_labels"

  #every toggle the Advanced Options windows offer has to be visible on the confirmation screen
    [ "$(run_in_engine 'WOR_RUN_ID=summary-test DRY_RUN=1 SKIP_IMAGE_VERIFICATION=1 USE_CACHE=2 APPLY_CUSTOM_CONFIG_TXT=0 UEFI_USE_LATEST=1 DRIVERS_USE_LATEST=0 OOBE_NETWORK_BYPASS=0 PI4_AUTO_DISABLE_3GB=0 HIDE_EMPTY_DRIVES=0 settings_summary | tail -n +2 | cut -f2 | tr "\n" "|"')" \
      == "/dev/does-not-exist|Raspberry Pi 4|Windows 11 (en-us) arm64 build 22631.2861|Install Windows onto this drive|Disabled|Windows setup will ask|Windows setup defaults|Disabled|Latest|Pinned (v0.17)|Using the firmware default|No|No (skipped)|Trust the cache without checking|Yes (no changes will be written)|/tmp/wor-test-dl|/tmp/wor-test-dl/logs/wor-flasher-summary-test.log|" ] \
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

  #the macOS confirmation screen and the CLI banner list the same settings from shared renderers
  [ "$(run_in_engine 'settings_summary_plain | sed -n 2p')" == 'Target drive: /dev/does-not-exist' ] \
    && [ "$(run_in_engine 'settings_summary | sed -n 2p')" == $'Target drive\t/dev/does-not-exist' ] \
    && [ "$(run_in_engine 'settings_summary_plain "  %-24s %s\n" | sed -n 2p')" == '  Target drive:            /dev/does-not-exist' ] \
    && grep -qF 'rows="$(settings_summary)"' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'const rows = rawRows.split' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF "settings_summary_plain '  %-24s %s" "$REPO_DIR/install-wor.sh" \
    && pass "the CLI banner and the macOS confirmation screen render one shared summary" \
    || fail "the CLI banner and the macOS confirmation screen do not share a renderer"

  #config.json loading and precedence
  cfg_test_dir="$(mktemp -d)"
  cat > "$cfg_test_dir/config.json" <<'JSON'
{
  "target": {
    "rpiModel": 5,
    "device": "/dev/sdz",
    "canInstallOnSameDrive": true
  },
  "media": {
    "winLang": "de-de",
    "bid": "22631.2861"
  },
  "execution": {
    "dryRun": true
  }
}
JSON
  cfg_test_out="$(run_in_engine "unset RPI_MODEL WIN_LANG BID DEVICE DRY_RUN; WOR_CONFIG_FILE='$cfg_test_dir/config.json' load_config_json; printf '%s|%s|%s|%s|%s\n' \"\$RPI_MODEL\" \"\$WIN_LANG\" \"\$BID\" \"\$DEVICE\" \"\$DRY_RUN\"")"
  cfg_override_out="$(run_in_engine "unset BID DEVICE DRY_RUN; RPI_MODEL=4 WIN_LANG=en-us WOR_CONFIG_FILE='$cfg_test_dir/config.json' load_config_json; printf '%s|%s|%s|%s|%s\n' \"\$RPI_MODEL\" \"\$WIN_LANG\" \"\$BID\" \"\$DEVICE\" \"\$DRY_RUN\"")"
  cfg_hook_out="$(cd "$REPO_DIR" && ./install-wor-hook.sh --config "$cfg_test_dir/config.json" summary | grep -E '^Target hardware|Operating system' | tr '\n' '|')"
  rm -rf "$cfg_test_dir"
  [ "$cfg_test_out" == "5|de-de|22631.2861|/dev/sdz|1" ] \
    && [ "$cfg_override_out" == "4|en-us|22631.2861|/dev/sdz|1" ] \
    && [ "$cfg_hook_out" == "Target hardware	Raspberry Pi 5|Operating system	Windows 11 (de-de) arm64 build 22631.2861|" ] \
    && pass "config.json populates unset variables while preserving environment overrides" \
    || fail "config.json loading failed: got '$cfg_test_out' / '$cfg_override_out' / '$cfg_hook_out'"

  #a Pi 3 or Pi 5 has no 3 GB RAM limit, so offering the line at all would be misleading
  ! run_in_engine 'RPI_MODEL=5 settings_summary' | grep -q 'Pi 4 RAM unlock' \
    && run_in_engine 'RPI_MODEL=4 settings_summary' | grep -q 'Pi 4 RAM unlock' \
    && pass "the Pi 4 RAM unlock line appears only for a Pi 4" \
    || fail "the Pi 4 RAM unlock line is shown for the wrong models"

  [ "$(run_in_engine 'for RPI_MODEL in 3 4 5 ;do uefi_pinned_version ;done | tr "\n" " "')" == "$(jq -r '.systemDefaults | [.uefiVerPi3, .uefiVerPi4, .uefiVerPi5] | join(" ") + " "' "$REPO_DIR/src/config/metadata.json")" ] \
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

  if [ "$HOST_OS" == Linux ];then
    package_arguments="$(run_in_engine 'package_installed() { return 1; }; sudo() { printf "%s\n" "$@"; }; status() { :; }; install_packages alpha beta gamma')"
    [ "$package_arguments" == $'apt\nupdate\napt\ninstall\n-yf\nalpha\nbeta\ngamma\n--no-install-recommends' ] \
      && pass "Linux dependency installation preserves individual package arguments" \
      || fail "Linux dependency installation combines package names before invoking apt"
  else
    skip "Linux dependency argument handling requires Linux"
  fi

  #the two front-ends used to keep their own export lists, so one could silently drop a setting
  exported_settings="$(run_in_engine 'SOURCE_FILE=/tmp/x.iso; set_default_config_txt; export_installer_settings
    comm -23 <(printf "%s\n" "${WOR_INSTALLER_SETTINGS[@]}" | sort -u) <(compgen -e | sort -u)')"
  [ -z "$exported_settings" ] \
    && [ "$(run_in_engine 'printf "%s\n" "${WOR_INSTALLER_SETTINGS[@]}" | sort | uniq -d | wc -l | tr -d " "')" == 0 ] \
    && pass "export_installer_settings hands the installer every collected setting exactly once" \
    || fail "these settings never reach the installer: $exported_settings"

  #shared behaviour belongs to install-wor.sh; a second copy in the GUI silently shadows it on sourcing
  duplicate_functions="$(comm -12 \
    <(grep -aoE '^[a-zA-Z_][a-zA-Z0-9_]*\(\) \{' "$REPO_DIR/install-wor.sh" | sort -u) \
    <(grep -aoE '^[a-zA-Z_][a-zA-Z0-9_]*\(\) \{' "$REPO_DIR/install-wor-gui.sh" | sort -u))"
  [ -z "$duplicate_functions" ] \
    && pass "no function is defined in both install-wor.sh and install-wor-gui.sh" \
    || fail "these functions are defined twice and will drift apart: $duplicate_functions"

  #bootstrap repair is the sole pre-source helper because it restores missing shared libraries;
  #every other GUI helper still has to come after the engine is loaded
  gui_source_line="$(grep -an 'source "$cli_script" source' "$REPO_DIR/install-wor-gui.sh" | head -n1 | cut -d: -f1)"
  gui_first_function_line="$(grep -anE '^[a-zA-Z_][a-zA-Z0-9_]*\(\) \{' "$REPO_DIR/install-wor-gui.sh" | head -n1 | cut -d: -f1)"
  gui_second_function_line="$(grep -anE '^[a-zA-Z_][a-zA-Z0-9_]*\(\) \{' "$REPO_DIR/install-wor-gui.sh" | sed -n '2p' | cut -d: -f1)"
  [ -n "$gui_source_line" ] && [ -n "$gui_first_function_line" ] && [ -n "$gui_second_function_line" ] \
    && [ "$gui_first_function_line" -lt "$gui_source_line" ] \
    && [ "$gui_source_line" -lt "$gui_second_function_line" ] \
    && grep -qF 'repair_missing_checkout_runtime() {' "$REPO_DIR/install-wor-gui.sh" \
    && [ "$(grep -ac 'source "$cli_script" source' "$REPO_DIR/install-wor-gui.sh")" == 1 ] \
    && pass "bootstrap repair runs before the GUI sources its engine; all other helpers run after" \
    || fail "a non-bootstrap GUI helper is defined before shared engine functions are available"

  #every shared name the GUI relies on has to survive as a function, on both platforms
  missing_shared="$(run_in_engine 'for fn in error warning status echo_red gui_error_dialog settings_summary \
    cache_mode_label install_mode_label uefi_pinned_version set_default_config_txt describe_device human_size \
    export_installer_settings read_config_template drive_capability validate_install_mode is_safe_target_device \
    list_bids list_bids_supported get_bid get_os_name list_langs default_win_lang list_windows_locale_options get_device_name get_size_raw \
    get_file_size setup ;do declare -F "$fn" >/dev/null || echo "$fn" ;done')"
  [ -z "$missing_shared" ] \
    && pass "install-wor.sh exports every shared function the GUI depends on" \
    || fail "the GUI calls these functions, but install-wor.sh does not define them: $missing_shared"

  #the CHECK_FOR_UPDATES/NO_UPDATE pair still gates the check, and `warning` stays defined
  [ "$(run_in_engine 'warning "update failed" 2>&1 | sed "s/\x1b\[[0-9;]*m//g"')" == 'update failed' ] \
    && [ "$(run_in_engine 'unset NO_UPDATE; CHECK_FOR_UPDATES=0 load_config_json; printf %s "$NO_UPDATE"')" == 1 ] \
    && [ "$(run_in_engine 'unset NO_UPDATE CHECK_FOR_UPDATES; load_config_json; printf %s "$NO_UPDATE"')" == 0 ] \
    && grep -qF '"checkForUpdates": true' "$REPO_DIR/config-templates/config.json" \
    && grep -qF 'set_check_updates_if_unset' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'Checking for WoR-Flasher updates...' "$REPO_DIR/install-wor.sh" \
    && grep -qF -- '--check-release' "$REPO_DIR/install-wor.sh" \
    && pass "the release check is gated by CHECK_FOR_UPDATES and NO_UPDATE" \
    || fail "the release check is no longer gated by CHECK_FOR_UPDATES and NO_UPDATE"

  #ISO acceptance used to be written out three times, so the CLI and the GUI could disagree on what is usable
  iso_dir="$(mktemp -d)"
  : > "$iso_dir/small.iso"
  : > "$iso_dir/notanimage.txt"
  #sparse, so these cost nothing on disk. 2 GB sits just under the 3 GB floor, making it a boundary test
  dd if=/dev/zero of="$iso_dir/22631.2861_ARM64_en-us.iso" bs=1 count=0 seek=$((4*1024*1024*1024)) >/dev/null 2>&1
  dd if=/dev/zero of="$iso_dir/truncated.iso" bs=1 count=0 seek=$((2*1024*1024*1024)) >/dev/null 2>&1
  valid_iso_result="$(run_in_engine 'validate_iso_file "'"$iso_dir"'/22631.2861_ARM64_en-us.iso"; echo "rc=$?"')"
  missing_iso_result="$(run_in_engine 'validate_iso_file "'"$iso_dir"'/missing.iso" >/dev/null; echo "rc=$?"')"
  extension_iso_result="$(run_in_engine 'validate_iso_file "'"$iso_dir"'/notanimage.txt" >/dev/null; echo "rc=$?"')"
  truncated_iso_result="$(run_in_engine 'validate_iso_file "'"$iso_dir"'/truncated.iso" >/dev/null; echo "rc=$?"')"
  truncated_iso_message="$(run_in_engine 'validate_iso_file "'"$iso_dir"'/truncated.iso" 2>&1')"
  engine_iso_references="$(awk '{ count += gsub(/validate_iso_file/, "&") } END { print count + 0 }' "$REPO_DIR/install-wor.sh")"
  gui_iso_references="$(awk '{ count += gsub(/validate_iso_file/, "&") } END { print count + 0 }' "$REPO_DIR/install-wor-gui.sh")"
  [ "$valid_iso_result" == 'rc=0' ] \
    && [ "$missing_iso_result" == 'rc=1' ] \
    && [ "$extension_iso_result" == 'rc=1' ] \
    && [ "$truncated_iso_result" == 'rc=1' ] \
    && [ "$truncated_iso_message" == 'This file is smaller than 3GB and is probably incomplete.' ] \
    && [ "$engine_iso_references" == 3 ] \
    && [ "$gui_iso_references" == 1 ] \
    && pass "the CLI and the GUI accept and reject exactly the same ISO files" \
    || fail "ISO validation mismatch: valid=$valid_iso_result missing=$missing_iso_result extension=$extension_iso_result truncated=$truncated_iso_result message=$truncated_iso_message references=$engine_iso_references/$gui_iso_references"

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
    && grep -qF 'linux_no_device_message()' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'No external writable target drive was found.' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'WoR-Flasher hides loop/snap devices' "$REPO_DIR/install-wor-gui.sh" \
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

  malformed_catalogue_out="$(run_in_engine 'RPI_MODEL=5; versions="<releases><version number=\"10\"><release build=\"22631.2861\"><date>2024-01-01</date>$(printf "\377")</release></version></releases>"; list_bids 10' 2>&1)"
  [[ "$malformed_catalogue_out" == *22631.2861* ]] \
    && [[ "$malformed_catalogue_out" != *'illegal byte sequence'* ]] \
    && pass "Windows release catalogue parsing tolerates a malformed response byte on macOS" \
    || fail "a malformed release catalogue byte can close the macOS wizard"

  #wiminfo --xml emits UTF-16LE with a BOM and no line breaks. Passing that directly to BSD sed
  #closes the macOS wizard with "illegal byte sequence" whenever cached Windows files are present.
  wim_locale_dir="$(mktemp -d)"
  mkdir -p "$wim_locale_dir/bin"
  touch "$wim_locale_dir/install.wim"
  printf '<WIM><IMAGE><WINDOWS><LANGUAGES><LANGUAGE>en-US</LANGUAGE><DEFAULT>en-US</DEFAULT></LANGUAGES></WINDOWS></IMAGE></WIM>' \
    | iconv -f UTF-8 -t UTF-16LE > "$wim_locale_dir/wim.xml"
  { printf '#!/bin/sh\n'; printf 'cat "$WIM_XML_FIXTURE"\n'; } > "$wim_locale_dir/bin/wiminfo"
  chmod +x "$wim_locale_dir/bin/wiminfo"
  wim_locale_out="$(run_in_engine 'WIM_XML_FIXTURE="'"$wim_locale_dir"'/wim.xml"; export WIM_XML_FIXTURE; PATH="'"$wim_locale_dir"'/bin:$PATH"; list_wim_locale_codes "'"$wim_locale_dir"'/install.wim"' 2>&1)"
  [ "$wim_locale_out" == en-us ] \
    && pass "cached WIM locale discovery decodes wiminfo UTF-16LE output before parsing" \
    || fail "cached WIM locale discovery cannot parse wiminfo XML: '$wim_locale_out'"
  rm -rf "$wim_locale_dir"

  [ "$(run_in_engine 'windows_locale_from_language_code sr-latn-rs')" == 'sr-Latn-RS' ] \
    && [ "$(run_in_engine 'list_windows_locale_options | head -n1')" == $'en-US\tEnglish (United States) (en-US)' ] \
    && run_in_engine 'WINDOWS_LOCALE_SETUP=1 WINDOWS_LOCALE=sr-Latn-RS true' \
    && grep -qF 'localeCheckbox = $.NSButton.checkboxWithTitleTargetAction' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'localePopup.enabled = enabled' "$REPO_DIR/install-wor-gui.sh" \
    && pass "Windows regional settings use dropdown locale options and accept multi-part locales" \
    || fail "Windows regional settings still depend on a freeform or incomplete locale field"

  rm -rf "$iso_dir" "$winfiles_dir"

  #one entry point, but never a guess: DISPLAY is also set over SSH and in CI, and this tool erases disks
  name="$(run_in_engine 'printf %s "$WOR_FLASHER_NAME"')"
  [ "$(cd "$REPO_DIR" && ./install-wor.sh --help | head -n1)" == "$name $(run_in_engine 'printf %s "$WOR_FLASHER_VERSION"')" ] \
    && grep -qF 'gui|--gui|-g)' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'exec "$DIRECTORY/install-wor-gui.sh" "$@"' "$REPO_DIR/install-wor.sh" \
    && ! grep -qE 'if .*-n .\$DISPLAY|command -v yad .*&&.*exec' "$REPO_DIR/install-wor.sh" \
    && [ "$(cd "$REPO_DIR" && NO_UPDATE=1 ./install-wor.sh --bogus 2>&1 | sed 's/\x1b\[[0-9;]*m//g'; echo "rc=${PIPESTATUS[0]}")" == "Unknown argument '--bogus'. Run 'install-wor.sh --help' for usage.
rc=1" ] \
    && pass "install-wor.sh gui/--gui hands over explicitly and never auto-detects a display" \
    || fail "the CLI entry point is missing, or it guesses whether to open a GUI"

  #a bug report is unactionable without knowing which version produced it
  version="$(run_in_engine 'printf %s "$WOR_FLASHER_VERSION"')"
  metadata_version="$(jq -r '.product.version' "$REPO_DIR/src/config/metadata.json")"
  [[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] \
    && [ "$name" == 'WoR-Flasher' ] \
    && [ "$version" == "$metadata_version" ] \
    && grep -qF 'wor_metadata_required product version WOR_FLASHER_VERSION' "$REPO_DIR/src/lib/metadata.sh" \
    && grep -qF 'source "$WOR_METADATA_FILE"' "$REPO_DIR/install-wor.sh" \
    && [ "$(cd "$REPO_DIR" && ./install-wor.sh --version)" == "$name $version" ] \
    && [ "$(cd "$REPO_DIR" && ./install-wor.sh -V)" == "$name $version" ] \
    && [ "$(run_in_engine 'settings_summary | head -n1')" == "$(printf '%s version\t%s' "$name" "$version")" ] \
    && pass "the version is a single semantic value, reported by --version and on every run" \
    || fail "the version is missing, malformed, or not reported: '$version'"

  #the version history and the README must not describe different releases
  grep -qE "^#$version - \S" "$REPO_DIR/install-wor.sh" \
    && grep -qE "^- \*\*$version\*\*" "$REPO_DIR/README.md" \
    && grep -qF "version-$version-" "$REPO_DIR/README.md" \
    && grep -A1 -F '<key>CFBundleShortVersionString</key>' "$REPO_DIR/src/macos-app/Contents/Info.plist" | grep -qF "<string>$version</string>" \
    && grep -A1 -F '<key>CFBundleVersion</key>' "$REPO_DIR/src/macos-app/Contents/Info.plist" | grep -qF "<string>$version</string>" \
    && pass "the version history, README and app bundle metadata all agree" \
    || fail "release metadata is out of step with WOR_FLASHER_VERSION"

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

  grep -qF '"$HOME/pi-apps/manage" install '\''More RAM'\''' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF '"$(dirname "$(cat /usr/local/bin/pi-apps | sed -n 2p)")/manage" install '\''More RAM'\''' "$REPO_DIR/install-wor-gui.sh" \
    && pass "the RAM-download option installs More RAM through Pi-Apps at default and custom locations" \
    || fail "the RAM-download option does not install More RAM through every supported Pi-Apps location"

  wsl_hook_out="$(cd "$REPO_DIR" && WSL_DISTRO_NAME=wor-test ./install-wor-hook.sh list-devices 2>&1; echo "rc=$?")"
  hook_progress_file="$(mktemp "${TMPDIR:-/tmp}/wor-hook-progress.XXXXXX")"
  rm -f "$hook_progress_file"
  hook_progress_out="$(cd "$REPO_DIR" && ./install-wor-hook.sh --progress-file "$hook_progress_file" run --version 2>&1)"
  hook_bad_progress_out="$(cd "$REPO_DIR" && ./install-wor-hook.sh --progress-file 2>&1; echo "rc=$?")"
  hook_set_summary="$(cd "$REPO_DIR" && ./install-wor-hook.sh --set RPI_MODEL=5 --set BID=26200.6899 --set WIN_LANG=de-de --set DEVICE=/dev/does-not-exist --set CAN_INSTALL_ON_SAME_DRIVE=1 summary | grep -E '^Target hardware|Operating system' | tr '\n' '|')"
  hook_set_expected=$'Target hardware	Raspberry Pi 5|Operating system	Windows 11 (de-de) arm64 build 26200.6899|'
  hook_bad_set_out="$(cd "$REPO_DIR" && ./install-wor-hook.sh --set NOT-VALID=1 summary 2>&1; echo "rc=$?")"
  hook_bootstrap_dir="$(mktemp -d)"
  mkdir -p "$hook_bootstrap_dir/source/config-templates" "$hook_bootstrap_dir/source/src/lib" "$hook_bootstrap_dir/source/src/config"
  cp "$REPO_DIR/install-wor.sh" "$hook_bootstrap_dir/source/"
  cp "$REPO_DIR/src/lib/metadata.sh" "$REPO_DIR/src/lib/dependencies.sh" "$REPO_DIR/src/lib/paths.sh" "$REPO_DIR/src/lib/cleanup.sh" "$REPO_DIR/src/lib/gui.sh" "$hook_bootstrap_dir/source/src/lib/"
  cp "$REPO_DIR/src/config/metadata.json" "$REPO_DIR/src/config/metadata.schema.json" "$hook_bootstrap_dir/source/src/config/"
  cp -R "$REPO_DIR/config-templates/." "$hook_bootstrap_dir/source/config-templates/"
  git -C "$hook_bootstrap_dir/source" init -q
  git -C "$hook_bootstrap_dir/source" add .
  git -C "$hook_bootstrap_dir/source" -c user.name=WoR-Test -c user.email=wor-test@example.invalid commit -qm fixture
  git -C "$hook_bootstrap_dir/source" branch -M fixture
  cp "$REPO_DIR/install-wor-hook.sh" "$hook_bootstrap_dir/hook.sh"
  hook_bootstrap_out="$(WOR_HOOK_REPOSITORY="$hook_bootstrap_dir/source" WOR_HOOK_REF=fixture WOR_HOOK_INSTALL_DIR="$hook_bootstrap_dir/installed" "$hook_bootstrap_dir/hook.sh" run --version 2>&1)"
  rm "$hook_bootstrap_dir/source/config-templates/prefinalize.cmd"
  git -C "$hook_bootstrap_dir/source" add -u
  git -C "$hook_bootstrap_dir/source" -c user.name=WoR-Test -c user.email=wor-test@example.invalid commit -qm incomplete-fixture
  hook_incomplete_out="$(WOR_HOOK_REPOSITORY="$hook_bootstrap_dir/source" WOR_HOOK_REF=fixture WOR_HOOK_INSTALL_DIR="$hook_bootstrap_dir/incomplete" "$hook_bootstrap_dir/hook.sh" run --version 2>&1; echo "rc=$?")"
  [ -x "$REPO_DIR/install-wor-hook.sh" ] \
    && grep -qF 'list-devices)' "$REPO_DIR/install-wor-hook.sh" \
    && grep -qF 'describe-device)' "$REPO_DIR/install-wor-hook.sh" \
    && grep -qF 'summary)' "$REPO_DIR/install-wor-hook.sh" \
    && grep -qF 'exec "$ENGINE"' "$REPO_DIR/install-wor-hook.sh" \
    && grep -qF ': "${WOR_HOOK_REF:=main}"' "$REPO_DIR/install-wor-hook.sh" \
    && grep -qF 'WOR_GUI_PROGRESS_FILE="${1#*=}"' "$REPO_DIR/install-wor-hook.sh" \
    && grep -qF 'export WOR_GUI_PROGRESS_FILE' "$REPO_DIR/install-wor-hook.sh" \
    && grep -qF 'Use --progress-file FILE with run' "$REPO_DIR/install-wor-hook.sh" \
    && grep -A2 -F 'list-devices)' "$REPO_DIR/install-wor-hook.sh" | grep -qF 'require_linux_host' \
    && ! grep -qE '(exec|source|bash)[[:space:]]+.*install-wor-hook\.sh' "$REPO_DIR/install-wor-gui.sh" \
    && [ "$(cd "$REPO_DIR" && ./install-wor-hook.sh run --version)" == "WoR-Flasher $version" ] \
    && [ "$hook_progress_out" == "WoR-Flasher $version" ] \
    && [ ! -e "$hook_progress_file" ] \
    && grep -qF 'Usage:' <<< "$hook_bad_progress_out" \
    && [ "$(tail -n1 <<< "$hook_bad_progress_out")" == 'rc=2' ] \
    && [ "$hook_set_summary" == "$hook_set_expected" ] \
    && grep -qF 'Invalid --set name:' <<< "$hook_bad_set_out" \
    && [ "$(tail -n1 <<< "$hook_bad_set_out")" == 'rc=2' ] \
    && grep -qF 'TASK<TAB>percent<TAB>label' "$REPO_DIR/README.md" \
    && [ "$(cd "$REPO_DIR" && WOR_RUN_ID=hook-test DL_DIR=/tmp/wor-test-dl RPI_MODEL=4 BID=22631.2861 WIN_LANG=en-us DEVICE=/dev/does-not-exist CAN_INSTALL_ON_SAME_DRIVE=1 ./install-wor-hook.sh summary)" == "$(run_in_engine 'WOR_RUN_ID=hook-test settings_summary')" ] \
    && [ "$(cd "$REPO_DIR" && ./install-wor-hook.sh describe-device 2>/dev/null; echo $?)" == 2 ] \
    && grep -qF 'WoR-Flasher does not support WSL.' <<< "$wsl_hook_out" \
    && [ "$(tail -n1 <<< "$wsl_hook_out")" == 'rc=1' ] \
    && grep -qF 'Obtaining WoR-Flasher' <<< "$hook_bootstrap_out" \
    && grep -qE 'WoR-Flasher [0-9]+\.[0-9]+\.[0-9]+$' <<< "$hook_bootstrap_out" \
    && [ -f "$hook_bootstrap_dir/installed/install-wor.sh" ] \
    && [ -d "$hook_bootstrap_dir/installed/config-templates" ] \
    && grep -qF 'The obtained WoR-Flasher checkout is incomplete.' <<< "$hook_incomplete_out" \
    && [ "$(tail -n1 <<< "$hook_incomplete_out")" == 'rc=1' ] \
    && [ ! -e "$hook_bootstrap_dir/incomplete" ] \
    && pass "integration adapter exposes the shared installer engine" \
    || fail "integration adapter is missing or does not use the shared engine"
  rm -f "$hook_progress_file"
  rm -rf "$hook_bootstrap_dir"

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
  [[ "$(run_in_engine 'wor_log_file')" == /tmp/wor-test-dl/logs/wor-flasher-*.log ]] \
    && [ "$(run_in_engine 'wor_last_log_file')" == '/tmp/wor-test-dl/last-run.log' ] \
    && [ "$(run_in_engine 'WOR_LOG_FILE=/tmp/elsewhere.log; wor_log_file')" == '/tmp/elsewhere.log' ] \
    && [[ "$(run_in_engine 'DL_DIR=/tmp/moved-later; wor_log_file')" == /tmp/moved-later/logs/wor-flasher-*.log ]] \
    && grep -qF 'WOR_RUN_ID' "$REPO_DIR/install-wor.sh" \
    && [ "$(run_in_engine 'WOR_RUN_ID=shared-log-id; settings_summary | sed -n "s/^Log file\t//p"')" == '/tmp/wor-test-dl/logs/wor-flasher-shared-log-id.log' ] \
    && grep -qF 'cp "$saved_log" "$last_log"' "$REPO_DIR/install-wor-gui.sh" \
    && [ "$(grep -cF 'last-run.log' "$REPO_DIR/install-wor-gui.sh")" == 0 ] \
    && pass "one variable decides where the timestamped run log goes, with last-run kept for support" \
    || fail "the log path is hardcoded, does not follow DL_DIR/WOR_LOG_FILE, or loses last-run"

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
  #Path is capped at 259 characters by the Windows unattend schema, and a non-zero script result
  #fails Windows Setup outright, so invoke the staged file and let it swallow and log its own errors.
  ram_command="$(sed -n 's/.*<Path>\(.*\)<\/Path>.*/\1/p' "$REPO_DIR/config-templates/pi4-ram-unlock-specialize.xml")"
  [ -z "$missing_ram_hook" ] \
    && grep -qF 'Setup\Scripts\Pi4Disable3GB.ps1' "$REPO_DIR/config-templates/pi4-ram-unlock-specialize.xml" \
    && [ "${#ram_command}" -le 259 ] \
    && grep -qF '} catch {' "$REPO_DIR/config-templates/pi4-ram-unlock.ps1" \
    && grep -qF "Set-Content \$log ('Pi 4 RAM unlock failed: ' + \$_.Exception.Message)" "$REPO_DIR/config-templates/pi4-ram-unlock.ps1" \
    && grep -qF 'runtime variable was changed successfully' "$REPO_DIR/README.md" \
    && grep -qF 'exit 0' "$REPO_DIR/config-templates/pi4-ram-unlock.ps1" \
    && ! grep -qF 'exit 1' "$REPO_DIR/config-templates/pi4-ram-unlock.ps1" \
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
  sudo losetup --partscan "$dev" "$img" || die "Failed to attach $img to $dev"
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

  #minimal containers have no device manager to create loop partition nodes
  local partprobe_wrapper
  partprobe_wrapper="$(mktemp)"
  cat > "$partprobe_wrapper" <<'PARTPROBE'
#!/bin/bash
/usr/sbin/partprobe "$@" || exit $?
for device in "$@" ;do
  device_name="${device##*/}"
  for partition in /sys/class/block/"$device_name"/"$device_name"p* ;do
    [ -e "$partition/dev" ] || continue
    node="/dev/${partition##*/}"
    IFS=: read -r major minor < "$partition/dev"
    [ -b "$node" ] || mknod "$node" b "$major" "$minor"
  done
done
PARTPROBE
  sudo install -m 0755 "$partprobe_wrapper" /usr/local/bin/partprobe
  rm -f "$partprobe_wrapper"
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
      #fold the container result into this run's tally: exiting on it alone would discard
      #every host-side failure recorded above, reporting a red run as green
      if "$REPO_DIR/tests/run-linux-integration.sh" "$@" ;then
        pass "Linux integration suite completed"
      else
        fail "Linux integration suite failed"
      fi
      summary
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
    && grep -qF 'darwin_prepare_disk_or_die "$DEVICE" "$sgdisk_bin" "$boot_size_mb" "$install_size_mb" "$PART1" "$PART2"' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'darwin_prepare_disk_or_die "$DEVICE" "$sgdisk_bin" "$boot_size_mb" "$install_size_mb" "$PART1" "$PART2"'$'\n''  gui_start_sudo_keepalive' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'darwin_mount_partition_or_die "$PART1"' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'darwin_mount_partition_or_die "$PART2"' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'darwin_finalize_partition_types_or_die "$DEVICE" "$sgdisk_bin"' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'darwin_verify_final_partition_types_or_die "$PART1" "$PART2"' "$REPO_DIR/install-wor.sh" \
    && grep -qF '[ "$boot_content" == EFI ] || [ "$boot_content" == "Microsoft Basic Data" ]' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'Final media verification failed: partition 1 is' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'for attempt in 1 2 3 4 5 6 7 8 9 10 ;do' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'sudo bash -s -- "$device" "$sgdisk_bin" "$boot_size_mb" "$install_size_mb" "$part1" "$part2"' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'darwin_report_copy_failure "$boot_mount"' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'sudo -n touch "$probe"' "$REPO_DIR/install-wor.sh" \
    && grep -qF '/usr/sbin/diskutil eraseVolume MS-DOS WOR_BOOT "$1"' "$REPO_DIR/install-wor.sh" \
    && grep -qF '/usr/sbin/diskutil eraseVolume ExFAT WOR_INSTALL "$2"' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'command_output="$(sudo bash -s -- "$boot_partition" "$install_partition"' "$REPO_DIR/install-wor.sh" \
    && ! grep -qF 'command_output="$(command sudo diskutil eraseVolume' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'less than 1 GiB remains unallocated for the Windows target partition' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'verify_written_image "$DEVICE" "$PART1" "$PART2" "$boot_mount" "$win_mount"' "$REPO_DIR/install-wor.sh" \
    && pass "Darwin creates WOR_BOOT as a real EFI System Partition" \
    || fail "Darwin does not type WOR_BOOT as an EFI System Partition"
  #a raw-disk-write denial reads as a bare "Operation not permitted"; the helper must
  #recognize that specific text and point the user at Full Disk Access instead of guessing
  fda_message="$(run_in_engine 'fake_cmd() { echo "newfs_msdos: /dev/rdisk9s1: Operation not permitted" >&2; return 1; }; open() { :; }; darwin_format_or_die "format the boot partition on /dev/disk9s1" fake_cmd' 2>&1)"
  echo "$fda_message" | grep -qF 'Full Disk Access' \
    && echo "$fda_message" | grep -qF 'click the + button' \
    && pass "darwin_format_or_die recognizes an Operation not permitted denial and explains adding the app via +" \
    || fail "darwin_format_or_die gave no Full Disk Access guidance for a permission denial"
  fda_message_native="$(run_in_engine 'export WOR_NATIVE_APP=1; fake_cmd() { echo "newfs_msdos: /dev/rdisk9s1: Operation not permitted" >&2; return 1; }; open() { :; }; darwin_format_or_die "format the boot partition on /dev/disk9s1" fake_cmd' 2>&1)"
  echo "$fda_message_native" | grep -qF 'find WoR-Flasher.app' \
    && pass "darwin_format_or_die names WoR-Flasher.app when launched as the native app" \
    || fail "darwin_format_or_die did not name WoR-Flasher.app under WOR_NATIVE_APP=1"
  fda_message_native_path="$(run_in_engine 'export WOR_NATIVE_APP=1 WOR_APP_BUNDLE_PATH=/Applications/WoR-Flasher.app; fake_cmd() { echo "newfs_msdos: /dev/rdisk9s1: Operation not permitted" >&2; return 1; }; open() { :; }; darwin_format_or_die "format the boot partition on /dev/disk9s1" fake_cmd' 2>&1)"
  echo "$fda_message_native_path" | grep -qF 'WoR-Flasher.app at /Applications/WoR-Flasher.app' \
    && pass "darwin_format_or_die includes the exact native app path when available" \
    || fail "darwin_format_or_die did not include WOR_APP_BUNDLE_PATH in the Full Disk Access message"
  fda_message_vscode="$(run_in_engine 'export WOR_NATIVE_APP=1 WOR_APP_BUNDLE_PATH=/Applications/WoR-Flasher.app TERM_PROGRAM=vscode; fake_cmd() { echo "newfs_msdos: /dev/rdisk9s1: Operation not permitted" >&2; return 1; }; open() { :; }; darwin_format_or_die "format the boot partition on /dev/disk9s1" fake_cmd' 2>&1)"
  echo "$fda_message_vscode" | grep -qF 'also enable Visual Studio Code.app' \
    && pass "darwin_format_or_die names the launcher app when macOS may attribute FDA to it" \
    || fail "darwin_format_or_die did not include a launcher fallback in the Full Disk Access message"
  removable_message="$(run_in_engine 'HOST_OS=Darwin; open() { :; }; touch() { echo "touch: /Volumes/WOR_BOOT/.wor-flasher-write-probe: Operation not permitted" >&2; return 1; }; darwin_require_mounted_volume_access /Volumes/WOR_BOOT "write to /Volumes/WOR_BOOT"' 2>&1)"
  echo "$removable_message" | grep -qF 'Removable Volumes' \
    && echo "$removable_message" | grep -qF 'Full Disk Access' \
    && echo "$removable_message" | grep -qF 'Visual Studio Code.app, Terminal.app, or iTerm.app' \
    && grep -qF 'darwin_require_mounted_volume_access "$boot_mount" "write to $boot_mount"' "$REPO_DIR/install-wor.sh" \
    && pass "macOS preflights removable-volume privacy before copying mounted media" \
    || fail "macOS mounted-media TCC guidance or preflight is missing"
  generic_message="$(run_in_engine 'fake_cmd() { echo "some other disk error" >&2; return 1; }; open() { :; }; darwin_format_or_die "format the boot partition on /dev/disk9s1" fake_cmd' 2>&1)"
  echo "$generic_message" | grep -qF 'Full Disk Access' \
    && fail "darwin_format_or_die wrongly blames Full Disk Access for an unrelated failure" \
    || pass "darwin_format_or_die does not misattribute an unrelated failure to Full Disk Access"
  grep -qF 'open "x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles"' "$REPO_DIR/install-wor.sh" \
    && grep -qF 'darwin_diskutil_format_pair_or_die()' "$REPO_DIR/install-wor.sh" \
    && pass "macOS formats partitions through diskutil instead of raw partition writers" \
    || fail "macOS still formats partitions through the raw disk writer path"
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

######## Every model gets a fresh model-specific run; the common PE installer is reused

#sourcing install-wor.sh sets IFS to a newline, so split the model list explicitly
pe_installer_populated=0
for model in $(tr ' ' '\n' <<<"$TEST_MODELS") ;do
  info "== Raspberry Pi $model, fresh run =="
  bid="$(RPI_MODEL=$model get_bid 11)"
  if [ -z "$bid" ];then
    skip "no build id detected for a Pi $model"
    continue
  fi
  echo "  using build $bid"
  seed_winfiles "$bid"

  run_flasher TERM=unknown BID="$bid" RPI_MODEL="$model" DEVICE="$DEV_INSTALL" CAN_INSTALL_ON_SAME_DRIVE=1 USE_CACHE=1
  expect_ok "Pi $model dry run completes"
  if [ "$pe_installer_populated" == 0 ];then
    expect_output "Pi $model downloads the PE installer" "Downloading WoR PE-based installer"
    pe_installer_populated=1
  else
    expect_output "Pi $model reuses the shared PE installer" "cached copy is up to date"
  fi
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
expect_output "USE_CACHE=1 keeps the current ones" "pi4-uefipackage - cached copy is up to date"

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
expect_output "automatic recovery mode is recorded" "Installation mode:       Recovery drive for another >16 GB drive"
expect_no_output "recovery-sized drive does not ask for install mode" "Choose the installation mode"

run_flasher_with_input '2\n' BID="$GOOD_BID" RPI_MODEL=4 DEVICE="$DEV_INSTALL" USE_CACHE=2
expect_ok "large drive can be used as a recovery drive"
expect_output "large-drive recovery choice is recorded" "Installation mode:       Recovery drive for another >16 GB drive"

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

info "== Update check =="
#The tool must never rewrite its own installation. A half-updated disk flasher is far more
#dangerous than an out-of-date one, so the release check is read-only by construction.
if grep -qE 'git (fetch|merge|pull|reset|clean|restore)' "$REPO_DIR/install-wor.sh" ;then
  fail "install-wor.sh still runs a mutating git command; the engine must not update itself"
else
  pass "install-wor.sh runs no mutating git command"
fi

#repair_missing_files legitimately restores missing tracked files from the LOCAL revision,
#so the launcher may still call git restore. What it must not do is fetch or merge a remote.
if grep -qE 'git (-C "\$REPO_DIR" )?(fetch|merge|pull|ls-remote)' "$REPO_DIR/src/macos-app/Contents/MacOS/WoR-Flasher" ;then
  fail "the macOS launcher still fetches or merges from a git remote"
else
  pass "the macOS launcher never fetches or merges from a git remote"
fi

grep -qF -- '--check-git' "$REPO_DIR/src/updater.mjs" "$REPO_DIR/package.json" \
  && fail "the retired --check-git updater mode is still referenced" \
  || pass "the retired --check-git updater mode is gone"

grep -qF 'execSync' "$REPO_DIR/src/lib/node-runtime.mjs" \
  && fail "the Node runtime library still shells out" \
  || pass "the Node runtime library shells out to nothing"

if ! command -v node >/dev/null ;then
  skip "the release-check CLI needs node"
  skip "the release check needs node to prove it leaves a checkout untouched"
else
  #an unroutable slug makes the request fail without touching the network fixture
  update_out="$(node "$REPO_DIR/src/updater.mjs" --check-release "--repo-dir=$REPO_DIR" --repo-slug='../evil' 2>/dev/null)"
  [ "$update_out" == UNKNOWN ] \
    && pass "the release check refuses an unsafe repository slug" \
    || fail "the release check did not reject an unsafe repository slug (got '$update_out')"

  if ! command -v git >/dev/null || ! git -C "$REPO_DIR" rev-parse --git-dir >/dev/null 2>&1 ;then
    skip "proving the checkout is left untouched needs a git checkout"
  else
    branch="$(git -C "$REPO_DIR" rev-parse --abbrev-ref HEAD)"
    if [ "$branch" == HEAD ];then
      skip "proving the checkout is left untouched needs a branch, not a detached HEAD"
    else
      #a disposable clone, deliberately left one commit behind - never runs against $REPO_DIR itself
      clone_dir="$TEST_DIR/update-check-clone"
      rm -rf "$clone_dir"
      if ! git clone -q --branch "$branch" "$REPO_DIR" "$clone_dir" 2>/dev/null ;then
        fail "could not clone $REPO_DIR (branch $branch) to test the update check"
      elif ! git -C "$clone_dir" reset -q --hard HEAD~1 2>/dev/null ;then
        skip "$branch has no earlier commit to fall behind"
      else
        behind_commit="$(git -C "$clone_dir" rev-parse HEAD)"
        printf '\n' >> "$clone_dir/README.md"
        LAST_OUT="$(run_with_timeout env NO_UPDATE=0 ROOT_DEV=/dev/__wor_flasher_test_root__ \
          DL_DIR="$TEST_DL_DIR" WIN_LANG="$TEST_WIN_LANG" RUN_MODE=cli DRY_RUN=1 BID="$GOOD_BID" \
          RPI_MODEL=4 DEVICE="$DEV_INSTALL" CAN_INSTALL_ON_SAME_DRIVE=1 USE_CACHE=2 \
          "$clone_dir/install-wor.sh" 2>&1)"
        LAST_CODE=$?
        expect_ok "a checkout behind its remote still runs"
        expect_no_output "the engine never announces a self-update" "Auto-updating wor-flasher"
        expect_no_output "the engine never reloads itself after updating" "Reloading script"
        [ "$(git -C "$clone_dir" rev-parse HEAD)" == "$behind_commit" ] \
          && pass "the update check left the checkout on its original commit" \
          || fail "the update check moved the checkout to another commit"
        git -C "$clone_dir" diff --quiet -- README.md \
          && fail "the update check discarded local changes" \
          || pass "the update check preserved local changes"
      fi
    fi
  fi
fi

info "== no spawned terminal emulator =="
#the GUI runs the engine in-process, so no terminal launcher may return to the tree
[ ! -e "$REPO_DIR/terminal-run" ] \
  && pass "terminal-run is not present" || fail "terminal-run reappeared; the GUI must not spawn a terminal"

######## Summary

echo
summary
