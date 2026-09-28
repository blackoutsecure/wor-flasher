import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { readFileSync, mkdtempSync, writeFileSync, existsSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";
import vm from "node:vm";

const root = fileURLToPath(new URL("../", import.meta.url));
const helperSource = readFileSync(join(root, "src/lib/macos-disk-alerts.js"), "utf8");
const guiSource = readFileSync(join(root, "install-wor-gui.sh"), "utf8");
const context = vm.createContext({});
vm.runInContext(helperSource, context);
const decide = context.worDiskAlertDecision;
const active = "STEP\t5\t8\tPartitioning\nDISK_WRITE\t1\t/dev/disk99\n";

function alert(overrides = {}) {
  return {
    owner: "UserNotificationCenter",
    reference: "system-window",
    texts: ["The disk you attached was not readable by this computer."],
    buttons: [
      { name: "Eject", enabled: true, reference: "eject" },
      { name: "Initialize...", enabled: true, reference: "initialize" },
      { name: "Ignore", enabled: true, reference: "ignore" },
    ],
    ...overrides,
  };
}

describe("macOS startup Accessibility probe", () => {
  it("checks current trust without starting alert automation or requesting permissions", () => {
    let trusted = true;
    let checks = 0;
    const imports = [];
    const probe = vm.createContext({
      ObjC: { import: (name) => imports.push(name) },
      $: { AXIsProcessTrusted: () => { checks++; return trusted; } },
      Application: () => assert.fail("The startup probe must not send Apple events."),
    });
    vm.runInContext(helperSource, probe);
    assert.equal(probe.run(["--check-accessibility"]), "granted");
    trusted = false;
    assert.equal(probe.run(["--check-accessibility"]), "missing");
    assert.equal(checks, 2);
    assert.deepEqual(imports, ["ApplicationServices", "ApplicationServices"]);
  });

  it("propagates check failures and rejects malformed probe arguments", () => {
    const probe = vm.createContext({
      ObjC: { import: () => {} },
      $: { AXIsProcessTrusted: () => { throw new Error("mock Accessibility failure"); } },
    });
    vm.runInContext(helperSource, probe);
    assert.throws(() => probe.run(["--check-accessibility"]), /mock Accessibility failure/);
    assert.throws(() => probe.run(["--check-accessibility", "extra"]), /Expected --check-accessibility/);
  });
});

function runStartupPreflight({ checks, choices = [], os = "Darwin", repeat = 1 }) {
  const directory = mkdtempSync(join(tmpdir(), "wor-startup-accessibility-"));
  try {
    const match = guiSource.match(/^macos_check_accessibility\(\) \{[\s\S]*?^\}/m);
    assert.ok(match, "The GUI startup preflight function is missing.");
    writeFileSync(join(directory, "function.sh"), match[0]);
    writeFileSync(join(directory, "checks"), checks.join("\n") + "\n");
    writeFileSync(join(directory, "choices"), choices.join("\n") + "\n");
    writeFileSync(join(directory, "check-count"), "0");
    writeFileSync(join(directory, "choice-count"), "0");
    writeFileSync(join(directory, "messages"), "");
    const result = spawnSync("bash", ["-c", `
      DIRECTORY="$1" WOR_ICON_PATH=mock-icon WOR_WINDOW_TITLE="WoR-Flasher test" WOR_APP_TITLE=WoR-Flasher
      source "$DIRECTORY/function.sh"
      is_macos() { [ "$TEST_OS" == Darwin ]; }
      status() { printf "status: %s\\n" "$*" >&2; }
      echo_red() { printf "warning: %s\\n" "$*" >&2; }
      error() { printf "error: %s\\n" "$*" >&2; exit 1; }
      wor_osascript() {
        [ "$#" == 4 ] && [ "$1" == -l ] && [ "$2" == JavaScript ] &&
          [ "$3" == "$DIRECTORY/src/lib/macos-disk-alerts.js" ] &&
          [ "$4" == --check-accessibility ] || return 99
        count="$(cat "$DIRECTORY/check-count")"
        count=$((count + 1))
        printf '%s\\n' "$count" > "$DIRECTORY/check-count"
        response="$(sed -n "\${count}p" "$DIRECTORY/checks")"
        if [ "$response" == failure ];then
          printf "mock helper failure\\n" >&2
          return 42
        fi
        [ -n "$response" ] || return 98
        printf '%s\\n' "$response"
      }
      macos_choose() {
        [ "$#" == 14 ] && [ "$3" == recheck ] && [ "$4" == "Continue Manually" ] &&
          [ "$5" == "Open Settings" ] && [ "$8" == Recheck ] &&
          [ "\${11}" == manual ] && [ "\${12}" == 0 ] &&
          [ "\${14}" == "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility" ] || return 99
        printf '%s\\n' "$2" >> "$DIRECTORY/messages"
        count="$(cat "$DIRECTORY/choice-count")"
        count=$((count + 1))
        printf '%s\\n' "$count" > "$DIRECTORY/choice-count"
        response="$(sed -n "\${count}p" "$DIRECTORY/choices")"
        [ "$response" != quit ] && [ -n "$response" ] || return 1
        printf '%s\\n' "$response"
      }
      for ((iteration=0; iteration<TEST_REPEAT; iteration++));do
        macos_check_accessibility || exit "$?"
      done
      printf "wizard may start\\n"
    `, "bash", directory], {
      encoding: "utf8",
      timeout: 5000,
      env: { ...process.env, TEST_OS: os, TEST_REPEAT: String(repeat) },
    });
    return {
      ...result,
      checks: Number(readFileSync(join(directory, "check-count"), "utf8")),
      dialogs: Number(readFileSync(join(directory, "choice-count"), "utf8")),
      messages: readFileSync(join(directory, "messages"), "utf8"),
    };
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
}

describe("macOS startup Accessibility preflight", () => {
  it("continues quietly when already granted and checks again on the next invocation", () => {
    const result = runStartupPreflight({ checks: ["granted", "granted"], repeat: 2 });
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.checks, 2);
    assert.equal(result.dialogs, 0);
    assert.equal(result.stdout, "wizard may start\n");
  });

  it("rechecks the real host after missing permission instead of caching denial", () => {
    const result = runStartupPreflight({ checks: ["missing", "granted"], choices: ["recheck"] });
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.checks, 2);
    assert.equal(result.dialogs, 1);
    assert.match(result.messages, /Automatic Ignore needs Accessibility permission/);
    assert.match(result.messages, /quit and reopen WoR-Flasher/);
    assert.match(result.stderr, /Accessibility permission is allowed/);
  });

  it("keeps prompting when a recheck is still denied, without implicitly granting access", () => {
    const result = runStartupPreflight({ checks: ["missing", "missing"], choices: ["recheck", "manual"] });
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.dialogs, 2);
    assert.doesNotMatch(result.stderr, /permission is allowed/);
    assert.match(result.stderr, /Choose Ignore manually/);
  });

  it("permits manual continuation only after the user makes that explicit choice", () => {
    const result = runStartupPreflight({ checks: ["missing"], choices: ["manual"] });
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.stderr, /continuing without confirmed Accessibility permission/);
    assert.equal(result.stdout, "wizard may start\n");
  });

  it("stops before setup when the permission dialog is closed or quit", () => {
    const result = runStartupPreflight({ checks: ["missing"], choices: ["quit"] });
    assert.equal(result.status, 1);
    assert.equal(result.stdout, "");
    assert.doesNotMatch(result.stderr, /continuing without/);
  });

  for (const check of ["failure", "unexpected-output"]) {
    it(`reports ${check} instead of silently assuming trust`, () => {
      const result = runStartupPreflight({ checks: [check], choices: ["manual"] });
      assert.equal(result.status, 0, result.stderr);
      assert.match(result.messages, /could not check Accessibility permission/);
      assert.match(result.stderr, /helper exit/);
      assert.match(result.stderr, /continuing without confirmed Accessibility permission/);
    });
  }

  it("rejects unexpected dialog responses without starting setup", () => {
    const result = runStartupPreflight({ checks: ["missing"], choices: ["invalid-choice"] });
    assert.equal(result.status, 1);
    assert.match(result.stderr, /Unexpected Accessibility startup response/);
    assert.equal(result.stdout, "");
  });

  it("leaves non-macOS startup unchanged", () => {
    const result = runStartupPreflight({ checks: [], os: "Linux" });
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.checks, 0);
    assert.equal(result.dialogs, 0);
  });

  it("runs after single-instance acquisition and before setup or the target wizard", () => {
    const branch = guiSource.match(/^if is_macos ;then\n  command -v osascript[\s\S]*?^fi$/m);
    assert.ok(branch);
    const preflight = branch[0].indexOf("macos_check_accessibility || exit 0");
    const setup = branch[0].indexOf("setup || exit 1");
    const announcement = branch[0].indexOf("macos_show_announcement");
    const wizard = branch[0].indexOf("macos_start_cli");
    assert.ok(preflight >= 0 && preflight < setup && setup < announcement && announcement < wizard);
    assert.ok(guiSource.indexOf("acquire_gui_instance || exit 0") < branch.index);
    assert.match(helperSource, /if \(!\$\.AXIsProcessTrusted\(\)\)/);
  });
});

describe("macOS permission dialog settings action", () => {
  const start = guiSource.indexOf("    'actionClicked:':", guiSource.indexOf("macos_choose() {"));
  const end = guiSource.indexOf("    'windowWillClose:':", start);
  assert.ok(start >= 0 && end > start);
  const action = guiSource.slice(start, end);

  for (const scenario of ["settings", "settings-failure", "existing-action"]) {
    it(`handles ${scenario} without changing other chooser behavior`, () => {
      const calls = [];
      const url = "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility";
      const bridge = (value) => value;
      Object.assign(bridge, {
        NSWorkspace: { sharedWorkspace: { openURL: (value) => { calls.push(["open", value]); return scenario !== "settings-failure"; } } },
        NSURL: { URLWithString: (value) => value },
        NSAlert: { alloc: { get init() {
          return {
            addButtonWithTitle: (title) => calls.push(["button", title]),
            get runModal() { calls.push(["warning", this.messageText]); return 0; },
          };
        } } },
        NSOKButton: 1,
      });
      const scope = vm.createContext({
        $: bridge,
        actionURL: scenario === "existing-action" ? "" : url,
        actionValue: "existing-result",
        selectedValue: null,
        app: { stopModalWithCode: () => calls.push(["stop"]) },
        window: { orderOut: () => calls.push(["close"]) },
      });
      vm.runInContext(`({${action}})["actionClicked:"].implementation()`, scope);
      if (scenario === "existing-action") {
        assert.equal(scope.selectedValue, "existing-result");
        assert.deepEqual(calls, [["stop"], ["close"]]);
      } else {
        assert.equal(scope.selectedValue, null);
        assert.deepEqual(calls[0], ["open", url]);
        assert.ok(!calls.some(([name]) => name === "stop" || name === "close"));
        assert.equal(calls.some(([name]) => name === "warning"), scenario === "settings-failure");
      }
    });
  }
});

describe("macOS alert status presentation", () => {
  const render = guiSource.match(/^function updateDiskAlertStatus\(\) \{[\s\S]*?^\}/m);
  assert.ok(render, "The actual progress status renderer is missing.");
  it("starts with no routine message before the first status update", () => {
    const initializer = guiSource.match(/^diskAlertLabel = .*$/m);
    assert.ok(initializer);
    const scope = vm.createContext({
      diskAlertLabel: null,
      $: { NSTextField: { wrappingLabelWithString: (message) => ({ stringValue: message }) } },
    });
    vm.runInContext(initializer[0], scope);
    assert.equal(scope.diskAlertLabel.stringValue, "");
  });

  for (const scenario of [
    { state: "watching", message: "Automatic Ignore is watching for the macOS unreadable-disk alert.", expected: "" },
    { state: "warning", message: "Accessibility permission is required.", expected: "Accessibility permission is required.", warning: true },
    { state: "ignored", message: "Automatically chose Ignore.", expected: "Automatically chose Ignore." },
    { state: "waiting", message: "Automatic Ignore is waiting for disk preparation.", expected: "" },
    { state: "waiting", message: "Waiting", done: "23", expected: /Automatic Ignore is unavailable/, warning: true },
    { state: "watching", message: "Watching", done: "23", expected: /Automatic Ignore is unavailable/, warning: true },
    { state: "invalid", raw: "not JSON", expected: /Automatic Ignore status is unavailable/, warning: true },
  ]) {
    it(`renders ${scenario.state}${scenario.done ? " with helper failure" : ""} without hiding actionable messages`, () => {
      const label = { stringValue: "previous warning", textColor: "orange" };
      const scope = vm.createContext({
        diskAlertLabel: label,
        diskAlertStatus: "status",
        diskAlertDone: "done",
        readFile: (path) => path === "done"
          ? (scenario.done || "")
          : (scenario.raw || JSON.stringify({ state: scenario.state, message: scenario.message })),
        $: { NSColor: { systemOrangeColor: "orange", secondaryLabelColor: "secondary" } },
      });
      vm.runInContext(`${render[0]}\nupdateDiskAlertStatus()`, scope);
      if (scenario.expected instanceof RegExp) assert.match(label.stringValue, scenario.expected);
      else assert.equal(label.stringValue, scenario.expected);
      assert.equal(label.textColor, scenario.warning ? "orange" : "secondary");
    });
  }
});

describe("macOS unreadable-disk alert policy", () => {
  it("selects only Ignore for the exact system alert during authorized writes", () => {
    for (const owner of ["UserNotificationCenter", "DiskArbitrationAgent"]) {
      const result = decide(active, false, [alert({ owner })]);
      assert.equal(result.action, "ignore");
      assert.equal(result.target.button, "ignore");
    }
  });

  it("does nothing before authentication, during downloads, or after writes finish", () => {
    for (const progress of [
      "",
      "STEP\t5\t8\tWaiting for administrator access\n",
      "STEP\t4\t8\tPreparing Windows\nDISK_WRITE\t1\t/dev/disk99\n",
      `${active}DISK_WRITE\t0\t/dev/disk99\n`,
      `${active}STEP\t4\t8\tRetry preparation\n`,
    ]) {
      assert.equal(decide(progress, false, [alert()]).action, "none");
    }
  });

  it("does nothing after completion, cancellation, or loss of the installer", () => {
    assert.equal(decide(active, true, [alert()]).action, "none");
  });

  it("rejects malformed progress and non-whole-disk write markers", () => {
    for (const progress of [
      "STEP\t5x\t8\tPartitioning\nDISK_WRITE\t1\t/dev/disk99\n",
      "STEP\t5\t9\tPartitioning\nDISK_WRITE\t1\t/dev/disk99\n",
      "STEP\t5\t8\tPartitioning\nDISK_WRITE\t1\t/dev/disk99s1\n",
      "STEP\t5\t8\tPartitioning\nDISK_WRITE\t1\t/tmp/image\n",
      "STATUS\tSTEP\t5\t8\tPartitioning\nDISK_WRITE\t1\t/dev/disk99\n",
    ]) {
      assert.equal(decide(progress, false, [alert()]).action, "none");
    }
  });

  it("leaves lookalike application dialogs and other system alerts untouched", () => {
    assert.equal(decide(active, false, [alert({ owner: "WoR-Flasher" })]).action, "none");
    assert.equal(decide(active, false, [alert({ owner: "Finder" })]).action, "none");
    assert.equal(decide(active, false, [alert({ texts: ["A different warning with an Ignore button."] })]).action, "none");
    assert.equal(decide(active, false, [alert({ texts: ["Do not ignore: The disk you attached was not readable by this computer."] })]).action, "none");
  });

  it("requires the expected controls and an enabled, unambiguous Ignore button", () => {
    const original = alert();
    for (const buttons of [
      original.buttons.slice(1),
      original.buttons.filter((button) => button.name !== "Initialize..."),
      original.buttons.map((button) => ({ ...button, enabled: false })),
      [...original.buttons, original.buttons[2]],
    ]) {
      assert.equal(decide(active, false, [alert({ buttons })]).action, "none");
    }
  });

  it("tolerates presentation whitespace and ellipses without choosing another action", () => {
    const candidate = alert({
      texts: ["The disk you attached was not\nreadable by this computer"],
      buttons: [
        { name: "Ignore", enabled: true, reference: "ignore" },
        { name: "Initialize\u2026", enabled: true, reference: "initialize" },
        { name: "Eject", enabled: true, reference: "eject" },
      ],
    });
    assert.equal(decide(active, false, [candidate]).target.button, "ignore");
  });

  it("leaves multiple matching alerts for manual handling", () => {
    const result = decide(active, false, [alert(), alert({ reference: "another-disk" })]);
    assert.equal(result.action, "ambiguous");
    assert.equal(result.target, undefined);
  });

  it("does not misclassify denied accessibility permission as a disappearing alert", () => {
    assert.equal(context.worDiskAlertTransientError({ number: -1719, message: "osascript is not allowed assistive access." }), false);
    assert.equal(context.worDiskAlertTransientError({ number: -1743, message: "Not authorized to send Apple events." }), false);
    assert.equal(context.worDiskAlertTransientError({ number: -1719, message: "Invalid index." }), true);
    assert.equal(context.worDiskAlertTransientError({ number: -1728, message: "Cannot get the missing window." }), true);
    assert.equal(context.worDiskAlertTransientError({ number: -600, message: "Application is not running." }), true);
    assert.equal(context.worDiskAlertTransientError({ number: -1, message: "Unexpected failure." }), false);
  });

  it("receives authorization only after authentication and finalizer readiness", () => {
    const engine = readFileSync(join(root, "install-wor.sh"), "utf8");
    const authenticate = engine.indexOf('sudo -v || error "Administrator authentication failed or was canceled. Enter the macOS password');
    const finalizer = engine.indexOf('darwin_start_partition_finalizer_or_die "$DEVICE"', authenticate);
    const begin = engine.indexOf('emit_gui_progress "DISK_WRITE"', finalizer);
    const prepare = engine.indexOf('darwin_prepare_disk_or_die "$DEVICE"', begin);
    assert.ok(authenticate >= 0 && authenticate < finalizer && finalizer < begin && begin < prepare);
    assert.match(engine.slice(prepare), /emit_gui_progress "DISK_WRITE"\$'\\t'"0"/);
  });
});

describe("macOS disk-alert helper isolation", () => {
  for (const scenario of ["success", "failure", "blocked"]) {
    const failed = scenario === "failure";
    const blocked = scenario === "blocked";
    it(`handles helper ${scenario} without changing installer status or hanging cleanup`, () => {
      const directory = mkdtempSync(join(tmpdir(), "wor-alert-lifecycle-"));
      let completed = false;
      try {
        const gui = readFileSync(join(root, "install-wor-gui.sh"), "utf8");
        const functions = ["gui_start_disk_alert_handler", "gui_stop_disk_alert_handler"].map((name) => {
          const match = gui.match(new RegExp(`^${name}\\(\\) \\{[\\s\\S]*?^\\}`, "m"));
          assert.ok(match, `missing ${name}`);
          return match[0];
        }).join("\n");
        writeFileSync(join(directory, "functions.sh"), functions);
        const result = spawnSync("bash", ["-c", `
          source "$1/functions.sh"
          DIRECTORY="$1" progress_file="$1/progress" done_marker="$1/done" abort_marker="$1/abort"
          disk_alert_status="$1/status" disk_alert_log="$1/log" disk_alert_done="$1/helper-done"
          output_log="$1/installer-log" installer_pid=$$ installer_status=0 disk_alert_warning=""
          printf "installer output\\n" > "$output_log"
          wor_osascript() {
            printf '%s\\n' "$MOCK_ALERT_STATUS" > "$disk_alert_status"
            printf "helper output\\n"
            if [ "$MOCK_ALERT_BLOCK" == 1 ];then
              trap "" TERM
              sleep 30 &
              printf '%s\\n' "$!" >> "$DIRECTORY/pids"
              wait "$!"
            fi
            return "$MOCK_ALERT_EXIT"
          }
          gui_start_disk_alert_handler
          printf '%s\\n' "$disk_alert_pid" >> "$DIRECTORY/pids"
          gui_stop_disk_alert_handler
          printf '%s\\n' "$installer_status" "$disk_alert_warning"
        `, "bash", directory], {
          encoding: "utf8",
          timeout: 8000,
          env: {
            ...process.env,
            MOCK_ALERT_BLOCK: blocked ? "1" : "0",
            MOCK_ALERT_EXIT: failed ? "23" : "0",
            MOCK_ALERT_STATUS: JSON.stringify({ state: failed ? "warning" : "ignored", message: failed ? "mock permission denied" : "mock ignored" }),
          },
        });
        assert.equal(result.status, 0, result.stderr);
        assert.equal(result.stdout.split("\n")[0], "0");
        if (failed) assert.match(result.stdout, /mock permission denied/);
        else if (blocked) assert.match(result.stdout, /Automatic Ignore stopped unexpectedly/);
        else assert.equal(result.stdout.split("\n")[1], "");
        const log = readFileSync(join(directory, "installer-log"), "utf8");
        assert.ok(log.startsWith("installer output\n"));
        assert.match(log, /helper output/);
        for (const name of ["status", "log", "helper-done"]) {
          assert.equal(existsSync(join(directory, name)), false);
        }
        completed = true;
      } finally {
        if (!completed && existsSync(join(directory, "pids"))) {
          for (const value of readFileSync(join(directory, "pids"), "utf8").trim().split("\n")) {
            if (!/^[1-9][0-9]*$/.test(value)) continue;
            try {
              process.kill(Number(value), "SIGKILL");
            } catch (error) {
              if (error.code !== "ESRCH") throw error;
            }
          }
        }
        rmSync(directory, { recursive: true, force: true });
      }
    });
  }
});
