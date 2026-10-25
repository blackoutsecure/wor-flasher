#!/usr/bin/env node
import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { desiredPackageFields } from "./sync-package-metadata.mjs";
import { readProjectMetadata } from "./lib/node-runtime.mjs";

const scriptDir = dirname(fileURLToPath(import.meta.url));
const repoDir = join(scriptDir, "..");
const args = process.argv.slice(2);

if (args.length !== 1 || !/^v?\d+\.\d+\.\d+$/i.test(args[0])) {
  console.error("Usage: node src/set-version.mjs <X.Y.Z>");
  process.exit(2);
}

try {
  const version = args[0].replace(/^v/i, "");
  const metadataFile = join(repoDir, "src/config/metadata.json");
  const packageFile = join(repoDir, "package.json");

  const metadata = readProjectMetadata(metadataFile);
  if (typeof metadata.product?.version !== "string") {
    throw new Error("metadata.product.version is missing.");
  }
  metadata.product.version = version;
  const desired = desiredPackageFields(metadata);
  const pkg = JSON.parse(readFileSync(packageFile, "utf8"));
  if (!pkg || typeof pkg !== "object" || Array.isArray(pkg)) throw new Error("package.json must be an object.");

  const updates = new Map([
    [metadataFile, `${JSON.stringify(metadata, null, 2)}\n`],
    [packageFile, `${JSON.stringify({ ...pkg, ...desired }, null, 2)}\n`],
  ]);
  for (const [path, content] of updates) writeFileSync(path, content);
  console.log(`Updated WoR-Flasher version surfaces to ${version}.`);
} catch (error) {
  console.error(error instanceof Error ? error.message : String(error));
  process.exitCode = 1;
}
