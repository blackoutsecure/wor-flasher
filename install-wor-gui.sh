#!/bin/bash

#Written by Botspot
#This script is a GUI front-end for the install-wor.sh script

#shared app title, used for window titles and dialog titles across this file and install-wor.sh
: "${WOR_APP_TITLE:=Windows on Raspberry}"

export RUN_MODE=gui #this variable is detected by install-wor.sh to display gui error messages

#Determine the directory that contains this script
[ -z "$DIRECTORY" ] && DIRECTORY="$(cd "$(dirname "$0")" && pwd -P)"

#This script and cli-based install-wor.sh must be in the same directory. Source it before anything else
#so every shared behaviour - error dialogs, status output, drive detection, the settings summary - comes
#from the installer itself and can never drift out of step with what actually gets flashed.
#Nothing above this point may call a shared function, so failures here are reported by hand.
cli_script="$DIRECTORY/install-wor.sh"
if [ ! -d "$DIRECTORY" ] || [ ! -f "$cli_script" ];then
  printf '\033[91m%b\033[0m\n' "No script found named install-wor.sh\nBoth scripts must be in the same directory." 1>&2
  exit 1
fi
#shellcheck disable=SC1090
source "$cli_script" source #by sourcing, this script checks for and applies updates.

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

gui_start_installer() { #Starts install-wor.sh in the background. Sets error_marker, output_log, progress_file, done_marker and installer_pid.
  error_marker="$(mktemp)" || error "Failed to create a GUI error marker."
  output_log="$(mktemp)" || error "Failed to create an install log."
  progress_file="$(mktemp)" || error "Failed to create a progress file."
  done_marker="$(mktemp -u)"
  #start with a clean marker; the installer creates it only if an error occurs
  rm -f "$error_marker"

  #No separate terminal is spawned, so the installer inherits this process's exported environment directly.
  export_installer_settings
  export WOR_GUI_ERROR_MARKER="$error_marker"
  export WOR_GUI_PROGRESS_FILE="$progress_file"

  #the job records its own status: a subshell cannot wait on a sibling, so waiting there returned 127 at once
  { "$cli_script" > "$output_log" 2>&1; echo $? > "$done_marker"; } &
  installer_pid=$!
}

installer_showed_own_error() { #Exit 0 if install-wor.sh already displayed its own native error dialog.
  #the marker has to exist and be non-empty: an empty file means the write never actually landed
  [ -e "$error_marker" ] && [ -s "$error_marker" ]
}

gui_save_failure_log() { #Output: where the installer log was kept. The dialog only shows a tail, and the GUI has no terminal to fall back on.
  local saved_log="$DL_DIR/last-run.log"
  mkdir -p "$DL_DIR" 2>/dev/null
  mv "$output_log" "$saved_log" 2>/dev/null || saved_log="$output_log"
  echo "Installer log saved to $saved_log" 1>&2
  echo "$saved_log"
}

gui_log_tail() { #Input: log path. Output: the last lines, with terminal escapes and carriage returns removed.
  sed 's/\x1b\[[0-9;]*[A-Za-z]//g; s/\r//g' "$1" | tail -n 18
}

loading_dialog() { #display a dialog to say something is loading
  local dialog_pid
  (echo '# ' ; sleep infinity) | yad "${yadflags[@]}" --height=0 \
    --progress --pulsate --title="$1" --text="$1" --no-buttons &
  dialog_pid=$!
  trap 'kill "$dialog_pid" 2>/dev/null' EXIT

  sleep infinity
}

stop_loader() {
  [ -z "${loader_pid:-}" ] || kill "$loader_pid" 2>/dev/null
  loader_pid=''
}

macos_choose() { #Input: newline-separated choices, prompt, default, cancel/back label, optional action label/value, optional image path/next label/icon path/title/cancel value. Output: selected choice or action value.
  local result
  result="$(osascript -l JavaScript - "$1" "$2" "$3" "${4:-Cancel}" "${5:-}" "${6:-}" "${7:-}" "${8:-Next}" "${9:-$DIRECTORY/logo.png}" "${10:-$WOR_APP_TITLE}" "${11:-}" <<'JXA'
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
const appTitle = ObjC.unwrap(args.objectAtIndex(13))
const cancelValue = ObjC.unwrap(args.objectAtIndex(14))
//a message screen has no selectable rows; it may still show an image (e.g. the welcome screen)
const isMessageMode = choices.length === 0

function writeResult(value) {
  const data = $(value + '\n').dataUsingEncoding($.NSUTF8StringEncoding)
  $.NSFileHandle.fileHandleWithStandardOutput.writeData(data)
}

if (ObjC.unwrap($.NSProcessInfo.processInfo.environment.objectForKey('WOR_CHOOSER_TEST') || '') === '1') {
  writeResult(defaultChoice || choices[0])
  $.NSApplication.sharedApplication.terminate(null)
}

const app = $.NSApplication.sharedApplication
$.NSProcessInfo.processInfo.processName = appTitle
app.setActivationPolicy($.NSApplicationActivationPolicyRegular)
if (iconPath.length > 0) {
  app.setApplicationIconImage($.NSImage.alloc.initWithContentsOfFile($(iconPath)))
}
let tableView
let window
let selectedValue = null
//set below, before the modal runs; read by linkClicked: when the user clicks the link button
let linkMatch = null

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
    'linkClicked:': {
      types: ['void', ['id']],
      implementation: function() {
        if (!linkMatch) return
        const url = linkMatch[0]
        const pasteboard = $.NSPasteboard.generalPasteboard
        pasteboard.clearContents
        pasteboard.setStringForType($(url), $.NSPasteboardTypeString)
        $.NSWorkspace.sharedWorkspace.openURL($.NSURL.URLWithString($(url)))
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

const controller = $.WorChooserController.alloc.init
app.setDelegate(controller)
const width = 760
const height = 500
const style = $.NSWindowStyleMaskTitled | $.NSWindowStyleMaskClosable | $.NSWindowStyleMaskResizable
window = $.NSWindow.alloc.initWithContentRectStyleMaskBackingDefer($.NSMakeRect(0, 0, width, height), style, $.NSBackingStoreBuffered, false)
window.title = appTitle
window.minSize = $.NSMakeSize(640, 260)
window.setDelegate(controller)
window.center

const content = $.NSView.alloc.initWithFrame($.NSMakeRect(0, 0, width, height))
content.autoresizingMask = $.NSViewWidthSizable | $.NSViewHeightSizable
window.contentView = content

const prompt = $.NSTextField.labelWithString(promptText)
prompt.font = $.NSFont.systemFontOfSizeWeight(14, $.NSFontWeightMedium)
prompt.autoresizingMask = $.NSViewWidthSizable | $.NSViewMinYMargin
let linkButton
if (isMessageMode) {
  prompt.setUsesSingleLineMode(false)
  prompt.cell.setWraps(true)
  prompt.cell.setScrollable(false)
  //if the prompt mentions a URL, add a clickable button below it that opens the browser and copies the link
  linkMatch = promptText.match(/https?:\/\/\S+/)
  if (imagePath.length > 0) {
    prompt.frame = $.NSMakeRect(20, 76, width - 40, 44)
    const imageView = $.NSImageView.alloc.initWithFrame($.NSMakeRect(20, 140, width - 40, 320))
    imageView.image = $.NSImage.alloc.initWithContentsOfFile($(imagePath))
    imageView.imageScaling = $.NSImageScaleProportionallyUpOrDown
    imageView.autoresizingMask = $.NSViewMaxXMargin | $.NSViewMinYMargin
    content.addSubview(imageView)
  } else if (linkMatch) {
    prompt.frame = $.NSMakeRect(20, 96, width - 40, height - 132)
  } else {
    prompt.frame = $.NSMakeRect(20, 72, width - 40, height - 112)
  }
  if (linkMatch) {
    linkButton = $.NSButton.buttonWithTitleTargetAction('Open ' + linkMatch[0], controller, 'linkClicked:')
    linkButton.bezelStyle = $.NSBezelStyleInline
    linkButton.setBordered(false)
    linkButton.contentTintColor = $.NSColor.linkColor
    linkButton.sizeToFit
    linkButton.frame = $.NSMakeRect(20, 54, width - 40, 18)
    linkButton.autoresizingMask = $.NSViewWidthSizable | $.NSViewMinYMargin
  }
  content.addSubview(prompt)
  if (linkButton) content.addSubview(linkButton)
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

//buttons are sized to their own text so any label (e.g. "Proceed with WoR-Flasher") fits without truncation
const nextButton = $.NSButton.buttonWithTitleTargetAction(nextLabel, controller, 'nextClicked:')
nextButton.bezelStyle = $.NSBezelStyleRounded
nextButton.keyEquivalent = '\r'
nextButton.sizeToFit
let nextWidth = Math.max(96, nextButton.frame.size.width)
nextButton.frame = $.NSMakeRect(width - 20 - nextWidth, 22, nextWidth, 32)
nextButton.autoresizingMask = $.NSViewMinXMargin | $.NSViewMaxYMargin
content.addSubview(nextButton)

//a message screen only hides Cancel for the image-based welcome screen; an explicitly empty cancelLabel hides it everywhere else
const showCancelButton = cancelLabel.length > 0 && !(isMessageMode && imagePath.length > 0)
if (showCancelButton) {
  const cancelButton = $.NSButton.buttonWithTitleTargetAction(cancelLabel, controller, 'cancelClicked:')
  cancelButton.bezelStyle = $.NSBezelStyleRounded
  cancelButton.keyEquivalent = '\u001b'
  cancelButton.sizeToFit
  const cancelWidth = Math.max(96, cancelButton.frame.size.width)
  cancelButton.frame = $.NSMakeRect(width - 20 - nextWidth - 8 - cancelWidth, 22, cancelWidth, 32)
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
//the Dock Quit item sends an aevt/quit Apple Event that a modal session would otherwise never see
$.NSAppleEventManager.sharedAppleEventManager.setEventHandlerAndSelectorForEventClassAndEventID(controller, 'handleQuitEvent:withReplyEvent:', 0x61657674, 0x71756974)
$.NSAppleEventManager.sharedAppleEventManager.setEventHandlerAndSelectorForEventClassAndEventID(controller, 'handleReopenEvent:withReplyEvent:', 0x61657674, 0x72617070)
const pumpTimer = $.NSTimer.timerWithTimeIntervalTargetSelectorUserInfoRepeats(0.25, controller, 'pumpEvents:', $(), true)
$.NSRunLoop.currentRunLoop.addTimerForMode(pumpTimer, $.NSModalPanelRunLoopMode)
app.runModalForWindow(window)

if (selectedValue === null) {
  writeResult('__WOR_CANCEL__')
  app.terminate(null)
} else {
  writeResult(selectedValue)
  app.terminate(null)
}
JXA
)"
  if [ "$result" == __WOR_CANCEL__ ];then
    return 1
  fi
  printf '%s\n' "$result"
}

macos_show_announcement() { #Output: proceed or bvm.
  local announcement_choices announcement_choice
  announcement_choices=''
  announcement_choice="$(macos_choose "$announcement_choices" 'Consider BVM for running Windows alongside Linux on Raspberry Pi. Learn more: https://github.com/Botspot/bvm' 'Proceed with WoR-Flasher' Cancel 'Check out BVM' 'Check out BVM' "$DIRECTORY/announcement.png" 'Proceed with WoR-Flasher' "$DIRECTORY/logo.png")" || return 1

  case "$announcement_choice" in
    'Proceed with WoR-Flasher' | 'Check out BVM') echo "$announcement_choice" ;;
    *) return 1 ;;
  esac
}

macos_choose_device() { #Input: newline-separated detected volume rows. Output: selected row or __REFRESH__.
  if [ -z "$1" ];then
    macos_choose '' 'No external, physical, writable drive was found. Connect a removable drive, then click Refresh.' __REFRESH__ Back '' '' '' Refresh >/dev/null 2>&1
    echo "__REFRESH__"
  else
    device_choice="$(macos_choose "$1" 'Choose the external drive and volumes to erase' "$(printf '%s\n' "$1" | head -n1)" Back Refresh __REFRESH__)" || return 1
    echo "$device_choice"
  fi
}

macos_advanced_options() { #Reads/updates OOBE_NETWORK_BYPASS, PI4_AUTO_DISABLE_3GB, UEFI_USE_LATEST, DRIVERS_USE_LATEST, SKIP_IMAGE_VERIFICATION, DRY_RUN, APPLY_CUSTOM_CONFIG_TXT, USE_CACHE, CONFIG_TXT.
  local advanced_jxa checkbox_spec result status line i uefi_pinned pi4_applicable pi4_label config_scope
  uefi_pinned="$(uefi_pinned_version)"
  #the engine ignores PI4_AUTO_DISABLE_3GB unless RPI_MODEL is 4, so don't offer it as a live choice elsewhere
  [ "$RPI_MODEL" == 4 ] && pi4_applicable=1 || pi4_applicable=0
  if [ "$pi4_applicable" == 1 ];then
    pi4_label='Automatically disable the Pi 4 3 GB RAM limit after install'
  else
    pi4_label="Automatically disable the Pi 4 3 GB RAM limit after install (not applicable to the Pi $RPI_MODEL)"
  fi
  #in recovery mode this config.txt boots the installer media; WoR-PE writes the target drive's own copy
  [ "$CAN_INSTALL_ON_SAME_DRIVE" == 1 ] && config_scope='applied to the boot partition' || config_scope='applied to the recovery media, not the installed Windows drive'
  checkbox_spec="Allow Windows setup to continue without a network connection	$OOBE_NETWORK_BYPASS	1
$pi4_label	$([ "$pi4_applicable" == 1 ] && echo "$PI4_AUTO_DISABLE_3GB" || echo 0)	$pi4_applicable
Use the latest UEFI firmware instead of the tested pinned version ($uefi_pinned)	$UEFI_USE_LATEST	1
Use the latest Windows ARM64 drivers instead of the pinned version ($DRIVER_VER)	$DRIVERS_USE_LATEST	1
Skip verifying the written image after flashing (not recommended)	$SKIP_IMAGE_VERIFICATION	1
Skip flashing the device (dry run)	$DRY_RUN	1"

  advanced_jxa="$(cat <<'JXA'
ObjC.import('AppKit')
ObjC.import('Foundation')
ObjC.import('stdlib')
const args = $.NSProcessInfo.processInfo.arguments
const checkboxSpec = ObjC.unwrap(args.objectAtIndex(4))
const configTxtDefault = ObjC.unwrap(args.objectAtIndex(5))
const applyConfigDefault = ObjC.unwrap(args.objectAtIndex(6))
const iconPath = ObjC.unwrap(args.objectAtIndex(7))
const appTitle = ObjC.unwrap(args.objectAtIndex(8))
const configScope = ObjC.unwrap(args.objectAtIndex(9))
const cacheModeDefault = ObjC.unwrap(args.objectAtIndex(10))

const rows = checkboxSpec.split('\n').map(function(line) {
  const parts = line.split('\t')
  return { label: parts[0], checked: parts[1] === '1', enabled: parts[2] !== '0' }
})

const app = $.NSApplication.sharedApplication
$.NSProcessInfo.processInfo.processName = appTitle
app.setActivationPolicy($.NSApplicationActivationPolicyRegular)
if (iconPath.length > 0) {
  app.setApplicationIconImage($.NSImage.alloc.initWithContentsOfFile($(iconPath)))
}

//without a menu bar, Cmd+C/V/X/A have no Edit menu item to route to and just beep
const mainMenu = $.NSMenu.alloc.init
const editMenuItem = $.NSMenuItem.alloc.init
mainMenu.addItem(editMenuItem)
const editMenu = $.NSMenu.alloc.initWithTitle('Edit')
editMenuItem.submenu = editMenu
editMenu.addItemWithTitleActionKeyEquivalent('Cut', 'cut:', 'x')
editMenu.addItemWithTitleActionKeyEquivalent('Copy', 'copy:', 'c')
editMenu.addItemWithTitleActionKeyEquivalent('Paste', 'paste:', 'v')
editMenu.addItemWithTitleActionKeyEquivalent('Select All', 'selectAll:', 'a')
app.mainMenu = mainMenu

let window, textView, scrollView, applyConfigCheckbox, cachePopup
let checkboxes = []
let confirmed = false

function updateConfigEditableState() {
  const enabled = applyConfigCheckbox.state == 1
  textView.editable = enabled
  scrollView.alphaValue = enabled ? 1.0 : 0.5
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

const width = 640
const rowHeight = 26
const height = (rows.length + 1) * rowHeight + 20 + 220 + 140 + rowHeight + 8
const style = $.NSWindowStyleMaskTitled | $.NSWindowStyleMaskClosable | $.NSWindowStyleMaskResizable
window = $.NSWindow.alloc.initWithContentRectStyleMaskBackingDefer($.NSMakeRect(0, 0, width, height), style, $.NSBackingStoreBuffered, false)
window.title = 'Advanced Options'
window.minSize = $.NSMakeSize(520, 420)
window.setDelegate(controller)
window.center

const content = $.NSView.alloc.initWithFrame($.NSMakeRect(0, 0, width, height))
content.autoresizingMask = $.NSViewWidthSizable | $.NSViewHeightSizable
window.contentView = content

let y = height - 40
for (let i = 0; i < rows.length; i++) {
  const checkbox = $.NSButton.checkboxWithTitleTargetAction(rows[i].label, undefined, undefined)
  checkbox.frame = $.NSMakeRect(20, y, width - 40, 20)
  checkbox.state = rows[i].checked ? 1 : 0
  checkbox.enabled = rows[i].enabled
  checkbox.autoresizingMask = $.NSViewWidthSizable | $.NSViewMinYMargin
  content.addSubview(checkbox)
  checkboxes.push(checkbox)
  y -= rowHeight
}

applyConfigCheckbox = $.NSButton.checkboxWithTitleTargetAction('Apply the customized config.txt below (unchecked uses the firmware\u2019s default config.txt)', controller, 'applyConfigToggled:')
applyConfigCheckbox.frame = $.NSMakeRect(20, y, width - 40, 20)
applyConfigCheckbox.state = applyConfigDefault === '1' ? 1 : 0
applyConfigCheckbox.autoresizingMask = $.NSViewWidthSizable | $.NSViewMinYMargin
content.addSubview(applyConfigCheckbox)
y -= rowHeight

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

const configLabel = $.NSTextField.labelWithString('config.txt (' + configScope + '):')
configLabel.font = $.NSFont.systemFontOfSizeWeight(12, $.NSFontWeightMedium)
configLabel.frame = $.NSMakeRect(20, y - 4, width - 40, 20)
configLabel.autoresizingMask = $.NSViewWidthSizable | $.NSViewMinYMargin
content.addSubview(configLabel)
y -= rowHeight

scrollView = $.NSScrollView.alloc.initWithFrame($.NSMakeRect(20, 60, width - 40, y - 60))
scrollView.autoresizingMask = $.NSViewWidthSizable | $.NSViewHeightSizable
scrollView.wantsLayer = true
scrollView.borderType = $.NSBezelBorder
scrollView.hasVerticalScroller = true
textView = $.NSTextView.alloc.initWithFrame(scrollView.bounds)
textView.font = $.NSFont.userFixedPitchFontOfSize(12)
textView.string = $(configTxtDefault)
textView.autoresizingMask = $.NSViewWidthSizable | $.NSViewHeightSizable
scrollView.documentView = textView
content.addSubview(scrollView)
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
//the Dock Quit item sends an aevt/quit Apple Event that a modal session would otherwise never see
$.NSAppleEventManager.sharedAppleEventManager.setEventHandlerAndSelectorForEventClassAndEventID(controller, 'handleQuitEvent:withReplyEvent:', 0x61657674, 0x71756974)
$.NSAppleEventManager.sharedAppleEventManager.setEventHandlerAndSelectorForEventClassAndEventID(controller, 'handleReopenEvent:withReplyEvent:', 0x61657674, 0x72617070)
const pumpTimer = $.NSTimer.timerWithTimeIntervalTargetSelectorUserInfoRepeats(0.25, controller, 'pumpEvents:', $(), true)
$.NSRunLoop.currentRunLoop.addTimerForMode(pumpTimer, $.NSModalPanelRunLoopMode)
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
out.push(applyConfigCheckbox.state == 1 ? '1' : '0')
out.push(String(cachePopup.indexOfSelectedItem))
out.push('---CONFIG_TXT---')
out.push(ObjC.unwrap(textView.string))
writeResult(out)
app.terminate(null)
JXA
)"

  result="$(osascript -l JavaScript - "$checkbox_spec" "$CONFIG_TXT" "$APPLY_CUSTOM_CONFIG_TXT" "$DIRECTORY/logo.png" "$WOR_APP_TITLE" "$config_scope" "$USE_CACHE" <<<"$advanced_jxa" 2>/dev/null)"
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
      7) APPLY_CUSTOM_CONFIG_TXT="$line" ;;
      #the popup reports its index, which lines up with USE_CACHE's 0/1/2
      8) [ "$line" == 0 ] || [ "$line" == 1 ] || [ "$line" == 2 ] && USE_CACHE="$line" ;;
    esac
  done < <(printf '%s\n' "$result" | sed -n '2,9p')

  CONFIG_TXT="$(printf '%s\n' "$result" | sed -n '/^---CONFIG_TXT---$/,$p' | tail -n +2)"
}

macos_start_cli() {
  local completion_jxa confirm_summary confirmation default_language device_choices device_capability device_choice done_marker abort_marker error_marker install_mode installer_pid installer_status language_choices mode_choices output_log pi_choices progress_file progress_jxa saved_log step windows_choices

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
        RPI_MODEL="$(macos_choose "$pi_choices" 'Choose Raspberry Pi model' '5' Back)" || { step=windows; continue; }
        list_bids 10 >/dev/null || error "Failed to retrieve available Windows versions."
        [ "$WINDOWS_VER" == 'Windows 11' ] && BID="$(get_bid 11)" || BID="$(get_bid 10)"
        [ -n "$BID" ] || error "No compatible Windows build is available for Raspberry Pi $RPI_MODEL."
        set_default_config_txt
        step=language
        ;;
      language)
        language_choices="$(list_langs_preferred | cut -d: -f1)"
        default_language="$(default_win_lang)"
        WIN_LANG="$(macos_choose "$language_choices" "Choose Windows language (default: $default_language)" "$default_language" Back)" || { step=pi; continue; }
        step=device
        ;;
      device)
        device_choices="$(darwin_list_device_choices)"
        device_choice="$(macos_choose_device "$device_choices")" || { step=language; continue; }
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
          install_mode="$(macos_choose "$mode_choices" 'Choose installation mode' 'Install Windows onto this drive' Back)" || { step=device; continue; }
          [ "$install_mode" == 'Install Windows onto this drive' ] && CAN_INSTALL_ON_SAME_DRIVE=1 || CAN_INSTALL_ON_SAME_DRIVE=0
        fi
        step=confirm
        ;;
      confirm)
        confirm_summary="$(settings_summary_plain)

All data on the target drive will be erased.

To continue, click Flash. To review or change these settings, click Advanced. To cancel, click Back or close this window."
        confirmation="$(macos_choose '' "$confirm_summary" Flash Back 'Advanced...' Advanced '' Flash "$DIRECTORY/logo.png" "$WOR_APP_TITLE" Back)" || confirmation=Cancel
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

  completion_jxa="$(cat <<'JXA'
ObjC.import('AppKit')
ObjC.import('stdlib')
const args = $.NSProcessInfo.processInfo.arguments
const message = ObjC.unwrap(args.objectAtIndex(4))
const iconPath = ObjC.unwrap(args.objectAtIndex(5))
const appTitle = ObjC.unwrap(args.objectAtIndex(6))

const app = $.NSApplication.sharedApplication
$.NSProcessInfo.processInfo.processName = appTitle
app.setActivationPolicy($.NSApplicationActivationPolicyRegular)
if (iconPath.length > 0) {
  app.setApplicationIconImage($.NSImage.alloc.initWithContentsOfFile($(iconPath)))
}

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

const width = 600
const height = 240
const style = $.NSWindowStyleMaskTitled | $.NSWindowStyleMaskClosable
window = $.NSWindow.alloc.initWithContentRectStyleMaskBackingDefer($.NSMakeRect(0, 0, width, height), style, $.NSBackingStoreBuffered, false)
window.title = appTitle
window.setDelegate(controller)
window.center

const content = $.NSView.alloc.initWithFrame($.NSMakeRect(0, 0, width, height))
window.contentView = content

const label = $.NSTextField.labelWithString(message)
label.frame = $.NSMakeRect(20, 70, width - 40, height - 100)
label.font = $.NSFont.systemFontOfSizeWeight(14, $.NSFontWeightMedium)
label.setUsesSingleLineMode(false)
label.cell.setWraps(true)
label.cell.setScrollable(false)
content.addSubview(label)

//extract the log file path if present (after "Full log: ") and create clickable buttons
let logPath = ''
const logMatch = message.match(/Full log: (.+)$/)
if (logMatch) {
  logPath = logMatch[1].trim()
}

let okButton, openButton, copyButton
const buttonGap = 8
if (logPath.length > 0) {
  openButton = $.NSButton.buttonWithTitleTargetAction('Open Log', controller, 'openLogClicked:')
  openButton.bezelStyle = $.NSBezelStyleRounded
  openButton.sizeToFit
  const openWidth = Math.max(96, openButton.frame.size.width)
  openButton.frame = $.NSMakeRect(20, 22, openWidth, 32)
  content.addSubview(openButton)

  copyButton = $.NSButton.buttonWithTitleTargetAction('Copy', controller, 'copyLogClicked:')
  copyButton.bezelStyle = $.NSBezelStyleRounded
  copyButton.sizeToFit
  const copyWidth = Math.max(80, copyButton.frame.size.width)
  copyButton.frame = $.NSMakeRect(20 + openWidth + buttonGap, 22, copyWidth, 32)
  content.addSubview(copyButton)

  okButton = $.NSButton.buttonWithTitleTargetAction('OK', controller, 'okClicked:')
  okButton.bezelStyle = $.NSBezelStyleRounded
  okButton.keyEquivalent = '\r'
  okButton.sizeToFit
  const okWidth = Math.max(96, okButton.frame.size.width)
  okButton.frame = $.NSMakeRect(width - 20 - okWidth, 22, okWidth, 32)
  content.addSubview(okButton)
} else {
  okButton = $.NSButton.buttonWithTitleTargetAction('OK', controller, 'okClicked:')
  okButton.bezelStyle = $.NSBezelStyleRounded
  okButton.keyEquivalent = '\r'
  okButton.sizeToFit
  const okWidth = Math.max(96, okButton.frame.size.width)
  okButton.frame = $.NSMakeRect(width - 20 - okWidth, 22, okWidth, 32)
  content.addSubview(okButton)
}

window.makeKeyAndOrderFront(null)
if (!app.isActive) app.requestUserAttention($.NSInformationalRequest)
app.activateIgnoringOtherApps(true)
//the Dock Quit item sends an aevt/quit Apple Event that a modal session would otherwise never see
$.NSAppleEventManager.sharedAppleEventManager.setEventHandlerAndSelectorForEventClassAndEventID(controller, 'handleQuitEvent:withReplyEvent:', 0x61657674, 0x71756974)
$.NSAppleEventManager.sharedAppleEventManager.setEventHandlerAndSelectorForEventClassAndEventID(controller, 'handleReopenEvent:withReplyEvent:', 0x61657674, 0x72617070)
const pumpTimer = $.NSTimer.timerWithTimeIntervalTargetSelectorUserInfoRepeats(0.25, controller, 'pumpEvents:', $(), true)
$.NSRunLoop.currentRunLoop.addTimerForMode(pumpTimer, $.NSModalPanelRunLoopMode)
app.runModalForWindow(window)
app.terminate(null)
JXA
)"

  progress_jxa="$(cat <<'JXA'
ObjC.import('AppKit')
ObjC.import('Foundation')
ObjC.import('stdlib')
const args = $.NSProcessInfo.processInfo.arguments
const progressFile = ObjC.unwrap(args.objectAtIndex(4))
const doneMarker = ObjC.unwrap(args.objectAtIndex(5))
const iconPath = ObjC.unwrap(args.objectAtIndex(6))
const appTitle = ObjC.unwrap(args.objectAtIndex(7))
const abortMarker = ObjC.unwrap(args.objectAtIndex(8))

const app = $.NSApplication.sharedApplication
$.NSProcessInfo.processInfo.processName = appTitle
app.setActivationPolicy($.NSApplicationActivationPolicyRegular)
if (iconPath.length > 0) {
  app.setApplicationIconImage($.NSImage.alloc.initWithContentsOfFile($(iconPath)))
}

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

let window, bar, phaseLabel, detailLabel, stepLabel

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
          const statusLine = lastMatch(lines, 'STATUS\t')
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
              //count in hundredths of a step so progress inside a step is visible too
              bar.indeterminate = false
              bar.minValue = 0
              bar.maxValue = stepTotal * 100
              bar.doubleValue = (stepNum - 1) * 100 + within
            }
            phaseLabel.stringValue = parts.slice(3).join('\t')
            if (!isNaN(stepNum) && !isNaN(stepTotal) && stepTotal > 0) {
              stepLabel.stringValue = 'Step ' + stepNum + ' of ' + stepTotal
            }
          } else if (subLine.length > 0) {
            //work before the first numbered step, such as clearing the cache, still has its own percentage
            const parsed = parseInt(subLine.split('\t')[1], 10)
            if (!isNaN(parsed)) {
              bar.indeterminate = false
              bar.minValue = 0
              bar.maxValue = 100
              bar.doubleValue = Math.max(0, Math.min(100, parsed))
            }
          }
          if (statusLine.length > 0) {
            detailLabel.stringValue = statusLine.split('\t').slice(1).join('\t')
          }
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

const width = 560
const height = 236
const style = $.NSWindowStyleMaskTitled | $.NSWindowStyleMaskClosable | $.NSWindowStyleMaskMiniaturizable
window = $.NSWindow.alloc.initWithContentRectStyleMaskBackingDefer($.NSMakeRect(0, 0, width, height), style, $.NSBackingStoreBuffered, false)
window.title = appTitle
window.setDelegate(controller)
window.center

const content = $.NSView.alloc.initWithFrame($.NSMakeRect(0, 0, width, height))
window.contentView = content

phaseLabel = $.NSTextField.labelWithString('Starting...')
phaseLabel.frame = $.NSMakeRect(20, 166, width - 140, 24)
phaseLabel.font = $.NSFont.systemFontOfSizeWeight(14, $.NSFontWeightBold)
content.addSubview(phaseLabel)

stepLabel = $.NSTextField.labelWithString('')
stepLabel.frame = $.NSMakeRect(width - 120, 166, 100, 24)
stepLabel.font = $.NSFont.systemFontOfSizeWeight(12, $.NSFontWeightRegular)
stepLabel.alignment = $.NSTextAlignmentRight
content.addSubview(stepLabel)

detailLabel = $.NSTextField.labelWithString('')
detailLabel.frame = $.NSMakeRect(20, 136, width - 40, 24)
detailLabel.font = $.NSFont.systemFontOfSizeWeight(12, $.NSFontWeightRegular)
content.addSubview(detailLabel)

bar = $.NSProgressIndicator.alloc.initWithFrame($.NSMakeRect(20, 106, width - 40, 20))
bar.indeterminate = true
bar.startAnimation(null)
content.addSubview(bar)

const noteLabel = $.NSTextField.labelWithString('This window will close automatically when the process finishes.')
noteLabel.frame = $.NSMakeRect(20, 74, width - 40, 20)
noteLabel.font = $.NSFont.systemFontOfSizeWeight(11, $.NSFontWeightRegular)
content.addSubview(noteLabel)

const abortButton = $.NSButton.buttonWithTitleTargetAction('Abort', controller, 'abortClicked:')
abortButton.bezelStyle = $.NSBezelStyleRounded
abortButton.sizeToFit
const abortWidth = Math.max(96, abortButton.frame.size.width)
abortButton.frame = $.NSMakeRect(width - 20 - abortWidth, 22, abortWidth, 32)
content.addSubview(abortButton)

window.makeKeyAndOrderFront(null)
app.activateIgnoringOtherApps(true)

const timer = $.NSTimer.scheduledTimerWithTimeIntervalTargetSelectorUserInfoRepeats(0.4, controller, 'tick:', undefined, true)
//the Dock Quit item sends an aevt/quit Apple Event that a modal session would otherwise never see
$.NSAppleEventManager.sharedAppleEventManager.setEventHandlerAndSelectorForEventClassAndEventID(controller, 'handleQuitEvent:withReplyEvent:', 0x61657674, 0x71756974)
$.NSAppleEventManager.sharedAppleEventManager.setEventHandlerAndSelectorForEventClassAndEventID(controller, 'handleReopenEvent:withReplyEvent:', 0x61657674, 0x72617070)
const pumpTimer = $.NSTimer.timerWithTimeIntervalTargetSelectorUserInfoRepeats(0.25, controller, 'pumpEvents:', $(), true)
$.NSRunLoop.currentRunLoop.addTimerForMode(pumpTimer, $.NSModalPanelRunLoopMode)
app.runModalForWindow(window)
timer.invalidate
window.orderOut(null)
JXA
)"

  abort_marker="$(mktemp -u)"

  #authenticate first: the progress window holds the front, and a password dialog cannot get past it
  if [ "$DRY_RUN" != 1 ] && ! sudo -v ;then
    error "Administrator authentication failed or was canceled. Enter the correct macOS password and try again."
  fi

  gui_start_installer
  #a flash can outlast the sudo timestamp, and by then no dialog could reach the front to renew it
  ( while kill -0 "$installer_pid" 2>/dev/null ;do command sudo -n -v >/dev/null 2>&1; sleep 30; done ) &

  osascript -l JavaScript - "$progress_file" "$done_marker" "$DIRECTORY/logo.png" "$WOR_APP_TITLE" "$abort_marker" <<<"$progress_jxa" >/dev/null 2>&1

  if [ -e "$abort_marker" ];then
    status "Aborting at your request"
    #most of the work runs under sudo, so the tree has to come down with the credential we already hold
    kill_process_tree "$installer_pid"
    wait "$installer_pid" 2>/dev/null
    rm -f "$progress_file" "$done_marker" "$abort_marker" "$error_marker"
    saved_log="$(gui_save_failure_log)"
    osascript -l JavaScript - "Flashing was stopped before it finished.

$DEVICE is now in an unusable state and has to be flashed again before it can boot.

Full log: $saved_log" "$DIRECTORY/logo.png" "$WOR_APP_TITLE" <<<"$completion_jxa" >/dev/null 2>&1
    exit 1
  fi

  #the window can disappear while the flash is still going; wait for the real status instead of
  #assuming failure, which would report a bogus error and leave the flash running unattended
  wait "$installer_pid" 2>/dev/null

  installer_status="$(cat "$done_marker" 2>/dev/null)"
  [ -z "$installer_status" ] && installer_status=1
  rm -f "$progress_file" "$done_marker" "$abort_marker"

  if [ "$installer_status" == 0 ];then
    rm -f "$output_log" "$error_marker"
    completion_text="Process completed successfully."
  else
    #keep the log on failure; the dialog only shows a tail, and the GUI has no terminal to fall back on
    saved_log="$(gui_save_failure_log)"
    #installer writes the error_marker before showing its own error dialog; if it exists, skip the completion dialog
    if installer_showed_own_error ;then
      rm -f "$error_marker"
      exit "$installer_status"
    fi
    completion_text="The Windows on Raspberry script stopped unexpectedly (exit code $installer_status).

$(gui_log_tail "$saved_log")

Full log: $saved_log"
  fi
  osascript -l JavaScript - "$completion_text" "$DIRECTORY/logo.png" "$WOR_APP_TITLE" <<<"$completion_jxa" >/dev/null 2>&1
  exit "$installer_status"
}

if is_macos ;then
  setup || exit 1
  announcement_choice="$(macos_show_announcement)" || exit 0
  if [ "$announcement_choice" == 'Check out BVM' ];then
    open_url 'https://github.com/Botspot/bvm'
    exit 0
  fi
  macos_start_cli
  exit $?
fi

#run safety checks and install packages
setup || exit 1

#this array stores flags that are used in all yad windows - saves on the typing and makes it easy to change an attribute on all dialogs from one place.
yadflags=(--center --width=400 --height=250 --window-icon="$DIRECTORY/logo.png" --title="$WOR_APP_TITLE" --separator='\n')

#display BVM announcement
yad "${yadflags[@]}" --buttons-layout=spread \
    --image="$DIRECTORY/announcement.png" \
    --button='<b>Proceed with WoR-Flasher</b>':1 \
    --button='<b>Check out the BVM project</b>':0
if [ "$?" == 0 ];then
  open_url 'https://github.com/Botspot/bvm'
  exit 0
fi

{ #choose destination RPi model and windows build ID
if [ -z "$RPI_MODEL" ] || [ -z "$BID" ];then
  output="$(yad "${yadflags[@]}" --height=0 --form --columns=2 \
    --image="$DIRECTORY/logo-full.png" \
    --text=$'<big><b>Welcome to Windows on Raspberry!</b></big>\nThis wizard will help you easily install the full desktop version of Windows on your Raspberry Pi computer.' \
    --field="Install":CB "Windows 11!Windows 10!More options" \
    --field="on a":CB "Pi5!Pi4/Pi400!Pi3/Pi2_v1.2" \
    --button='<b>Next</b>':0)"
  button=$?
  [ $button != 0 ] && exit 0

  WINDOWS_VER="$(echo "$output" | sed -n 1p)"
  RPI_MODEL="$(echo "$output" | sed -n 2p | sed 's+Pi5+5+g' | sed 's+Pi4/Pi400+4+g' | sed 's+Pi3/Pi2_v1.2+3+g')"

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
FALSE\nUse a cached version of Windows from a previous run\nuse cached" | yad "${yadflags[@]}" --width=420 \
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
              list_bids_supported 10 | sed 's/^/Windows 10 /g')" | sed 's/^/FALSE\n/g' | yad "${yadflags[@]}" --width=420 \
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
            SOURCE_FILE="$(yad "${yadflags[@]}" --width=420 \
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

              folder="$(echo -ne "$list" | yad "${yadflags[@]}" --height=320 \
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
                    --width=500 --height=400 --title="Choose DL_DIR" \
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

  #en-us is listed first so it is the preselected row
  LANG_LIST="$(list_langs_preferred)"

  while true; do
    WIN_LANG="$(echo "$LANG_LIST" | sed 's/^/FALSE:/g' | tr ':' '\n' | sed -e '0,/FALSE/ s/FALSE/TRUE/' | yad "${yadflags[@]}" \
      --list --radiolist --column=chk:CHK --column=short --column=long --no-headers --print-column=2 --no-selection \
      --text=$'<big><b>Language</b></big>\nChoose language for Windows:' \
      --button='<b>Next</b>':0)"
    button=$?
    [ $button != 0 ] && exit 1

    if is_known_win_lang "$WIN_LANG" ;then
      break
    fi
  done
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

    DEVICE="$(echo -n "$DEV_LIST" | sed -e '0,/FALSE/ s/FALSE/TRUE/' | yad "${yadflags[@]}" --text='Choose device to flash:' --width=420 \
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
    install_mode="$(echo -e "TRUE\ninstall\nInstallation drive\nInstall Windows onto this 25 GB+ drive\nFALSE\nrecovery\nRecovery drive\nInstall Windows onto another >16 GB drive" | yad "${yadflags[@]}" --width=520 \
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

      yad "${yadflags[@]}" --width=500 --form --field="About 4.2GB of files need to be downloaded to system storage before flashing can begin.
But your system has $(echo "scale=1 ; $( awk '/MemTotal/ {print $2}' /proc/meminfo ) / 1048576 " | bc )GB of RAM. Everything can be downloaded to RAM if you prefer.
Choose this if:
- You don't have enough space in $HOME
- You want your system storage to last as long as possible
- You don't plan to use WoR-Flasher often:LBL" \
        --image="$DIRECTORY/ram.png" --image-on-top \
        --button="Use ${DL_DIR}":2 \
        --button="<b>Use RAM</b>!!${tooltip}":0
      button=$?

      if [ "$button" == 0 ];then
        status "User chose to download everything to RAM."
        echo "For best results, please close all other programs. (especially web browsers and games)"
        yad "${yadflags[@]}" --width=500 --image="$DIRECTORY/ram.png" --image-on-top \
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
            "$(dirname "$(cat /usr/local/bin/pi-apps | sed -n 2p)")/manage"
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

  output="$(yad "${yadflags[@]}" --width=500 --height=400 --image="$DIRECTORY/overview.png" --image-on-top \
    --form --field="$window_text":LBL '' \
    "${existing_img_chk[@]}" \
    --field="<b>Edit config.txt:</b>     <small><a href=\"https://www.raspberrypi.com/documentation/computers/config_txt.html\">Configuration reference</a></small>":TXT "$CONFIG_TXT" \
    --field="$deletion_warning":LBL '' \
    --button='<b>Advanced...</b>'!!"More settings, intended for the advanced user or for troubleshooting":2 \
    --button='<b>Flash</b>'!!"$deletion_warning_2":0
  )"
  button=$?

  #remove first line from yad output - remove newline from label field
  output="$(echo -e "$output" | tail -n +2)"

  CONFIG_TXT="$output"

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
      fields+=("--field=Skip flashing the device (DRY_RUN)":CHK "$(echo "$DRY_RUN" | sed 's/1/TRUE/g' | sed 's/0/FALSE/g')")

      #make entries for the customization toggles
      #the engine ignores PI4_AUTO_DISABLE_3GB unless RPI_MODEL is 4; yad can't disable one field, so mark it and drop the value below
      [ "$RPI_MODEL" == 4 ] && pi4_applicable=1 || pi4_applicable=0
      if [ "$pi4_applicable" == 1 ];then
        pi4_label='Automatically disable the Pi 4 3 GB RAM limit after install'
      else
        pi4_label="<i>Automatically disable the Pi 4 3 GB RAM limit after install (not applicable to the Pi $RPI_MODEL)</i>"
      fi
      fields+=("--field=Allow Windows setup to continue without a network connection":CHK "$(echo "$OOBE_NETWORK_BYPASS" | sed 's/1/TRUE/g' | sed 's/0/FALSE/g')")
      fields+=("--field=$pi4_label":CHK "$(echo "$([ "$pi4_applicable" == 1 ] && echo "$PI4_AUTO_DISABLE_3GB" || echo 0)" | sed 's/1/TRUE/g' | sed 's/0/FALSE/g')")
      fields+=("--field=Use the latest UEFI firmware instead of the tested pinned version ($uefi_pinned)":CHK "$(echo "$UEFI_USE_LATEST" | sed 's/1/TRUE/g' | sed 's/0/FALSE/g')")
      fields+=("--field=Use the latest Windows ARM64 drivers instead of the pinned version ($DRIVER_VER)":CHK "$(echo "$DRIVERS_USE_LATEST" | sed 's/1/TRUE/g' | sed 's/0/FALSE/g')")
      fields+=("--field=Skip verifying the written image after flashing (not recommended)":CHK "$(echo "$SKIP_IMAGE_VERIFICATION" | sed 's/1/TRUE/g' | sed 's/0/FALSE/g')")
      fields+=("--field=Apply the customized config.txt below (unchecked uses the firmware's default config.txt)":CHK "$(echo "$APPLY_CUSTOM_CONFIG_TXT" | sed 's/1/TRUE/g' | sed 's/0/FALSE/g')")
      #USE_CACHE has three values, so it needs a combo rather than a check box; the selected item comes first
      case "$USE_CACHE" in
        0) cache_items='Re-download everything, ignoring the cache!Reuse cached files when they still match (recommended)!Trust the cache without checking it' ;;
        2) cache_items='Trust the cache without checking it!Re-download everything, ignoring the cache!Reuse cached files when they still match (recommended)' ;;
        *) cache_items='Reuse cached files when they still match (recommended)!Re-download everything, ignoring the cache!Trust the cache without checking it' ;;
      esac
      fields+=("--field=Downloaded files":CB "$cache_items")

      output="$(yad "${yadflags[@]}" --width=500 --height=400 --image-on-top \
        "${refresh_prompt[@]}" \
        --form \
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
          #end of parsing check-box values for advanced options window

          break #as the DL_DIR value was not changed, go back to the Installation Overview window
        fi

      else #button != OK
        break #Don't save and go back to Installation Overview
      fi
    done #end of repeating the advanced options window

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

gui_start_installer

progress_fifo="$(mktemp -u)"
mkfifo "$progress_fifo"
tail -n +1 -F "$progress_file" > "$progress_fifo" 2>/dev/null &
tail_pid=$!
awk -F'\t' '
  #pct, not sub: sub() is a built-in awk function and cannot be used as a variable
  #before the first STEP (e.g. while clearing the cache) the percentage stands on its own
  function overall() { if (total+0 > 0) printf("%d\n", ((step-1)*100 + pct) / (total*100) * 100); else printf("%d\n", pct+0) }
  /^STEP/    { step=$2+0; total=$3+0; pct=0; overall(); printf("# [Step %s/%s] %s\n", $2, $3, $4); fflush() }
  /^SUBSTEP/ { pct=$2+0; if (pct<0) pct=0; if (pct>100) pct=100; overall(); fflush() }
  /^STATUS/  { printf("# %s\n", $2); fflush() }
' < "$progress_fifo" | yad "${yadflags[@]}" --progress --no-buttons --text="Starting..." &
yad_pid=$!

while [ ! -f "$done_marker" ];do
  sleep 0.3
done
exitcode="$(cat "$done_marker" 2>/dev/null)"
[ -z "$exitcode" ] && exitcode=1

kill "$tail_pid" "$yad_pid" 2>/dev/null
wait "$tail_pid" "$yad_pid" 2>/dev/null
rm -f "$progress_fifo" "$progress_file" "$done_marker"

#clear zram - avoid leaving files occupying space in /zram
if [ "$DL_DIR" == /zram ];then
  sudo zram.sh &>/dev/null
fi

if [ "$exitcode" == 0 ];then
  rm -f "$output_log" "$error_marker"
  #display "next steps" window
  yad --center --window-icon="$DIRECTORY/logo.png" --title="$WOR_APP_TITLE" \
    --image="$DIRECTORY/next-steps.png" --button=Close:0
else
  #keep the log on failure; the dialog only shows a tail, and the GUI has no terminal to fall back on
  saved_log="$(gui_save_failure_log)"
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
