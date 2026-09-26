import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";
import { readProjectMetadata, readRuntimePaths } from "../src/lib/node-runtime.mjs";

const root = fileURLToPath(new URL("../", import.meta.url));
const release = readFileSync(join(root, ".github/workflows/release.yml"), "utf8");
const checks = readFileSync(join(root, ".github/workflows/shellcheck.yml"), "utf8");
const harness = readFileSync(join(root, "tests/run-tests.sh"), "utf8");

describe("Release workflow prerequisites", () => {
  it("resolves the canonical version in preparation, validation, and packaging", () => {
    const lookups = [...release.matchAll(/^\s*(version="\$\(.+\)")$/gm)];
    assert.equal(lookups.length, 3);
    for (const [, lookup] of lookups) {
      const result = spawnSync("bash", ["-c", `set -euo pipefail\n${lookup}\nprintf '%s' "$version"`], {
        cwd: root,
        encoding: "utf8",
      });
      assert.equal(result.status, 0, result.stderr);
      assert.equal(result.stdout, readProjectMetadata().product.version);
    }
  });

  it("scopes version-commit permissions to the prepare job", () => {
    assert.match(release, /^permissions:\n  contents: read$/m);
    const prepare = release.slice(release.indexOf("  prepare:"), release.indexOf("  validate-linux:"));
    assert.match(prepare, /    permissions:\n      contents: write/);
    assert.match(prepare, /Co-authored-by: Copilot/);
  });

  it("installs pv before integration checks on both release and push workflows", () => {
    for (const [workflow, testStep] of [
      [checks, "- name: Run one-model dry-run integration suite"],
      [release, "- name: Run Linux test suite"],
    ]) {
      const prerequisite = workflow.search(/sudo apt-get install[^\n]*\bpv\b/);
      assert.ok(prerequisite >= 0 && prerequisite < workflow.indexOf(testStep));
    }
    assert.match(harness, /command -v pv >\/dev\/null \|\| die /);
  });

  it("runs CI for standalone GUI helper and canonical metadata changes", () => {
    assert.equal((checks.match(/      - "\*\*\.js"/g) || []).length, 2);
    assert.equal((checks.match(/      - "src\/config\/\*\*"/g) || []).length, 2);
  });

  it("includes source attribution and license notices in both runtime distributions", () => {
    const paths = readRuntimePaths();
    for (const document of ["README.md", "LICENSE", "NOTICE"]) {
      assert.ok(paths.includes(document), `${document} is missing from the runtime manifest`);
    }
  });
});

describe("CI loop-device verification", () => {
  for (const fails of [false, true]) {
    it(fails ? "fails explicitly when privileged geometry inspection fails" : "reads partition geometry using the same privileges as the flash", () => {
      const directory = mkdtempSync(join(tmpdir(), "wor-release-layout-"));
      try {
        writeFileSync(join(directory, "sudo"), `#!/bin/bash
[ "$1" == parted ] || exit 99
shift
export WOR_TEST_PRIVILEGED=1
exec "$WOR_TEST_BIN/parted" "$@"
`, { mode: 0o755 });
        writeFileSync(join(directory, "parted"), `#!/bin/bash
[ "$WOR_TEST_PRIVILEGED" == 1 ] || { printf "permission denied\\n" >&2; exit 1; }
[ "$WOR_TEST_FAIL" != 1 ] || { printf "mock geometry failure\\n" >&2; exit 42; }
printf 'BYT;\\n/dev/wor-test-loop:52428800s:file:512:512:gpt:fixture:;\\n1:2048s:3147775s:3145728s:fat32:WOR_BOOT:boot, esp;\\n2:3147776s:40013823s:36866048s::WOR_INSTALL:msftdata;\\n'
`, { mode: 0o755 });
        const start = harness.indexOf('partition_layout="$(sudo parted');
        const stop = harness.indexOf('\nboot_mount=', start);
        assert.ok(start >= 0 && stop > start);
        const result = spawnSync("bash", ["-c", `
          die() { printf '%s\\n' "$*" >&2; exit 1; }
          DEV_INSTALL=/dev/wor-test-loop
          [ "$(command -v sudo)" == "$WOR_TEST_BIN/sudo" ] || exit 99
          ${harness.slice(start, stop)}
          printf '%s|%s\\n' "$partition_count" "$boot_filesystem"
        `], {
          encoding: "utf8",
          env: {
            ...process.env,
            PATH: `${directory}:${process.env.PATH}`,
            WOR_TEST_BIN: directory,
            WOR_TEST_PRIVILEGED: "0",
            WOR_TEST_FAIL: fails ? "1" : "0",
          },
        });
        if (fails) {
          assert.equal(result.status, 1);
          assert.match(result.stderr, /Could not inspect the test loop-device partition layout/);
          assert.equal(result.stdout, "");
        } else {
          assert.equal(result.status, 0, result.stderr);
          assert.equal(result.stdout.trim(), "2|fat32");
        }
      } finally {
        rmSync(directory, { recursive: true, force: true });
      }
    });
  }
});
