import { describe, it } from "node:test";
import assert from "node:assert/strict";
import {
  mkdtempSync,
  rmSync,
  writeFileSync,
  mkdirSync,
  readFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { spawnSync } from "node:child_process";
import {
  selectPeAsset,
  sha256Buffer,
  updatePeInstallerMetadata,
  verifyPinnedPeInstaller,
} from "../src/check-pe-installer.mjs";
import {
  checkPackageMetadata,
  desiredPackageFields,
  writePackageMetadata,
} from "../src/sync-package-metadata.mjs";
import {
  parseVersion,
  compareVersions,
  readProjectMetadata,
  readRuntimePaths,
  generateRuntimeManifest,
  formatRuntimeManifest,
  verifyRuntimeManifest,
  checkReleaseUpdate,
  isSafeRepoSlug,
  loadSystemConfig,
} from "../src/lib/node-runtime.mjs";

function shellMetadata(name) {
  const result = spawnSync(
    "bash",
    [
      "-c",
      `source src/lib/metadata.sh >/dev/null 2>&1; printf '%s' "$${name}"`,
    ],
    { encoding: "utf8" },
  );
  assert.equal(result.status, 0, result.stderr);
  return result.stdout;
}

function shellNumberMetadata(name) {
  const value = shellMetadata(name);
  return value === null ? null : Number(value);
}

function plistString(content, key) {
  const match = content.match(
    new RegExp(`<key>${key}</key>\\s*<string>([^<]*)</string>`),
  );
  return match ? match[1] : null;
}

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

describe("Node.js Tooling - Release Update Check", () => {
  //the slug is interpolated into the update URL, so a loose one could redirect the check
  it("should only accept a plain owner/name repository slug", () => {
    assert.equal(isSafeRepoSlug("Botspot/wor-flasher"), true);
    assert.equal(isSafeRepoSlug("owner/repo.name-1_2"), true);
    assert.equal(isSafeRepoSlug("evil.com/a/b"), false);
    assert.equal(isSafeRepoSlug("../../etc/passwd"), false);
    assert.equal(isSafeRepoSlug("owner/repo?x=1"), false);
    assert.equal(isSafeRepoSlug("owner"), false);
    assert.equal(isSafeRepoSlug(""), false);
    assert.equal(isSafeRepoSlug(undefined), false);
  });

  it("should refuse to contact anything when the slug is unsafe", async () => {
    let called = false;
    const res = await checkReleaseUpdate({
      currentVersion: "1.0.0",
      repoSlug: "https://evil.invalid/x",
      fetchImpl: () => {
        called = true;
        throw new Error("fetch must not run for an unsafe slug");
      },
    });
    assert.equal(called, false);
    assert.equal(res.updateAvailable, false);
    assert.equal(res.error, "unsafe-repo-slug");
  });

  it("should report an available update over HTTPS without following redirects", async () => {
    let seenUrl = "";
    let seenRedirect = "";
    const res = await checkReleaseUpdate({
      currentVersion: "1.0.2",
      repoSlug: "Botspot/wor-flasher",
      fetchImpl: (url, options) => {
        seenUrl = url;
        seenRedirect = options.redirect;
        return {
          ok: true,
          json: async () => ({
            tag_name: "v1.1.0",
            html_url: "https://github.com/Botspot/wor-flasher/releases/v1.1.0",
          }),
        };
      },
    });
    assert.equal(
      seenUrl,
      "https://api.github.com/repos/Botspot/wor-flasher/releases/latest",
    );
    assert.equal(seenRedirect, "error");
    assert.equal(res.updateAvailable, true);
    assert.equal(res.latestVersion, "v1.1.0");
  });

  it("should not report an update for equal or older releases", async () => {
    const stub = (tag) => () => ({
      ok: true,
      json: async () => ({ tag_name: tag, html_url: "" }),
    });
    for (const tag of ["v1.0.2", "v1.0.1", "v0.9.9"]) {
      const res = await checkReleaseUpdate({
        currentVersion: "1.0.2",
        repoSlug: "Botspot/wor-flasher",
        fetchImpl: stub(tag),
      });
      assert.equal(res.updateAvailable, false, `tag ${tag} must not update`);
    }
  });

  it("should ignore drafts and prereleases", async () => {
    for (const flags of [{ draft: true }, { prerelease: true }]) {
      const res = await checkReleaseUpdate({
        currentVersion: "1.0.2",
        repoSlug: "Botspot/wor-flasher",
        fetchImpl: () => ({
          ok: true,
          json: async () => ({ tag_name: "v9.9.9", ...flags }),
        }),
      });
      assert.equal(res.updateAvailable, false);
    }
  });

  it("should drop a non-HTTPS release link and survive a failed request", async () => {
    const downgraded = await checkReleaseUpdate({
      currentVersion: "1.0.2",
      repoSlug: "Botspot/wor-flasher",
      fetchImpl: () => ({
        ok: true,
        json: async () => ({
          tag_name: "v1.1.0",
          html_url: "http://evil.invalid/release",
        }),
      }),
    });
    assert.equal(downgraded.updateAvailable, true);
    assert.equal(downgraded.htmlUrl, "");

    const failed = await checkReleaseUpdate({
      currentVersion: "1.0.2",
      repoSlug: "Botspot/wor-flasher",
      fetchImpl: () => {
        throw new Error("network down");
      },
    });
    assert.equal(failed.updateAvailable, false);
    assert.equal(failed.error, "network down");
  });
});

describe("Node.js Tooling - Package Metadata Sync", () => {
  it("should report the real package.json as in sync with src/config/metadata.json", () => {
    assert.deepEqual(checkPackageMetadata(), []);
  });

  it("should derive package.json fields from metadata.product and systemDefaults.repoSlug", () => {
    const desired = desiredPackageFields({
      product: {
        version: "9.9.9",
        description: "Example description.",
        license: "MIT",
        keywords: ["a", "b"],
        fundingUrls: ["https://github.com/sponsors/example"],
      },
      systemDefaults: { repoSlug: "example/repo" },
    });
    assert.equal(desired.version, "9.9.9");
    assert.equal(desired.homepage, "https://github.com/example/repo#readme");
    assert.deepEqual(desired.repository, {
      type: "git",
      url: "git+https://github.com/example/repo.git",
    });
    assert.deepEqual(desired.bugs, {
      url: "https://github.com/example/repo/issues",
    });
    assert.deepEqual(desired.funding, [
      { type: "github", url: "https://github.com/sponsors/example" },
    ]);
    assert.deepEqual(desired.keywords, ["a", "b"]);
  });

  it("should detect drift and rewrite package.json to match metadata.json", () => {
    const tempDir = mkdtempSync(join(tmpdir(), "wor-pkg-sync-"));
    try {
      mkdirSync(join(tempDir, "src", "config"), { recursive: true });
      const metadataFile = join(tempDir, "src", "config", "metadata.json");
      const packageFile = join(tempDir, "package.json");
      writeFileSync(
        metadataFile,
        JSON.stringify({
          product: {
            version: "2.0.0",
            description: "New description.",
            license: "MIT",
            keywords: ["x"],
            fundingUrls: ["https://github.com/sponsors/example"],
          },
          systemDefaults: { repoSlug: "example/repo" },
        }),
      );
      writeFileSync(
        packageFile,
        JSON.stringify(
          { name: "example", version: "1.0.0", private: true },
          null,
          2,
        ),
      );

      const drift = checkPackageMetadata(packageFile, metadataFile);
      assert.ok(drift.some((d) => d.key === "version"));
      assert.ok(drift.some((d) => d.key === "description"));

      writePackageMetadata(packageFile, metadataFile);
      const updated = JSON.parse(readFileSync(packageFile, "utf8"));
      assert.equal(updated.name, "example");
      assert.equal(updated.version, "2.0.0");
      assert.equal(updated.description, "New description.");
      assert.equal(updated.homepage, "https://github.com/example/repo#readme");
      assert.deepEqual(checkPackageMetadata(packageFile, metadataFile), []);
    } finally {
      rmSync(tempDir, { recursive: true, force: true });
    }
  });
});

describe("Node.js Tooling - Runtime Paths & Manifests", () => {
  it("should verify PE installer metadata hashes without network when given a file URL", async () => {
    const tempDir = mkdtempSync(join(tmpdir(), "wor-pe-check-"));
    try {
      const peFile = join(tempDir, "WoR-PE_Package_fixture.zip");
      const bytes = Buffer.from("fixture pe installer bytes", "utf8");
      writeFileSync(peFile, bytes);
      const metadataFile = join(tempDir, "metadata.json");
      writeFileSync(
        metadataFile,
        JSON.stringify({
          systemDefaults: {
            peInstallerUrl: `file://${peFile}`,
            peInstallerSha256: sha256Buffer(bytes),
          },
        }),
      );

      const result = await verifyPinnedPeInstaller(metadataFile);
      assert.equal(result.ok, true);
      assert.equal(result.actual, sha256Buffer(bytes));
    } finally {
      rmSync(tempDir, { recursive: true, force: true });
    }
  });

  it("should select the WoR-PE package asset from a GitHub release payload", () => {
    const asset = selectPeAsset({
      assets: [
        {
          name: "notes.txt",
          browser_download_url: "https://example.invalid/notes.txt",
        },
        {
          name: "WoR-PE_Package_1.2.3.zip",
          browser_download_url:
            "https://example.invalid/WoR-PE_Package_1.2.3.zip",
        },
      ],
    });
    assert.equal(asset.name, "WoR-PE_Package_1.2.3.zip");
  });

  it("should update PE installer metadata from a release payload and downloaded hash", async () => {
    const tempDir = mkdtempSync(join(tmpdir(), "wor-pe-update-"));
    try {
      const peFile = join(tempDir, "WoR-PE_Package_9.9.9.zip");
      const bytes = Buffer.from("updated pe installer bytes", "utf8");
      writeFileSync(peFile, bytes);
      const metadataFile = join(tempDir, "metadata.json");
      writeFileSync(
        metadataFile,
        JSON.stringify({
          systemDefaults: {
            peInstallerUrl: "https://example.invalid/old.zip",
            peInstallerSha256: "0".repeat(64),
          },
        }),
      );

      const previousFetch = globalThis.fetch;
      globalThis.fetch = async () => ({
        ok: true,
        json: async () => ({
          assets: [
            {
              name: "WoR-PE_Package_9.9.9.zip",
              browser_download_url: `file://${peFile}`,
            },
          ],
        }),
      });
      try {
        const result = await updatePeInstallerMetadata({
          metadataFile,
          releaseApiUrl: "https://api.example.invalid/latest",
        });
        assert.equal(result.sha256, sha256Buffer(bytes));
        const updated = JSON.parse(readFileSync(metadataFile, "utf8"));
        assert.equal(updated.systemDefaults.peInstallerUrl, `file://${peFile}`);
        assert.equal(
          updated.systemDefaults.peInstallerSha256,
          sha256Buffer(bytes),
        );
      } finally {
        globalThis.fetch = previousFetch;
      }
    } finally {
      rmSync(tempDir, { recursive: true, force: true });
    }
  });

  it("should keep project metadata aligned with package metadata, shell metadata and the app plist", () => {
    const pkg = JSON.parse(readFileSync("package.json", "utf8"));
    const project = readProjectMetadata();
    const plist = readFileSync("src/macos-app/Contents/Info.plist", "utf8");

    assert.equal(pkg.name, "wor-flasher");
    assert.equal(pkg.license, "GPL-3.0-or-later");
    assert.equal(pkg.version, project.product.version);
    assert.equal(project.product.version, shellMetadata("WOR_FLASHER_VERSION"));
    assert.equal(project.product.name, shellMetadata("WOR_FLASHER_NAME"));
    assert.equal(project.product.iconName, shellMetadata("WOR_ICON_NAME"));
    assert.equal(
      project.product.iconFilename,
      shellMetadata("WOR_ICON_FILENAME"),
    );
    assert.equal(
      project.product.logoFilename,
      shellMetadata("WOR_LOGO_FILENAME"),
    );
    assert.equal(
      project.product.assetsDirname,
      shellMetadata("WOR_ASSETS_DIRNAME"),
    );
    assert.equal(
      project.systemDefaults.peInstallerUrl,
      shellMetadata("WOR_DEFAULT_PE_INSTALLER_URL"),
    );
    assert.equal(
      project.systemDefaults.peInstallerSha256,
      shellMetadata("WOR_DEFAULT_PE_INSTALLER_SHA256"),
    );
    assert.equal(
      project.systemDefaults.uefiVerPi3,
      shellMetadata("WOR_DEFAULT_UEFI_VER_PI3"),
    );
    assert.equal(
      project.systemDefaults.uefiVerPi4,
      shellMetadata("WOR_DEFAULT_UEFI_VER_PI4"),
    );
    assert.equal(
      project.systemDefaults.uefiVerPi5,
      shellMetadata("WOR_DEFAULT_UEFI_VER_PI5"),
    );
    assert.equal(
      project.systemDefaults.uefiRepoPi3,
      shellMetadata("WOR_DEFAULT_UEFI_REPO_PI3"),
    );
    assert.equal(
      project.systemDefaults.uefiRepoPi4,
      shellMetadata("WOR_DEFAULT_UEFI_REPO_PI4"),
    );
    assert.equal(
      project.systemDefaults.uefiRepoPi5,
      shellMetadata("WOR_DEFAULT_UEFI_REPO_PI5"),
    );
    assert.equal(
      project.systemDefaults.driverVer,
      shellMetadata("WOR_DEFAULT_DRIVER_VER"),
    );
    assert.equal(
      project.systemDefaults.driversRepo,
      shellMetadata("WOR_DEFAULT_DRIVERS_REPO"),
    );
    assert.equal(
      project.systemDefaults.armv80MaxBuild,
      shellNumberMetadata("WOR_DEFAULT_ARMV80_MAX_BUILD"),
    );
    assert.equal(
      project.systemDefaults.win11MinBuild,
      shellNumberMetadata("WOR_DEFAULT_WIN11_MIN_BUILD"),
    );
    assert.equal(
      project.systemDefaults.win10OldestBuild,
      shellMetadata("WOR_DEFAULT_WIN10_OLDEST_BUILD"),
    );
    assert.equal(
      project.systemDefaults.exampleBid,
      shellMetadata("WOR_DEFAULT_EXAMPLE_BID"),
    );
    assert.equal(
      project.systemDefaults.armv80SafeBid,
      shellMetadata("WOR_DEFAULT_ARMV80_SAFE_BID"),
    );
    assert.equal(
      project.systemDefaults.repoSlug,
      shellMetadata("WOR_DEFAULT_REPO_SLUG"),
    );
    assert.equal(
      project.systemDefaults.logDirname,
      shellMetadata("WOR_DEFAULT_LOG_DIRNAME"),
    );
    assert.equal(
      project.product.name,
      plistString(plist, "CFBundleDisplayName"),
    );
    assert.equal(
      project.product.name,
      plistString(plist, "CFBundleExecutable"),
    );
    assert.equal(
      project.product.iconFilename,
      plistString(plist, "CFBundleIconFile"),
    );
    assert.equal(
      project.product.iconName,
      plistString(plist, "CFBundleIconName"),
    );
    assert.equal(project.product.name, plistString(plist, "CFBundleName"));
    assert.equal(
      project.product.bundleIdentifier,
      plistString(plist, "CFBundleIdentifier"),
    );
    assert.equal(
      project.product.minimumMacosVersion,
      plistString(plist, "LSMinimumSystemVersion"),
    );
    assert.equal(pkg.version, plistString(plist, "CFBundleShortVersionString"));
    assert.equal(pkg.version, plistString(plist, "CFBundleVersion"));
  });

  it("should load system defaults from project metadata, not the default config template", () => {
    const template = JSON.parse(
      readFileSync("config-templates/config.json", "utf8"),
    );
    assert.equal(template.system, undefined);

    const config = loadSystemConfig();
    assert.ok(config.system);
    assert.equal(
      config.system.peInstallerUrl,
      "https://github.com/worproject/dldserv-mirror/releases/download/13%2F02%2F2024/WoR-PE_Package_1.1.0.zip",
    );
    assert.equal(
      config.system.peInstallerSha256,
      "A039E28FE7E39147899B0634C15E336C3B26A6F76201092EBB9732474CD43D0A",
    );
    assert.equal(config.system.uefiVerPi4, "v1.50");
    assert.equal(config.system.driverVer, "v0.17");
    assert.equal(config.system.repoSlug, "Botspot/wor-flasher");
    //the retired git self-updater pins must not come back through the config cascade
    assert.equal(config.system.updateRepoUrl, undefined);
    assert.equal(config.system.updateRef, undefined);
  });

  it("should let a custom config system block override project metadata defaults", () => {
    const tempDir = mkdtempSync(join(tmpdir(), "wor-system-config-"));
    try {
      mkdirSync(join(tempDir, "config-templates"), { recursive: true });
      mkdirSync(join(tempDir, "src", "config"), { recursive: true });
      writeFileSync(
        join(tempDir, "src", "config", "metadata.json"),
        JSON.stringify({
          systemDefaults: {
            peInstallerUrl: "https://example.invalid/default.zip",
            peInstallerSha256: "abc",
            repoSlug: "example/default",
          },
        }),
      );
      writeFileSync(
        join(tempDir, "config-templates", "config.json"),
        JSON.stringify({
          system: {
            peInstallerUrl: "https://example.invalid/override.zip",
            repoSlug: "example/override",
          },
        }),
      );

      const config = loadSystemConfig(tempDir);
      assert.equal(
        config.system.peInstallerUrl,
        "https://example.invalid/override.zip",
      );
      assert.equal(config.system.peInstallerSha256, "abc");
      assert.equal(config.system.repoSlug, "example/override");
    } finally {
      rmSync(tempDir, { recursive: true, force: true });
    }
  });

  it("should read runtime paths configuration from project metadata", () => {
    const paths = readRuntimePaths();
    assert.ok(Array.isArray(paths));
    assert.ok(paths.includes("install-wor.sh"));
    assert.ok(paths.includes("src/lib"));
    assert.ok(paths.includes("src/updater.mjs"));
    assert.ok(!paths.includes("src/build-release.mjs"));
    assert.ok(!paths.includes("src/package-macos-app.mjs"));
    assert.ok(!paths.includes("src/set-version.mjs"));
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

      writeFileSync(manifestFile, formatRuntimeManifest(manifest));
      assert.equal(verifyRuntimeManifest(tempDir), true);
      assert.match(
        formatRuntimeManifest(manifest),
        /\{"path": "test\.txt", "sha256": "[0-9a-f]{64}", "mode": "[0-7]+"\}/,
      );

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
      ["src/build-release.mjs", "--platform=macos"],
      { encoding: "utf8" },
    );
    assert.equal(buildRes.status, 0);

    const checkRes = spawnSync(
      "node",
      ["src/package-macos-app.mjs", "--check"],
      { encoding: "utf8" },
    );
    assert.equal(checkRes.status, 0);
    assert.match(checkRes.stdout, /Embedded macOS runtime is current/);
  });

  it("should execute package-macos-app --write cleanly when app bundle exists", () => {
    const writeRes = spawnSync(
      "node",
      ["src/package-macos-app.mjs", "--write"],
      { encoding: "utf8" },
    );
    assert.equal(writeRes.status, 0);
    assert.match(writeRes.stdout, /Packaged macOS runtime/);
  });

  it("should reject set-version with invalid semver format", () => {
    const result = spawnSync("node", ["src/set-version.mjs", "invalid"], {
      encoding: "utf8",
    });
    assert.notEqual(result.status, 0);
  });
});
