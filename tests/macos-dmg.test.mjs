import { after, before, describe, it } from "node:test";
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import {
  existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync,
  rmSync, writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";
import { packageMacosDmg } from "../src/package-macos-dmg.mjs";
import { formatRuntimeManifest, generateRuntimeManifest, readProjectMetadata, verifyRuntimeManifest } from "../src/lib/node-runtime.mjs";

const root = fileURLToPath(new URL("../", import.meta.url));
const macos = process.platform === "darwin";
let workspace;
let image;
let mountedApp;
let mountpoint;
let attached = false;

describe("macOS DMG packaging", { skip: !macos }, () => {
  before(() => {
    workspace = mkdtempSync(join(tmpdir(), "wor-dmg-test-"));
    const app = join(workspace, "WoR-Flasher.app");
    const runtime = join(app, "Contents/Resources/runtime");
    mkdirSync(join(app, "Contents/MacOS"), { recursive: true });
    writeFileSync(join(app, "Contents/MacOS/WoR-Flasher"), "#!/bin/bash\nexit 0\n", { mode: 0o755 });
    const version = readProjectMetadata().product.version;
    writeFileSync(join(app, "Contents/Info.plist"), readFileSync(join(root, "src/macos-app/Contents/Info.plist")));
    const manifest = generateRuntimeManifest(runtime, version);
    writeFileSync(join(app, "Contents/Resources/runtime-manifest.json"), formatRuntimeManifest(manifest));
    image = join(workspace, `WoR-Flasher-${version}-macos.dmg`);
    packageMacosDmg(app, image, version);
    mountpoint = join(workspace, "mounted");
    mkdirSync(mountpoint);
    const result = spawnSync("/usr/bin/hdiutil", [
      "attach", image, "-readonly", "-nobrowse", "-noautoopen", "-mountpoint", mountpoint, "-plist",
    ], { encoding: "utf8", timeout: 60000 });
    attached = result.status === 0 || existsSync(join(mountpoint, "WoR-Flasher.app"));
    assert.equal(result.status, 0, result.stderr);
    mountedApp = join(mountpoint, "WoR-Flasher.app");
  });
  after(() => {
    if (attached) {
      const result = spawnSync("/usr/bin/hdiutil", ["detach", mountpoint], { encoding: "utf8", timeout: 30000 });
      assert.equal(result.status, 0, `Could not detach the test DMG at ${mountpoint}: ${result.stderr}`);
    }
    if (workspace) rmSync(workspace, { recursive: true, force: true });
  });

  it("creates a compressed, verifiable image containing only the complete app and instructions", () => {
    const imageInfo = spawnSync("/usr/bin/hdiutil", ["imageinfo", image, "-plist"], { encoding: "utf8" });
    assert.equal(imageInfo.status, 0, imageInfo.stderr);
    const converted = spawnSync("/usr/bin/plutil", ["-convert", "json", "-o", "-", "-"], { input: imageInfo.stdout, encoding: "utf8" });
    assert.equal(converted.status, 0, converted.stderr);
    assert.equal(JSON.parse(converted.stdout).Format, "UDZO");
    assert.deepEqual(readdirSync(mountpoint).filter((name) => !name.startsWith(".")).sort(), ["README.txt", "WoR-Flasher.app"]);
    assert.match(readFileSync(join(mountpoint, "README.txt"), "utf8"), /Copy WoR-Flasher\.app/);
    assert.equal(verifyRuntimeManifest(mountedApp, readProjectMetadata().product.version), true);
  });

  it("preserves all canonical runtime content inside the mounted app", () => {
    const expected = generateRuntimeManifest(root, readProjectMetadata().product.version, undefined, root);
    for (const entry of expected.files) {
      const bytes = readFileSync(join(mountedApp, "Contents/Resources/runtime", entry.path));
      assert.equal(createHash("sha256").update(bytes).digest("hex"), entry.sha256, entry.path);
    }
  });

  it("does not replace an existing disk image", () => {
    const original = readFileSync(image);
    assert.throws(() => packageMacosDmg(join(workspace, "WoR-Flasher.app"), image, readProjectMetadata().product.version), /Refusing to replace/);
    assert.deepEqual(readFileSync(image), original);
  });

  it("rejects stale app metadata and damaged runtime contents before creating an image", () => {
    const app = join(workspace, "WoR-Flasher.app");
    const version = readProjectMetadata().product.version;
    const plistPath = join(app, "Contents/Info.plist");
    const plist = readFileSync(plistPath, "utf8");
    writeFileSync(plistPath, plist.replace(/(<key>CFBundleVersion<\/key>\s*<string>)[^<]*/, (_match, prefix) => `${prefix}0.0.0`));
    assert.throws(() => packageMacosDmg(app, join(workspace, "stale.dmg"), version), /does not match/);
    assert.equal(existsSync(join(workspace, "stale.dmg")), false);
    writeFileSync(plistPath, plist);
    writeFileSync(join(app, "Contents/Resources/runtime/install-wor.sh"), "tampered\n");
    assert.throws(() => packageMacosDmg(app, join(workspace, "damaged.dmg"), version), /verified current runtime/);
    assert.equal(existsSync(join(workspace, "damaged.dmg")), false);
  });
});

it("reports that native DMG creation requires macOS on other build hosts", { skip: macos }, () => {
  assert.throws(() => packageMacosDmg("/not-an-app", "/not-an-image.dmg", "2.0.0"), /requires hdiutil on macOS/);
});
