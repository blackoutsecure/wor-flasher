import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";

const root = fileURLToPath(new URL("../", import.meta.url));
const booleanSettings = [
  ["CAN_INSTALL_ON_SAME_DRIVE", "target", "canInstallOnSameDrive"],
  ["APPLY_CUSTOM_CONFIG_TXT", "customization", "applyCustomConfigTxt"],
  ["OOBE_NETWORK_BYPASS", "customization", "oobeNetworkBypass"],
  ["PI4_AUTO_DISABLE_3GB", "customization", "pi4AutoDisable3Gb"],
  ["PI4_UEFI_SHELL_UNLOCK", "customization", "pi4UefiShellUnlock"],
  ["UEFI_USE_LATEST", "customization", "uefiUseLatest"],
  ["DRIVERS_USE_LATEST", "customization", "driversUseLatest"],
  ["HIDE_EMPTY_DRIVES", "customization", "hideEmptyDrives"],
  ["WINDOWS_ACCOUNT_SETUP", "userAccount", "setupAccount"],
  ["WINDOWS_LOCALE_SETUP", "userAccount", "localeSetup"],
  ["DRY_RUN", "execution", "dryRun"],
  ["SKIP_IMAGE_VERIFICATION", "execution", "skipImageVerification"],
  ["VERIFY_TLS", "execution", "verifyTls"],
  ["PLAY_SOUND", "notifications", "playSound"],
  ["SHOW_NOTIFICATION", "notifications", "showNotification"],
];

function run(body, { config = {}, argumentsFor = () => [], explicitConfig = true, env = {} } = {}) {
  const directory = mkdtempSync(join(tmpdir(), "wor-configuration-"));
  try {
    const configPath = join(directory, "config with spaces.json");
    writeFileSync(configPath, JSON.stringify(config));
    return spawnSync("/bin/bash", ["-c", `
      source "$DIRECTORY/install-wor.sh" source "$@" >/dev/null || exit 90
      ${body}
    `, "configuration-test", ...argumentsFor(configPath)], {
      cwd: directory, encoding: "utf8", timeout: 15000,
      env: {
        ...process.env, NO_UPDATE: "1", CHECK_FOR_UPDATES: "0", RUN_MODE: "cli",
        DIRECTORY: root, WOR_CACHE_DIR: join(directory, "cache"), TEST_DIR: directory,
        CONFIG_REPRO: configPath, WOR_CONFIG_FILE: explicitConfig ? configPath : "", ...env,
      },
    });
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
}

describe("JSON configuration contract", () => {
  for (const value of [false, true]) {
    it(`preserves explicit ${value} values ahead of legacy aliases`, () => {
      const config = {};
      for (const [variable, section, key] of booleanSettings) {
        (config[section] ??= {})[key] = value;
        config[variable] = !value;
      }
      config.userAccount.username = "fixture-user";
      config.userAccount.password = "fixture-not-a-secret";
      const variables = booleanSettings.map(([name]) => name);
      const result = run(`
        unset ${variables.join(" ")}
        load_config_json "$CONFIG_REPRO"
        ${variables.map((name) => `printf '${name}=%s\\n' "$${name}"`).join("\n")}
      `, { config });
      assert.equal(result.status, 0, result.stderr);
      assert.deepEqual(result.stdout.trim().split("\n"),
        variables.map((name) => `${name}=${value ? "1" : "0"}`));
    });
  }

  it("preserves explicit environment settings over JSON preferences", () => {
    const result = run(`
      PLAY_SOUND=1
      load_config_json "$CONFIG_REPRO"
      printf '%s' "$PLAY_SOUND"
    `, { config: { notifications: { playSound: false } } });
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.stdout, "1");
  });

  for (const value of [false, true]) {
    it(`honors execution.checkForUpdates=${value} ahead of aliases`, () => {
      const result = run(`
        unset NO_UPDATE CHECK_FOR_UPDATES
        load_config_json "$CONFIG_REPRO"
        printf '%s' "$NO_UPDATE"
      `, { config: { execution: { checkForUpdates: value }, checkForUpdates: !value } });
      assert.equal(result.status, 0, result.stderr);
      assert.equal(result.stdout, value ? "0" : "1");
    });
  }

  it("rejects malformed automatically discovered configuration", () => {
    const result = run(`
      unset WOR_CONFIG_FILE
      DIRECTORY="$TEST_DIR"
      printf '{invalid\\n' > "$DIRECTORY/config.json"
      load_config_json
    `);
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /Configuration must be a valid JSON object/);
  });

  it("rejects invalid boolean values instead of silently choosing defaults", () => {
    const result = run("printf 'unexpected success'", { config: { execution: { dryRun: "not-a-boolean" } } });
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /DRY_RUN must be a boolean/);
  });

  for (const equals of [false, true]) {
    it(`loads forwarded source-mode configuration arguments (${equals ? "=" : "separate"} form)`, () => {
      const result = run("printf '%s' \"$DRY_RUN\"", {
        config: { execution: { dryRun: true } }, explicitConfig: false,
        argumentsFor: (path) => equals ? [`--config=${path}`] : ["--config", path],
      });
      assert.equal(result.status, 0, result.stderr);
      assert.equal(result.stdout, "1");
    });
  }

  for (const args of [["--config"], ["--config="]]) {
    it(`rejects an incomplete configuration option ${args.join(" ")}`, () => {
      const result = run("printf 'unexpected success'", { argumentsFor: () => args });
      assert.notEqual(result.status, 0);
      assert.match(result.stderr, /--config requires a file path/);
    });
  }
});

describe("Desktop target validation", () => {
  for (const apfs of [false, true]) {
    it(`protects and hides physical macOS boot disks (${apfs ? "APFS stores" : "direct filesystem"})`, () => {
      const rootInfo = apfs ? {
        DeviceIdentifier: "disk3s1s1", ParentWholeDisk: "disk3",
        APFSPhysicalStores: [{ APFSPhysicalStore: "disk4s1" }, { APFSPhysicalStore: "disk5s2" }],
      } : { DeviceIdentifier: "disk4s2", ParentWholeDisk: "disk4" };
      const result = run(`
        HOST_OS=Darwin
        darwin_plist_json() {
          case "$2:$4" in
            info:/) printf '%s\\n' "$TEST_ROOT_INFO" ;;
            info:/dev/disk4s1) printf '{"ParentWholeDisk":"disk4"}\\n' ;;
            info:/dev/disk5s2) printf '{"ParentWholeDisk":"disk5"}\\n' ;;
            list:external) printf '{"AllDisks":["disk4","disk5","disk6"]}\\n' ;;
            *) return 99 ;;
          esac
        }
        resolve_path() { printf '%s\\n' "$1"; }
        darwin_is_safe_device() { return 0; }
        detect_root_dev
        printf 'ROOT\\n%s\\nDEVICES\\n' "$ROOT_DEV"
        ! is_safe_target_device /dev/disk4 || exit 91
        if [ "$TEST_APFS" == 1 ];then ! is_safe_target_device /dev/disk5 || exit 92;fi
        is_safe_target_device /dev/disk6 || exit 93
        darwin_list_device_paths
      `, { env: { TEST_ROOT_INFO: JSON.stringify(rootInfo), TEST_APFS: apfs ? "1" : "0" } });
      assert.equal(result.status, 0, result.stderr);
      assert.equal(result.stdout, apfs
        ? "ROOT\n/dev/disk4\n/dev/disk5\nDEVICES\n/dev/disk6\n"
        : "ROOT\n/dev/disk4\nDEVICES\n/dev/disk5\n/dev/disk6\n");
    });
  }

  for (const stores of [[], null, [{ APFSPhysicalStore: "invalid" }]]) {
    it(`rejects incomplete APFS physical-store metadata ${JSON.stringify(stores)}`, () => {
      const result = run(`
        HOST_OS=Darwin
        darwin_plist_json() { printf '%s\\n' "$TEST_ROOT_INFO"; }
        detect_root_dev
      `, { env: { TEST_ROOT_INFO: JSON.stringify({ ParentWholeDisk: "disk3", APFSPhysicalStores: stores }) } });
      assert.notEqual(result.status, 0);
      assert.match(result.stderr, /boot|APFS|physical/i);
    });
  }

  it("protects every whole disk behind a layered or multi-disk Linux root filesystem", () => {
    const result = run(`
      HOST_OS=Linux
      findmnt() { printf '/dev/mapper/root[/system]\\n'; }
      lsblk() {
        if [ "$1" == -snrpo ];then
          [ "$3" == /dev/mapper/root ] || return 99
          printf '/dev/mapper/root lvm\\n/dev/md0 raid1\\n/dev/sda2 part\\n/dev/sda disk\\n/dev/sdb2 part\\n/dev/sdb disk\\n'
        else
          case "$2" in TYPE) printf 'disk\\n';; RO) printf '0\\n';; *) return 99;; esac
        fi
      }
      resolve_path() { printf '%s\\n' "$1"; }
      detect_root_dev
      printf '%s\\n' "$ROOT_DEV"
      ! is_safe_target_device /dev/sda || exit 91
      ! is_safe_target_device /dev/sdb || exit 92
      is_safe_target_device /dev/sdc
    `);
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.stdout, "/dev/sda\n/dev/sdb\n");
  });

  it("fails closed when root-device discovery cannot find a whole backing disk", () => {
    const result = run(`
      HOST_OS=Linux
      findmnt() { printf '/dev/mapper/root\\n'; }
      lsblk() { printf '/dev/mapper/root lvm\\n'; }
      detect_root_dev
    `);
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /Failed to determine the current boot drives/);
  });

  for (const [name, type, readOnly, device, expected] of [
    ["writable whole disk", "disk", "0", "/dev/target", 0],
    ["owned loop-device test target", "loop", "0", "/dev/target", 0],
    ["partition", "part", "0", "/dev/target", 1],
    ["read-only disk", "disk", "1", "/dev/target", 1],
    ["unknown device type", "", "0", "/dev/target", 1],
    ["unreadable read-only flag", "disk", "", "/dev/target", 1],
    ["alias of the boot disk", "disk", "0", "/dev/root-alias", 1],
  ]) {
    it(`${expected === 0 ? "accepts" : "rejects"} ${name} without accessing a disk`, () => {
      const result = run(`
        HOST_OS=Linux ROOT_DEV=/dev/root-disk
        resolve_path() {
          if [ "$1" == /dev/root-alias ];then printf '/dev/root-disk\\n';else printf '%s\\n' "$1";fi
        }
        lsblk() {
          case "$2" in TYPE) printf '%s\\n' "$TEST_TYPE";; RO) printf '%s\\n' "$TEST_RO";; *) return 99;; esac
        }
        is_safe_target_device "$TEST_DEVICE"
      `, { env: { TEST_TYPE: type, TEST_RO: readOnly, TEST_DEVICE: device } });
      assert.equal(result.status, expected, result.stderr);
    });
  }

  for (const [name, exists, safe, size, mode, expected] of [
    ["unchanged valid target", "1", "1", "34359738368", "1", 0],
    ["removed target", "0", "1", "34359738368", "1", 1],
    ["now-unsafe target", "1", "0", "34359738368", "1", 1],
    ["unreadable capacity", "1", "1", "", "1", 1],
    ["malformed capacity", "1", "1", "unknown", "1", 1],
    ["zero capacity", "1", "1", "0", "1", 1],
    ["overflowing capacity", "1", "1", "99999999999999999999999999999999", "1", 1],
    ["smaller replacement target", "1", "1", "8589934592", "1", 1],
  ]) {
    it(`rechecks ${name} at the write boundary`, () => {
      const result = run(`
        function [ {
          if builtin [ "$#" -eq 3 ] && builtin [ "$1" == -b ];then
            builtin [ "$TEST_EXISTS" == 1 ]
          else
            builtin [ "$@"
          fi
        }
        DEVICE=/dev/mock-only CAN_INSTALL_ON_SAME_DRIVE="$TEST_MODE"
        is_safe_target_device() { [ "$TEST_SAFE" == 1 ]; }
        get_size_raw() { printf '%s\\n' "$TEST_SIZE"; }
        validate_target_before_write
      `, { env: { TEST_EXISTS: exists, TEST_SAFE: safe, TEST_SIZE: size, TEST_MODE: mode } });
      assert.equal(result.status, expected, result.stderr);
      if (expected !== 0) {
        assert.match(result.stderr, /not a valid block device|Refusing to overwrite|Cannot determine|smaller than 25GB/);
      }
    });
  }

  it("runs the shared final guard after dry-run exit and before either platform writes", () => {
    const engine = readFileSync(join(root, "install-wor.sh"), "utf8");
    const guard = engine.indexOf("\nvalidate_target_before_write\n");
    assert.ok(guard > engine.indexOf('if [ "$DRY_RUN" == 1 ];then\n  status "Exiting'));
    assert.ok(guard < engine.lastIndexOf("\n  darwin_flash_device\n"));
    assert.ok(guard < engine.indexOf('sudo parted -s "$DEVICE" mklabel gpt'));
  });
});
