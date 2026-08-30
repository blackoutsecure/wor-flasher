#!/bin/bash

# GUI test harness for wor-flasher. Creates fake drives through run-tests.sh, then
# launches install-wor-gui.sh with DRY_RUN=1 so no storage is written.
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

if [ "$(uname -s)" != Linux ];then
  skip "GUI walkthrough needs a Linux desktop host with loop-device support"
  summary
fi
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

info "== GUI walkthrough =="
"$REPO_DIR/tests/run-tests.sh" --gui "$@"
