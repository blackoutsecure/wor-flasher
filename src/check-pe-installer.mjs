#!/usr/bin/env node
import { createHash } from "node:crypto";
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { basename, dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { readProjectMetadata } from "./lib/node-runtime.mjs";

const scriptDir = dirname(fileURLToPath(import.meta.url));
const repoDir = join(scriptDir, "..");
const defaultMetadataFile = join(repoDir, "src", "config", "metadata.json");
const defaultReleaseApiUrl =
  "https://api.github.com/repos/worproject/dldserv-mirror/releases/latest";
const peAssetPattern = /^WoR-PE_Package_.*\.zip$/i;

function fail(message, code = 1) {
  console.error(message);
  process.exit(code);
}

function valueFor(args, name) {
  const index = args.indexOf(name);
  if (index !== -1) return args[index + 1];
  const prefix = `${name}=`;
  const match = args.find((arg) => arg.startsWith(prefix));
  return match ? match.slice(prefix.length) : undefined;
}

export function sha256Buffer(buffer) {
  return createHash("sha256").update(buffer).digest("hex").toUpperCase();
}

export async function readUrlBytes(url) {
  if (url.startsWith("file://")) {
    return readFileSync(fileURLToPath(url));
  }
  if (!url.startsWith("https://")) {
    throw new Error(`refusing to download non-HTTPS URL: ${url}`);
  }
  const response = await fetch(url, {
    headers: { "User-Agent": "WoR-Flasher-PE-Checker" },
    signal: AbortSignal.timeout(30000),
  });
  if (!response.ok) {
    throw new Error(`download failed for ${url}: HTTP ${response.status}`);
  }
  return Buffer.from(await response.arrayBuffer());
}

export async function hashUrl(url) {
  return sha256Buffer(await readUrlBytes(url));
}

export function selectPeAsset(release) {
  const assets = Array.isArray(release?.assets) ? release.assets : [];
  const asset = assets.find((item) => peAssetPattern.test(item?.name || ""));
  if (!asset?.browser_download_url) {
    throw new Error(
      "latest dldserv-mirror release does not contain a WoR-PE_Package_*.zip asset",
    );
  }
  return asset;
}

export async function fetchLatestPeAsset(releaseApiUrl = defaultReleaseApiUrl) {
  const response = await fetch(releaseApiUrl, {
    headers: { "User-Agent": "WoR-Flasher-PE-Checker" },
    signal: AbortSignal.timeout(10000),
  });
  if (!response.ok) {
    throw new Error(`GitHub release lookup failed: HTTP ${response.status}`);
  }
  return selectPeAsset(await response.json());
}

export async function verifyPinnedPeInstaller(
  metadataFile = defaultMetadataFile,
) {
  const metadata = readProjectMetadata(metadataFile);
  const url = metadata.systemDefaults?.peInstallerUrl;
  const expected = metadata.systemDefaults?.peInstallerSha256;
  if (!url || !expected) {
    throw new Error(
      "metadata is missing systemDefaults.peInstallerUrl or peInstallerSha256",
    );
  }
  const actual = await hashUrl(url);
  return {
    ok: actual.toUpperCase() === expected.toUpperCase(),
    url,
    expected,
    actual,
  };
}

export async function updatePeInstallerMetadata({
  metadataFile = defaultMetadataFile,
  releaseApiUrl = defaultReleaseApiUrl,
} = {}) {
  if (!existsSync(metadataFile)) {
    throw new Error(`missing project metadata: ${metadataFile}`);
  }
  const metadata = readProjectMetadata(metadataFile);
  const asset = await fetchLatestPeAsset(releaseApiUrl);
  const url = asset.browser_download_url;
  const sha256 = await hashUrl(url);
  metadata.systemDefaults = metadata.systemDefaults || {};
  metadata.systemDefaults.peInstallerUrl = url;
  metadata.systemDefaults.peInstallerSha256 = sha256;
  writeFileSync(metadataFile, `${JSON.stringify(metadata, null, 2)}\n`);
  return { url, sha256, name: asset.name || basename(url) };
}

async function main() {
  const args = process.argv.slice(2);
  if (args.includes("--help") || args.includes("-h")) {
    console.log(
      `WoR-Flasher PE installer metadata tool\n\nUsage:\n  node src/check-pe-installer.mjs --check [--metadata FILE]\n  node src/check-pe-installer.mjs --write-latest [--metadata FILE] [--release-api-url URL]\n\nOptions:\n  --check             Download the pinned PE installer and verify peInstallerSha256\n  --write-latest      Resolve the latest WoR-PE asset, hash it, and update metadata\n  --metadata FILE     Project metadata file (default: src/config/metadata.json)\n  --release-api-url   GitHub release API URL for latest lookup\n`,
    );
    return;
  }

  const metadataFile = valueFor(args, "--metadata") || defaultMetadataFile;
  if (args.includes("--write-latest")) {
    const result = await updatePeInstallerMetadata({
      metadataFile,
      releaseApiUrl:
        valueFor(args, "--release-api-url") || defaultReleaseApiUrl,
    });
    console.log(`Updated PE installer metadata: ${result.name}`);
    console.log(`URL: ${result.url}`);
    console.log(`SHA-256: ${result.sha256}`);
    return;
  }

  if (args.includes("--check") || args.length === 0) {
    const result = await verifyPinnedPeInstaller(metadataFile);
    if (!result.ok) {
      console.error(`PE installer SHA-256 mismatch for ${result.url}`);
      console.error(`Expected: ${result.expected}`);
      console.error(`Actual:   ${result.actual}`);
      process.exit(1);
    }
    console.log(`PE installer SHA-256 verified: ${result.actual}`);
    return;
  }

  fail("Unsupported option. Run with --help for usage.", 2);
}

if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) {
  main().catch((error) =>
    fail(error instanceof Error ? error.message : String(error)),
  );
}
