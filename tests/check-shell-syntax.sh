#!/bin/bash

set -euo pipefail

REPO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
cd "$REPO_DIR"

for script in src/standalone-launcher.sh src/lib/*.sh install-wor.sh install-wor-gui.sh install-wor-hook.sh src/macos-app/Contents/MacOS/WoR-Flasher tests/*.sh;do
  "$BASH" -n "$script"
done
