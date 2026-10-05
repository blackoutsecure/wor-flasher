import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";

const root = fileURLToPath(new URL("../", import.meta.url));
const gui = readFileSync(join(root, "install-wor-gui.sh"), "utf8");
const start = gui.indexOf("macos_choose_target() {");
const end = gui.indexOf("\nmacos_start_cli() {", start);
assert.ok(start >= 0 && end > start);
const picker = gui.slice(start, end);
const choices = ["Raspberry Pi 5", "Raspberry Pi 4 / Pi 400", "Raspberry Pi 3", "Raspberry Pi 2 v1.2"];

function run(body) {
  const directory = mkdtempSync(join(tmpdir(), "wor-board-choice-"));
  try {
    return spawnSync("bash", ["-c", `
      source "$DIRECTORY/install-wor.sh" source >/dev/null || exit 90
      ${body}
    `], {
      cwd: directory, encoding: "utf8", timeout: 10000,
      env: { ...process.env, NO_UPDATE: "1", DIRECTORY: root, WOR_CACHE_DIR: directory },
    });
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
}

describe("Separate Raspberry Pi 2 v1.2 board entry", () => {
  it("supplies four distinct choices to both GUI toolkits", () => {
    const result = run('wor_rpi_board_options; wor_rpi_board_options | paste -sd "!" -');
    assert.equal(result.status, 0, result.stderr);
    assert.deepEqual(result.stdout.trim().split("\n"), [...choices, choices.join("!")]);
    assert.match(gui, /const piModels = ObjC\.unwrap\(args\.objectAtIndex\(9\)\)\.split/);
    assert.match(gui, /linux_choose_target\(\)/);
    assert.match(gui, /--field='Raspberry Pi model:CB' "\$boards"/);
    assert.match(gui, /boards="\$\(wor_rpi_board_options "\$family"\)"/);
    assert.doesNotMatch(gui, /Raspberry Pi 3 \/ Pi 2 v1\.2/);
  });

  for (const [label, route, displayed] of [
    ["Raspberry Pi 2 v1.2", "3", "Raspberry Pi 2 v1.2"],
    ["Raspberry Pi 3", "3", "Raspberry Pi 3"],
    ["Raspberry Pi 4 / Pi 400", "4", "Raspberry Pi 4"],
    ["Raspberry Pi 5", "5", "Raspberry Pi 5"],
  ]) {
    it(`maps ${label} to its existing route and correct summary label`, () => {
      const result = run(`
        select_rpi_board "${label}" || exit 91
        printf '%s|' "$RPI_MODEL"; rpi_board_label; printf '\\n'
        DEVICE=/dev/mock-only BID=19045.3803 CAN_INSTALL_ON_SAME_DRIVE=1
        describe_device() { printf '%s' "$1"; }
        settings_summary | grep '^Target hardware'
      `);
      assert.equal(result.status, 0, result.stderr);
      assert.equal(result.stdout, `${route}|${displayed}\nTarget hardware\t${displayed}\n`);
    });
  }

  it("preserves Pi 2's existing Pi 3 firmware, driver, config and Windows-build routing", () => {
    const result = run(`
      select_rpi_board "Raspberry Pi 2 v1.2"
      [ "$RPI_MODEL" == 3 ] || exit 91
      release_package_source uefi || exit 92
      printf '%s|%s\\n' "$RELEASE_REPO" "$RELEASE_ASSET_PREFIX"
      release_package_source drivers || exit 93
      printf '%s|%s\\n' "$RELEASE_REPO" "$RELEASE_ASSET_PREFIX"
      [ "$(default_config_txt "$RPI_MODEL")" == "$(default_config_txt 3)" ] || exit 94
      advanced_option_applies drivers || exit 95
      if advanced_option_applies pi4;then exit 96;fi
    `);
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.stdout, "pftf/RPi3|RPi3_UEFI_Firmware_\nworproject/RPi-Windows-Drivers|RPi3_Windows_ARM64_Drivers_\n");
  });

  it("forwards the Pi 2 label to the installer without changing RPI_MODEL=3", () => {
    const result = run(`
      select_rpi_board "Raspberry Pi 2 v1.2"
      export_installer_settings
      bash -c 'source "$DIRECTORY/install-wor.sh" source >/dev/null || exit 91; printf "%s|" "$RPI_MODEL"; rpi_board_label'
    `);
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.stdout, "3|Raspberry Pi 2 v1.2");
  });

  it("clears the Pi 2 display alias when another board is chosen", () => {
    const result = run(`
      select_rpi_board "Raspberry Pi 2 v1.2"
      select_rpi_board "Raspberry Pi 3"
      rpi_board_label; printf '\\n'
      select_rpi_board "Raspberry Pi 2 v1.2"
      select_rpi_board "Raspberry Pi 4 / Pi 400"
      rpi_board_label; printf '\\n'
      [ -z "$WOR_TARGET_BOARD" ]
    `);
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.stdout, "Raspberry Pi 3\nRaspberry Pi 4\n");
  });

  it("rejects unsupported Pi 2 revisions without modifying the previous selection", () => {
    const result = run(`
      select_rpi_board "Raspberry Pi 5"
      if select_rpi_board "Raspberry Pi 2 v1.1";then exit 91;fi
      rpi_board_label
    `);
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.stdout, "Raspberry Pi 5");
    assert.match(result.stderr, /Unrecognized Raspberry Pi selection/);
  });

  it("keeps the Pi 2 choice through the real macOS Back-navigation flow", () => {
    const start = gui.indexOf("macos_start_cli() {");
    const end = gui.indexOf("\nif is_macos ;then", start);
    assert.ok(start >= 0 && end > start);
    const result = run(`
      ${gui.slice(start, end)}
      macos_choose_target() {
        if [ ! -f first-choice ];then
          touch first-choice
          printf 'Windows 10\\tRaspberry Pi 2 v1.2\\n'
        else
          [ "$2" != "Raspberry Pi 2 v1.2" ] || printf 'BACK_BOARD_OK\\n' >&2
          return 1
        fi
      }
      list_bids() { :; }
      get_bid() { printf '19045.3803\\n'; }
      darwin_list_device_choices() { printf '/dev/mock-only\\tFixture\\n'; }
      macos_choose_device() { printf 'Back\\n'; }
      macos_start_cli
    `);
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.stderr, /BACK_BOARD_OK/);
  });

  for (const previous of choices) {
    it(`preselects ${previous} when returning to the macOS target picker`, () => {
      const result = run(`
        ${picker}
        wor_osascript() {
          cat >/dev/null
          [ "$9" == "$(wor_rpi_board_options)" ] || return 99
          printf '%s\\t%s\\n' "$7" "$8"
        }
        macos_choose_target "Windows 10" "${previous}"
      `);
      assert.equal(result.status, 0, result.stderr);
      assert.equal(result.stdout, `Windows 10\t${previous}\n`);
    });
  }
});
