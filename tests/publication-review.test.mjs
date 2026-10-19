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

const versionFiles = [
  "src/config/metadata.json", "src/macos-app/Contents/Info.plist",
  "README.md", "install-wor.sh", "package.json",
];

function versionFixture(directory) {
  for (const path of [
    "src/set-version.mjs", "src/sync-package-metadata.mjs", "src/lib/node-runtime.mjs", ...versionFiles,
  ]) copySource(path, directory);
  writeFileSync(join(directory, "src/build-release.mjs"), 'console.log("mock build complete");\n');
}

describe("Shell syntax gate", () => {
  it("checks each script rather than treating later filenames as Bash arguments", () => temporary((directory) => {
    const checker = "tests/check-shell-syntax.sh";
    copySource(checker, directory);
    const scripts = [
      "src/standalone-launcher.sh", "src/lib/fixture.sh", "install-wor.sh",
      "install-wor-gui.sh", "install-wor-hook.sh", "src/macos-app/Contents/MacOS/WoR-Flasher",
      "tests/last-script.sh",
    ];
    for (const path of scripts) {
      mkdirSync(dirname(join(directory, path)), { recursive: true });
      writeFileSync(join(directory, path), "#!/bin/bash\n:\n");
    }
    const check = () => spawnSync("/bin/bash", [join(directory, checker)], { encoding: "utf8" });
    const valid = check();
    assert.equal(valid.status, 0, valid.stderr);
    writeFileSync(join(directory, "tests/last-script.sh"), "#!/bin/bash\nif true;then\n");
    const broken = check();
    assert.notEqual(broken.status, 0);
    assert.match(broken.stderr, /last-script\.sh.*|unexpected end of file/s);
    const oldGate = spawnSync("/bin/bash", ["-n", ...scripts.map((path) => join(directory, path))]);
    assert.equal(oldGate.status, 0, "The former multi-file invocation must demonstrate the regression.");
  }));

  it("uses the same complete syntax gate in npm and both CI platforms", () => {
    const pkg = JSON.parse(readFileSync(join(root, "package.json"), "utf8"));
    assert.equal(pkg.scripts["check:shell"], "bash tests/check-shell-syntax.sh");
    assert.match(pkg.scripts.check, /^npm run check:shell && /);
    for (const path of [".github/workflows/shellcheck.yml", ".github/workflows/release.yml"]) {
      const workflow = readFileSync(join(root, path), "utf8");
      assert.equal((workflow.match(/bash tests\/check-shell-syntax\.sh/g) || []).length, 2);
      assert.doesNotMatch(workflow, /bash -n src\/lib\/\*\.sh/);
    }
  });
});

describe("Maintainer command validation", () => {
  for (const [path, args, output] of [
    ["src/updater.mjs", ["--help"], /WoR-Flasher Updater Tooling/],
    ["src/sync-package-metadata.mjs", ["--help"], /Sync package\.json metadata/],
    ["src/check-pe-installer.mjs", ["--help"], /WoR-Flasher PE installer metadata tool/],
    ["src/build-pi3-boot-refresh.mjs", ["--check"], /native finalizer.*verified/],
  ]) {
    it(`executes ${path} through a symlink rather than silently skipping main`, () => temporary((directory) => {
      const entry = join(directory, "linked-entry.mjs");
      symlinkSync(join(root, path), entry);
      const result = spawnSync(process.execPath, [entry, ...args], { encoding: "utf8" });
      assert.equal(result.status, 0, result.stderr);
      assert.match(result.stdout, output);
    }));
  }

  for (const args of [
    ["--unknown"], ["--verify-manifest"], ["--compare-versions", "1.0.0"],
    ["--compare-versions", "1.0.0", "2.0.0", "extra"],
    ["--check-release", "--verify-manifest", "unused", "--repo-slug=../unsafe"],
    ["--check-release", "--unknown", "--repo-slug=../unsafe"],
  ]) {
    it(`rejects invalid updater arguments ${args.join(" ")}`, () => {
      const result = spawnSync(process.execPath, [join(root, "src/updater.mjs"), ...args], { encoding: "utf8" });
      assert.notEqual(result.status, 0, result.stdout);
      assert.match(result.stderr, /option|argument|command|requires/i);
    });
  }

  it("builds a manifest from the selected repository, including paths containing equals signs", () => temporary((directory) => {
    const source = join(directory, "repo=with spaces");
    mkdirSync(join(source, "src/config"), { recursive: true });
    writeFileSync(join(source, "src/config/metadata.json"), JSON.stringify({
      product: { version: "8.0.0" }, runtimePaths: ["only.txt"],
    }));
    writeFileSync(join(source, "only.txt"), "selected runtime");
    const stage = join(directory, "stage");
    const output = join(directory, "manifest.json");
    const result = spawnSync(process.execPath, [
      join(root, "src/updater.mjs"), "--build-manifest", stage, output, `--repo-dir=${source}`,
    ], { encoding: "utf8" });
    assert.equal(result.status, 0, result.stderr);
    const manifest = JSON.parse(readFileSync(output, "utf8"));
    assert.equal(manifest.version, "8.0.0");
    assert.deepEqual(manifest.files.map((entry) => entry.path), ["only.txt"]);
  }));

  for (const args of [["--check", "--write"], ["--write", "--unknown"]]) {
    it(`rejects ambiguous metadata commands without writing: ${args.join(" ")}`, () => temporary((directory) => {
      versionFixture(directory);
      const packageFile = join(directory, "package.json");
      const before = readFileSync(packageFile);
      const result = spawnSync(process.execPath, [join(directory, "src/sync-package-metadata.mjs"), ...args], { encoding: "utf8" });
      assert.notEqual(result.status, 0);
      assert.match(result.stderr, /option|mutually exclusive/i);
      assert.deepEqual(readFileSync(packageFile), before);
    }));
  }

  it("does not ignore extra macOS package arguments", () => temporary((directory) => {
    const fixture = packageFixture(directory);
    const result = spawnSync(process.execPath, [
      join(directory, "src/package-macos-app.mjs"), "--check", "--unexpected",
    ], { encoding: "utf8", env: { ...process.env, WOR_MACOS_APP_ROOT: fixture.app } });
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /option|Usage/i);
  }));

  it("rejects an unknown PE-check option before a local download", () => temporary((directory) => {
    const payload = join(directory, "pe.zip");
    writeFileSync(payload, "local PE fixture");
    const metadata = join(directory, "metadata.json");
    writeFileSync(metadata, JSON.stringify({ systemDefaults: {
      peInstallerUrl: new URL(`file://${payload}`).href,
      peInstallerSha256: "0".repeat(64),
    } }));
    const result = spawnSync(process.execPath, [
      join(root, "src/check-pe-installer.mjs"), "--check", "--unexpected", "--metadata", metadata,
    ], { encoding: "utf8" });
    assert.equal(result.status, 2);
    assert.match(result.stderr, /option/i);
    assert.doesNotMatch(result.stderr, /SHA-256 mismatch/);
  }));
});

describe("Version update preflight", () => {
  for (const [path, before, after] of [
    ["src/macos-app/Contents/Info.plist", "CFBundleVersion", "MissingBundleVersion"],
    ["README.md", `version-${readProjectMetadata().product.version}-`, "missing-version-badge-"],
    ["README.md", "## Versions", "## Missing versions"],
    ["install-wor.sh", "#Version history", "#Missing history"],
    ["package.json", '"funding": [', '"funding": invalid ['],
  ]) {
    it(`leaves every version surface untouched when ${path} cannot be updated (${before})`, () => temporary((directory) => {
      versionFixture(directory);
      const target = join(directory, path);
      const content = readFileSync(target, "utf8");
      assert.ok(content.includes(before));
      writeFileSync(target, content.replace(before, after));
      const originals = new Map(versionFiles.map((name) => [name, readFileSync(join(directory, name))]));
      const result = spawnSync(process.execPath, [join(directory, "src/set-version.mjs"), "9.9.9"], { encoding: "utf8" });
      assert.notEqual(result.status, 0);
      assert.doesNotMatch(result.stdout, /mock build complete|Updated WoR-Flasher version surfaces/);
      for (const [name, bytes] of originals) {
        assert.deepEqual(readFileSync(join(directory, name)), bytes, `${name} was partially updated`);
      }
    }));
  }

  it("updates every version surface, including the existing engine history format", () => temporary((directory) => {
    versionFixture(directory);
    const result = spawnSync(process.execPath, [join(directory, "src/set-version.mjs"), "9.9.9"], { encoding: "utf8" });
    assert.equal(result.status, 0, result.stderr);
    assert.equal(JSON.parse(readFileSync(join(directory, "package.json"), "utf8")).version, "9.9.9");
    assert.equal(JSON.parse(readFileSync(join(directory, "src/config/metadata.json"), "utf8")).product.version, "9.9.9");
    assert.match(readFileSync(join(directory, "install-wor.sh"), "utf8"), /#Version history\n#9\.9\.9 - /);
    assert.match(readFileSync(join(directory, "README.md"), "utf8"), /## Versions\n\n- \*\*9\.9\.9\*\*/);
  }));
});

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
