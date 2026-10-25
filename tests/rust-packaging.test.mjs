import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { mkdtempSync, mkdirSync, readFileSync, statSync, writeFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { sourcePaths, stageBinary, verifyRustPi3Candidate, verifyUniversalMachO } from "../src/package-rust.mjs";

const packager = fileURLToPath(new URL("../src/package-rust.mjs", import.meta.url));
const releaseWorkflow = readFileSync(new URL("../.github/workflows/release.yml", import.meta.url), "utf8");
const packageSource = readFileSync(new URL("../src/package-rust.mjs", import.meta.url), "utf8");

describe("Separate Rust migration packaging", () => {
  function universal() {
    const binary = Buffer.alloc(128);
    binary.writeUInt32BE(0xcafebabe, 0);
    binary.writeUInt32BE(2, 4);
    for (const [index, cpu] of [0x01000007, 0x0100000c].entries()) {
      const entry = 8 + index * 20;
      const offset = 64 + index * 32;
      binary.writeUInt32BE(cpu, entry);
      binary.writeUInt32BE(offset, entry + 8);
      binary.writeUInt32BE(32, entry + 12);
      binary.writeUInt32LE(0xfeedfacf, offset);
      binary.writeUInt32LE(cpu, offset + 4);
    }
    return binary;
  }

  it("requires complete distinct Intel and Apple Silicon Mach-O slices", () => {
    verifyUniversalMachO(universal());
    for (const [offset, value] of [[0, 0], [4, 1], [8, 0x0100000c], [20, 999999],
      [36, 64], [96, 0], [100, 0x01000007]]) {
      const binary = universal();
      binary.writeUInt32BE(value, offset);
      assert.throws(() => verifyUniversalMachO(binary));
    }
    for (const size of [0, 7, 47, 100]) {
      assert.throws(() => verifyUniversalMachO(universal().subarray(0, size)));
    }
  });

  it("signs and verifies the complete universal bundle after staging resources", () => {
    assert.match(packageSource, /codesign", \["--verify", "--deep", "--strict", app\]/);
    assert.match(packageSource, /MACOSX_DEPLOYMENT_TARGET: product\.minimumMacosVersion/);
    assert.match(packageSource, /macosTargets = \["aarch64-apple-darwin", "x86_64-apple-darwin"\]/);
  });

  it("rejects universal mode without desktop before building", () => {
    const result = spawnSync(process.execPath, [packager, "--universal"], {
      encoding: "utf8", env: { ...process.env, PATH: "" },
    });
    assert.equal(result.status, 1);
    assert.match(result.stderr, /--universal requires a macOS host and --desktop|supports macOS and Linux only/);
    assert.doesNotMatch(result.stderr, /ENOENT/);
  });

  function candidate() {
    const binary = Buffer.alloc(512);
    binary.write("MZ");
    binary.writeUInt32LE(128, 0x3c);
    binary.writeUInt32LE(0x4550, 128);
    binary.writeUInt16LE(0xaa64, 132);
    binary.writeUInt16LE(2, 150);
    binary.writeUInt16LE(0x20b, 152);
    binary.writeUInt16LE(3, 220);
    binary.writeUInt16LE(0x160, 222);
    return binary;
  }
  const imports = ["KERNEL32.dll", "ntdll.dll", "advapi32.dll"]
    .map((name) => `Import {\n  Name: ${name}\n}`).join("\n");

  it("verifies a hardened ARM64 console candidate with only Windows system imports", () => {
    assert.deepEqual(verifyRustPi3Candidate(candidate(), imports),
      ["advapi32.dll", "kernel32.dll", "ntdll.dll"]);
    assert.deepEqual(verifyRustPi3Candidate(candidate(), imports.replaceAll("\n", "\r\n")),
      ["advapi32.dll", "kernel32.dll", "ntdll.dll"]);
  });

  it("rejects a helper requiring a non-system unwind or compiler DLL", () => {
    for (const name of ["libunwind.dll", "libgcc_s_seh-1.dll", "unknown.dll"]) {
      assert.throws(() => verifyRustPi3Candidate(candidate(), `${imports}\nImport {\n  Name: ${name}\n}`),
        /Non-system or unsupported/);
    }
  });

  it("rejects wrong-machine, non-console, DLL, unprotected and truncated candidates", () => {
    for (const [offset, value] of [[132, 0x8664], [136, 1], [152, 0x10b], [150, 0x2002], [220, 2], [222, 0]]) {
      const binary = candidate();
      binary.writeUInt16LE(value, offset);
      assert.throws(() => verifyRustPi3Candidate(binary, imports));
    }
    assert.throws(() => verifyRustPi3Candidate(candidate().subarray(0, 160), imports));
  });

  it("refuses incomplete, duplicate or malformed import inspection rather than approving an empty list", () => {
    for (const output of ["", "Import {\n}", imports + "\nImport {", imports + "\nImport {\nImport {\n}",
      imports + "\nImport {\n  Name: ntdll.dll\n}", "Import {\n  Name: ntdll.dll\n  Name: advapi32.dll\n}"]) {
      assert.throws(() => verifyRustPi3Candidate(candidate(), output));
    }
  });

  it("stages binaries from Cargo's actual target directory, including an external build cache", () => {
    const directory = mkdtempSync(join(tmpdir(), "wor-cargo-target-"));
    try {
      const targetDirectory = join(directory, "external-target");
      mkdirSync(join(targetDirectory, "release"), { recursive: true });
      const marker = "native-build-fixture\n";
      writeFileSync(join(targetDirectory, "release", "wor-flasher"), marker);
      const output = join(directory, "staged-cli");
      stageBinary("wor-flasher", output, targetDirectory);
      assert.equal(statSync(output).size, Buffer.byteLength(marker));
      assert.equal(readFileSync(output, "utf8"), marker);
      if (process.platform !== "win32") assert.equal(statSync(output).mode & 0o777, 0o755);
    } finally {
      rmSync(directory, { recursive: true, force: true });
    }
  });

  it("excludes ignored/generated build caches from the corresponding-source snapshot", () => {
    const directory = mkdtempSync(join(tmpdir(), "wor-source-inventory-"));
    try {
      assert.equal(spawnSync("git", ["init", "-q", directory]).status, 0);
      for (const path of ["crates/example/src", "crates/example/target/debug", "tests/__pycache__"]) mkdirSync(join(directory, path), { recursive: true });
      for (const [path, content] of [
        ["Cargo.toml", "[workspace]\n"], ["crates/example/src/lib.rs", "pub fn example() {}\n"],
        ["crates/example/target/debug/private-build-output", "must not ship"],
        ["tests/__pycache__/fixture.pyc", "must not ship"],
      ]) writeFileSync(join(directory, path), content);
      assert.deepEqual(sourcePaths(directory), ["Cargo.toml", "crates/example/src/lib.rs"]);
      writeFileSync(join(directory, "source-manifest.json"), JSON.stringify({ formatVersion: 1, files: sourcePaths(directory) }));
      rmSync(join(directory, ".git"), { recursive: true, force: true });
      assert.deepEqual(sourcePaths(directory), ["Cargo.toml", "crates/example/src/lib.rs"]);
      writeFileSync(join(directory, "source-manifest.json"), JSON.stringify({ formatVersion: 1, files: ["../outside"] }));
      assert.throws(() => sourcePaths(directory), /Unsafe/);
    } finally {
      rmSync(directory, { recursive: true, force: true });
    }
  });
  it("describes the build without requiring Cargo or a display", () => {
    const result = spawnSync(process.execPath, [packager, "--help"], {
      encoding: "utf8", env: { ...process.env, PATH: "" },
    });

    it("publishes validated native Rust packages as the only release clients", () => {
      assert.match(releaseWorkflow, /node src\/package-rust\.mjs --desktop --check/);
      assert.match(releaseWorkflow, /node src\/package-rust\.mjs --desktop/);
      assert.match(releaseWorkflow, /actions\/upload-artifact@/);
      assert.match(releaseWorkflow, /actions\/download-artifact@/);
      assert.match(releaseWorkflow, /wor-flasher-\$\{RELEASE_VERSION#v\}-\$\{target\}\.tar\.gz/);
      assert.match(releaseWorkflow, /wor-flasher-\*\.tar\.gz/);
      assert.doesNotMatch(releaseWorkflow, /release\/standalone\/install-wor\.sh|linux-cli\.tar\.gz|macos-runtime\.json|\.dmg/);
      assert.match(releaseWorkflow, /Native physical writes remain explicitly experimental/);
      assert.match(packageSource, /This is the native Rust application/);
      assert.match(packageSource, /Physical native writes are experimental/);
    });
    assert.equal(result.status, 0, result.stderr);
    assert.match(result.stdout, /--desktop.*--check/);
    assert.match(result.stdout, /temporary staging/);
    assert.doesNotMatch(result.stderr, /cargo|ENOENT/);
  });

  it("requires an explicit open-source toolchain before building a Windows helper candidate", () => {
    const result = spawnSync(process.execPath, [packager, "--pi3-helper", "--check"], {
      encoding: "utf8", env: { ...process.env, PATH: "", LLVM_MINGW_ROOT: "" },
    });
    assert.equal(result.status, 1);
    assert.match(result.stderr, /LLVM_MINGW_ROOT/);
    assert.doesNotMatch(result.stderr, /Unknown option|spawn|ENOENT/);
  });

  for (const option of ["--unknown", "--platform=windows", "--out=/"]) {
    it(`rejects ${option} before starting a build or changing output`, () => {
      const result = spawnSync(process.execPath, [packager, option], {
        encoding: "utf8", env: { ...process.env, PATH: "" },
      });
      assert.equal(result.status, 1);
      assert.match(result.stderr, /Unknown option/);
      assert.doesNotMatch(result.stderr, /spawn|ENOENT/);
    });
  }
});
