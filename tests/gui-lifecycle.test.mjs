import { describe, it } from "node:test";
import assert from "node:assert/strict";
import {
  cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync,
} from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { spawn, spawnSync } from "node:child_process";

const root = fileURLToPath(new URL("../", import.meta.url));
const source = readFileSync(join(root, "install-wor-gui.sh"), "utf8");

function definition(text, name) {
  const match = text.match(new RegExp(`^${name}\\(\\) \\{[\\s\\S]*?^\\}`, "m"));
  assert.ok(match, `Missing ${name}`);
  return match[0];
}

async function fixture(callback, gui = source) {
  const directory = mkdtempSync(join(root, ".review-gui-lifecycle-"));
  try {
    writeFileSync(join(directory, "gui.sh"), gui);
    cpSync(join(root, "src/lib/cleanup.sh"), join(directory, "cleanup.sh"));
    return await callback(directory, readFileSync(join(directory, "gui.sh"), "utf8"));
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
}

function run(directory, script, args = [], extra = {}) {
  return spawnSync("/bin/bash", ["-c", script, "install-wor-gui.sh", ...args], {
    cwd: directory, encoding: "utf8", timeout: 10000,
    env: { ...process.env, NO_UPDATE: "1", TMPDIR: directory, TEST_DIR: directory, ...extra },
  });
}

const lifecycleNames = [
  "release_gui_instance", "acquire_gui_instance", "kill_process_tree",
  "gui_owned_child", "gui_stop_child", "gui_cleanup", "gui_handle_signal",
];
const lifecycle = (gui) => lifecycleNames.map((name) => definition(gui, name)).join("\n");

function editorEntry(directory, gui, editor = true) {
  const start = gui.indexOf("release_gui_instance() {");
  const dispatch = gui.indexOf('if [ "${1:-}" == --boot-config-editor ];then');
  const end = gui.indexOf("\nif is_macos ;then", dispatch);
  assert.ok(start >= 0 && dispatch > start && end > dispatch);
  const entry = gui.slice(start, end).replace(definition(gui, "linux_boot_config_editor"),
    'linux_boot_config_editor() { printf "EDITOR:%s:%s:%s\\n" "$1" "$2" "$3"; }');
  writeFileSync(join(directory, "entry.sh"), `activate_macos_gui() { :; }\n${entry}\nprintf 'WIZARD_REACHED\\n'\n`);
  return run(directory, `
    source "$TEST_DIR/cleanup.sh"
    ${lifecycle(gui)}
    acquire_gui_instance || exit 99
    /bin/bash "$TEST_DIR/entry.sh" ${editor ? '--boot-config-editor "pending config" 4 desktop' : ""}
    [ "$(cat "$WOR_GUI_INSTANCE_DIR/pid")" == "$$" ] || exit 98
  `);
}

describe("Linux internal boot editor entry", () => {
  it("opens the editor beneath a live wizard lock without releasing that lock", () => fixture((directory, gui) => {
    const result = editorEntry(directory, gui);
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.stdout, "EDITOR:pending config:4:desktop\n");
  }));

  it("still rejects a second wizard", () => fixture((directory, gui) => {
    const result = editorEntry(directory, gui, false);
    assert.equal(result.status, 0, result.stderr);
    assert.doesNotMatch(result.stdout, /WIZARD_REACHED|EDITOR:/);
  }));

  it("detects moving singleton acquisition ahead of internal dispatch in a disposable copy", () => {
    const mutant = source.replace('if [ "${1:-}" == --boot-config-editor ];then',
      'acquire_gui_instance || exit 0\nif [ "${1:-}" == --boot-config-editor ];then');
    return fixture((directory, gui) => {
      const result = editorEntry(directory, gui);
      assert.equal(result.status, 0, result.stderr);
      assert.throws(() => assert.match(result.stdout, /EDITOR:/));
    }, mutant);
  });
});

function configEntry(directory, gui, args) {
  const runtime = join(directory, "runtime");
  mkdirSync(runtime);
  for (const path of ["install-wor.sh", "src/lib", "src/config", "config-templates"]) {
    cpSync(join(root, path), join(runtime, path), { recursive: true });
  }
  const prefix = gui.slice(0, gui.indexOf("\n#outside the packaged .app"))
    .replace(definition(gui, "gui_runtime_complete"), "gui_runtime_complete() { return 0; }");
  writeFileSync(join(directory, "config with spaces.json"), '{"execution":{"dryRun":true}}\n');
  const env = { ...process.env };
  for (const name of ["WOR_CONFIG_FILE", "DRY_RUN", "DIRECTORY", "WOR_IMAGE_FAMILY"]) delete env[name];
  return spawnSync("/bin/bash", ["-c", `${prefix}\nprintf 'dry=%s\\n' "$DRY_RUN"`, "install-wor-gui.sh", ...args], {
    cwd: directory, encoding: "utf8", timeout: 10000,
    env: { ...env, DIRECTORY: runtime, NO_UPDATE: "1", WOR_CACHE_DIR: join(directory, "cache") },
  });
}

describe("Direct GUI configuration forwarding", () => {
  for (const args of [[], ["--config", "config with spaces.json"], ["--config=config with spaces.json"]]) {
    it(`uses the engine parser for ${JSON.stringify(args)}`, () => fixture((directory, gui) => {
      const result = configEntry(directory, gui, args);
      assert.equal(result.status, 0, result.stderr);
      assert.equal(result.stdout, `dry=${args.length ? "1" : "0"}\n`);
    }));
  }

  it("detects losing source-sentinel arguments in a disposable copy", () => {
    const mutant = source.replace('source "$cli_script" source "$@"', 'source "$cli_script" source');
    assert.notEqual(mutant, source);
    return fixture((directory, gui) => {
      const result = configEntry(directory, gui, ["--config", "config with spaces.json"]);
      assert.equal(result.status, 0, result.stderr);
      assert.equal(result.stdout, "dry=0\n");
      assert.throws(() => assert.equal(result.stdout, "dry=1\n"));
    }, mutant);
  });
});

function alive(pid) {
  if (!pid) return false;
  const result = spawnSync("ps", ["-p", String(pid), "-o", "stat="], { encoding: "utf8" });
  return result.status === 0 && !result.stdout.trim().startsWith("Z");
}

async function interruptedRun(directory, gui, signal) {
  mkdirSync(join(directory, "bin"));
  writeFileSync(join(directory, "bin/sudo"), "#!/bin/bash\nexit 95\n", { mode: 0o755 });
  const unrelated = spawn("/bin/sleep", ["20"], { stdio: "ignore" });
  const progressStart = gui.indexOf("    progress_status=0", gui.indexOf("macos_start_cli() {"));
  const progressEnd = gui.indexOf("\n    #Command-Q", progressStart);
  assert.ok(progressStart >= 0 && progressEnd > progressStart);
  const progress = gui.slice(progressStart, progressEnd).replace('    progress_pid=$!', `
    progress_pid=$!
    while [ ! -f "$TEST_DIR/installer-ready" ] || [ ! -f "$TEST_DIR/progress-ready" ] || [ ! -f "$TEST_DIR/alerts-ready" ];do sleep 0.02;done
    printf 'READY %s %s %s\\n' "$installer_pid" "$progress_pid" "$disk_alert_pid"
  `);
  assert.match(progress, /printf 'READY/);
  const script = `
    source "$TEST_DIR/cleanup.sh"
    ${lifecycle(gui)}
    acquire_gui_instance || exit 99
    worker() {
      trap - EXIT
      trap 'printf "stopped\\n" > "$TEST_DIR/$role-stopped"; exit 0' TERM
      local role="$1"
      : > "$TEST_DIR/$role-ready"
      while true;do sleep 0.02;done
    }
    abort_marker="$TEST_DIR/abort"
    worker installer & installer_pid=$!
    worker alerts & disk_alert_pid=$!
    tail_pid="$UNRELATED_PID"
    : > "$TEST_DIR/registered"
    register_file_cleanup "$TEST_DIR/registered"
    wor_osascript() { worker progress; }
    output_log="$TEST_DIR/progress-log"
    progress_jxa=fixture
    ${progress}
  `;
  const child = spawn("/bin/bash", ["-c", script, "install-wor-gui.sh"], {
    cwd: directory, stdio: ["ignore", "pipe", "pipe"],
    env: { ...process.env, TEST_DIR: directory, TMPDIR: directory, UNRELATED_PID: String(unrelated.pid),
      PATH: `${directory}/bin:${process.env.PATH}` },
  });
  let output = "", error = "", pids = [];
  child.stderr.on("data", (chunk) => { error += chunk; });
  const exited = new Promise((resolve) => child.once("exit", (code, exitSignal) => resolve({ code, exitSignal })));
  const closed = new Promise((resolve) => child.once("close", resolve));
  const timeout = setTimeout(() => child.kill("SIGTERM"), 7000);
  child.stdout.on("data", (chunk) => {
    output += chunk;
    const ready = output.match(/READY ([0-9]+) ([0-9]+) ([0-9]+)/);
    if (ready && pids.length === 0) {
      pids = ready.slice(1).map(Number);
      child.kill(signal);
    }
  });
  try {
    const status = await exited;
    return {
      ...status, error, survivors: pids.filter(alive), unrelatedAlive: alive(unrelated.pid),
      aborted: existsSync(join(directory, "abort")),
      registeredRemoved: !existsSync(join(directory, "registered")),
      lockRemoved: !existsSync(join(directory, `wor-flasher-gui-${process.getuid()}.lock`)),
    };
  } finally {
    clearTimeout(timeout);
    for (const pid of pids) if (alive(pid)) process.kill(pid, "SIGTERM");
    if (alive(unrelated.pid)) unrelated.kill("SIGTERM");
    await closed;
  }
}

function assertInterrupted(result, status) {
  assert.equal(result.code, status, result.error);
  assert.deepEqual(result.survivors, []);
  assert.equal(result.unrelatedAlive, true);
  assert.equal(result.aborted, true);
  assert.equal(result.registeredRemoved, true);
  assert.equal(result.lockRemoved, true);
}

describe("GUI interruption ownership and cleanup", () => {
  for (const [signal, status] of [["SIGTERM", 143], ["SIGHUP", 143], ["SIGINT", 130]]) {
    it(`stops owned workers on ${signal}, retains unrelated processes and runs registered cleanup`, () =>
      fixture(async (directory, gui) => assertInterrupted(await interruptedRun(directory, gui, signal), status)));
  }

  it("detects dropping installer shutdown in a disposable copy", () => {
    const mutant = source.replace('  gui_stop_child "${installer_pid:-}" 1\n', "");
    assert.notEqual(mutant, source);
    return fixture(async (directory, gui) => {
      const result = await interruptedRun(directory, gui, "SIGTERM");
      assert.throws(() => assertInterrupted(result, 143));
      assert.equal(result.survivors.length, 1);
    }, mutant);
  });
});

function linuxResult(directory, gui, installerStatus, ownError) {
  const start = gui.lastIndexOf('exitcode="$(cat "$done_marker"');
  assert.ok(start >= 0);
  writeFileSync(join(directory, "done"), `${installerStatus}\n`);
  return run(directory, `
    done_marker="$TEST_DIR/done"; progress_fifo="$TEST_DIR/fifo"; progress_file="$TEST_DIR/progress"
    auth_marker="$TEST_DIR/auth"; abort_marker="$TEST_DIR/abort"; error_marker="$TEST_DIR/error"
    output_log="$TEST_DIR/output"; DL_DIR="$TEST_DIR/downloads"; tail_pid=unused; yad_pid=unused
    kill() { :; }
    wait() { :; }
    gui_save_installer_log() { printf 'fixture-log\\n'; }
    wor_play_result_sound() { :; }
    wor_show_result_notification() { :; }
    windows_version_label() { printf 'Windows 11'; }
    installer_showed_own_error() { return ${ownError ? 0 : 1}; }
    yad() { return 7; }
    wor_yad_image_for_screen() { printf 'fixture-image'; }
    is_iot_core() { return 1; }
    linux_show_completion_dialog() { return 0; }
    ${gui.slice(start)}
  `);
}

describe("Linux GUI installer result status", () => {
  for (const status of [0, 23]) {
    for (const ownError of [false, true]) {
      it(`preserves installer status ${status} with engine-owned error ${ownError}`, () => fixture((directory, gui) => {
        const result = linuxResult(directory, gui, status, ownError);
        assert.equal(result.status, status, result.stderr);
      }));
    }
  }

  it("detects losing the final installer status in a disposable copy", () => {
    const mutant = source.replace(/\nexit "\$exitcode"\s*$/, "\n");
    assert.notEqual(mutant, source);
    return fixture((directory, gui) => {
      const result = linuxResult(directory, gui, 23, true);
      assert.equal(result.status, 0, result.stderr);
      assert.throws(() => assert.equal(result.status, 23));
    }, mutant);
  });
});
