#!/bin/bash
#WOR_STANDALONE_CLIENT

#WoR-Flasher @WOR_VERSION@ standalone release. Generated from the canonical runtime.
#Original author: Botspot. Maintained with Blackout Secure support.
#README, LICENSE, and NOTICE are included in the embedded runtime.
#Verify this entire download against the release SHA256SUMS before running it.

if [ -z "${BASH_SOURCE[0]:-}" ];then
  printf 'Save and verify install-wor.sh before running it; do not pipe it into Bash.\n' >&2
  exit 2
fi
if [ "${BASH_SOURCE[0]}" != "$0" ];then
  printf 'Run this release with bash install-wor.sh; use the full checkout to source engine functions.\n' >&2
  return 2
fi
set -euo pipefail
original_umask="$(umask)"
umask 077

fail() {
  printf 'WoR-Flasher standalone: %s\n' "$*" >&2
  exit 1
}

[ "${1:-}" != source ] || fail "The standalone release must be executed, not sourced."
for tool in base64 tar gzip awk find wc tr mktemp mkdir chmod mv rm rmdir;do
  command -v "$tool" >/dev/null 2>&1 || fail "Required unpacking tool is missing: $tool"
done
if command -v sha256sum >/dev/null 2>&1;then
  hash_command=(sha256sum)
elif command -v shasum >/dev/null 2>&1;then
  hash_command=(shasum -a 256)
else
  fail "SHA-256 verification requires sha256sum or shasum."
fi

payload_sha256='@WOR_PAYLOAD_SHA256@'
manifest_sha256='@WOR_MANIFEST_SHA256@'
runtime_file_count='@WOR_RUNTIME_FILE_COUNT@'
cache_root="${XDG_CACHE_HOME:-${HOME:?HOME is required}/.cache}/wor-flasher/standalone"
[[ "$cache_root" == /* ]] || fail "The user cache directory must be an absolute path."
[ ! -L "$cache_root" ] || fail "Refusing a symbolic-link runtime cache: $cache_root"
mkdir -p "$cache_root" || fail "Could not create the user runtime cache: $cache_root"
[ -O "$cache_root" ] || fail "The runtime cache is not owned by the current user: $cache_root"
chmod 700 "$cache_root" || fail "Could not secure the user runtime cache: $cache_root"
runtime_root="$cache_root/$payload_sha256"
stage=''
lock=''

cleanup() {
  local result=$?
  trap - EXIT
  if [ -n "$stage" ];then
    rm -rf -- "$stage" || { printf 'Could not remove standalone staging directory: %s\n' "$stage" >&2; result=1; }
  fi
  if [ -n "$lock" ];then
    rmdir "$lock" || { printf 'Could not remove standalone preparation lock: %s\n' "$lock" >&2; result=1; }
  fi
  exit "$result"
}
trap cleanup EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

verify_runtime() { #Only an exact, user-owned release payload may be reused.
  local root="$1" digest count links
  [ -d "$root" ] && [ ! -L "$root" ] && [ -O "$root" ] || return 1
  [ -d "$root/runtime" ] && [ ! -L "$root/runtime" ] || return 1
  [ -f "$root/runtime.sha256" ] && [ ! -L "$root/runtime.sha256" ] || return 1
  digest="$("${hash_command[@]}" "$root/runtime.sha256")" || return 1
  [ "${digest%% *}" == "$manifest_sha256" ] || return 1
  links="$(find "$root/runtime" -type l -print)" || return 1
  [ -z "$links" ] || return 1
  count="$(find "$root/runtime" -type f -print | wc -l | tr -d '[:space:]')" || return 1
  [ "$count" == "$runtime_file_count" ] || return 1
  (cd "$root" && "${hash_command[@]}" -c runtime.sha256 >/dev/null) || return 1
  [ -x "$root/runtime/install-wor.sh" ] && [ -x "$root/runtime/install-wor-gui.sh" ]
}

if [ -e "$runtime_root" ] || [ -L "$runtime_root" ];then
  verify_runtime "$runtime_root" \
    || fail "Cached runtime failed verification. After closing WoR-Flasher, move this cache aside and retry: $runtime_root"
else
  if ! mkdir "$runtime_root.lock" 2>/dev/null;then
    fail "This runtime is already being prepared. Retry after the other launch finishes. If no launch is active, remove the stale lock: $runtime_root.lock"
  fi
  lock="$runtime_root.lock"
  stage="$(mktemp -d "$cache_root/.stage.XXXXXX")" || fail "Could not create a private staging directory."
  base64 -d > "$stage/payload.tar.gz" <<'WOR_STANDALONE_PAYLOAD'
@WOR_PAYLOAD_BASE64@
WOR_STANDALONE_PAYLOAD
  actual_sha256="$("${hash_command[@]}" "$stage/payload.tar.gz")" || fail "Could not verify the embedded runtime."
  [ "${actual_sha256%% *}" == "$payload_sha256" ] || fail "Embedded runtime checksum mismatch; download the release again."
  tar -tzf "$stage/payload.tar.gz" > "$stage/entries" || fail "The embedded archive is invalid."
  while IFS= read -r entry;do
    case "$entry" in
      runtime|runtime/|runtime/*|runtime.sha256) ;;
      *) fail "Unexpected embedded path: $entry" ;;
    esac
    case "/$entry/" in
      *'/../'*|*'/./'*|*\\*) fail "Unsafe embedded path: $entry" ;;
    esac
  done < "$stage/entries"
  tar -tvzf "$stage/payload.tar.gz" > "$stage/types" || fail "Could not inspect the embedded archive."
  awk 'substr($1, 1, 1) != "-" && substr($1, 1, 1) != "d" { exit 1 }' "$stage/types" \
    || fail "The embedded runtime contains links or special files."
  tar -xzf "$stage/payload.tar.gz" -C "$stage" || fail "Could not unpack the embedded runtime."
  rm "$stage/payload.tar.gz" "$stage/entries" "$stage/types"
  verify_runtime "$stage" || fail "The unpacked runtime failed verification."
  [ ! -e "$runtime_root" ] && [ ! -L "$runtime_root" ] || fail "The runtime cache appeared during preparation; retry."
  mv "$stage" "$runtime_root" || fail "Could not install the verified runtime cache."
  stage=''
  rmdir "$lock" || fail "Could not release the runtime preparation lock."
  lock=''
fi

#Exec preserves the engine's signal/cleanup handling. Do not delete files used by an active flash.
export DIRECTORY="$runtime_root/runtime"
export WOR_CACHE_DIR="${WOR_CACHE_DIR:-$cache_root/responses}"
umask "$original_umask"
trap - EXIT HUP INT TERM
exec "$BASH" "$DIRECTORY/install-wor.sh" "$@"
