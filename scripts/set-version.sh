#!/bin/bash

#Update every tracked WoR-Flasher version surface, then rebuild the embedded macOS runtime.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
REPO_DIR="$(cd "$SCRIPT_DIR/.." && pwd -P)"
VERSION="${1:-}"

case "$VERSION" in
  '')
    printf 'Usage: %s <X.Y.Z>\n' "${0##*/}" >&2
    exit 2
    ;;
  *[!0-9.]*|*.*.*.*|.*|*.)
    printf 'Version must be X.Y.Z (got: %s).\n' "$VERSION" >&2
    exit 2
    ;;
esac

IFS=. read -r major minor patch <<< "$VERSION"
[[ "$major" =~ ^[0-9]+$ && "$minor" =~ ^[0-9]+$ && "$patch" =~ ^[0-9]+$ ]] || {
  printf 'Version must be X.Y.Z (got: %s).\n' "$VERSION" >&2
  exit 2
}

metadata_file="$REPO_DIR/src/lib/metadata.sh"
plist_file="$REPO_DIR/WoR-Flasher.app/Contents/Info.plist"
readme_file="$REPO_DIR/README.md"
engine_file="$REPO_DIR/install-wor.sh"

for file in "$metadata_file" "$plist_file" "$readme_file" "$engine_file" ;do
  [ -f "$file" ] || { printf 'Required version file is missing: %s\n' "$file" >&2; exit 1; }
done

VERSION="$VERSION" perl -0pi -e '
  my $version = $ENV{VERSION};
  s{^WOR_FLASHER_VERSION=[\x27\x22][^\x27\x22]+[\x27\x22]$}{WOR_FLASHER_VERSION=\x27$version\x27}m
' "$metadata_file"

VERSION="$VERSION" perl -0pi -e '
  my $version = $ENV{VERSION};
  s{(<key>CFBundleShortVersionString</key>\s*<string>)[^<]*(</string>)}{$1 . $version . $2}eg;
  s{(<key>CFBundleVersion</key>\s*<string>)[^<]*(</string>)}{$1 . $version . $2}eg;
' "$plist_file"

VERSION="$VERSION" perl -0pi -e '
  my $version = $ENV{VERSION};
  s{version-[0-9]+\.[0-9]+\.[0-9]+-}{version-$version-};
  if ($_ !~ /^- \*\*\Q$version\E\*\*/m) {
    s{(## Versions\n\n)}{$1 . "- **$version** - Release version update.\n"}e;
  }
' "$readme_file"

VERSION="$VERSION" perl -0pi -e '
  my $version = $ENV{VERSION};
  if ($_ !~ /^#\Q$version\E - /m) {
    s{(#Version history\n#---------------\n)}{$1 . "#$version - Release version update.\n"}e;
  }
' "$engine_file"

"$SCRIPT_DIR/package-macos-app.sh" --write
printf 'Updated WoR-Flasher version surfaces to %s.\n' "$VERSION"
