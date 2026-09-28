"use strict";

function worDiskAlertText(value) {
  return typeof value === "string"
    ? value.replace(/\u2026/g, "...").replace(/\s+/g, " ").trim().toLowerCase()
    : "";
}

function worDiskAlertWritePhase(progress) {
  const lines = String(progress).split("\n");
  let step = "";
  let write = "";
  for (let index = lines.length - 1; index >= 0; index--) {
    if (!step && lines[index].indexOf("STEP\t") === 0) step = lines[index];
    if (!write && lines[index].indexOf("DISK_WRITE\t") === 0) write = lines[index];
    if (step && write) break;
  }
  const fields = step.split("\t");
  const target = write.split("\t");
  return /^[0-9]+$/.test(fields[1]) && Number(fields[2]) === 8 &&
    Number(fields[1]) >= 5 && Number(fields[1]) <= 8 &&
    target[1] === "1" && /^\/dev\/disk[0-9]+$/.test(target[2]);
}

function worDiskAlertDecision(progress, stopped, windows) {
  if (stopped || !worDiskAlertWritePhase(progress)) return { action: "none" };
  const matches = [];
  for (const window of windows) {
    if (window.owner !== "UserNotificationCenter" && window.owner !== "DiskArbitrationAgent") continue;
    if (!window.texts.some(function (text) {
      return /^the disk you (attached|inserted) was not readable by this computer\.?$/.test(worDiskAlertText(text));
    })) continue;
    const buttons = window.buttons;
    const ignore = buttons.filter(function (button) {
      return worDiskAlertText(button.name) === "ignore" && button.enabled === true;
    });
    if (ignore.length !== 1 || !buttons.some(function (button) {
      return worDiskAlertText(button.name) === "eject";
    }) || !buttons.some(function (button) {
      return /^(initialize|initialize\.\.\.)$/.test(worDiskAlertText(button.name));
    })) continue;
    matches.push({ owner: window.owner, window: window.reference, button: ignore[0].reference });
  }
  if (matches.length > 1) return { action: "ambiguous" };
  if (matches.length === 1) return { action: "ignore", target: matches[0] };
  return { action: "none" };
}

function worDiskAlertTransientError(error) {
  const code = Number(error.errorNumber || error.number);
  const message = String(error.message || "");
  if (/not allowed|not authorized|assistive access|permission/i.test(message)) return false;
  return code === -1728 || code === -600 ||
    (code === -1719 && /invalid index|can.t get|cannot get|not found/i.test(message));
}

function run(argv) {
  const checkAccessibility = argv.length === 1 && argv[0] === "--check-accessibility";
  if (!checkAccessibility && (argv.length !== 5 || !/^[1-9][0-9]*$/.test(argv[3]))) {
    throw new Error("Expected --check-accessibility or progress, completion, abort, installer PID, and status paths.");
  }
  ObjC.import("ApplicationServices");
  if (checkAccessibility) return $.AXIsProcessTrusted() ? "granted" : "missing";
  ObjC.import("Foundation");
  ObjC.bindFunction("kill", ["int", ["int", "int"]]);
  const progressPath = argv[0];
  const donePath = argv[1];
  const abortPath = argv[2];
  const installerPid = Number(argv[3]);
  const statusPath = argv[4];
  const files = $.NSFileManager.defaultManager;
  let lastStatus = "";
  let lastState = "";
  let dismissed = 0;

  function readFile(path) {
    if (!files.fileExistsAtPath($(path))) return "";
    const text = $.NSString.stringWithContentsOfFileEncodingError($(path), $.NSUTF8StringEncoding, Ref());
    if (text.isNil()) throw new Error("Could not read the disk-alert control file.");
    return ObjC.unwrap(text);
  }

  function stopped() {
    return files.fileExistsAtPath($(donePath)) ||
      files.fileExistsAtPath($(abortPath)) || $.kill(installerPid, 0) !== 0;
  }

  function report(state, message) {
    const json = JSON.stringify({ state: state, message: message, dismissed: dismissed });
    if (json === lastStatus) return;
    if (!$(json).writeToFileAtomicallyEncodingError($(statusPath), true, $.NSUTF8StringEncoding, Ref())) {
      throw new Error("Could not write the disk-alert status.");
    }
    lastStatus = json;
    lastState = state;
  }

  function snapshot(owner, window) {
    const texts = [window.name()];
    const buttons = [];
    const elements = window.entireContents();
    if (elements.length > 100) return null;
    for (const element of elements) {
      const properties = element.properties();
      if (properties.role === "AXStaticText") {
        texts.push(properties.value);
        texts.push(properties.name);
      } else if (properties.role === "AXButton") {
        buttons.push({ name: properties.name, enabled: properties.enabled, reference: element });
      }
    }
    return { owner: owner, reference: window, texts: texts, buttons: buttons };
  }

  const systemEvents = Application("System Events");
  report("waiting", "Automatic Ignore is waiting for disk preparation.");
  try {
    while (!stopped()) {
      const progress = readFile(progressPath);
      if (worDiskAlertWritePhase(progress)) {
        if (!$.AXIsProcessTrusted()) {
          report("warning", "Automatic Ignore needs Accessibility permission for WoR-Flasher. Choose Ignore manually until it is allowed.");
        } else {
          try {
            const windows = [];
            for (const owner of ["UserNotificationCenter", "DiskArbitrationAgent"]) {
              const processes = systemEvents.processes.whose({ name: owner })();
              for (const process of processes) {
                for (const window of process.windows()) {
                  const entry = snapshot(owner, window);
                  if (entry) windows.push(entry);
                }
              }
            }
            const decision = worDiskAlertDecision(progress, stopped(), windows);
            if (decision.action === "ambiguous") {
              report("warning", "Multiple unreadable-disk alerts are open. Choose Ignore manually; no alert was selected automatically.");
            } else if (decision.action === "ignore") {
              const current = snapshot(decision.target.owner, decision.target.window);
              const confirmed = worDiskAlertDecision(readFile(progressPath), stopped(), current ? [current] : []);
              if (confirmed.action === "ignore" && worDiskAlertText(confirmed.target.button.name()) === "ignore") {
                confirmed.target.button.click();
                dismissed++;
                report("ignored", "Automatically chose Ignore for the macOS unreadable-disk alert.");
                console.log("Automatically chose Ignore for the macOS unreadable-disk alert.");
              }
            } else if (dismissed === 0 || lastState === "warning") {
              report("watching", "Automatic Ignore is watching for the macOS unreadable-disk alert.");
            }
          } catch (error) {
            if (!worDiskAlertTransientError(error)) {
              const message = "Automatic Ignore is unavailable. Allow WoR-Flasher in Privacy & Security > Accessibility and Automation, or choose Ignore manually.";
              report("warning", message);
              throw new Error(message);
            }
          }
        }
      }
      $.NSThread.sleepForTimeInterval(0.4);
    }
  } catch (error) {
    if (lastState !== "warning") {
      report("warning", "Automatic Ignore stopped. Choose Ignore manually if the unreadable-disk alert appears.");
    }
    throw error;
  }
}
