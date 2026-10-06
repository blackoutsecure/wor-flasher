import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { existsSync, mkdtempSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";
import vm from "node:vm";

const root = fileURLToPath(new URL("../", import.meta.url)).replace(/\/$/, "");
const gui = readFileSync(join(root, "install-wor-gui.sh"), "utf8");
const hash = "1".repeat(64);
const metadata = JSON.stringify({
  architecture: "arm32", profile: "iot-core", build: "17763.107",
  format: "ffu-v1", sector_size: 512, trust: "pinned-official-sha256",
  recommended_boards: ["pi2-v1.1", "pi2-v1.2", "pi3-b"],
  minimum_disk_bytes: 8000000000, payload_bytes: 1000, sha256: hash,
});

function run(body, files = {}) {
  const directory = mkdtempSync(join(tmpdir(), "wor-iot-profile-"));
  try {
    for (const [name, value] of Object.entries(files)) writeFileSync(join(directory, name), value);
    const result = spawnSync("bash", ["-c", `
      source "$DIRECTORY/install-wor.sh" source >/dev/null || exit 90
      IOT_CORE_HDMI_MODE=official
      if [ -n "\${WOR_TEST_IOT_LIBRARY:-}" ];then source "$WOR_TEST_IOT_LIBRARY" || exit 89;fi
      ${body}
    `], {
      cwd: directory, encoding: "utf8", timeout: 15000,
      env: {
        ...process.env, NO_UPDATE: "1", DIRECTORY: root, WOR_CACHE_DIR: directory,
        WOR_LOG_FILE: join(directory, "run.log"), DL_DIR: directory,
      },
    });
    const log = join(directory, "run.log");
    const lastLog = join(directory, "last-run.log");
    return {
      ...result,
      log: existsSync(log) ? readFileSync(log, "utf8") : "",
      logMode: existsSync(log) ? statSync(log).mode & 0o777 : null,
      lastLog: existsSync(lastLog) ? readFileSync(lastLog, "utf8") : "",
    };
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
}

describe("IoT Core profile", () => {
  it("preserves four desktop choices and supplies only documented IoT boards", () => {
    const result = run('wor_rpi_board_options; echo ---; wor_rpi_board_options iot-core');
    assert.equal(result.status, 0, result.stderr);
    assert.deepEqual(result.stdout.trim().split("\n"), [
      "Raspberry Pi 5", "Raspberry Pi 4 / Pi 400", "Raspberry Pi 3", "Raspberry Pi 2 v1.2",
      "---", "Raspberry Pi 3 Model B", "Raspberry Pi 2 v1.2", "Raspberry Pi 2 v1.1",
    ]);
  });

  for (const [label, board, route] of [
    ["Raspberry Pi 2 v1.1", "pi2-v1.1", "2"],
    ["Raspberry Pi 2 v1.2", "pi2-v1.2", "3"],
    ["Raspberry Pi 3 Model B", "pi3-b", "3"],
  ]) {
    it(`selects ${label} without an ARM64 package route`, () => {
      const result = run(`
        WOR_IMAGE_FAMILY=iot-core
        select_rpi_board "${label}" || exit 91
        iot_core_validate_options || exit 92
        printf '%s|%s|' "$RPI_MODEL" "$WOR_TARGET_BOARD"; rpi_board_label; printf '\\n'
        if release_package_source drivers;then exit 93;fi
        if release_package_source uefi;then exit 94;fi
      `);
      assert.equal(result.status, 0, result.stderr);
      assert.equal(result.stdout, `${route}|${board}|${label}\n`);
      assert.match(result.stderr, /not desktop ARM64 release packages/);
    });
  }

  for (const label of ["Raspberry Pi 3", "Raspberry Pi 3B+", "Raspberry Pi 4 / Pi 400", "Raspberry Pi 5"]) {
    it(`rejects ambiguous or unsupported IoT selection ${label}`, () => {
      const result = run(`
        WOR_IMAGE_FAMILY=iot-core
        select_rpi_board "Raspberry Pi 2 v1.1" || exit 91
        if select_rpi_board "${label}";then exit 92;fi
        printf '%s|%s\\n' "$RPI_MODEL" "$WOR_TARGET_BOARD"
      `);
      assert.equal(result.status, 0, result.stderr);
      assert.equal(result.stdout, "2|pi2-v1.1\n");
      assert.match(result.stderr, /Unsupported IoT Core board/);
    });
  }

  it("refuses ARM64 IoT, ARM32 desktop and FFU in the desktop workflow", () => {
    const result = run(`
      WOR_IMAGE_FAMILY=iot-core WOR_IMAGE_ARCH=arm64
      if validate_image_family;then exit 91;fi
      WOR_IMAGE_FAMILY=desktop WOR_IMAGE_ARCH=arm32
      if validate_image_family;then exit 92;fi
      WOR_IMAGE_ARCH=arm64 SOURCE_FILE=flash.ffu
      if validate_image_family;then exit 93;fi
    `);
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.stderr, /ARM64 IoT Core is not a supported/);
    assert.match(result.stderr, /FFU media cannot use the desktop ARM64/);
  });

  it("clears incompatible media and unsafe desktop overrides on a GUI family change", () => {
    const result = run(`
      SOURCE_FILE=desktop.iso IOT_CORE_SHA256=old
      WINDOWS_ACCOUNT_SETUP=1 WINDOWS_ACCOUNT_PASSWORD=fixture-only SKIP_IMAGE_VERIFICATION=1
      select_windows_family "Windows 10 IoT Core (ARM32, legacy)" || exit 91
      printf '%s|%s|%s|%s|%s\\n' "$WOR_IMAGE_FAMILY" "$SOURCE_FILE" "$WINDOWS_ACCOUNT_SETUP" "$SKIP_IMAGE_VERIFICATION" "$WINDOWS_ACCOUNT_PASSWORD"
      SOURCE_FILE=flash.ffu
      select_windows_family "Windows 11" || exit 92
      printf '%s|%s\\n' "$WOR_IMAGE_FAMILY" "$SOURCE_FILE"
    `);
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.stdout, "iot-core||0|0|\ndesktop|\n");
  });

  it("requires an explicit physical IoT board rather than guessing from RPI_MODEL", () => {
    const result = run(`
      WOR_IMAGE_FAMILY=iot-core RPI_MODEL=3 WOR_TARGET_BOARD=''
      if iot_core_resolve_board;then exit 91;fi
      WOR_TARGET_BOARD=pi2-v1.1 RPI_MODEL=4
      iot_core_resolve_board || exit 92
      printf '%s\\n' "$RPI_MODEL"
    `);
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.stdout, "2\n");
    assert.match(result.stderr, /requires target.board/);
  });

  for (const setting of ["SKIP_IMAGE_VERIFICATION=1", "CAN_INSTALL_ON_SAME_DRIVE=0", "WINDOWS_ACCOUNT_SETUP=1"]) {
    it(`rejects incompatible core setting ${setting}`, () => {
      const result = run(`
        WOR_IMAGE_FAMILY=iot-core WOR_TARGET_BOARD=pi3-b
        ${setting}
        if iot_core_validate_options;then exit 91;fi
      `);
      assert.equal(result.status, 0, result.stderr);
      assert.ok(result.stderr.length > 0);
    });
  }

  it("offers only dry-run customization, preserves the FFU firmware configuration and explains the workflow", () => {
    const result = run(`
      WOR_IMAGE_FAMILY=iot-core WOR_TARGET_BOARD=pi3-b
      for key in oobe pi4 drivers language account locale uefi verify config;do
        if advanced_option_applies "$key";then exit 91;fi
      done
      advanced_option_applies dryrun || exit 92
      CONFIG_TXT=sentinel
      set_default_config_txt || exit 93
      [ "$CONFIG_TXT" == sentinel ] || exit 94
      iot_core_guidance
      progress_task_label iot_core_apply
    `);
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.stdout, /not the Windows desktop/);
    assert.match(result.stdout, /WoR-PE and desktop customization options do not apply/);
    assert.match(result.stdout, /Applying and verifying Windows 10 IoT Core/);
  });

  it("inspects without invoking any desktop downloads, sudo or target discovery", () => {
    const result = run(`
      WOR_IMAGE_FAMILY=iot-core WOR_TARGET_BOARD=pi2-v1.1 SOURCE_FILE="$PWD/image.ffu" DRY_RUN=1 DEVICE=''
      python3() { if [ "$2" == profile ];then command python3 "$@";else printf '%s\\n' '${metadata}';fi; }
      sudo() { exit 91; }
      choose_device() { exit 92; }
      setup() { exit 93; }
      release_package_source() { exit 94; }
      list_bids() { exit 95; }
      iot_core_run || exit 96
    `, { "image.ffu": "metadata-routing fixture only" });
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.stdout, /Operating system:\s+Windows 10 IoT Core ARM32 \(legacy\), build 17763.107/);
    assert.match(result.stdout, /Minimum drive size:\s+8000000000 bytes/);
    assert.match(result.stderr, /no drive was opened for writing/);
    assert.doesNotMatch(result.stdout, /Windows ARM64 drivers\t|Offline OOBE\t/);
  });

  it("rejects incompatible inspector output and changed previously approved content", () => {
    const result = run(`
      WOR_IMAGE_FAMILY=iot-core WOR_TARGET_BOARD=pi3-b SOURCE_FILE="$PWD/image.ffu"
      python3() { printf '%s\\n' '${metadata.replace("arm32", "arm64")}'; }
      if iot_core_inspect_source;then exit 91;fi
      python3() { if [ "$2" == profile ];then command python3 "$@";else printf '%s\\n' '${metadata}';fi; }
      IOT_CORE_SHA256=${"2".repeat(64)}
      if iot_core_inspect_source;then exit 92;fi
    `, { "image.ffu": "metadata-routing fixture only" });
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.stderr, /incompatible image metadata/);
    assert.match(result.stderr, /changed after inspection/);
  });

  for (const extension of ["iso", "msi"]) {
    it(`routes a local ${extension.toUpperCase()} through the verified native importer`, () => {
      const result = run(`
        WOR_IMAGE_FAMILY=iot-core WOR_TARGET_BOARD=pi3-b SOURCE_FILE="$PWD/package.${extension}"
        python3() {
          [ "$2" == prepare ] && [ "$4" == --cache-dir ] && [ "$6" == --json ] || return 91
          jq --arg source "$PWD/ready.ffu" '. + {source_file:$source, acquisition:"local-${extension}"}' <<'JSON'
${metadata}
JSON
        }
        iot_core_inspect_source || exit 92
        printf '%s|%s|%s\\n' "\${SOURCE_FILE##*/}" "$IOT_CORE_ACQUISITION" "$WOR_IOT_DOWNLOAD"
      `, { [`package.${extension}`]: "routing-only fixture" });
      assert.equal(result.status, 0, result.stderr);
      assert.equal(result.stdout, `ready.ffu|local-${extension}|0\n`);
    });
  }

  it("downloads only on explicit request and consumes that request after preparation", () => {
    const result = run(`
      WOR_IMAGE_FAMILY=iot-core WOR_TARGET_BOARD=pi3-b SOURCE_FILE='' WOR_IOT_DOWNLOAD=1
      python3() {
        [ "$2" == download ] && [ "$3" == --cache-dir ] && [ "$5" == --json ] || return 91
        jq --arg source "$PWD/ready.ffu" '. + {source_file:$source, acquisition:"download"}' <<'JSON'
${metadata}
JSON
      }
      iot_core_inspect_source || exit 92
      printf '%s|%s|%s\\n' "\${SOURCE_FILE##*/}" "$IOT_CORE_ACQUISITION" "$WOR_IOT_DOWNLOAD"
    `);
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.stdout, "ready.ffu|download|0\n");
  });

  it("does not silently choose between a local source and download request", () => {
    const result = run(`
      WOR_IMAGE_FAMILY=iot-core WOR_TARGET_BOARD=pi3-b SOURCE_FILE=local.iso WOR_IOT_DOWNLOAD=1
      python3() { printf 'UNEXPECTED_DOWNLOAD\\n' >&2; return 91; }
      if iot_core_inspect_source;then exit 92;fi
    `);
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.stderr, /either a local IoT image or an official download/);
    assert.doesNotMatch(result.stderr, /UNEXPECTED_DOWNLOAD/);
  });

  for (const status of [0, 7]) {
    it(`keeps private CLI logs and preserves workflow status ${status}`, () => {
      const result = run(`
        RUN_MODE=cli
        iot_core_run() { printf 'IoT fixture output\\n'; printf 'IoT fixture diagnostics\\n' >&2; return ${status}; }
        iot_core_run_logged
      `);
      assert.equal(result.status, status, result.stderr);
      assert.equal(result.log, "IoT fixture output\nIoT fixture diagnostics\n");
      assert.equal(result.lastLog, result.log);
      assert.equal(result.logMode, 0o600);
    });
  }

  it("preserves the CLI input stream while recording output", () => {
    const result = run(`
      RUN_MODE=cli
      iot_core_run() { read -r value; printf 'received:%s\\n' "$value"; }
      iot_core_run_logged <<'INPUT'
fixture-choice
INPUT
    `);
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.log, "received:fixture-choice\n");
  });

  it("refuses to overwrite a pre-existing CLI log", () => {
    const result = run(`
      RUN_MODE=cli
      iot_core_run() { printf 'UNEXPECTED_WORKFLOW\\n'; }
      iot_core_run_logged
    `, { "run.log": "previous run\n" });
    assert.notEqual(result.status, 0);
    assert.equal(result.log, "previous run\n");
    assert.doesNotMatch(result.stdout, /UNEXPECTED_WORKFLOW/);
  });

  it("uses FFU capacity, not the 25 GiB desktop recovery threshold", () => {
    const result = run(`
      WOR_IMAGE_FAMILY=iot-core WOR_TARGET_BOARD=pi3-b IOT_CORE_MINIMUM_BYTES=8000000000
      get_size_raw() { printf '%s\\n' "$SIZE"; }
      SIZE=7999999999; drive_capability /dev/mock
      SIZE=8000000000; drive_capability /dev/mock
      SIZE=16000000000; drive_capability /dev/mock
      CAN_INSTALL_ON_SAME_DRIVE=1
      validate_install_mode install
    `);
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.stdout, "too-small\ninstall\ninstall\n");
  });

  it("resolves Linux boot-volume ancestry instead of comparing only a mapper or partition name", () => {
    const result = run(`
      HOST_OS=Linux DEVICE=/dev/mock-host ROOT_DEV=/dev/mapper/root
      resolve_path() { printf '%s\\n' "$1"; }
      findmnt() { printf '/dev/mapper/root[/@]\\n'; }
      lsblk() {
        [ "$1" == -slnpo ] && [ "$2" == PATH ] && [ "$3" == /dev/mapper/root ] || return 91
        printf '/dev/mapper/root\\n/dev/mock-host2\\n/dev/mock-host\\n'
      }
      if iot_core_validate_device;then exit 92;fi
    `);
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.stderr, /backs the current boot volume/);
  });

  it("protects the physical backing store of a macOS APFS boot container", () => {
    const result = run(`
      HOST_OS=Darwin DEVICE=/dev/mock-host ROOT_DEV=/dev/mock-container
      resolve_path() { printf '%s\\n' "$1"; }
      darwin_device_value() {
        case "$1" in
          /) printf 'mock-container\\n' ;;
          /dev/mock-container) printf 'Virtual\\n' ;;
          /dev/mock-host2) printf 'mock-host\\n' ;;
          *) return 91 ;;
        esac
      }
      darwin_plist_json() {
        printf '%s\\n' '{"Containers":[{"ContainerReference":"mock-container","PhysicalStores":[{"DeviceIdentifier":"mock-host2"}]}]}'
      }
      if iot_core_validate_device;then exit 92;fi
    `);
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.stderr, /backs the current boot volume/);
  });

  for (const protectedPath of ["image", "runtime"]) {
    it(`rejects a target that holds the ${protectedPath} before unmounting`, () => {
      const result = run(`
        DEVICE=/dev/mock-target SOURCE_FILE=/tmp/image.ffu
        iot_core_backing_disks() {
          case "$1" in
            /tmp/image.ffu) printf '/dev/mock-${protectedPath === "image" ? "target" : "source"}\\n' ;;
            *) printf '/dev/mock-${protectedPath === "runtime" ? "target" : "runtime"}\\n' ;;
          esac
        }
        if iot_core_protect_source_disks;then exit 91;fi
      `);
      assert.equal(result.status, 0, result.stderr);
      assert.match(result.stderr, /contains the source image or running installer/);
    });
  }

  it("refuses to write if source/runtime disk ancestry cannot be determined", () => {
    const result = run(`
      DEVICE=/dev/mock-target SOURCE_FILE=/tmp/image.ffu
      iot_core_backing_disks() { return 1; }
      if iot_core_protect_source_disks;then exit 91;fi
    `);
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.stderr, /Cannot determine the disks backing/);
  });

  it("clears target identity and erase consent when another target is selected", () => {
    const result = run(`
      IOT_CORE_TARGET_ID=1:2:3 IOT_CORE_TARGET_BYTES=16000000000 WOR_IOT_CONFIRM_ERASE=1
      iot_core_clear_target_approval
      printf '%s|%s|%s\\n' "$IOT_CORE_TARGET_ID" "$IOT_CORE_TARGET_BYTES" "$WOR_IOT_CONFIRM_ERASE"
    `);
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.stdout, "||0\n");
  });

  it("rejects a same-size target replacement after approval", () => {
    const result = run(`
      HOST_OS=Linux DEVICE=/dev/mock-target IOT_CORE_MINIMUM_BYTES=8000000000 WOR_IOT_CONFIRM_ERASE=0
      resolve_path() { printf '%s\\n' "$1"; }
      iot_core_boot_disks() { printf '/dev/mock-host\\n'; }
      iot_core_protect_source_disks() { return 0; }
      is_safe_target_device() { return 0; }
      [() {
        if builtin [ "\${1:-}" == -b ];then return 0;fi
        builtin [ "$@"
      }
      lsblk() {
        case "$2" in TYPE) printf 'disk\\n';; RO) printf '0\\n';; LOG-SEC) printf '512\\n';; PTTYPE) printf 'dos\\n';; *) return 91;; esac
      }
      get_size_raw() { printf '16000000000\\n'; }
      python3() { printf '{"target_id":"%s"}\\n' "$FAKE_ID"; }
      FAKE_ID=1:2:3
      iot_core_validate_device || exit 92
      [ "$IOT_CORE_TARGET_ID" == 1:2:3 ] || exit 93
      FAKE_ID=1:4:5
      if iot_core_validate_device;then exit 94;fi
    `);
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.stderr, /target device changed since it was selected/);
  });

  it("accepts read-only GPT selection and binds the layout without starting cleanup", () => {
    const result = run(`
      HOST_OS=Linux DEVICE=/dev/mock-target IOT_CORE_MINIMUM_BYTES=8000000000
      IOT_CORE_TARGET_ID='' IOT_CORE_TARGET_BYTES='' IOT_CORE_TARGET_LAYOUT=''
      resolve_path() { printf '%s\\n' "$1"; }
      iot_core_boot_disks() { printf '/dev/mock-host\\n'; }
      is_safe_target_device() { return 0; }
      [() {
        if builtin [ "\${1:-}" == -b ];then return 0;fi
        builtin [ "$@"
      }
      lsblk() {
        case "$2" in TYPE) printf 'disk\\n';; RO) printf '0\\n';; LOG-SEC) printf '512\\n';; PTTYPE) printf 'gpt\\n';; *) return 91;; esac
      }
      get_size_raw() { printf '16000000000\\n'; }
      python3() {
        [ "$2" == identify-target ] || { printf 'UNEXPECTED_WRITER\\n' >&2; return 92; }
        printf '{"target_id":"1:2:3"}\\n'
      }
      iot_core_protect_source_disks() { return 0; }
      iot_core_validate_device preview || exit 94
      printf '%s|%s|%s\\n' "$IOT_CORE_TARGET_LAYOUT" "$IOT_CORE_TARGET_ID" "\${WOR_IOT_CONFIRM_ERASE:-0}"
    `);
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.stdout, "gpt|1:2:3|0\n");
    assert.doesNotMatch(result.stderr, /UNEXPECTED_/);
  });

  it("refuses a same-device partition scheme change after selection", () => {
    const result = run(`
      HOST_OS=Linux DEVICE=/dev/mock-target IOT_CORE_MINIMUM_BYTES=8000000000
      IOT_CORE_TARGET_ID='' IOT_CORE_TARGET_BYTES='' IOT_CORE_TARGET_LAYOUT=''
      resolve_path() { printf '%s\\n' "$1"; }
      iot_core_boot_disks() { printf '/dev/mock-host\\n'; }
      iot_core_protect_source_disks() { return 0; }
      is_safe_target_device() { return 0; }
      [() {
        if builtin [ "\${1:-}" == -b ];then return 0;fi
        builtin [ "$@"
      }
      FAKE_LAYOUT=gpt
      lsblk() {
        case "$2" in TYPE) printf 'disk\\n';; RO) printf '0\\n';; LOG-SEC) printf '512\\n';; PTTYPE) printf '%s\\n' "$FAKE_LAYOUT";; *) return 91;; esac
      }
      get_size_raw() { printf '16000000000\\n'; }
      python3() { printf '{"target_id":"1:2:3"}\\n'; }
      iot_core_validate_device preview || exit 92
      FAKE_LAYOUT=dos
      if iot_core_validate_device;then exit 93;fi
      printf '%s\\n' "$IOT_CORE_TARGET_LAYOUT"
    `);
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.stdout, "gpt\n");
    assert.match(result.stderr, /partition layout changed since selection/);
  });

  it("accepts macOS signed native device IDs without losing replacement protection", () => {
    const result = run(`
      HOST_OS=Darwin DEVICE=/dev/disk999 IOT_CORE_MINIMUM_BYTES=8000000000
      IOT_CORE_TARGET_ID='' IOT_CORE_TARGET_BYTES='' IOT_CORE_TARGET_LAYOUT=''
      resolve_path() { printf '%s\\n' "$1"; }
      iot_core_boot_disks() { printf '/dev/mock-host\\n'; }
      iot_core_protect_source_disks() { return 0; }
      is_safe_target_device() { return 0; }
      [() { if builtin [ "\${1:-}" == -b ];then return 0;fi; builtin [ "$@"; }
      darwin_device_value() { case "$2" in .DeviceBlockSize) printf '512\\n';; *) printf 'FDisk_partition_scheme\\n';; esac; }
      get_size_raw() { printf '16000000000\\n'; }
      FAKE_ID=-2088985291:687:16777240
      python3() {
        [ "$2" == identify-target ] && [ "$3" == /dev/rdisk999 ] || return 91
        printf '{"target_id":"%s"}\\n' "$FAKE_ID"
      }
      iot_core_validate_device preview || exit 92
      printf '%s\\n' "$IOT_CORE_TARGET_ID"
      FAKE_ID=-2088985291:688:16777240
      if iot_core_validate_device;then exit 93;fi
    `);
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.stdout, "-2088985291:687:16777240\n");
    assert.match(result.stderr, /target device changed since it was selected/);
  });

  it("never requests writing or GPT cleanup without erase consent and prior identities", () => {
    const result = run(`
      HOST_OS=Darwin DEVICE=/dev/disk999 SOURCE_FILE=/tmp/fixture.ffu
      IOT_CORE_SHA256=${hash} IOT_CORE_TARGET_ID=1:2:3
      WOR_IOT_CONFIRM_ERASE=0
      sudo() { printf 'UNEXPECTED_WRITE\\n' >&2; return 91; }
      if iot_core_apply;then exit 92;fi
      WOR_IOT_CONFIRM_ERASE=1 IOT_CORE_TARGET_ID=''
      if iot_core_apply;then exit 93;fi
    `);
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.stderr, /explicit erase consent and verified source\/target identities/);
    assert.doesNotMatch(result.stderr, /UNEXPECTED_WRITE/);
  });

  it("loads and exports profile settings and mandatory verification without treating false as missing", () => {
    const config = JSON.stringify({
      target: { board: "pi2-v1.1" },
      media: { imageFamily: "iot-core", architecture: "arm32" },
      execution: { confirmIotErase: false },
    });
    const result = run(`
      unset WOR_TARGET_BOARD WOR_IMAGE_FAMILY WOR_IMAGE_ARCH WOR_IOT_CONFIRM_ERASE
      load_config_json "$PWD/iot.json"
      iot_core_resolve_board || exit 91
      export_installer_settings
      bash -c 'printf "%s|%s|%s|%s|%s\\n" "$WOR_IMAGE_FAMILY" "$WOR_IMAGE_ARCH" "$WOR_TARGET_BOARD" "$RPI_MODEL" "$WOR_IOT_CONFIRM_ERASE"'
    `, { "iot.json": config });
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.stdout, "iot-core|arm32|pi2-v1.1|2|0\n");
  });

  for (const family of ["desktop", "iot-core"]) {
    it(`honors an explicitly disabled completion notification for ${family}`, () => {
      const result = run(`
        unset SHOW_NOTIFICATION
        WOR_IMAGE_FAMILY=${family}
        load_config_json "$PWD/notification.json"
        printf '%s\\n' "$SHOW_NOTIFICATION"
      `, { "notification.json": JSON.stringify({ notifications: { showNotification: false } }) });
      assert.equal(result.status, 0, result.stderr);
      assert.equal(result.stdout, "0\n");
    });
  }

  it("loads explicit --config before defaults so an inspection-only request cannot become a write", () => {
    const config = JSON.stringify({
      target: { board: "pi2-v1.1" },
      media: { imageFamily: "iot-core", architecture: "arm32", sourceFile: "./image.ffu" },
      execution: { dryRun: true, confirmIotErase: false },
    });
    const result = run(`
      python3() { if [ "$2" == profile ];then command python3 "$@";else printf '%s\\n' '${metadata}';fi; }
      export -f python3
      exec bash "$DIRECTORY/install-wor.sh" --config "$PWD/iot.json"
    `, { "iot.json": config, "image.ffu": "metadata-routing fixture only" });
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.stdout, /Raspberry Pi 2 v1.1/);
    assert.match(result.stdout + result.stderr, /DRY_RUN=1: no drive was opened for writing/);
    assert.doesNotMatch(result.stderr, /Checking for internet|Downloading|Partitioning/);
  });

  it("does not treat RUN_MODE=gui as erase consent", () => {
    const result = run(`
      WOR_IMAGE_FAMILY=iot-core WOR_TARGET_BOARD=pi3-b SOURCE_FILE="$PWD/image.ffu"
      DRY_RUN=0 DEVICE=/dev/mock-only RUN_MODE=gui WOR_IOT_CONFIRM_ERASE=0
      gui_error_dialog() { :; }
      python3() { if [ "$2" == profile ];then command python3 "$@";else printf '%s\\n' '${metadata}';fi; }
      detect_root_dev() { ROOT_DEV=/dev/host; }
      iot_core_validate_device() { IOT_CORE_TARGET_BYTES=8000000000; }
      sudo() { printf 'UNEXPECTED_SUDO\\n' >&2; exit 91; }
      iot_core_run
    `, { "image.ffu": "metadata-routing fixture only" });
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /explicit erase confirmation/);
    assert.doesNotMatch(result.stderr, /UNEXPECTED_SUDO/);
  });

  it("rechecks the target after authentication and refuses a changed target before unmount/write", () => {
    const result = run(`
      WOR_IMAGE_FAMILY=iot-core WOR_TARGET_BOARD=pi3-b SOURCE_FILE="$PWD/image.ffu"
      DRY_RUN=0 DEVICE=/dev/mock-only RUN_MODE=cli WOR_IOT_CONFIRM_ERASE=1
      python3() { if [ "$2" == profile ];then command python3 "$@";else printf '%s\\n' '${metadata}';fi; }
      detect_root_dev() { ROOT_DEV=/dev/host; }
      checked=0
      iot_core_validate_device() { [ "\${1:-}" != preview ] || return 0; checked=$((checked+1)); [ "$checked" == 1 ]; }
      sudo() { [ "$1" == -v ] || exit 91; }
      iot_core_apply() { printf 'UNEXPECTED_WRITE\\n' >&2; exit 92; }
      diskutil() { printf 'UNEXPECTED_UNMOUNT\\n' >&2; exit 93; }
      iot_core_run
    `, { "image.ffu": "metadata-routing fixture only" });
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /target changed or is no longer safe/);
    assert.doesNotMatch(result.stderr, /UNEXPECTED_WRITE|UNEXPECTED_UNMOUNT/);
  });

  it("binds the privileged writer to the inspected image digest and raw macOS target", () => {
    const result = run(`
      HOST_OS=Darwin DEVICE=/dev/disk999 SOURCE_FILE=/tmp/fixture.ffu WOR_IOT_CONFIRM_ERASE=1
      IOT_CORE_TARGET_BYTES=16000000000 IOT_CORE_SHA256=${hash} IOT_CORE_TARGET_ID=1:2:3
      sudo() { printf '%s\\n' "$@"; }
      iot_core_apply
    `);
    assert.equal(result.status, 0, result.stderr);
    assert.deepEqual(result.stdout.trim().split("\n"), [
      "python3", join(root, "src/lib/iot-ffu.py"), "apply", "/tmp/fixture.ffu",
      "/dev/rdisk999", "--target-size", "16000000000", "--expected-sha256", hash,
      "--expected-target-id=1:2:3",
      "--allow-gpt-cleanup", "--hdmi-mode", "official", "--hdmi-config",
    ]);
  });

  it("loads HDMI JSON preferences, preserves overrides and exports the exact custom text", () => {
    const config = "hdmi_force_hotplug=1\nhdmi_group=1\nhdmi_mode=4";
    const result = run(`
      unset IOT_CORE_HDMI_MODE IOT_CORE_HDMI_CONFIG
      load_config_json "$PWD/hdmi.json"
      export_installer_settings
      bash -c 'printf "%s\\n%s\\n" "$IOT_CORE_HDMI_MODE" "$IOT_CORE_HDMI_CONFIG"'
      IOT_CORE_HDMI_MODE=1080p60
      load_config_json "$PWD/hdmi.json"
      printf '%s\\n' "$IOT_CORE_HDMI_MODE"
    `, { "hdmi.json": JSON.stringify({ customization: { iotHdmiMode: "custom", iotHdmiConfig: config } }) });
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.stdout, `custom\n${config}\n1080p60\n`);
  });

  it("exports and forwards the recommended compatibility mode only when no preference is set", () => {
    const result = run(`
      unset IOT_CORE_HDMI_MODE
      export_installer_settings
      printf '%s\\n' "$IOT_CORE_HDMI_MODE"
      IOT_CORE_HDMI_MODE=official
      export_installer_settings
      printf '%s\\n' "$IOT_CORE_HDMI_MODE"
      unset IOT_CORE_HDMI_MODE
      HOST_OS=Darwin DEVICE=/dev/disk999 SOURCE_FILE=/tmp/fixture.ffu WOR_IOT_CONFIRM_ERASE=1
      IOT_CORE_TARGET_BYTES=16000000000 IOT_CORE_SHA256=${hash} IOT_CORE_TARGET_ID=1:2:3
      sudo() { printf '%s\\0' "$@"; }
      iot_core_apply
    `);
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.stdout, /^720p60\nofficial\n/);
    const args = result.stdout.slice("720p60\nofficial\n".length).split("\0").slice(0, -1);
    assert.deepEqual(args.slice(-4), ["--hdmi-mode", "720p60", "--hdmi-config", ""]);
  });

  for (const mode of ["official", "1080p60", "custom"]) {
    it(`preserves the explicit JSON video preference ${mode} over the compatibility default`, () => {
      const result = run(`
        unset IOT_CORE_HDMI_MODE IOT_CORE_HDMI_CONFIG
        load_config_json "$PWD/video.json"
        export_installer_settings
        printf '%s\\n' "$IOT_CORE_HDMI_MODE"
        iot_core_hdmi_state | jq -r '.mode, .recommended'
      `, { "video.json": JSON.stringify({ customization: {
        iotHdmiMode: mode, iotHdmiConfig: mode === "custom" ? "hdmi_group=2\nhdmi_mode=16" : "",
      } }) });
      assert.equal(result.status, 0, result.stderr);
      assert.equal(result.stdout, `${mode}\n${mode}\nfalse\n`);
    });
  }

  it("loads, exports and preserves explicit IoT language and disabled setup JSON preferences", () => {
    const result = run(`
      unset IOT_CORE_LANGUAGE IOT_CORE_LANGUAGE_SETUP
      load_config_json "$PWD/language.json"
      export_installer_settings
      bash -c 'printf "%s|%s\\n" "$IOT_CORE_LANGUAGE" "$IOT_CORE_LANGUAGE_SETUP"'
      IOT_CORE_LANGUAGE=de-DE
      load_config_json "$PWD/language.json"
      printf '%s\\n' "$IOT_CORE_LANGUAGE"
    `, {"language.json":JSON.stringify({userAccount:{iotCore:{language:"fr-FR",languageSetup:false}}})});
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.stdout, "fr-FR|0\nde-DE\n");
  });

  it("loads current IoT connection preferences separately from desired account credentials", () => {
    const result = run(`
      unset IOT_CORE_HOST IOT_CORE_CURRENT_USERNAME IOT_CORE_CURRENT_PASSWORD
      IOT_CORE_ACCOUNT_PASSWORD=Fixture-desired-456
      load_config_json "$PWD/connection.json"
      [ "$IOT_CORE_HOST" == pi-fixture.local ] && [ "$IOT_CORE_CURRENT_USERNAME" == RenamedAdmin ] || exit 91
      [ "$IOT_CORE_CURRENT_PASSWORD" == Fixture-current-123 ] && [ "$IOT_CORE_ACCOUNT_PASSWORD" == Fixture-desired-456 ] || exit 92
      IOT_CORE_HOST=explicit.local
      load_config_json "$PWD/connection.json"
      [ "$IOT_CORE_HOST" == explicit.local ] || exit 93
    `, {"connection.json":JSON.stringify({userAccount:{iotCore:{
      host:"pi-fixture.local", currentUsername:"RenamedAdmin", currentPassword:"Fixture-current-123",
    }}})});
    assert.equal(result.status, 0, result.stderr);
    assert.doesNotMatch(result.stdout + result.stderr, /Fixture-(current|desired)-[0-9]+/);
  });

  it("preserves explicit manual addressing and false JSON values through export", () => {
    const result = run(`
      unset IOT_CORE_AUTOMATIC_ADDRESS IOT_CORE_HOST
      [ "$(iot_core_automatic_address)" == 1 ] || exit 91
      IOT_CORE_HOST=192.168.50.23
      [ "$(iot_core_automatic_address)" == 0 ] || exit 92
      unset IOT_CORE_HOST
      load_config_json "$PWD/address.json"
      export_installer_settings
      bash -c 'printf "%s|%s\\n" "$IOT_CORE_AUTOMATIC_ADDRESS" "$IOT_CORE_HOST"'
      IOT_CORE_AUTOMATIC_ADDRESS=1
      load_config_json "$PWD/address.json"
      [ "$(iot_core_automatic_address)" == 1 ] || exit 93
    `, {"address.json":JSON.stringify({userAccount:{iotCore:{host:"192.168.50.23",automaticAddress:false}}})});
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.stdout, "0|192.168.50.23\n");
  });

  it("preserves explicit empty current-login configuration for validation instead of using factory values", () => {
    const result = run(`
      unset IOT_CORE_CURRENT_USERNAME IOT_CORE_CURRENT_PASSWORD
      load_config_json "$PWD/empty-login.json"
      [ "\${IOT_CORE_CURRENT_USERNAME+x}" == x ] && [ -z "$IOT_CORE_CURRENT_USERNAME" ] || exit 91
      [ "\${IOT_CORE_CURRENT_PASSWORD+x}" == x ] && [ -z "$IOT_CORE_CURRENT_PASSWORD" ] || exit 92
      load_config_json "$PWD/valid-login.json"
      [ -z "$IOT_CORE_CURRENT_USERNAME" ] && [ -z "$IOT_CORE_CURRENT_PASSWORD" ] || exit 93
    `, {
      "empty-login.json": JSON.stringify({userAccount:{iotCore:{currentUsername:"",currentPassword:""}}}),
      "valid-login.json": JSON.stringify({userAccount:{iotCore:{currentUsername:"Administrator",currentPassword:"Fixture-current-123"}}}),
    });
    assert.equal(result.status, 0, result.stderr);
  });

  for (const field of ["currentUsername", "currentPassword"]) {
    it(`rejects non-string current login configuration for ${field}`, () => {
      const result = run(`
        unset IOT_CORE_CURRENT_USERNAME IOT_CORE_CURRENT_PASSWORD
        load_config_json "$PWD/invalid-login.json"
        printf 'UNEXPECTED_ACCEPTANCE\\n'
      `, {"invalid-login.json": JSON.stringify({userAccount:{iotCore:{[field]:42}}})});
      assert.notEqual(result.status, 0);
      assert.match(result.stderr, /must be a string in configuration/);
      assert.doesNotMatch(result.stdout, /UNEXPECTED_ACCEPTANCE/);
    });
  }

  it("passes HDMI customization through the same privileged FFU writer without a separate sudo or mount", () => {
    const config = "hdmi_group=1\nhdmi_mode=4";
    const result = run(`
      HOST_OS=Darwin DEVICE=/dev/disk999 SOURCE_FILE=/tmp/fixture.ffu WOR_IOT_CONFIRM_ERASE=1
      IOT_CORE_TARGET_BYTES=16000000000 IOT_CORE_SHA256=${hash} IOT_CORE_TARGET_ID=1:2:3
      IOT_CORE_HDMI_MODE=custom IOT_CORE_HDMI_CONFIG=$'hdmi_group=1\\nhdmi_mode=4'
      sudo() { printf '%s\\0' "$@"; }
      iot_core_apply
    `);
    assert.equal(result.status, 0, result.stderr);
    const argumentsPassed = result.stdout.split("\0").slice(0, -1);
    assert.equal(argumentsPassed[0], "python3");
    assert.equal(argumentsPassed[2], "apply");
    assert.deepEqual(argumentsPassed.slice(-4), ["--hdmi-mode", "custom", "--hdmi-config", config]);
    for (const required of ["--expected-sha256", "--expected-target-id=1:2:3", "--allow-gpt-cleanup"]) {
      assert.ok(argumentsPassed.includes(required));
    }
  });

  it("requires separate full-wipe consent and forwards the signed identity safely to the existing writer", () => {
    const result = run(`
      HOST_OS=Darwin DEVICE=/dev/disk999 SOURCE_FILE=/tmp/fixture.ffu WOR_IOT_CONFIRM_ERASE=1
      IOT_CORE_SHA256=${hash} IOT_CORE_TARGET_BYTES=16000000000 IOT_CORE_TARGET_ID=-2088985291:687:16777240
      IOT_CORE_WIPE_DRIVE=1 WOR_IOT_CONFIRM_WIPE=0
      sudo() { printf 'UNEXPECTED_WIPE\\n' >&2; return 91; }
      if iot_core_apply;then exit 92;fi
      WOR_IOT_CONFIRM_WIPE=1
      sudo() { printf '%s\\0' "$@"; }
      iot_core_apply || exit 93
    `);
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.stderr, /separate explicit wipe confirmation/);
    assert.doesNotMatch(result.stderr, /UNEXPECTED_WIPE/);
    const args = result.stdout.split("\0").slice(0, -1);
    assert.equal(args[0], "python3");
    assert.equal(args[2], "apply");
    assert.ok(args.includes("--wipe-entire-drive"));
    assert.ok(args.includes("--expected-target-id=-2088985291:687:16777240"));
    assert.ok(!args.includes("--allow-gpt-cleanup"));
  });

  it("loads full-wipe settings without discarding an explicit false and exports separate consent", () => {
    const result = run(`
      unset IOT_CORE_WIPE_DRIVE WOR_IOT_CONFIRM_WIPE
      load_config_json "$PWD/reset.json"
      export_installer_settings
      printf '%s|%s\\n' "$IOT_CORE_WIPE_DRIVE" "$WOR_IOT_CONFIRM_WIPE"
      IOT_CORE_WIPE_DRIVE=1
      load_config_json "$PWD/reset.json"
      printf '%s|%s\\n' "$IOT_CORE_WIPE_DRIVE" "$WOR_IOT_CONFIRM_WIPE"
      WOR_IOT_CONFIRM_WIPE=1
      iot_core_clear_target_approval
      printf '%s\\n' "$WOR_IOT_CONFIRM_WIPE"
    `, {"reset.json": JSON.stringify({execution:{wipeIotDrive:false,confirmIotWipe:false}})});
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.stdout, "0|0\n1|0\n0\n");
  });
  it("never reports success or ejects after an application/verification error", () => {
    const result = run(`
      WOR_IMAGE_FAMILY=iot-core WOR_TARGET_BOARD=pi3-b SOURCE_FILE="$PWD/image.ffu"
      DRY_RUN=0 DEVICE=/dev/mock-only RUN_MODE=cli WOR_IOT_CONFIRM_ERASE=1
      python3() { if [ "$2" == profile ];then command python3 "$@";else printf '%s\\n' '${metadata}';fi; }
      detect_root_dev() { ROOT_DEV=/dev/host; }
      iot_core_validate_device() { IOT_CORE_TARGET_BYTES=8000000000; }
      sudo() { [ "$1" == -v ] || { printf 'UNEXPECTED_EJECT\\n' >&2; exit 91; }; }
      diskutil() { [ "$1" == unmountDisk ] || { printf 'UNEXPECTED_EJECT\\n' >&2; exit 92; }; }
      lsblk() { printf '/dev/mock-only\\n'; }
      findmnt() { return 1; }
      iot_core_apply() { printf 'Fixture verification failure\\n' >&2; return 1; }
      iot_core_run
    `, { "image.ffu": "metadata-routing fixture only" });
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /application or read-back verification failed/);
    assert.doesNotMatch(result.stderr, /UNEXPECTED_EJECT|FFU written and verified/);
  });

  it("wires both GUI paths and never displays desktop advanced options for IoT", () => {
    assert.match(gui, /const windows = \[.*Windows 10 IoT Core \(ARM32, legacy\)/);
    assert.match(gui, /wor_rpi_board_options iot-core/);
    assert.match(gui, /familyChanged:/);
    assert.match(gui, /macos_advanced_options\(\)[^\n]*\n  if is_iot_core;then\n    macos_iot_options/);
    assert.match(gui, /wor_iot_option_label file/);
    assert.match(gui, /linux_iot_options/);
    assert.doesNotMatch(gui, /macos_iot_select_source|Download the reviewed image\\nUse a local FFU/);
    assert.match(gui, /if is_iot_core;then step=device;else step=mode;fi/);
    assert.match(gui, /iot_core_next_steps/);
  });

  it("updates the native board picker when the image family changes", () => {
    const source = gui.match(/target_jxa="\$\(wor_jxa_window_lib; cat <<'JXA'\n([\s\S]*?)\nJXA/)[1];
    const desktop = ["Raspberry Pi 5", "Raspberry Pi 4 / Pi 400", "Raspberry Pi 3", "Raspberry Pi 2 v1.2"];
    const iot = ["Raspberry Pi 3 Model B", "Raspberry Pi 2 v1.2", "Raspberry Pi 2 v1.1"];
    const popup = {
      items: [...desktop], indexOfSelectedItem: 0,
      get titleOfSelectedItem() { return this.items[this.indexOfSelectedItem]; },
      get removeAllItems() { this.items = []; return undefined; },
      addItemWithTitle(title) { this.items.push(title); },
      selectItemAtIndex(index) { this.indexOfSelectedItem = index; },
    };
    const dollar = Object.assign((value) => value, { NSObject: {}, NSOKButton: 1 });
    const context = vm.createContext({
      $: dollar, ObjC: { unwrap: (value) => value, registerSubclass: (value) => value },
      args: { objectAtIndex: (index) => (index === 9 ? desktop : iot).join("\n") },
      defaultWindows: "Windows 11", defaultPiModel: "Raspberry Pi 5",
      app: { stopModalWithCode() {} }, windowsPopup: { indexOfSelectedItem: 2 }, piPopup: popup,
    });
    vm.runInContext(source.slice(source.indexOf("const windows ="), source.indexOf("function writeResult")), context);
    vm.runInContext(source.slice(source.indexOf("const Controller ="), source.indexOf("const controller =")), context);
    vm.runInContext("Controller.methods['familyChanged:'].implementation()", context);
    assert.deepEqual(popup.items, iot);
    assert.equal(popup.indexOfSelectedItem, 0);
    popup.indexOfSelectedItem = 2;
    vm.runInContext("window = { orderOut() {} }; Controller.methods['nextClicked:'].implementation()", context);
    assert.equal(vm.runInContext("selectedValue", context), "Windows 10 IoT Core (ARM32, legacy)\tRaspberry Pi 2 v1.1");
    context.windowsPopup.indexOfSelectedItem = 0;
    vm.runInContext("Controller.methods['familyChanged:'].implementation()", context);
    assert.deepEqual(popup.items, desktop);
    assert.equal(popup.indexOfSelectedItem, 0);
  });
});

it("uses the post-confirmation installer instead of a separate eager IoT preparation window", () => {
  assert.match(gui, /gui_iot_plan_source/);
  assert.doesNotMatch(gui, /gui_iot_prepare_source|WorIotPreparationController/);
  assert.match(gui, /iot_core_validate_device preview/);
});

it("validates the FFU parser with offline Python fixtures", () => {
  const result = spawnSync("python3", ["-B", join(root, "tests/test-iot-ffu.py")], {
    cwd: root, encoding: "utf8", timeout: 120000,
  });
  assert.equal(result.status, 0, `${result.stdout}\n${result.stderr}`);
});

it("validates native IoT media acquisition with offline Python fixtures", () => {
  const result = spawnSync("python3", ["-B", join(root, "tests/test-iot-media.py")], {
    cwd: root, encoding: "utf8", timeout: 120000,
  });
  assert.equal(result.status, 0, `${result.stdout}\n${result.stderr}`);
});
