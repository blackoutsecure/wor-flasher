#!/usr/bin/env node
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  chmodSync,
  copyFileSync,
  existsSync,
  mkdtempSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, relative } from "node:path";
import { fileURLToPath } from "node:url";
import { generateRuntimeManifest } from "./updater.mjs";

const scriptDir = dirname(fileURLToPath(import.meta.url));
const repoDir = join(scriptDir, "..", "..");
const releaseDir = join(repoDir, "release");
const macosAppTemplate = join(repoDir, "src", "macos-app");
const runtimePathsFile = join(scriptDir, "runtime-paths.json");
const runtimePaths = Object.freeze(
  JSON.parse(readFileSync(runtimePathsFile, "utf8")),
);

const platformPlans = Object.freeze({
  macos: {
    directory: "macos",
    description: "macOS app bundle",
    build: buildMacos,
  },
  linux: {
    directory: "linux",
    description: "Linux shell distribution",
    build: buildLinux,
  },
  windows: {
    directory: "windows",
    description: "Windows placeholder",
    build: buildWindowsPlaceholder,
  },
});

const args = process.argv.slice(2);
const checkOnly = args.includes("--check");
const cleanOnly = args.includes("--clean");
const platform = valueFor("--platform") ?? "all";
let checkDir;

function valueFor(name) {
  const arg = args.find((item) => item.startsWith(`${name}=`));
  return arg ? arg.split("=", 2)[1] : undefined;
}

function fail(message) {
  throw new Error(message);
}

function run(command, commandArgs, env = {}) {
  const result = spawnSync(command, commandArgs, {
    cwd: repoDir,
    env: { ...process.env, ...env },
    stdio: "inherit",
  });
  if (result.status !== 0) fail(`${command} ${commandArgs.join(" ")} failed`);
}

function assertExists(path, message) {
  if (!existsSync(path)) fail(message);
}

function walk(root) {
  const entries = [];
  for (const name of readdirSync(root)) {
    const path = join(root, name);
    const info = statSync(path);
    if (info.isDirectory()) entries.push(...walk(path));
    else if (info.isFile()) entries.push(path);
  }
  return entries.sort();
}

function copyTree(source, target) {
  const info = statSync(source);
  if (info.isDirectory()) {
    mkdirSync(target, { recursive: true });
    for (const name of readdirSync(source))
      copyTree(join(source, name), join(target, name));
    return;
  }
  mkdirSync(dirname(target), { recursive: true });
  copyFileSync(source, target);
  chmodSync(target, info.mode);
}

function copyRuntime(targetRoot) {
  for (const runtimePath of runtimePaths) {
    const source = join(repoDir, runtimePath);
    assertExists(source, `Missing runtime path: ${runtimePath}`);
    copyTree(source, join(targetRoot, runtimePath));
  }
}

function sha256(path) {
  return createHash("sha256").update(readFileSync(path)).digest("hex");
}

function writeChecksums(root) {
  const lines = walk(root)
    .filter((file) => relative(root, file) !== "SHA256SUMS")
    .map((file) => `${sha256(file)}  ${relative(root, file)}`);
  writeFileSync(join(root, "SHA256SUMS"), `${lines.join("\n")}\n`);
}

function writeReadme(root, lines) {
  writeFileSync(join(root, "README.txt"), `${lines.join("\n")}\n`);
}

function selectedPlatforms() {
  if (platform === "all") return ["macos", "linux", "windows"];
  if (!Object.hasOwn(platformPlans, platform)) {
    fail(
      `Unsupported platform '${platform}'. Supported values: all, macos, linux, windows.`,
    );
  }
  return [platform];
}

function preparePlatformDir(name) {
  const base = checkDir ?? releaseDir;
  const root = join(base, platformPlans[name].directory);
  rmSync(root, { recursive: true, force: true });
  mkdirSync(root, { recursive: true });
  return root;
}

function verifyShellRuntime() {
  for (const runtimePath of runtimePaths)
    assertExists(join(repoDir, runtimePath), `Missing ${runtimePath}`);

  const requiredTemplates = [
    "config.schema.json",
    "config.json",
    "pi3.config.txt",
    "pi4.config.txt",
    "pi5.config.txt",
    "pi4-ram-unlock.ps1",
    "pi4-ram-unlock-specialize.xml",
    "oobe-network-bypass.xml",
    "prefinalize.cmd",
  ];

  for (const templateFile of requiredTemplates) {
    const fullPath = join(repoDir, "config-templates", templateFile);
    assertExists(fullPath, `Missing config-templates/${templateFile}`);
    if (statSync(fullPath).size === 0) {
      fail(`Template config-templates/${templateFile} is empty`);
    }
  }

  const schemaPath = join(repoDir, "config-templates", "config.schema.json");
  const configJsonPath = join(repoDir, "config-templates", "config.json");
  try {
    JSON.parse(readFileSync(schemaPath, "utf8"));
    JSON.parse(readFileSync(configJsonPath, "utf8"));
  } catch (err) {
    fail(`Invalid JSON in configuration templates: ${err.message}`);
  }
}

function buildMacos(root) {
  verifyShellRuntime();
  assertExists(macosAppTemplate, "src/macos-app is missing");
  assertExists(
    join(macosAppTemplate, "Contents", "Info.plist"),
    "src/macos-app/Contents/Info.plist is missing",
  );
  assertExists(
    join(macosAppTemplate, "Contents", "MacOS", "WoR-Flasher"),
    "src/macos-app/Contents/MacOS/WoR-Flasher is missing",
  );
  assertExists(
    join(macosAppTemplate, "Contents", "Resources", "WoR-Flasher.icns"),
    "src/macos-app/Contents/Resources/WoR-Flasher.icns is missing",
  );
  const appTarget = join(root, "WoR-Flasher.app");
  copyTree(macosAppTemplate, appTarget);
  const pkg = JSON.parse(readFileSync(join(repoDir, "package.json"), "utf8"));
  const runtimeTarget = join(appTarget, "Contents", "Resources", "runtime");
  const manifestTarget = join(
    appTarget,
    "Contents",
    "Resources",
    "runtime-manifest.json",
  );
  const manifest = generateRuntimeManifest(
    runtimeTarget,
    pkg.version,
    runtimePaths,
    repoDir,
  );
  writeFileSync(manifestTarget, JSON.stringify(manifest, null, 2) + "\n");
  writeReadme(root, [
    "WoR-Flasher macOS release artifact",
    "",
    "This directory is regenerated by npm run build.",
    "Publish the WoR-Flasher.app bundle after code signing/notarization review.",
    "Disk flashing logic remains in install-wor.sh; Node only stages release artifacts.",
    "",
    "Verify SHA256SUMS before publishing.",
  ]);
  writeChecksums(root);
}

function buildLinux(root) {
  verifyShellRuntime();
  const payload = join(root, "wor-flasher");
  copyRuntime(payload);
  writeReadme(root, [
    "WoR-Flasher Linux release artifact",
    "",
    "This directory is regenerated by npm run build.",
    "Run ./wor-flasher/install-wor-gui.sh on a supported Debian-family desktop, or ./wor-flasher/install-wor.sh for CLI use.",
    "Loop, snap, current boot and non-writable devices are hidden from the GUI device list by design.",
    "",
    "Verify SHA256SUMS before publishing.",
  ]);
  writeChecksums(root);
}

function buildWindowsPlaceholder(root) {
  writeReadme(root, [
    "WoR-Flasher Windows release placeholder",
    "",
    "Windows packaging is intentionally not implemented yet.",
    "A Windows UI must get its own reviewed device-safety, elevation and removable-media design before this project ships one.",
    "Until then, Windows users should use the official Windows on Raspberry Imager.",
  ]);
  writeChecksums(root);
}

try {
  if (args.includes("--help") || args.includes("-h")) {
    console.log(
      `WoR-Flasher Release Builder\n\nUsage: node src/node/build-release.mjs [options]\n\nOptions:\n  --platform=all|macos|linux|windows  Target platform (default: all)\n  --check                             Validate release packaging without writing artifacts\n  --clean                             Remove release/ staging directory\n  --help, -h                          Show this help message\n`,
    );
    process.exit(0);
  }

  const unknownArg = args.find(
    (arg) =>
      arg !== "--check" && arg !== "--clean" && !arg.startsWith("--platform="),
  );
  if (unknownArg) fail(`Unsupported option '${unknownArg}'.`);
  if (checkOnly && cleanOnly)
    fail("--check and --clean cannot be used together.");

  if (cleanOnly) {
    rmSync(releaseDir, { recursive: true, force: true });
    console.log("Removed release");
  } else {
    checkDir = checkOnly
      ? mkdtempSync(join(tmpdir(), "wor-flasher-release-check-"))
      : undefined;
    for (const name of selectedPlatforms()) {
      const root = preparePlatformDir(name);
      platformPlans[name].build(root);
    }

    if (checkOnly) console.log("Release packaging plan is valid");
    else
      console.log(
        `Release artifacts staged in ${relative(repoDir, releaseDir)}`,
      );
  }
} catch (error) {
  console.error(error instanceof Error ? error.message : String(error));
  process.exitCode = 1;
} finally {
  if (checkDir) rmSync(checkDir, { recursive: true, force: true });
}
