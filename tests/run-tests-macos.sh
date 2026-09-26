#!/bin/bash

#macOS-only checks for the WoR-Flasher test suite. This file is sourced by tests/run-tests.sh,
#never run standalone: it relies on REPO_DIR, pass(), fail(), skip() and run_in_engine() already
#being defined by the caller. It stages and exercises the real macOS app bundle and its native
#launcher script, so it can only run truthfully on a real macOS host.
#
#tests/run-tests.sh calls macos_app_and_launcher_checks() when "$(uname -s)" == Darwin, and
#otherwise emits one skip() per entry in MACOS_ONLY_TEST_NAMES so every skipped check is named
#and the reason ("requires macOS") is explicit in the test output and the final summary count.

MACOS_ONLY_TEST_NAMES=(
  "macOS app packaging and update paths preserve bounded trusted execution"
  "packaged macOS engine loads without modifying its immutable runtime"
  "macOS runtime bootstrap handles spaces and rejects a tampered active runtime"
  "verified macOS runtime update promotes atomically and rolls back to the previous runtime"
  "macOS runtime updater preserves pointers across downgrade, digest, and incomplete-archive failures"
  "macOS runtime updates accept only newer numeric release versions"
  "macOS runtime validation rejects digest, mode, and symlink substitutions"
  "macOS runtime archive validation rejects symbolic links"
  "version-order test detects an equal-version acceptance mutation"
  "runtime-integrity test detects a removed digest guard"
  "runtime-integrity test detects a removed symlink guard"
  "archive-safety test detects a removed entry-type guard"
  "runtime-selection test detects an active/previous priority mutation"
  "macOS launcher startup status is source-testable and cleans up its state"
  "macOS launcher inventories Homebrew once and installs only missing dependencies"
  "macOS startup repairs missing tracked runtime assets without overwriting modifications"
  "macOS launcher no longer updates a source checkout from a git remote"
  "macOS app bundle metadata matches the shared name, version, and logo"
)

macos_app_and_launcher_checks() { #Requires: real Darwin host. Stages release/macos/WoR-Flasher.app and exercises its launcher.
  if command -v node >/dev/null ;then
    node "$REPO_DIR/src/build-release.mjs" --platform=macos >/dev/null 2>&1 \
      || fail "generated macOS app could not be staged for launcher tests"
  else
    fail "node is required to stage the generated macOS app for launcher tests"
  fi
  launcher="$REPO_DIR/release/macos/WoR-Flasher.app/Contents/MacOS/WoR-Flasher"
  node "$REPO_DIR/src/package-macos-app.mjs" --check >/dev/null 2>&1 \
    && grep -qF 'validate_runtime "$EMBEDDED_RUNTIME" "$EMBEDDED_MANIFEST"' "$launcher" \
    && grep -qF 'actual_digest="$(shasum -a 256 "$archive"' "$launcher" \
    && grep -qF 'archive_entries_are_safe "$archive"' "$launcher" \
    && grep -qF 'mv -f "$temporary" "$pointer"' "$launcher" \
    && grep -qF 'ln -sfn /usr/bin/osascript "$script_host"' "$REPO_DIR/src/lib/metadata.sh" \
    && grep -qF 'wor_osascript -l JavaScript' "$REPO_DIR/install-wor-gui.sh" \
    && ! grep -qE '^[[:space:]]*osascript -l JavaScript' "$REPO_DIR/install-wor-gui.sh" \
    && grep -qF 'git -C "$REPO_DIR" restore --source=HEAD -- "${missing[@]}"' "$launcher" \
    && grep -qF 'brew install "${missing[@]}"' "$launcher" \
    && grep -qF "open 'https://brew.sh'" "$launcher" \
    && grep -qF "start_startup_window" "$launcher" \
    && grep -qF 'local icon_path="$RESOURCES_DIR/$BUNDLE_ICON_FILENAME"' "$launcher" \
    && grep -qF 'const controller = $.WorStartupController.alloc.init' "$launcher" \
    && grep -qF 'installed_formulae="$(brew list --formula 2>/dev/null)"' "$launcher" \
    && grep -qF '[ "$logo_source" -nt "$icon_resource" ]' "$launcher" \
    && grep -qF 'trap cleanup_startup EXIT INT TERM' "$launcher" \
    && grep -qF "startup_phase 'Checking for updates and repairs...'" "$launcher" \
    && grep -qF 'if [ -d "$CHECKOUT_DIR/.git" ];then' "$launcher" \
    && grep -qF 'set_runtime_paths "$CHECKOUT_DIR"' "$launcher" \
    && grep -qF 'export NO_UPDATE=1' "$launcher" \
    && ! grep -qE 'git (reset|clean)|brew upgrade|curl.+\|.+(ba)?sh' "$launcher" \
    && [ -x "$launcher" ] \
    && pass "macOS app packaging and update paths preserve bounded trusted execution" \
    || fail "macOS app packaging or update path is stale, incomplete, or destructive"

  #install-wor.sh's "source" early-return (before any argument parsing) is not exercised under
  #set -e in this repo's own suite anywhere else; drop it here too so a harmless non-zero exit
  #from an earlier defensive check does not mask this assertion.
  if bash -c '
    DIRECTORY="$1/Contents/Resources/runtime" source "$1/Contents/Resources/runtime/install-wor.sh" source >/dev/null 2>&1
    source "$1/Contents/Resources/runtime/src/lib/metadata.sh"
    [ "$WOR_FLASHER_VERSION" == "$(sed -n "s/^[[:space:]]*\"version\": \"\([^\"]*\)\".*/\1/p" "$1/Contents/Resources/runtime-manifest.json")" ]
    [ ! -e "$1/Contents/Resources/runtime/cache" ]
  ' _ "$REPO_DIR/release/macos/WoR-Flasher.app" ;then
    pass "packaged macOS engine loads without modifying its immutable runtime"
  else
    fail "packaged macOS engine load failed or modified its immutable runtime"
  fi

  launcher_test_dir="$(mktemp -d "${TMPDIR:-/tmp}/wor launcher state.XXXXXX")"
  if WOR_LAUNCHER_SOURCE_ONLY=1 WOR_APP_SUPPORT_DIR="$launcher_test_dir/Application Support" bash -c '
    set -e
    source "$1"
    CHECKOUT_DIR="$2/missing checkout"
    select_runtime
    [ "$REPO_DIR" == "$EMBEDDED_RUNTIME" ]
    bootstrap_embedded_runtime
    select_runtime
    case "$REPO_DIR" in "$WOR_APP_SUPPORT_DIR"/runtimes/*/runtime) ;; *) exit 1 ;; esac
    validate_runtime "$REPO_DIR" "$RUNTIME_MANIFEST"
    cp "$EMBEDDED_MANIFEST" "$RUNTIME_MANIFEST"
    printf 'stale\n' >> "$REPO_DIR/install-wor.sh"
    bootstrap_embedded_runtime
    select_runtime
    cmp -s "$RUNTIME_MANIFEST" "$EMBEDDED_MANIFEST"
    printf "tampered\n" >> "$REPO_DIR/install-wor.sh"
    select_runtime
    [ "$REPO_DIR" == "$EMBEDDED_RUNTIME" ]
  ' _ "$launcher" "$launcher_test_dir" ;then
    pass "macOS runtime bootstrap handles spaces and rejects a tampered active runtime"
  else
    fail "macOS runtime bootstrap or tampered-runtime fallback failed"
  fi
  rm -rf "$launcher_test_dir"

  launcher_test_dir="$(mktemp -d)"
  if WOR_LAUNCHER_SOURCE_ONLY=1 WOR_APP_SUPPORT_DIR="$launcher_test_dir/support" WOR_CURL_BIN="$(command -v curl)" bash -c '
    set -e
    source "$1"
    metadata_value() { sed -n "s/.*\"$2\":\"\([^\"]*\)\".*/\1/p" "$1"; }
    CHECKOUT_DIR="$2/no-checkout"
    bootstrap_embedded_runtime
    select_runtime
    update_root="$2/update"
    mkdir -p "$update_root/runtime"
    cp -R "$EMBEDDED_RUNTIME/." "$update_root/runtime/"
    sed "s/\"version\": \"[^\"]*\"/\"version\": \"9.9.9\"/" "$EMBEDDED_MANIFEST" > "$update_root/runtime-manifest.json"
    tar -czf "$2/runtime.tar.gz" -C "$update_root" runtime runtime-manifest.json
    update_digest="$(shasum -a 256 "$2/runtime.tar.gz" | awk "{print \$1}")"
    printf "{\"version\":\"9.9.9\",\"url\":\"file://%s/runtime.tar.gz\",\"sha256\":\"%s\"}\n" "$2" "$update_digest" > "$2/metadata.json"
    ask_to_continue() { return 0; }
    WOR_CURL_BIN="$(command -v curl)" WOR_UPDATE_METADATA_URL="file://$2/metadata.json" WOR_ALLOW_INSECURE_UPDATE=1 check_for_runtime_update
    [ "$(cat "$ACTIVE_RUNTIME_FILE")" == 9.9.9 ]
    [ "$(cat "$PREVIOUS_RUNTIME_FILE")" == "$(manifest_version "$EMBEDDED_MANIFEST")" ]
    select_runtime
    [ "$(manifest_version "$RUNTIME_MANIFEST")" == 9.9.9 ]
    printf "broken\n" >> "$REPO_DIR/install-wor.sh"
    select_runtime
    [ "$(manifest_version "$RUNTIME_MANIFEST")" == "$(manifest_version "$EMBEDDED_MANIFEST")" ]
  ' _ "$launcher" "$launcher_test_dir" ;then
    pass "verified macOS runtime update promotes atomically and rolls back to the previous runtime"
  else
    fail "verified macOS runtime promotion or rollback failed"
  fi
  rm -rf "$launcher_test_dir"

  launcher_test_dir="$(mktemp -d)"
  if WOR_LAUNCHER_SOURCE_ONLY=1 WOR_APP_SUPPORT_DIR="$launcher_test_dir/support" WOR_CURL_BIN="$(command -v curl)" bash -c '
    set -e
    source "$1"
    metadata_value() { sed -n "s/.*\"$2\":\"\([^\"]*\)\".*/\1/p" "$1"; }
    CHECKOUT_DIR="$2/no-checkout"
    bootstrap_embedded_runtime
    select_runtime
    active_before="$(cat "$ACTIVE_RUNTIME_FILE")"
    prompts=0
    ask_to_continue() { prompts=$((prompts + 1)); return 0; }
    for candidate in "$active_before" 0.9.9 ;do
      printf "{\"version\":\"%s\",\"url\":\"file://%s/missing.tar.gz\",\"sha256\":\"%064d\"}\n" "$candidate" "$2" 0 > "$2/metadata.json"
      WOR_CURL_BIN="$(command -v curl)" WOR_UPDATE_METADATA_URL="file://$2/metadata.json" WOR_ALLOW_INSECURE_UPDATE=1 check_for_runtime_update
    done
    [ "$prompts" -eq 0 ]
    [ "$(cat "$ACTIVE_RUNTIME_FILE")" == "$active_before" ]
    [ ! -e "$PREVIOUS_RUNTIME_FILE" ]

    mkdir -p "$2/incomplete/runtime"
    printf "incomplete\n" > "$2/incomplete/runtime/README"
    cp "$EMBEDDED_MANIFEST" "$2/incomplete/runtime-manifest.json"
    tar -czf "$2/incomplete.tar.gz" -C "$2/incomplete" runtime runtime-manifest.json
    incomplete_digest="$(shasum -a 256 "$2/incomplete.tar.gz" | awk "{print \$1}")"

    printf "{\"version\":\"9.9.8\",\"url\":\"file://%s/incomplete.tar.gz\",\"sha256\":\"%064d\"}\n" "$2" 0 > "$2/metadata.json"
    WOR_CURL_BIN="$(command -v curl)" WOR_UPDATE_METADATA_URL="file://$2/metadata.json" WOR_ALLOW_INSECURE_UPDATE=1 check_for_runtime_update
    printf "{\"version\":\"9.9.9\",\"url\":\"file://%s/incomplete.tar.gz\",\"sha256\":\"%s\"}\n" "$2" "$incomplete_digest" > "$2/metadata.json"
    WOR_CURL_BIN="$(command -v curl)" WOR_UPDATE_METADATA_URL="file://$2/metadata.json" WOR_ALLOW_INSECURE_UPDATE=1 check_for_runtime_update

    [ "$prompts" -eq 2 ]
    [ "$(cat "$ACTIVE_RUNTIME_FILE")" == "$active_before" ]
    [ ! -e "$PREVIOUS_RUNTIME_FILE" ]
    select_runtime
    validate_runtime "$REPO_DIR" "$RUNTIME_MANIFEST"
  ' _ "$launcher" "$launcher_test_dir" ;then
    pass "macOS runtime updater preserves pointers across downgrade, digest, and incomplete-archive failures"
  else
    fail "macOS runtime updater changes state after a rejected update"
  fi
  rm -rf "$launcher_test_dir"

  if WOR_LAUNCHER_SOURCE_ONLY=1 bash -c '
    source "$1"
    version_is_newer 1.0.3 1.0.2
    version_is_newer 1.1 1.0.99
    version_is_newer 2 1.99.99
    ! version_is_newer 1.0.2 1.0.2
    ! version_is_newer 1.0.1 1.0.2
    ! version_is_newer 1.0.2-beta 1.0.1
  ' _ "$launcher" ;then
    pass "macOS runtime updates accept only newer numeric release versions"
  else
    fail "macOS runtime version ordering permits a downgrade or invalid release"
  fi

  launcher_test_dir="$(mktemp -d)"
  if WOR_LAUNCHER_SOURCE_ONLY=1 bash -c '
    set -e
    source "$1"
    cp -R "$EMBEDDED_RUNTIME" "$2/runtime"
    cp "$EMBEDDED_MANIFEST" "$2/runtime-manifest.json"
    validate_runtime "$2/runtime" "$2/runtime-manifest.json"
    printf "tampered\n" >> "$2/runtime/install-wor.sh"
    ! validate_runtime "$2/runtime" "$2/runtime-manifest.json"
    cp "$EMBEDDED_RUNTIME/install-wor.sh" "$2/runtime/install-wor.sh"
    chmod 644 "$2/runtime/install-wor.sh"
    ! validate_runtime "$2/runtime" "$2/runtime-manifest.json"
    rm "$2/runtime/install-wor.sh"
    ln -s "$EMBEDDED_RUNTIME/install-wor.sh" "$2/runtime/install-wor.sh"
    ! validate_runtime "$2/runtime" "$2/runtime-manifest.json"
  ' _ "$launcher" "$launcher_test_dir" ;then
    pass "macOS runtime validation rejects digest, mode, and symlink substitutions"
  else
    fail "macOS runtime validation accepts a tampered manifest entry"
  fi
  rm -rf "$launcher_test_dir"

  launcher_test_dir="$(mktemp -d)"
  mkdir -p "$launcher_test_dir/link-archive"
  ln -s /tmp "$launcher_test_dir/link-archive/escape"
  tar -czf "$launcher_test_dir/link.tar.gz" -C "$launcher_test_dir/link-archive" escape
  if WOR_LAUNCHER_SOURCE_ONLY=1 bash -c '
    source "$1"
    ! archive_entries_are_safe "$2/link.tar.gz"
  ' _ "$launcher" "$launcher_test_dir" ;then
    pass "macOS runtime archive validation rejects symbolic links"
  else
    fail "macOS runtime archive validation accepts symbolic links"
  fi
  rm -rf "$launcher_test_dir"

  launcher_test_dir="$(mktemp -d)"
  resources_dir="$REPO_DIR/release/macos/WoR-Flasher.app/Contents/Resources"
  info_plist="$REPO_DIR/release/macos/WoR-Flasher.app/Contents/Info.plist"

  cp "$launcher" "$launcher_test_dir/version-mutant"
  perl -0pi -e 's/done\n  return 1\n}\n\nvalidate_runtime/done\n  return 0\n}\n\nvalidate_runtime/' "$launcher_test_dir/version-mutant"
  if WOR_LAUNCHER_SOURCE_ONLY=1 WOR_APP_RESOURCES_DIR="$resources_dir" WOR_APP_INFO_PLIST="$info_plist" bash -c '
    source "$1"
    version_is_newer 1.0.2 1.0.2
  ' _ "$launcher_test_dir/version-mutant" ;then
    pass "version-order test detects an equal-version acceptance mutation"
  else
    fail "version-order mutation did not alter the focused test behavior"
  fi

  cp "$launcher" "$launcher_test_dir/integrity-mutant"
  sed 's/\[ "$actual_digest" == "$expected_digest" \] && \[ "$actual_mode" == "$expected_mode" \]/[ "$actual_mode" == "$expected_mode" ]/' "$launcher_test_dir/integrity-mutant" > "$launcher_test_dir/integrity-mutant.tmp"
  mv "$launcher_test_dir/integrity-mutant.tmp" "$launcher_test_dir/integrity-mutant"
  if WOR_LAUNCHER_SOURCE_ONLY=1 WOR_APP_RESOURCES_DIR="$resources_dir" WOR_APP_INFO_PLIST="$info_plist" bash -c '
    set -e
    source "$1"
    cp -R "$EMBEDDED_RUNTIME" "$2/runtime"
    cp "$EMBEDDED_MANIFEST" "$2/runtime-manifest.json"
    printf "tampered\n" >> "$2/runtime/install-wor.sh"
    validate_runtime "$2/runtime" "$2/runtime-manifest.json"
  ' _ "$launcher_test_dir/integrity-mutant" "$launcher_test_dir" ;then
    pass "runtime-integrity test detects a removed digest guard"
  else
    fail "digest mutation did not alter the focused test behavior"
  fi

  cp "$launcher" "$launcher_test_dir/symlink-mutant"
  sed 's/\[ -f "$runtime_root\/$path" \] && \[ ! -L "$runtime_root\/$path" \]/[ -f "$runtime_root\/$path" ]/' "$launcher_test_dir/symlink-mutant" > "$launcher_test_dir/symlink-mutant.tmp"
  mv "$launcher_test_dir/symlink-mutant.tmp" "$launcher_test_dir/symlink-mutant"
  if WOR_LAUNCHER_SOURCE_ONLY=1 WOR_APP_RESOURCES_DIR="$resources_dir" WOR_APP_INFO_PLIST="$info_plist" bash -c '
    set -e
    source "$1"
    cp -R "$EMBEDDED_RUNTIME" "$2/symlink-runtime"
    cp "$EMBEDDED_MANIFEST" "$2/symlink-manifest.json"
    rm "$2/symlink-runtime/install-wor.sh"
    ln -s "$EMBEDDED_RUNTIME/install-wor.sh" "$2/symlink-runtime/install-wor.sh"
    validate_runtime "$2/symlink-runtime" "$2/symlink-manifest.json"
  ' _ "$launcher_test_dir/symlink-mutant" "$launcher_test_dir" ;then
    pass "runtime-integrity test detects a removed symlink guard"
  else
    fail "symlink mutation did not alter the focused test behavior"
  fi

  cp "$launcher" "$launcher_test_dir/archive-mutant"
  sed 's/substr($1, 1, 1) != "-" && substr($1, 1, 1) != "d"/0/' "$launcher_test_dir/archive-mutant" > "$launcher_test_dir/archive-mutant.tmp"
  mv "$launcher_test_dir/archive-mutant.tmp" "$launcher_test_dir/archive-mutant"
  mkdir -p "$launcher_test_dir/mutation-link-archive"
  ln -s /tmp "$launcher_test_dir/mutation-link-archive/escape"
  tar -czf "$launcher_test_dir/mutation-link.tar.gz" -C "$launcher_test_dir/mutation-link-archive" escape
  if WOR_LAUNCHER_SOURCE_ONLY=1 WOR_APP_RESOURCES_DIR="$resources_dir" WOR_APP_INFO_PLIST="$info_plist" bash -c '
    source "$1"
    archive_entries_are_safe "$2/mutation-link.tar.gz"
  ' _ "$launcher_test_dir/archive-mutant" "$launcher_test_dir" ;then
    pass "archive-safety test detects a removed entry-type guard"
  else
    fail "archive entry-type mutation did not alter the focused test behavior"
  fi

  awk '
    /selected=.*ACTIVE_RUNTIME_FILE/ { sub("ACTIVE_RUNTIME_FILE", "PREVIOUS_RUNTIME_FILE") }
    /\|\| selected=.*PREVIOUS_RUNTIME_FILE/ { sub("PREVIOUS_RUNTIME_FILE", "ACTIVE_RUNTIME_FILE") }
    { print }
  ' "$launcher" > "$launcher_test_dir/fallback-mutant"
  if WOR_LAUNCHER_SOURCE_ONLY=1 WOR_APP_SUPPORT_DIR="$launcher_test_dir/support" WOR_APP_RESOURCES_DIR="$resources_dir" WOR_APP_INFO_PLIST="$info_plist" bash -c '
    set -e
    source "$1"
    CHECKOUT_DIR="$2/no-checkout"
    bootstrap_embedded_runtime
    mkdir -p "$RUNTIMES_DIR/9.9.9"
    cp -R "$EMBEDDED_RUNTIME" "$RUNTIMES_DIR/9.9.9/runtime"
    sed "s/\"version\": \"[^\"]*\"/\"version\": \"9.9.9\"/" "$EMBEDDED_MANIFEST" > "$RUNTIMES_DIR/9.9.9/runtime-manifest.json"
    write_runtime_pointer "$PREVIOUS_RUNTIME_FILE" 9.9.9
    select_runtime
    [ "$(manifest_version "$RUNTIME_MANIFEST")" == 9.9.9 ]
  ' _ "$launcher_test_dir/fallback-mutant" "$launcher_test_dir" ;then
    pass "runtime-selection test detects an active/previous priority mutation"
  else
    fail "fallback-order mutation did not alter the focused test behavior"
  fi
  rm -rf "$launcher_test_dir"

  launcher_test_dir="$(mktemp -d)"
  if TMPDIR="$launcher_test_dir" WOR_LAUNCHER_SOURCE_ONLY=1 bash -c '
    source "$1"
    [ -z "$(find "$2" -mindepth 1 -print -quit)" ] || exit 1
    STARTUP_STATUS="$2/status"
    STARTUP_DONE="$2/status.done"
    startup_phase "Checking dependencies..."
    [ "$(cat "$STARTUP_STATUS")" == "Checking dependencies..." ] || exit 1
    cleanup_startup
    [ ! -e "$STARTUP_STATUS" ] && [ ! -e "$STARTUP_DONE" ]
  ' _ "$launcher" "$launcher_test_dir" ;then
    pass "macOS launcher startup status is source-testable and cleans up its state"
  else
    fail "macOS launcher startup status leaks state or runs during source-only tests"
  fi
  rm -rf "$launcher_test_dir"

  launcher_test_dir="$(mktemp -d)"
  if TMPDIR="$launcher_test_dir" WOR_LAUNCHER_SOURCE_ONLY=1 BREW_CALLS="$launcher_test_dir/brew-calls" bash -c '
    source "$1"
    WOR_MACOS_BREW_FORMULAE=(git jq wimlib)
    brew() {
      printf "%s\n" "$*" >> "$BREW_CALLS"
      if [ "$1 $2" == "list --formula" ];then
        printf "git\n"
      fi
    }
    ask_to_continue() { [ "$2" == Install ]; }
    prepare_dependencies || exit 1
    [ "$(grep -c "^list --formula$" "$BREW_CALLS")" -eq 1 ] || exit 1
    [ "$(grep -c "^install jq wimlib$" "$BREW_CALLS")" -eq 1 ] || exit 1
    [ "$(wc -l < "$BREW_CALLS" | tr -d " ")" -eq 2 ] || exit 1
    [ "$HOMEBREW_NO_AUTO_UPDATE" == 1 ]
  ' _ "$launcher" ;then
    pass "macOS launcher inventories Homebrew once and installs only missing dependencies"
  else
    fail "macOS launcher repeats Homebrew inventory or installs the wrong dependencies"
  fi
  rm -rf "$launcher_test_dir"

  launcher_test_dir="$(mktemp -d)"
  repair_checkout="$launcher_test_dir/checkout"
  mkdir -p "$repair_checkout/assets"
  printf 'original script\n' > "$repair_checkout/install-wor.sh"
  printf 'next steps\n' > "$repair_checkout/assets/next-steps.png"
  git -C "$repair_checkout" init -q
  git -C "$repair_checkout" add install-wor.sh assets/next-steps.png
  git -C "$repair_checkout" -c user.name=Test -c user.email=test@example.com commit -qm fixture
  printf 'modified script\n' > "$repair_checkout/install-wor.sh"
  rm "$repair_checkout/assets/next-steps.png"
  if WOR_LAUNCHER_SOURCE_ONLY=1 bash -c '
    set -e
    source "$1"
    REPO_DIR="$2"
    ask_to_continue() { [ "$2" == Repair ]; }
    show_error() { printf "unexpected error: %s\n" "$1" >&2; return 1; }
    repair_missing_files
    [ "$(cat "$REPO_DIR/assets/next-steps.png")" == "next steps" ]
    [ "$(cat "$REPO_DIR/install-wor.sh")" == "modified script" ]
  ' _ "$launcher" "$repair_checkout" ;then
    pass "macOS startup repairs missing tracked runtime assets without overwriting modifications"
  else
    fail "macOS startup repair misses assets or overwrites a modified file"
  fi
  rm -rf "$launcher_test_dir"

  #the git source updater was removed outright; only local repair and the verified runtime update remain
  if WOR_LAUNCHER_SOURCE_ONLY=1 bash -c '
    source "$1"
    declare -F check_for_update >/dev/null && exit 1
    declare -F repair_missing_files >/dev/null || exit 1
    declare -F check_for_runtime_update >/dev/null || exit 1
  ' _ "$launcher" \
    && ! grep -qF 'ls-remote' "$launcher" \
    && ! grep -qF 'merge --ff-only' "$launcher" ;then
    pass "macOS launcher no longer updates a source checkout from a git remote"
  else
    fail "macOS launcher still carries the git source updater"
  fi

  metadata_name="$(run_in_engine 'printf %s "$WOR_FLASHER_NAME"')"
  metadata_version="$(run_in_engine 'printf %s "$WOR_FLASHER_VERSION"')"
  metadata_logo="$(run_in_engine 'printf %s "$WOR_LOGO_FILENAME"')"
  metadata_assets="$(run_in_engine 'printf %s "$WOR_ASSETS_DIRNAME"')"
  metadata_icon="$(run_in_engine 'printf %s "$WOR_ICON_FILENAME"')"
  info_plist="$REPO_DIR/release/macos/WoR-Flasher.app/Contents/Info.plist"
  plutil -lint "$info_plist" >/dev/null 2>&1 \
    && [ "$(plutil -extract CFBundleDisplayName raw -o - "$info_plist")" == "$metadata_name" ] \
    && [ "$(plutil -extract CFBundleExecutable raw -o - "$info_plist")" == "$metadata_name" ] \
    && [ "$(plutil -extract CFBundleIconFile raw -o - "$info_plist")" == "$metadata_icon" ] \
    && [ "$(plutil -extract CFBundleIconName raw -o - "$info_plist")" == "$metadata_name" ] \
    && [ -s "$REPO_DIR/release/macos/WoR-Flasher.app/Contents/Resources/$metadata_icon" ] \
    && [ -f "$REPO_DIR/release/macos/WoR-Flasher.app/Contents/Resources/$metadata_logo" ] \
    && cmp -s "$REPO_DIR/$metadata_assets/$metadata_logo" "$REPO_DIR/release/macos/WoR-Flasher.app/Contents/Resources/$metadata_logo" \
    && [ "$(plutil -extract CFBundleName raw -o - "$info_plist")" == "$metadata_name" ] \
    && [ "$(plutil -extract CFBundleShortVersionString raw -o - "$info_plist")" == "$metadata_version" ] \
    && [ "$(plutil -extract CFBundleVersion raw -o - "$info_plist")" == "$metadata_version" ] \
    && pass "macOS app bundle metadata matches the shared name, version, and logo" \
    || fail "macOS app bundle metadata differs from the shared name, version, or logo"
}
