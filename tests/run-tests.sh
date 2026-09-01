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
  git -C "$REPO_DIR" diff --check >/dev/null 2>&1 \
    && pass "working tree has no whitespace errors" || fail "working tree has whitespace errors"
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

  grep -qF 'sources/install.esd' "$REPO_DIR/install-wor.sh" \
    && pass "ISO import accepts install.esd media" \
    || fail "ISO import does not accept install.esd media"

  grep -qF 'macos_start_cli()' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'Choose Windows version' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'Choose Raspberry Pi model' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'Choose Windows language' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'darwin_list_device_paths' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'Choose installation mode' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'All data on the target drive will be erased.' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'do script (item 1 of argv)' "$REPO_DIR/install-wor-gui.sh" \
    && pass "macOS GUI collects choices and hands off to the Terminal CLI" \
    || fail "macOS GUI handoff is missing"
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

run_with_timeout() {
  if command -v timeout >/dev/null ;then
    timeout "$TEST_COMMAND_TIMEOUT" "$@"
  else
    "$@"
  fi
}

run_flasher() { #Input: VAR=VALUE pairs. Sets LAST_OUT and LAST_CODE.
  local output_file
  progress "running install-wor.sh with $(printf '%s ' "$@")"
  progress "live installer output follows (timeout: ${TEST_COMMAND_TIMEOUT}s)"
  output_file="$TEST_DIR/flasher-output.log"
  #stdin is closed so an unexpected prompt (e.g. the root-user confirmation) fails fast instead of hanging
  (cd "$TEST_DIR" && run_with_timeout env ROOT_DEV=/dev/__wor_flasher_test_root__ DL_DIR="$TEST_DL_DIR" WIN_LANG="$TEST_WIN_LANG" RUN_MODE=cli DRY_RUN=1 SKIP_PACKAGE_INSTALL="${SKIP_PACKAGE_INSTALL:-0}" "$@" "$REPO_DIR/install-wor.sh" </dev/null) 2>&1 | tee "$output_file" 1>&2
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
  DARWIN_DEVICE_INFO='{"WholeDisk":true,"Internal":false,"VirtualOrPhysical":"Physical","ReadOnlyMedia":false}'
  darwin_plist_json() {
    if [ "$2" == list ];then
      printf '%s\n' '{"AllDisks":["disk2"]}'
    else
      printf '%s\n' "$DARWIN_DEVICE_INFO"
    fi
  }
  is_safe_target_device /dev/disk2 && pass "Darwin accepts an external physical writable disk" || fail "Darwin rejected an external physical writable disk"
  is_safe_target_device /dev/disk0 && fail "Darwin accepted the startup disk" || pass "Darwin rejects the startup disk"
  [ "$(darwin_list_device_paths)" == /dev/disk2 ] && pass "Darwin GUI lists safe external disks" || fail "Darwin GUI listed an unexpected disk"
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
  done
done

######## Cache modes, which need a populated cache to test against

info "== Cache modes =="
run_flasher BID="$GOOD_BID" RPI_MODEL=4 DEVICE="$DEV_INSTALL" CAN_INSTALL_ON_SAME_DRIVE=1 USE_CACHE=0
run_flasher BID="$GOOD_BID" RPI_MODEL=4 DEVICE="$DEV_INSTALL" CAN_INSTALL_ON_SAME_DRIVE=1 USE_CACHE=1
expect_output "USE_CACHE=1 reuses a current cache" "cached copy is up to date"
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
