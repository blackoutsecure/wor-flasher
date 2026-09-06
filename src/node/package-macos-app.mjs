#!/usr/bin/env node
import { existsSync, readFileSync, writeFileSync, mkdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import {
  generateRuntimeManifest,
  verifyRuntimeManifest,
  readRuntimePaths,
} from "./updater.mjs";

const scriptDir = dirname(fileURLToPath(import.meta.url));
const repoDir = join(scriptDir, "..", "..");
const appRoot =
  process.env.WOR_MACOS_APP_ROOT ||
  join(repoDir, "release", "macos", "WoR-Flasher.app");
const mode = process.argv[2] || "--check";

function fail(msg) {
  console.error(msg);
  process.exit(1);
}

if (mode !== "--check" && mode !== "--write") {
  console.error("Usage: node src/node/package-macos-app.mjs [--check|--write]");
  process.exit(2);
}

const infoPlist = join(appRoot, "Contents", "Info.plist");
if (!existsSync(infoPlist)) {
  fail(`macOS app template is missing at ${appRoot}; run npm run build:macos.`);
}

const metadataFile = join(repoDir, "src", "lib", "metadata.sh");
const metadataContent = readFileSync(metadataFile, "utf8");
const match = metadataContent.match(/^WOR_FLASHER_VERSION=['"]([^'"]+)['"]/m);
const version = match ? match[1] : "1.0.2";

const resourcesDir = join(appRoot, "Contents", "Resources");
const runtimeDir = join(resourcesDir, "runtime");
const manifestFile = join(resourcesDir, "runtime-manifest.json");

if (mode === "--check") {
  if (
    !existsSync(appRoot) ||
    !existsSync(runtimeDir) ||
    !existsSync(manifestFile)
  ) {
    fail(
      "Generated macOS app or runtime manifest is missing; run npm run build:macos.",
    );
  }
  if (!verifyRuntimeManifest(appRoot)) {
    fail(
      "Embedded macOS runtime manifest is invalid or stale; run npm run build:macos.",
    );
  }
  console.log(`Embedded macOS runtime is current (${version}).`);
} else if (mode === "--write") {
  mkdirSync(resourcesDir, { recursive: true });
  const runtimePaths = readRuntimePaths();
  const manifest = generateRuntimeManifest(
    runtimeDir,
    version,
    runtimePaths,
    repoDir,
  );
  writeFileSync(manifestFile, JSON.stringify(manifest, null, 2) + "\n");
  console.log(`Packaged macOS runtime ${version}.`);
}
