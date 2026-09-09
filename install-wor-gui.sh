#!/bin/bash

#WoR-Flasher graphical front-end for Linux and macOS.
#Presentation only: every decision, validation and device operation belongs to install-wor.sh,
#which this script sources so the two can never disagree about what actually gets flashed.
#Native AppKit/JXA dialogs are used on macOS and yad dialogs on Linux; the two must stay aligned.
#Version, licensing and attribution are defined in src/lib/metadata.sh and install-wor.sh.
#
#Original author: Botspot - https://github.com/Botspot/wor-flasher
#Maintained with Blackout Secure support - https://github.com/Botspot/wor-flasher

: "${WOR_ANNOUNCEMENT_TIMEOUT:=30}"

export RUN_MODE=gui #this variable is detected by install-wor.sh to display gui error messages

#Determine the directory that contains this script
[ -z "$DIRECTORY" ] && DIRECTORY="$(cd "$(dirname "$0")" && pwd -P)"

#On Windows, hand off to the packaged Windows executable if present
if [ "${OS:-}" == "Windows_NT" ] || [[ "$(uname -s 2>/dev/null)" =~ MINGW|MSYS|CYGWIN|Windows ]];then
  if [ -x "$DIRECTORY/release/windows/wor-flasher.exe" ];then
    exec "$DIRECTORY/release/windows/wor-flasher.exe" "$@"
  elif [ -x "$DIRECTORY/release/windows/WoR-Flasher.exe" ];then
    exec "$DIRECTORY/release/windows/WoR-Flasher.exe" "$@"
  fi
fi

#The native app owns macOS preflight and dependency setup. Local development runs the working tree
#directly; opt in to app handoff only when intentionally testing the packaged launcher.
if [ "$(uname -s 2>/dev/null)" == Darwin ] && [ "${WOR_NATIVE_APP:-0}" != 1 ] && [ "${WOR_USE_PACKAGED_APP:-0}" == 1 ] && [ -x "$DIRECTORY/release/macos/WoR-Flasher.app/Contents/MacOS/WoR-Flasher" ];then
  exec /usr/bin/open -W "$DIRECTORY/release/macos/WoR-Flasher.app"
fi

#This script and cli-based install-wor.sh must be in the same directory. Source it before anything else
#so every shared behaviour - error dialogs, status output, drive detection, the settings summary - comes
#from the installer itself and can never drift out of step with what actually gets flashed.
#Nothing above this point may call a shared function, so failures here are reported by hand.
cli_script="$DIRECTORY/install-wor.sh"
if [ ! -d "$DIRECTORY" ] || [ ! -f "$cli_script" ];then
  printf '\033[91m%b\033[0m\n' "No script found named install-wor.sh\nBoth scripts must be in the same directory." 1>&2
  exit 1
fi

repair_missing_checkout_runtime() { #Restore only absent runtime files from local HEAD before shared libraries are sourced.
  local required_path
  local missing=()
  command -v git >/dev/null 2>&1 && git -C "$DIRECTORY" rev-parse --git-dir >/dev/null 2>&1 || return 0
  while IFS= read -r required_path ;do
    [ -e "$DIRECTORY/$required_path" ] || missing+=("$required_path")
  done < <(git -C "$DIRECTORY" ls-tree -r --name-only HEAD -- \
    install-wor.sh install-wor-gui.sh install-wor-hook.sh src/lib src/config src/updater.mjs \
    config-templates assets)
  [ "${#missing[@]}" -gt 0 ] || return 0
  printf 'Repairing missing WoR-Flasher files from local Git HEAD:\n' 1>&2
  printf '  %s\n' "${missing[@]}" 1>&2
  git -C "$DIRECTORY" restore --source=HEAD -- "${missing[@]}" \
    || { printf 'Failed to restore missing WoR-Flasher files from local Git HEAD.\n' 1>&2; return 1; }
}

repair_missing_checkout_runtime || exit 1

#shellcheck disable=SC1090
source "$cli_script" source #by sourcing, this script checks for and applies updates.

#outside the packaged .app (which sets this from its own bundled .icns), fall back to the same
#logo PNG the rest of the app already uses, so the Dock and minimized-window tile are branded
#instead of showing the generic osascript icon
[ -z "$WOR_ICON_PATH" ] && WOR_ICON_PATH="$WOR_LOGO_PATH"

find_macos_gui_process() { #Input: parent pid. Output: first descendant WoR-Flasher script-host pid.
  local child found process_name
  for child in $(pgrep -P "$1" 2>/dev/null) ;do
    process_name="$(ps -p "$child" -o comm= 2>/dev/null)"
    if [ "${process_name##*/}" == "$WOR_FLASHER_NAME" ];then
      printf '%s\n' "$child"
      return 0
    fi
    found="$(find_macos_gui_process "$child")" && { printf '%s\n' "$found"; return 0; }
  done
  return 1
}

activate_macos_gui() { #Input: owning shell pid. Brings its current JXA window forward.
  local gui_pid
  gui_pid="$(find_macos_gui_process "$1")" || return 1
  wor_osascript -l JavaScript - "$gui_pid" <<'JXA' >/dev/null 2>&1
ObjC.import('AppKit')
const pid = Number(ObjC.unwrap($.NSProcessInfo.processInfo.arguments.objectAtIndex(4)))
const runningApp = $.NSRunningApplication.runningApplicationWithProcessIdentifier(pid)
if (!runningApp) $.exit(1)
runningApp.unhide
if (!runningApp.activateWithOptions($.NSApplicationActivateAllWindows | $.NSApplicationActivateIgnoringOtherApps)) $.exit(1)
JXA
}

release_gui_instance() {
  local owner_pid
  owner_pid="$(cat "$WOR_GUI_INSTANCE_DIR/pid" 2>/dev/null)"
  [ "$owner_pid" == "$$" ] || return 0
  rm -f "$WOR_GUI_INSTANCE_DIR/pid"
  rmdir "$WOR_GUI_INSTANCE_DIR" 2>/dev/null
}

acquire_gui_instance() {
  local owner_command owner_pid
  WOR_GUI_INSTANCE_DIR="${TMPDIR:-/tmp}/wor-flasher-gui-${UID}.lock"
  if ! mkdir "$WOR_GUI_INSTANCE_DIR" 2>/dev/null ;then
    owner_pid="$(cat "$WOR_GUI_INSTANCE_DIR/pid" 2>/dev/null)"
    owner_command="$(ps -p "$owner_pid" -o args= 2>/dev/null)"
    if [ -n "$owner_pid" ] && kill -0 "$owner_pid" 2>/dev/null && [[ "$owner_command" == *install-wor-gui.sh* ]];then
      [ "$(uname -s)" != Darwin ] || activate_macos_gui "$owner_pid"
      return 1
    fi
    rm -f "$WOR_GUI_INSTANCE_DIR/pid"
    rmdir "$WOR_GUI_INSTANCE_DIR" 2>/dev/null
    mkdir "$WOR_GUI_INSTANCE_DIR" 2>/dev/null || return 1
  fi
  printf '%s\n' "$$" > "$WOR_GUI_INSTANCE_DIR/pid"
  export WOR_GUI_INSTANCE_DIR
  trap release_gui_instance EXIT
  trap 'exit 130' INT
  trap 'exit 143' HUP TERM
}

acquire_gui_instance || exit 0

echo "DIRECTORY: $DIRECTORY"
echo "DL_DIR: $DL_DIR"

open_url() { #Input: url
  if command -v x-www-browser >/dev/null ;then
    x-www-browser "$1" &
  elif command -v xdg-open >/dev/null ;then
    xdg-open "$1" &
  elif command -v open >/dev/null ;then
    open "$1" &
  else
    error "Failed to locate a browser opener for $1"
  fi
}

kill_process_tree() { #Input: pid. Stops it and everything it started; most of the flash runs under sudo.
  local pid="$1" child
  for child in $(pgrep -P "$pid" 2>/dev/null) ;do
    kill_process_tree "$child"
  done
  kill -TERM "$pid" 2>/dev/null || command sudo -n kill -TERM "$pid" 2>/dev/null
}

gui_start_installer() { #Starts install-wor.sh in the background and waits for it to authenticate. Sets error_marker, output_log, progress_file, done_marker, auth_marker and installer_pid.
  error_marker="$(mktemp)" || error "Failed to create a GUI error marker."
  output_log="$(mktemp)" || error "Failed to create an install log."
  progress_file="$(mktemp)" || error "Failed to create a progress file."
  done_marker="$(mktemp -u)"
  auth_marker="$(mktemp -u)"
  #start with a clean marker; the installer creates it only if an error occurs
  rm -f "$error_marker"

  #No separate terminal is spawned, so the installer inherits this process's exported environment directly.
  export_installer_settings
  export WOR_GUI_ERROR_MARKER="$error_marker"
  export WOR_GUI_PROGRESS_FILE="$progress_file"
  export WOR_GUI_AUTH_MARKER="$auth_marker"
  export WOR_GUI_ABORT_MARKER="${abort_marker:-}"
  #this script already ran the update check while sourcing install-wor.sh; a second one would only
  #repeat the network round-trip, and an update applied here would re-exec the installer mid-launch
  export NO_UPDATE=1
  #only a password retry sets this; the installer still verifies the prepared files before trusting it
  export WOR_RESUME_AT_FLASH="${resume_at_flash:-0}"

  #the job records its own status: a subshell cannot wait on a sibling, so waiting there returned 127 at once
  { "$cli_script" > "$output_log" 2>&1; echo $? > "$done_marker"; } &
  installer_pid=$!

  #macOS opens progress immediately; its askpass dialog activates itself when authentication is needed.
  if [ "${GUI_PROGRESS_EARLY:-0}" != 1 ];then
    while [ ! -e "$auth_marker" ] && [ ! -f "$done_marker" ] ;do
      sleep 0.3
    done
  fi
}

installer_showed_own_error() { #Exit 0 if install-wor.sh already displayed its own native error dialog.
  #gui_error_dialog creates the marker before opening its own native dialog; the log has the details
  [ -e "$error_marker" ]
}

gui_save_failure_log() { #Output: where the installer log was kept. The dialog only shows a tail, and the GUI has no terminal to fall back on.
  local saved_log last_log
  saved_log="$(wor_log_file)"
  mkdir -p "$(dirname "$saved_log")" 2>/dev/null
  mv "$output_log" "$saved_log" 2>/dev/null || saved_log="$output_log"
  last_log="$(wor_last_log_file)"
  if [ "$saved_log" != "$last_log" ];then
    mkdir -p "$(dirname "$last_log")" 2>/dev/null
    cp "$saved_log" "$last_log" 2>/dev/null || true
  fi
  echo "Installer log saved to $saved_log" 1>&2
  echo "$saved_log"
}

gui_log_tail() { #Input: log path. Output: the last lines, with terminal escapes and carriage returns removed.
  #Installer logs can contain raw tool output; macOS sed rejects an invalid UTF-8 byte under the user locale.
  LC_ALL=C sed 's/\x1b\[[0-9;]*[A-Za-z]//g; s/\r//g' "$1" | tail -n 18
}

loading_dialog() { #display a dialog to say something is loading
  local dialog_pid
  #1, not 0: gtk_window_resize asserts height > 0, and GTK still grows the window to its natural size
  (echo '# ' ; sleep infinity) | yad "${yadflags[@]}" --height=1 \
    --progress --pulsate --title="$1" --text="$1" --no-buttons &
  dialog_pid=$!
  trap 'kill "$dialog_pid" 2>/dev/null' EXIT

  sleep infinity
}

stop_loader() {
  [ -z "${loader_pid:-}" ] || kill "$loader_pid" 2>/dev/null
  loader_pid=''
}

macos_choose() { #Input: newline-separated choices, prompt, default, cancel/back label, optional action label/value, optional image path/next label/icon path/window title/cancel value/timeout/process title. Output: selected choice or action value.
  local result choose_jxa
  choose_jxa="$(wor_jxa_window_lib; cat <<'JXA'
ObjC.import('AppKit')
ObjC.import('stdlib')
ObjC.import('Foundation')

const args = $.NSProcessInfo.processInfo.arguments
const rawChoices = ObjC.unwrap(args.objectAtIndex(4))
const choices = rawChoices.length > 0 ? rawChoices.split('\n') : []
const promptText = ObjC.unwrap(args.objectAtIndex(5))
const defaultChoice = ObjC.unwrap(args.objectAtIndex(6))
const cancelLabel = ObjC.unwrap(args.objectAtIndex(7))
const actionLabel = ObjC.unwrap(args.objectAtIndex(8))
const actionValue = ObjC.unwrap(args.objectAtIndex(9))
const imagePath = ObjC.unwrap(args.objectAtIndex(10))
const nextLabel = ObjC.unwrap(args.objectAtIndex(11))
const iconPath = ObjC.unwrap(args.objectAtIndex(12))
const windowTitle = ObjC.unwrap(args.objectAtIndex(13))
const cancelValue = ObjC.unwrap(args.objectAtIndex(14))
const announcementTimeout = Number(ObjC.unwrap(args.objectAtIndex(15) || '0'))
const appTitle = ObjC.unwrap(args.objectAtIndex(16) || windowTitle)
//a message screen has no selectable rows; it may still show an image (e.g. the welcome screen)
const isMessageMode = choices.length === 0
const isPartnershipAnnouncement = imagePath.endsWith('/partnership.png')

function writeResult(value) {
  const data = $(value + '\n').dataUsingEncoding($.NSUTF8StringEncoding)
  $.NSFileHandle.fileHandleWithStandardOutput.writeData(data)
}

function cancelAndExit() {
  writeResult('__WOR_CANCEL__')
  $.exit(0)
}

$.NSProcessInfo.processInfo.processName = appTitle
const app = $.NSApplication.sharedApplication
app.setActivationPolicy($.NSApplicationActivationPolicyRegular)
worInstallAppMenu(app, appTitle, windowTitle, iconPath)
worSetAppIcon(app, iconPath)
let tableView
let window
let selectedValue = null
let allowTermination = false
let countdownSeconds = announcementTimeout
let countdownTimer = null
let nextButton
const Controller = ObjC.registerSubclass({
  name: 'WorChooserController',
  superclass: 'NSObject',
  methods: {
    'numberOfRowsInTableView:': {
      types: ['NSInteger', ['id']],
      implementation: function() {
        return choices.length
      }
    },
    'tableView:objectValueForTableColumn:row:': {
      types: ['id', ['id', 'id', 'NSInteger']],
      implementation: function(_tableView, _tableColumn, row) {
        return $(choices[row])
      }
    },
    'nextClicked:': {
      types: ['void', ['id']],
      implementation: function() {
        if (isMessageMode) {
          selectedValue = defaultChoice
          app.stopModalWithCode($.NSOKButton)
          window.orderOut(null)
          return
        }
        const row = tableView.selectedRow
        if (row >= 0 && row < choices.length) {
          selectedValue = choices[row]
          app.stopModalWithCode($.NSOKButton)
          window.orderOut(null)
        }
      }
    },
    'cancelClicked:': {
      types: ['void', ['id']],
      implementation: function() {
        selectedValue = cancelValue.length > 0 ? cancelValue : null
        app.stopModalWithCode($.NSCancelButton)
        window.orderOut(null)
      }
    },
    'countdownTick:': {
      types: ['void', ['id']],
      implementation: function() {
        if (countdownSeconds <= 0) return
        countdownSeconds -= 1
        nextButton.title = nextLabel + ' (' + countdownSeconds + ')'
        if (countdownSeconds === 0) {
          selectedValue = defaultChoice
          app.stopModalWithCode($.NSOKButton)
          window.orderOut(null)
        }
      }
    },
    'textView:clickedOnLink:atIndex:': {
      types: ['BOOL', ['id', 'id', 'NSUInteger']],
      implementation: function(_textView, link, _index) {
        const url = ObjC.unwrap(link)
        $.NSWorkspace.sharedWorkspace.openURL($.NSURL.URLWithString($(url)))
        return true
      }
    },
    'actionClicked:': {
      types: ['void', ['id']],
      implementation: function() {
        selectedValue = actionValue
        app.stopModalWithCode($.NSOKButton)
        window.orderOut(null)
      }
    },
    'windowWillClose:': {
      types: ['void', ['id']],
      implementation: function() {
        selectedValue = null
        app.stopModalWithCode($.NSCancelButton)
      }
    },
    //right-click Quit from the Dock/app-switcher sends terminate: to NSApp; since this window is
    //hosted by osascript rather than a full NSApplicationMain run loop, that does not otherwise exit the process
    'handleQuitEvent:withReplyEvent:': {
      types: ['void', ['id', 'id']],
      implementation: function() {
        cancelAndExit()
      }
    },
    'pumpEvents:': {
      types: ['void', ['id']],
      implementation: function() {
        $.NSRunLoop.currentRunLoop.runModeBeforeDate($.NSDefaultRunLoopMode, $.NSDate.dateWithTimeIntervalSinceNow(0.01))
      }
    },
    //clicking the Dock icon sends aevt/rapp; without a handler a minimised window can never come back
    'handleReopenEvent:withReplyEvent:': {
      types: ['void', ['id', 'id']],
      implementation: function() {
        if (window.isMiniaturized) window.deminiaturize(null)
        window.makeKeyAndOrderFront(null)
        app.activateIgnoringOtherApps(true)
      }
    },
    'applicationShouldTerminate:': {
      types: ['NSUInteger', ['id']],
      implementation: function() {
        if (allowTermination) return $.NSTerminateNow
        cancelAndExit()
      }
    }
  }
})

const controller = $.WorChooserController.alloc.init
app.setDelegate(controller)
const screenFrame = $.NSScreen.mainScreen.visibleFrame
//clamp the desired size to whatever screen real estate is actually available, rather than assuming a full-size display
const maxWidth = Math.min(760, screenFrame.size.width - 40)
//a message screen only hides Cancel for the image-based welcome screen; an explicitly empty cancelLabel hides it everywhere else
const showCancelButton = cancelLabel.length > 0 && !(isMessageMode && imagePath.length > 0)
//the bottom button row is the hard floor on width; fitting the window to its content must never clip it
const minButtonRowWidth = 40 + 180 + (showCancelButton ? 188 : 0) + (actionLabel.length > 0 ? 120 : 0)

function measuredTextWidth(text, font) {
  const measured = $.NSMutableAttributedString.alloc.init
  measured.mutableString.appendString($(text))
  measured.addAttributeValueRange($.NSFontAttributeName, font, $.NSMakeRange(0, text.length))
  const bounds = measured.boundingRectWithSizeOptions($.NSMakeSize(2000, 200), $.NSStringDrawingUsesLineFragmentOrigin | $.NSStringDrawingUsesFontLeading)
  return Math.ceil(bounds.size.width)
}

const measureText = $.NSMutableAttributedString.alloc.init
measureText.mutableString.appendString($(promptText))
const measureRange = $.NSMakeRange(0, promptText.length)
const promptFont = $.NSFont.systemFontOfSizeWeight(14, $.NSFontWeightMedium)
measureText.addAttributeValueRange($.NSFontAttributeName, promptFont, measureRange)
const naturalBounds = measureText.boundingRectWithSizeOptions($.NSMakeSize(1000, 1000), $.NSStringDrawingUsesLineFragmentOrigin | $.NSStringDrawingUsesFontLeading)

const listRowHeight = 24
//cap the visible list so a two-item chooser stays compact and a 190-entry language list still scrolls instead of filling the screen
const visibleListHeight = isMessageMode ? 0 : Math.min(384, Math.max(96, choices.length * listRowHeight + 8))
let requestedWidth
if (isPartnershipAnnouncement || (isMessageMode && imagePath.length > 0)) {
  requestedWidth = maxWidth
} else if (isMessageMode) {
  requestedWidth = Math.max(360, Math.ceil(naturalBounds.size.width) + 40)
} else {
  //fit the widest row plus its inset and the vertical scroller, so rows read fully without a needlessly wide window
  const rowFont = $.NSFont.systemFontOfSize(13)
  let widestChoice = 0
  for (let i = 0; i < choices.length; i++) widestChoice = Math.max(widestChoice, measuredTextWidth(choices[i], rowFont))
  requestedWidth = Math.max(Math.ceil(naturalBounds.size.width), widestChoice + 60) + 40
}
const width = Math.min(maxWidth, Math.max(minButtonRowWidth, requestedWidth))
const measuredBounds = measureText.boundingRectWithSizeOptions($.NSMakeSize(width - 40, 1000), $.NSStringDrawingUsesLineFragmentOrigin | $.NSStringDrawingUsesFontLeading)
const contentHeight = Math.ceil(measuredBounds.size.height) + 150
const requestedHeight = isPartnershipAnnouncement
  ? 600
  : isMessageMode
    ? (imagePath.length > 0 ? 500 : Math.max(220, contentHeight))
    : visibleListHeight + 132
const height = Math.min(requestedHeight, screenFrame.size.height - 60)
//fixed layout: no drag-resize and no zoom/maximize button, only minimize (and restore) via the titlebar
window = worMakeWindow({ width: width, height: height, title: windowTitle, delegate: controller })

const content = window.contentView
content.autoresizingMask = $.NSViewWidthSizable | $.NSViewHeightSizable

const prompt = $.NSTextField.labelWithString(promptText)
prompt.font = $.NSFont.systemFontOfSizeWeight(14, $.NSFontWeightMedium)
prompt.autoresizingMask = $.NSViewWidthSizable | $.NSViewMinYMargin
if (isMessageMode) {
  prompt.setUsesSingleLineMode(false)
  prompt.cell.setWraps(true)
  prompt.cell.setScrollable(false)
  if (imagePath.length > 0) {
    prompt.frame = $.NSMakeRect(20, 76, width - 40, 122)
    const imageHeight = isPartnershipAnnouncement ? Math.min(315, height - 185) : 320
    const imageFrame = isPartnershipAnnouncement
      ? $.NSMakeRect(20, height - imageHeight - 12, width - 40, imageHeight)
      : $.NSMakeRect(20, 140, width - 40, imageHeight)
    const imageView = $.NSImageView.alloc.initWithFrame(imageFrame)
    imageView.image = $.NSImage.alloc.initWithContentsOfFile($(imagePath))
    imageView.imageScaling = $.NSImageScaleProportionallyUpOrDown
    imageView.autoresizingMask = $.NSViewWidthSizable | $.NSViewMinYMargin
    content.addSubview(imageView)
  } else if (isPartnershipAnnouncement) {
    prompt.frame = $.NSMakeRect(20, 96, width - 40, height - 132)
  } else {
    prompt.frame = $.NSMakeRect(20, 72, width - 40, height - 112)
  }
  if (isPartnershipAnnouncement) {
    const attributedPrompt = $.NSMutableAttributedString.alloc.init
    attributedPrompt.mutableString.appendString($(promptText))
    const fullPromptRange = $.NSMakeRange(0, promptText.length)
    const paragraphStyle = $.NSMutableParagraphStyle.alloc.init
    paragraphStyle.alignment = $.NSTextAlignmentCenter
    paragraphStyle.lineBreakMode = $.NSLineBreakByWordWrapping
    paragraphStyle.lineSpacing = 1
    paragraphStyle.paragraphSpacing = 4
    attributedPrompt.addAttributeValueRange($.NSForegroundColorAttributeName, $.NSColor.labelColor, fullPromptRange)
    attributedPrompt.addAttributeValueRange($.NSFontAttributeName, $.NSFont.systemFontOfSizeWeight(14, $.NSFontWeightMedium), fullPromptRange)
    attributedPrompt.addAttributeValueRange($.NSParagraphStyleAttributeName, paragraphStyle, fullPromptRange)
    function addPromptLink(label, url) {
      const index = promptText.indexOf(label)
      if (index < 0) return
      const range = $.NSMakeRange(index, label.length)
      attributedPrompt.addAttributeValueRange($.NSLinkAttributeName, $(url), range)
      attributedPrompt.addAttributeValueRange($.NSForegroundColorAttributeName, $.NSColor.linkColor, range)
      attributedPrompt.addAttributeValueRange($.NSUnderlineStyleAttributeName, $(1), range)
    }
    addPromptLink('Blackout Secure', 'https://blackoutsecure.app/')
    addPromptLink('Botspot', 'https://github.com/Botspot')
    addPromptLink('Windows on R', 'https://worproject.com/')
    addPromptLink('Botspot/wor-flasher', 'https://github.com/Botspot/wor-flasher')
    addPromptLink('sponsoring Botspot', 'https://github.com/sponsors/Botspot')
    addPromptLink('buying Blackout Secure a coffee', 'https://github.com/sponsors/blackoutsecure?frequency=one-time&amount=8')
    const textFrame = isPartnershipAnnouncement
      ? (() => {
          const textWidth = 680
          const textBounds = attributedPrompt.boundingRectWithSizeOptions($.NSMakeSize(textWidth, 1000), $.NSStringDrawingUsesLineFragmentOrigin | $.NSStringDrawingUsesFontLeading)
          const textHeight = Math.ceil(textBounds.size.height) + 12
          return $.NSMakeRect((width - textWidth) / 2, 58, textWidth, textHeight)
        })()
      : imagePath.length > 0
        ? $.NSMakeRect(32, 76, width - 64, 122)
        : $.NSMakeRect(20, 96, width - 40, height - 132)
    const textView = $.NSTextView.alloc.initWithFrame(textFrame)
    textView.editable = false
    textView.selectable = false
    textView.drawsBackground = false
    textView.alignment = $.NSTextAlignmentCenter
    textView.textContainerInset = $.NSMakeSize(0, 0)
    textView.textContainer.lineFragmentPadding = 0
    textView.textContainer.widthTracksTextView = true
    textView.horizontallyResizable = false
    textView.verticallyResizable = false
    textView.autoresizingMask = isPartnershipAnnouncement
      ? $.NSViewMinXMargin | $.NSViewMaxXMargin | $.NSViewMinYMargin
      : $.NSViewWidthSizable | $.NSViewMinYMargin
    textView.textStorage.setAttributedString(attributedPrompt)
    textView.delegate = controller
    content.addSubview(textView)
  } else {
    content.addSubview(prompt)
  }
} else {
  prompt.frame = $.NSMakeRect(20, height - 52, width - 40, 24)
  content.addSubview(prompt)
}

if (!isMessageMode) {
  const scrollView = $.NSScrollView.alloc.initWithFrame($.NSMakeRect(20, 72, width - 40, height - 132))
  scrollView.autoresizingMask = $.NSViewWidthSizable | $.NSViewHeightSizable
  scrollView.borderType = $.NSBezelBorder
  scrollView.hasVerticalScroller = true

  tableView = $.NSTableView.alloc.initWithFrame(scrollView.bounds)
  tableView.setHeaderView(undefined)
  tableView.rowHeight = 24
  tableView.setDelegate(controller)
  tableView.setDataSource(controller)
  tableView.setTarget(controller)
  tableView.setDoubleAction('nextClicked:')
  tableView.setAllowsEmptySelection(false)
  tableView.setUsesAlternatingRowBackgroundColors(true)

  const column = $.NSTableColumn.alloc.initWithIdentifier('choice')
  column.width = scrollView.contentSize.width
  column.resizingMask = $.NSTableColumnAutoresizingMask
  column.editable = false
  tableView.addTableColumn(column)
  scrollView.documentView = tableView
  content.addSubview(scrollView)
}

//a lone button in a message dialog reads better centered; once a Back/Advanced companion button is
//present, centering the primary button made the two overlap, so both use the standard bottom-right row instead
const centerNextButton = isMessageMode && !showCancelButton && actionLabel.length === 0

//buttons are sized to their own text so any label (e.g. "Proceed with WoR-Flasher") fits without truncation
nextButton = $.NSButton.buttonWithTitleTargetAction(nextLabel, controller, 'nextClicked:')
nextButton.setTarget(controller)
nextButton.setAction('nextClicked:')
nextButton.bezelStyle = $.NSBezelStyleRounded
nextButton.keyEquivalent = '\r'
if (announcementTimeout > 0 && isMessageMode) nextButton.title = nextLabel + ' (' + announcementTimeout + ')'
nextButton.sizeToFit
const nextWidth = Math.max(180, nextButton.frame.size.width)
const nextX = centerNextButton ? (width - nextWidth) / 2 : width - 20 - nextWidth
nextButton.frame = $.NSMakeRect(nextX, 22, nextWidth, 32)
nextButton.autoresizingMask = $.NSViewMinXMargin | $.NSViewMaxXMargin | $.NSViewMaxYMargin
content.addSubview(nextButton)

if (announcementTimeout > 0 && isMessageMode) {
  countdownTimer = $.NSTimer.timerWithTimeIntervalTargetSelectorUserInfoRepeats(1, controller, 'countdownTick:', $(), true)
  $.NSRunLoop.currentRunLoop.addTimerForMode(countdownTimer, $.NSModalPanelRunLoopMode)
}

if (showCancelButton) {
  const cancelButton = $.NSButton.buttonWithTitleTargetAction(cancelLabel, controller, 'cancelClicked:')
  cancelButton.bezelStyle = $.NSBezelStyleRounded
  cancelButton.keyEquivalent = '\u001b'
  cancelButton.sizeToFit
  //match the next button width so the pair reads as one row instead of two different sizes
  const cancelWidth = Math.max(nextWidth, cancelButton.frame.size.width)
  //anchor to the next button real x position instead of re-deriving one, so the two never overlap
  cancelButton.frame = $.NSMakeRect(nextButton.frame.origin.x - 8 - cancelWidth, 22, cancelWidth, 32)
  cancelButton.autoresizingMask = $.NSViewMinXMargin | $.NSViewMaxYMargin
  content.addSubview(cancelButton)
}

if (actionLabel.length > 0) {
  const actionButton = $.NSButton.buttonWithTitleTargetAction(actionLabel, controller, 'actionClicked:')
  actionButton.bezelStyle = $.NSBezelStyleRounded
  actionButton.sizeToFit
  const actionWidth = Math.max(112, actionButton.frame.size.width)
  actionButton.frame = $.NSMakeRect(20, 22, actionWidth, 32)
  actionButton.autoresizingMask = $.NSViewMaxXMargin | $.NSViewMaxYMargin
  content.addSubview(actionButton)
}

if (!isMessageMode) {
  const defaultIndex = Math.max(0, choices.indexOf(defaultChoice))
  tableView.selectRowIndexesByExtendingSelection($.NSIndexSet.indexSetWithIndex(defaultIndex), false)
  tableView.scrollRowToVisible(defaultIndex)
}

window.makeKeyAndOrderFront(null)
if (!app.isActive) app.requestUserAttention($.NSInformationalRequest)
app.activateIgnoringOtherApps(true)
worInstallWindowHandlers(controller)
app.runModalForWindow(window)

if (selectedValue === null) {
  writeResult('__WOR_CANCEL__')
} else {
  writeResult(selectedValue)
}
allowTermination = true
app.terminate(null)
JXA
)"
  result="$(wor_osascript -l JavaScript - "$1" "$2" "$3" "${4:-Cancel}" "${5:-}" "${6:-}" "${7:-}" "${8:-Next}" "${9:-$WOR_ICON_PATH}" "${10:-$WOR_WINDOW_TITLE}" "${11:-}" "${12:-0}" "${13:-$WOR_APP_TITLE}" <<<"$choose_jxa")"
  if [ "$result" == __WOR_CANCEL__ ];then
    return 1
  fi
  printf '%s\n' "$result"
}

macos_show_announcement() { #Output: proceed or project partner.
  local announcement_choices announcement_choice announcement_text
  announcement_choices=''
  printf -v announcement_text '%s\n\n%s\n\n%s\n' \
    "Blackout Secure is proud to partner with Botspot and the Windows on R community, carrying WoR-Flasher forward while preserving Botspot's original authorship and project direction." \
    "Report issues, share feedback, or contribute at Botspot/wor-flasher." \
    "Support continued development by sponsoring Botspot or buying Blackout Secure a coffee on GitHub."
  announcement_choice="$(macos_choose "$announcement_choices" "$announcement_text" 'Proceed with WoR-Flasher' Cancel '' '' "$WOR_ASSETS_DIR/partnership.png" 'Proceed with WoR-Flasher' "$WOR_ICON_PATH" "$WOR_WINDOW_TITLE" '' "$WOR_ANNOUNCEMENT_TIMEOUT" "$WOR_APP_TITLE")" || return 1

  case "$announcement_choice" in
    'Proceed with WoR-Flasher') echo "$announcement_choice" ;;
    *) return 1 ;;
  esac
}

macos_choose_device() { #Input: newline-separated detected volume rows. Output: selected row, __REFRESH__, or Back. Fails only on a genuine Quit/close, never on Back.
  if [ -z "$1" ];then
    #cancelValue Back makes clicking Back succeed with a literal value instead of failing like Quit does,
    #so the wizard can tell "go back a step" apart from "the user is quitting the whole thing"
    result="$(macos_choose '' 'No external, physical, writable drive was found. Connect a removable drive, then click Refresh.' __REFRESH__ Back '' '' '' Refresh "$WOR_ICON_PATH" "$WOR_WINDOW_TITLE" Back 0 "$WOR_APP_TITLE")" || return 1
    echo "$result"
  else
    device_choice="$(macos_choose "$1" 'Choose the external drive and volumes to erase' "$(printf '%s\n' "$1" | head -n1)" Back Refresh __REFRESH__ '' '' '' '' Back)" || return 1
    echo "$device_choice"
  fi
}

macos_confirm_flash() { #Output: Flash, Advanced, Back, or failure for Quit/close.
  local result rows
  rows="$(settings_summary)"
  local confirm_jxa
  confirm_jxa="$(wor_jxa_window_lib; cat <<'JXA'
ObjC.import('AppKit')
ObjC.import('Foundation')
ObjC.import('stdlib')

const args = $.NSProcessInfo.processInfo.arguments
const rawRows = ObjC.unwrap(args.objectAtIndex(4))
const targetDevice = ObjC.unwrap(args.objectAtIndex(5))
const iconPath = ObjC.unwrap(args.objectAtIndex(6))
const windowTitle = ObjC.unwrap(args.objectAtIndex(7))
const appTitle = ObjC.unwrap(args.objectAtIndex(8))
const rows = rawRows.split('\n')
  .filter((line) => line.length > 0)
  .map((line) => {
    const parts = line.split('\t')
    return { label: parts[0] || '', value: parts.slice(1).join('\t') || '' }
  })

function writeResult(value) {
  const data = $(value + '\n').dataUsingEncoding($.NSUTF8StringEncoding)
  $.NSFileHandle.fileHandleWithStandardOutput.writeData(data)
}

$.NSProcessInfo.processInfo.processName = appTitle
const app = $.NSApplication.sharedApplication
app.setActivationPolicy($.NSApplicationActivationPolicyRegular)
worInstallAppMenu(app, appTitle, windowTitle, iconPath)
worSetAppIcon(app, iconPath)

let window
let selectedValue = 'Cancel'
let allowTermination = false
const Controller = ObjC.registerSubclass({
  name: 'WorConfirmFlashController',
  superclass: 'NSObject',
  methods: {
    'flashClicked:': {
      types: ['void', ['id']],
      implementation: function() {
        selectedValue = 'Flash'
        app.stopModalWithCode($.NSOKButton)
        window.orderOut(null)
      }
    },
    'advancedClicked:': {
      types: ['void', ['id']],
      implementation: function() {
        selectedValue = 'Advanced'
        app.stopModalWithCode($.NSOKButton)
        window.orderOut(null)
      }
    },
    'backClicked:': {
      types: ['void', ['id']],
      implementation: function() {
        selectedValue = 'Back'
        app.stopModalWithCode($.NSCancelButton)
        window.orderOut(null)
      }
    },
    'windowWillClose:': {
      types: ['void', ['id']],
      implementation: function() {
        selectedValue = 'Cancel'
        app.stopModalWithCode($.NSCancelButton)
      }
    },
    'handleQuitEvent:withReplyEvent:': {
      types: ['void', ['id', 'id']],
      implementation: function() {
        selectedValue = 'Cancel'
        app.stopModalWithCode($.NSCancelButton)
        window.orderOut(null)
      }
    },
    'pumpEvents:': {
      types: ['void', ['id']],
      implementation: function() {
        $.NSRunLoop.currentRunLoop.runModeBeforeDate($.NSDefaultRunLoopMode, $.NSDate.dateWithTimeIntervalSinceNow(0.01))
      }
    },
    //clicking the Dock icon sends aevt/rapp; without a handler a minimised window can never come back
    'handleReopenEvent:withReplyEvent:': {
      types: ['void', ['id', 'id']],
      implementation: function() {
        if (window.isMiniaturized) window.deminiaturize(null)
        window.makeKeyAndOrderFront(null)
        app.activateIgnoringOtherApps(true)
      }
    },
    'applicationShouldTerminate:': {
      types: ['NSUInteger', ['id']],
      implementation: function() {
        if (allowTermination) return $.NSTerminateNow
        selectedValue = 'Cancel'
        app.stopModalWithCode($.NSCancelButton)
        return 0
      }
    }
  }
})
const controller = $.WorConfirmFlashController.alloc.init
app.setDelegate(controller)

const screenFrame = $.NSScreen.mainScreen.visibleFrame
const rowHeight = 24
//the settings list is the only variable-height part, so fit the window to it instead of always opening at full size
const valueFont = $.NSFont.systemFontOfSizeWeight(12, $.NSFontWeightRegular)
let widestValue = 0
for (let i = 0; i < rows.length; i++) {
  const measured = $.NSMutableAttributedString.alloc.init
  measured.mutableString.appendString($(rows[i].value))
  measured.addAttributeValueRange($.NSFontAttributeName, valueFont, $.NSMakeRange(0, rows[i].value.length))
  const bounds = measured.boundingRectWithSizeOptions($.NSMakeSize(2000, 100), $.NSStringDrawingUsesLineFragmentOrigin | $.NSStringDrawingUsesFontLeading)
  widestValue = Math.max(widestValue, Math.ceil(bounds.size.width))
}
const width = Math.min(760, screenFrame.size.width - 40, Math.max(560, 214 + widestValue + 72))
const height = Math.min(620, screenFrame.size.height - 60, Math.max(320, rows.length * rowHeight + 16 + 220))
window = worMakeWindow({ width: width, height: height, title: windowTitle, delegate: controller })

const content = window.contentView

const heading = $.NSTextField.labelWithString('Review flash settings')
heading.font = $.NSFont.systemFontOfSizeWeight(17, $.NSFontWeightBold)
heading.frame = $.NSMakeRect(24, height - 54, width - 48, 26)
content.addSubview(heading)

const target = $.NSTextField.labelWithString('Target: ' + targetDevice)
target.font = $.NSFont.systemFontOfSizeWeight(12, $.NSFontWeightMedium)
target.textColor = $.NSColor.secondaryLabelColor
target.frame = $.NSMakeRect(24, height - 80, width - 48, 20)
content.addSubview(target)

const warning = $.NSTextField.labelWithString('All data on the target drive will be erased.')
warning.font = $.NSFont.systemFontOfSizeWeight(13, $.NSFontWeightSemibold)
warning.textColor = $.NSColor.systemRedColor
warning.frame = $.NSMakeRect(24, height - 114, width - 48, 22)
content.addSubview(warning)

const documentHeight = Math.max(rows.length * rowHeight + 16, height - 210)
const scrollView = $.NSScrollView.alloc.initWithFrame($.NSMakeRect(24, 86, width - 48, height - 220))
scrollView.borderType = $.NSBezelBorder
scrollView.hasVerticalScroller = true
scrollView.autoresizingMask = $.NSViewWidthSizable | $.NSViewHeightSizable
const documentView = $.NSView.alloc.initWithFrame($.NSMakeRect(0, 0, width - 48, documentHeight))
scrollView.documentView = documentView
content.addSubview(scrollView)

let y = documentHeight - rowHeight - 8
for (let i = 0; i < rows.length; i++) {
  const label = $.NSTextField.labelWithString(rows[i].label)
  label.font = $.NSFont.systemFontOfSizeWeight(12, $.NSFontWeightMedium)
  label.textColor = $.NSColor.secondaryLabelColor
  label.alignment = $.NSTextAlignmentRight
  label.frame = $.NSMakeRect(8, y, 190, 20)
  documentView.addSubview(label)

  const value = $.NSTextField.labelWithString(rows[i].value)
  value.font = $.NSFont.systemFontOfSizeWeight(12, $.NSFontWeightRegular)
  value.lineBreakMode = $.NSLineBreakByTruncatingMiddle
  value.frame = $.NSMakeRect(214, y, width - 286, 20)
  documentView.addSubview(value)
  y -= rowHeight
}

const guidance = $.NSTextField.labelWithString('Flash begins immediately after administrator approval. Use Advanced to change these settings.')
guidance.font = $.NSFont.systemFontOfSizeWeight(11, $.NSFontWeightRegular)
guidance.textColor = $.NSColor.secondaryLabelColor
guidance.frame = $.NSMakeRect(24, 58, width - 48, 18)
content.addSubview(guidance)

const flashButton = $.NSButton.buttonWithTitleTargetAction('Flash', controller, 'flashClicked:')
flashButton.bezelStyle = $.NSBezelStyleRounded
flashButton.keyEquivalent = '\r'
flashButton.sizeToFit
const flashWidth = Math.max(132, flashButton.frame.size.width)
flashButton.frame = $.NSMakeRect(width - 24 - flashWidth, 20, flashWidth, 32)
content.addSubview(flashButton)

const backButton = $.NSButton.buttonWithTitleTargetAction('Back', controller, 'backClicked:')
backButton.bezelStyle = $.NSBezelStyleRounded
backButton.keyEquivalent = '\u001b'
backButton.sizeToFit
const backWidth = Math.max(112, backButton.frame.size.width)
backButton.frame = $.NSMakeRect(width - 24 - flashWidth - 8 - backWidth, 20, backWidth, 32)
content.addSubview(backButton)

const advancedButton = $.NSButton.buttonWithTitleTargetAction('Advanced...', controller, 'advancedClicked:')
advancedButton.bezelStyle = $.NSBezelStyleRounded
advancedButton.sizeToFit
const advancedWidth = Math.max(130, advancedButton.frame.size.width)
advancedButton.frame = $.NSMakeRect(24, 20, advancedWidth, 32)
content.addSubview(advancedButton)

window.makeKeyAndOrderFront(null)
if (!app.isActive) app.requestUserAttention($.NSInformationalRequest)
app.activateIgnoringOtherApps(true)
worInstallWindowHandlers(controller)
app.runModalForWindow(window)
allowTermination = true
writeResult(selectedValue)
app.terminate(null)
JXA
)"
  result="$(wor_osascript -l JavaScript - "$rows" "$DEVICE" "$WOR_ICON_PATH" "$WOR_WINDOW_TITLE" "$WOR_APP_TITLE" <<<"$confirm_jxa")" || return 1
  [ "$result" == Cancel ] && return 1
  printf '%s\n' "$result"
}

macos_advanced_options() { #Reads/updates OOBE_NETWORK_BYPASS, PI4_AUTO_DISABLE_3GB, UEFI_USE_LATEST, DRIVERS_USE_LATEST, SKIP_IMAGE_VERIFICATION, DRY_RUN, APPLY_CUSTOM_CONFIG_TXT, USE_CACHE, CONFIG_TXT, WIN_LANG, PLAY_SOUND, COMPLETION_SOUND.
  local advanced_jxa checkbox_spec result status line i uefi_pinned pi4_applicable pi4_label config_scope lang_spec locale_spec l_code l_name sel_win_lang sound_spec sel_sound LC_ALL
  uefi_pinned="$(uefi_pinned_version)"
  #the engine ignores PI4_AUTO_DISABLE_3GB unless RPI_MODEL is 4, so don't offer it as a live choice elsewhere
  [ "$RPI_MODEL" == 4 ] && pi4_applicable=1 || pi4_applicable=0
  #in recovery mode this config.txt boots the installer media; WoR-PE writes the target drive's own copy
  config_scope="$(wor_config_scope "$CAN_INSTALL_ON_SAME_DRIVE")"
  #labels and caution flags come from gui.sh so the Linux dialog cannot describe these differently
  checkbox_spec="$(wor_advanced_label oobe "$uefi_pinned" "$DRIVER_VER" "$RPI_MODEL")	$OOBE_NETWORK_BYPASS	1	$(wor_advanced_caution oobe)
$(wor_advanced_label pi4 "$uefi_pinned" "$DRIVER_VER" "$RPI_MODEL")	$([ "$pi4_applicable" == 1 ] && echo "$PI4_AUTO_DISABLE_3GB" || echo 0)	$pi4_applicable	$(wor_advanced_caution pi4)
$(wor_advanced_label uefi "$uefi_pinned" "$DRIVER_VER" "$RPI_MODEL")	$UEFI_USE_LATEST	1	$(wor_advanced_caution uefi)
$(wor_advanced_label drivers "$uefi_pinned" "$DRIVER_VER" "$RPI_MODEL")	$DRIVERS_USE_LATEST	1	$(wor_advanced_caution drivers)
$(wor_advanced_label verify "$uefi_pinned" "$DRIVER_VER" "$RPI_MODEL")	$SKIP_IMAGE_VERIFICATION	1	$(wor_advanced_caution verify)
$(wor_advanced_label dryrun "$uefi_pinned" "$DRIVER_VER" "$RPI_MODEL")	$DRY_RUN	1	$(wor_advanced_caution dryrun)"

  lang_spec=""
  while IFS=: read -r l_code l_name ;do
    [ -z "$l_code" ] && continue
    lang_spec+="${l_code}	${l_name}"$'\n'
  done < <(list_langs_preferred)

  locale_spec="$(list_windows_locale_options)"
  sound_spec="$(wor_sound_options)"

  advanced_jxa="$(wor_jxa_window_lib; cat <<'JXA'
ObjC.import('AppKit')
ObjC.import('Foundation')
ObjC.import('stdlib')
const args = $.NSProcessInfo.processInfo.arguments
const checkboxSpec = ObjC.unwrap(args.objectAtIndex(4))
const configTxtDefault = ObjC.unwrap(args.objectAtIndex(5))
const applyConfigDefault = ObjC.unwrap(args.objectAtIndex(6))
const iconPath = ObjC.unwrap(args.objectAtIndex(7))
const appTitle = ObjC.unwrap(args.objectAtIndex(8))
const applyConfigLabel = ObjC.unwrap(args.objectAtIndex(9))
const cacheModeDefault = ObjC.unwrap(args.objectAtIndex(10))
const accountUsernameDefault = ObjC.unwrap(args.objectAtIndex(11))
const accountPasswordDefault = ObjC.unwrap(args.objectAtIndex(12))
const localeDefault = ObjC.unwrap(args.objectAtIndex(13))
const langSpec = ObjC.unwrap(args.objectAtIndex(14))
const currentLangCode = ObjC.unwrap(args.objectAtIndex(15))
const accountSetupDefault = ObjC.unwrap(args.objectAtIndex(16))
const localeSetupDefault = ObjC.unwrap(args.objectAtIndex(17))
const localeSpec = ObjC.unwrap(args.objectAtIndex(18))
const windowTitle = ObjC.unwrap(args.objectAtIndex(19))
const soundSpec = ObjC.unwrap(args.objectAtIndex(20) || '')
const playSoundDefault = ObjC.unwrap(args.objectAtIndex(21) || '1')
const soundDefault = ObjC.unwrap(args.objectAtIndex(22) || '')
const showNotificationDefault = ObjC.unwrap(args.objectAtIndex(23) || '1')

const soundOptions = []
let initialSoundIdx = 0
if (soundSpec && soundSpec.length > 0) {
  const soundLines = soundSpec.split('\n')
  for (let i = 0; i < soundLines.length; i++) {
    const line = soundLines[i].trim()
    if (!line) continue
    const parts = line.split('\t')
    soundOptions.push({ value: parts[0], label: parts[1] || parts[0] })
    if (parts[0] === soundDefault) initialSoundIdx = soundOptions.length - 1
  }
}

const rows = checkboxSpec.split('\n').map(function(line) {
  const parts = line.split('\t')
  return { label: parts[0], checked: parts[1] === '1', enabled: parts[2] !== '0', caution: parts[3] === '1' }
})

const langOptions = []
let initialLangIdx = 0
if (langSpec && langSpec.length > 0) {
  const langLines = langSpec.split('\n')
  for (let i = 0; i < langLines.length; i++) {
    const line = langLines[i].trim()
    if (!line) continue
    const parts = line.split('\t')
    const code = parts[0]
    const name = parts[1] || code
    const label = name + ' (' + code + ')'
    langOptions.push({ code: code, label: label })
    if (code === currentLangCode) {
      initialLangIdx = langOptions.length - 1
    }
  }
}

const localeOptions = []
let initialLocaleIdx = 0
if (localeSpec && localeSpec.length > 0) {
  const localeLines = localeSpec.split('\n')
  for (let i = 0; i < localeLines.length; i++) {
    const line = localeLines[i].trim()
    if (!line) continue
    const parts = line.split('\t')
    const locale = parts[0]
    const label = parts[1] || locale
    localeOptions.push({ locale: locale, label: label })
    if (locale.toLowerCase() === localeDefault.toLowerCase()) {
      initialLocaleIdx = localeOptions.length - 1
    }
  }
}

$.NSProcessInfo.processInfo.processName = appTitle
const app = $.NSApplication.sharedApplication
app.setActivationPolicy($.NSApplicationActivationPolicyRegular)
worSetAppIcon(app, iconPath)

//this screen adds an Edit menu on top of the shared one: without it Cmd+C/V/X/A have nothing to route to
const mainMenu = worInstallAppMenu(app, appTitle, windowTitle, iconPath)
const editMenuItem = $.NSMenuItem.alloc.init
mainMenu.addItem(editMenuItem)
const editMenu = $.NSMenu.alloc.initWithTitle('Edit')
editMenuItem.submenu = editMenu
editMenu.addItemWithTitleActionKeyEquivalent('Cut', 'cut:', 'x')
editMenu.addItemWithTitleActionKeyEquivalent('Copy', 'copy:', 'c')
editMenu.addItemWithTitleActionKeyEquivalent('Paste', 'paste:', 'v')
editMenu.addItemWithTitleActionKeyEquivalent('Select All', 'selectAll:', 'a')

let window, applyConfigCheckbox, editConfigButton, cachePopup, winLangPopup, accountCheckbox, accountUsernameField, accountPasswordField, localeCheckbox, localePopup, playSoundCheckbox, soundPopup, soundLabel, notificationCheckbox
let checkboxes = []
let confirmed = false
let configTxtValue = configTxtDefault

function updateConfigEditableState() {
  const enabled = applyConfigCheckbox.state == 1
  editConfigButton.enabled = enabled
  editConfigButton.alphaValue = enabled ? 1.0 : 0.5
}

function editConfigTxt() {
  const editorScroll = $.NSScrollView.alloc.initWithFrame($.NSMakeRect(0, 0, 560, 320))
  editorScroll.borderType = $.NSBezelBorder
  editorScroll.hasVerticalScroller = true
  const editor = $.NSTextView.alloc.initWithFrame(editorScroll.bounds)
  editor.font = $.NSFont.userFixedPitchFontOfSize(12)
  editor.string = $(configTxtValue)
  editor.autoresizingMask = $.NSViewWidthSizable | $.NSViewHeightSizable
  editorScroll.documentView = editor

  const dialog = $.NSAlert.alloc.init
  dialog.messageText = $('View / Edit config.txt')
  dialog.informativeText = $('These settings control Raspberry Pi firmware and boot behavior.')
  dialog.alertStyle = $.NSAlertStyleInformational
  const dialogIcon = $.NSImage.alloc.initWithContentsOfFile($(iconPath))
  if (!dialogIcon.isNil()) dialog.icon = dialogIcon
  dialog.accessoryView = editorScroll
  dialog.addButtonWithTitle('Save')
  dialog.addButtonWithTitle('Cancel')
  if (dialog.runModal == $.NSAlertFirstButtonReturn) {
    configTxtValue = ObjC.unwrap(editor.string)
  }
}

function updateAccountEditableState() {
  const enabled = accountCheckbox.state == 1
  accountUsernameField.enabled = enabled
  accountPasswordField.enabled = enabled
  accountUsernameField.alphaValue = enabled ? 1.0 : 0.5
  accountPasswordField.alphaValue = enabled ? 1.0 : 0.5
}

function updateLocaleEditableState() {
  const enabled = localeCheckbox.state == 1
  localePopup.enabled = enabled
  localePopup.alphaValue = enabled ? 1.0 : 0.5
}

function updateSoundEnabledState() {
  if (!soundPopup) return
  const enabled = playSoundCheckbox.state == 1
  soundPopup.enabled = enabled
  soundPopup.alphaValue = enabled ? 1.0 : 0.5
  soundLabel.alphaValue = enabled ? 1.0 : 0.5
}

function previewSelectedSound() {
  if (!soundPopup || playSoundCheckbox.state != 1) return
  const idx = soundPopup.indexOfSelectedItem
  if (idx < 0 || idx >= soundOptions.length) return
  //soundNamed hands back a truthy wrapper for a name it cannot find, so isNil is the only guard
  const preview = $.NSSound.soundNamed(soundOptions[idx].value)
  if (!preview.isNil()) preview.play
}

const Controller = ObjC.registerSubclass({
  name: 'WorAdvancedController',
  superclass: 'NSObject',
  methods: {
    'okClicked:': {
      types: ['void', ['id']],
      implementation: function() {
        confirmed = true
        app.stopModalWithCode($.NSOKButton)
        window.orderOut(null)
      }
    },
    'cancelClicked:': {
      types: ['void', ['id']],
      implementation: function() {
        confirmed = false
        app.stopModalWithCode($.NSCancelButton)
        window.orderOut(null)
      }
    },
    'applyConfigToggled:': {
      types: ['void', ['id']],
      implementation: function() {
        updateConfigEditableState()
      }
    },
    'editConfigClicked:': {
      types: ['void', ['id']],
      implementation: function() {
        editConfigTxt()
      }
    },
    'accountToggled:': {
      types: ['void', ['id']],
      implementation: function() {
        updateAccountEditableState()
      }
    },
    'localeToggled:': {
      types: ['void', ['id']],
      implementation: function() {
        updateLocaleEditableState()
      }
    },
    'playSoundToggled:': {
      types: ['void', ['id']],
      implementation: function() {
        updateSoundEnabledState()
        previewSelectedSound()
      }
    },
    'soundPicked:': {
      types: ['void', ['id']],
      implementation: function() {
        previewSelectedSound()
      }
    },
    'windowWillClose:': {
      types: ['void', ['id']],
      implementation: function() {
        confirmed = false
        app.stopModalWithCode($.NSCancelButton)
      }
    },
    //right-click Quit from the Dock/app-switcher sends terminate: to NSApp; since this window is
    //hosted by osascript rather than a full NSApplicationMain run loop, that does not otherwise exit the process
    'handleQuitEvent:withReplyEvent:': {
      types: ['void', ['id', 'id']],
      implementation: function() {
        $.exit(0)
      }
    },
    'pumpEvents:': {
      types: ['void', ['id']],
      implementation: function() {
        $.NSRunLoop.currentRunLoop.runModeBeforeDate($.NSDefaultRunLoopMode, $.NSDate.dateWithTimeIntervalSinceNow(0.01))
      }
    },
    //clicking the Dock icon sends aevt/rapp; without a handler a minimised window can never come back
    'handleReopenEvent:withReplyEvent:': {
      types: ['void', ['id', 'id']],
      implementation: function() {
        if (window.isMiniaturized) window.deminiaturize(null)
        window.makeKeyAndOrderFront(null)
        app.activateIgnoringOtherApps(true)
      }
    },
    'applicationShouldTerminate:': {
      types: ['NSUInteger', ['id']],
      implementation: function() {
        $.exit(0)
      }
    }
  }
})
const controller = $.WorAdvancedController.alloc.init
app.setDelegate(controller)

const screenFrame = $.NSScreen.mainScreen.visibleFrame
const rowHeight = 26
const sectionHeaderCount = soundOptions.length > 0 ? 8 : 7
const desiredWidth = 640
//height grows with the number of advanced-option rows, so clamp both dimensions to the visible screen instead of assuming they fit
const desiredHeight = (rows.length + 1) * rowHeight + 20 + 220 + 60 + 8 + sectionHeaderCount * 30
  + (soundOptions.length > 0 ? rowHeight * 3 + 8 : 0)
const width = Math.min(desiredWidth, screenFrame.size.width - 40)
const height = Math.min(desiredHeight, screenFrame.size.height - 60)
//fixed layout: no drag-resize and no zoom/maximize button, only minimize (and restore) via the titlebar
window = worMakeWindow({ width: width, height: height, title: windowTitle + ' | Advanced Options', delegate: controller })

const content = window.contentView
content.autoresizingMask = $.NSViewWidthSizable | $.NSViewHeightSizable

let y = height - 40
function addSectionHeader(title) {
  y -= 8
  const section = $.NSTextField.labelWithString(title)
  section.font = $.NSFont.systemFontOfSizeWeight(12, $.NSFontWeightSemibold)
  section.textColor = $.NSColor.secondaryLabelColor
  section.frame = $.NSMakeRect(20, y, width - 40, 18)
  section.autoresizingMask = $.NSViewWidthSizable | $.NSViewMinYMargin
  content.addSubview(section)
  y -= 24
}

for (let i = 0; i < rows.length; i++) {
  if (i === 0) addSectionHeader('Windows setup')
  if (i === 2) addSectionHeader('Firmware and drivers')
  if (i === 4) addSectionHeader('Validation')
  const checkbox = $.NSButton.checkboxWithTitleTargetAction(rows[i].label, undefined, undefined)
  if (rows[i].caution) worAnnotateCheckbox(checkbox, rows[i].label, 'Not recommended', $.NSColor.systemRedColor)
  checkbox.frame = $.NSMakeRect(20, y, width - 40, 20)
  checkbox.state = rows[i].checked ? 1 : 0
  checkbox.enabled = rows[i].enabled
  checkbox.autoresizingMask = $.NSViewWidthSizable | $.NSViewMinYMargin
  content.addSubview(checkbox)
  checkboxes.push(checkbox)
  y -= rowHeight
}

addSectionHeader('Downloads')
//USE_CACHE has three values, so it needs a menu rather than a checkbox
const cacheLabel = $.NSTextField.labelWithString('Downloaded files:')
cacheLabel.font = $.NSFont.systemFontOfSizeWeight(12, $.NSFontWeightMedium)
cacheLabel.frame = $.NSMakeRect(20, y - 2, 120, 20)
cacheLabel.autoresizingMask = $.NSViewMaxXMargin | $.NSViewMinYMargin
content.addSubview(cacheLabel)
cachePopup = $.NSPopUpButton.alloc.initWithFramePullsDown($.NSMakeRect(146, y - 6, width - 166, 26), false)
cachePopup.addItemWithTitle('Re-download everything, ignoring the cache')
cachePopup.addItemWithTitle('Reuse cached files when they still match (recommended)')
cachePopup.addItemWithTitle('Trust the cache without checking it')
cachePopup.selectItemAtIndex(cacheModeDefault === '0' ? 0 : (cacheModeDefault === '2' ? 2 : 1))
cachePopup.autoresizingMask = $.NSViewWidthSizable | $.NSViewMinYMargin
content.addSubview(cachePopup)
y -= rowHeight + 8

if (langOptions.length > 0) {
  const winLangLabel = $.NSTextField.labelWithString('Choose Windows language:')
  winLangLabel.font = $.NSFont.systemFontOfSizeWeight(12, $.NSFontWeightMedium)
  winLangLabel.frame = $.NSMakeRect(20, y - 2, 170, 20)
  winLangLabel.autoresizingMask = $.NSViewMaxXMargin | $.NSViewMinYMargin
  content.addSubview(winLangLabel)
  winLangPopup = $.NSPopUpButton.alloc.initWithFramePullsDown($.NSMakeRect(196, y - 6, width - 216, 26), false)
  for (let i = 0; i < langOptions.length; i++) {
    winLangPopup.addItemWithTitle($(langOptions[i].label))
  }
  winLangPopup.selectItemAtIndex(initialLangIdx)
  winLangPopup.autoresizingMask = $.NSViewWidthSizable | $.NSViewMinYMargin
  content.addSubview(winLangPopup)
  y -= rowHeight + 8
}

if (soundOptions.length > 0) {
  addSectionHeader('Notifications')
  playSoundCheckbox = $.NSButton.checkboxWithTitleTargetAction('Play a sound when the flash finishes', controller, 'playSoundToggled:')
  playSoundCheckbox.frame = $.NSMakeRect(20, y, width - 40, 20)
  playSoundCheckbox.state = playSoundDefault === '0' ? 0 : 1
  playSoundCheckbox.autoresizingMask = $.NSViewWidthSizable | $.NSViewMinYMargin
  content.addSubview(playSoundCheckbox)
  y -= rowHeight

  soundLabel = $.NSTextField.labelWithString('Completion sound:')
  soundLabel.font = $.NSFont.systemFontOfSizeWeight(12, $.NSFontWeightMedium)
  soundLabel.frame = $.NSMakeRect(20, y - 2, 130, 20)
  soundLabel.autoresizingMask = $.NSViewMaxXMargin | $.NSViewMinYMargin
  content.addSubview(soundLabel)
  soundPopup = $.NSPopUpButton.alloc.initWithFramePullsDown($.NSMakeRect(156, y - 6, width - 176, 26), false)
  for (let i = 0; i < soundOptions.length; i++) {
    soundPopup.addItemWithTitle($(soundOptions[i].label))
  }
  soundPopup.selectItemAtIndex(initialSoundIdx)
  soundPopup.autoresizingMask = $.NSViewWidthSizable | $.NSViewMinYMargin
  //hearing the choice is the whole point, so play it as it is picked
  soundPopup.target = controller
  soundPopup.action = 'soundPicked:'
  content.addSubview(soundPopup)
  updateSoundEnabledState()
  y -= rowHeight + 8

  notificationCheckbox = $.NSButton.checkboxWithTitleTargetAction('Show a notification when the flash finishes', undefined, undefined)
  notificationCheckbox.frame = $.NSMakeRect(20, y, width - 40, 20)
  notificationCheckbox.state = showNotificationDefault === '0' ? 0 : 1
  notificationCheckbox.autoresizingMask = $.NSViewWidthSizable | $.NSViewMinYMargin
  content.addSubview(notificationCheckbox)
  y -= rowHeight
}

addSectionHeader('Windows account')
accountCheckbox = $.NSButton.checkboxWithTitleTargetAction('Create a local Windows administrator account', controller, 'accountToggled:')
accountCheckbox.frame = $.NSMakeRect(20, y, width - 40, 20)
accountCheckbox.state = accountSetupDefault === '1' ? 1 : 0
accountCheckbox.autoresizingMask = $.NSViewWidthSizable | $.NSViewMinYMargin
content.addSubview(accountCheckbox)
y -= rowHeight

function addAdvancedField(label, value, secure) {
  const fieldLabel = $.NSTextField.labelWithString(label)
  fieldLabel.frame = $.NSMakeRect(20, y, 170, 20)
  fieldLabel.autoresizingMask = $.NSViewMaxXMargin | $.NSViewMinYMargin
  content.addSubview(fieldLabel)
  const field = secure ? $.NSSecureTextField.alloc.initWithFrame($.NSMakeRect(196, y - 2, width - 216, 24)) : $.NSTextField.alloc.initWithFrame($.NSMakeRect(196, y - 2, width - 216, 24))
  field.stringValue = $(value)
  field.autoresizingMask = $.NSViewWidthSizable | $.NSViewMinYMargin
  content.addSubview(field)
  y -= rowHeight
  return field
}
accountUsernameField = addAdvancedField('Windows username:', accountUsernameDefault, false)
accountPasswordField = addAdvancedField('Windows password:', accountPasswordDefault, true)
updateAccountEditableState()

addSectionHeader('Regional settings')
localeCheckbox = $.NSButton.checkboxWithTitleTargetAction('Configure Windows keyboard and regional settings', controller, 'localeToggled:')
localeCheckbox.frame = $.NSMakeRect(20, y, width - 40, 20)
localeCheckbox.state = localeSetupDefault === '1' ? 1 : 0
localeCheckbox.autoresizingMask = $.NSViewWidthSizable | $.NSViewMinYMargin
content.addSubview(localeCheckbox)
y -= rowHeight

const localeLabel = $.NSTextField.labelWithString('Windows locale:')
localeLabel.frame = $.NSMakeRect(20, y, 170, 20)
localeLabel.autoresizingMask = $.NSViewMaxXMargin | $.NSViewMinYMargin
content.addSubview(localeLabel)
localePopup = $.NSPopUpButton.alloc.initWithFramePullsDown($.NSMakeRect(196, y - 6, width - 216, 26), false)
for (let i = 0; i < localeOptions.length; i++) {
  localePopup.addItemWithTitle($(localeOptions[i].label))
}
if (localeOptions.length === 0) {
  localeOptions.push({ locale: localeDefault, label: localeDefault })
  localePopup.addItemWithTitle($(localeDefault))
}
localePopup.selectItemAtIndex(initialLocaleIdx)
localePopup.autoresizingMask = $.NSViewWidthSizable | $.NSViewMinYMargin
content.addSubview(localePopup)
y -= rowHeight
updateLocaleEditableState()

addSectionHeader('Raspberry Pi boot config')
applyConfigCheckbox = $.NSButton.checkboxWithTitleTargetAction(applyConfigLabel, controller, 'applyConfigToggled:')
worAnnotateCheckbox(applyConfigCheckbox, applyConfigLabel, 'Recommended', $.NSColor.systemGreenColor)
applyConfigCheckbox.frame = $.NSMakeRect(20, y, width - 200, 20)
applyConfigCheckbox.state = applyConfigDefault === '1' ? 1 : 0
applyConfigCheckbox.autoresizingMask = $.NSViewWidthSizable | $.NSViewMinYMargin
content.addSubview(applyConfigCheckbox)
editConfigButton = $.NSButton.buttonWithTitleTargetAction('View / Edit…', controller, 'editConfigClicked:')
editConfigButton.bezelStyle = $.NSBezelStyleRounded
editConfigButton.frame = $.NSMakeRect(width - 160, y - 6, 140, 28)
editConfigButton.autoresizingMask = $.NSViewMinXMargin | $.NSViewMinYMargin
content.addSubview(editConfigButton)
updateConfigEditableState()

const okButton = $.NSButton.buttonWithTitleTargetAction('OK', controller, 'okClicked:')
okButton.bezelStyle = $.NSBezelStyleRounded
okButton.keyEquivalent = '\r'
okButton.sizeToFit
const okWidth = Math.max(96, okButton.frame.size.width)
okButton.frame = $.NSMakeRect(width - 20 - okWidth, 20, okWidth, 32)
okButton.autoresizingMask = $.NSViewMinXMargin | $.NSViewMaxYMargin
content.addSubview(okButton)

const cancelButton = $.NSButton.buttonWithTitleTargetAction('Back', controller, 'cancelClicked:')
cancelButton.bezelStyle = $.NSBezelStyleRounded
cancelButton.keyEquivalent = '\u001b'
cancelButton.sizeToFit
const cancelWidth = Math.max(96, cancelButton.frame.size.width)
cancelButton.frame = $.NSMakeRect(width - 20 - okWidth - 8 - cancelWidth, 20, cancelWidth, 32)
cancelButton.autoresizingMask = $.NSViewMinXMargin | $.NSViewMaxYMargin
content.addSubview(cancelButton)

window.makeKeyAndOrderFront(null)
if (!app.isActive) app.requestUserAttention($.NSInformationalRequest)
app.activateIgnoringOtherApps(true)
worInstallWindowHandlers(controller)
app.runModalForWindow(window)

function writeResult(lines) {
  const data = $(lines.join('\n') + '\n').dataUsingEncoding($.NSUTF8StringEncoding)
  $.NSFileHandle.fileHandleWithStandardOutput.writeData(data)
}

if (!confirmed) {
  writeResult(['CANCEL'])
  app.terminate(null)
}

const out = ['OK']
for (let i = 0; i < checkboxes.length; i++) {
  out.push(checkboxes[i].state == 1 ? '1' : '0')
}
out.push(accountCheckbox.state == 1 ? '1' : '0')
out.push(localeCheckbox.state == 1 ? '1' : '0')
out.push(applyConfigCheckbox.state == 1 ? '1' : '0')
out.push(String(cachePopup.indexOfSelectedItem))
out.push(ObjC.unwrap(accountUsernameField.stringValue))
out.push(ObjC.unwrap(accountPasswordField.stringValue))
let selectedLocale = localeDefault
if (localePopup && localeOptions.length > 0) {
  const localeIdx = localePopup.indexOfSelectedItem
  if (localeIdx >= 0 && localeIdx < localeOptions.length) {
    selectedLocale = localeOptions[localeIdx].locale
  }
}
out.push(selectedLocale)
let selectedLang = currentLangCode
if (winLangPopup && langOptions.length > 0) {
  const selIdx = winLangPopup.indexOfSelectedItem
  if (selIdx >= 0 && selIdx < langOptions.length) {
    selectedLang = langOptions[selIdx].code
  }
}
out.push(selectedLang)
//appended after the existing fields so the positional reader above them keeps working
out.push(playSoundCheckbox && playSoundCheckbox.state == 1 ? '1' : (soundOptions.length > 0 ? '0' : playSoundDefault))
let selectedSound = soundDefault
if (soundPopup && soundOptions.length > 0) {
  const soundIdx = soundPopup.indexOfSelectedItem
  if (soundIdx >= 0 && soundIdx < soundOptions.length) {
    selectedSound = soundOptions[soundIdx].value
  }
}
out.push(selectedSound)
out.push(notificationCheckbox && soundOptions.length > 0 ? (notificationCheckbox.state == 1 ? '1' : '0') : showNotificationDefault)
out.push('---CONFIG_TXT---')
out.push(configTxtValue)
writeResult(out)
app.terminate(null)
JXA
)"

  result="$(wor_osascript -l JavaScript - "$checkbox_spec" "$CONFIG_TXT" "$APPLY_CUSTOM_CONFIG_TXT" "$WOR_ICON_PATH" "$WOR_APP_TITLE" "$(wor_config_txt_label "$config_scope")" "$USE_CACHE" "$WINDOWS_ACCOUNT_USERNAME" "$WINDOWS_ACCOUNT_PASSWORD" "$WINDOWS_LOCALE" "$lang_spec" "$WIN_LANG" "$WINDOWS_ACCOUNT_SETUP" "$WINDOWS_LOCALE_SETUP" "$locale_spec" "$WOR_WINDOW_TITLE" "$sound_spec" "$PLAY_SOUND" "$(wor_completion_sound)" "$SHOW_NOTIFICATION" <<<"$advanced_jxa" 2>/dev/null)"
  #config.txt and account fields are user-editable bytes; BSD sed rejects malformed UTF-8 under the
  #desktop locale, so parse the machine-readable result in byte mode after AppKit has finished.
  LC_ALL=C
  status="$(printf '%s\n' "$result" | sed -n '1p')"
  [ "$status" == OK ] || return 1

  i=0
  while IFS= read -r line;do
    i=$((i+1))
    case "$i" in
      1) OOBE_NETWORK_BYPASS="$line" ;;
      2) [ "$pi4_applicable" == 1 ] && PI4_AUTO_DISABLE_3GB="$line" ;;
      3) UEFI_USE_LATEST="$line" ;;
      4) DRIVERS_USE_LATEST="$line" ;;
      5) SKIP_IMAGE_VERIFICATION="$line" ;;
      6) DRY_RUN="$line" ;;
      7) WINDOWS_ACCOUNT_SETUP="$line" ;;
      8) WINDOWS_LOCALE_SETUP="$line" ;;
      9) APPLY_CUSTOM_CONFIG_TXT="$line" ;;
    esac
  done < <(printf '%s\n' "$result" | sed -n '2,10p')
  case "$(printf '%s\n' "$result" | sed -n '11p')" in
    0 | 1 | 2) USE_CACHE="$(printf '%s\n' "$result" | sed -n '11p')" ;;
  esac
  WINDOWS_ACCOUNT_USERNAME="$(printf '%s\n' "$result" | sed -n '12p')"
  WINDOWS_ACCOUNT_PASSWORD="$(printf '%s\n' "$result" | sed -n '13p')"
  WINDOWS_LOCALE="$(printf '%s\n' "$result" | sed -n '14p')"
  sel_win_lang="$(printf '%s\n' "$result" | sed -n '15p')"
  if is_known_win_lang "$sel_win_lang" ;then
    WIN_LANG="$sel_win_lang"
  fi
  case "$(printf '%s\n' "$result" | sed -n '16p')" in
    0 | 1) PLAY_SOUND="$(printf '%s\n' "$result" | sed -n '16p')" ;;
  esac
  sel_sound="$(printf '%s\n' "$result" | sed -n '17p')"
  [ -n "$sel_sound" ] && COMPLETION_SOUND="$sel_sound"
  case "$(printf '%s\n' "$result" | sed -n '18p')" in
    0 | 1) SHOW_NOTIFICATION="$(printf '%s\n' "$result" | sed -n '18p')" ;;
  esac

  CONFIG_TXT="$(printf '%s\n' "$result" | sed -n '/^---CONFIG_TXT---$/,$p' | tail -n +2)"
}

macos_start_cli() {
  local completion_jxa confirm_summary confirmation default_language device_choices device_capability device_choice done_marker abort_marker auth_marker error_marker install_mode installer_pid installer_status language_choices mode_choices output_log password_retry_choice password_retry_reason pi_choices progress_file progress_jxa resume_at_flash saved_log step windows_choices

  windows_choices=$'Windows 11\nWindows 10'
  pi_choices=$'5\n4\n3'
  step=windows
  while true; do
    case "$step" in
      windows)
        WINDOWS_VER="$(macos_choose "$windows_choices" 'Choose Windows version' 'Windows 11')" || exit 0
        step=pi
        ;;
      pi)
        #cancelValue Back makes clicking Back succeed with a literal value instead of failing like
        #Quit does, so a real Quit exits the wizard instead of just stepping back to the previous screen
        RPI_MODEL="$(macos_choose "$pi_choices" 'Choose Raspberry Pi model' '5' Back '' '' '' '' '' '' Back)" || exit 0
        if [ "$RPI_MODEL" == Back ];then
          step=windows
          continue
        fi
        list_bids 10 >/dev/null || error "Failed to retrieve available Windows versions."
        [ "$WINDOWS_VER" == 'Windows 11' ] && BID="$(get_bid 11)" || BID="$(get_bid 10)"
        [ -n "$BID" ] || error "No compatible Windows build is available for Raspberry Pi $RPI_MODEL."
        set_default_config_txt
        [ -z "$WIN_LANG" ] && WIN_LANG="$(default_win_lang)"
        step=device
        ;;
      language)
        [ -z "$WIN_LANG" ] && WIN_LANG="$(default_win_lang)"
        step=device
        ;;
      device)
        device_choices="$(darwin_list_device_choices)"
        device_choice="$(macos_choose_device "$device_choices")" || exit 0
        if [ "$device_choice" == Back ];then
          step=pi
          continue
        fi
        [ "$device_choice" == __REFRESH__ ] && continue
        DEVICE="${device_choice%%$'\t'*}"
        is_safe_target_device "$DEVICE" || error "Refusing to overwrite $DEVICE. Choose an external, physical, writable whole disk."
        device_capability="$(drive_capability "$DEVICE")"
        validate_install_mode "$device_capability"
        step=mode
        ;;
      mode)
        if [ "$device_capability" == recovery ];then
          CAN_INSTALL_ON_SAME_DRIVE=0
        else
          mode_choices=$'Install Windows onto this drive\nCreate a recovery drive'
          install_mode="$(macos_choose "$mode_choices" 'Choose installation mode' 'Install Windows onto this drive' Back '' '' '' '' '' '' Back)" || exit 0
          if [ "$install_mode" == Back ];then
            step=device
            continue
          fi
          [ "$install_mode" == 'Install Windows onto this drive' ] && CAN_INSTALL_ON_SAME_DRIVE=1 || CAN_INSTALL_ON_SAME_DRIVE=0
        fi
        step=confirm
        ;;
      confirm)
        confirmation="$(macos_confirm_flash)" || confirmation=Cancel
        [ "$confirmation" == Cancel ] && exit 0
        if [ "$confirmation" == Advanced ];then
          macos_advanced_options
          continue
        fi
        [ "$confirmation" == Flash ] && break
        step=mode
        ;;
    esac
  done

  completion_jxa="$(wor_jxa_window_lib; cat <<'JXA'
ObjC.import('AppKit')
ObjC.import('stdlib')
const args = $.NSProcessInfo.processInfo.arguments
const message = ObjC.unwrap(args.objectAtIndex(4))
const iconPath = ObjC.unwrap(args.objectAtIndex(5))
const appTitle = ObjC.unwrap(args.objectAtIndex(6))
const imagePath = ObjC.unwrap(args.objectAtIndex(7) || '')
const windowTitle = ObjC.unwrap(args.objectAtIndex(8) || appTitle)
const successSound = ObjC.unwrap(args.objectAtIndex(9) || '')

$.NSProcessInfo.processInfo.processName = appTitle
const app = $.NSApplication.sharedApplication
app.setActivationPolicy($.NSApplicationActivationPolicyRegular)
worInstallAppMenu(app, appTitle, windowTitle, iconPath)
worSetAppIcon(app, iconPath)

let window
const Controller = ObjC.registerSubclass({
  name: 'WorCompletionController',
  superclass: 'NSObject',
  methods: {
    'openLogClicked:': {
      types: ['void', ['id']],
      implementation: function() {
        if (logPath.length > 0) {
          $.NSWorkspace.sharedWorkspace.openFileWithApplication($(logPath), $())
        }
      }
    },
    'copyLogClicked:': {
      types: ['void', ['id']],
      implementation: function() {
        if (logPath.length > 0) {
          const pb = $.NSPasteboard.generalPasteboard
          pb.clearContents
          pb.setStringForType($(logPath), $.NSPasteboardTypeString)
        }
      }
    },
    'okClicked:': {
      types: ['void', ['id']],
      implementation: function() {
        app.stopModalWithCode($.NSOKButton)
        window.orderOut(null)
      }
    },
    'windowWillClose:': {
      types: ['void', ['id']],
      implementation: function() {
        app.stopModalWithCode($.NSOKButton)
      }
    },
    //right-click Quit from the Dock/app-switcher sends terminate: to NSApp; since this window is
    //hosted by osascript rather than a full NSApplicationMain run loop, that does not otherwise exit the process
    'handleQuitEvent:withReplyEvent:': {
      types: ['void', ['id', 'id']],
      implementation: function() {
        $.exit(0)
      }
    },
    'pumpEvents:': {
      types: ['void', ['id']],
      implementation: function() {
        $.NSRunLoop.currentRunLoop.runModeBeforeDate($.NSDefaultRunLoopMode, $.NSDate.dateWithTimeIntervalSinceNow(0.01))
      }
    },
    //clicking the Dock icon sends aevt/rapp; without a handler a minimised window can never come back
    'handleReopenEvent:withReplyEvent:': {
      types: ['void', ['id', 'id']],
      implementation: function() {
        if (window.isMiniaturized) window.deminiaturize(null)
        window.makeKeyAndOrderFront(null)
        app.activateIgnoringOtherApps(true)
      }
    },
    'applicationShouldTerminate:': {
      types: ['NSUInteger', ['id']],
      implementation: function() {
        $.exit(0)
      }
    }
  }
})
const controller = $.WorCompletionController.alloc.init
app.setDelegate(controller)

const contentMargin = 20
const buttonY = 22
const buttonHeight = 32
let width = 600
let height = 360
let bannerImage = null
let bannerWidth = 0
let bannerHeight = 0

if (imagePath.length > 0) {
  bannerImage = $.NSImage.alloc.initWithContentsOfFile($(imagePath))
  //NSImage.size is DPI-scaled, so a 96dpi PNG reports three quarters of its pixels; measure the bitmap.
  //Number() is required: JXA hands these back as strings, and '650' + 40 would concatenate into a bogus frame.
  const reps = bannerImage.representations
  const rep = Number(reps.count) > 0 ? reps.objectAtIndex(0) : null
  bannerWidth = rep ? Number(rep.pixelsWide) : Number(bannerImage.size.width)
  bannerHeight = rep ? Number(rep.pixelsHigh) : Number(bannerImage.size.height)
  const maxBannerWidth = 760
  if (bannerWidth > maxBannerWidth) {
    bannerHeight = Math.round(bannerHeight * (maxBannerWidth / bannerWidth))
    bannerWidth = maxBannerWidth
  }
  width = Math.max(600, bannerWidth + contentMargin * 2)
}

//measure at the final width so neither layout can clip the message
const measured = $.NSMutableAttributedString.alloc.init
measured.mutableString.appendString($(message))
measured.addAttributeValueRange($.NSFontAttributeName, $.NSFont.systemFontOfSizeWeight(14, $.NSFontWeightMedium), $.NSMakeRange(0, message.length))
const measuredBounds = measured.boundingRectWithSizeOptions($.NSMakeSize(width - contentMargin * 2, 2000), $.NSStringDrawingUsesLineFragmentOrigin | $.NSStringDrawingUsesFontLeading)
const textHeight = Math.ceil(Number(measuredBounds.size.height))

if (imagePath.length > 0) {
  height = contentMargin + bannerHeight + 24 + textHeight + 20 + buttonHeight + buttonY
} else {
  //100pt of chrome: the button row below the text and the gap under the title bar
  height = Math.min(560, Math.max(200, textHeight + 100))
}
//fixed layout: no drag-resize, no zoom/maximize, and no minimize - this is a terminal screen, so
//the close box is the only titlebar control and behaves the same as clicking OK (see windowWillClose:)
window = worMakeWindow({ width: width, height: height, title: windowTitle, delegate: controller, miniaturizable: false })

const content = window.contentView

const textY = imagePath.length > 0 ? buttonY + buttonHeight + 20 : 70
const label = $.NSTextField.labelWithString(message)
label.font = $.NSFont.systemFontOfSizeWeight(14, $.NSFontWeightMedium)
label.setUsesSingleLineMode(false)
label.cell.setWraps(true)
label.cell.setScrollable(false)
label.frame = $.NSMakeRect(contentMargin, textY, width - contentMargin * 2, imagePath.length > 0 ? textHeight : height - 100)
//1, not $.NSTextAlignmentCenter: that constant bridges as 2, which this runtime draws right-aligned
if (imagePath.length > 0) label.setAlignment(1)
content.addSubview(label)

if (imagePath.length > 0) {
  const imageView = $.NSImageView.alloc.initWithFrame($.NSMakeRect(Math.round((width - bannerWidth) / 2), textY + textHeight + 24, bannerWidth, bannerHeight))
  imageView.image = bannerImage
  imageView.imageScaling = $.NSImageScaleProportionallyUpOrDown
  content.addSubview(imageView)
}

//extract the log file path if present (after "Full log: ") and create clickable buttons
let logPath = ''
const logMatch = message.match(/Full log: (.+)$/)
if (logMatch) {
  logPath = logMatch[1].trim()
}

let okButton, openButton, copyButton
const buttonGap = 8
//"Complete" only on the success screen; a failure dialog reached through Open Log/Copy is not a completion
const primaryLabel = imagePath.length > 0 ? 'Complete' : 'OK'
if (logPath.length > 0) {
  openButton = $.NSButton.buttonWithTitleTargetAction('Open Log', controller, 'openLogClicked:')
  openButton.bezelStyle = $.NSBezelStyleRounded
  openButton.sizeToFit
  const openWidth = Math.max(96, openButton.frame.size.width)
  openButton.frame = $.NSMakeRect(20, buttonY, openWidth, buttonHeight)
  content.addSubview(openButton)

  copyButton = $.NSButton.buttonWithTitleTargetAction('Copy', controller, 'copyLogClicked:')
  copyButton.bezelStyle = $.NSBezelStyleRounded
  copyButton.sizeToFit
  const copyWidth = Math.max(80, copyButton.frame.size.width)
  copyButton.frame = $.NSMakeRect(20 + openWidth + buttonGap, buttonY, copyWidth, buttonHeight)
  content.addSubview(copyButton)

  okButton = $.NSButton.buttonWithTitleTargetAction(primaryLabel, controller, 'okClicked:')
  okButton.bezelStyle = $.NSBezelStyleRounded
  okButton.keyEquivalent = '\r'
  okButton.sizeToFit
  const okWidth = Math.max(96, okButton.frame.size.width)
  okButton.frame = $.NSMakeRect(width - 20 - okWidth, buttonY, okWidth, buttonHeight)
  content.addSubview(okButton)
} else {
  okButton = $.NSButton.buttonWithTitleTargetAction(primaryLabel, controller, 'okClicked:')
  okButton.bezelStyle = $.NSBezelStyleRounded
  okButton.keyEquivalent = '\r'
  okButton.sizeToFit
  const okWidth = Math.max(96, okButton.frame.size.width)
  //centred under the message when it is the only button, right-aligned otherwise
  const okX = imagePath.length > 0 ? Math.round((width - okWidth) / 2) : width - 20 - okWidth
  okButton.frame = $.NSMakeRect(okX, buttonY, okWidth, buttonHeight)
  content.addSubview(okButton)
}

//a flash runs long enough to walk away from, so announce the result; these ship with macOS, so there
//is no audio asset to bundle, and they follow the system alert volume and Do Not Disturb settings.
//soundNamed returns a truthy wrapper for a missing name, so isNil is the only guard that works.
//An empty successSound means the user turned the sound off; a failure keeps the standard error sound.
const soundName = imagePath.length > 0 ? successSound : 'Basso'
if (soundName.length > 0) {
  const completionSound = $.NSSound.soundNamed(soundName)
  if (!completionSound.isNil()) completionSound.play
}

window.makeKeyAndOrderFront(null)
if (!app.isActive) app.requestUserAttention($.NSInformationalRequest)
app.activateIgnoringOtherApps(true)
worInstallWindowHandlers(controller)
app.runModalForWindow(window)
app.terminate(null)
JXA
)"

  progress_jxa="$(wor_jxa_window_lib; cat <<'JXA'
ObjC.import('AppKit')
ObjC.import('Foundation')
ObjC.import('stdlib')
const args = $.NSProcessInfo.processInfo.arguments
const progressFile = ObjC.unwrap(args.objectAtIndex(4))
const doneMarker = ObjC.unwrap(args.objectAtIndex(5))
const iconPath = ObjC.unwrap(args.objectAtIndex(6))
const appTitle = ObjC.unwrap(args.objectAtIndex(7))
const abortMarker = ObjC.unwrap(args.objectAtIndex(8))
const windowTitle = ObjC.unwrap(args.objectAtIndex(9) || appTitle)

$.NSProcessInfo.processInfo.processName = appTitle
const app = $.NSApplication.sharedApplication
app.setActivationPolicy($.NSApplicationActivationPolicyRegular)
worInstallAppMenu(app, appTitle, windowTitle, iconPath)
worSetAppIcon(app, iconPath)

const fm = $.NSFileManager.defaultManager

function readFile(path) {
  if (!fm.fileExistsAtPath($(path))) return ''
  try {
    const data = $.NSString.stringWithContentsOfFileEncodingError($(path), $.NSUTF8StringEncoding, undefined)
    return ObjC.unwrap(data) || ''
  } catch (e) {
    return ''
  }
}

function lastMatch(lines, prefix) {
  for (let i = lines.length - 1; i >= 0; i--) {
    if (lines[i].indexOf(prefix) === 0) return lines[i]
  }
  return ''
}

let window, bar, taskBar, phaseLabel, detailLabel, stepLabel, stepPercentLabel, taskPercentLabel

//stopping part-way leaves an unbootable drive, so make the user confirm and record why we stopped
function confirmAbort() {
  const alert = $.NSAlert.alloc.init
  alert.messageText = 'Stop flashing this drive?'
  alert.informativeText = 'The drive will be left unusable and has to be flashed again before it can boot.'
  alert.alertStyle = $.NSAlertStyleCritical
  alert.addButtonWithTitle('Stop flashing')
  alert.addButtonWithTitle('Keep going')
  if (alert.runModal !== $.NSAlertFirstButtonReturn) return false
  $('').writeToFileAtomicallyEncodingError(abortMarker, true, $.NSUTF8StringEncoding, null)
  return true
}

const Controller = ObjC.registerSubclass({
  name: 'WorProgressController',
  superclass: 'NSObject',
  methods: {
    'tick:': {
      types: ['void', ['id']],
      implementation: function() {
        try {
          if (fm.fileExistsAtPath($(doneMarker))) {
            app.stopModalWithCode($.NSOKButton)
            return
          }
          const content = readFile(progressFile)
          if (content.length === 0) return
          const lines = content.split('\n')
          const stepLine = lastMatch(lines, 'STEP\t')
          const subLine = lastMatch(lines, 'SUBSTEP\t')
          const taskLine = lastMatch(lines, 'TASK\t')
          const statusLine = lastMatch(lines, 'STATUS\t')
          let taskText = ''
          let taskPercentText = ''
          let currentPercent = -1
          if (taskLine.length > 0) {
            const taskParts = taskLine.split('\t')
            const taskPct = parseInt(taskParts[1], 10)
            const taskLabel = taskParts.slice(2).join('\t')
            if (taskLabel.length > 0 && !isNaN(taskPct)) {
              currentPercent = Math.max(0, Math.min(100, taskPct))
              taskText = taskLabel
              taskPercentText = 'Sub-progress: ' + currentPercent + '%'
            }
          }
          if (stepLine.length > 0) {
            const parts = stepLine.split('\t')
            const stepNum = parseInt(parts[1], 10)
            const stepTotal = parseInt(parts[2], 10)
            if (!isNaN(stepNum) && !isNaN(stepTotal) && stepTotal > 0) {
              let within = 0
              if (subLine.length > 0) {
                const parsed = parseInt(subLine.split('\t')[1], 10)
                if (!isNaN(parsed)) within = Math.max(0, Math.min(100, parsed))
              }
              if (currentPercent < 0) currentPercent = within
              if (taskPercentText.length === 0 && currentPercent > 0) taskPercentText = 'Sub-progress: ' + currentPercent + '%'
              //count in hundredths of a step so progress inside a step is visible too
              bar.indeterminate = false
              bar.minValue = 0
              bar.maxValue = stepTotal * 100
              bar.doubleValue = (stepNum - 1) * 100 + within
              stepPercentLabel.stringValue = 'Overall: ' + Math.round(bar.doubleValue / bar.maxValue * 100) + '%'
              taskBar.indeterminate = false
              taskBar.minValue = 0
              taskBar.maxValue = 100
              taskBar.doubleValue = Math.max(0, Math.min(100, currentPercent))

              let stepStr = 'Step ' + stepNum + ' of ' + stepTotal
              stepLabel.stringValue = stepStr
            }
            phaseLabel.stringValue = parts.slice(3).join('\t')
          } else if (subLine.length > 0) {
            //work before the first numbered step, such as clearing the cache, still has its own percentage
            const parsed = parseInt(subLine.split('\t')[1], 10)
            if (!isNaN(parsed)) {
              bar.indeterminate = false
              bar.minValue = 0
              bar.maxValue = 100
              bar.doubleValue = Math.max(0, Math.min(100, parsed))
              stepPercentLabel.stringValue = 'Overall: ' + parsed + '%'
              taskBar.indeterminate = false
              taskBar.minValue = 0
              taskBar.maxValue = 100
              taskBar.doubleValue = Math.max(0, Math.min(100, parsed))
              stepLabel.stringValue = parsed + '%'
              taskPercentLabel.stringValue = 'Sub-progress: ' + parsed + '%'
            }
          }
          if (taskText.length > 0) {
            detailLabel.stringValue = taskText
          } else if (statusLine.length > 0) {
            detailLabel.stringValue = statusLine.split('\t').slice(1).join('\t')
          }
          taskPercentLabel.stringValue = taskPercentText
        } catch (e) {}
      }
    },
    //right-click Quit from the Dock/app-switcher sends terminate: to NSApp; since this window is
    //hosted by osascript rather than a full NSApplicationMain run loop, that does not otherwise exit the process
    'handleQuitEvent:withReplyEvent:': {
      types: ['void', ['id', 'id']],
      implementation: function() {
        //quitting mid-flash must go through the same confirmation and let the shell stop the installer
        if (confirmAbort()) app.stopModalWithCode($.NSCancelButton)
      }
    },
    'pumpEvents:': {
      types: ['void', ['id']],
      implementation: function() {
        $.NSRunLoop.currentRunLoop.runModeBeforeDate($.NSDefaultRunLoopMode, $.NSDate.dateWithTimeIntervalSinceNow(0.01))
      }
    },
    'abortClicked:': {
      types: ['void', ['id']],
      implementation: function() {
        if (confirmAbort()) app.stopModalWithCode($.NSCancelButton)
      }
    },
    //the close box must go through the same confirmation, so veto it unless the user means it
    'windowShouldClose:': {
      types: ['bool', ['id']],
      implementation: function() {
        if (!confirmAbort()) return false
        app.stopModalWithCode($.NSCancelButton)
        return true
      }
    },
    //clicking the Dock icon sends aevt/rapp; without a handler a minimised window can never come back
    'handleReopenEvent:withReplyEvent:': {
      types: ['void', ['id', 'id']],
      implementation: function() {
        if (window.isMiniaturized) window.deminiaturize(null)
        window.makeKeyAndOrderFront(null)
        app.activateIgnoringOtherApps(true)
      }
    },
    'applicationShouldTerminate:': {
      types: ['NSUInteger', ['id']],
      implementation: function() {
        if (confirmAbort()) app.stopModalWithCode($.NSCancelButton)
        //NSTerminateCancel: never let AppKit kill this process while a flash is running
        return 0
      }
    }
  }
})
const controller = $.WorProgressController.alloc.init
app.setDelegate(controller)

const width = 680
const height = 330
window = worMakeWindow({ width: width, height: height, title: windowTitle, delegate: controller })

const content = window.contentView
window.contentView = content

const logoWidth = 56
const logoHeight = 173
const progressX = 96
const progressWidth = width - progressX - 20
const logoImage = $.NSImage.alloc.initWithContentsOfFile($(iconPath))
if (!logoImage.isNil()) {
  const logoView = $.NSImageView.alloc.initWithFrame($.NSMakeRect(20, height - 20 - logoHeight, logoWidth, logoHeight))
  logoView.image = logoImage
  logoView.imageScaling = $.NSImageScaleProportionallyUpOrDown
  content.addSubview(logoView)
}

phaseLabel = $.NSTextField.labelWithString('Starting...')
phaseLabel.frame = $.NSMakeRect(progressX, 260, width - progressX - 140, 24)
phaseLabel.font = $.NSFont.systemFontOfSizeWeight(14, $.NSFontWeightBold)
content.addSubview(phaseLabel)

stepLabel = $.NSTextField.labelWithString('')
stepLabel.frame = $.NSMakeRect(width - 120, 260, 100, 24)
stepLabel.font = $.NSFont.systemFontOfSizeWeight(12, $.NSFontWeightRegular)
stepLabel.alignment = $.NSTextAlignmentRight
content.addSubview(stepLabel)

stepPercentLabel = $.NSTextField.labelWithString('')
stepPercentLabel.frame = $.NSMakeRect(width - 140, 207, 120, 16)
stepPercentLabel.font = $.NSFont.monospacedDigitSystemFontOfSizeWeight(11, $.NSFontWeightMedium)
stepPercentLabel.alignment = $.NSTextAlignmentRight
content.addSubview(stepPercentLabel)

detailLabel = $.NSTextField.labelWithString('')
detailLabel.frame = $.NSMakeRect(progressX, 163, progressWidth - 170, 16)
detailLabel.font = $.NSFont.systemFontOfSizeWeight(11, $.NSFontWeightRegular)
content.addSubview(detailLabel)

taskPercentLabel = $.NSTextField.labelWithString('')
taskPercentLabel.frame = $.NSMakeRect(width - 170, 163, 150, 16)
taskPercentLabel.font = $.NSFont.monospacedDigitSystemFontOfSizeWeight(11, $.NSFontWeightMedium)
taskPercentLabel.alignment = $.NSTextAlignmentRight
content.addSubview(taskPercentLabel)

bar = $.NSProgressIndicator.alloc.initWithFrame($.NSMakeRect(progressX, 185, progressWidth, 20))
bar.indeterminate = true
bar.startAnimation(null)
content.addSubview(bar)

taskBar = $.NSProgressIndicator.alloc.initWithFrame($.NSMakeRect(progressX, 142, progressWidth, 10))
taskBar.indeterminate = false
taskBar.minValue = 0
taskBar.maxValue = 100
taskBar.doubleValue = 0
content.addSubview(taskBar)

const noteLabel = $.NSTextField.labelWithString('This window will close automatically when the process finishes.')
noteLabel.frame = $.NSMakeRect(20, 82, width - 40, 20)
noteLabel.font = $.NSFont.systemFontOfSizeWeight(11, $.NSFontWeightRegular)
//1, not $.NSTextAlignmentCenter: that constant bridges as 2, which this runtime draws right-aligned
noteLabel.alignment = 1
content.addSubview(noteLabel)

const abortButton = $.NSButton.buttonWithTitleTargetAction('Abort', controller, 'abortClicked:')
abortButton.bezelStyle = $.NSBezelStyleRounded
abortButton.sizeToFit
const abortWidth = Math.max(96, abortButton.frame.size.width)
abortButton.frame = $.NSMakeRect(Math.round((width - abortWidth) / 2), 22, abortWidth, 32)
content.addSubview(abortButton)

window.makeKeyAndOrderFront(null)
app.activateIgnoringOtherApps(true)

const timer = $.NSTimer.scheduledTimerWithTimeIntervalTargetSelectorUserInfoRepeats(0.4, controller, 'tick:', undefined, true)
worInstallWindowHandlers(controller)
app.runModalForWindow(window)
timer.invalidate
window.orderOut(null)
JXA
)"

  resume_at_flash=0
  while true; do
    abort_marker="$(mktemp -u)"

    gui_start_installer

    wor_osascript -l JavaScript - "$progress_file" "$done_marker" "$WOR_ICON_PATH" "$WOR_APP_TITLE" "$abort_marker" "$WOR_WINDOW_TITLE" <<<"$progress_jxa" >/dev/null 2>&1

    #Command-Q or a crashed/killed osascript can bypass the JXA abort handler. Never leave the flash unattended.
    [ -f "$done_marker" ] || touch "$abort_marker"

    if [ -e "$abort_marker" ];then
      status "Aborting at your request"
      #most of the work runs under sudo, so the tree has to come down with the credential we already hold
      kill_process_tree "$installer_pid"
      wait "$installer_pid" 2>/dev/null
      rm -f "$progress_file" "$done_marker" "$abort_marker" "$auth_marker" "$error_marker"
      saved_log="$(gui_save_failure_log)"
      wor_show_result_notification failure
      wor_osascript -l JavaScript - "Flashing was stopped before it finished.

$DEVICE is now in an unusable state and has to be flashed again before it can boot.

  Full log: $saved_log" "$WOR_ICON_PATH" "$WOR_APP_TITLE" '' "$WOR_WINDOW_TITLE" '' <<<"$completion_jxa" >/dev/null 2>&1
      exit 1
    fi

    #the window can disappear while the flash is still going; wait for the real status instead of
    #assuming failure, which would report a bogus error and leave the flash running unattended
    wait "$installer_pid" 2>/dev/null

    installer_status="$(cat "$done_marker" 2>/dev/null)"
    [ -z "$installer_status" ] && installer_status=1
    rm -f "$progress_file" "$done_marker" "$abort_marker" "$auth_marker"

    if [ "$installer_status" == 0 ];then
      rm -f "$output_log" "$error_marker"
      completion_text="Process completed successfully.

    It is now safe to remove your USB drive."
    else
      #keep the log on failure; the dialog only shows a tail, and the GUI has no terminal to fall back on
      saved_log="$(gui_save_failure_log)"
      #installer writes the error_marker before showing its own error dialog; if it exists, skip the completion dialog
      if installer_showed_own_error ;then
        rm -f "$error_marker"
        exit "$installer_status"
      fi
      #canceling the password dialog, or mistyping it three times, fails before the destructive script
      #ever runs; say so plainly instead of the generic "stopped unexpectedly" wording, which reads like
      #a real crash, and offer to try the password again rather than forcing a full app restart
      password_retry_reason=''
      if grep -qF 'Administrator authentication was canceled or unavailable' "$saved_log" 2>/dev/null ;then
        password_retry_reason='Flashing did not start: administrator access was canceled or the password dialog closed before a password was entered.'
      elif grep -qF 'incorrect password attempts' "$saved_log" 2>/dev/null ;then
        password_retry_reason='Flashing did not start: the administrator password was entered incorrectly too many times.'
      fi
      if [ -n "$password_retry_reason" ];then
        password_retry_choice="$(macos_choose '' "$password_retry_reason

No changes have been made to $DEVICE yet. Trying again picks up at the password step and keeps the files already prepared.

  Full log: $saved_log" retry Abort '' '' '' 'Try Again')" || password_retry_choice=abort
        if [ "$password_retry_choice" == retry ];then
          #the downloads and extraction finished before the password was ever asked for
          resume_at_flash=1
          continue
        fi
        exit "$installer_status"
      fi
      completion_text="The Windows on Raspberry script stopped unexpectedly (exit code $installer_status).

$(gui_log_tail "$saved_log")

Full log: $saved_log"
    fi
    completion_image=''
    [ "$installer_status" == 0 ] && completion_image="$WOR_ASSETS_DIR/next-steps.png"
    #posted before the window opens, so it lands while the app is still in the background
    [ "$installer_status" == 0 ] && wor_show_result_notification success || wor_show_result_notification failure
    wor_osascript -l JavaScript - "$completion_text" "$WOR_ICON_PATH" "$WOR_APP_TITLE" "$completion_image" "$WOR_WINDOW_TITLE" "$([ "${PLAY_SOUND:-1}" == 1 ] && wor_completion_sound)" <<<"$completion_jxa" >/dev/null 2>&1
    exit "$installer_status"
  done
}

if is_macos ;then
  command -v osascript >/dev/null 2>&1 || error "Cannot present graphical interface: osascript is unavailable on this macOS host. Cannot continue."
  setup || exit 1
  announcement_choice="$(macos_show_announcement)" || exit 0
  macos_start_cli
  exit $?
fi

if is_wsl ;then
  error "Cannot present graphical interface: WoR-Flasher does not support WSL.
WSL cannot access USB drives directly, and the drives it does list are WSL's own virtual disks.
On Windows, use the official Windows on Raspberry Imager instead: https://worproject.com/downloads"
fi

if [ "$HOST_OS" != Linux ];then
  if [ "${OS:-}" == "Windows_NT" ] || [[ "$HOST_OS" =~ MINGW|MSYS|CYGWIN|Windows ]];then
    error "Cannot present graphical interface: WoR-Flasher does not support Windows hosts. On Windows, use the official Windows on Raspberry Imager instead: https://worproject.com/downloads"
  else
    error "Cannot present graphical interface: WoR-Flasher supports Linux and macOS hosts only. This host is $HOST_OS."
  fi
fi

if [ -z "${DISPLAY:-}" ] && [ -z "${WAYLAND_DISPLAY:-}" ];then
  error "Cannot present graphical interface: No active graphical display session (DISPLAY or WAYLAND_DISPLAY is unset). Cannot continue."
fi

#run safety checks and install packages
setup || exit 1

if ! command -v yad >/dev/null 2>&1 ;then
  error "Cannot present graphical interface: 'yad' is missing and could not be installed. Cannot continue."
fi

ensure_linux_desktop_identity() { #Install a desktop identity so GNOME maps yad windows to the WoR-Flasher icon.
  is_macos && return 0
  [ -n "${HOME:-}" ] || return 0
  [ -f "$WOR_LOGO_PATH" ] || return 0
  local app_dir desktop_file exec_path icon_path
  app_dir="$HOME/.local/share/applications"
  desktop_file="$app_dir/wor-flasher.desktop"
  mkdir -p "$app_dir" 2>/dev/null || return 0
  exec_path="$(printf '%s' "$DIRECTORY/install-wor-gui.sh" | sed 's/\\/\\\\/g; s/"/\\"/g')"
  icon_path="$(printf '%s' "$WOR_LOGO_PATH" | sed 's/\\/\\\\/g; s/"/\\"/g')"
  cat > "$desktop_file" <<DESKTOP 2>/dev/null || return 0
[Desktop Entry]
Type=Application
Name=$WOR_APP_TITLE
Exec="$exec_path"
Icon=$icon_path
StartupWMClass=$WOR_ICON_NAME
Terminal=false
Categories=Utility;
DESKTOP
}

linux_no_device_message() { #Output: explanation when lsblk sees devices but no safe target drive.
  printf '%s\n\n%s\n%s\n%s' \
    'No external writable target drive was found.' \
    'WoR-Flasher hides loop/snap devices, the current boot disk, and anything that is not a writable whole disk.' \
    'Connect a USB drive or SD card reader, then click Refresh.' \
    'If you expected a drive to appear, run lsblk and confirm it is not the system disk.'
}

linux_choose_one() { #Input: newline choices, prompt, default, optional Back label. Output: choice or Back.
  local choices="$1" prompt="$2" default_choice="$3" back_label="${4:-}" choice result button rows=''
  local buttons=(--button='<b>Next</b>':0)
  [ -z "$back_label" ] || buttons=(--button="<b>$back_label</b>":1 "${buttons[@]}")
  while IFS= read -r choice ;do
    [ -n "$choice" ] || continue
    if [ "$choice" == "$default_choice" ];then
      rows+="TRUE\n${choice}\n"
    else
      rows+="FALSE\n${choice}\n"
    fi
  done <<< "$choices"
  result="$(printf '%b' "$rows" | yad "${yadflags[@]}" --width="$(wor_yad_width 560)" --height="$(wor_yad_height 420)" \
    --list --radiolist --no-selection --no-headers --column=selected:CHK --column=choice \
    --print-column=2 --text="<big><b>$prompt</b></big>" "${buttons[@]}")"
  button=$?
  if [ "$button" == 0 ];then
    [ -n "$result" ] || return 1
    printf '%s\n' "$result"
  elif [ "$button" == 1 ] && [ -n "$back_label" ];then
    printf 'Back\n'
  else
    return 1
  fi
}

ensure_linux_desktop_identity

#this array stores flags that are used in all yad windows - saves on the typing and makes it easy to change an attribute on all dialogs from one place.
#--class sets the window's WM_CLASS so the taskbar/Alt-Tab switcher shows WoR-Flasher instead of
#the generic "yad" process name
wor_init_yad_flags
announcement_image="$(wor_yad_image_for_screen "$WOR_ASSETS_DIR/partnership.png" "$WOR_LOGO_PATH" 880 740)"

#display partnership announcement
#match the macOS announcement composition: the full banner sits above readable centered copy rather
#than consuming almost the whole width and squeezing the text into a one-word column beside it
yad "${yadflags[@]}" --width="$(wor_yad_width 840)" --height="$(wor_yad_height 720)" --center --image-on-top --text-align=center \
  --form --align=center --buttons-layout=center --timeout="$WOR_ANNOUNCEMENT_TIMEOUT" --timeout-indicator=bottom \
  --image="$announcement_image" \
  --field=$'<a href="https://blackoutsecure.app/">Blackout Secure</a> is proud to partner with <a href="https://github.com/Botspot">Botspot</a> and the <a href="https://worproject.com/">Windows on R</a> community, carrying WoR-Flasher forward while preserving Botspot\'s original authorship and project direction.\n\nReport issues, share feedback, or contribute at <a href="https://github.com/Botspot/wor-flasher">Botspot/wor-flasher</a>.\n\nSupport continued development by <a href="https://github.com/sponsors/Botspot">sponsoring Botspot</a> or <a href="https://github.com/sponsors/blackoutsecure?frequency=one-time&amp;amount=8">buying Blackout Secure a coffee</a> on GitHub.':LBL '' \
  --button='<b>Proceed with WoR-Flasher</b>':0 >/dev/null || exit 0

{ #choose destination RPi model and windows build ID
if [ -z "$RPI_MODEL" ] || [ -z "$BID" ];then
  while true;do
    WINDOWS_VER="$(linux_choose_one $'Windows 11\nWindows 10\nMore options' 'Choose Windows version' 'Windows 11')" || exit 0
    rpi_choice="$(linux_choose_one $'Raspberry Pi 5\nRaspberry Pi 4 / Pi 400\nRaspberry Pi 3 / Pi 2 v1.2' 'Choose Raspberry Pi model' 'Raspberry Pi 5' Back)" || exit 0
    [ "$rpi_choice" == Back ] && continue
    case "$rpi_choice" in
      'Raspberry Pi 5') RPI_MODEL=5 ;;
      'Raspberry Pi 4 / Pi 400') RPI_MODEL=4 ;;
      'Raspberry Pi 3 / Pi 2 v1.2') RPI_MODEL=3 ;;
      *) continue ;;
    esac
    break
  done

  case "$WINDOWS_VER" in
    'Windows 11' | 'Windows 10')
      loading_dialog "Finding best $WINDOWS_VER image version..." &
      loader_pid=$!
      trap stop_loader EXIT

      list_bids 10 >/dev/null #set $versions globally so it is not downloaded twice
      if [ "$WINDOWS_VER" == 'Windows 11' ];then
        BID="$(get_bid 11)" || exit 1
      elif [ "$WINDOWS_VER" == 'Windows 10' ];then
        BID="$(get_bid 10)" || exit 1
      fi

      stop_loader
      ;;

    'More options')
      #display more options for OS choice to user: enter exact version, use ISO, use pre-extracted ISO

      BID=''
      while [ -z "$BID" ];do
        reply="$(echo -e "FALSE\nChoose an exact Windows version to download\nenter exact
FALSE\nUse a Windows ISO file\nuse iso
FALSE\nUse a cached version of Windows from a previous run\nuse cached" | yad "${yadflags[@]}" --width="$(wor_yad_width 420)" \
          --list --radiolist --column=chk:CHK --column=human --column=script:HD --no-headers --print-column=3 --no-selection \
          --text=$'<big><b>More options</b></big>' \
          --button='<b>Next</b>':0)"
        button=$?
        [ $button != 0 ] && exit 0

        case "$reply" in
          'enter exact')
            list_bids 10 >/dev/null #set $versions globally so it is not downloaded twice
            while [ -z "$BID" ];do
              BID="$(echo -n "$(list_bids_supported 11 | sed 's/^/Windows 11 /g'
              list_bids_supported 10 | sed 's/^/Windows 10 /g')" | sed 's/^/FALSE\n/g' | yad "${yadflags[@]}" --width="$(wor_yad_width 420)" \
                --list --radiolist --column=chk:CHK --column=human --no-headers --print-column=2 --no-selection \
                --text=$'Choose version of Windows:' \
                --button='<b>Next</b>':0)"
              button=$?
              [ $button != 0 ] && exit 0

              #Isolate build number from selection
              BID="$(echo "$BID" | awk '{print $3}')"
            done
            break
            ;;
          'use iso')
            SOURCE_FILE="$(yad "${yadflags[@]}" --width="$(wor_yad_width 420)" \
              --file --file-filter "ISO disk images | *.ISO *.iso" \
              --text=$'<big><b>Import ISO file</b></big>\nMust be an ARM64 version of Windows from <a href="https://uupdump.net">uupdump.net</a>' \
              --button="<b>Cancel</b>":1 --button="<b>OK</b>":0)"

            #verify ISO file
            if [ -z "$SOURCE_FILE" ];then
              break #exit ISO file menu
            elif ! iso_problem="$(validate_iso_file "$SOURCE_FILE")" ;then
              yad "${yadflags[@]}" --text="$iso_problem"
              SOURCE_FILE=''
            else #ISO file looks good
              #Infer Build ID based on filename of ISO
              BID="$(bid_from_iso_name "$SOURCE_FILE")"
              if [ -z "$BID" ];then
                BID="$(yad --form --field= '' "${yadflags[@]}" \
                  --text='To store files from this ISO, this script needs to know the Windows build number of this ISO.\nPlease enter it now: (example: '"$EXAMPLE_BID"')' \
                  --button="<b>OK</b>":0)"
                [ -z "$BID" ] && error "Cannot proceed without a build number for your ISO file."
              fi
              #Infer language based on filename of ISO
              WIN_LANG="$(lang_from_iso_name "$SOURCE_FILE")"
              if [ -z "$WIN_LANG" ];then
                WIN_LANG="$(yad --form --field= '' "${yadflags[@]}" \
                  --text='To store files from this ISO, this script needs to know the language of this Windows ISO.\nPlease enter it now: (example: en-us)' \
                  --button="<b>OK</b>":0)"
                if [ -z "$WIN_LANG" ];then
                  error "Cannot proceed without a language for your ISO file."
                elif ! is_known_win_lang "$WIN_LANG" ;then
                  error "Language code was not found in the list!\n$(list_langs | awk '{print $1}' | tr '\n' ' ')"
                fi
              fi
              break
            fi
            ;;
          'use cached')
            #Discover past extracted ISO files in this DL_DIR so user does not need to keep ISO
            #folders in DL_DIR named winfiles_from_iso_<BID>_<WIN_LANG>
            while true;do
              list=''
              existing_winfiles="$(list_cached_winfiles)"

              echo "$existing_winfiles"

              for folder in $existing_winfiles ;do
                BID="$(bid_from_winfiles_dir "$folder")"
                WIN_LANG="$(lang_from_winfiles_dir "$folder")"

                list+="FALSE\n$(get_os_name "$BID") $WIN_LANG\n${folder}\n"
                num_opts=$((num_opts+1))
              done
              unset BID WIN_LANG #Avoid leaving these variables set from the loop

              folder="$(echo -ne "$list" | yad "${yadflags[@]}" --height="$(wor_yad_height 320)" \
                --list --radiolist --column=chk:CHK --column=human --column=script:HD --no-headers --print-column=3 --no-selection \
                --text=$'<big><b>Choose cached version</b></big>\nIf the list is empty, please use the same working directory (DL_DIR) you used last time.\nDL_DIR: <b><u>'"$DL_DIR"'</u></b>' \
                --button='<b>Change DL<u>  </u>DIR</b>':2 \
                --button='<b>Next</b>':0)"
              button=$?

              case $button in
                0) #Next
                  if [ ! -z "$folder" ];then
                    #A cached version of windows (winfiles folder) was selected; infer BID and WIN_LANG from it
                    BID="$(bid_from_winfiles_dir "$folder")"
                    WIN_LANG="$(lang_from_winfiles_dir "$folder")"

                    #DL_DIR cannot be changed later on - it is being relied upon for winfiles
                    break
                  else
                    #nothing selected; present the window again
                    true
                  fi
                  ;;
                2) #change DL_DIR
                  DL_DIR="$(yad "${yadflags[@]}" --file --directory --mime-filter="Directories | inode/directory" \
                    --width="$(wor_yad_width 500)" --height="$(wor_yad_height 400)" --title="Choose DL_DIR" \
                    --text=$'Choose directory for everything to be downloaded.\nIn this case you should select the directory where everything <i>was</i> downloaded the last time you ran WoR-Flasher.' \
                    --button="<b>Cancel</b>":1 --button="<b>OK</b>":0 \
                    || echo "$DL_DIR")"
                    #This ^^^^^^^^^^^ preserves the current value of DL_DIR if anything other than OK is clicked
                  ;;
                *)
                  exit 0 #user wishes to exit the list of previously extracted winfiles
                  ;;
              esac
            done
            ;;
        esac
      done
      ;;
    *)
      error "Unrecognized user-selected WINDOWS_VER '$WINDOWS_VER'"
      ;;
  esac
fi
echo "BID: $BID
RPI_MODEL: $RPI_MODEL"
}

{ #choose language
if [ -z "$WIN_LANG" ];then
  WIN_LANG="$(default_win_lang)"
fi
echo "WIN_LANG: $WIN_LANG"
}

{ #choose device to flash
if [ -z "$DEVICE" ];then
  while [ -z "$DEVICE" ] || [ ! -b "$DEVICE" ];do
    IFS=$'\n'
    DEV_LIST=''
    for device in $(list_dev_paths) ;do
      [ "$(get_size_raw "$device")" -le 0 ] && continue
      DEV_LIST="FALSE
${device}
<b>${device}</b>
$(lsblk -dno SIZE "$device")B
$(get_device_name "$device")
$DEV_LIST"
    done

    if [ -z "$DEV_LIST" ];then
      device_prompt="$(linux_no_device_message)"
    else
      device_prompt='Choose device to flash:'
    fi

    DEVICE="$(echo -n "$DEV_LIST" | sed -e '0,/FALSE/ s/FALSE/TRUE/' | yad "${yadflags[@]}" --text="$device_prompt" --width="$(wor_yad_width 520)" \
      --list --radiolist --no-selection --no-headers --column=chk:CHK --column=echoname:HD --column=name --column=size --column=pretty-name \
      --print-column=2 --tooltip-column=3 \
      --button="<b>Refresh</b>!!Reload the list of connected drives to detect new ones":2 --button='<b>Next</b>':0)"
    button=$?
    if [ $button == 0 ];then
      #OK
      true #do nothing and while loop will exit if $DEVICE is valid
    elif [ $button == 2 ];then
      #Refresh
      DEVICE=''
    else
      #Cancel, or unknown button
      exit 1
    fi
  done
elif [ -z "$(lsblk -no PATH "$DEVICE")" ];then
  error "Invalid value for DEVICE: $DEVICE is not a valid drive!"
fi
#same guard the macOS wizard applies, so a caller-supplied DEVICE cannot be the host's own boot disk
is_safe_target_device "$DEVICE" || error "Refusing to overwrite $DEVICE, which is this system's boot drive."
echo "DEVICE: $DEVICE"
}

{ #choose installation mode from the detected drive capacity
device_capability="$(drive_capability "$DEVICE")"
validate_install_mode "$device_capability"

if [ -z "$CAN_INSTALL_ON_SAME_DRIVE" ] && [ "$device_capability" == recovery ];then
  echo "Drive $DEVICE is too small to install Windows to itself. Using recovery-drive mode to install Windows on another larger device."
  CAN_INSTALL_ON_SAME_DRIVE=0
elif [ -z "$CAN_INSTALL_ON_SAME_DRIVE" ];then
  while [ -z "$CAN_INSTALL_ON_SAME_DRIVE" ];do
    install_mode="$(echo -e "TRUE\ninstall\nInstallation drive\nInstall Windows onto this 25 GB+ drive\nFALSE\nrecovery\nRecovery drive\nInstall Windows onto another >16 GB drive" | yad "${yadflags[@]}" --width="$(wor_yad_width 520)" \
      --list --radiolist --column=chk:CHK --column=value:HD --column=Mode --column=Description --no-headers --print-column=2 --no-selection \
      --text=$'<big><b>Installation mode</b></big>\nThis drive is large enough for either mode. Choose what you want it to do:' \
      --button='<b>Next</b>':0)"
    button=$?
    [ $button != 0 ] && exit 1

    case "$install_mode" in
      install)
        CAN_INSTALL_ON_SAME_DRIVE=1
        ;;
      recovery)
        CAN_INSTALL_ON_SAME_DRIVE=0
        ;;
    esac
  done
fi
echo "CAN_INSTALL_ON_SAME_DRIVE: $CAN_INSTALL_ON_SAME_DRIVE"
}

{ #Offer to use ZRAM DL_DIR if appropriate
#if a windows ESD file will be downloaded (no point in using ram if windows is already in DL_DIR), an ISO will not be used, and DL_DIR has not already been customized
if [ ! -f "${DL_DIR}/winfiles_from_iso_${BID}_${WIN_LANG}/alldone" ] && [ ! -f "${DL_DIR}/winfiles_${BID}_${WIN_LANG}/alldone" ] && [ -z "$SOURCE_FILE" ] && [ "$DL_DIR" == "$HOME/wor-flasher-files" ];then
  #if total usable RAM is >= 5GB
  if [ "$(awk '/MemTotal/ {print $2}' /proc/meminfo)" -ge $((5*1024*1024)) ];then
    #if kernel modules are available
    if [ -d "/lib/modules/$(uname -r)" ];then
      #tooltip text of 'Use RAM' button will explain that More RAM app from Pi-Apps will be installed, assuming it is not already installed.
      if [ -f /usr/bin/zram.sh ] && [ -d /zram ];then
        tooltip='Will set DL_DIR to the /zram folder - this folder was set up when you installed <b>More RAM</b> from Pi-Apps.'
      elif [ -f /usr/local/bin/pi-apps ];then
        tooltip='Will install <b>More RAM</b> from Pi-Apps and then set DL_DIR to the new ramdisk at <u>/zram</u>.'
      else
        tooltip='Will setup a RAM-compression tool from Pi-Apps and then set DL_DIR to the new ramdisk at <u>/zram</u>. Please note that Pi-Apps itself will not be installed.'
      fi

      yad "${yadflags[@]}" --width="$(wor_yad_width 500)" --form --field="About 4.2GB of files need to be downloaded to system storage before flashing can begin.
But your system has $(echo "scale=1 ; $( awk '/MemTotal/ {print $2}' /proc/meminfo ) / 1048576 " | bc )GB of RAM. Everything can be downloaded to RAM if you prefer.
Choose this if:
- You don't have enough space in $HOME
- You want your system storage to last as long as possible
- You don't plan to use WoR-Flasher often:LBL" \
        --image="$WOR_ASSETS_DIR/ram.png" --image-on-top \
        --button="Use ${DL_DIR}":2 \
        --button="<b>Use RAM</b>!!${tooltip}":0
      button=$?

      if [ "$button" == 0 ];then
        status "User chose to download everything to RAM."
        echo "For best results, please close all other programs. (especially web browsers and games)"
        yad "${yadflags[@]}" --width="$(wor_yad_width 500)" --image="$WOR_ASSETS_DIR/ram.png" --image-on-top \
          --form --field="OK! Will download everything to RAM. For best results, please close all other programs. (especially web browsers and games):LBL" \
          --button='<b>OK</b>':0 >/dev/null

        #install zram if necessary
        if [ ! -f /usr/bin/zram.sh ];then
          #install More RAM
          loading_dialog "Setting up RAM..." &
          loader_pid=$!
          trap stop_loader EXIT

          if [ -f "$HOME/pi-apps/manage" ];then
            #if Pi-Apps installed to default location, install More RAM from there
            "$HOME/pi-apps/manage" install 'More RAM'
            exitcode=$?
          elif [ -f /usr/local/bin/pi-apps ] && [ -f "$(dirname "$(cat /usr/local/bin/pi-apps | sed -n 2p)")/manage" ];then
            #if Pi-Apps installed to another folder, install More RAM from there
            "$(dirname "$(cat /usr/local/bin/pi-apps | sed -n 2p)")/manage" install 'More RAM'
            exitcode=$?
          else
            #Pi-Apps is not installed, so run More RAM script straight from the pi-apps github repo
            wget -qO- 'https://raw.githubusercontent.com/Botspot/pi-apps/master/apps/More%20RAM/install' | bash
            #either wget or bash could have failed, so check them both
            if [ ${PIPESTATUS[0]} == 0 ] && [ ${PIPESTATUS[1]} == 0 ];then
              exitcode=0
            else
              exitcode=1
            fi
          fi
          #installation complete, so close pulsating progress bar dialog
          stop_loader

          #edge case: if user had installed More RAM before and disabled the /zram folder, enable it now
          if [ "$exitcode" == 0 ] && [ ! -d /zram ];then
            sudo zram.sh storage-on
            if [ ! -d /zram ];then
              echo_red "zram.sh failed to create /zram ramdisk."
              exitcode=1
            fi
          fi

          #display warning dialog if installing More RAM failed
          if [ "$exitcode" == 0 ];then
            DL_DIR='/zram'
          else
            yad "${yadflags[@]}" --text="Failed to install 'More RAM' app from Pi-Apps.\nWoR-Flasher will not download files to RAM."
          fi
        else
          #zram already installed; now make sure /zram exists
          if [ -d /zram ];then
            DL_DIR='/zram'
          else
            sudo zram.sh storage-on
            if [ -d /zram ];then
              DL_DIR='/zram'
            else
              yad "${yadflags[@]}" --text="Failed to set up ZRAM ramdisk.\nWoR-Flasher will not download files to RAM."
            fi
          fi
        fi
      fi
    fi
  fi
fi
}

#if no user-supplied CONFIG_TXT variable, set it to initial value for yad to change later
set_default_config_txt

{ #confirmation dialog and edit config.txt

window_text="$(settings_summary_markup)

To continue, click Flash. To review or change these settings, click Advanced. To cancel, close this window."

#by default, if a windows image exists, don't delete it to rebuild it
rm_img=FALSE
existing_img_chk=()

while true;do #repeat the Installation Overview window until Flash button clicked

  if [ "$DRY_RUN" == 1 ];then
    deletion_warning="DRY_RUN=1, so the target drive will not be modified."
    deletion_warning_2="$deletion_warning"
  else
    deletion_warning="<b>Warning!</b> All data on the target drive will be deleted!"
    deletion_warning_2="$deletion_warning Backup any files before it's too late!"
  fi

  yad "${yadflags[@]}" --width="$(wor_yad_width 640)" --height="$(wor_yad_height 700)" --image="$WOR_ASSETS_DIR/overview.png" --image-on-top \
    --form --scroll --field="$window_text":LBL '' \
    "${existing_img_chk[@]}" \
    --field="$deletion_warning":LBL '' \
    --button='<b>View / Edit config.txt...</b>':3 \
    --button='<b>Advanced...</b>'!!"More settings, intended for the advanced user or for troubleshooting":2 \
    --button='<b>Flash</b>'!!"$deletion_warning_2":0 >/dev/null
  button=$?

  if [ $button == 0 ];then
    #button: Flash
    break
  elif [ $button == 2 ];then
    #button: Advanced options

    refresh_prompt=() #this variable is populated if the Advanced Options window is repeated, to let the user know why

    while true;do #repeat the advanced options window until the DL_DIR is not changed, or until Cancel is clicked
      fields=()
      uefi_pinned="$(uefi_pinned_version)"
      #make entry to change DL_DIR
      if [ -f "${DL_DIR}/winfiles_from_iso_${BID}_${WIN_LANG}/alldone" ];then
        #lock DL_DIR if winfiles come from previously extracted ISO - changing it would lose these files and they cannot be replaced by the internet
        fields+=("--field=Working directory: (DL<u>  </u>DIR):RO" 'Cannot be changed')
      else
        fields+=("--field=Working directory: (DL<u>  </u>DIR):DIR" "$DL_DIR")
      fi

      #make entry for peinstaller
      if [ -d "$DL_DIR/peinstaller" ];then
        fields+=("--field=Check this box to re-download PE Installer":CHK 'FALSE')
      else
        fields+=("--field=Will download PE Installer":LBL '')
      fi
      fields+=("--field=            <u>$DL_DIR/peinstaller</u>":LBL '')

      #make entry for driverpackage
      if [ -d "$DL_DIR/driverpackage" ];then
        fields+=("--field=Check this box to re-download RPi Drivers":CHK 'FALSE')
      else
        fields+=("--field=Will download RPi Drivers":LBL '')
      fi
      fields+=("--field=            <u>$DL_DIR/driverpackage</u>":LBL '')

      #make entry for uefipackage
      if [ -d "$DL_DIR/pi${RPI_MODEL}-uefipackage" ];then
        fields+=("--field=Check this box to re-download UEFI package":CHK 'FALSE')
      else
        fields+=("--field=Will download UEFI package":LBL '')
      fi
      fields+=("--field=            <u>$DL_DIR/pi${RPI_MODEL}-uefipackage</u>":LBL '')

      #display status of winfiles - if they will be downloaded or are ready to use
      if [ -f "${DL_DIR}/winfiles_${BID}_${WIN_LANG}/alldone" ];then
        #already extracted
        fields+=("--field=Windows files: Already extracted and ready to use.":LBL '')
        fields+=("--field=            <small><u>${DL_DIR}/winfiles_${BID}_${WIN_LANG}</u></small>":LBL '')
      elif [ -f "${DL_DIR}/winfiles_from_iso_${BID}_${WIN_LANG}/alldone" ];then
        #already extracted
        fields+=("--field=Windows files: Already extracted and ready to use.":LBL '')
        fields+=("--field=            <small><u>${DL_DIR}/winfiles_from_iso_${BID}_${WIN_LANG}</u></small>":LBL '')
      elif [ ! -z "$SOURCE_FILE" ];then
        #will use ISO file
        fields+=("--field=Windows files: Will be extracted from your ISO file.":LBL '')
        fields+=("--field=            <small><u>${SOURCE_FILE}</u></small>":LBL '')
      else
        #ESD will be downloaded
        fields+=("--field=Windows files: Will download and extract Windows ESD image":LBL '')
        fields+=("--field=            <small><u>${DL_DIR}/winfiles_${BID}_${WIN_LANG}</u></small>":LBL '')
      fi

      #make entry for dry run
      fields+=("--field=$(wor_advanced_label dryrun "$uefi_pinned" "$DRIVER_VER" "$RPI_MODEL")":CHK "$(wor_yad_bool "$DRY_RUN")")

      #make entries for the customization toggles
      #the engine ignores PI4_AUTO_DISABLE_3GB unless RPI_MODEL is 4; yad can't disable one field, so mark it and drop the value below
      [ "$RPI_MODEL" == 4 ] && pi4_applicable=1 || pi4_applicable=0
      #yad renders markup, so an inapplicable row is italicised rather than greyed out
      pi4_label="$(wor_pi4_label "$RPI_MODEL")"
      [ "$pi4_applicable" == 1 ] || pi4_label="<i>$pi4_label</i>"
      fields+=("--field=$(wor_yad_label "$(wor_advanced_label oobe "$uefi_pinned" "$DRIVER_VER" "$RPI_MODEL")" "$(wor_advanced_caution oobe)")":CHK "$(wor_yad_bool "$OOBE_NETWORK_BYPASS")")
      fields+=("--field=$pi4_label":CHK "$(wor_yad_bool "$([ "$pi4_applicable" == 1 ] && echo "$PI4_AUTO_DISABLE_3GB" || echo 0)")")
      fields+=("--field=$(wor_yad_label "$(wor_advanced_label uefi "$uefi_pinned" "$DRIVER_VER" "$RPI_MODEL")" "$(wor_advanced_caution uefi)")":CHK "$(wor_yad_bool "$UEFI_USE_LATEST")")
      fields+=("--field=$(wor_yad_label "$(wor_advanced_label drivers "$uefi_pinned" "$DRIVER_VER" "$RPI_MODEL")" "$(wor_advanced_caution drivers)")":CHK "$(wor_yad_bool "$DRIVERS_USE_LATEST")")
      fields+=("--field=$(wor_yad_label "$(wor_advanced_label verify "$uefi_pinned" "$DRIVER_VER" "$RPI_MODEL")" "$(wor_advanced_caution verify)")":CHK "$(wor_yad_bool "$SKIP_IMAGE_VERIFICATION")")
      #in recovery mode this config.txt boots the installer media; WoR-PE writes the target drive's own copy
      config_scope="$(wor_config_scope "$CAN_INSTALL_ON_SAME_DRIVE")"
      fields+=("--field=$(wor_config_txt_label "$config_scope") - recommended":CHK "$(wor_yad_bool "$APPLY_CUSTOM_CONFIG_TXT")")
      #USE_CACHE has three values, so it needs a combo rather than a check box; the selected item comes first
      case "$USE_CACHE" in
        0) cache_items='Re-download everything, ignoring the cache!Reuse cached files when they still match (recommended)!Trust the cache without checking it' ;;
        2) cache_items='Trust the cache without checking it!Re-download everything, ignoring the cache!Reuse cached files when they still match (recommended)' ;;
        *) cache_items='Reuse cached files when they still match (recommended)!Re-download everything, ignoring the cache!Trust the cache without checking it' ;;
      esac
      fields+=("--field=Downloaded files":CB "$cache_items")
      fields+=("--field=Create an optional local Windows administrator account":CHK "$(wor_yad_bool "$WINDOWS_ACCOUNT_SETUP")")
      fields+=("--field=Windows username":TXT "$WINDOWS_ACCOUNT_USERNAME")
      fields+=("--field=Windows password":H "$WINDOWS_ACCOUNT_PASSWORD")
      fields+=("--field=Configure Windows keyboard and regional settings":CHK "$(wor_yad_bool "$WINDOWS_LOCALE_SETUP")")
      locale_items=""
      curr_locale_item=""
      other_locale_items=""
      while IFS=$'\t' read -r locale_code locale_label ;do
        [ -z "$locale_code" ] && continue
        entry="${locale_code}: ${locale_label:-$locale_code}"
        if [ "$(printf '%s' "$locale_code" | tr '[:upper:]' '[:lower:]')" == "$(printf '%s' "$WINDOWS_LOCALE" | tr '[:upper:]' '[:lower:]')" ];then
          curr_locale_item="$entry"
        else
          [ -n "$other_locale_items" ] && other_locale_items+="!${entry}" || other_locale_items="${entry}"
        fi
      done < <(list_windows_locale_options)
      [ -n "$curr_locale_item" ] && locale_items="${curr_locale_item}!${other_locale_items}" || locale_items="${WINDOWS_LOCALE}: ${WINDOWS_LOCALE}!${other_locale_items}"
      fields+=("--field=Windows locale":CB "$locale_items")
      lang_items=""
      curr_item=""
      other_items=""
      while IFS=: read -r l_code l_name ;do
        [ -z "$l_code" ] && continue
        entry="${l_code}: ${l_name}"
        if [ "$l_code" == "$WIN_LANG" ];then
          curr_item="$entry"
        else
          [ -n "$other_items" ] && other_items+="!${entry}" || other_items="${entry}"
        fi
      done < <(list_langs_preferred)
      [ -n "$curr_item" ] && lang_items="${curr_item}!${other_items}" || lang_items="${other_items}"
      fields+=("--field=Choose Windows language":CB "$lang_items")
      #appended last so every sed -n Np above keeps its position
      sound_items=""
      curr_sound_item=""
      other_sound_items=""
      sel_sound="$(wor_completion_sound)"
      while IFS=$'\t' read -r sound_value sound_label ;do
        [ -z "$sound_value" ] && continue
        if [ "$sound_value" == "$sel_sound" ];then
          curr_sound_item="$sound_label"
        else
          [ -n "$other_sound_items" ] && other_sound_items+="!${sound_label}" || other_sound_items="${sound_label}"
        fi
      done < <(wor_sound_options)
      if [ -n "$curr_sound_item" ] || [ -n "$other_sound_items" ];then
        [ -n "$curr_sound_item" ] && sound_items="${curr_sound_item}!${other_sound_items}" || sound_items="${other_sound_items}"
        fields+=("--field=Play a sound when the flash finishes":CHK "$(wor_yad_bool "${PLAY_SOUND:-1}")")
        fields+=("--field=Completion sound":CB "$sound_items")
        fields+=("--field=Show a notification when the flash finishes":CHK "$(wor_yad_bool "${SHOW_NOTIFICATION:-1}")")
      fi

      output="$(yad "${yadflags[@]}" --width="$(wor_yad_width 640)" --height="$(wor_yad_height 700)" --image-on-top \
        "${refresh_prompt[@]}" \
        --form --scroll \
        "${fields[@]}" \
        --button="<b>Back</b>":1 --button="<b>OK</b>":0
      )"
      button=$?

      if [ "$button" == 0 ];then #everything in this if statement is skipped if Cancel is clicked
        if [ ! -f "${DL_DIR}/winfiles_from_iso_${BID}_${WIN_LANG}/alldone" ] && [ "$DL_DIR" != "$(echo "$output" | sed -n 1p)" ];then
          #DL_DIR was changed - only honor the value if it is allowed to be changed
          DL_DIR="$(echo "$output" | sed -n 1p)"
          echo "In the Advanced Options window, user changed DL_DIR to $DL_DIR"

          #explain to user why the Advanced Options window was refreshed when they clicked OK
          refresh_prompt=("--text=<b>Note:</b> As you changed the working directory, this window has refreshed."$'\n'"Any previous checkbox values have been ignored.")

          #skipping the 'break' command to repeat the Advanced Options window

        else #if DL_DIR was not changed, then review the subsequent check-box values
          #peinstaller
          if [ "$(echo "$output" | sed -n 2p)" == TRUE ];then
            echo "User checked the box to delete $DL_DIR/peinstaller"
            rm -rf "$DL_DIR/peinstaller"
          fi
          #driverpackage
          if [ "$(echo "$output" | sed -n 4p)" == TRUE ];then
            echo "User checked the box to delete $DL_DIR/driverpackage"
            rm -rf "$DL_DIR/driverpackage"
          fi
          #uefipackage
          if [ "$(echo "$output" | sed -n 6p)" == TRUE ];then
            echo "User checked the box to delete $DL_DIR/pi${RPI_MODEL}-uefipackage"
            rm -rf "$DL_DIR/pi${RPI_MODEL}-uefipackage"
          fi
          #windows image
          if [ "$(echo "$output" | sed -n 8p)" == TRUE ];then
            echo "User checked the box to delete $(echo "$DL_DIR"/uupdump/*ARM64*.ISO)"
            rm -f "$DL_DIR"/uupdump/*ARM64*.ISO
            rm_img=FALSE #This "Advanced..." dialog just deleted the windows image, so no need for the var to remain 'TRUE' - remove unnecessary output when removing twice
          fi
          #DRY_RUN
          if [ "$(echo "$output" | sed -n 10p)" == TRUE ] && [ "$DRY_RUN" == 0 ];then
            echo "User checked the box to set DRY_RUN=1"
            DRY_RUN=1
          elif [ "$(echo "$output" | sed -n 10p)" == FALSE ] && [ "$DRY_RUN" == 1 ];then
            echo "User checked the box to set DRY_RUN=0"
            DRY_RUN=0
          fi
          #customization toggles
          [ "$(echo "$output" | sed -n 11p)" == TRUE ] && OOBE_NETWORK_BYPASS=1 || OOBE_NETWORK_BYPASS=0
          #keep the existing preference when the toggle wasn't applicable, so switching back to a Pi 4 doesn't lose it
          if [ "$pi4_applicable" == 1 ];then
            [ "$(echo "$output" | sed -n 12p)" == TRUE ] && PI4_AUTO_DISABLE_3GB=1 || PI4_AUTO_DISABLE_3GB=0
          fi
          [ "$(echo "$output" | sed -n 13p)" == TRUE ] && UEFI_USE_LATEST=1 || UEFI_USE_LATEST=0
          [ "$(echo "$output" | sed -n 14p)" == TRUE ] && DRIVERS_USE_LATEST=1 || DRIVERS_USE_LATEST=0
          [ "$(echo "$output" | sed -n 15p)" == TRUE ] && SKIP_IMAGE_VERIFICATION=1 || SKIP_IMAGE_VERIFICATION=0
          [ "$(echo "$output" | sed -n 16p)" == TRUE ] && APPLY_CUSTOM_CONFIG_TXT=1 || APPLY_CUSTOM_CONFIG_TXT=0
          case "$(echo "$output" | sed -n 17p)" in
            'Re-download everything'*) USE_CACHE=0 ;;
            'Trust the cache'*) USE_CACHE=2 ;;
            'Reuse cached files'*) USE_CACHE=1 ;;
          esac
          [ "$(echo "$output" | sed -n 18p)" == TRUE ] && WINDOWS_ACCOUNT_SETUP=1 || WINDOWS_ACCOUNT_SETUP=0
          WINDOWS_ACCOUNT_USERNAME="$(echo "$output" | sed -n 19p)"
          WINDOWS_ACCOUNT_PASSWORD="$(echo "$output" | sed -n 20p)"
          [ "$(echo "$output" | sed -n 21p)" == TRUE ] && WINDOWS_LOCALE_SETUP=1 || WINDOWS_LOCALE_SETUP=0
          WINDOWS_LOCALE="$(echo "$output" | sed -n 22p | awk -F': ' '{print $1}')"
          sel_lang="$(echo "$output" | sed -n 23p)"
          sel_code="${sel_lang%%:*}"
          if is_known_win_lang "$sel_code" ;then
            WIN_LANG="$sel_code"
          fi
          if [ -n "$sound_items" ];then
            [ "$(echo "$output" | sed -n 24p)" == TRUE ] && PLAY_SOUND=1 || PLAY_SOUND=0
            #the combo shows labels, so map the chosen one back to the value the player needs
            sel_sound_label="$(echo "$output" | sed -n 25p)"
            sel_sound="$(wor_sound_options | awk -F'\t' -v l="$sel_sound_label" '$2 == l {print $1; exit}')"
            [ -n "$sel_sound" ] && COMPLETION_SOUND="$sel_sound"
            [ "$(echo "$output" | sed -n 26p)" == TRUE ] && SHOW_NOTIFICATION=1 || SHOW_NOTIFICATION=0
          fi
          #end of parsing check-box values for advanced options window

          break #as the DL_DIR value was not changed, go back to the Installation Overview window
        fi

      else #button != OK
        break #Don't save and go back to Installation Overview
      fi
    done #end of repeating the advanced options window

  elif [ $button == 3 ];then
    config_output="$(yad "${yadflags[@]}" --width="$(wor_yad_width 640)" --height="$(wor_yad_height 600)" \
      --form --scroll \
      --field="<b>View / Edit config.txt</b>     <small><a href=\"https://www.raspberrypi.com/documentation/computers/config_txt.html\">Configuration reference</a></small>":TXT "$CONFIG_TXT" \
      --button="<b>Back</b>":1 --button="<b>Save</b>":0)"
    config_button=$?
    [ "$config_button" == 0 ] && CONFIG_TXT="$config_output"
    continue

  else
    #User exited when reviewing information and customizing config.txt
    exit 1
  fi

done #end of repeating the Installation Overview window

#if user checked the box to rebuild the image, delete the image now
if [ "$rm_img" == TRUE ];then
  echo "User checked the box to delete the pre-existing windows image."
  rm -f "$DL_DIR/uupdump"/*ARM64*.ISO
fi

#display multi-line CONFIG_TXT variable
echo -e "CONFIG_TXT: ⤵\n$(echo "$CONFIG_TXT" | sed 's/^/  > /g')\nCONFIG_TXT: ⤴\n"
}

echo "Running install-wor.sh"

abort_marker="$(mktemp -u)"
gui_start_installer

progress_fifo="$(mktemp -u)"
mkfifo "$progress_fifo"
tail -n +1 -F "$progress_file" > "$progress_fifo" 2>/dev/null &
tail_pid=$!
awk -F'\t' '
  #pct, not sub: sub() is a built-in awk function and cannot be used as a variable
  #before the first STEP (e.g. while clearing the cache) the percentage stands on its own
  function overall() {
    msg=title;
    if (pct > 0 && task_title != "") msg=task_title;
    if (total+0 > 0) {
      printf("%d\n", ((step-1)*100 + pct) / (total*100) * 100);
      if (pct > 0) {
        printf("# [Step %d/%d] %s (%d%%)\n", step, total, msg, pct);
      } else {
        printf("# [Step %d/%d] %s\n", step, total, title);
      }
    } else {
      printf("%d\n", pct+0);
      printf("# %s (%d%%)\n", status_msg, pct);
    }
  }
  /^STEP/    { step=$2+0; total=$3+0; title=$4; pct=0; task_title=""; overall(); fflush() }
  /^SUBSTEP/ { pct=$2+0; if (pct<0) pct=0; if (pct>100) pct=100; overall(); fflush() }
  /^TASK/    { pct=$2+0; if (pct<0) pct=0; if (pct>100) pct=100; task_title=$3; overall(); fflush() }
  /^STATUS/  { status_msg=$2; if (step+0 == 0) overall(); else printf("# %s\n", status_msg); fflush() }
' < "$progress_fifo" | yad "${yadflags[@]}" --width="$(wor_yad_width 680)" --height="$(wor_yad_height 330)" \
  --progress --image="$WOR_LOGO_PATH" --text="Starting..." --button='<b>Abort</b>':1 &
yad_pid=$!

progress_aborted=0
while [ ! -f "$done_marker" ];do
  if ! kill -0 "$yad_pid" 2>/dev/null ;then
    progress_aborted=1
    break
  fi
  sleep 0.3
done

if [ "$progress_aborted" == 1 ];then
  touch "$abort_marker"
  status "Aborting at your request"
  kill_process_tree "$installer_pid"
  wait "$installer_pid" 2>/dev/null
  kill "$tail_pid" 2>/dev/null
  wait "$tail_pid" 2>/dev/null
  rm -f "$progress_fifo" "$progress_file" "$done_marker" "$auth_marker" "$abort_marker" "$error_marker"
  saved_log="$(gui_save_failure_log)"
  wor_play_result_sound failure
  wor_show_result_notification failure
  yad "${yadflags[@]}" --text="Flashing was stopped before it finished.\n\n$DEVICE is now in an unusable state and has to be flashed again before it can boot.\n\nFull log: $saved_log"
  exit 1
fi

exitcode="$(cat "$done_marker" 2>/dev/null)"
[ -z "$exitcode" ] && exitcode=1

kill "$tail_pid" "$yad_pid" 2>/dev/null
wait "$tail_pid" "$yad_pid" 2>/dev/null
rm -f "$progress_fifo" "$progress_file" "$done_marker" "$auth_marker" "$abort_marker"

#clear zram - avoid leaving files occupying space in /zram
if [ "$DL_DIR" == /zram ];then
  sudo zram.sh &>/dev/null
fi

if [ "$exitcode" == 0 ];then
  rm -f "$output_log" "$error_marker"
  wor_play_result_sound success
  wor_show_result_notification success
  #display "next steps" window
  linux_completion_image="$(wor_yad_image_for_screen "$WOR_ASSETS_DIR/next-steps.png" "$WOR_LOGO_PATH" 730 440)"
  yad --center --width="$(wor_yad_width 690)" --height="$(wor_yad_height 380)" --window-icon="$WOR_LOGO_PATH" --class="$WOR_ICON_NAME" --title="$WOR_WINDOW_TITLE" \
    --form --align=center --image-on-top --buttons-layout=center --image="$linux_completion_image" \
    --field="It is now safe to remove your USB drive.":LBL '' --button=Close:0 >/dev/null
else
  #keep the log on failure; the dialog only shows a tail, and the GUI has no terminal to fall back on
  saved_log="$(gui_save_failure_log)"
  wor_play_result_sound failure
  wor_show_result_notification failure
  if installer_showed_own_error ;then
    : #install-wor.sh already displayed its own native error dialog.
  else
    yad "${yadflags[@]}" --text="The Windows on Raspberry script stopped unexpectedly (exit code $exitcode).\n\n$(gui_log_tail "$saved_log")\n\nFull log: $saved_log"
  fi
  rm -f "$error_marker"
fi

echo "install-wor.sh has finished."

#if downloading to ram, empty it now
if [ "$DL_DIR" == /zram ] && [ -d /zram/peinstaller ];then
  sudo zram.sh &>/dev/null
fi
