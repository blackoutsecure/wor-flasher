import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { readFileSync, mkdtempSync, writeFileSync, existsSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";
import vm from "node:vm";

const root = fileURLToPath(new URL("../", import.meta.url));
const context = vm.createContext({});
vm.runInContext(readFileSync(join(root, "src/lib/macos-disk-alerts.js"), "utf8"), context);
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
