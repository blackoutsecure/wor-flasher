import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, rmSync, writeFileSync, mkdirSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { spawnSync } from "node:child_process";
import {
  parseVersion,
  compareVersions,
  readRuntimePaths,
  generateRuntimeManifest,
  verifyRuntimeManifest,
  checkGitUpdate,
  loadSystemConfig,
} from "../src/node/updater.mjs";

describe("Node.js Tooling - Version Parsing & Comparison", () => {
  it("should parse semver version strings into tuple arrays", () => {
    assert.deepEqual(parseVersion("1.0.2"), [1, 0, 2]);
    assert.deepEqual(parseVersion("v1.2.3"), [1, 2, 3]);
    assert.deepEqual(parseVersion("2.0"), [2, 0, 0]);
    assert.deepEqual(parseVersion("invalid"), [0, 0, 0]);
    assert.deepEqual(parseVersion(""), [0, 0, 0]);
  });

  it("should compare version strings correctly", () => {
    assert.equal(compareVersions("1.0.2", "1.0.3"), -1);
    assert.equal(compareVersions("1.0.2", "1.0.2"), 0);
    assert.equal(compareVersions("1.1.0", "1.0.9"), 1);
    assert.equal(compareVersions("v2.0.0", "1.9.9"), 1);
    assert.equal(compareVersions("1.0.0", "v1.0.0"), 0);
  });
});

describe("Node.js Tooling - Runtime Paths & Manifests", () => {
  it("should load system configuration from config-templates/config.json", () => {
    const config = loadSystemConfig();
    assert.ok(config.system);
    assert.equal(
      config.system.updateRepoUrl,
      "https://github.com/blackoutsecure/wor-flasher.git",
    );
    assert.equal(config.system.updateRef, "HEAD");
  });

  it("should read runtime paths configuration from JSON", () => {
    const paths = readRuntimePaths();
    assert.ok(Array.isArray(paths));
    assert.ok(paths.includes("install-wor.sh"));
    assert.ok(paths.includes("src/lib"));
    assert.ok(paths.includes("src/node"));
  });

  it("should generate and verify runtime manifests for temporary directory structure", () => {
    const tempDir = mkdtempSync(join(tmpdir(), "wor-test-manifest-"));
    try {
      const stageDir = join(tempDir, "Contents", "Resources", "runtime");
      const manifestFile = join(
        tempDir,
        "Contents",
        "Resources",
        "runtime-manifest.json",
      );
      mkdirSync(stageDir, { recursive: true });

      const testFile = join(stageDir, "test.txt");
      writeFileSync(testFile, "hello wor-flasher", "utf8");

      const manifest = generateRuntimeManifest(
        stageDir,
        "1.0.2",
        ["test.txt"],
        stageDir,
      );
      assert.equal(manifest.schemaVersion, 1);
      assert.equal(manifest.version, "1.0.2");
      assert.equal(manifest.files.length, 1);
      assert.equal(manifest.files[0].path, "test.txt");

      writeFileSync(manifestFile, JSON.stringify(manifest, null, 2));
      assert.equal(verifyRuntimeManifest(tempDir), true);

      // Tamper with test file
      writeFileSync(testFile, "tampered content", "utf8");
      assert.equal(verifyRuntimeManifest(tempDir), false);
    } finally {
      rmSync(tempDir, { recursive: true, force: true });
    }
  });
});

describe("Node.js Tooling - macOS Packaging & Version CLI", () => {
  it("should pass packaging validation for staged macOS app", () => {
    const buildRes = spawnSync(
      "node",
      ["src/node/build-release.mjs", "--platform=macos"],
      { encoding: "utf8" },
    );
    assert.equal(buildRes.status, 0);

    const checkRes = spawnSync(
      "node",
      ["src/node/package-macos-app.mjs", "--check"],
      { encoding: "utf8" },
    );
    assert.equal(checkRes.status, 0);
    assert.match(checkRes.stdout, /Embedded macOS runtime is current/);
  });

  it("should execute package-macos-app --write cleanly when app bundle exists", () => {
    const writeRes = spawnSync(
      "node",
      ["src/node/package-macos-app.mjs", "--write"],
      { encoding: "utf8" },
    );
    assert.equal(writeRes.status, 0);
    assert.match(writeRes.stdout, /Packaged macOS runtime/);
  });

  it("should reject set-version with invalid semver format", () => {
    const result = spawnSync("node", ["src/node/set-version.mjs", "invalid"], {
      encoding: "utf8",
    });
    assert.notEqual(result.status, 0);
  });
});
