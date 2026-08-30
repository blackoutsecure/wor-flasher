#!/bin/bash

#Test harness for wor-flasher. Creates loopback devices to stand in for real drives,
#so nothing can be written to physical storage.
#
#Usage:
#  ./tests/run-tests.sh              run the automated suite
#  ./tests/run-tests.sh --walkthrough  create fake drives, then run the CLI interactively
#  ./tests/run-tests.sh --gui          create fake drives, then launch the GUI (needs a display)
#  ./tests/run-tests.sh --full         also download the real Windows image (several GB)
#  ./tests/run-tests.sh --keep         leave the fake drives and downloads in place afterwards
#  ./tests/run-tests.sh --clean        remove the test workspace and detach fake drives

######## Defaults. Every one can be overridden from the environment.

REPO_DIR="$(readlink -f "$(dirname "$0")/..")"

#Everything this script creates lives here. It is listed in .gitignore.
[ -z "$TEST_DIR" ] && TEST_DIR="$REPO_DIR/.test-workspace"

#Where install-wor.sh downloads components to during tests
[ -z "$TEST_DL_DIR" ] && TEST_DL_DIR="$TEST_DIR/downloads"

#A build that boots on every supported model
[ -z "$TEST_BID" ] && TEST_BID='22631.2861'

#A build that requires ARMv8.1, so it must be rejected on a Pi 3 or Pi 4
[ -z "$TEST_BAD_BID" ] && TEST_BAD_BID='26100.1742'

[ -z "$TEST_WIN_LANG" ] && TEST_WIN_LANG='en-us'
[ -z "$TEST_RPI_MODEL" ] && TEST_RPI_MODEL=4

#Fake drive sizes, one per tier that drive_capability() recognises
[ -z "$SIZE_INSTALL" ] && SIZE_INSTALL=32G   #>=25GB, can install Windows onto itself
[ -z "$SIZE_RECOVERY" ] && SIZE_RECOVERY=16G #8-25GB, recovery drive only
[ -z "$SIZE_TOO_SMALL" ] && SIZE_TOO_SMALL=2G #<8GB, must be refused

######## End of defaults

PASSED=0
FAILED=0
SKIPPED=0
LOOP_DEVICES=()
LAST_OUT=''
KEEP=0
MODE=suite
SKIP_ESD=1

pass() { printf '  \e[92mPASS\e[0m  %s\n' "$1"; PASSED=$((PASSED+1)); }
fail() { printf '  \e[91mFAIL\e[0m  %s\n' "$1"; FAILED=$((FAILED+1)); }
skip() { printf '  \e[93mSKIP\e[0m  %s\n' "$1"; SKIPPED=$((SKIPPED+1)); }
info() { printf '\e[96m%s\e[0m\n' "$1"; }
die()  { printf '\e[91m%s\e[0m\n' "$1" 1>&2; exit 1; }

cleanup() {
  local dev
  for dev in "${LOOP_DEVICES[@]}" ;do
    sudo losetup -d "$dev" 2>/dev/null
  done
  if [ "$KEEP" == 0 ] && [ -d "$TEST_DIR" ];then
    rm -rf "$TEST_DIR"
  fi
}

make_disk() { #Input: size, name. Output: loop device path
  local size="$1"
  local name="$2"
  local img="$TEST_DIR/${name}.img"
  mkdir -p "$TEST_DIR"
  #sparse, so a 32G image costs nothing until something writes to it
  truncate -s "$size" "$img" || die "Failed to create $img"
  local dev
  dev="$(sudo losetup -f --show "$img")" || die "Failed to attach $img to a loop device"
  LOOP_DEVICES+=("$dev")
  echo "$dev"
}

require_tools() {
  sudo -n true 2>/dev/null || die "This harness needs passwordless sudo to create loopback devices."
  command -v losetup >/dev/null || die "losetup is not installed."
  [ -e /dev/loop-control ] || die "No /dev/loop-control, so loopback devices cannot be created here."
}

guard_self_update() {
  #install-wor.sh runs 'git restore .' before updating itself, which would discard local edits
  if [ ! -f "$REPO_DIR/no-update" ];then
    touch "$REPO_DIR/no-update"
    info "Created $REPO_DIR/no-update so the self-updater cannot discard your changes."
  fi
}

stub_kernel_modules() {
  #containers have no /lib/modules, which install-wor.sh treats as a pending reboot
  local moddir="/lib/modules/$(uname -r)"
  if [ ! -d "$moddir" ];then
    sudo mkdir -p "$moddir" && info "Created $moddir so the reboot check passes in this container."
  fi
}

seed_winfiles() {
  #a completed marker makes install-wor.sh skip the multi-gigabyte Windows download
  [ "$SKIP_ESD" == 0 ] && return 0
  mkdir -p "$TEST_DL_DIR/winfiles_${TEST_BID}_${TEST_WIN_LANG}"
  touch "$TEST_DL_DIR/winfiles_${TEST_BID}_${TEST_WIN_LANG}/alldone"
}

run_flasher() { #Input: VAR=VALUE pairs. Sets LAST_OUT, returns the script's exit code
  LAST_OUT="$(env DL_DIR="$TEST_DL_DIR" WIN_LANG="$TEST_WIN_LANG" RUN_MODE=cli DRY_RUN=1 "$@" "$REPO_DIR/install-wor.sh" 2>&1)"
}

expect_success() { #Input: description
  if [ $? == 0 ];then pass "$1"; else fail "$1 (exit code was not 0)"; fi
}

expect_output() { #Input: description, string that must appear in LAST_OUT
  if grep -qF "$2" <<<"$LAST_OUT" ;then pass "$1"; else fail "$1 (did not find: $2)"; fi
}

expect_no_output() { #Input: description, string that must NOT appear in LAST_OUT
  if grep -qF "$2" <<<"$LAST_OUT" ;then fail "$1 (unexpectedly found: $2)"; else pass "$1"; fi
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
  #detach any loop device still backed by an image in the test workspace
  while read -r dev ;do
    [ ! -z "$dev" ] && sudo losetup -d "$dev" 2>/dev/null && info "Detached $dev"
  done < <(losetup -a 2>/dev/null | grep -F "$TEST_DIR" | cut -d: -f1)
  rm -rf "$TEST_DIR"
  info "Removed $TEST_DIR"
  exit 0
fi

trap cleanup EXIT
require_tools
guard_self_update
stub_kernel_modules
mkdir -p "$TEST_DL_DIR"

######## Interactive modes

if [ "$MODE" == walkthrough ] || [ "$MODE" == gui ];then
  seed_winfiles
  DEV_INSTALL="$(make_disk "$SIZE_INSTALL" install)"
  DEV_RECOVERY="$(make_disk "$SIZE_RECOVERY" recovery)"
  DEV_SMALL="$(make_disk "$SIZE_TOO_SMALL" toosmall)"
  info "Fake drives ready:"
  echo "  $DEV_INSTALL  ($SIZE_INSTALL, installation tier)"
  echo "  $DEV_RECOVERY ($SIZE_RECOVERY, recovery tier)"
  echo "  $DEV_SMALL    ($SIZE_TOO_SMALL, must be refused)"
  echo
  info "DRY_RUN is set, so nothing will be written even if you pick a drive."
  echo

  if [ "$MODE" == walkthrough ];then
    DL_DIR="$TEST_DL_DIR" DRY_RUN=1 "$REPO_DIR/install-wor.sh"
  else
    [ -z "$DISPLAY" ] && [ -z "$WAYLAND_DISPLAY" ] && die "The GUI needs a display. Run this on a desktop session."
    DL_DIR="$TEST_DL_DIR" DRY_RUN=1 "$REPO_DIR/install-wor-gui.sh"
  fi
  exit $?
fi

######## Automated suite

info "== Static checks =="
for f in install-wor.sh install-wor-gui.sh terminal-run ;do
  if bash -n "$REPO_DIR/$f" 2>/dev/null ;then pass "$f parses"; else fail "$f has a syntax error"; fi
done

if command -v shellcheck >/dev/null ;then
  if shellcheck --severity=error "$REPO_DIR"/install-wor.sh "$REPO_DIR"/install-wor-gui.sh "$REPO_DIR"/terminal-run >/dev/null 2>&1 ;then
    pass "shellcheck reports no errors"
  else
    fail "shellcheck reports errors"
  fi
else
  skip "shellcheck is not installed"
fi

info "== Library functions =="
#shellcheck disable=SC1090
source "$REPO_DIR/install-wor.sh" source >/dev/null 2>&1

for pair in '19045.1234:Windows 10' '22631.2861:Windows 11' ;do
  bid="${pair%%:*}"; want="${pair#*:}"
  if [[ "$(get_os_name "$bid")" == "$want"* ]];then pass "get_os_name $bid -> $want"; else fail "get_os_name $bid"; fi
done

RPI_MODEL=4
cpu_supports_bid "$TEST_BID" && pass "cpu_supports_bid allows $TEST_BID on a Pi 4" || fail "cpu_supports_bid rejected $TEST_BID on a Pi 4"
cpu_supports_bid "$TEST_BAD_BID" && fail "cpu_supports_bid allowed $TEST_BAD_BID on a Pi 4" || pass "cpu_supports_bid blocks $TEST_BAD_BID on a Pi 4"
RPI_MODEL=5
cpu_supports_bid "$TEST_BAD_BID" && pass "cpu_supports_bid allows $TEST_BAD_BID on a Pi 5" || fail "cpu_supports_bid rejected $TEST_BAD_BID on a Pi 5"
unset RPI_MODEL

info "== Fake drives =="
DEV_INSTALL="$(make_disk "$SIZE_INSTALL" install)"
DEV_RECOVERY="$(make_disk "$SIZE_RECOVERY" recovery)"
DEV_SMALL="$(make_disk "$SIZE_TOO_SMALL" toosmall)"
echo "  install tier:  $DEV_INSTALL"
echo "  recovery tier: $DEV_RECOVERY"
echo "  too small:     $DEV_SMALL"

[ "$(drive_capability "$DEV_INSTALL")" == install ] && pass "drive_capability $SIZE_INSTALL -> install" || fail "drive_capability $SIZE_INSTALL"
[ "$(drive_capability "$DEV_RECOVERY")" == recovery ] && pass "drive_capability $SIZE_RECOVERY -> recovery" || fail "drive_capability $SIZE_RECOVERY"
[ "$(drive_capability "$DEV_SMALL")" == too-small ] && pass "drive_capability $SIZE_TOO_SMALL -> too-small" || fail "drive_capability $SIZE_TOO_SMALL"

seed_winfiles

info "== Full run, USE_CACHE=0 =="
run_flasher BID="$TEST_BID" RPI_MODEL="$TEST_RPI_MODEL" DEVICE="$DEV_INSTALL" CAN_INSTALL_ON_SAME_DRIVE=1 USE_CACHE=0
expect_success "dry run completes"
expect_output "downloads the PE installer" "Downloading WoR PE-based installer"
expect_output "downloads the ARM64 drivers" "Downloading ARM64 drivers"
expect_output "downloads the UEFI firmware" "UEFI firmware"
expect_output "stops before flashing" "DRY_RUN"

for folder in peinstaller driverpackage "pi${TEST_RPI_MODEL}-uefipackage" ;do
  if [ -f "$TEST_DL_DIR/$folder/.wor-flasher-version" ];then
    pass "$folder is version stamped"
  else
    fail "$folder has no version stamp"
  fi
done

info "== Cache modes =="
run_flasher BID="$TEST_BID" RPI_MODEL="$TEST_RPI_MODEL" DEVICE="$DEV_INSTALL" CAN_INSTALL_ON_SAME_DRIVE=1 USE_CACHE=1
expect_output "USE_CACHE=1 reuses a current cache" "cached copy is up to date"
expect_no_output "USE_CACHE=1 downloads nothing" "Downloading ARM64 drivers"

echo 'https://example.com/stale.zip' > "$TEST_DL_DIR/driverpackage/.wor-flasher-version"
run_flasher BID="$TEST_BID" RPI_MODEL="$TEST_RPI_MODEL" DEVICE="$DEV_INSTALL" CAN_INSTALL_ON_SAME_DRIVE=1 USE_CACHE=1
expect_output "USE_CACHE=1 refreshes a stale component" "Downloading ARM64 drivers"
expect_output "USE_CACHE=1 keeps the current ones" "peinstaller - cached copy is up to date"

run_flasher BID="$TEST_BID" RPI_MODEL="$TEST_RPI_MODEL" DEVICE="$DEV_INSTALL" CAN_INSTALL_ON_SAME_DRIVE=1 USE_CACHE=2
expect_output "USE_CACHE=2 skips update checks" "without checking for updates"

run_flasher BID="$TEST_BID" RPI_MODEL="$TEST_RPI_MODEL" DEVICE="$DEV_INSTALL" CAN_INSTALL_ON_SAME_DRIVE=1 USE_CACHE=9
if [ $? != 0 ];then pass "USE_CACHE=9 is rejected"; else fail "USE_CACHE=9 was accepted"; fi

info "== Guards =="
run_flasher BID="$TEST_BAD_BID" RPI_MODEL=4 DEVICE="$DEV_INSTALL" CAN_INSTALL_ON_SAME_DRIVE=1 USE_CACHE=2
if [ $? != 0 ];then pass "an ARMv8.1 build is refused on a Pi 4"; else fail "an ARMv8.1 build was accepted on a Pi 4"; fi
expect_output "the refusal explains why" "ARMv8.1"

run_flasher BID="$TEST_BID" RPI_MODEL="$TEST_RPI_MODEL" DEVICE="$DEV_SMALL" CAN_INSTALL_ON_SAME_DRIVE=0 USE_CACHE=2
if [ $? != 0 ];then pass "a drive under 8GB is refused"; else fail "a drive under 8GB was accepted"; fi

run_flasher BID="$TEST_BID" RPI_MODEL="$TEST_RPI_MODEL" DEVICE="$DEV_RECOVERY" CAN_INSTALL_ON_SAME_DRIVE=1 USE_CACHE=2
if [ $? != 0 ];then pass "self-install is refused on a recovery-sized drive"; else fail "self-install was accepted on a recovery-sized drive"; fi

run_flasher BID="$TEST_BID" RPI_MODEL=9 DEVICE="$DEV_INSTALL" CAN_INSTALL_ON_SAME_DRIVE=1 USE_CACHE=2
if [ $? != 0 ];then pass "an unknown RPI_MODEL is rejected"; else fail "an unknown RPI_MODEL was accepted"; fi

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
if [ "$(head -n1 <<<"$handoff_out")" == "$REPO_DIR/install-wor.sh" ];then
  pass "values survive a terminal that does not inherit the environment"
else
  fail "values were lost in a terminal that does not inherit the environment"
fi
[ "$(tail -n1 <<<"$handoff_out")" == 3 ] && pass "a multi-line CONFIG_TXT stays intact" || fail "CONFIG_TXT was mangled"
if [ -e "$TEST_DIR/INJECTED" ] || [ -e "$TEST_DIR/INJECTED2" ];then
  fail "CONFIG_TXT was executed as code"
else
  pass "CONFIG_TXT is not executed as code"
fi

info "== terminal-run =="
if [ -z "$DISPLAY" ] && [ -z "$WAYLAND_DISPLAY" ];then
  skip "terminal-run needs a display"
else
  if DEBUG=1 timeout 30 "$REPO_DIR/terminal-run" 'true' 'wor-flasher test' >/dev/null 2>&1 ;then
    pass "terminal-run launches a terminal"
  else
    fail "terminal-run could not launch a terminal"
  fi
fi

######## Summary

echo
printf 'passed %s, failed %s, skipped %s\n' "$PASSED" "$FAILED" "$SKIPPED"
[ "$KEEP" == 1 ] && info "Left in place: $TEST_DIR"
[ "$FAILED" -gt 0 ] && exit 1
exit 0
