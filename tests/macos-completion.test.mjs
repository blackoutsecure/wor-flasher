import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";
import vm from "node:vm";

const root = fileURLToPath(new URL("../", import.meta.url));
const gui = readFileSync(join(root, "install-wor-gui.sh"), "utf8");
const helpers = [
  "gui_start_installer", "installer_showed_own_error", "gui_update_last_log",
  "gui_save_installer_log", "gui_log_tail", "macos_password_retry_dialog", "macos_show_result_dialog",
].map((name) => {
  const match = gui.match(new RegExp(`^${name}\\(\\) \\{[\\s\\S]*?^\\}`, "m"));
  assert.ok(match, `Missing ${name}`);
  return match[0];
}).join("\n");
const start = gui.indexOf("macos_start_cli() {");
const end = gui.indexOf("\nif is_macos ;then", start);
assert.ok(start >= 0 && end > start);
const flow = gui.slice(start, end);
const completion = gui.match(/^  completion_jxa="\$\(wor_jxa_window_lib; cat <<'JXA'\n([\s\S]*?)\nJXA$/m);
assert.ok(completion, "Missing completion JXA");

function runFlow({ installerStatus = 0, dialogStatus = 0, fallbackStatus = 0, abort = false, progressStatus = 0, ownError = false } = {}) {
  const directory = mkdtempSync(join(tmpdir(), "wor-completion-"));
  try {
    for (const path of ["bin", "tmp", "logs"]) mkdirSync(join(directory, path));
    writeFileSync(join(directory, "functions.sh"), `${helpers}\n${flow}\n`);
    writeFileSync(join(directory, "bin/mktemp"), `#!/bin/bash
if [ "$#" == 0 ];then
  exec /usr/bin/mktemp "$TEST_DIRECTORY/tmp/tmp.XXXXXX"
fi
if [ "$#" == 1 ] && [ "$1" == -u ];then
  exec /usr/bin/mktemp -u "$TEST_DIRECTORY/tmp/tmp.XXXXXX"
fi
printf 'Unexpected mktemp arguments\\n' >&2
exit 99
`, { mode: 0o755 });
    writeFileSync(join(directory, "installer.sh"), `#!/bin/bash
printf 'Simulated installer output; no physical media was accessed.\\n'
printf 'STEP\\t6\\t8\\tCopying files\\n' > "$WOR_GUI_PROGRESS_FILE"
: > "$WOR_GUI_AUTH_MARKER"
[ "$TEST_OWN_ERROR" != 1 ] || touch "$WOR_GUI_ERROR_MARKER"
exit "$TEST_INSTALLER_STATUS"
`, { mode: 0o755 });
    const result = spawnSync("bash", ["-c", `
      source "$TEST_DIRECTORY/functions.sh"
      cli_script="$TEST_DIRECTORY/installer.sh"
      WOR_ICON_PATH=mock-icon WOR_APP_TITLE=WoR-Flasher WOR_WINDOW_TITLE="WoR-Flasher test"
      WOR_ASSETS_DIR="$TEST_DIRECTORY/assets" WIN_LANG=en-us GUI_PROGRESS_EARLY=1 PLAY_SOUND=0
      is_macos() { return 0; }
      error() { printf "error: %s\\n" "$*" >&2; exit 1; }
      warning() { printf "warning: %s\\n" "$*" >&2; }
      status() { printf "status: %s\\n" "$*" >&2; }
      wor_log_file() { printf '%s/logs/run.log\\n' "$TEST_DIRECTORY"; }
      wor_last_log_file() { printf '%s/last-run.log\\n' "$TEST_DIRECTORY"; }
      export_installer_settings() { :; }
      gui_start_disk_alert_handler() { :; }
      gui_stop_disk_alert_handler() { :; }
      wor_jxa_window_lib() { :; }
      wor_show_result_notification() { printf '%s\\n' "$1" >> "$TEST_DIRECTORY/notifications"; }
      kill_process_tree() { printf '%s\\n' "$1" > "$TEST_DIRECTORY/stopped"; }
      macos_choose_target() { printf 'Windows 10\\tRaspberry Pi 3 / Pi 2 v1.2\\n'; }
      list_bids() { :; }
      get_bid() { printf '19045.3803\\n'; }
      set_default_config_txt() { :; }
      darwin_list_device_choices() { printf '/dev/mock-only\\tFixture disk\\n'; }
      macos_choose_device() { printf '/dev/mock-only\\tFixture disk\\n'; }
      is_safe_target_device() { [ "$1" == /dev/mock-only ]; }
      drive_capability() { printf 'install\\n'; }
      validate_install_mode() { :; }
      macos_choose() { printf 'Install Windows onto this drive\\n'; }
      macos_confirm_flash() { printf 'Flash\\n'; }
      wor_osascript() {
        local script attempt
        script="$(cat)"
        if [[ "$script" == *"WorProgressController"* ]];then
          for attempt in {1..100};do
            [ ! -f "$5" ] || break
            sleep 0.02
          done
          [ -f "$5" ] || { printf 'Fixture installer timed out\\n' >&2; return 99; }
          if [ "$TEST_PROGRESS_STATUS" != 0 ];then
            rm "$5"
            printf 'fixture progress renderer failed\\n' >&2
            return "$TEST_PROGRESS_STATUS"
          fi
          [ "$TEST_ABORT" != 1 ] || touch "$8"
          return 0
        fi
        [[ "$script" == *"WorCompletionController"* ]] || return 98
        printf '%s\\0' "$@" > "$TEST_DIRECTORY/dialog-args"
        if [ "$TEST_DIALOG_STATUS" != 0 ];then
          printf 'fixture custom result renderer failed\\n' >&2
          return "$TEST_DIALOG_STATUS"
        fi
      }
      osascript() {
        cat > "$TEST_DIRECTORY/fallback-script"
        printf '%s\\0' "$@" > "$TEST_DIRECTORY/fallback-args"
        if [ "$TEST_FALLBACK_STATUS" != 0 ];then
          printf 'fixture fallback renderer failed\\n' >&2
          return "$TEST_FALLBACK_STATUS"
        fi
      }
      open() { printf '%s\\0' "$@" > "$TEST_DIRECTORY/open-args"; }
      macos_start_cli
    `], {
      cwd: directory, encoding: "utf8", timeout: 10000,
      env: {
        ...process.env, TEST_DIRECTORY: directory, TEST_INSTALLER_STATUS: String(installerStatus),
        TEST_DIALOG_STATUS: String(dialogStatus), TEST_FALLBACK_STATUS: String(fallbackStatus),
        TEST_ABORT: abort ? "1" : "0", TEST_PROGRESS_STATUS: String(progressStatus),
        TEST_OWN_ERROR: ownError ? "1" : "0", PATH: `${directory}/bin:${process.env.PATH}`,
      },
    });
    const read = (name) => existsSync(join(directory, name)) ? readFileSync(join(directory, name), "utf8") : "";
    const args = (name) => read(name).split("\0").slice(0, -1);
    return {
      ...result, log: read("logs/run.log"), lastLog: read("last-run.log"),
      logMode: existsSync(join(directory, "logs/run.log")) ? statSync(join(directory, "logs/run.log")).mode & 0o777 : null,
      dialog: args("dialog-args"), fallback: args("fallback-args"), open: args("open-args"),
      fallbackScript: read("fallback-script"), notifications: read("notifications"), stopped: read("stopped"),
    };
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
}

describe("macOS progress-to-result handoff", () => {
  it("keeps a successful run log and passes every completion argument", () => {
    const result = runFlow();
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.log, /Simulated installer output/);
    assert.match(result.log, /Installer exit status: 0/);
    assert.match(result.log, /Result dialog closed normally/);
    assert.equal(result.lastLog, result.log);
    assert.equal(result.logMode, 0o600);
    assert.equal(result.dialog.length, 10);
    assert.match(result.dialog[3], /Process completed successfully/);
    assert.match(result.dialog[3], /Full log: /);
    assert.match(result.dialog[6], /next-steps\.png$/);
    assert.equal(result.dialog[9], "");
    assert.deepEqual(result.fallback, []);
    assert.equal(result.notifications, "success\n");
  });

  for (const installerStatus of [0, 42]) {
    it(`falls back visibly without changing installer status ${installerStatus}`, () => {
      const result = runFlow({ installerStatus, dialogStatus: 7 });
      assert.equal(result.status, installerStatus, result.stderr);
      assert.match(result.log, /fixture custom result renderer failed/);
      assert.match(result.log, /Custom result dialog failed \(status 7\)/);
      assert.match(result.log, /Fallback result dialog closed normally/);
      assert.equal(result.lastLog, result.log);
      assert.deepEqual(result.fallback.slice(0, 1), ["-"]);
      assert.equal(result.fallback[1], result.dialog[3]);
      assert.equal(result.fallback[3], installerStatus === 0 ? "Complete" : "OK");
      assert.match(result.fallbackScript, /on run argv/);
      assert.match(result.fallbackScript, /activate/);
      if (installerStatus !== 0) assert.doesNotMatch(result.fallback[1], /completed successfully|safe to remove/);
    });
  }

  it("opens the preserved log when both dialog renderers fail", () => {
    const result = runFlow({ installerStatus: 42, dialogStatus: 7, fallbackStatus: 8 });
    assert.equal(result.status, 42, result.stderr);
    assert.match(result.log, /fixture fallback renderer failed/);
    assert.match(result.log, /Fallback result dialog failed \(status 8\)/);
    assert.equal(result.lastLog, result.log);
    assert.equal(result.open[0], "-t");
    assert.match(result.open[1], /logs\/run\.log$/);
    assert.match(result.stderr, /Neither result dialog could be displayed/);
  });

  it("shows an aborted result with all seven JXA arguments", () => {
    const result = runFlow({ abort: true });
    assert.equal(result.status, 1, result.stderr);
    assert.equal(result.dialog.length, 10);
    assert.match(result.dialog[3], /Flashing was stopped before it finished/);
    assert.equal(result.dialog[6], "");
    assert.equal(result.dialog[9], "");
    assert.match(result.log, /Installer exit status: 1 \(interrupted\)/);
    assert.equal(result.notifications, "failure\n");
    assert.notEqual(result.stopped, "");
  });

  it("retains a progress crash diagnostic and does not blame the user", () => {
    const result = runFlow({ progressStatus: 97, dialogStatus: 7 });
    assert.equal(result.status, 1, result.stderr);
    assert.match(result.log, /Progress window exit status: 97/);
    assert.match(result.log, /fixture progress renderer failed/);
    assert.match(result.fallback[1], /progress window closed unexpectedly/);
    assert.doesNotMatch(result.stderr, /Aborting at your request/);
    assert.notEqual(result.stopped, "");
  });

  it("does not show a duplicate dialog after an engine-owned error was acknowledged", () => {
    const result = runFlow({ installerStatus: 1, ownError: true });
    assert.equal(result.status, 1, result.stderr);
    assert.match(result.log, /Installer exit status: 1/);
    assert.deepEqual(result.dialog, []);
    assert.deepEqual(result.fallback, []);
  });
});

describe("macOS completion argument bounds", () => {
  const start = completion[1].indexOf("const args =");
  const end = completion[1].indexOf("$.NSProcessInfo.processInfo.processName");
  assert.ok(start >= 0 && end > start);

  for (const optional of [[], ["", "test title", ""]]) {
    it(`accepts ${optional.length} optional arguments without a native array overrun`, () => {
      const args = ["osascript", "-l", "JavaScript", "-", "message", "icon", "title", ...optional];
      const context = vm.createContext({
        ObjC: { unwrap: (value) => value },
        $: { NSProcessInfo: { processInfo: { arguments: {
          count: String(args.length),
          objectAtIndex(index) {
            assert.ok(index < args.length, "Native argument array would crash");
            return args[index];
          },
        } } } },
      });
      vm.runInContext(completion[1].slice(start, end), context);
      assert.equal(vm.runInContext("settingsUrl", context), "");
      assert.equal(vm.runInContext("imagePath", context), "");
    });
  }
});
