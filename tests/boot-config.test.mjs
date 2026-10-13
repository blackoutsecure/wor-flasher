import { before, describe, it } from "node:test";
import assert from "node:assert/strict";
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";
import vm from "node:vm";

const root = fileURLToPath(new URL("../", import.meta.url));
const helper = join(root, "src/lib/boot-config.py");
const contexts = new Map();
before(() => {
  for (const model of ["3", "4", "5"]) {
    const result = spawnSync("bash", ["-c", `
      source "$DIRECTORY/install-wor.sh" source >/dev/null || exit 90
      RPI_MODEL="$TEST_MODEL"
      boot_config_context
    `], { cwd: root, encoding: "utf8", timeout: 15000,
      env: { ...process.env, DIRECTORY: root, NO_UPDATE: "1", TEST_MODEL: model } });
    assert.equal(result.status, 0, result.stderr);
    contexts.set(model, JSON.parse(result.stdout));
  }
});

function processConfig(changes = {}, model = "4", script = helper, cwd = root) {
  const context = contexts.get(model);
  const result = spawnSync("python3", [script], {
    input: JSON.stringify({ ...context, config: context.recommended, ...changes }),
    cwd, encoding: "utf8", timeout: 10000,
  });
  return { ...result, data: JSON.parse(result.stdout) };
}

function success(changes = {}, model = "4") {
  const result = processConfig(changes, model);
  assert.equal(result.status, 0, result.stdout + result.stderr);
  assert.equal(result.data.ok, true);
  return result.data;
}

describe("Shared boot configuration parsing and ownership", () => {
  for (const model of ["3", "4", "5"]) {
    it(`reads the actual Pi ${model} template and preserves required UEFI entries`, () => {
      const state = success({}, model);
      assert.match(state.config, /^# >>> WoR-Flasher managed boot settings >>>/);
      assert.match(state.config, /# <<< WoR-Flasher managed boot settings <<</);
      assert.match(state.config, /# User additions/);
      for (const protectedValue of state.protected) assert.ok(state.config.includes(protectedValue));
      assert.equal(state.hdmiMode, model === "4" ? "recommended" : "official");
      assert.deepEqual(state.warnings, []);
    });
  }
  it("is idempotent instead of duplicating headers, comments or user additions on every save", () => {
    const first = success({ config: contexts.get("4").recommended + "\n# My extra setting\ncustom_firmware_key=example\n" });
    const second = success({ config: first.config });
    assert.equal(second.config, first.config);
    assert.equal(second.userText, first.userText);
    assert.match(second.warnings.join("\n"), /custom_firmware_key.*not validated/);
  });
  it("turns existing boolean values into editable controls without treating numeric enums as booleans", () => {
    const state = success({}, "3");
    assert.equal(state.fields.find((field) => field.key === "enable_uart").type, "boolean");
    assert.equal(state.fields.some((field) => field.key === "disable_commandline_tags"), false);
    assert.match(state.config, /^disable_commandline_tags=2$/m);
    const changed = success({ config: state.config, values: { enable_uart: "0", disable_overscan: "0" } }, "3");
    assert.match(changed.config, /^enable_uart=0$/m);
    assert.match(changed.config, /^disable_overscan=0$/m);
  });
  it("keeps the last duplicate scalar in a section and reports the cleanup", () => {
    const state = success({ config: contexts.get("4").official + "\nenable_uart=0\nenable_uart=1\n" });
    assert.equal((state.config.match(/^enable_uart=/gm) || []).length, 1);
    assert.match(state.config, /^enable_uart=1$/m);
    assert.equal(state.warnings.filter((item) => item.includes("Removed duplicate enable_uart")).length, 2);
  });
  it("does not guess duplicate semantics for unknown future parameters", () => {
    const state = success({ config: contexts.get("4").official + "\nfuture_option=first\nfuture_option=second\n" });
    assert.equal((state.userText.match(/^future_option=/gm) || []).length, 2);
    assert.match(state.warnings.join("\n"), /future_option.*not validated/);
  });
  it("preserves inline comments when moving guided settings or cleaning a duplicate", () => {
    const state = success({ config: contexts.get("4").official
      + "\nenable_uart=0 # serial disabled\nenable_uart=1 # final choice\n" });
    assert.match(state.userText, /# serial disabled/);
    assert.match(state.userText, /# final choice/);
    assert.match(state.config, /^enable_uart=1$/m);
  });
  it("preserves repeated overlays, dtparam, GPIO commands, comments and conditional blocks", () => {
    const additions = "# My hardware\n"
      + "dtoverlay=custom-overlay\ndtoverlay=other-overlay\ndtparam=audio=on\ndtparam=i2c_arm=on\n"
      + "gpio=2=op,dh\ngpio=3=op,dl\n[HDMI:0]\nhdmi_drive=2\n[HDMI:1]\nhdmi_drive=1\n[all]\n";
    const state = success({ config: contexts.get("4").official + "\n" + additions });
    assert.ok(state.userText.includes(additions.trimEnd()));
    assert.equal((state.config.match(/^dtoverlay=/gm) || []).length, 4);
    assert.equal((state.config.match(/^dtparam=/gm) || []).length, 2);
    assert.equal((state.config.match(/^gpio=/gm) || []).length, 2);
    assert.equal((state.config.match(/^hdmi_drive=/gm) || []).length, 2);
    assert.match(state.warnings.join("\n"), /Conditional sections.*cannot be verified/);
  });
  it("does not globally deduplicate two different filter blocks", () => {
    const state = success({ config: contexts.get("3").official + "\n[pi3]\ncore_freq=250\n[pi4]\ncore_freq=300\n" }, "3");
    assert.equal((state.config.match(/^core_freq=/gm) || []).length, 2);
  });
  it("marks controls that advanced conditional settings can override as read-only", () => {
    const state = success({ config: contexts.get("4").official + "\n[pi4]\nenable_uart=0\n[all]\n" });
    assert.equal(state.fields.find((field) => field.key === "enable_uart").disabled, true);
    const invalid = processConfig({ config: state.config, values: { enable_uart: "0" } });
    assert.equal(invalid.status, 1);
    assert.match(invalid.data.error, /advanced conditional/);
  });
  it("preserves user additions while changing HDMI presets instead of replacing the whole file", () => {
    const first = success({ config: contexts.get("4").recommended + "\n# User hardware\ndtparam=i2c_arm=on\n",
      values: { enable_uart: "0" } });
    const official = success({ config: first.config, hdmiMode: "official" });
    assert.equal(official.hdmiMode, "official");
    assert.doesNotMatch(official.config, /^hdmi_/m);
    assert.match(official.config, /^enable_uart=0$/m);
    assert.match(official.userText, /# User hardware\ndtparam=i2c_arm=on/);
    const recommended = success({ config: official.config, hdmiMode: "recommended" });
    assert.match(recommended.config, /^hdmi_cvt=1920 1200 60 6 0 0 0$/m);
    assert.match(recommended.config, /^enable_uart=0$/m);
    assert.equal(recommended.hdmiMode, "recommended");
  });
  it("restores supported defaults without removing user additions", () => {
    const initial = success({ config: contexts.get("3").official + "\ncustom_option=keep\n",
      values: { enable_uart: "0", core_freq: "250" } }, "3");
    const restored = success({ config: initial.config, reset: true }, "3");
    assert.match(restored.config, /^enable_uart=1$/m);
    assert.doesNotMatch(restored.config, /^core_freq=/m);
    assert.match(restored.userText, /^custom_option=keep$/m);
  });
  it("labels unmodeled HDMI overrides Custom rather than pretending to use display defaults", () => {
    const state = success({ config: contexts.get("4").official + "\nhdmi_ignore_hotplug=1\n" });
    assert.equal(state.hdmiMode, "custom");
    const result = processConfig({ config: state.config, hdmiMode: "official" });
    assert.equal(result.status, 1);
    assert.match(result.data.error, /advanced HDMI overrides/);
  });
  it("removes a leading UTF-8 BOM and reports it", () => {
    const state = success({ config: "\ufeff" + contexts.get("3").official }, "3");
    assert.doesNotMatch(state.config, /\ufeff/);
    assert.match(state.warnings.join("\n"), /byte-order mark/);
  });
  it("normalizes CRLF input without changing user-owned text or interpreting it as shell code", () => {
    const directory = mkdtempSync(join(tmpdir(), "wor-boot-config-injection-"));
    try {
      const marker = join(directory, "not-created");
      const result = processConfig({ config: contexts.get("3").official.replaceAll("\n", "\r\n")
        + "\r\n# keep me\r\ncustom_key=$(touch not-created)\r\n" }, "3", helper, directory);
      assert.equal(result.status, 0, result.stdout + result.stderr);
      const state = result.data;
      assert.doesNotMatch(state.config, /\r/);
      assert.ok(state.userText.includes("custom_key=$(touch not-created)"));
      assert.equal(existsSync(marker), false);
    } finally { rmSync(directory, { recursive: true, force: true }); }
  });
  it("lets advanced users add known supported keys and reads their values back into controls", () => {
    const initial = success({}, "3");
    const state = success({ config: initial.config, userText: initial.userText + "\nenable_uart=0\n" }, "3");
    assert.equal(state.fields.find((field) => field.key === "enable_uart").value, "0");
    assert.equal((state.config.match(/^enable_uart=/gm) || []).length, 1);
  });
  it("reads newly added HDMI controls back into the simple screen", () => {
    const initial = success({}, "3");
    const state = success({ config: initial.config, userText: "hdmi_group=1\nhdmi_mode=16\nhdmi_force_hotplug=1\n" }, "3");
    assert.equal(state.fields.find((field) => field.key === "hdmi_group").value, "1");
    assert.equal(state.fields.find((field) => field.key === "hdmi_mode").value, "16");
    assert.equal(state.fields.find((field) => field.key === "hdmi_force_hotplug").type, "boolean");
    assert.equal(state.hdmiMode, "custom");
  });
  it("preserves repeatable uses of an otherwise required overlay", () => {
    const state = success({ config: contexts.get("4").official + "\ndtoverlay=miniuart-bt\n" });
    assert.equal((state.config.match(/^dtoverlay=miniuart-bt$/gm) || []).length, 2);
  });
  it("rebases only managed required entries when the selected model changes", () => {
    const original = success({ config: contexts.get("4").official + "\n# Personal hardware\ncustom_option=keep\n",
      values: { enable_uart: "0" } });
    const changed = success({ config: original.config, previousDefaults: contexts.get("4") }, "3");
    assert.match(changed.config, /^device_tree_address=0x1f0000$/m);
    assert.match(changed.config, /^disable_commandline_tags=2$/m);
    assert.doesNotMatch(changed.config, /upstream-pi4|enable_gic|0x3e0000/);
    assert.match(changed.config, /^enable_uart=0$/m);
    assert.match(changed.userText, /# Personal hardware\ncustom_option=keep/);
  });
});

describe("Boot configuration validation", () => {
  const cases = [
    ["invalid boolean", "enable_uart=2", /enable_uart must be 0/],
    ["word instead of boolean", "enable_uart=true", /must be an integer/],
    ["empty assignment", "enable_uart=", /has no value/],
    ["invalid HDMI group", "hdmi_group=3", /hdmi_group must be/],
    ["invalid HDMI output", "hdmi_drive=9", /hdmi_drive must be/],
    ["invalid HDMI mode", "hdmi_mode=108", /documented 0-107/],
    ["invalid DMT mode", "hdmi_group=2\nhdmi_mode=88", /DMT hdmi_mode/],
    ["invalid custom timing", "hdmi_cvt=1920 nope 60", /must be an integer/],
    ["invalid timing boolean", "hdmi_cvt=1920 1200 60 6 0 0 2", /must be 0 or 1/],
    ["malformed section", "[pi4", /malformed conditional/],
    ["unknown syntax", "not a setting", /expected key=value/],
    ["unsafe include", "include other.txt", /cannot be verified/],
    ["firmware-truncated line", "unknown=" + "a".repeat(99), /98-character limit/],
    ["control character", "unknown=a\0b", /invalid control character/],
  ];
  for (const [label, text, error] of cases) {
    it(`rejects ${label} before a config can be saved`, () => {
      const result = processConfig({ config: contexts.get("4").official + "\n" + text });
      assert.equal(result.status, 1, result.stdout);
      assert.equal(result.data.ok, false);
      assert.match(result.data.error, error);
    });
  }
  for (const key of ["armstub", "arm_64bit", "enable_gic", "device_tree_address", "disable_commandline_tags"]) {
    it(`does not let an advanced edit override required ${key}`, () => {
      const result = processConfig({ config: contexts.get("4").official + `\n[pi4]\n${key}=0\n` });
      assert.equal(result.status, 1);
      assert.match(result.data.error, /required.*must remain/);
    });
  }
  it("refuses missing boot entries, missing required overlays and malformed managed markers", () => {
    for (const config of [
      contexts.get("4").official.replace("armstub=RPI_EFI.fd", ""),
      contexts.get("4").official.replace("dtoverlay=upstream-pi4", ""),
      "# >>> WoR-Flasher managed boot settings >>>\n" + contexts.get("4").official,
    ]) {
      const result = processConfig({ config });
      assert.equal(result.status, 1);
      assert.match(result.data.error, /missing|markers/);
    }
  });
  it("cannot redefine protected boot entries in the user section", () => {
    const state = success();
    const result = processConfig({ config: state.config, userText: "armstub=other.fd\n" });
    assert.equal(result.status, 1);
    assert.match(result.data.error, /protected section/);
  });
  it("preserves unknown firmware options with an explicit validation limitation", () => {
    const state = success({ config: contexts.get("4").official + "\nfuture_firmware_option=example\n" });
    assert.match(state.userText, /future_firmware_option=example/);
    assert.match(state.warnings.join("\n"), /firmware support.*not validated/);
  });
});

describe("IoT structured boot editor preserves its display-only boundary", () => {
  const context = { profile: "iot-core", config: "hdmi_force_hotplug=1\nhdmi_group=1\nhdmi_mode=4\n" };
  it("provides controls and uses the existing FFU validator for values", () => {
    const state = success(context);
    assert.equal(state.fields.length, 4);
    const edited = success({ ...context, values: { hdmi_drive: "2", hdmi_mode: "16" } });
    assert.match(edited.config, /^hdmi_mode=16$/m);
    assert.match(edited.config, /^hdmi_drive=2$/m);
  });
  it("cleans duplicate HDMI scalars before invoking the strict existing validator", () => {
    const state = success({ ...context, config: context.config + "hdmi_mode=16\n" });
    assert.equal((state.config.match(/^hdmi_mode=/gm) || []).length, 1);
    assert.match(state.warnings.join("\n"), /Removed duplicate hdmi_mode/);
  });
  it("keeps the legacy CEA/DMT limits and never enables boot/memory editing", () => {
    for (const extra of ["arm_64bit=1", "gpu_mem=64", "hdmi_cvt=1920 1080 60", "hdmi_mode=65"]) {
      const result = processConfig({ ...context, config: context.config + extra + "\n" });
      assert.equal(result.status, 1, result.stdout);
    }
  });
  it("restores the established 720p defaults without touching an image", () => {
    const state = success({ ...context, config: "hdmi_group=2\nhdmi_mode=16\n", reset: true });
    assert.equal(state.hdmiMode, "720p60");
    assert.match(state.config, /^hdmi_group=1$/m);
    assert.match(state.config, /^hdmi_mode=4$/m);
  });
});

describe("Boot editor regression coverage detects broken protections", () => {
  it("detects mutations to boolean validation and required-boot validation", () => {
    const original = readFileSync(helper, "utf8");
    const directory = mkdtempSync(join(tmpdir(), "wor-boot-config-mutations-"));
    try {
      for (const [old, replacement, config] of [
        ["if key in KNOWN_BOOLEANS and integer(value, key) not in (0, 1):", "if False:", contexts.get("4").official + "\nenable_uart=2\n"],
        ["if any(not same_value(key, line.value, expected) for line in matches):", "if False:", contexts.get("4").official + "\n[pi4]\narmstub=other.fd\n"],
      ]) {
        assert.ok(original.includes(old));
        const mutated = join(directory, "boot-config.py");
        writeFileSync(mutated, original.replace(old, replacement));
        const result = processConfig({ config }, "4", mutated);
        assert.equal(result.status, 0, "The broken implementation should escape the validation assertion.");
      }
    } finally { rmSync(directory, { recursive: true, force: true }); }
  });
});

describe("Native boot editor uses typed controls transactionally", () => {
  it("turns controls into values and leaves advanced-controlled fields unchanged", () => {
    const source = readFileSync(join(root, "src/lib/boot-config-gui.js"), "utf8");
    const context = vm.createContext({ ObjC: { unwrap: (value) => value } });
    vm.runInContext(source, context);
    const values = context.worBootConfigValues([
      { field: { key: "enable_uart", type: "boolean" }, control: { state: 0 } },
      { field: { key: "arm_boost", type: "boolean" }, control: { state: 1 } },
      { field: { key: "hdmi_drive", type: "choice", choices: [{ value: "1" }, { value: "2" }] }, control: { indexOfSelectedItem: 1 } },
      { field: { key: "hdmi_mode", type: "number" }, control: { stringValue: "16" } },
      { field: { key: "disabled_key", disabled: true }, control: {} },
    ]);
    assert.equal(JSON.stringify(values), JSON.stringify({ enable_uart: "0", arm_boost: "1", hdmi_drive: "2", hdmi_mode: "16" }));
  });
});

describe("GUI and CLI use the same engine boot settings", () => {
  function runEngine(body, settings = "", environment = {}) {
    return spawnSync("bash", ["-c", `
      source "$DIRECTORY/install-wor.sh" source >/dev/null || exit 90
      RPI_MODEL=3 CONFIG_TXT='' CONFIG_TXT_MODEL='' APPLY_CUSTOM_CONFIG_TXT=1
      set_default_config_txt || exit 91
      ${body}
    `], { cwd: root, encoding: "utf8", timeout: 15000,
      env: { ...process.env, DIRECTORY: root, NO_UPDATE: "1", BOOT_CONFIG_SETTINGS: settings, ...environment } });
  }
  it("accepts actual JSON booleans, removes an inherited optional clock and exports the settings", () => {
    const result = runEngine(`
      validate_boot_config || exit 92
      export_installer_settings
      env | grep -q '^BOOT_CONFIG_SETTINGS=' || exit 93
      printf '%s\\n' "$CONFIG_TXT"
    `, '{"enable_uart":false,"disable_overscan":true,"core_freq":null}');
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.stdout, /^enable_uart=0$/m);
    assert.match(result.stdout, /^disable_overscan=1$/m);
    assert.doesNotMatch(result.stdout, /^core_freq=/m);
  });
  it("does not reapply old JSON preferences over subsequent GUI choices", () => {
    const result = runEngine(`
      apply_boot_config_preferences || exit 92
      [ -z "$BOOT_CONFIG_SETTINGS" ] || exit 93
      CONFIG_TXT="\${CONFIG_TXT/enable_uart=0/enable_uart=1}"
      validate_boot_config || exit 94
      printf '%s\\n' "$CONFIG_TXT"
    `, '{"enable_uart":false}');
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.stdout, /^enable_uart=1$/m);
  });
  it("loads structured preferences from the existing JSON configuration cascade", () => {
    const directory = mkdtempSync(join(tmpdir(), "wor-boot-json-"));
    try {
      const file = join(directory, "config.json");
      writeFileSync(file, JSON.stringify({ customization: { bootSettings: { enable_uart: false, disable_overscan: true } } }));
      const result = runEngine(`
        load_config_json "$TEST_JSON_FILE"
        [ -n "$BOOT_CONFIG_SETTINGS" ] || exit 92
        apply_boot_config_preferences || exit 93
        printf '%s\\n' "$CONFIG_TXT"
      `, "", { TEST_JSON_FILE: file });
      assert.equal(result.status, 0, result.stderr);
      assert.match(result.stdout, /^enable_uart=0$/m);
      assert.match(result.stdout, /^disable_overscan=1$/m);
    } finally { rmSync(directory, { recursive: true, force: true }); }
  });
  it("rejects protected structured settings without changing the caller configuration", () => {
    const result = runEngine(`
      original="$CONFIG_TXT"
      if validate_boot_config;then exit 92;fi
      [ "$CONFIG_TXT" == "$original" ] || exit 93
    `, '{"armstub":"other.fd"}');
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.stderr, /Invalid boot configuration.*unsupported control/);
  });
  it("rebases a saved guided config through real model navigation without losing user text", () => {
    const result = runEngine(`
      RPI_MODEL=4 CONFIG_TXT='' CONFIG_TXT_MODEL=''
      set_default_config_txt || exit 92
      CONFIG_TXT="$CONFIG_TXT
custom_option=keep"
      validate_boot_config || exit 93
      RPI_MODEL=3
      set_default_config_txt || exit 94
      printf '%s\\n' "$CONFIG_TXT"
    `);
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.stdout, /^device_tree_address=0x1f0000$/m);
    assert.match(result.stdout, /^custom_option=keep$/m);
    assert.doesNotMatch(result.stdout, /upstream-pi4/);
  });
});

describe("Linux boot editor transactions", () => {
  const source = readFileSync(join(root, "install-wor-gui.sh"), "utf8");
  const definition = source.match(/^linux_boot_config_editor\(\) \{[\s\S]*?^\}/m);
  assert.ok(definition);
  function runEditor(scenario, profile = "desktop") {
    const directory = mkdtempSync(join(tmpdir(), "wor guided editor "));
    try {
      const file = join(directory, "pending config.txt");
      const original = profile === "desktop" ? contexts.get("3").official : "hdmi_group=1\nhdmi_mode=4\n";
      writeFileSync(file, original);
      const result = spawnSync("bash", ["-c", `
        source "$DIRECTORY/install-wor.sh" source >/dev/null || exit 90
        ${definition[0]}
        sudo() { printf 'UNEXPECTED_PRIVILEGE\\n' >&2; return 99; }
        yad() {
          printf '%s\\0' "$@" >> "$TEST_ARGS"
          local argument form=0 raw=0 index label value call
          for argument;do
            [ "$argument" != --form ] || form=1
            [ "$argument" != --text-info ] || raw=1
          done
          if [ "$raw" == 1 ];then
            printf '# Personal hardware\\ncustom_option=keep\\n'
            return 0
          fi
          [ "$form" == 1 ] || return 0
          call=0
          [ ! -f "$TEST_COUNT" ] || call="$(cat "$TEST_COUNT")"
          call=$((call+1))
          printf '%s' "$call" > "$TEST_COUNT"
          for ((index=0;index<\${#fields[@]};index+=2));do
            label="\${fields[$index]}" value="\${fields[$((index+1))]}"
            [ "$label" != '--field=Enable serial console:CHK' ] || value=FALSE
            printf '%s\\n' "\${value%%!*}"
          done
          case "$TEST_SCENARIO:$call" in
            cancel:* | advanced-cancel:2) return 1 ;;
            advanced:1 | advanced-cancel:1) return 2 ;;
            reset:1) return 3 ;;
          esac
          return 0
        }
        WOR_YAD_SCREEN_WIDTH=1024 WOR_YAD_SCREEN_HEIGHT=768
        linux_boot_config_editor "$TEST_FILE" 3 "$TEST_PROFILE"
      `], { cwd: directory, encoding: "utf8", timeout: 20000,
        env: { ...process.env, DIRECTORY: root, NO_UPDATE: "1", BOOT_CONFIG_SETTINGS: "",
          TEST_FILE: file, TEST_ARGS: join(directory, "args"), TEST_COUNT: join(directory, "count"),
          TEST_SCENARIO: scenario, TEST_PROFILE: profile } });
      return { ...result, original, config: readFileSync(file, "utf8"),
        state: existsSync(file + ".editor.json") ? JSON.parse(readFileSync(file + ".editor.json", "utf8")) : null,
        args: existsSync(join(directory, "args")) ? readFileSync(join(directory, "args"), "utf8").split("\0") : [] };
    } finally { rmSync(directory, { recursive: true, force: true }); }
  }
  it("saves real checkbox values through the shared validator without any privilege", () => {
    const result = runEditor("save");
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.config, /^enable_uart=0$/m);
    assert.ok(result.args.includes("--field=Enable serial console:CHK"));
    assert.ok(result.args.includes("--button=Advanced Text Edit (Not Recommended):2"));
    assert.equal(result.state.ok, true);
    assert.doesNotMatch(result.stderr, /UNEXPECTED_PRIVILEGE/);
  });
  it("leaves the pending file unchanged when Cancel is clicked", () => {
    const result = runEditor("cancel");
    assert.equal(result.status, 1);
    assert.equal(result.config, result.original);
    assert.equal(result.state, null);
  });
  it("saves advanced additions below the managed section", () => {
    const result = runEditor("advanced");
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.config, /# <<< WoR-Flasher managed boot settings <<<[\s\S]*# Personal hardware\ncustom_option=keep/);
    assert.match(result.state.warnings.join("\n"), /custom_option.*not validated/);
  });
  it("discards both guided and advanced changes if the final editor screen is canceled", () => {
    const result = runEditor("advanced-cancel");
    assert.equal(result.status, 1);
    assert.equal(result.config, result.original);
    assert.equal(result.state, null);
  });
  it("uses the same screen for IoT without exposing desktop or bootloader controls", () => {
    const result = runEditor("save", "iot-core");
    assert.equal(result.status, 0, result.stderr);
    assert.ok(result.args.includes("--field=HDMI mode number:NUM"));
    assert.equal(result.args.some((item) => /serial console|armstub|memory|core clock/.test(item)), false);
    assert.equal(result.state.hdmiMode, "custom");
  });
});
