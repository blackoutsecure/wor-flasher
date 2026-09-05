#!/bin/bash

#Build or verify the immutable runtime payload embedded in the macOS app bundle.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
REPO_DIR="$(cd "$SCRIPT_DIR/.." && pwd -P)"
APP_RESOURCES="$REPO_DIR/WoR-Flasher.app/Contents/Resources"
RUNTIME_DIR="$APP_RESOURCES/runtime"
MANIFEST_FILE="$APP_RESOURCES/runtime-manifest.json"
MODE="${1:---check}"

#shellcheck source=../src/lib/metadata.sh
source "$REPO_DIR/src/lib/metadata.sh"

case "$MODE" in
  --check|--write) ;;
  *)
    printf 'Usage: %s [--check|--write]\n' "${0##*/}" >&2
    exit 2
    ;;
esac

runtime_paths=(
  install-wor.sh
  install-wor-gui.sh
  install-wor-hook.sh
  src/lib
  config-templates
  assets
)

file_mode() { #Input: file. Output: octal permission mode.
  if stat -f '%Lp' "$1" >/dev/null 2>&1 ;then
    stat -f '%Lp' "$1"
  else
    stat -c '%a' "$1"
  fi
}

file_digest() { #Input: file. Output: SHA-256 digest.
  if command -v shasum >/dev/null 2>&1 ;then
    shasum -a 256 "$1" | awk '{print $1}'
  elif command -v sha256sum >/dev/null 2>&1 ;then
    sha256sum "$1" | awk '{print $1}'
  else
    printf 'A SHA-256 digest command is required (shasum or sha256sum).\n' >&2
    return 1
  fi
}

json_escape() { #Input: string. Output: JSON string contents.
  printf '%s' "$1" | sed 's/\\/\\\\/g; s/"/\\"/g'
}

build_payload() { #Input: staging root and manifest path.
  local stage_root="$1"
  local manifest="$2"
  local path source target relative digest mode separator=''
  mkdir -p "$stage_root"
  for path in "${runtime_paths[@]}" ;do
    source="$REPO_DIR/$path"
    [ -e "$source" ] || { printf 'Missing runtime path: %s\n' "$path" >&2; return 1; }
    target="$stage_root/$path"
    mkdir -p "$(dirname "$target")"
    cp -R "$source" "$target"
  done

  {
    printf '{\n  "schemaVersion": 1,\n  "version": "'
    json_escape "$WOR_FLASHER_VERSION"
    printf '",\n  "files": [\n'
    while IFS= read -r target ;do
      relative="${target#"$stage_root/"}"
      digest="$(file_digest "$target")"
      mode="$(file_mode "$target")"
      printf '%s    {"path": "' "$separator"
      json_escape "$relative"
      printf '", "sha256": "%s", "mode": "%s"}' "$digest" "$mode"
      separator=$',\n'
    done < <(find "$stage_root" -type f -print | LC_ALL=C sort)
    printf '\n  ]\n}\n'
  } > "$manifest"
}

stage_parent="$(mktemp -d "${TMPDIR:-/tmp}/wor-flasher-package.XXXXXX")"
trap 'rm -rf "$stage_parent"' EXIT INT TERM
stage_runtime="$stage_parent/runtime"
stage_manifest="$stage_parent/runtime-manifest.json"
build_payload "$stage_runtime" "$stage_manifest"

if [ "$MODE" == --check ];then
  [ -d "$RUNTIME_DIR" ] || { printf 'Embedded runtime is missing; run %s --write.\n' "${0##*/}" >&2; exit 1; }
  [ -f "$MANIFEST_FILE" ] || { printf 'Runtime manifest is missing; run %s --write.\n' "${0##*/}" >&2; exit 1; }
  diff -qr "$stage_runtime" "$RUNTIME_DIR" >/dev/null \
    && cmp -s "$stage_manifest" "$MANIFEST_FILE" \
    || { printf 'Embedded runtime is stale; run %s --write.\n' "${0##*/}" >&2; exit 1; }
  printf 'Embedded macOS runtime is current (%s).\n' "$WOR_FLASHER_VERSION"
  exit 0
fi

mkdir -p "$APP_RESOURCES"
backup_runtime="$stage_parent/runtime.backup"
backup_manifest="$stage_parent/runtime-manifest.backup.json"
rm -rf "$RUNTIME_DIR.new" "$MANIFEST_FILE.new"
mv "$stage_runtime" "$RUNTIME_DIR.new"
mv "$stage_manifest" "$MANIFEST_FILE.new"
if [ -e "$RUNTIME_DIR" ];then
  mv "$RUNTIME_DIR" "$backup_runtime"
fi
if [ -e "$MANIFEST_FILE" ];then
  mv "$MANIFEST_FILE" "$backup_manifest"
fi
if ! mv "$RUNTIME_DIR.new" "$RUNTIME_DIR" || ! mv "$MANIFEST_FILE.new" "$MANIFEST_FILE" ;then
  rm -rf "$RUNTIME_DIR" "$MANIFEST_FILE"
  [ ! -e "$backup_runtime" ] || mv "$backup_runtime" "$RUNTIME_DIR"
  [ ! -e "$backup_manifest" ] || mv "$backup_manifest" "$MANIFEST_FILE"
  printf 'Could not replace the embedded macOS runtime; the previous runtime was restored.\n' >&2
  exit 1
fi
printf 'Packaged macOS runtime %s.\n' "$WOR_FLASHER_VERSION"
