#!/bin/bash

#Canonical product metadata and the shared macOS script-host helper.
#Every entry point loads this file, so nothing here may depend on another library.
WOR_METADATA_CONFIG="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/config/metadata.json"

wor_metadata_value() { #Input: section and key. Output: scalar value from src/config/metadata.json.
	awk -v section="$1" -v key="$2" '
		BEGIN { quote=sprintf("%c", 34) }
		$0 ~ "^[[:space:]]*" quote section quote "[[:space:]]*:" { in_section=1; next }
		in_section && $0 ~ /^[[:space:]]*}/ { exit }
		in_section {
			pattern="^[[:space:]]*" quote key quote "[[:space:]]*:[[:space:]]*"
			if ($0 ~ pattern) {
				line=$0
				sub(pattern, "", line)
				sub(/,[[:space:]]*$/, "", line)
				if (substr(line, 1, 1) == quote) {
					line=substr(line, 2)
					if (substr(line, length(line), 1) == quote) line=substr(line, 1, length(line) - 1)
				}
				print line
				exit
			}
		}
	' "$WOR_METADATA_CONFIG"
}

wor_metadata_required() { #Input: section, key, env var. Assigns a required scalar metadata value.
	local value
	value="$(wor_metadata_value "$1" "$2")"
	if [ -z "$value" ];then
		printf 'Required WoR-Flasher metadata %s.%s is missing from %s.\n' "$1" "$2" "$WOR_METADATA_CONFIG" >&2
		return 1
	fi
	printf -v "$3" '%s' "$value"
}

wor_metadata_required product name WOR_FLASHER_NAME || return 1
wor_metadata_required product version WOR_FLASHER_VERSION || return 1
wor_metadata_required product assetsDirname WOR_ASSETS_DIRNAME || return 1
wor_metadata_required product logoFilename WOR_LOGO_FILENAME || return 1
wor_metadata_required product iconFilename WOR_ICON_FILENAME || return 1
wor_metadata_required product iconName WOR_ICON_NAME || return 1
wor_metadata_required systemDefaults peInstallerUrl WOR_DEFAULT_PE_INSTALLER_URL || return 1
wor_metadata_required systemDefaults peInstallerSha256 WOR_DEFAULT_PE_INSTALLER_SHA256 || return 1
wor_metadata_required systemDefaults uefiVerPi3 WOR_DEFAULT_UEFI_VER_PI3 || return 1
wor_metadata_required systemDefaults uefiVerPi4 WOR_DEFAULT_UEFI_VER_PI4 || return 1
wor_metadata_required systemDefaults uefiVerPi5 WOR_DEFAULT_UEFI_VER_PI5 || return 1
wor_metadata_required systemDefaults uefiRepoPi3 WOR_DEFAULT_UEFI_REPO_PI3 || return 1
wor_metadata_required systemDefaults uefiRepoPi4 WOR_DEFAULT_UEFI_REPO_PI4 || return 1
wor_metadata_required systemDefaults uefiRepoPi5 WOR_DEFAULT_UEFI_REPO_PI5 || return 1
wor_metadata_required systemDefaults driverVer WOR_DEFAULT_DRIVER_VER || return 1
wor_metadata_required systemDefaults driversRepo WOR_DEFAULT_DRIVERS_REPO || return 1
wor_metadata_required systemDefaults armv80MaxBuild WOR_DEFAULT_ARMV80_MAX_BUILD || return 1
wor_metadata_required systemDefaults win11MinBuild WOR_DEFAULT_WIN11_MIN_BUILD || return 1
wor_metadata_required systemDefaults win10OldestBuild WOR_DEFAULT_WIN10_OLDEST_BUILD || return 1
wor_metadata_required systemDefaults exampleBid WOR_DEFAULT_EXAMPLE_BID || return 1
wor_metadata_required systemDefaults armv80SafeBid WOR_DEFAULT_ARMV80_SAFE_BID || return 1
wor_metadata_required systemDefaults repoSlug WOR_DEFAULT_REPO_SLUG || return 1
wor_metadata_required systemDefaults logDirname WOR_DEFAULT_LOG_DIRNAME || return 1
: "${WOR_APP_TITLE:=$WOR_FLASHER_NAME}"
: "${WOR_WINDOW_TITLE:=$WOR_FLASHER_NAME v$WOR_FLASHER_VERSION}"
: "${WOR_RUN_ID:=$(date -u +%Y%m%dT%H%M%SZ)}"

wor_osascript() {
	local host_dir="${TMPDIR:-/tmp}/wor-flasher-script-host-${UID}"
	local script_host="$host_dir/$WOR_FLASHER_NAME"
	if [ -e "$host_dir" ];then
		[ -d "$host_dir" ] && [ "$(stat -f '%u' "$host_dir" 2>/dev/null)" == "$UID" ] || return 1
	else
		mkdir -m 700 "$host_dir" || return 1
	fi
	chmod 700 "$host_dir" || return 1
	ln -sfn /usr/bin/osascript "$script_host" || return 1
	"$script_host" "$@"
}
