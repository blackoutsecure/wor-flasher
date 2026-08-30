#!/bin/bash

#Test harness for wor-flasher. Creates loopback devices to stand in for real drives,
#so nothing can be written to physical storage. Every run is a fresh download.
#
#Usage:
#  ./tests/run-tests.sh               run the automated suite
#  ./tests/run-tests.sh --walkthrough create fake drives, then run the CLI interactively
#  ./tests/run-tests.sh --gui         create fake drives, then launch the GUI (needs a display)
#  ./tests/run-tests.sh --full        also download the real Windows image (several GB)
#  ./tests/run-tests.sh --keep        leave the fake drives and downloads in place afterwards
#  ./tests/run-tests.sh --clean       remove the test workspace and detach fake drives

######## Defaults. Every one can be overridden from the environment.

script_dir() {
  if command -v realpath >/dev/null ;then
    realpath "$1"
  else
    (cd "$1" && pwd -P)
  fi
}

REPO_DIR="$(script_dir "$(dirname "$0")/..")"

#install-wor.sh runs 'git restore .' before self-updating, which would discard local edits.
#Exporting this here covers every invocation below, including the sourced library functions.
#The "Self-updater" test further down deliberately overrides this to exercise the real thing,
#but only against a disposable clone, never against this working tree.
export NO_UPDATE=1

#Everything this script creates lives here. It is listed in .gitignore.
[ -z "$TEST_DIR" ] && TEST_DIR="$REPO_DIR/.test-workspace"
[ -z "$TEST_DL_DIR" ] && TEST_DL_DIR="$TEST_DIR/downloads"

[ -z "$TEST_WIN_LANG" ] && TEST_WIN_LANG='en-us'

#Which Raspberry Pi models to exercise. Each one pulls a different UEFI package.
[ -z "$TEST_MODELS" ] && TEST_MODELS='3 4 5'

#Loopback images are sparse, so a larger size costs nothing on disk. These are the exact
#thresholds drive_capability() switches on, which makes them boundary tests rather than
#arbitrary sizes.
[ -z "$SIZE_TOO_SMALL" ] && SIZE_TOO_SMALL=1G #under the 8GB minimum, must be refused
[ -z "$SIZE_RECOVERY" ] && SIZE_RECOVERY=8G   #exactly the recovery threshold
[ -z "$SIZE_INSTALL" ] && SIZE_INSTALL=25G    #exactly the self-install threshold

######## End of defaults

PASSED=0
FAILED=0
SKIPPED=0
LOOP_DEVICES=()
LAST_OUT=''
LAST_CODE=0
KEEP=0
MODE=suite
SKIP_ESD=1

pass() { printf '  \e[92mPASS\e[0m  %s\n' "$1"; PASSED=$((PASSED+1)); }
fail() { printf '  \e[91mFAIL\e[0m  %s\n' "$1"; FAILED=$((FAILED+1)); }
skip() { printf '  \e[93mSKIP\e[0m  %s\n' "$1"; SKIPPED=$((SKIPPED+1)); }
info() { printf '\e[96m%s\e[0m\n' "$1"; }
die()  { printf '\e[91m%s\e[0m\n' "$1" 1>&2; exit 1; }

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
}

summary() {
  echo
  printf 'passed %s, failed %s, skipped %s\n' "$PASSED" "$FAILED" "$SKIPPED"
  [ "$FAILED" -gt 0 ] && exit 1
  exit 0
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

run_flasher() { #Input: VAR=VALUE pairs. Sets LAST_OUT and LAST_CODE.
  LAST_OUT="$(env DL_DIR="$TEST_DL_DIR" WIN_LANG="$TEST_WIN_LANG" RUN_MODE=cli DRY_RUN=1 "$@" "$REPO_DIR/install-wor.sh" 2>&1)"
  LAST_CODE=$?
}

run_flasher_with_input() { #Input: stdin text, then VAR=VALUE pairs. Sets LAST_OUT and LAST_CODE.
  local input="$1"
  shift
  LAST_OUT="$(printf '%b' "$input" | env DL_DIR="$TEST_DL_DIR" WIN_LANG="$TEST_WIN_LANG" RUN_MODE=cli DRY_RUN=1 "$@" "$REPO_DIR/install-wor.sh" 2>&1)"
  LAST_CODE=$?
}

expect_ok()   { [ "$LAST_CODE" == 0 ] && pass "$1" || fail "$1 (exit $LAST_CODE)"; }
expect_fail() { [ "$LAST_CODE" != 0 ] && pass "$1" || fail "$1 (unexpectedly succeeded)"; }
expect_output()    { grep -qF "$2" <<<"$LAST_OUT" && pass "$1" || fail "$1 (did not find: $2)"; }
expect_no_output() { grep -qF "$2" <<<"$LAST_OUT" && fail "$1 (unexpectedly found: $2)" || pass "$1"; }

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

static_checks

if [ "$(uname -s)" != Linux ];then
  skip "integration tests require Linux loop devices"
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

  run_flasher BID="$bid" RPI_MODEL="$model" DEVICE="$DEV_INSTALL" CAN_INSTALL_ON_SAME_DRIVE=1 USE_CACHE=0
  expect_ok "Pi $model dry run completes"
  expect_output "Pi $model downloads the PE installer" "Downloading WoR PE-based installer"
  expect_output "Pi $model downloads UEFI firmware" "UEFI firmware"
  expect_output "Pi $model stops before flashing" "DRY_RUN"

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
handoff_out="$(
  CONFIG_TXT='arm_64bit=1
# a "quoted" line with $(touch '"$TEST_DIR"'/INJECTED) and `touch '"$TEST_DIR"'/INJECTED2`
armstub=RPI_EFI.fd'
  cli_script="$REPO_DIR/install-wor.sh"
  export CONFIG_TXT cli_script
  env_file="$(mktemp)"
  declare -p CONFIG_TXT cli_script > "$env_file"
  env -i /bin/bash -c "source '$env_file'; printf '%s\n' \"\$cli_script\"; echo \"\$CONFIG_TXT\" | wc -l"
  rm -f "$env_file"
)"
[ "$(head -n1 <<<"$handoff_out")" == "$REPO_DIR/install-wor.sh" ] \
  && pass "values survive a terminal that does not inherit the environment" \
  || fail "values were lost in a terminal that does not inherit the environment"
[ "$(tail -n1 <<<"$handoff_out")" == 3 ] && pass "a multi-line CONFIG_TXT stays intact" || fail "CONFIG_TXT was mangled"
if [ -e "$TEST_DIR/INJECTED" ] || [ -e "$TEST_DIR/INJECTED2" ];then
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
      update_env=(UPDATE_REPO_URL="$REPO_DIR" UPDATE_REF="$branch" NO_UPDATE=0 DL_DIR="$TEST_DL_DIR" WIN_LANG="$TEST_WIN_LANG" RUN_MODE=cli DRY_RUN=1 BID="$GOOD_BID" RPI_MODEL=4 DEVICE="$DEV_INSTALL" CAN_INSTALL_ON_SAME_DRIVE=1 USE_CACHE=2)

      LAST_OUT="$(env "${update_env[@]}" "$clone_dir/install-wor.sh" 2>&1)"
      LAST_CODE=$?
      expect_no_output "already level with $branch: self-updater stays quiet" "Auto-updating wor-flasher"

      #fall one commit behind $REPO_DIR's $branch, so the clone actually has something to pull
      if git -C "$clone_dir" reset -q --hard HEAD~1 2>/dev/null ;then
        LAST_OUT="$(env "${update_env[@]}" "$clone_dir/install-wor.sh" 2>&1)"
        LAST_CODE=$?
        expect_output "behind $branch: self-updater detects the update" "Auto-updating wor-flasher"
        expect_output "behind $branch: self-updater pulls and reloads" "Reloading script"
        [ "$(git -C "$clone_dir" rev-parse HEAD)" == "$(git -C "$REPO_DIR" rev-parse "$branch")" ] \
          && pass "self-updater fast-forwarded the clone to $branch" \
          || fail "self-updater did not fast-forward the clone to $branch"
      else
        skip "$branch has no earlier commit to fall behind"
      fi
    fi
  fi
fi

info "== terminal-run =="
if [ -z "$DISPLAY" ] && [ -z "$WAYLAND_DISPLAY" ];then
  skip "terminal-run needs a display"
else
  DEBUG=1 timeout 30 "$REPO_DIR/terminal-run" 'true' 'wor-flasher test' >/dev/null 2>&1 \
    && pass "terminal-run launches a terminal" || fail "terminal-run could not launch a terminal"
fi

######## Summary

echo
summary
