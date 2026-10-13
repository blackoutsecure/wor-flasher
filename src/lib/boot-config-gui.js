"use strict";

function worBootConfigCall(context, changes) {
  const task = $.NSTask.alloc.init;
  task.launchPath = $(context.python);
  task.arguments = $([context.helper]);
  const input = $.NSPipe.pipe;
  const output = $.NSPipe.pipe;
  const errors = $.NSPipe.pipe;
  task.standardInput = input;
  task.standardOutput = output;
  task.standardError = errors;
  task.launch;
  const request = JSON.stringify(Object.assign({}, context, changes));
  input.fileHandleForWriting.writeData($(request).dataUsingEncoding($.NSUTF8StringEncoding));
  input.fileHandleForWriting.closeFile;
  const data = output.fileHandleForReading.readDataToEndOfFile;
  const errorData = errors.fileHandleForReading.readDataToEndOfFile;
  task.waitUntilExit;
  const text = ObjC.unwrap($.NSString.alloc.initWithDataEncoding(data, $.NSUTF8StringEncoding));
  const diagnostic = ObjC.unwrap($.NSString.alloc.initWithDataEncoding(errorData, $.NSUTF8StringEncoding));
  if (!text) throw new Error("Boot configuration validation failed: " + diagnostic);
  const result = JSON.parse(text);
  if (Number(task.terminationStatus) !== 0 || result.ok !== true) {
    throw new Error(result.error || diagnostic || "Cannot validate boot configuration.");
  }
  return result;
}

function worBootConfigAlert(message) {
  const alert = $.NSAlert.alloc.init;
  alert.messageText = $("Boot configuration");
  alert.informativeText = $(message);
  alert.alertStyle = $.NSAlertStyleWarning;
  alert.addButtonWithTitle("OK");
  alert.runModal;
}

function worBootConfigNotes(warnings) {
  const notes = warnings.slice(0, 3);
  if (warnings.length > notes.length) notes.push("Additional notes will be included in boot validation before flashing.");
  return notes.join("\n");
}

function worBootConfigValues(rows) {
  const values = {};
  for (const row of rows) {
    if (row.field.disabled) continue;
    if (row.field.type === "boolean") values[row.field.key] = Number(row.control.state) === 1 ? "1" : "0";
    else if (row.field.type === "choice") {
      values[row.field.key] = row.field.choices[Number(row.control.indexOfSelectedItem)].value;
    } else values[row.field.key] = ObjC.unwrap(row.control.stringValue);
  }
  return values;
}

function worBootConfigSave(context, state, rows, selectedMode) {
  const changes = { config: state.config, values: worBootConfigValues(rows) };
  if (selectedMode !== state.hdmiMode) changes.hdmiMode = selectedMode;
  return worBootConfigCall(context, changes);
}

function worEditBootConfig(value, context, iconPath) {
  let state;
  try {
    state = worBootConfigCall(context, { config: value });
  } catch (error) {
    worBootConfigAlert(String(error.message || error));
    return { saved: false, text: value };
  }
  while (true) {
    const width = 580;
    const documentHeight = 76 + state.fields.length * 38;
    const visibleHeight = Math.min(documentHeight, Number($.NSScreen.mainScreen.visibleFrame.size.height) - 380);
    const scroll = $.NSScrollView.alloc.initWithFrame($.NSMakeRect(0, 0, width, Math.max(130, visibleHeight)));
    scroll.hasVerticalScroller = documentHeight > visibleHeight;
    const content = $.NSView.alloc.initWithFrame($.NSMakeRect(0, 0, width - 18, documentHeight));
    scroll.documentView = content;
    const rows = [];
    let y = documentHeight - 32;
    const presetLabel = $.NSTextField.labelWithString("HDMI display");
    presetLabel.frame = $.NSMakeRect(0, y, 160, 26);
    content.addSubview(presetLabel);
    const preset = $.NSPopUpButton.alloc.initWithFramePullsDown($.NSMakeRect(170, y - 4, width - 196, 30), false);
    for (const item of state.hdmiChoices) preset.addItemWithTitle($(item.label));
    preset.selectItemAtIndex(state.hdmiChoices.findIndex(function (item) { return item.value === state.hdmiMode; }));
    content.addSubview(preset);
    y -= 42;
    for (const field of state.fields) {
      let control;
      if (field.type === "boolean") {
        control = $.NSButton.checkboxWithTitleTargetAction($(field.label), $(), $());
        control.frame = $.NSMakeRect(0, y, width - 24, 28);
        control.state = field.value === "1" ? 1 : 0;
      } else {
        const label = $.NSTextField.labelWithString($(field.label));
        label.frame = $.NSMakeRect(0, y, 210, 26);
        content.addSubview(label);
        if (field.type === "choice") {
          control = $.NSPopUpButton.alloc.initWithFramePullsDown($.NSMakeRect(220, y - 4, width - 246, 30), false);
          for (const item of field.choices) control.addItemWithTitle($(item.label));
          control.selectItemAtIndex(field.choices.findIndex(function (item) { return item.value === field.value; }));
        } else {
          control = $.NSTextField.alloc.initWithFrame($.NSMakeRect(220, y, width - 246, 26));
          control.stringValue = $(field.value);
        }
      }
      control.enabled = field.disabled !== true;
      control.toolTip = $(field.key + (field.disabled ? " - controlled by advanced conditional settings" : ""));
      content.addSubview(control);
      rows.push({ field: field, control: control });
      y -= 38;
    }
    content.scrollPoint($.NSMakePoint(0, Math.max(0, documentHeight - visibleHeight)));
    const dialog = $.NSAlert.alloc.init;
    dialog.messageText = $("Boot Config Editor");
    let help = context.profile === "iot-core"
      ? "Display settings only. Required IoT boot settings remain unchanged in the verified image."
      : "Use these controls instead of editing config.txt. Required UEFI boot settings are protected. User additions are kept separately.";
    if (state.warnings.length > 0) help += "\n\n" + worBootConfigNotes(state.warnings);
    dialog.informativeText = $(help);
    dialog.accessoryView = scroll;
    const icon = $.NSImage.alloc.initWithContentsOfFile($(iconPath));
    if (!icon.isNil()) dialog.icon = icon;
    dialog.addButtonWithTitle("Save");
    dialog.addButtonWithTitle("Cancel");
    dialog.addButtonWithTitle("Advanced Text Edit (Not Recommended)...");
    dialog.addButtonWithTitle("Restore defaults");
    const response = Number(dialog.runModal);
    if (response === Number($.NSAlertFirstButtonReturn) + 3) {
      try {
        state = worBootConfigCall(context, { config: state.config, reset: true });
      } catch (error) {
        worBootConfigAlert(String(error.message || error));
      }
      continue;
    }
    if (response !== Number($.NSAlertFirstButtonReturn) && response !== Number($.NSAlertThirdButtonReturn)) {
      return { saved: false, text: value };
    }
    let next;
    try {
      next = worBootConfigSave(context, state, rows, state.hdmiChoices[Number(preset.indexOfSelectedItem)].value);
    } catch (error) {
      worBootConfigAlert(String(error.message || error));
      //Keep the unsaved controls available for correction rather than closing the editor.
      const pending = worBootConfigValues(rows);
      for (const field of state.fields) {
        if (Object.prototype.hasOwnProperty.call(pending, field.key)) field.value = pending[field.key];
      }
      continue;
    }
    if (response === Number($.NSAlertThirdButtonReturn)) {
      let draft = next.userText;
      while (true) {
        const edit = worEditText(draft, "Advanced Text Edit - Not Recommended",
          context.profile === "iot-core"
            ? "Only the four supported HDMI parameters are allowed. Required IoT boot entries cannot be edited."
            : "Edit user additions only. Required UEFI settings stay protected. Unknown parameters are preserved with a warning, not certified as supported.",
          iconPath);
        if (!edit.saved) break;
        draft = edit.text;
        try {
          next = worBootConfigCall(context, { config: next.config, userText: draft });
          break;
        } catch (error) {
          worBootConfigAlert(String(error.message || error));
        }
      }
      state = next;
      continue;
    }
    if (next.warnings.length > 0) worBootConfigAlert(worBootConfigNotes(next.warnings));
    return { saved: true, text: next.config, hdmiMode: next.hdmiMode };
  }
}
