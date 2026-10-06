import { it } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { join } from "node:path";
import { spawnSync } from "node:child_process";

const root = fileURLToPath(new URL("../", import.meta.url));
const gui = readFileSync(join(root, "install-wor-gui.sh"), "utf8");
const engine = readFileSync(join(root, "install-wor.sh"), "utf8");
const native = readFileSync(join(root, "src/lib/iot-account.ps1"), "utf8");

it("validates pinned IoT account administration with offline Python contracts", () => {
  const result = spawnSync("python3", ["-B", join(root, "tests/test-iot-account.py")],
    { encoding: "utf8", cwd: root, timeout: 60000 });
  assert.equal(result.status, 0, result.stderr);
});

it("keeps IoT desired passwords out of installer settings and uses private native state", () => {
  const settings = engine.slice(engine.indexOf("WOR_INSTALLER_SETTINGS=("), engine.indexOf("\nexport_installer_settings()"));
  assert.match(settings, /IOT_CORE_ACCOUNT_SETUP IOT_CORE_ACCOUNT_USERNAME/);
  assert.doesNotMatch(settings, /IOT_CORE_ACCOUNT_PASSWORD/);
  const options = gui.slice(gui.indexOf("macos_iot_options() {"), gui.indexOf("\nlinux_iot_options() {"));
  assert.match(options, /gui_iot_private_json "\$state" state_file/);
  assert.match(options, /wor_osascript -l JavaScript - "\$state_file"/);
  assert.doesNotMatch(options, /wor_osascript -l JavaScript - "\$state"/);
  assert.match(options, /NSSecureTextField/);
});

it("runs optional setup only after successful non-dry-run IoT media completion on both hosts", () => {
  assert.match(gui, /if \[ "\$installer_status" == 0 \] && is_iot_core && iot_core_personalization_requested && \[ "\$DRY_RUN" != 1 \];then/);
  assert.match(gui, /if is_iot_core && iot_core_personalization_requested && \[ "\$DRY_RUN" != 1 \];then/);
  assert.match(gui, /if \[ "\$completion_connect" == 1 \] && \[ "\$GUI_RESULT_ACTION" == connect \];then/);
  assert.match(gui, /if \[ "\$linux_completion_connect" == 1 \] && \[ "\$GUI_RESULT_ACTION" == connect \];then/);
  assert.doesNotMatch(gui, /gui_iot_account_connection\(\)|Finish IoT personalization/);
  assert.match(native, /DefinePInvokeMethod\('NetUserSetInfo'/);
  assert.match(native, /level = 1003/);
  assert.match(native, /level = 0/);
  assert.match(native, /Not the local built-in administrator/);
  assert.doesNotMatch(native, /net\.exe|net user|DefaultAccount|csc\.exe/);
});
