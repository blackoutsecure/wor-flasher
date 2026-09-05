#!/bin/bash

# Run the integration test harness inside a privileged Linux container. This is useful
# on macOS hosts, where Linux loop devices do not exist natively.

set -euo pipefail

TEST_SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd -P)"
#shellcheck disable=SC1091
source "$TEST_SCRIPT_DIR/test-lib.sh"

REPO_DIR="$(script_dir "$TEST_SCRIPT_DIR/..")"
IMAGE="${LINUX_TEST_IMAGE:-ubuntu:24.04}"
TEST_UID="${LINUX_TEST_UID:-$(id -u)}"
TEST_GID="${LINUX_TEST_GID:-$(id -g)}"
CONTAINER_TEST_DIR="${LINUX_TEST_DIR:-/tmp/wor-flasher-test-workspace}"

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
progress "launching privileged $IMAGE container"
progress "container will install test dependencies, then run ./tests/run-tests.sh"

if docker run --rm --privileged \
  -v "$REPO_DIR:/work" \
  -w /work \
  -e NO_UPDATE=1 \
  -e TEST_DIR="$CONTAINER_TEST_DIR" \
  -e TEST_UID="$TEST_UID" \
  -e TEST_GID="$TEST_GID" \
  -e TEST_MODELS="${TEST_MODELS:-}" \
  -e CONTAINER_TEST_DIR="$CONTAINER_TEST_DIR" \
  "$IMAGE" \
  bash -lc '
    set -euo pipefail
    export DEBIAN_FRONTEND=noninteractive
    printf "  ... container: updating apt metadata\n"
    apt-get update -qq
    printf "  ... container: installing test dependencies\n"
    apt-get install -y -qq --no-install-recommends \
      aria2 bc cabextract ca-certificates chntpw coreutils curl dosfstools exfat-fuse exfatprogs findutils gawk genisoimage git grep jq passwd parted pv sed shellcheck sudo udftools unzip util-linux wget wimtools yad
    printf "  ... container: preparing test user %s:%s\n" "$TEST_UID" "$TEST_GID"
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
    printf "  ... container: starting nested run-tests.sh\n"
    sudo -E -H -u "$user_name" env WOR_FLASHER_CONTAINER_TEST=1 SKIP_PACKAGE_INSTALL=1 TEST_DIR="$CONTAINER_TEST_DIR" ./tests/run-tests.sh "$@"
  ' bash "$@" ;then
  pass "Linux integration container completed"
else
  fail "Linux integration container failed"
fi

summary
