import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";

const root = fileURLToPath(new URL("../", import.meta.url));
const gui = readFileSync(join(root, "install-wor-gui.sh"), "utf8");
const functions = ["gui_start_installer", "gui_save_failure_log", "macos_password_retry_dialog"].map((name) => {
  const match = gui.match(new RegExp(`^${name}\\(\\) \\{[\\s\\S]*?^\\}`, "m"));
  assert.ok(match, `Missing ${name}`);
  return match[0];
}).join("\n");

describe("macOS GUI failed-then-successful password retry", () => {
  for (const first of ["rejected", "canceled", "empty"]) {
    it(`recovers from ${first} input with one fresh prompt and advances progress`, () => {
      const directory = mkdtempSync(join(tmpdir(), "wor-password-retry-"));
      try {
        for (const path of ["bin", "tmp", "windows/bootpart", "peinstaller/winpe/2", "peinstaller/efi"]) {
          mkdirSync(join(directory, path), { recursive: true });
        }
        writeFileSync(join(directory, "gui-functions.sh"), functions);
        writeFileSync(join(directory, "bin/mktemp"), `#!/bin/bash
#macOS mktemp without a template can prefer its system temp directory over TMPDIR.
if [ "$#" == 0 ];then
  exec /usr/bin/mktemp "$WOR_TEST_DIR/tmp/tmp.XXXXXX"
elif [ "$#" == 1 ] && { [ "$1" == -d ] || [ "$1" == -u ]; };then
  exec /usr/bin/mktemp "$1" "$WOR_TEST_DIR/tmp/tmp.XXXXXX"
fi
exec /usr/bin/mktemp "$@"
`, { mode: 0o755 });
        writeFileSync(join(directory, "metadata.sh"), `
wor_osascript() {
  printf 'prompt\\n' >> "$WOR_TEST_DIR/prompts"
  case "$WOR_TEST_PASSWORD" in
    rejected) printf 'invalid-fixture-answer\\n' ;;
    correct) printf 'valid-fixture-answer\\n' ;;
    canceled) printf 'execution error: User canceled. (-128)\\n' >&2; return 1 ;;
    empty) printf '\\n' ;;
    *) return 99 ;;
  esac
}
`);
        writeFileSync(join(directory, "bin/sudo"), `#!/bin/bash
if [ "$*" == '-A -v' ];then
  printf '%s\\n' "$WOR_GUI_ASKPASS_STATE" >> "$WOR_TEST_DIR/states"
  printf '%s\\n' "$WOR_RESUME_AT_FLASH" >> "$WOR_TEST_DIR/resume"
  for attempt in 1 2 3;do
    answer="$("$SUDO_ASKPASS")"
    status=$?
    if [ "$status" != 0 ] || [ -z "$answer" ];then
      printf 'sudo: no password was provided\\n' >&2
      exit 1
    fi
    if [ "$answer" == valid-fixture-answer ];then
      printf '%s\\n' "$PPID" > "$WOR_TEST_DIR/authorized-parent"
      exit 0
    fi
    printf 'Sorry, try again.\\n' >&2
  done
  printf 'sudo: 3 incorrect password attempts\\n' >&2
  exit 1
fi
parent=''
[ ! -r "$WOR_TEST_DIR/authorized-parent" ] || read -r parent < "$WOR_TEST_DIR/authorized-parent"
[ "$parent" == "$PPID" ] || exit 1
[ "$*" != '-n -v' ] || exit 0
[ "$1" == -n ] || exit 99
shift
[ "$#" == 14 ] && [ "$1" == bash ] && [ "$2" == -c ] && [ "$4" == wor-partition-finalizer ] || exit 99
#Simulate only the worker handshake; never execute its privileged command argument.
printf 'ready\\n' > "\${11}"
for attempt in {1..100};do
  if [ -e "$8" ];then
    printf '0\\n' > "$9"
    exit 0
  fi
  kill -0 "\${10}" 2>/dev/null || exit 1
  sleep 0.1
done
printf 'Mock worker timed out\\n' >&2
exit 124
`, { mode: 0o755 });
        writeFileSync(join(directory, "bin/sgdisk"), '#!/bin/bash\nprintf "Unexpected disk command\\n" >&2\nexit 99\n', { mode: 0o755 });
        writeFileSync(join(directory, "installer.sh"), `#!/bin/bash
source "$WOR_TEST_ROOT/install-wor.sh" source >/dev/null
HOST_OS=Darwin RUN_MODE=gui DEVICE=/dev/mock-target RPI_MODEL=4 CAN_INSTALL_ON_SAME_DRIVE=1
winfiles=windows STEP_NUM=4 STEP_TOTAL=8 MACOS_ASKPASS=''
WOR_METADATA_FILE="$WOR_TEST_DIR/metadata.sh"
export WOR_METADATA_FILE
is_safe_target_device() { [ "$1" == /dev/mock-target ]; }
darwin_prepare_disk_or_die() {
  command sudo -n -v || exit 98
  printf 'prepared\\n' >> "$WOR_TEST_DIR/prepared"
  darwin_finalize_partition_types_or_die
  exit 0
}
darwin_flash_device
`, { mode: 0o755 });
        const result = spawnSync("bash", ["-c", `
          source "$WOR_TEST_ROOT/install-wor.sh" source >/dev/null
          source "$WOR_TEST_DIR/gui-functions.sh"
          HOST_OS=Darwin RUN_MODE=gui DEVICE=/dev/mock-target GUI_PROGRESS_EARLY=1
          DL_DIR="$WOR_TEST_DIR/downloads" WOR_ICON_PATH=mock-icon
          cli_script="$WOR_TEST_DIR/installer.sh"
          gui_start_disk_alert_handler() { disk_alert_pid=''; }
          macos_choose() { printf '%s\\n' "$2" > "$WOR_TEST_DIR/retry-dialog"; printf 'retry\\n'; }
          resume_at_flash=0
          gui_start_installer
          wait "$installer_pid"
          first_status="$(cat "$done_marker")"
          cp "$progress_file" "$WOR_TEST_DIR/first-progress"
          saved_log="$(gui_save_failure_log)"
          cp "$saved_log" "$WOR_TEST_DIR/first-log"
          choice="$(macos_password_retry_dialog "$saved_log" "$progress_file")"
          [ "$choice" == retry ] || exit 97
          resume_at_flash=1
          export WOR_TEST_PASSWORD=correct
          gui_start_installer
          wait "$installer_pid"
          second_status="$(cat "$done_marker")"
          cp "$progress_file" "$WOR_TEST_DIR/second-progress"
          cp "$output_log" "$WOR_TEST_DIR/second-log"
          printf '%s|%s\\n' "$first_status" "$second_status"
        `], {
          cwd: directory, encoding: "utf8", timeout: 20000,
          env: {
            ...process.env, NO_UPDATE: "1", DIRECTORY: root, WOR_TEST_ROOT: root,
            WOR_TEST_DIR: directory, WOR_TEST_PASSWORD: first, TMPDIR: join(directory, "tmp"),
            PATH: `${directory}/bin:${process.env.PATH}`,
          },
        });
        assert.equal(result.status, 0, result.stderr);
        assert.equal(result.stdout.trim(), "1|0");
        assert.equal(readFileSync(join(directory, "prompts"), "utf8"), "prompt\nprompt\n");
        assert.equal(readFileSync(join(directory, "resume"), "utf8"), "0\n1\n");
        assert.equal(readFileSync(join(directory, "prepared"), "utf8"), "prepared\n");
        const firstProgress = readFileSync(join(directory, "first-progress"), "utf8");
        assert.doesNotMatch(firstProgress, /^DISK_WRITE\t1\t/m);
        const secondProgress = readFileSync(join(directory, "second-progress"), "utf8");
        assert.match(secondProgress, /^DISK_WRITE\t1\t/m);
        assert.equal(secondProgress.split("\n").filter((line) => line.startsWith("TASK\t")).at(-1),
          "TASK\t0\tPreparing the target disk...");
        const states = readFileSync(join(directory, "states"), "utf8").trim().split("\n");
        assert.equal(states.length, 2);
        assert.notEqual(states[0], states[1]);
        for (const state of states) {
          assert.ok(state.startsWith(join(directory, "tmp")), `Prompt state ${state} was not created under ${join(directory, "tmp")}`);
          assert.equal(existsSync(state), false);
        }
        const logs = readFileSync(join(directory, "first-log"), "utf8") + readFileSync(join(directory, "second-log"), "utf8");
        assert.doesNotMatch(logs + result.stdout + result.stderr, /invalid-fixture-answer|valid-fixture-answer/);
        assert.match(readFileSync(join(directory, "second-log"), "utf8"), /Administrator access granted\./);
        if (first === "rejected") {
          assert.match(readFileSync(join(directory, "retry-dialog"), "utf8"), /^The administrator password was not accepted\./);
        }
      } finally {
        rmSync(directory, { recursive: true, force: true });
      }
    });
  }
});
