import { describe, it } from "node:test";
import assert from "node:assert/strict";
import {
  copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync,
  renameSync, rmSync, symlinkSync, writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";
import {
  formatRuntimeManifest, generateRuntimeManifest, readProjectMetadata, verifyRuntimeManifest,
} from "../src/lib/node-runtime.mjs";

const root = fileURLToPath(new URL("../", import.meta.url));

function temporary(callback) {
  const directory = mkdtempSync(join(tmpdir(), "wor-publication-review-"));
  try {
    callback(directory);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
}

function copySource(relative, target) {
  mkdirSync(dirname(join(target, relative)), { recursive: true });
  copyFileSync(join(root, relative), join(target, relative));
}

function packageFixture(directory) {
  for (const path of ["src/package-macos-app.mjs", "src/lib/node-runtime.mjs"]) copySource(path, directory);
  mkdirSync(join(directory, "src/config"), { recursive: true });
  writeFileSync(join(directory, "src/config/metadata.json"), JSON.stringify({
    ...readProjectMetadata(), runtimePaths: ["payload.txt"],
  }));
  writeFileSync(join(directory, "payload.txt"), "current source");
  const app = join(directory, "staged.app");
  const runtime = join(app, "Contents/Resources/runtime");
  mkdirSync(runtime, { recursive: true });
  writeFileSync(join(app, "Contents/Info.plist"), "<plist/>");
  const manifest = generateRuntimeManifest(runtime, readProjectMetadata().product.version, ["payload.txt"], directory);
  const manifestPath = join(app, "Contents/Resources/runtime-manifest.json");
  writeFileSync(manifestPath, formatRuntimeManifest(manifest));
  return { app, runtime, manifest, manifestPath };
}

describe("Publication runtime integrity", () => {
  it("refreshes changed files and removes obsolete staged files on every package write", () => temporary((directory) => {
    const source = join(directory, "source");
    const stage = join(directory, "stage");
    mkdirSync(join(source, "tree"), { recursive: true });
    writeFileSync(join(source, "tree/current.txt"), "before");
    writeFileSync(join(source, "tree/obsolete.txt"), "obsolete");
    generateRuntimeManifest(stage, "2.0.0", ["tree"], source);
    writeFileSync(join(source, "tree/current.txt"), "after");
    rmSync(join(source, "tree/obsolete.txt"));
    const result = generateRuntimeManifest(stage, "2.0.0", ["tree"], source);
    assert.equal(readFileSync(join(stage, "tree/current.txt"), "utf8"), "after");
    assert.equal(readFileSync(join(source, "tree/current.txt"), "utf8"), "after");
    assert.equal(existsSync(join(stage, "tree/obsolete.txt")), false);
    assert.deepEqual(result.files.map((entry) => entry.path), ["tree/current.txt"]);
  }));

  for (const linkParent of [false, true]) {
    it(`rejects a matching ${linkParent ? "parent-directory" : "file"} symlink during verification`, () => temporary((directory) => {
      const runtime = join(directory, "Contents/Resources/runtime");
      mkdirSync(join(runtime, "tree"), { recursive: true });
      writeFileSync(join(runtime, "tree/payload"), "verified bytes");
      const manifest = generateRuntimeManifest(runtime, "2.0.0", ["tree"], runtime);
      writeFileSync(join(directory, "Contents/Resources/runtime-manifest.json"), formatRuntimeManifest(manifest));
      assert.equal(verifyRuntimeManifest(directory), true);
      const original = join(runtime, linkParent ? "tree" : "tree/payload");
      const outside = join(directory, "outside");
      renameSync(original, outside);
      symlinkSync(outside, original);
      assert.equal(verifyRuntimeManifest(directory), false);
    }));
  }

  it("refuses to refresh a staging path through a symlink without changing the linked data", () => temporary((directory) => {
    const source = join(directory, "source");
    const stage = join(directory, "stage");
    const outside = join(directory, "outside");
    mkdirSync(join(source, "src/lib"), { recursive: true });
    mkdirSync(join(outside, "lib"), { recursive: true });
    mkdirSync(stage);
    writeFileSync(join(source, "src/lib/file"), "new");
    writeFileSync(join(outside, "lib/file"), "do not overwrite");
    symlinkSync(outside, join(stage, "src"));
    assert.throws(() => generateRuntimeManifest(stage, "2.0.0", ["src/lib"], source), /Symbolic link/);
    assert.equal(readFileSync(join(outside, "lib/file"), "utf8"), "do not overwrite");
  }));

  it("rejects a valid old manifest version instead of reporting the current release", () => temporary((directory) => {
    const fixture = packageFixture(directory);
    fixture.manifest.version = "1.0.0";
    writeFileSync(fixture.manifestPath, formatRuntimeManifest(fixture.manifest));
    const result = spawnSync(process.execPath, [join(directory, "src/package-macos-app.mjs"), "--check"], {
      encoding: "utf8", env: { ...process.env, WOR_MACOS_APP_ROOT: fixture.app },
    });
    assert.notEqual(result.status, 0);
    assert.doesNotMatch(result.stdout, /runtime is current/);
  }));

  it("detects stale same-version source and repairs it with the actual package CLI", () => temporary((directory) => {
    const fixture = packageFixture(directory);
    writeFileSync(join(directory, "payload.txt"), "new source at the same version");
    const run = (mode) => spawnSync(process.execPath, [join(directory, "src/package-macos-app.mjs"), mode], {
      encoding: "utf8", env: { ...process.env, WOR_MACOS_APP_ROOT: fixture.app },
    });
    assert.notEqual(run("--check").status, 0);
    const write = run("--write");
    assert.equal(write.status, 0, write.stderr);
    assert.equal(readFileSync(join(fixture.runtime, "payload.txt"), "utf8"), "new source at the same version");
    const check = run("--check");
    assert.equal(check.status, 0, check.stderr);
  }));

  it("propagates a failed macOS build from version:set", () => temporary((directory) => {
    for (const path of [
      "src/set-version.mjs", "src/sync-package-metadata.mjs", "src/lib/node-runtime.mjs",
      "src/config/metadata.json", "src/macos-app/Contents/Info.plist", "README.md", "install-wor.sh", "package.json",
    ]) copySource(path, directory);
    writeFileSync(join(directory, "src/build-release.mjs"), 'console.error("mock build failure"); process.exit(42);\n');
    const result = spawnSync(process.execPath, [join(directory, "src/set-version.mjs"), "2.0.1"], { encoding: "utf8" });
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /mock build failure/);
    assert.doesNotMatch(result.stdout, /Updated WoR-Flasher version surfaces/);
  }));
});

describe("Publication defaults and optional configuration", () => {
  it("uses the publishing repository for bootstrap and updates, while retaining upstream attribution", () => {
    const metadata = readProjectMetadata();
    assert.equal(metadata.systemDefaults.repoSlug, "blackoutsecure/wor-flasher");
    const hook = readFileSync(join(root, "install-wor-hook.sh"), "utf8");
    assert.ok(hook.includes(`WOR_HOOK_REPOSITORY:=https://github.com/${metadata.systemDefaults.repoSlug}.git`));
    assert.match(readFileSync(join(root, "NOTICE"), "utf8"), /https:\/\/github.com\/Botspot\/wor-flasher/);
  });

  it("enforces macOS 13 and documents the current security-supported line", () => {
    const metadata = readProjectMetadata();
    assert.equal(metadata.product.minimumMacosVersion, "13.0");
    assert.match(readFileSync(join(root, "src/macos-app/Contents/Info.plist"), "utf8"), /<key>LSMinimumSystemVersion<\/key>\s*<string>13\.0<\/string>/);
    const line = metadata.product.version.split(".").slice(0, 2).join("\\.");
    assert.match(readFileSync(join(root, "SECURITY.md"), "utf8"), new RegExp(`\\| ${line}\\.x\\s*\\| Yes\\s*\\|`));
  });

  it("stages and removes the Pi 4 UEFI Shell handoff independently of answer-file options", () => temporary((directory) => {
    mkdirSync(join(directory, "peinstaller/winpe/2"), { recursive: true });
    const result = spawnSync("bash", ["-c", `
      source "$DIRECTORY/install-wor.sh" source >/dev/null
      RPI_MODEL=4 PI4_UEFI_SHELL_UNLOCK=1 PI4_AUTO_DISABLE_3GB=0 OOBE_NETWORK_BYPASS=0
      WINDOWS_ACCOUNT_SETUP=0 WINDOWS_LOCALE_SETUP=0
      prepare_uefi_shell() { mkdir -p uefi-shell; printf "mock shell" > uefi-shell/Shell.efi; }
      configure_pe_prefinalize || exit 1
      [ -s peinstaller/winpe/2/scripts/prefinalize.cmd ] &&
        [ -s peinstaller/winpe/2/scripts/Shell.efi ] &&
        [ ! -e peinstaller/winpe/2/scripts/unattend.xml ] || exit 2
      PI4_UEFI_SHELL_UNLOCK=0
      configure_pe_prefinalize || exit 3
      [ ! -e peinstaller/winpe/2/scripts ] || exit 4
    `], { cwd: directory, encoding: "utf8", env: { ...process.env, DIRECTORY: root, NO_UPDATE: "1" } });
    assert.equal(result.status, 0, result.stdout + result.stderr);
    const batch = readFileSync(join(root, "config-templates/prefinalize.cmd"), "utf8");
    const missingAnswer = batch.match(/if not exist "%answerSource%" \([\s\S]*?\n\)/);
    assert.ok(missingAnswer);
    assert.match(missingAnswer[0], /goto :stage_shell/);
    assert.doesNotMatch(missingAnswer[0], /goto :end/);
  }));
});
