import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { existsSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";
import vm from "node:vm";

const root = fileURLToPath(new URL("../", import.meta.url)).replace(/\/$/, "");
const gui = readFileSync(join(root, "install-wor-gui.sh"), "utf8");
const hash = "1".repeat(64);
const metadata = JSON.stringify({
  architecture: "arm32", profile: "iot-core", build: "17763.107", format: "ffu-v1",
  sector_size: 512, trust: "pinned-official-sha256", sha256: hash,
  recommended_boards: ["pi2-v1.1", "pi2-v1.2", "pi3-b"], minimum_disk_bytes: 3774873600,
});
const start = gui.indexOf("gui_iot_validate_target() {");
const end = gui.indexOf("\nmacos_start_cli() {", start);
assert.ok(start >= 0 && end > start);
const helpers = gui.slice(start, end);
const support = ["kill_process_tree", "gui_update_last_log", "gui_save_installer_log", "gui_log_tail"].map((name) => {
  const match = gui.match(new RegExp(`^${name}\\(\\) \\{[\\s\\S]*?^\\}`, "m"));
  assert.ok(match, `Missing ${name}`);
  return match[0];
}).join("\n");
const native = helpers.match(/options_jxa="\$\(wor_jxa_window_lib; cat <<'JXA'\n([\s\S]*?)\nJXA/)[1];
const linuxTarget = gui.match(/^linux_choose_target\(\) \{[\s\S]*?^\}/m);
assert.ok(linuxTarget, "Missing combined Linux target picker");
const flowStart = gui.indexOf("  step=target", gui.indexOf("macos_start_cli() {"));
const flowEnd = gui.indexOf("\n  completion_jxa=", flowStart);
assert.ok(flowStart >= 0 && flowEnd > flowStart);
const selectionFlow = gui.slice(flowStart, flowEnd);
const selectionStubs = `
macos_choose_target() { printf 'Windows 10 IoT Core (ARM32, legacy)\\tRaspberry Pi 3 Model B\\n'; }
macos_choose_device() { printf '/dev/mock-only\\tFixture disk\\n'; }
darwin_list_device_choices() { printf '/dev/mock-only\\tFixture disk\\n'; }
is_safe_target_device() { return 0; }
get_size_raw() { printf '16000000000\\n'; }
iot_core_validate_device() {
  [ "$1" == preview ] || { printf 'UNEXPECTED_FULL_IMAGE_VALIDATION\\n' >&2; return 91; }
  printf 'preview\\0' >> "$TEST_DIR/fields"
}
`;

function run(body, { host = "Darwin" } = {}) {
  const directory = realpathSync(mkdtempSync(join(tmpdir(), "wor-iot-options-")));
  try {
    writeFileSync(join(directory, "prepared.ffu"), "Routing fixture, never flashed.\n");
    writeFileSync(join(directory, "local image.iso"), "Routing fixture, never flashed.\n");
    const result = spawnSync("bash", ["-c", `
      source "$DIRECTORY/install-wor.sh" source >/dev/null || exit 90
      ${support}
      ${helpers}
      ${linuxTarget[0]}
      HOST_OS="$TEST_HOST" RUN_MODE=cli WOR_IMAGE_FAMILY=iot-core WOR_TARGET_BOARD=pi3-b RPI_MODEL=3
      SOURCE_FILE="$PWD/prepared.ffu" GUI_IOT_SOURCE_MODE=official GUI_IOT_LOCAL_SOURCE=''
      IOT_CORE_SHA256="${hash}" IOT_CORE_BUILD=17763.107 IOT_CORE_MINIMUM_BYTES=3774873600
      IOT_CORE_ACQUISITION=download IOT_CORE_INSPECTION_JSON=approved
      IOT_CORE_TARGET_ID=1:2:3 IOT_CORE_TARGET_BYTES=16000000000 WOR_IOT_CONFIRM_ERASE=1
      WOR_IOT_DOWNLOAD=0 DRY_RUN=0 PLAY_SOUND=1 SHOW_NOTIFICATION=1
      IOT_CORE_HDMI_MODE=official IOT_CORE_HDMI_CONFIG=''
      IOT_CORE_ACCOUNT_SETUP=0 IOT_CORE_ACCOUNT_USERNAME=Administrator IOT_CORE_ACCOUNT_PASSWORD=''
      IOT_CORE_LANGUAGE_SETUP=0 IOT_CORE_LANGUAGE=en-US
      WOR_YAD_SCREEN_WIDTH=1920 WOR_YAD_SCREEN_HEIGHT=1080
      COMPLETION_SOUND=Glass
      [ "$TEST_HOST" != Linux ] || COMPLETION_SOUND=complete
      wor_host_platform() { [ "$TEST_HOST" == Darwin ] && printf macos || printf linux; }
      wor_jxa_window_lib() { :; }
      wor_osascript() {
        cat >/dev/null
        printf '__WOR_CANCEL__\\n'
      }
      yad() { cat > "$TEST_DIR/progress"; }
      python3() {
        if [ "\${1##*/}" == iot-account.py ];then command python3 "$@"; return;fi
        if [ "$2" == profile ] || [ "$2" == hdmi-options ];then
          command python3 "$@"
          return
        fi
        printf '%s\\0' "$@" > "$TEST_DIR/worker-args"
        jq --arg source "$PWD/prepared.ffu" '. + {source_file: $source, acquisition: "download"}' <<'JSON'
${metadata}
JSON
      }
      sudo() { printf 'UNEXPECTED_PRIVILEGE\\n' >&2; return 95; }
      iot_core_apply() { printf 'UNEXPECTED_DEVICE_WRITE\\n' >&2; return 96; }
      ${body}
    `], {
      cwd: directory, encoding: "utf8", timeout: 20000,
      env: {
        ...process.env, NO_UPDATE: "1", DIRECTORY: root, WOR_CACHE_DIR: join(directory, "tool-cache"),
        DL_DIR: directory, TEST_DIR: directory, TEST_HOST: host, WOR_LOG_FILE: join(directory, "run.log"),
      },
    });
    const read = (name) => existsSync(join(directory, name)) ? readFileSync(join(directory, name), "utf8") : "";
    return {
      ...result, directory, args: read("worker-args").split("\0").slice(0, -1),
      progress: read("progress"), trace: read("trace").trim().split("\n").filter(Boolean),
      fields: read("fields").split("\0").filter(Boolean),
      rawFields: read("fields").split("\0").slice(0, -1),
      firstFields: read("first-fields").split("\0").filter(Boolean),
    };
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
}

const options = (changes = "") => `gui_iot_options_state | jq '${changes || "."}'`;

describe("Official IoT image wizard default", () => {
  for (const [size, expected] of [[3774873599, "too-small"], [3774873600, "install"]]) {
    it(`checks the exact planned capacity threshold at ${size} bytes without an image`, () => {
      const result = run(`
        SOURCE_FILE='' IOT_CORE_MINIMUM_BYTES='' IOT_CORE_SHA256=''
        gui_iot_plan_source || exit 91
        get_size_raw() { printf '${size}\\n'; }
        drive_capability /dev/mock-only
      `);
      assert.equal(result.status, 0, result.stderr);
      assert.equal(result.stdout, `${expected}\n`);
      assert.deepEqual(result.args, []);
    });
  }

  for (const host of ["Darwin", "Linux"]) {
    it(`queues the official image without acquiring or verifying bytes on ${host}`, () => {
      const result = run(`
        SOURCE_FILE='' IOT_CORE_SHA256='' GUI_IOT_SOURCE_MODE=''
        IOT_CORE_BUILD='' IOT_CORE_MINIMUM_BYTES=''
        gui_iot_plan_source || exit 91
        printf '%s|%s|%s|%s\\n' "$GUI_IOT_SOURCE_MODE" "$WOR_IOT_DOWNLOAD" "$IOT_CORE_BUILD" "$IOT_CORE_SHA256"
        iot_core_summary
      `, { host });
      assert.equal(result.status, 0, result.stderr);
      assert.match(result.stdout, /^official\|1\|\|\n/);
      assert.match(result.stdout, /build 17763\.107 \(planned\)/);
      assert.match(result.stdout, /Image SHA-256\tNot Assessed/);
      assert.match(result.stdout, /Source validation\tNot Assessed.*after Flash/);
      assert.deepEqual(result.args, []);
      assert.doesNotMatch(result.stdout + result.stderr, /UNEXPECTED_/);
    });

    it(`preserves an explicitly configured local image on ${host}`, () => {
      const result = run(`
        GUI_IOT_SOURCE_MODE='' SOURCE_FILE="$PWD/local image.iso" IOT_CORE_SHA256=''
        gui_iot_plan_source || exit 91
        printf '%s|%s\\n' "$GUI_IOT_SOURCE_MODE" "$GUI_IOT_LOCAL_SOURCE"
      `, { host });
      assert.equal(result.status, 0, result.stderr);
      assert.equal(result.stdout, `local|${join(result.directory, "local image.iso")}\n`);
      assert.deepEqual(result.args, []);
    });
  }

  it("runs the actual macOS target step without any source-selection screen", () => {
    const flow = gui.slice(gui.indexOf("macos_start_cli() {"));
    const target = flow.slice(flow.indexOf("      target)"), flow.indexOf("      language)"));
    const result = run(`
      WOR_IMAGE_FAMILY=desktop SOURCE_FILE=desktop.iso GUI_IOT_SOURCE_MODE=local
      macos_choose_target() { printf 'Windows 10 IoT Core (ARM32, legacy)\\tRaspberry Pi 3 Model B\\n'; }
      macos_choose() { printf 'UNEXPECTED_SOURCE_SCREEN\\n' >&2; exit 92; }
      step=target
      while true;do
        case "$step" in
${target}
          device) printf 'PLANNED|%s|%s\\n' "$WOR_TARGET_BOARD" "$IOT_CORE_SHA256"; break ;;
        esac
      done
    `);
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.stdout, "PLANNED|pi3-b|\n");
    assert.deepEqual(result.args, []);
    assert.doesNotMatch(result.stderr, /UNEXPECTED_/);
  });

  it("does not acquire the image in the Linux board-selection step", () => {
    const start = gui.indexOf("case \"$WINDOWS_VER\" in", gui.indexOf("{ #choose destination RPi model"));
    const end = gui.indexOf("    'Windows 11' | 'Windows 10')", start);
    const branch = gui.slice(start, end);
    const result = run(`
      select_windows_family 'Windows 10 IoT Core (ARM32, legacy)'
      SOURCE_FILE='' IOT_CORE_SHA256=''
      WINDOWS_VER='Windows 10 IoT Core (ARM32, legacy)'
      yad() {
        for argument in "$@";do
          if [ "$argument" == --form ];then
            printf '%s\\0' "$@" > "$TEST_DIR/fields"
            printf 'Raspberry Pi 3 Model B\\n'
            return 0
          fi
        done
        cat > "$TEST_DIR/progress"
      }
${branch}
      esac
      printf 'PLANNED|%s\\n' "$IOT_CORE_SHA256"
    `, { host: "Linux" });
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.stdout, "PLANNED|\n");
    assert.deepEqual(result.args, []);
    assert.deepEqual(result.fields, []);
    assert.ok(!result.fields.some((field) => /:FL|download.*:CHK|inspect.*:CHK/i.test(field)));
  });

  it("Cancel on review never starts image acquisition", () => {
    const result = run(`
      WOR_IMAGE_FAMILY=desktop SOURCE_FILE=desktop.iso GUI_IOT_SOURCE_MODE=local IOT_CORE_SHA256=''
      DEVICE=''
      ${selectionStubs}
      macos_confirm_flash() { printf 'Cancel\\n'; }
      ${selectionFlow}
    `);
    assert.equal(result.status, 0, result.stderr);
    assert.deepEqual(result.args, []);
    assert.deepEqual(result.fields, ["preview"]);
    assert.doesNotMatch(result.stderr, /UNEXPECTED_/);
  });

  describe("Common Linux Windows/board selection", () => {
    it("selects an IoT board in the same two-field target form without a second screen", () => {
      const result = run(`
        yad() {
          printf '%s\\0' "$@" > "$TEST_DIR/fields"
          printf 'Windows 10 IoT Core (ARM32, legacy)\\nRaspberry Pi 2 v1.1\\n'
        }
        linux_choose_target 'Windows 10 IoT Core (ARM32, legacy)' 'Raspberry Pi 3 Model B'
      `, { host: "Linux" });
      assert.equal(result.status, 0, result.stderr);
      assert.equal(result.stdout, "Windows 10 IoT Core (ARM32, legacy)\tRaspberry Pi 2 v1.1\n");
      assert.equal(result.fields.filter((field) => field.startsWith("--field=")).length, 2);
      const boardField = result.fields.indexOf("--field=Raspberry Pi model:CB");
      assert.equal(result.fields[boardField + 1], "Raspberry Pi 3 Model B!Raspberry Pi 2 v1.2!Raspberry Pi 2 v1.1");
      assert.deepEqual(result.args, []);
      assert.doesNotMatch(gui, /iot_choice=|Supported IoT Core board:CB/);
    });

    describe("Actionable IoT target rejection", () => {
      for (const host of ["Darwin", "Linux"]) {
        it(`shows detailed metadata/safety reasons and offers another drive on ${host} without writing`, () => {
          const result = run(`
            DEVICE=/dev/mock-target WOR_TARGET_BOARD=pi2-v1.1
            iot_core_validate_device() {
              warning 'Fixture GPT/device metadata is inconsistent. No automatic cleanup is safe; no metadata was erased.'
              return 1
            }
            macos_choose() { printf '%s\\0' "$@" > "$TEST_DIR/fields"; printf '__RETRY__\\n'; }
            yad() { printf '%s\\0' "$@" > "$TEST_DIR/fields"; return 0; }
            status=0
            gui_iot_validate_target || status=$?
            [ "$status" == 2 ] || exit 92
          `, { host });
          assert.equal(result.status, 0, result.stderr);
          const dialog = result.fields.join("\n");
          assert.match(dialog, /GPT\/device metadata is inconsistent/);
          assert.match(dialog, /no metadata was erased/);
          assert.match(dialog, /No image was downloaded and no drive was modified/);
          assert.match(dialog, /Choose another drive/);
          assert.doesNotMatch(dialog, /\x1b|Exiting now/);
          assert.deepEqual(result.args, []);
          assert.doesNotMatch(result.stderr, /UNEXPECTED_PRIVILEGE|UNEXPECTED_DEVICE_WRITE/);
        });

        it(`preserves successful target binding in the owning ${host} shell`, () => {
          const result = run(`
            iot_core_validate_device() { IOT_CORE_TARGET_ID=7:8:9; IOT_CORE_TARGET_BYTES=62226694144; }
            macos_choose() { printf 'UNEXPECTED_DIALOG\\n' >&2; return 91; }
            yad() { printf 'UNEXPECTED_DIALOG\\n' >&2; return 92; }
            gui_iot_validate_target || exit 93
            printf '%s|%s\\n' "$IOT_CORE_TARGET_ID" "$IOT_CORE_TARGET_BYTES"
          `, { host });
          assert.equal(result.status, 0, result.stderr);
          assert.equal(result.stdout, "7:8:9|62226694144\n");
          assert.doesNotMatch(result.stderr, /UNEXPECTED_/);
        });

        it(`allows Cancel on ${host} without changing a rejected target`, () => {
          const result = run(`
            iot_core_validate_device() { warning 'Fixture target uses unsupported geometry.'; return 1; }
            macos_choose() { return 1; }
            yad() { return 1; }
            status=0
            gui_iot_validate_target || status=$?
            [ "$status" == 1 ] || exit 91
          `, { host });
          assert.equal(result.status, 0, result.stderr);
          assert.deepEqual(result.args, []);
        });
      }

      it("keeps a non-GPT failure specific instead of labeling every failure as a layout issue", () => {
        const result = run(`
          iot_core_validate_device() { warning '512-byte sectors are required; detected 4096.'; return 1; }
          macos_choose() { printf '%s\\0' "$@" > "$TEST_DIR/fields"; printf '__RETRY__\\n'; }
          status=0
          gui_iot_validate_target || status=$?
          [ "$status" == 2 ] || exit 91
        `);
        const dialog = result.fields.join("\n");
        assert.equal(result.status, 0, result.stderr);
        assert.match(dialog, /512-byte sectors.*4096/);
        assert.doesNotMatch(dialog, /GPT|blank\/MBR/);
      });

      it("returns the macOS wizard to drive selection after rejection", () => {
        const result = run(`
          SOURCE_FILE='' IOT_CORE_SHA256='' DEVICE=''
          ${selectionStubs}
          macos_choose_device() {
            if [ ! -e "$TEST_DIR/retried" ];then printf '/dev/bad\\tGPT fixture\\n';else printf '/dev/good\\tMBR fixture\\n';fi
          }
          iot_core_validate_device() {
            if [ "$DEVICE" == /dev/bad ];then warning 'Fixture GPT target is rejected.'; return 1;fi
            [ "$DEVICE" == /dev/good ] || return 91
            IOT_CORE_TARGET_ID=7:8:9
          }
          macos_choose() { touch "$TEST_DIR/retried"; printf '__RETRY__\\n'; }
          macos_confirm_flash() { [ "$IOT_CORE_TARGET_ID" == 7:8:9 ] || exit 92; printf 'Cancel\\n'; }
          ${selectionFlow}
        `);
        assert.equal(result.status, 0, result.stderr);
        assert.match(result.stderr, /Fixture GPT target is rejected/);
        assert.deepEqual(result.args, []);
      });

      it("returns the Linux wizard to drive selection after rejection", () => {
        const result = run(`
          DEVICE=/dev/bad
          iot_core_validate_device() {
            if [ "$DEVICE" == /dev/bad ];then warning 'Fixture GPT target is rejected.'; return 1;fi
            [ "$DEVICE" == /dev/good ] || return 91
            IOT_CORE_TARGET_ID=7:8:9
          }
          yad() { return 0; }
          linux_choose_flash_device() { DEVICE=/dev/good; }
          linux_iot_validate_selection || exit 92
          printf '%s|%s|%s\\n' "$DEVICE" "$IOT_CORE_TARGET_ID" "$WOR_IOT_CONFIRM_ERASE"
        `, { host: "Linux" });
        assert.equal(result.status, 0, result.stderr);
        assert.equal(result.stdout, "/dev/good|7:8:9|0\n");
        assert.deepEqual(result.args, []);
      });
    });

    it("refreshes compatible choices and preserves each family's pending board", () => {
      const result = run(`
        kill() { printf 'refresh-signal\\n' >> "$TEST_DIR/signals"; }
        export -f kill
        yad() {
          local count action='' argument board_choices=''
          count="$(cat "$TEST_DIR/count" 2>/dev/null || printf 0)"
          count=$((count+1)); printf '%s' "$count" > "$TEST_DIR/count"
          for argument in "$@";do
            case "$argument" in --changed-action=*) action="\${argument#*=}" ;; esac
          done
          case "$count" in
            1 | 3)
              YAD_PID=999999 bash -c "$action" _ 1 'Windows 10 IoT Core (ARM32, legacy)' || return 91
              printf 'Windows 10 IoT Core (ARM32, legacy)\\nRaspberry Pi 4 / Pi 400\\n'
              ;;
            2)
              YAD_PID=999999 bash -c "$action" _ 1 'Windows 11' || return 92
              printf 'Windows 11\\nRaspberry Pi 2 v1.1\\n'
              ;;
            4)
              printf '%s\\0' "$@" > "$TEST_DIR/fields"
              printf 'Windows 10 IoT Core (ARM32, legacy)\\nRaspberry Pi 2 v1.1\\n'
              ;;
            *) return 93 ;;
          esac
        }
        linux_choose_target 'Windows 11' 'Raspberry Pi 4 / Pi 400' || exit 94
        [ "$(cat "$TEST_DIR/count")" == 4 ] || exit 95
        [ "$(wc -l < "$TEST_DIR/signals" | tr -d ' ')" == 3 ] || exit 96
      `, { host: "Linux" });
      assert.equal(result.status, 0, result.stderr);
      assert.equal(result.stdout, "Windows 10 IoT Core (ARM32, legacy)\tRaspberry Pi 2 v1.1\n");
      const boardField = result.fields.indexOf("--field=Raspberry Pi model:CB");
      assert.ok(result.fields[boardField + 1].startsWith("Raspberry Pi 2 v1.1!"));
      assert.deepEqual(result.args, []);
    });

    for (const [name, reply, status] of [
      ["Cancel", "", 1],
      ["unsupported IoT board", "Windows 10 IoT Core (ARM32, legacy)\nRaspberry Pi 5\n", 0],
    ]) {
      it(`does not advance or prepare media after ${name}`, () => {
        const result = run(`
          yad() { printf '%s' '${
            reply
          }'; return ${status}; }
          if linux_choose_target 'Windows 10 IoT Core (ARM32, legacy)' 'Raspberry Pi 3 Model B';then exit 91;fi
        `, { host: "Linux" });
        assert.equal(result.status, 0, result.stderr);
        assert.deepEqual(result.args, []);
        if (name !== "Cancel") assert.match(result.stderr, /Choose a board supported/);
      });
    }
  });

  it("requires review confirmation for dry-run and selects a drive if Advanced turns dry-run off", () => {
    const result = run(`
      SOURCE_FILE='' IOT_CORE_SHA256='' DEVICE='' DRY_RUN=1
      ${selectionStubs}
      macos_confirm_flash() {
        if [ ! -e "$TEST_DIR/advanced" ];then
          [ -z "$DEVICE" ] || exit 92
          touch "$TEST_DIR/advanced"
          printf 'Advanced\\n'
        else printf 'Cancel\\n';fi
      }
      macos_advanced_options() { DRY_RUN=0; }
      ${selectionFlow}
    `);
    assert.equal(result.status, 0, result.stderr);
    assert.deepEqual(result.args, []);
    assert.deepEqual(result.fields, ["preview"]);
  });

  it("does not enter Linux desktop/device prompts for an inspection-only IoT review", () => {
    const start = gui.indexOf("{ #choose language");
    const end = gui.indexOf("{ #Offer to use ZRAM", start);
    const result = run(`
      DRY_RUN=1 DEVICE='' WIN_LANG='' SOURCE_FILE='' IOT_CORE_MINIMUM_BYTES=''
      gui_iot_plan_source || exit 91
      default_win_lang() { printf 'UNEXPECTED_DESKTOP_LANGUAGE\\n' >&2; return 92; }
      yad() { printf 'UNEXPECTED_PROMPT\\n' >&2; return 93; }
      ${gui.slice(start, end)}
      printf 'REVIEW_READY\\n'
    `, { host: "Linux" });
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.stdout, /REVIEW_READY/);
    assert.deepEqual(result.args, []);
    assert.doesNotMatch(result.stderr, /UNEXPECTED_/);
  });
});

describe("IoT Advanced Options transaction", () => {
  for (const [version, build] of [[10, "19045.3803"], [11, "22631.2861"]]) {
    it(`shares the selected download root with desktop Windows ${version} while preserving namespaced cache storage`, () => {
      const result = run(`
        settings="$(gui_iot_options_state | jq --arg directory "$PWD/shared-downloads" '.downloadDir=$directory')"
        gui_iot_apply_options "$settings" || exit 91
        iot_core_summary | grep '^Download folder'
        select_windows_family 'Windows ${version}' || exit 92
        BID=${build} RPI_MODEL=3
        describe_device() { printf 'Fixture disk'; }
        summary="$(settings_summary)"
        [ "$(awk -F'\\t' '$1 == "Download folder" || $1 == "Download directory" {count++} END {print count}' <<<"$summary")" == 1 ] || exit 93
        grep '^Download folder' <<<"$summary"
      `);
      assert.equal(result.status, 0, result.stderr);
      assert.equal(result.stdout, `Download folder\t${join(result.directory, "shared-downloads")}\n`.repeat(2));
      assert.deepEqual(result.args, []);
      assert.match(readFileSync(join(root, "src/lib/iot-core.sh"), "utf8"), /--cache-dir "\$DL_DIR\/iot-core"/);
    });
  }

  for (const save of [true, false]) {
    it(`refreshes Custom Image controls without losing pending edits, then ${save ? "saves" : "cancels"}`, () => {
      const result = run(`
        original="$(gui_iot_options_state)"
        kill() { :; }
        export -f kill
        yad() {
          local count action='' argument
          count="$(cat "$TEST_DIR/counter" 2>/dev/null || printf 0)"
          count=$((count+1)); printf '%s' "$count" > "$TEST_DIR/counter"
          for argument in "$@";do case "$argument" in --changed-action=*) action="\${argument#*=}";; esac;done
          if [ "$count" == 1 ];then
            printf '%s\\0' "$@" > "$TEST_DIR/first-fields"
            YAD_PID=999999 bash -c "$action" _ 1 "$(wor_iot_option_label local)" || return 91
            printf '%s\\n' "$(wor_iot_option_label local)" "$PWD/pending-downloads" 'Official image default' TRUE FALSE Complete FALSE FALSE '@disabled@' '@disabled@' FALSE 'English (United States) (en-US)'
          else
            printf '%s\\0' "$@" > "$TEST_DIR/fields"
            ${save ? `
            printf '%s\\n' "$(wor_iot_option_label local)" "$PWD/local image.iso" "$(jq -r .downloadDir <<<"$state")" 'Official image default' TRUE FALSE Complete FALSE FALSE '@disabled@' '@disabled@' FALSE 'English (United States) (en-US)'
            ` : "return 1"}
          fi
        }
        linux_iot_options || exit 92
        ${save ? `
        printf '%s|%s|%s|%s|%s\\n' "$GUI_IOT_SOURCE_MODE" "$GUI_IOT_LOCAL_SOURCE" "$DRY_RUN" "$PLAY_SOUND" "$SHOW_NOTIFICATION"
        ` : '[ "$(gui_iot_options_state)" == "$original" ] || exit 93'}
      `, { host: "Linux" });
      assert.equal(result.status, 0, result.stderr);
      assert.ok(result.fields.includes("--field=Custom Image file:FL"));
      const folder = result.fields.indexOf("--field=Download folder:DIR");
      const dry = result.fields.indexOf("--field=Inspect only; do not write the drive:CHK");
      const notification = result.fields.indexOf("--field=Show a completion notification:CHK");
      assert.equal(result.fields[dry + 1], "TRUE");
      assert.equal(result.fields[notification + 1], "FALSE");
      assert.equal(result.fields[folder + 1], join(result.directory, "pending-downloads"));
      assert.deepEqual(result.args, []);
      if (save) assert.equal(result.stdout, `local|${join(result.directory, "local image.iso")}|1|0|0\n`);
    });
  }

  it("changes only applicable completion/dry-run settings without re-downloading a verified image", () => {
    const result = run(`
      settings="$(${options('.dryRun = true | .playSound = false | .showNotification = false | .completionSound = "Ping"')})"
      gui_iot_apply_options "$settings" || exit 92
      printf '%s|%s|%s|%s|%s|%s|%s\\n' "$DRY_RUN" "$PLAY_SOUND" "$SHOW_NOTIFICATION" "$COMPLETION_SOUND" "$IOT_CORE_SHA256" "$IOT_CORE_TARGET_ID" "$WOR_IOT_CONFIRM_ERASE"
      iot_core_summary
    `);
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.stdout, new RegExp(`^1\\|0\\|0\\|Ping\\|${hash}\\|1:2:3\\|0\\n`));
    assert.match(result.stdout, /Source validation\tVerified approved image; rechecked before writing/);
    assert.match(result.stdout, /Verify written image\tRequired \(cannot be skipped\)/);
    assert.match(result.stdout, /Play completion sound\tNo/);
    assert.doesNotMatch(result.stderr, /UNEXPECTED_/);
  });

  it("queues a local override and invalidates prior verification without importing", () => {
    const result = run(`
      settings="$(gui_iot_options_state | jq --arg path "$PWD/local image.iso" '.mode="local" | .source=$path')"
      gui_iot_apply_options "$settings" || exit 91
      printf '%s|%s|%s\\n' "$GUI_IOT_SOURCE_MODE" "$GUI_IOT_LOCAL_SOURCE" "$IOT_CORE_SHA256"
    `);
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.stdout, `local|${join(result.directory, "local image.iso")}|\n`);
    assert.deepEqual(result.args, []);
  });

  it("uses the official image when returning from a local override", () => {
    const result = run(`
      GUI_IOT_SOURCE_MODE=local GUI_IOT_LOCAL_SOURCE="$PWD/local image.iso"
      settings="$(${options('.mode="official"')})"
      gui_iot_apply_options "$settings" || exit 91
      printf '%s|%s|%s\\n' "$GUI_IOT_SOURCE_MODE" "$GUI_IOT_LOCAL_SOURCE" "$WOR_IOT_DOWNLOAD"
    `);
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.stdout, "official||1\n");
    assert.deepEqual(result.args, []);
  });

  it("queues a cache-directory change without reading or creating a cache", () => {
    const result = run(`
      settings="$(gui_iot_options_state | jq --arg directory "$PWD/new cache" '.downloadDir=$directory')"
      gui_iot_apply_options "$settings" || exit 91
      printf '%s|%s\\n' "$DL_DIR" "$IOT_CORE_SHA256"
    `);
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.stdout, `${join(result.directory, "new cache")}|\n`);
    assert.deepEqual(result.args, []);
  });

  it("retains the target binding but clears consent and source approval when image settings change", () => {
    const result = run(`
      settings="$(gui_iot_options_state | jq --arg source "$PWD/local image.iso" '.mode="local" | .source=$source | .dryRun=true | .showNotification=false')"
      gui_iot_apply_options "$settings" || exit 91
      printf '%s|%s|%s|%s|%s\\n' "$IOT_CORE_SHA256" "$IOT_CORE_BUILD" "$IOT_CORE_INSPECTION_JSON" "$IOT_CORE_TARGET_ID" "$WOR_IOT_CONFIRM_ERASE"
    `);
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.stdout, "|||1:2:3|0\n");
    assert.deepEqual(result.args, []);
  });

  it("does not describe an inspection-only result as bootable media", () => {
    const result = run(`
      DRY_RUN=1 SHOW_NOTIFICATION=1
      wor_osascript() { cat >/dev/null; printf '%s\\n' "$3" > "$TEST_DIR/notification"; }
      wor_show_result_notification success "$(windows_version_label)"
      wait
      cat "$TEST_DIR/notification"
    `);
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.stdout, /No drive was written; hardware boot has not been assessed/);
    assert.doesNotMatch(result.stdout, /ready to boot/);
  });

  for (const invalid of [
    '.mode="unknown"', '.source="/tmp/invalid.img" | .mode="local"', '.source="" | .mode="local"',
    '.downloadDir="relative/cache"', '.dryRun="true"', '.completionSound="unknown-sound"',
  ]) {
    it(`rejects invalid settings without mutation: ${invalid}`, () => {
      const result = run(`
        before="$(gui_iot_options_state)"
        settings="$(${options(invalid)})"
        if gui_iot_apply_options "$settings";then exit 91;fi
        [ "$(gui_iot_options_state)" == "$before" ] || exit 92
      `);
      assert.equal(result.status, 0, result.stderr);
      assert.match(result.stderr, /No preferences were changed/);
    });
  }

  for (const host of ["Darwin", "Linux"]) {
    it(`Back on ${host} leaves options and image approval untouched`, () => {
      const result = run(`
        before="$(gui_iot_options_state)"
        wor_osascript() { cat >/dev/null; printf '__WOR_CANCEL__\\n'; }
        yad() { return 1; }
        ${host === "Darwin" ? "macos_iot_options" : "linux_iot_options"} || exit 91
        [ "$(gui_iot_options_state)" == "$before" ] || exit 92
        [ "$IOT_CORE_SHA256" == "${hash}" ] && [ "$WOR_IOT_CONFIRM_ERASE" == 1 ] || exit 93
      `, { host });
      assert.equal(result.status, 0, result.stderr);
    });

    it(`saves the same applicable options through the ${host} form`, () => {
      const result = run(`
        wor_osascript() {
          cat >/dev/null
          cat "$4" | jq '.dryRun=true | .playSound=false | .showNotification=false'
        }
        yad() {
          printf '%s\\0' "$@" > "$TEST_DIR/fields"
          printf '%s\\n' "$(wor_iot_option_label official) ($(wor_iot_option_label recommended))" "$DL_DIR" 'Official image default' TRUE FALSE Complete FALSE FALSE '@disabled@' '@disabled@' FALSE 'English (United States) (en-US)'
        }
        ${host === "Darwin" ? "macos_iot_options" : "linux_iot_options"} || exit 91
        printf '%s|%s|%s|%s\\n' "$DRY_RUN" "$PLAY_SOUND" "$SHOW_NOTIFICATION" "$IOT_CORE_SHA256"
      `, { host });
      assert.equal(result.status, 0, result.stderr);
      assert.equal(result.stdout, `1|0|0|${hash}\n`);
      if (host === "Linux") {
        const labels = result.fields.filter((field) => field.startsWith("--field=")).join("\n");
        assert.match(labels, /Image source:CB/);
        assert.doesNotMatch(labels, /Custom Image.*:FL/);
        assert.match(labels, /Download folder:DIR/);
        assert.match(labels, /completion sound:CHK|Completion sound:CB|completion notification:CHK/);
        assert.doesNotMatch(labels, /OOBE|account|locale|UEFI|ARM64|config\.txt|Skip verifying/);
        assert.ok(!result.fields.includes("--scroll"));
      }
    });
  }
});

describe("IoT HDMI Advanced Options", () => {
  it("defaults an unset video preference to recommended compatibility without accessing an image", () => {
    const result = run("unset IOT_CORE_HDMI_MODE; gui_iot_options_state");
    assert.equal(result.status, 0, result.stderr);
    const state = JSON.parse(result.stdout);
    assert.equal(state.hdmiMode, "720p60");
    assert.equal(state.hdmi.recommended, true);
    assert.deepEqual(state.hdmi.choices.filter((choice) => choice.recommended).map((choice) => choice.value), ["720p60"]);
    assert.deepEqual(result.args, []);
  });

  it("preselects and labels compatibility Recommended in the Linux dropdown", () => {
    const result = run(`
      unset IOT_CORE_HDMI_MODE
      yad() {
        local index value
        printf '%s\\0' "$@" > "$TEST_DIR/fields"
        for ((index=0;index<\${#fields[@]};index+=2));do
          value="\${fields[$((index+1))]}"
          printf '%s\\n' "\${value%%!*}"
        done
      }
      linux_iot_options || exit 91
      printf '%s\\n' "$IOT_CORE_HDMI_MODE"
    `, { host: "Linux" });
    assert.equal(result.status, 0, result.stderr);
    const field = result.fields.indexOf("--field=HDMI display:CB");
    assert.ok(field >= 0);
    assert.ok(result.fields[field + 1].startsWith("1280 x 720 / 60 Hz (compatibility) (Recommended)!"));
    assert.ok(result.fields[field + 1].includes("Official image default"));
    assert.ok(!result.fields[field + 1].includes("Official image default (Recommended)"));
    assert.equal(result.stdout, "720p60\n");
  });

  it("identifies the custom boot-text editor as video-only while preserving an explicit official preference", () => {
    const result = run("gui_iot_options_state");
    assert.equal(result.status, 0, result.stderr);
    const state = JSON.parse(result.stdout);
    assert.equal(state.hdmiMode, "official");
    assert.equal(state.hdmi.choices.find((choice) => choice.value === "custom").label, "Custom video settings");
    assert.equal(state.labels.editHdmi, "View / Edit config.txt (video only)");
    assert.match(state.labels.hdmiEditorHelp, /merged into the existing IoT boot config\.txt/);
    assert.match(state.labels.hdmiEditorHelp, /Required Windows boot, memory and framebuffer settings are preserved/);
    assert.deepEqual(result.args, []);
  });

  for (const [mode, expected] of [
    ["official", "Official image default"],
    ["720p60", "1280 x 720 / 60 Hz (compatibility) (Recommended)"],
    ["1080p60", "1920 x 1080 / 60 Hz"],
    ["custom", "Custom HDMI (group 1, mode 4)"],
  ]) {
    it(`saves ${mode} without acquiring media or losing the verified target/image`, () => {
      const result = run(`
        settings="$(gui_iot_options_state | jq '.hdmiMode="${mode}" | .hdmiConfig="hdmi_group=1\\nhdmi_mode=4"')"
        gui_iot_apply_options "$settings" || exit 91
        printf '%s|%s|%s|%s\\n' "$IOT_CORE_HDMI_MODE" "$IOT_CORE_SHA256" "$IOT_CORE_TARGET_ID" "$WOR_IOT_CONFIRM_ERASE"
        iot_core_summary | grep '^HDMI display'
      `);
      assert.equal(result.status, 0, result.stderr);
      assert.equal(result.stdout, `${mode}|${hash}|1:2:3|0\nHDMI display\t${expected}\n`);
      assert.deepEqual(result.args, []);
    });
  }

  it("rejects non-display firmware edits without changing any preferences", () => {
    const result = run(`
      before="$(gui_iot_options_state)"
      settings="$(jq '.hdmiMode="custom" | .hdmiConfig="kernel_old=0"' <<<"$before")"
      if gui_iot_apply_options "$settings";then exit 91;fi
      [ "$(gui_iot_options_state)" == "$before" ] || exit 92
    `);
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.stderr, /accept only hdmi_force_hotplug/);
    assert.deepEqual(result.args, []);
  });

  for (const save of [true, false]) {
    it(`shows the Linux HDMI editor only for Custom and ${save ? "saves" : "discards"} pending edits`, () => {
      const result = run(`
        original="$(gui_iot_options_state)"
        kill() { :; }; export -f kill
        yad() {
          local count index label value action argument
          count="$(cat "$TEST_DIR/counter" 2>/dev/null || printf 0)"
          count=$((count+1)); printf '%s' "$count" > "$TEST_DIR/counter"
          for argument in "$@";do case "$argument" in --changed-action=*) action="\${argument#*=}";; esac;done
          if [ "$count" == 1 ];then
            printf '%s\\0' "$@" > "$TEST_DIR/first-fields"
            YAD_PID=999999 bash -c "$action" _ "$hdmi_field" 'Custom video settings'
          else
            printf '%s\\0' "$@" > "$TEST_DIR/fields"
            printf 'hdmi_group=2\\nhdmi_mode=16\\n' > "$hdmi_edit_file"
            ${save ? "" : "return 1"}
          fi
          for ((index=0;index<\${#fields[@]};index+=2));do
            label="\${fields[$index]}" value="\${fields[$((index+1))]}"
            [ "$label" != "--field=HDMI display:CB" ] || value='Custom video settings'
            printf '%s\\n' "\${value%%!*}"
          done
        }
        linux_iot_options || exit 91
        ${save ? 'printf "%s|%s\\n" "$IOT_CORE_HDMI_MODE" "$IOT_CORE_HDMI_CONFIG"' : '[ "$(gui_iot_options_state)" == "$original" ] || exit 92'}
      `, { host: "Linux" });
      assert.equal(result.status, 0, result.stderr);
      assert.ok(result.fields.includes("--field=View / Edit config.txt (video only):BT"));
      assert.ok(result.fields.includes("--field=Reset to 720p defaults:BT"));
      assert.ok(!result.firstFields.includes("--field=View / Edit config.txt (video only):BT"));
      assert.deepEqual(result.args, []);
      if (save) assert.equal(result.stdout, "custom|hdmi_group=2\nhdmi_mode=16\n");
    });
  }

  it("resets pending Linux custom HDMI text to the supplied 720p defaults", () => {
    const result = run(`
      IOT_CORE_HDMI_MODE=custom IOT_CORE_HDMI_CONFIG=$'hdmi_group=2\\nhdmi_mode=16'
      yad() {
        local index label value action
        printf '%s\\0' "$@" > "$TEST_DIR/fields"
        for ((index=0;index<\${#fields[@]};index+=2));do
          label="\${fields[$index]}" value="\${fields[$((index+1))]}"
          if [ "$label" == "--field=Reset to 720p defaults:BT" ];then
            action="\${value#@}"
            bash -c "$action" || return 91
          fi
          printf '%s\\n' "\${value%%!*}"
        done
      }
      linux_iot_options || exit 92
      printf '%s\\n' "$IOT_CORE_HDMI_CONFIG"
    `, { host: "Linux" });
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.stdout, "hdmi_force_hotplug=1\nhdmi_group=1\nhdmi_mode=4\n");
  });

  for (const host of ["Darwin", "Linux"]) {
    it(`uses the shared saved-option validation on ${host} when custom settings are invalid`, () => {
      const result = run(`
        settings="$(gui_iot_options_state | jq '.hdmiMode="custom" | .hdmiConfig="include forbidden.txt"')"
        macos_choose() { printf '%s\\n' "$2" > "$TEST_DIR/validation"; printf 'edit\\n'; }
        yad() { printf '%s\\0' "$@" > "$TEST_DIR/fields"; return 0; }
        status=0
        gui_iot_save_options "$settings" || status=$?
        [ "$status" == 1 ] || exit 91
        [ "$IOT_CORE_HDMI_MODE" == official ] || exit 92
      `, { host });
      assert.equal(result.status, 0, result.stderr);
      assert.match(result.stderr, /No image or drive was modified/);
      if (host === "Linux") assert.ok(result.fields.includes("--warning"));
    });
  }
});

describe("Optional IoT administrator preferences", () => {
  it("defaults off without storing a factory password", () => {
    const result = run("gui_iot_options_state");
    assert.equal(result.status, 0, result.stderr);
    const state = JSON.parse(result.stdout);
    assert.equal(state.accountSetup, false);
    assert.equal(state.accountUsername, "Administrator");
    assert.equal(state.accountPassword, "");
  });

  it("validates and saves desired credentials without changing source approval or exposing a password in the summary", () => {
    const result = run(`
      settings="$(gui_iot_options_state | jq '.accountSetup=true | .accountUsername="FixtureAdmin" | .accountPassword="Fixture-password-123"')"
      gui_iot_apply_options "$settings" || exit 91
      [ "$IOT_CORE_ACCOUNT_PASSWORD" == Fixture-password-123 ] || exit 92
      [ "$IOT_CORE_SHA256" == "${hash}" ] || exit 93
      iot_core_summary | grep '^IoT administrator'
      export_installer_settings
      env | grep '^IOT_CORE_ACCOUNT_PASSWORD=' && exit 94
      printf '%s\\n' "$IOT_CORE_ACCOUNT_SETUP|$IOT_CORE_ACCOUNT_USERNAME"
    `);
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.stdout, /Pending after first boot \(FixtureAdmin\)/);
    assert.match(result.stdout, /1\|FixtureAdmin/);
    assert.doesNotMatch(result.stdout + result.stderr, /Fixture-password-123/);
    assert.deepEqual(result.args, []);
  });

  it("rejects weak credentials without mutation and clears a disabled password", () => {
    const result = run(`
      before="$(gui_iot_options_state)"
      invalid="$(jq '.accountSetup=true | .accountPassword="short"' <<<"$before")"
      if gui_iot_apply_options "$invalid";then exit 91;fi
      [ "$(gui_iot_options_state)" == "$before" ] || exit 92
      IOT_CORE_ACCOUNT_SETUP=1 IOT_CORE_ACCOUNT_PASSWORD=Fixture-password-123
      disabled="$(gui_iot_options_state | jq '.accountSetup=false')"
      gui_iot_apply_options "$disabled" || exit 93
      [ -z "$IOT_CORE_ACCOUNT_PASSWORD" ] || exit 94
    `);
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.stderr, /12-127 characters/);
  });

  it("passes private state paths to the native options dialog, never password-bearing JSON", () => {
    const result = run(`
      IOT_CORE_ACCOUNT_SETUP=1 IOT_CORE_ACCOUNT_PASSWORD=Fixture-password-123
      wor_osascript() {
        cat >/dev/null
        printf '%s\\0' "$@" > "$TEST_DIR/fields"
        [ "$(stat -f %Lp "$4")" == 600 ] || return 91
        cat "$4"
      }
      macos_iot_options || exit 92
      [ "$IOT_CORE_ACCOUNT_PASSWORD" == Fixture-password-123 ] || exit 93
    `);
    assert.equal(result.status, 0, result.stderr);
    assert.ok(result.fields[3].startsWith("/"));
    assert.ok(!result.fields.join(" ").includes("Fixture-password-123"));
  });

  for (const save of [true, false]) {
    it(`${save ? "saves" : "discards"} Linux account edits while keeping the hidden field's argv empty`, () => {
      const result = run(`
        IOT_CORE_ACCOUNT_SETUP=1 IOT_CORE_ACCOUNT_PASSWORD=Fixture-original-123
        original="$(gui_iot_options_state)"
        yad() {
          local index label value
          printf '%s\\0' "$@" > "$TEST_DIR/fields"
          ${save ? "" : "return 1"}
          for ((index=0;index<\${#fields[@]};index+=2));do
            label="\${fields[$index]}" value="\${fields[$((index+1))]}"
            [ "$label" != "--field=IoT administrator username:TXT" ] || value=FixtureAdmin
            [ "$label" != "--field=New IoT password:H" ] || value=Fixture-updated-123
            printf '%s\\n' "\${value%%!*}"
          done
        }
        linux_iot_options || exit 91
        ${save ? '[ "$IOT_CORE_ACCOUNT_PASSWORD" == Fixture-updated-123 ] && [ "$IOT_CORE_ACCOUNT_USERNAME" == FixtureAdmin ] || exit 92' :
          '[ "$(gui_iot_options_state)" == "$original" ] || exit 93'}
      `, { host: "Linux" });
      assert.equal(result.status, 0, result.stderr);
      const field = result.rawFields.indexOf("--field=New IoT password:H");
      assert.ok(field >= 0);
      assert.equal(result.rawFields[field + 1], "");
      assert.ok(!result.fields.join(" ").includes("Fixture-original-123"));
    });
  }

  it("keeps account setup Pending when skipped and never connects during a dry run", () => {
    const result = run(`
      IOT_CORE_ACCOUNT_SETUP=1 IOT_CORE_ACCOUNT_PASSWORD=Fixture-password-123
      DRY_RUN=1
      gui_iot_account_connection() { printf 'UNEXPECTED_CONNECTION\\n' >&2; return 95; }
      gui_iot_account_setup || exit 91
      DRY_RUN=0
      gui_iot_account_connection() { return 2; }
      gui_iot_account_setup || exit 92
      [ "$IOT_CORE_ACCOUNT_STATUS" == pending ] && [ -z "$IOT_CORE_ACCOUNT_PASSWORD" ] || exit 93
    `);
    assert.equal(result.status, 0, result.stderr);
    assert.doesNotMatch(result.stderr, /UNEXPECTED_CONNECTION/);
    assert.match(result.stderr, /Pending/);
  });

  it("requires pairing confirmation before sending credentials and clears them after verified setup", () => {
    const result = run(`
      DRY_RUN=0 IOT_CORE_ACCOUNT_SETUP=1 IOT_CORE_ACCOUNT_USERNAME=FixtureAdmin IOT_CORE_ACCOUNT_PASSWORD=Fixture-desired-123
      gui_iot_account_connection() { printf '{"host":"192.168.50.23","currentUsername":"Administrator","currentPassword":"Fixture-current-123"}\\n'; }
      macos_choose() { printf 'apply\\n'; }
      macos_show_result_dialog() { printf '%s\\n' "$1" > "$TEST_DIR/account-message"; }
      python3() {
        [ "\${1##*/}" == iot-account.py ] || return 91
        local input
        input="$(cat)"
        printf '%s\\0' "$@" >> "$TEST_DIR/fields"
        case "$2" in
          probe)
            [ "$(jq -r 'has("currentPassword")' <<<"$input")" == false ] || return 92
            printf '{"address":"192.168.50.23","fingerprint":"SHA256:fixture","keyType":"ssh-ed25519","key":"fixture"}\\n' ;;
          configure)
            [ "$(jq -r .confirmIdentity <<<"$input")" == true ] || return 93
            [ "$(jq -r .accountPassword <<<"$input")" == Fixture-desired-123 ] || return 94
            printf '{"state":"verified","accountState":"verified","languageState":"not-assessed","language":"en-US","username":"FixtureAdmin","sid":"S-1-5-21-1-2-3-500"}\\n' ;;
          *) return 95 ;;
        esac
      }
      gui_iot_account_setup || exit 96
      [ "$IOT_CORE_ACCOUNT_STATUS" == verified ] && [ -z "$IOT_CORE_ACCOUNT_PASSWORD" ] || exit 97
    `);
    assert.equal(result.status, 0, result.stderr);
    assert.ok(!result.fields.join(" ").includes("Fixture-desired-123"));
    assert.ok(!result.fields.join(" ").includes("Fixture-current-123"));
    assert.doesNotMatch(result.stderr, /Fixture-(desired|current)-123/);
  });
});

describe("Shared IoT language preferences", () => {
  for (const host of ["Darwin", "Linux"]) {
    for (const [hostLocale, expected] of [["fr_FR.UTF-8", "fr-FR"], ["sr_Latn_RS.UTF-8", "sr-Latn-RS"], ["unsupported_LOCALE", "en-US"]]) {
      it(`auto-selects ${expected} from ${host} locale ${hostLocale}`, () => {
        const result = run(`
          unset IOT_CORE_LANGUAGE IOT_CORE_LANGUAGE_SETUP
          WINDOWS_LOCALE=''
          defaults() { printf '%s\\n' "${hostLocale}"; }
          LANG="${hostLocale}" LC_ALL='' LC_MESSAGES=''
          gui_iot_options_state
        `, {host});
        assert.equal(result.status, 0, result.stderr);
        const state = JSON.parse(result.stdout);
        assert.equal(state.language, expected);
        assert.equal(state.languageSetup, true);
        assert.ok(state.languages.some((entry) => entry.value === expected));
        assert.deepEqual(result.args, []);
      });
    }
  }

  it("preserves explicit language changes without reacquiring or changing the verified image", () => {
    const result = run(`
      options="$(gui_iot_options_state | jq '.languageSetup=true | .language="fr-FR"')"
      gui_iot_apply_options "$options" || exit 91
      [ "$IOT_CORE_SHA256" == "${hash}" ] || exit 92
      printf '%s|%s\\n' "$IOT_CORE_LANGUAGE_SETUP" "$IOT_CORE_LANGUAGE"
      iot_core_summary | grep '^IoT language'
    `);
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.stdout, /^1\|fr-FR/);
    assert.match(result.stdout, /Pending after first boot; installed language must be verified/);
    assert.deepEqual(result.args, []);
  });

  for (const save of [true, false]) {
    it(`${save ? "saves" : "discards"} the Linux language selection in Advanced Options`, () => {
      const result = run(`
        original="$(gui_iot_options_state)"
        yad() {
          local index label value
          printf '%s\\0' "$@" > "$TEST_DIR/fields"
          ${save ? "" : "return 1"}
          for ((index=0;index<\${#fields[@]};index+=2));do
            label="\${fields[$index]}" value="\${fields[$((index+1))]}"
            [ "$label" != "--field=IoT language:CB" ] || value='French (France) (fr-FR)'
            [ "$label" != "--field=Apply IoT language after first boot:CHK" ] || value=TRUE
            printf '%s\\n' "\${value%%!*}"
          done
        }
        linux_iot_options || exit 91
        ${save ? '[ "$IOT_CORE_LANGUAGE" == fr-FR ] && [ "$IOT_CORE_LANGUAGE_SETUP" == 1 ] || exit 92' :
          '[ "$(gui_iot_options_state)" == "$original" ] || exit 92'}
      `, {host:"Linux"});
      assert.equal(result.status, 0, result.stderr);
      const dropdown = result.fields.indexOf("--field=IoT language:CB");
      assert.ok(dropdown >= 0);
      assert.ok(result.fields[dropdown+1].startsWith("English (United States) (en-US)!"));
    });
  }

  for (const languageState of ["verified", "pending-reboot"]) {
    it(`forwards a language-only request and reports ${languageState} without changing a password`, () => {
      const result = run(`
        DRY_RUN=0 IOT_CORE_ACCOUNT_SETUP=0 IOT_CORE_LANGUAGE_SETUP=1 IOT_CORE_LANGUAGE=fr-FR
        gui_iot_account_connection() { printf '{"host":"192.168.50.23","currentUsername":"Administrator","currentPassword":"Fixture-current-123"}\\n'; }
        macos_choose() { printf 'apply\\n'; }
        macos_show_result_dialog() { printf '%s\\n' "$1" > "$TEST_DIR/result-message"; }
        python3() {
          local input
          input="$(cat)"
          case "$2" in
            probe) printf '{"address":"192.168.50.23","fingerprint":"SHA256:fixture","keyType":"ssh-ed25519","key":"fixture"}\\n';;
            configure)
              [ "$(jq -r .language <<<"$input")" == fr-FR ] && [ "$(jq -r .languageSetup <<<"$input")" == true ] \
                && [ "$(jq -r .accountSetup <<<"$input")" == false ] || return 95
              printf '{"state":"${languageState}","accountState":"not-assessed","languageState":"${languageState}","language":"fr-FR","username":"Administrator"}\\n';;
            *) return 96;;
          esac
        }
        gui_iot_account_setup || exit 91
        [ "$IOT_CORE_LANGUAGE_STATUS" == "${languageState}" ] && [ "$IOT_CORE_ACCOUNT_STATUS" == not-assessed ] || exit 92
      `);
      assert.equal(result.status, 0, result.stderr);
      assert.match(result.stderr, new RegExp(`language=${languageState}`));
      assert.doesNotMatch(result.stderr, /Fixture-current-123/);
    });
  }
});

describe("Post-Flash IoT preparation", () => {
  const backend = `
    RUN_MODE=gui WOR_GUI_PROGRESS_FILE="$PWD/progress" WOR_IOT_CONFIRM_ERASE=1
    DEVICE=/dev/mock-only SOURCE_FILE='' WOR_IOT_DOWNLOAD=1
    IOT_CORE_SHA256='' IOT_CORE_BUILD='' IOT_CORE_MINIMUM_BYTES=''
    require_macos_tools() { :; }
    detect_root_dev() { ROOT_DEV=/dev/mock-host; }
    iot_core_validate_device() { printf '%s\\n' "\${1:-verified}" >> "$TEST_DIR/trace"; }
    python3() {
      if [ "$2" == profile ] || [ "$2" == hdmi-options ];then command python3 "$@"; return;fi
      printf '%s\\0' "$@" > "$TEST_DIR/worker-args"
      printf 'acquire\\n' >> "$TEST_DIR/trace"
      printf 'Downloading official package: 86%%\\nVerifying official package: 4%%\\n' >&2
      jq --arg source "$PWD/prepared.ffu" '. + {source_file: $source, acquisition: "download"}' <<'JSON'
${metadata}
JSON
    }
    sudo() { [ "$1" == -v ] || return 95; printf 'authenticate\\n' >> "$TEST_DIR/trace"; }
    diskutil() {
      case "$1" in unmountDisk) printf 'unmount\\n';; eject) printf 'eject\\n';; *) return 96;; esac >> "$TEST_DIR/trace"
    }
    iot_core_apply() { printf 'write-and-readback\\n' >> "$TEST_DIR/trace"; }
  `;

  it("downloads and validates only after confirmation, before authentication, unmount and writing", () => {
    const result = run(`${backend}\niot_core_run\nsleep 0.2`);
    assert.equal(result.status, 0, result.stderr);
    assert.deepEqual(result.trace, [
      "preview", "acquire", "verified", "authenticate", "verified", "unmount", "write-and-readback", "eject",
    ]);
    assert.equal(result.args[1], "download");
    assert.match(result.progress, /STEP\t1\t3\tPreparing and verifying Windows 10 IoT Core image/);
    assert.match(result.progress, /TASK\t86\tDownloading and verifying Windows 10 IoT Core/);
    assert.match(result.progress, /TASK\t4\tDownloading and verifying Windows 10 IoT Core/);
    assert.match(result.progress, /DISK_WRITE\t1\t\/dev\/mock-only/);
  });

  it("does not acquire anything when Flash/erase confirmation is absent", () => {
    const result = run(`${backend}\nWOR_IOT_CONFIRM_ERASE=0\niot_core_run`);
    assert.notEqual(result.status, 0);
    assert.deepEqual(result.trace, ["preview"]);
    assert.deepEqual(result.args, []);
    assert.match(result.stderr, /explicit erase confirmation/);
  });

  it("checks a configured boot image before requesting credentials or touching the target", () => {
    const result = run(`
      ${backend}
      IOT_CORE_HDMI_MODE=720p60
      iot_core_hdmi_preflight() { printf 'hdmi-preflight\\n' >> "$TEST_DIR/trace"; return 1; }
      iot_core_run
    `);
    assert.notEqual(result.status, 0);
    assert.deepEqual(result.trace, ["preview", "acquire", "hdmi-preflight"]);
    assert.match(result.stderr, /Cannot apply the selected IoT HDMI settings. No drive was written/);
    assert.doesNotMatch(result.progress, /DISK_WRITE/);
  });

  for (const failure of ["validation", "cancellation"]) {
    it(`does not authenticate, unmount or write after source ${failure}`, () => {
      const result = run(`
        ${backend}
        python3() {
          if [ "$2" == profile ] || [ "$2" == hdmi-options ];then command python3 "$@"; return;fi
          printf 'acquire\\n' >> "$TEST_DIR/trace"
          printf 'Fixture source ${failure}\\n' >&2
          return ${failure === "cancellation" ? "130" : "2"}
        }
        iot_core_run
      `);
      assert.notEqual(result.status, 0);
      assert.deepEqual(result.trace, ["preview", "acquire"]);
      assert.match(result.stderr, /Cannot use this IoT Core image. No drive was written/);
      assert.doesNotMatch(result.progress, /DISK_WRITE/);
    });
  }

  it("inspects only after the dry-run action, without selecting or opening a device", () => {
    const result = run(`${backend}\nDRY_RUN=1 DEVICE='' WOR_IOT_CONFIRM_ERASE=0\niot_core_run`);
    assert.equal(result.status, 0, result.stderr);
    assert.deepEqual(result.trace, ["acquire"]);
    assert.match(result.stderr, /no drive was opened for writing/);
    assert.doesNotMatch(result.progress, /DISK_WRITE/);
  });

  it("refuses a target replacement during preparation before requesting administrator access", () => {
    const result = run(`
      ${backend}
      iot_core_validate_device() {
        printf '%s\\n' "\${1:-verified}" >> "$TEST_DIR/trace"
        [ "\${1:-verified}" == preview ]
      }
      iot_core_run
    `);
    assert.notEqual(result.status, 0);
    assert.deepEqual(result.trace, ["preview", "acquire", "verified"]);
    assert.match(result.stderr, /target changed during image preparation/);
    assert.doesNotMatch(result.progress, /DISK_WRITE/);
  });

  it("protects the planned cache and log disk before an image exists", () => {
    const result = run(`
      SOURCE_FILE='' DEVICE=/dev/mock-target DL_DIR="$PWD/future-cache"
      iot_core_backing_disks() {
        [ -n "$1" ] || { printf 'UNEXPECTED_EMPTY_SOURCE\\n' >&2; return 91; }
        if [ "$1" == "$TEST_DIR" ];then printf '/dev/mock-target\\n';else printf '/dev/mock-host\\n';fi
      }
      if iot_core_protect_source_disks preview;then exit 92;fi
    `);
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.stderr, /Refusing to erase.*log\/cache/);
    assert.doesNotMatch(result.stderr, /UNEXPECTED_EMPTY_SOURCE/);
    assert.deepEqual(result.args, []);
  });
});

describe("Native HDMI preset and editor state", () => {
  const hdmiFunctions = native.slice(native.indexOf("function updateHdmiControls()"), native.indexOf("function choosePath"));
  const compatibility = "hdmi_force_hotplug=1\nhdmi_group=1\nhdmi_mode=4\n";
  const hdmiChoices = [
    { value: "official", settings: "" }, { value: "720p60", settings: compatibility, recommended: true },
    { value: "1080p60", settings: compatibility.replace("mode=4", "mode=16") },
    { value: "custom", settings: "" },
  ];
  it("shows the green Recommended badge only for compatibility and never overlaps its editor", () => {
    const context = vm.createContext({
      state: { hdmi: { choices: hdmiChoices } }, accountCheckbox: { state: 0 },
      hdmiPopup: { indexOfSelectedItem: 1, fittingSize: { width: 254 } }, hdmiLabel: {},
      editHdmiButton: {}, hdmiRecommendedBadge: {}, innerWidth: 712,
      $: { NSMakeRect: (x, y, width, height) => ({ x, y, width, height }) },
    });
    vm.runInContext(hdmiFunctions, context);
    for (const [index, recommended] of [[1, true], [0, false], [2, false], [3, false]]) {
      context.hdmiPopup.indexOfSelectedItem = index;
      vm.runInContext("updateHdmiControls()", context);
      assert.equal(context.hdmiRecommendedBadge.hidden, !recommended);
      assert.equal(context.editHdmiButton.hidden, index !== 3);
      if (recommended) {
        assert.ok(context.hdmiPopup.frame.x + context.hdmiPopup.frame.width < context.hdmiRecommendedBadge.frame.x);
      }
    }
    assert.match(native, /hdmiRecommendedBadge = addRecommendedBadge\(\)/);
    assert.match(native, /badge\.textColor = \$\.NSColor\.systemGreenColor/);
    context.innerWidth = 535;
    context.hdmiPopup.indexOfSelectedItem = 1;
    vm.runInContext("updateHdmiControls()", context);
    assert.ok(context.hdmiPopup.frame.width >= context.hdmiPopup.fittingSize.width);
    assert.ok(context.hdmiPopup.frame.x + context.hdmiPopup.frame.width < context.hdmiRecommendedBadge.frame.x);
  });

  it("reveals the editor only for Custom and seeds it from the prior preset", () => {
    const context = vm.createContext({
      state: {
        hdmi: { choices: hdmiChoices, custom: compatibility },
        labels: { editHdmi: "View / Edit config.txt (video only)", hdmiEditorHelp: "Merged video settings; required boot entries stay protected." },
      },
      hdmiPopup: { indexOfSelectedItem: 0, fittingSize: { width: 254 } }, hdmiLabel: {},
      editHdmiButton: {}, hdmiRecommendedBadge: {}, innerWidth: 712,
      previousHdmi: "official", hdmiConfig: compatibility, hdmiEdited: false, accountCheckbox: { state: 0 },
      $: { NSMakeRect: (x, y, width, height) => ({ x, y, width, height }) },
    });
    vm.runInContext(hdmiFunctions, context);
    vm.runInContext("updateHdmiControls()", context);
    assert.equal(context.editHdmiButton.hidden, true);
    context.hdmiPopup.indexOfSelectedItem = 2;
    vm.runInContext("hdmiChanged()", context);
    context.hdmiPopup.indexOfSelectedItem = 3;
    vm.runInContext("hdmiChanged()", context);
    assert.equal(context.hdmiConfig, hdmiChoices[2].settings);
    assert.equal(context.editHdmiButton.hidden, false);
    assert.equal(context.editHdmiButton.enabled, true);
  });
  for (const save of [true, false]) {
    it(`${save ? "retains saved" : "discards canceled"} custom edits when switching modes`, () => {
      const original = "hdmi_group=2\nhdmi_mode=16";
      let editorArguments;
      const context = vm.createContext({
        state: {
          hdmi: { choices: hdmiChoices, custom: compatibility },
          labels: { editHdmi: "View / Edit config.txt (video only)", hdmiEditorHelp: "Merged video settings; required boot entries stay protected." },
        },
        hdmiPopup: { indexOfSelectedItem: 3, fittingSize: { width: 254 } }, hdmiLabel: {},
        editHdmiButton: {}, hdmiRecommendedBadge: {}, innerWidth: 712,
        previousHdmi: "custom", hdmiConfig: original, hdmiEdited: true, iconPath: "", accountCheckbox: { state: 0 },
        worEditText: (...args) => { editorArguments = args; return { saved: save, text: "hdmi_group=1\nhdmi_mode=16" }; },
        $: { NSMakeRect: () => ({}) },
      });
      vm.runInContext(hdmiFunctions, context);
      vm.runInContext("editHdmi()", context);
      assert.equal(editorArguments[1], "View / Edit config.txt (video only)");
      assert.match(editorArguments[2], /required boot entries stay protected/);
      assert.equal(editorArguments[4], compatibility);
      context.hdmiPopup.indexOfSelectedItem = 0;
      vm.runInContext("hdmiChanged()", context);
      context.hdmiPopup.indexOfSelectedItem = 3;
      vm.runInContext("hdmiChanged()", context);
      assert.equal(context.hdmiConfig, save ? "hdmi_group=1\nhdmi_mode=16" : original);
      assert.equal(context.hdmiEdited, true);
    });
  }
});

describe("Native IoT options controls", () => {
  function layout(index, screenHeight = 900) {
    const start = native.indexOf("function updateSourceControls()");
    const end = native.indexOf("\nfunction choosePath", start);
    const sourcePopup = { indexOfSelectedItem: index };
    const sourceField = {}, browseSourceButton = {}, customLabel = {}, sourceLabel = {}, recommendedBadge = {};
    const window = {
      frame: { origin: { x: 100, y: 100 }, size: { width: 760, height: 598 } },
      setContentSize(size) { this.frame = { origin: this.frame.origin, size: { width: size.width, height: size.height + 28 } }; },
      setFrameOrigin(origin) { this.frame.origin = origin; },
    };
    const scroll = {
      frame: { size: { width: 712, height: 440 } }, contentView: {},
      get contentSize() { return { width: this.frame.size.width - (this.hasVerticalScroller ? 17 : 0), height: this.frame.size.height }; },
      reflectScrolledClipView() {},
    };
    const content = { scrollPoint(point) { this.scrollPosition = point; } };
    const context = vm.createContext({
      sourcePopup, sourceField, browseSourceButton, customLabel, sourceLabel, recommendedBadge,
      window, heading: {}, scroll, content, screen: { size: { height: screenHeight } }, width: 760, innerWidth: 712,
      hdmiPopup: { indexOfSelectedItem: 0, fittingSize: { width: 254 } }, hdmiLabel: {},
      editHdmiButton: {}, hdmiRecommendedBadge: {}, state: { hdmi: { choices: [{ value: "official" }] } },
      accountCheckbox: { state: 0 }, accountRows: [], accountUsernameLabel: {}, accountPasswordLabel: {},
      accountUsernameField: {}, accountPasswordField: {}, accountHelp: {},
      languageCheckbox: { state: 0 }, languagePopup: {}, languageLabel: {}, languageHelp: {},
      $: {
        NSMakeRect: (x, y, width, height) => ({ origin: { x, y }, size: { width, height } }),
        NSMakeSize: (width, height) => ({ width, height }), NSMakePoint: (x, y) => ({ x, y }),
      },
    });

    vm.runInContext(native.slice(start, end), context);
    vm.runInContext("updateSourceControls()", context);
    return { context, sourcePopup, sourceField, browseSourceButton, customLabel, recommendedBadge, window, scroll, content };
  }

  it("hides the custom row, shows Recommended and shrinks the official-image form", () => {
    const { sourceField, browseSourceButton, customLabel, recommendedBadge, window, scroll, content } = layout(0);
    assert.equal(sourceField.enabled, false);
    assert.equal(browseSourceButton.enabled, false);
    assert.equal(sourceField.hidden, true);
    assert.equal(browseSourceButton.hidden, true);
    assert.equal(customLabel.hidden, true);
    assert.equal(recommendedBadge.hidden, false);
    assert.equal(window.frame.size.height, 840 + 28);
    assert.equal(content.frame.size.height, 772);
    assert.equal(scroll.hasVerticalScroller, true);
    assert.match(native, /badge\.textColor = \$\.NSColor\.systemGreenColor/);
  });

  it("reveals Custom Image controls in place and hides the official recommendation", () => {
    const { context, sourcePopup, sourceField, browseSourceButton, customLabel, recommendedBadge, window, scroll } = layout(0);
    sourcePopup.indexOfSelectedItem = 1;
    vm.runInContext("updateSourceControls()", context);
    assert.equal(sourceField.enabled, true);
    assert.equal(browseSourceButton.enabled, true);
    assert.equal(sourceField.hidden, false);
    assert.equal(customLabel.hidden, false);
    assert.equal(recommendedBadge.hidden, true);
    assert.equal(window.frame.size.height, 840 + 28);
    assert.equal(window.frame.origin.y + window.frame.size.height, 698);
    assert.equal(scroll.hasVerticalScroller, true);
    sourcePopup.indexOfSelectedItem = 0;
    vm.runInContext("updateSourceControls()", context);
    assert.equal(window.frame.size.height, 840 + 28);
    assert.equal(sourceField.hidden, true);
  });

  it("scrolls only when the selected form actually exceeds the screen", () => {
    for (const index of [0, 1]) {
      const { scroll, content, window } = layout(index, 500);
      assert.equal(scroll.hasVerticalScroller, true);
      assert.equal(window.frame.size.height, 440 + 28);
      assert.ok(content.frame.size.height > scroll.frame.size.height);
    }
    assert.match(native, /documentHeight > viewportHeight/);
    assert.match(native, /pumped|pumpTimer\.invalidate/);
    assert.doesNotMatch(native, /SKIP_IMAGE_VERIFICATION|OOBE|UEFI_USE_LATEST|WINDOWS_ACCOUNT_SETUP/);
  });
});
