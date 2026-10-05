#!/usr/bin/env node
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, mkdtempSync, readFileSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

const repoDir = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const sources = ["bootstrap.go", "main_windows.go"];
const sourceDir = join(repoDir, "src/pi3-boot-refresh");
const outputDir = join(repoDir, "src/lib/pi3-boot-refresh");
const binaryName = "Pi3BootRefresh.exe";

function sha256(data) {
  return createHash("sha256").update(data).digest("hex");
}

function sourceHash(root) {
  const hash = createHash("sha256");
  for (const name of sources) {
    hash.update(`${name}\0`);
    hash.update(readFileSync(join(root, "src/pi3-boot-refresh", name)));
  }
  return hash.digest("hex");
}

function verifyPE(binary) {
  if (binary.length < 256 || binary.toString("ascii", 0, 2) !== "MZ") {
    throw new Error("Pi 3 finalizer is not a Windows executable");
  }
  const pe = binary.readUInt32LE(0x3c);
  if (pe + 96 > binary.length || binary.readUInt32LE(pe) !== 0x4550 ||
      binary.readUInt16LE(pe + 4) !== 0xaa64 || binary.readUInt16LE(pe + 24) !== 0x20b ||
      binary.readUInt16LE(pe + 92) !== 3) {
    throw new Error("Pi 3 finalizer must be a Windows ARM64 PE32+ console executable");
  }
}

export function verifyPi3BootRefresh(root = repoDir) {
  const directory = join(root, "src/lib/pi3-boot-refresh");
  const manifest = JSON.parse(readFileSync(join(directory, "manifest.json"), "utf8"));
  const binary = readFileSync(join(directory, binaryName));
  const license = readFileSync(join(directory, "GO-LICENSE.txt"));
  verifyPE(binary);
  if (manifest.schema !== 1 || manifest.target !== "windows/arm64" ||
      manifest.sourceSha256 !== sourceHash(root) || manifest.binarySha256 !== sha256(binary) ||
      manifest.goLicenseSha256 !== sha256(license)) {
    throw new Error("Pi 3 finalizer source/binary manifest is stale; run npm run build:pi3-boot-refresh");
  }
}

function runGo(args, options = {}) {
  const result = spawnSync("go", args, { encoding: "utf8", ...options });
  if (result.error) throw new Error(`Go is required only to rebuild/test the Pi 3 helper: ${result.error.message}`);
  if (result.status !== 0) throw new Error(result.stderr || result.stdout || `go exited ${result.status}`);
  return result.stdout.trim();
}

function build() {
  const goVersion = runGo(["version"]);
  const goRoot = runGo(["env", "GOROOT"]);
  // Homebrew places the SDK license beside libexec rather than inside GOROOT.
  const licensePath = [join(goRoot, "LICENSE"), join(dirname(goRoot), "LICENSE")].find(existsSync);
  if (!licensePath) throw new Error(`Cannot bundle the Go runtime without its license from ${goRoot}`);
  const license = readFileSync(licensePath);
  mkdirSync(outputDir, { recursive: true });
  const staging = mkdtempSync(join(outputDir, ".build-"));
  try {
    const binaryPath = join(staging, binaryName);
    runGo(["build", "-trimpath", "-buildvcs=false", "-ldflags=-s -w -buildid=", "-o", binaryPath, ...sources], {
      cwd: sourceDir,
      env: {
        ...process.env, GO111MODULE: "off", GOTOOLCHAIN: "local", CGO_ENABLED: "0",
        GOOS: "windows", GOARCH: "arm64", GOARM64: "v8.0",
      },
    });
    const binary = readFileSync(binaryPath);
    verifyPE(binary);
    writeFileSync(join(staging, "manifest.json"), `${JSON.stringify({
      schema: 1, target: "windows/arm64", goVersion,
      sourceSha256: sourceHash(repoDir), binarySha256: sha256(binary), goLicenseSha256: sha256(license),
    }, null, 2)}\n`);
    writeFileSync(join(staging, "GO-LICENSE.txt"), license);
    renameSync(binaryPath, join(outputDir, binaryName));
    renameSync(join(staging, "GO-LICENSE.txt"), join(outputDir, "GO-LICENSE.txt"));
    renameSync(join(staging, "manifest.json"), join(outputDir, "manifest.json"));
    verifyPi3BootRefresh();
    console.log(`Built and verified ${binaryName} (${binary.length} bytes, Windows ARM64 v8.0).`);
  } finally {
    rmSync(staging, { recursive: true, force: true });
  }
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const args = process.argv.slice(2);
  if (args.length === 1 && args[0] === "--check") {
    verifyPi3BootRefresh();
    console.log("Pi 3 native finalizer source, ARM64 binary and manifest verified.");
  } else if (args.length === 0) {
    build();
  } else {
    throw new Error("Usage: node src/build-pi3-boot-refresh.mjs [--check]");
  }
}
