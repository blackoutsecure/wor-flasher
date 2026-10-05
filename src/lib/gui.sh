#!/bin/bash

#GUI definitions for both front-ends, in three sections: shared, then macOS, then Linux.
#The two toolkits render very differently, but they must never describe the same option differently,
#which is what happened when a "not recommended" marker was added on macOS and the Linux wording
#drifted behind it. Kept in one file to match cleanup.sh, which likewise branches by platform inside
#a single concern, and because every entry point sources this unconditionally on both platforms.

# --- Shared -----------------------------------------------------------------------------------
#Every user-facing option label lives here exactly once.
#Ordering stays with each front-end: yad parses its results positionally, so its field order is load-bearing.

wor_rpi_board_options() { #Output: separate board labels shared by the macOS and Linux target pickers.
	if [ "${1:-desktop}" == iot-core ];then
		printf '%s\n' 'Raspberry Pi 3 Model B' 'Raspberry Pi 2 v1.2' 'Raspberry Pi 2 v1.1'
		return
	fi
	printf '%s\n' 'Raspberry Pi 5' 'Raspberry Pi 4 / Pi 400' 'Raspberry Pi 3' 'Raspberry Pi 2 v1.2'
}

wor_iot_option_label() { #Input: IoT option key. Labels shared by both Advanced Options forms.
	case "$1" in
		official) printf 'Official Microsoft Image' ;;
		recommended) printf 'Recommended' ;;
		local) printf 'Custom Image' ;;
		source) printf 'Image source' ;;
		file) printf 'Custom Image file' ;;
		cache) printf 'Download folder' ;;
		dryrun) printf 'Inspect only; do not write the drive' ;;
		playSound) printf 'Play a completion sound' ;;
		sound) printf 'Completion sound' ;;
		notification) printf 'Show a completion notification' ;;
		hdmi) printf 'HDMI display' ;;
		editHdmi) printf 'View / Edit config.txt (video only)' ;;
		hdmiHelp) printf 'Boot config.txt video settings only. Required IoT boot settings stay protected; Windows may select its own display mode.' ;;
		hdmiEditorHelp) printf 'These HDMI key=value lines are merged into the existing IoT boot config.txt. Required Windows boot, memory and framebuffer settings are preserved. Use hdmi_group and hdmi_mode; optional: hdmi_force_hotplug and hdmi_drive. Save Advanced Options to apply them to newly flashed media.' ;;
		accountSetup) printf 'Change IoT administrator after first boot' ;;
		accountUsername) printf 'IoT administrator username' ;;
		accountPassword) printf 'New IoT password' ;;
		accountHelp) printf 'Optional post-boot SSH step; not desktop unattended setup. Boot the Pi, confirm its SSH identity and enter its current password. Account changes stay Pending until the new login is verified.' ;;
		languageSetup) printf 'Apply IoT language after first boot' ;;
		language) printf 'IoT language' ;;
		languageHelp) printf 'Defaults to the host language, like desktop Windows. The selected language must already be installed in IoT Core; it is applied under DefaultAccount and remains Pending until verified.' ;;
		verification) printf 'Download/import and full image verification begin after Flash. Package and FFU integrity, board compatibility and written-image read-back are required; these checks cannot be disabled.' ;;
		*) warning "Unknown IoT option label: $1"; return 1 ;;
	esac
}

#Output: key<TAB>label<TAB>caution<TAB>recommended for every Advanced Options toggle.
#caution 1 marks an option that departs from the tested defaults, so both front-ends can flag it.
wor_advanced_toggles() { #Input: pinned UEFI, pinned driver, Pi model, Windows family, installation mode.
	local drivers_recommended=0
	case "$3" in 3 | 4) drivers_recommended=1 ;; esac
	printf 'oobe\tAllow Windows 11 setup to continue without a network connection\t0\t0\n'
	printf 'pi4\t%s\t0\t0\n' "$(wor_pi4_label "$3")"
	if [ "$3" == 3 ];then
		printf 'uefi\tUse the latest UEFI firmware\t0\t1\n'
	else
		printf 'uefi\tUse the latest UEFI firmware\t1\t0\n'
	fi
	printf 'drivers\tUse the latest Windows ARM64 drivers\t0\t%s\n' "$drivers_recommended"
	if [ "${5:-1}" == 0 ];then
		printf 'verify\tSkip verifying the prepared recovery media\t1\t0\n'
	else
		printf 'verify\tSkip verifying the written image after flashing\t1\t0\n'
	fi
	printf 'dryrun\tSkip flashing the device (dry run)\t0\t0\n'
}

wor_advanced_label() { #Input: key, pinned UEFI, pinned driver, Pi model, Windows family, installation mode.
	wor_advanced_toggles "$2" "$3" "$4" "${5:-}" "${6:-1}" | awk -F'\t' -v key="$1" '$1 == key {print $2}'
}

wor_advanced_caution() { #Input: key, optional Pi model. Output: 1 when the option departs from model defaults.
	wor_advanced_toggles '' '' "${2:-}" | awk -F'\t' -v key="$1" '$1 == key {print $3}'
}

wor_advanced_recommended() { #Input: key, Pi model. Output: 1 when the option gets a Recommended label.
	wor_advanced_toggles '' '' "$2" | awk -F'\t' -v key="$1" '$1 == key {print $4}'
}

wor_release_version_label() { #Input: tag, verified latest tag. Display text only; downloads keep the raw tag.
	printf '%s' "$1"
	[ -z "$2" ] || [ "$1" != "$2" ] || printf ' (latest) [recommended]'
}

wor_pi4_label() { #Output: label for the applicable Pi 4 RAM-unlock toggle.
	printf 'Automatically disable the Pi 4 3 GB RAM limit after install'
}

wor_setup_scope() { #Input: Windows family, installation mode. Output: the target of first-boot customization.
	if [ "$2" == 0 ];then
		printf '%s setup on the destination drive' "$1"
	else
		printf '%s setup on this drive' "$1"
	fi
}

wor_advanced_context() { #Input: Windows family, installation mode. Output: scope explanation for both GUIs.
	if [ "$2" == 0 ];then
		printf 'Creating recovery media for %s.\nSetup, account and regional choices apply when WoR-PE installs Windows on another drive.' "$1"
	else
		printf 'Preparing %s for installation on this drive.\nSetup, account and regional choices apply when WoR-PE installs Windows.' "$1"
	fi
}

wor_config_scope() { #Input: 1 when the installer writes the target drive directly. Output: where config.txt lands.
	if [ "$1" == 1 ];then
		printf 'boot partition'
	else
		printf 'recovery media, not the Windows drive'
	fi
}

wor_config_txt_label() { #Input: config scope. Output: the config.txt toggle label.
	printf 'Apply the customized config.txt to the %s' "$1"
}

wor_host_platform() { #Output: the config key naming this host: macos, linux or other.
	case "$(uname -s 2>/dev/null)" in
		Darwin) printf 'macos' ;;
		Linux) printf 'linux' ;;
		*) printf 'other' ;;
	esac
}

wor_sound_options() { #Output: value<TAB>label for every completion sound this host can play, or nothing.
	local name
	case "$(wor_host_platform)" in
		macos)
			#every macOS install ships these in /System/Library/Sounds, so nothing has to be bundled
			for name in Basso Blow Bottle Frog Funk Glass Hero Morse Ping Pop Purr Sosumi Submarine Tink ;do
				printf '%s\t%s\n' "$name" "$name"
			done
			;;
		linux)
			#freedesktop sound-theme event names, resolved by canberra or the shipped .oga files
			printf 'complete\tComplete\n'
			printf 'bell\tBell\n'
			printf 'message\tMessage\n'
			printf 'device-added\tDevice added\n'
			printf 'dialog-information\tInformation\n'
			;;
	esac
}

wor_sound_default() { #Output: the completion sound used when the config names none, or nothing on a host with no catalogue.
	case "$(wor_host_platform)" in
		macos) printf 'Glass' ;;
		linux) printf 'complete' ;;
	esac
}

wor_sound_label() { #Input: sound value. Output: its menu label, or the value itself when unknown.
	wor_sound_options | awk -F'\t' -v v="$1" '$1 == v {print $2; found=1} END {if (!found) print v}'
}

wor_completion_sound() { #Output: the sound to play, ignoring a value this host cannot play.
	local candidate="${COMPLETION_SOUND:-}"
	if [ -n "$candidate" ] && wor_sound_options | awk -F'\t' -v v="$candidate" '$1 == v {found=1} END {exit !found}' ;then
		printf '%s' "$candidate"
		return 0
	fi
	wor_sound_default
}


# --- macOS (AppKit through JXA) ----------------------------------------------------------------
#Linux has no equivalent of these calls; they are macOS-only by nature rather than by omission.

wor_jxa_window_lib() { #Output: the JXA every window is built with, so no screen can drift from the others.
	cat <<'JXA'
//Prepended to every WoR-Flasher window. Each screen still lays out its own content and picks its own
//size, but the title bar, menu bar and Dock icon come from here so they cannot differ between screens.
function worSetAppIcon(app, iconPath) {
  if (iconPath && iconPath.length > 0) {
    app.setApplicationIconImage($.NSImage.alloc.initWithContentsOfFile($(iconPath)))
  }
}

function worAppVersion(windowTitle, appTitle) {
  const prefix = appTitle + ' v'
  if (windowTitle && windowTitle.indexOf(prefix) === 0) return windowTitle.substring(prefix.length)
  return ''
}

//macOS does not deliver app-menu action dispatch to ANY item - custom or native, including Quit -
//while the screen-specific app.runModalForWindow session is active (every screen using this menu is one
//of those). A clickable-looking item that silently does nothing on click is worse than a disabled,
//informational one, so this shows the product name and version only; Quit remains reachable through
//the Dock, Cmd-Q (handled separately via Apple Events, see worInstallWindowHandlers) and each
//screen-specific Cancel/Abort/close-box button.
function worInstallAppMenu(app, appTitle, windowTitle, iconPath) {
  const mainMenu = $.NSMenu.alloc.initWithTitle(appTitle)
  const appMenuItem = $.NSMenuItem.alloc.init
  appMenuItem.title = appTitle
  const appMenu = $.NSMenu.alloc.initWithTitle(appTitle)
  const version = worAppVersion(windowTitle, appTitle)
  const infoItem = appMenu.addItemWithTitleActionKeyEquivalent(version.length > 0 ? appTitle + ' v' + version : appTitle, '', '')
  infoItem.enabled = false
  appMenuItem.submenu = appMenu
  mainMenu.addItem(appMenuItem)
  app.mainMenu = mainMenu
  //returned so a screen can append its own menu, e.g. Edit for the text fields in Advanced Options
  return mainMenu
}

//options: width, height, title, delegate, closable (default true)
function worMakeWindow(options) {
  let style = $.NSWindowStyleMaskTitled
  if (options.miniaturizable !== false) style = style | $.NSWindowStyleMaskMiniaturizable
  //a screen opts out only where closing the window would not stop the work behind it
  if (options.closable !== false) style = style | $.NSWindowStyleMaskClosable
  const window = $.NSWindow.alloc.initWithContentRectStyleMaskBackingDefer($.NSMakeRect(0, 0, options.width, options.height), style, $.NSBackingStoreBuffered, false)
  window.title = options.title
  window.center
  //no window here is resizable, and without that mask the zoom button is drawn disabled rather than
  //omitted, so hide it outright instead of leaving a dead third dot in the title bar
  const zoomButton = window.standardWindowButton($.NSWindowZoomButton)
  if (zoomButton) zoomButton.hidden = true
  if (options.delegate) window.setDelegate(options.delegate)
  window.contentView = $.NSView.alloc.initWithFrame($.NSMakeRect(0, 0, options.width, options.height))
  return window
}

//a coloured trailing note tells the reader at a glance whether an option matches the tested defaults
function worAnnotateCheckbox(checkbox, label, note, color) {
  const suffix = '  ' + note
  const title = $.NSMutableAttributedString.alloc.init
  title.mutableString.appendString($(label + suffix))
  const labelRange = $.NSMakeRange(0, label.length)
  const noteRange = $.NSMakeRange(label.length, suffix.length)
  title.addAttributeValueRange($.NSFontAttributeName, $.NSFont.systemFontOfSize(13), labelRange)
  title.addAttributeValueRange($.NSForegroundColorAttributeName, $.NSColor.labelColor, labelRange)
  title.addAttributeValueRange($.NSFontAttributeName, $.NSFont.systemFontOfSizeWeight(13, $.NSFontWeightSemibold), noteRange)
  title.addAttributeValueRange($.NSForegroundColorAttributeName, color, noteRange)
  checkbox.attributedTitle = title
}

function worEditText(value, title, message, iconPath, defaults) {
  const editorScroll = $.NSScrollView.alloc.initWithFrame($.NSMakeRect(0, 0, 560, 320))
  editorScroll.borderType = $.NSBezelBorder
  editorScroll.hasVerticalScroller = true
  const editor = $.NSTextView.alloc.initWithFrame(editorScroll.bounds)
  editor.font = $.NSFont.userFixedPitchFontOfSize(12)
  editor.string = $(value)
  editor.autoresizingMask = $.NSViewWidthSizable | $.NSViewHeightSizable
  editorScroll.documentView = editor
  const dialog = $.NSAlert.alloc.init
  dialog.messageText = $(title)
  dialog.informativeText = $(message)
  dialog.alertStyle = $.NSAlertStyleInformational
  const icon = $.NSImage.alloc.initWithContentsOfFile($(iconPath))
  if (!icon.isNil()) dialog.icon = icon
  dialog.accessoryView = editorScroll
  dialog.addButtonWithTitle('Save')
  dialog.addButtonWithTitle('Cancel')
  if (typeof defaults === 'string') dialog.addButtonWithTitle('Reset to 720p defaults')
  while (true) {
    const response = dialog.runModal
    if (response === $.NSAlertFirstButtonReturn) return { saved: true, text: ObjC.unwrap(editor.string) }
    if (response !== $.NSAlertThirdButtonReturn || typeof defaults !== 'string') return { saved: false, text: value }
    editor.string = $(defaults)
  }
}

//the Dock Quit item sends an aevt/quit Apple Event that a modal session would otherwise never see,
//and a modal session starves the default run loop, so events have to be pumped by hand
function worInstallWindowHandlers(controller) {
  $.NSAppleEventManager.sharedAppleEventManager.setEventHandlerAndSelectorForEventClassAndEventID(controller, 'handleQuitEvent:withReplyEvent:', 0x61657674, 0x71756974)
  $.NSAppleEventManager.sharedAppleEventManager.setEventHandlerAndSelectorForEventClassAndEventID(controller, 'handleReopenEvent:withReplyEvent:', 0x61657674, 0x72617070)
  const pumpTimer = $.NSTimer.timerWithTimeIntervalTargetSelectorUserInfoRepeats(0.25, controller, 'pumpEvents:', $(), true)
  $.NSRunLoop.currentRunLoop.addTimerForMode(pumpTimer, $.NSModalPanelRunLoopMode)
  return pumpTimer
}
JXA
}


# --- Linux (yad) ------------------------------------------------------------------------------
#macOS has no equivalent of these flags; they are Linux-only by nature rather than by omission.

wor_detect_yad_screen() { #Sets WOR_YAD_SCREEN_WIDTH/HEIGHT from the active desktop, with a conservative fallback.
	local geometry=''
	[[ "${WOR_YAD_SCREEN_WIDTH:-}x${WOR_YAD_SCREEN_HEIGHT:-}" =~ ^[0-9]+x[0-9]+$ ]] && return 0
	if command -v xrandr >/dev/null ;then
		geometry="$(xrandr --current 2>/dev/null | awk '$0 ~ /[0-9]+x[0-9]+/ && $0 ~ /\*/ {print $1; exit}')"
	fi
	if [[ ! "$geometry" =~ ^[0-9]+x[0-9]+$ ]] && command -v xdpyinfo >/dev/null ;then
		geometry="$(xdpyinfo 2>/dev/null | awk '/dimensions:/ {print $2; exit}')"
	fi
	if [[ ! "$geometry" =~ ^[0-9]+x[0-9]+$ ]] && command -v xwininfo >/dev/null ;then
		local root_width root_height
		root_width="$(xwininfo -root 2>/dev/null | awk '/Width:/ {print $2; exit}')"
		root_height="$(xwininfo -root 2>/dev/null | awk '/Height:/ {print $2; exit}')"
		geometry="${root_width}x${root_height}"
	fi
	[[ "$geometry" =~ ^[0-9]+x[0-9]+$ ]] || geometry='1024x768'
	WOR_YAD_SCREEN_WIDTH="${geometry%x*}"
	WOR_YAD_SCREEN_HEIGHT="${geometry#*x}"
}

wor_yad_width() { #Input: desired width. Output: width clamped inside the detected desktop.
	local desired_width="$1" maximum_width=$((WOR_YAD_SCREEN_WIDTH - 40))
	[ "$maximum_width" -gt 0 ] || maximum_width="$WOR_YAD_SCREEN_WIDTH"
	[ "$desired_width" -le "$maximum_width" ] || desired_width="$maximum_width"
	printf '%s' "$desired_width"
}

wor_yad_height() { #Input: desired height. Output: height clamped inside the detected desktop.
	local desired_height="$1" maximum_height=$((WOR_YAD_SCREEN_HEIGHT - 60))
	[ "$maximum_height" -gt 0 ] || maximum_height="$WOR_YAD_SCREEN_HEIGHT"
	[ "$desired_height" -le "$maximum_height" ] || desired_height="$maximum_height"
	printf '%s' "$desired_height"
}

wor_yad_image_for_screen() { #Input: preferred image, fallback image, minimum width and height for preferred image.
	if [ "$WOR_YAD_SCREEN_WIDTH" -ge "$3" ] && [ "$WOR_YAD_SCREEN_HEIGHT" -ge "$4" ];then
		printf '%s' "$1"
	else
		printf '%s' "$2"
	fi
}

wor_init_yad_flags() { #Sets the shared yadflags array. Assigns rather than echoes, because it is an array.
	wor_detect_yad_screen
	export GIO_USE_VOLUME_MONITOR=unix
	yadflags=(--center --fixed --buttons-layout=center --width="$(wor_yad_width 400)" --height="$(wor_yad_height 250)" --window-icon="$WOR_LOGO_PATH" --class="$WOR_ICON_NAME" --title="$WOR_WINDOW_TITLE" --separator='\n')
}

wor_yad_label() { #Input: label, caution flag, optional recommendation flag. Output: annotated checkbox label.
	#yad cannot colour a checkbox label, so the macOS badges become plain text here
	if [ "$2" == 1 ];then
		printf '%s (not recommended)' "$1"
	elif [ "${3:-0}" == 1 ];then
		printf '%s (recommended)' "$1"
	else
		printf '%s' "$1"
	fi
}

wor_yad_bool() { #Input: 1 or 0. Output: the TRUE/FALSE yad expects for a check box.
	[ "$1" == 1 ] && printf 'TRUE' || printf 'FALSE'
}

wor_play_result_sound() { #Input: success or failure. The macOS side uses NSSound; this is the yad equivalent.
	#Best effort only: backgrounded so it cannot delay the dialog, and always successful so a desktop
	#without a sound theme or player never turns a finished flash into a failed run
	[ "${PLAY_SOUND:-1}" == 1 ] || return 0
	local event='dialog-error'
	if [ "$1" == success ];then
		#only the completion sound is user-selectable; a failure keeps the desktop's own error sound
		event="$(wor_completion_sound)"
		[ -n "$event" ] || return 0
	fi
	if command -v canberra-gtk-play >/dev/null ;then
		canberra-gtk-play -i "$event" >/dev/null 2>&1 &
	elif command -v paplay >/dev/null && [ -f "/usr/share/sounds/freedesktop/stereo/${event}.oga" ] ;then
		paplay "/usr/share/sounds/freedesktop/stereo/${event}.oga" >/dev/null 2>&1 &
	fi
	return 0
}

wor_show_result_notification() { #Input: success or failure, optional Windows family. Reports the selected media result.
	#Best effort only: backgrounded, and always successful, so a denied notification permission or a
	#desktop with no notification daemon never turns a finished flash into a failed run
	[ "${SHOW_NOTIFICATION:-1}" == 1 ] || return 0
	local title="${WOR_APP_TITLE:-WoR-Flasher}" body windows_name="${2:-Windows}"
	if [ "$1" == success ] && [ "${WOR_IMAGE_FAMILY:-desktop}" == iot-core ] && [ "${DRY_RUN:-0}" == 1 ];then
		body="Verified the Windows 10 IoT Core image. No drive was written; hardware boot has not been assessed."
	elif [ "$1" == success ];then
		body="Finished preparing $windows_name media on ${DEVICE:-the drive}. It is ready to boot on your Raspberry Pi."
	else
		body="Preparing $windows_name media stopped before it finished. Open ${title} for details."
	fi
	case "$(wor_host_platform)" in
		macos)
			#passed as argv, never pasted into the source: a device path or title must not be able to close
			#a string literal and run as AppleScript
			wor_osascript - "$title" "$body" >/dev/null 2>&1 <<'APPLESCRIPT' &
on run argv
  display notification (item 2 of argv) with title (item 1 of argv)
end run
APPLESCRIPT
			;;
		linux)
			command -v notify-send >/dev/null && notify-send "$title" "$body" >/dev/null 2>&1 &
			;;
	esac
	return 0
}
