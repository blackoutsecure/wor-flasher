import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  formatRuntimeManifest,
  generateRuntimeManifest,
  isSafeRepoSlug,
  readProjectMetadata,
} from "../src/lib/node-runtime.mjs";

describe("native package tooling", () => {
  it("loads canonical product metadata and accepts only repository slugs", () => {
    const metadata = readProjectMetadata();
    assert.equal(metadata.product.name, "WoR-Flasher");
    assert.ok(metadata.nativeRuntimePaths.includes("src/lib/pi3-boot-refresh"));
    assert.ok(metadata.nativeRuntimePaths.includes("config-templates"));
    assert.equal("runtimePaths" in metadata, false);
    assert.equal(isSafeRepoSlug("Botspot/wor-flasher"), true);
    assert.equal(isSafeRepoSlug("owner/repo.name-1_2"), true);
    for (const slug of ["evil.com/a/b", "../../etc/passwd", "owner/repo?x=1", "owner", ""]) {
      assert.equal(isSafeRepoSlug(slug), false, slug);
    }
  });

  it("generates deterministic native runtime manifests from explicit paths", () => {
    const directory = mkdtempSync(join(tmpdir(), "wor-native-runtime-"));
    try {
      const stage = join(directory, "stage");
      writeFileSync(join(directory, "payload.txt"), "payload\n");
      const manifest = generateRuntimeManifest(stage, "2.1.0", ["payload.txt"], directory);
      assert.deepEqual(manifest.files.map(({ path }) => path), ["payload.txt"]);
      assert.match(manifest.files[0].sha256, /^[a-f0-9]{64}$/);
      assert.equal(readFileSync(join(stage, "payload.txt"), "utf8"), "payload\n");
      assert.match(formatRuntimeManifest(manifest), /"schemaVersion": 1/);
      assert.throws(
        () => generateRuntimeManifest(stage, "2.1.0", ["../outside"], directory),
        /Invalid runtime path/,
      );
      assert.throws(
        () => generateRuntimeManifest(stage, "2.1.0", ["missing"], directory),
        /Missing required runtime path/,
      );
    } finally {
      rmSync(directory, { recursive: true, force: true });
    }
  });

  it("rejects symbolic links in native runtime source trees", () => {
    const directory = mkdtempSync(join(tmpdir(), "wor-native-symlink-"));
    try {
      writeFileSync(join(directory, "real.txt"), "payload\n");
      symlinkSync(join(directory, "real.txt"), join(directory, "linked.txt"));
      assert.throws(
        () => generateRuntimeManifest(join(directory, "stage"), "2.1.0", ["linked.txt"], directory),
        /Symbolic link/,
      );
    } finally {
      rmSync(directory, { recursive: true, force: true });
    }
  });
});
