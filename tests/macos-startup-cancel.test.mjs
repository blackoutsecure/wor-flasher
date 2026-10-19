import { describe, it } from "node:test";
import assert from "node:assert/strict";
import {
  existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync,
} from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { spawn, spawnSync } from "node:child_process";

const root = fileURLToPath(new URL("../", import.meta.url));
const source = readFileSync(join(root, "src/macos-app/Contents/MacOS/WoR-Flasher"), "utf8");
const delay = (milliseconds) => new Promise((resolve) => setTimeout(resolve, milliseconds));

function alive(pid) {
  if (!pid) return false;
  const result = spawnSync("ps", ["-p", String(pid), "-o", "stat="], { encoding: "utf8" });
  return result.status === 0 && !result.stdout.trim().startsWith("Z");
}

async function startup({ checkout = false, block = "", cancel = false, launcher = source } = {}) {
  const directory = mkdtempSync(join(root, ".review-macos-startup-"));
  let child;
  let windowPid = 0;
  let taskPid = 0;
  let taskChildPid = 0;
  try {
    const copied = launcher
      .replace('  /usr/bin/osascript -l JavaScript - "$STARTUP_STATUS"',
        '  "$TEST_OSASCRIPT" -l JavaScript - "$STARTUP_STATUS"')
      .replace(/^  STARTUP_STATUS="\$\(mktemp[^\n]*$/m, '  STARTUP_STATUS="$TEST_DIR/status"');
    assert.notEqual(copied, launcher);
    writeFileSync(join(directory, "launcher.sh"), copied);
    writeFileSync(join(directory, "window.sh"), `#!/bin/bash
cat >/dev/null
printf '%s\\n' "$$" > "$TEST_DIR/window-pid"
: > "$TEST_DIR/window-ready"
while [ ! -e "$5" ];do
  [ ! -e "$TEST_DIR/close-window" ] || exit 0
  sleep 0.02
done
`, { mode: 0o755 });
    const gui = '#!/bin/bash\nprintf "gui\\n" >> "$TEST_DIR/calls"\nprintf "%s\\n" "$PATH" > "$TEST_DIR/gui-path"\n';
    writeFileSync(join(directory, "gui.sh"), gui, { mode: 0o755 });
    mkdirSync(join(directory, "checkout"));
    if (checkout) mkdirSync(join(directory, "checkout/.git"));
    writeFileSync(join(directory, "checkout/install-wor-gui.sh"), gui, { mode: 0o755 });
    const script = `
      source "$TEST_DIR/launcher.sh"
      CHECKOUT_DIR="$TEST_DIR/checkout"
      mock_phase() {
        local phase="$1"
        printf '%s\\n' "$phase" >> "$TEST_DIR/calls"
        if [ "$phase" == "$TEST_BLOCK" ];then
          /bin/sh -c 'printf "%s\\n" "$PPID"' > "$TEST_DIR/task-pid"
          sleep 20 &
          printf '%s\\n' "$!" > "$TEST_DIR/task-child-pid"
          : > "$TEST_DIR/phase-ready"
          wait "$!"
        fi
      }
      repair_missing_files() { mock_phase repair; }
      sync_bundle_metadata() { mock_phase sync; }
      bootstrap_embedded_runtime() { mock_phase bootstrap; }
      select_runtime() { mock_phase select; GUI_SCRIPT="$TEST_DIR/gui.sh"; }
      load_metadata() { mock_phase metadata; WOR_ICON_FILENAME=fixture-icon; }
      check_for_runtime_update() { mock_phase update; }
      prepare_dependencies() { mock_phase dependencies; }
      show_error() { printf 'UNEXPECTED ERROR: %s\\n' "$*" >&2; return 1; }
      launcher_main
    `;
    child = spawn("/bin/bash", ["-c", script], {
      cwd: directory, stdio: ["ignore", "pipe", "pipe"],
      env: { ...process.env, WOR_LAUNCHER_SOURCE_ONLY: "1", TEST_DIR: directory,
        TEST_OSASCRIPT: join(directory, "window.sh"), TEST_BLOCK: block, TMPDIR: directory },
    });
    let error = "";
    child.stderr.on("data", (chunk) => { error += chunk; });
    child.stdout.resume();
    const exited = new Promise((resolve) => child.once("exit", (code, signal) => resolve({ code, signal })));
    const closed = new Promise((resolve) => child.once("close", resolve));
    if (cancel) {
      for (let attempt = 0; attempt < 250; attempt++) {
        if (existsSync(join(directory, "window-ready")) && existsSync(join(directory, "phase-ready"))) break;
        await delay(20);
      }
      assert.equal(existsSync(join(directory, "window-ready")), true, error);
      assert.equal(existsSync(join(directory, "phase-ready")), true, error);
      windowPid = Number(readFileSync(join(directory, "window-pid"), "utf8"));
      taskPid = Number(readFileSync(join(directory, "task-pid"), "utf8"));
      taskChildPid = Number(readFileSync(join(directory, "task-child-pid"), "utf8"));
      writeFileSync(join(directory, "close-window"), "");
    }
    let automatic = true;
    const timeout = setTimeout(() => { automatic = false; child.kill("SIGTERM"); }, cancel ? 2500 : 7000);
    const status = await exited;
    clearTimeout(timeout);
    await closed;
    const calls = readFileSync(join(directory, "calls"), "utf8").trim().split("\n");
    return {
      ...status, error, automatic, calls,
      workerAlive: alive(taskPid), windowAlive: alive(windowPid),
      taskChildAlive: alive(taskChildPid),
      stateRemoved: !existsSync(join(directory, "status")) && !existsSync(join(directory, "status.done")),
      guiPath: existsSync(join(directory, "gui-path")) ? readFileSync(join(directory, "gui-path"), "utf8") : "",
    };
  } finally {
    for (const pid of [taskChildPid, taskPid, windowPid]) if (alive(pid)) process.kill(pid, "SIGTERM");
    if (child?.exitCode === null && child.signalCode === null) child.kill("SIGTERM");
    rmSync(directory, { recursive: true, force: true });
  }
}

function assertCanceled(result) {
  assert.equal(result.automatic, true, "Closing startup must cancel without another signal.");
  assert.equal(result.code, 143, result.error);
  assert.equal(result.workerAlive, false);
  assert.equal(result.taskChildAlive, false);
  assert.equal(result.windowAlive, false);
  assert.equal(result.stateRemoved, true);
  assert.ok(!result.calls.includes("gui"));
}

describe("macOS launcher startup cancellation", () => {
  for (const [block, checkout, expected] of [
    ["repair", true, ["repair"]],
    ["sync", true, ["repair", "metadata", "sync"]],
    ["bootstrap", false, ["bootstrap"]],
    ["update", false, ["bootstrap", "select", "metadata", "update"]],
    ["dependencies", false, ["bootstrap", "select", "metadata", "update", "select", "metadata", "dependencies"]],
  ]) {
    it(`stops the owned ${block} task when the startup window closes`, async () => {
      const result = await startup({ block, checkout, cancel: true });
      assertCanceled(result);
      assert.deepEqual(result.calls, expected);
    });
  }

  for (const checkout of [true, false]) {
    it(`retains the ${checkout ? "checkout repair" : "verified runtime update"} sequence on normal startup`, async () => {
      const result = await startup({ checkout });
      assert.equal(result.code, 0, result.error);
      assert.deepEqual(result.calls, checkout
        ? ["repair", "metadata", "sync", "dependencies", "gui"]
        : ["bootstrap", "select", "metadata", "update", "select", "metadata", "dependencies", "gui"]);
      assert.match(result.guiPath, /^\/opt\/homebrew\/bin:\/usr\/local\/bin:/);
      assert.equal(result.stateRemoved, true);
    });
  }

  it("detects losing startup-close propagation in a disposable launcher copy", async () => {
    const mutant = source.replace('    [ -e "$STARTUP_DONE" ] || kill -TERM "$$"', "    :");
    assert.notEqual(mutant, source);
    const result = await startup({ block: "update", cancel: true, launcher: mutant });
    assert.equal(result.automatic, false);
    assert.throws(() => assertCanceled(result));
  });
});
