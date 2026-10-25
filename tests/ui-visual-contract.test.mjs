import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { readFileSync, mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { browserCoverage, gallery } from "../src/test-ui.mjs";

describe("Visual regression safety and platform coverage", () => {
  it("keeps the supplied partnership artwork local, unchanged and in the native runtime", () => {
    const contract = JSON.parse(readFileSync("tests/visual/design-contract.json", "utf8"));
    const metadata = JSON.parse(readFileSync("src/config/metadata.json", "utf8"));
    const artwork = readFileSync(contract.branding.artwork);
    assert.equal(createHash("sha256").update(artwork).digest("hex"), contract.branding.artworkSha256);
    assert.equal(artwork.readUInt32BE(16), contract.branding.width);
    assert.equal(artwork.readUInt32BE(20), contract.branding.height);
    assert.ok(metadata.nativeRuntimePaths.includes("assets"));
    assert.equal(contract.provenance, "wor-partnership-ui-2026-10-v2");
    assert.ok(contract.requirements.includes("BRAND-001"));
  });
  it("requires all three engines on Linux and unaffected macOS versions", () => {
    for (const [platform, version] of [["linux", ""], ["darwin", "26.1"], ["darwin", "28.0"]]) {
      assert.deepEqual(browserCoverage(platform, version, "1.64.1").projects, ["chromium", "firefox", "webkit"]);
    }
    assert.deepEqual(browserCoverage("darwin", "27.2", "1.65.0").projects, ["chromium", "firefox", "webkit"]);
  });
  it("reports the reproduced macOS Firefox blocker rather than reporting it passed", () => {
    const coverage = browserCoverage("darwin", "27.2", "1.64.1");
    assert.deepEqual(coverage.projects, ["chromium", "webkit"]);
    assert.match(coverage.unavailable.join(" "), /Could not find profile folder/);
    assert.deepEqual(browserCoverage("darwin", "27.2", "1.64.1", ["firefox"]).projects, ["firefox"]);
  });
  it("refuses gallery attachments outside the evidence directory and escapes labels", () => {
    const root = mkdtempSync(join(tmpdir(), "wor-gallery-"));
    try {
      assert.throws(() => gallery({ attachments: [{path:join(root, "../private.png")}] }, root), /escapes/);
      const html = gallery({stats:{expected:1},attachments:[{path:join(root, 'sample<unsafe>.png')}]}, root);
      assert.match(html, /sample&lt;unsafe&gt;\.png/);
      assert.doesNotMatch(html, /sample<unsafe>/);
    } finally { rmSync(root, {recursive:true,force:true}); }
  });
});
