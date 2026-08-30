#!/bin/bash

# Run the integration test harness inside a privileged Linux container. This is useful
# on macOS hosts, where Linux loop devices do not exist natively.

set -euo pipefail

script_dir() {
  if command -v realpath >/dev/null ;then
    realpath "$1"
  else
    (cd "$1" && pwd -P)
  fi
}

REPO_DIR="$(script_dir "$(dirname "$0")/..")"
IMAGE="${LINUX_TEST_IMAGE:-ubuntu:24.04}"
TEST_UID="${LINUX_TEST_UID:-$(id -u)}"
TEST_GID="${LINUX_TEST_GID:-$(id -g)}"
CONTAINER_TEST_DIR="${LINUX_TEST_DIR:-/tmp/wor-flasher-test-workspace}"

PASSED=0
FAILED=0
SKIPPED=0

pass() { printf '  \e[92mPASS\e[0m  %s\n' "$1"; PASSED=$((PASSED+1)); }
fail() { printf '  \e[91mFAIL\e[0m  %s\n' "$1"; FAILED=$((FAILED+1)); }
skip() { printf '  \e[93mSKIP\e[0m  %s\n' "$1"; SKIPPED=$((SKIPPED+1)); }
info() { printf '\e[96m%s\e[0m\n' "$1"; }

summary() {
  echo
  printf 'passed %s, failed %s, skipped %s\n' "$PASSED" "$FAILED" "$SKIPPED"
  [ "$FAILED" -gt 0 ] && exit 1
  exit 0
}

info "== Docker preflight =="

if ! command -v docker >/dev/null ;then
  skip "Docker is not installed; Linux loop-device integration tests cannot run on this host"
  summary
fi
pass "Docker command is available"

if ! docker info >/dev/null 2>&1 ;then
  skip "Docker is installed but not available; Linux loop-device integration tests cannot run on this host"
  summary
fi
pass "Docker daemon is available"

if ! docker run --rm "$IMAGE" true >/dev/null 2>&1 ;then
  skip "Docker cannot start the $IMAGE container; Linux loop-device integration tests cannot run on this host"
  summary
fi
pass "Docker can start $IMAGE"

info "== Linux integration container =="

if docker run --rm --privileged \
  -v "$REPO_DIR:/work" \
  -w /work \
  -e NO_UPDATE=1 \
  -e TEST_DIR="$CONTAINER_TEST_DIR" \
  -e TEST_UID="$TEST_UID" \
  -e TEST_GID="$TEST_GID" \
  -e CONTAINER_TEST_DIR="$CONTAINER_TEST_DIR" \
  "$IMAGE" \
  bash -lc '
    set -euo pipefail
    export DEBIAN_FRONTEND=noninteractive
    apt-get update -qq
    apt-get install -y -qq --no-install-recommends \
      aria2 bc cabextract ca-certificates chntpw coreutils dosfstools exfat-fuse exfatprogs findutils gawk genisoimage git grep passwd parted sed shellcheck sudo udftools unzip util-linux wget wimtools yad
    if ! getent group "$TEST_GID" >/dev/null; then
      groupadd -g "$TEST_GID" wor-test
    fi
    if ! getent passwd "$TEST_UID" >/dev/null; then
      useradd -m -u "$TEST_UID" -g "$TEST_GID" -s /bin/bash wor-test
    fi
    user_name="$(getent passwd "$TEST_UID" | cut -d: -f1)"
    printf "%s ALL=(ALL) NOPASSWD:ALL\n" "$user_name" >/etc/sudoers.d/wor-test
    chmod 0440 /etc/sudoers.d/wor-test
    chown -R "$TEST_UID:$TEST_GID" "$CONTAINER_TEST_DIR" 2>/dev/null || true
    cd /work
    sudo -E -H -u "$user_name" env WOR_FLASHER_CONTAINER_TEST=1 SKIP_PACKAGE_INSTALL=1 TEST_DIR="$CONTAINER_TEST_DIR" ./tests/run-tests.sh "$@"
  ' bash "$@" ;then
  pass "Linux integration container completed"
else
  fail "Linux integration container failed"
fi

summary
