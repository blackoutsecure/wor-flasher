import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";

const root = fileURLToPath(new URL("../", import.meta.url));
const gui = readFileSync(join(root, "install-wor-gui.sh"), "utf8");
const dialog = gui.match(/^macos_password_retry_dialog\(\) \{[\s\S]*?^\}/m);
assert.ok(dialog, "The administrator-password retry dialog is missing.");
const authFailure = "\u001b[91mAdministrator authentication failed or was canceled. Enter the macOS password in the WoR-Flasher dialog and try again.\u001b[0m\n";
const canceledLog = "364:467: execution error: User canceled. (-128)\nsudo: no password was provided\nsudo: a password is required\n" + authFailure;
const prewrite = "STEP\t5\t8\tPartitioning and formatting /dev/mock-target\nTASK\t0\tWaiting for administrator access...\n";

function invokeDialog({ log = canceledLog, progress = prewrite, choice = "retry" } = {}) {
  const directory = mkdtempSync(join(tmpdir(), "wor-password-cancel-"));
  try {
    writeFileSync(join(directory, "dialog.sh"), dialog[0]);
    if (log !== null) writeFileSync(join(directory, "run.log"), log);
    if (progress !== null) writeFileSync(join(directory, "progress"), progress);
    const result = spawnSync("bash", ["-c", `
      source "$1/dialog.sh"
      DEVICE=/dev/mock-target WOR_ICON_PATH=mock-icon WOR_WINDOW_TITLE="WoR-Flasher test"
      capture="$1/dialog-args"
      warning() { printf "warning: %s\\n" "$*" >&2; }
      macos_choose() {
        printf '%s\\0' "$@" > "$capture"
        [ "$TEST_CHOICE" != close ] || return 1
        printf '%s\\n' "$TEST_CHOICE"
      }
      macos_password_retry_dialog "$1/run.log" "$1/progress"
    `, "bash", directory], {
      encoding: "utf8", timeout: 5000, env: { ...process.env, TEST_CHOICE: choice },
    });
    const args = existsSync(join(directory, "dialog-args"))
      ? readFileSync(join(directory, "dialog-args"), "utf8").split("\0").slice(0, -1) : [];
    if (log !== null) assert.equal(readFileSync(join(directory, "run.log"), "utf8"), log);
    return { ...result, args };
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
}

describe("Canceled macOS administrator password", () => {
  it("shows a concise retry screen for the reported native Cancel output", () => {
    const result = invokeDialog();
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.stdout, "retry\n");
    assert.match(result.args[1], /^Administrator password entry was canceled\./);
    assert.match(result.args[1], /Flashing has not started\. No changes have been made to \/dev\/mock-target\./);
    assert.match(result.args[1], /prepared downloads have been kept/);
    assert.match(result.args[1], /Log: .*run\.log/);
    assert.doesNotMatch(result.args[1], /stopped unexpectedly|execution error|sudo:|turning back|exit code/);
    assert.equal(result.args[3], "Close");
    assert.equal(result.args[7], "Try Again");
    assert.match(result.args[9], /Administrator access$/);
  });

  it("closes without retrying when Close or the window close control is used", () => {
    const result = invokeDialog({ choice: "close" });
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.stdout, "close\n");
  });

  it("distinguishes an empty password from clicking Cancel", () => {
    const result = invokeDialog({ log: "sudo: no password was provided\n" + authFailure });
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.args[1], /^No administrator password was entered\./);
    assert.doesNotMatch(result.args[1], /entry was canceled/);
  });

  it("distinguishes rejected passwords from cancellation", () => {
    const result = invokeDialog({ log: "sudo: 3 incorrect password attempts\n" + authFailure });
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.args[1], /^The administrator password was not accepted\./);
  });

  it("does not call a generic authorization failure a user cancellation", () => {
    const result = invokeDialog({ log: "sudo: account is not authorized\n" + authFailure });
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.args[1], /^Administrator access was not obtained\./);
  });

  it("recognizes native Cancel by its error number rather than its translated text", () => {
    const result = invokeDialog({ log: "execution error: translated message (-128)\n" + authFailure });
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.args[1], /^Administrator password entry was canceled\./);
  });

  it("supports the Pi 5 preparation step and the older authentication diagnostic", () => {
    const result = invokeDialog({
      progress: "STEP\t4\t7\tPartitioning and formatting /dev/mock-target\n",
      log: "sudo: no password was provided\nAdministrator authentication was canceled or unavailable while preparing /dev/mock-target.\n",
    });
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.stdout, "retry\n");
  });

  for (const progress of [
    prewrite + "DISK_WRITE\t1\t/dev/mock-target\n",
    prewrite + "DISK_WRITE\t1\t/dev/mock-target\nDISK_WRITE\t0\t/dev/mock-target\n",
    prewrite + "STEP\t6\t8\tCopying files\n",
    "STEP\t4\t8\tPreparing the Windows image\n",
    "",
  ]) {
    it(`never claims no changes for unsafe or unconfirmed progress ${JSON.stringify(progress)}`, () => {
      const result = invokeDialog({ progress });
      assert.equal(result.status, 1);
      assert.equal(result.stdout, "");
      assert.equal(result.args.length, 0);
    });
  }

  it("leaves unrelated pre-write errors on the normal diagnostic path", () => {
    const result = invokeDialog({ log: "Failed to start partition finalizer.\n" });
    assert.equal(result.status, 1);
    assert.equal(result.args.length, 0);
  });

  for (const missing of ["log", "progress"]) {
    it(`reports unavailable ${missing} without presenting a no-changes claim`, () => {
      const result = invokeDialog({ [missing]: null });
      assert.equal(result.status, 1);
      assert.equal(result.args.length, 0);
      assert.match(result.stderr, /diagnostics are unavailable/);
    });
  }

  it("falls back explicitly if the native dialog returns an invalid choice", () => {
    const result = invokeDialog({ choice: "unexpected" });
    assert.equal(result.status, 1);
    assert.match(result.stderr, /showing the full error instead/);
    assert.equal(result.stdout, "");
  });

  it("classifies the saved log before deleting progress and preserves retry resume", () => {
    const status = gui.indexOf('installer_status="$(cat "$done_marker"');
    const save = gui.indexOf('saved_log="$(gui_save_installer_log)"', status);
    const retry = gui.indexOf('macos_password_retry_dialog "$saved_log" "$progress_file"', save);
    const cleanup = gui.indexOf('rm -f "$progress_file" "$done_marker" "$abort_marker" "$auth_marker"', retry);
    assert.ok(status >= 0 && status < save && save < retry && retry < cleanup);
    assert.match(gui.slice(cleanup), /if \[ "\$password_retry_choice" == retry \];then\n\s+resume_at_flash=1\n\s+continue/);
  });
});

describe("Administrator cancellation at the engine write boundary", () => {
  it("starts no disk worker or preparation and emits no premature write announcement", () => {
    const directory = mkdtempSync(join(tmpdir(), "wor-cancel-engine-"));
    try {
      for (const path of ["bin", "windows/bootpart", "peinstaller/winpe/2", "peinstaller/efi"]) {
        mkdirSync(join(directory, path), { recursive: true });
      }
      writeFileSync(join(directory, "bin/sgdisk"), '#!/bin/bash\nprintf "unexpected disk command\\n" >&2\nexit 99\n', { mode: 0o755 });
      const result = spawnSync("bash", ["-c", `
        source "$DIRECTORY/install-wor.sh" source >/dev/null
        HOST_OS=Darwin RUN_MODE=gui DEVICE=/dev/mock-target RPI_MODEL=4
        winfiles=windows CAN_INSTALL_ON_SAME_DRIVE=1 STEP_NUM=4 STEP_TOTAL=8
        WOR_GUI_PROGRESS_FILE="$PWD/progress"
        is_safe_target_device() { return 0; }
        sudo() {
          printf "execution error: User canceled. (-128)\\nsudo: no password was provided\\n" >&2
          return 1
        }
        darwin_start_partition_finalizer_or_die() { printf "unexpected worker start\\n"; exit 99; }
        darwin_prepare_disk_or_die() { printf "unexpected disk preparation\\n"; exit 99; }
        darwin_flash_device
      `], {
        cwd: directory, encoding: "utf8", timeout: 10000,
        env: { ...process.env, DIRECTORY: root, NO_UPDATE: "1", PATH: `${directory}/bin:${process.env.PATH}` },
      });
      assert.equal(result.status, 1);
      assert.match(result.stderr, /Administrator authentication failed or was canceled/);
      assert.doesNotMatch(result.stdout + result.stderr, /unexpected|There is no turning back|Creating WOR_BOOT/);
      const progress = readFileSync(join(directory, "progress"), "utf8");
      assert.doesNotMatch(progress, /^DISK_WRITE\t1\t/m);
      const rendered = invokeDialog({ log: result.stderr, progress });
      assert.equal(rendered.status, 0, rendered.stderr);
      assert.match(rendered.args[1], /^Administrator password entry was canceled\./);
    } finally {
      rmSync(directory, { recursive: true, force: true });
    }
  });
});
