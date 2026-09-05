#!/bin/bash

#Canonical product metadata and the shared macOS script-host helper.
#Every entry point loads this file, so nothing here may depend on another library.
WOR_FLASHER_NAME='WoR-Flasher'
WOR_FLASHER_VERSION='1.0.2'
WOR_ASSETS_DIRNAME='assets'
WOR_LOGO_FILENAME='logo-full.png'
WOR_ICON_FILENAME='WoR-Flasher.icns'
WOR_ICON_NAME='WoR-Flasher'
: "${WOR_APP_TITLE:=$WOR_FLASHER_NAME}"
: "${WOR_WINDOW_TITLE:=$WOR_FLASHER_NAME v$WOR_FLASHER_VERSION}"

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
