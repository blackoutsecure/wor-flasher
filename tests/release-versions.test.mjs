import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { cpSync, mkdtempSync, mkdirSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../", import.meta.url));

function fixture() {
  const directory = mkdtempSync(join(tmpdir(), "wor-version-tool-"));
  mkdirSync(join(directory, "src/config"), { recursive: true });
  mkdirSync(join(directory, "src/lib"), { recursive: true });
  for (const path of [
    "src/config/metadata.json",
    "src/set-version.mjs",
    "src/sync-package-metadata.mjs",
    "src/lib/node-runtime.mjs",
    "package.json",
  ]) {
    cpSync(join(root, path), join(directory, path));
  }
  return directory;
}

describe("native product version tool", () => {
  it("updates only canonical and derived package metadata", () => {
    const directory = fixture();
    try {
      const result = spawnSync(process.execPath, ["src/set-version.mjs", "v3.4.5"], {
        cwd: directory,
        encoding: "utf8",
      });
      assert.equal(result.status, 0, result.stderr);
      const metadata = JSON.parse(readFileSync(join(directory, "src/config/metadata.json"), "utf8"));
      const pkg = JSON.parse(readFileSync(join(directory, "package.json"), "utf8"));
      assert.equal(metadata.product.version, "3.4.5");
      assert.equal(pkg.version, "3.4.5");
      assert.equal(pkg.description, metadata.product.description);
      assert.equal(pkg.repository.url, `git+https://github.com/${metadata.systemDefaults.repoSlug}.git`);
    } finally {
      rmSync(directory, { recursive: true, force: true });
    }
  });

  it("rejects malformed versions without modifying product metadata", () => {
    const directory = fixture();
    try {
      const before = readFileSync(join(directory, "src/config/metadata.json"));
      const result = spawnSync(process.execPath, ["src/set-version.mjs", "not-a-version"], {
        cwd: directory,
        encoding: "utf8",
      });
      assert.equal(result.status, 2);
      assert.match(result.stderr, /Usage:/);
      assert.deepEqual(readFileSync(join(directory, "src/config/metadata.json")), before);
    } finally {
      rmSync(directory, { recursive: true, force: true });
    }
  });
});
