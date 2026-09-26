#!/bin/bash

# GUI test harness for wor-flasher. On Linux it creates fake drives through
# run-tests.sh; on macOS it uses a real removable drive in DRY_RUN=1 mode.
#
# Usage:
#   ./tests/run-tests-gui.sh        run the GUI walkthrough test
#   ./tests/run-tests-gui.sh --full allow the real Windows image download
#   ./tests/run-tests-gui.sh --keep leave fake drives and downloads in place
#   ./tests/run-tests-gui.sh --clean remove the test workspace and detach fake drives

TEST_SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd -P)"
#shellcheck disable=SC1091
source "$TEST_SCRIPT_DIR/test-lib.sh"

REPO_DIR="$(script_dir "$TEST_SCRIPT_DIR/..")"

info "== GUI test preflight =="

case "$(uname -s)" in
  Linux)
    pass "Linux host detected"
    if [ -z "$DISPLAY" ] && [ -z "$WAYLAND_DISPLAY" ];then
      skip "GUI walkthrough needs DISPLAY or WAYLAND_DISPLAY"
      summary
    fi
    pass "desktop display detected"
    if ! command -v yad >/dev/null ;then
      skip "GUI walkthrough needs yad installed"
      summary
    fi
    pass "yad is available"
    ;;
  Darwin)
    command -v osascript >/dev/null || { skip "macOS GUI walkthrough needs osascript"; summary; }
    pass "macOS native GUI is available"
    [ -f "$REPO_DIR/partnership.png" ] || die "macOS GUI walkthrough needs partnership.png"
    pass "macOS welcome image is available"
    ;;
  *)
    skip "GUI walkthrough supports Linux and macOS hosts only"
    summary
    ;;
esac

info "== GUI walkthrough =="
"$REPO_DIR/tests/run-tests.sh" --gui "$@"
