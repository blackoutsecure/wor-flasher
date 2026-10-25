import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { cpSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { verifyPi3BootRefresh } from "../src/build-pi3-boot-refresh.mjs";

const root = fileURLToPath(new URL("../", import.meta.url));

function fixture() {
  const directory = mkdtempSync(join(tmpdir(), "wor-pi3-helper-"));
  for (const path of ["src/pi3-boot-refresh", "src/lib/pi3-boot-refresh"]) {
    cpSync(join(root, path), join(directory, path), { recursive: true });
  }
  return directory;
}

describe("shipped Pi 3 Windows payload integrity", () => {
  it("verifies the Rust-consumed Go helper, source, manifest and license", () => {
    assert.doesNotThrow(() => verifyPi3BootRefresh());
  });

  for (const [label, path] of [
    ["source", "src/pi3-boot-refresh/bootstrap.go"],
    ["binary", "src/lib/pi3-boot-refresh/Pi3BootRefresh.exe"],
    ["license", "src/lib/pi3-boot-refresh/GO-LICENSE.txt"],
  ]) {
    it(`rejects a changed ${label} instead of packaging a stale helper`, () => {
      const directory = fixture();
      try {
        const original = readFileSync(join(directory, path));
        writeFileSync(join(directory, path), Buffer.concat([original, Buffer.from("\nchanged\n")]));
        assert.throws(() => verifyPi3BootRefresh(directory), /manifest is stale/);
      } finally {
        rmSync(directory, { recursive: true, force: true });
      }
    });
  }
});
