#!/usr/bin/env node
import { readFileSync, writeFileSync, existsSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";
import { isMainModule, isSafeRepoSlug, readProjectMetadata } from "./lib/node-runtime.mjs";

const scriptDir = dirname(fileURLToPath(import.meta.url));
const repoDir = join(scriptDir, "..");
const defaultPackageFile = join(repoDir, "package.json");

//src/config/metadata.json is the source of truth for these package.json fields. Anything here
//is derived, never hand-edited in package.json: edit metadata.json and run --write instead.
export function desiredPackageFields(metadata) {
  const product = metadata.product || {};
  const repoSlug = metadata.systemDefaults?.repoSlug;
  if (!isSafeRepoSlug(repoSlug)) {
    throw new Error(
      "metadata.systemDefaults.repoSlug must be an owner/name repository",
    );
  }
  for (const key of ["version", "description", "license"]) {
    if (typeof product[key] !== "string" || product[key].length === 0) {
      throw new Error(
        `metadata.product.${key} must be a nonempty string to sync package.json`,
      );
    }
  }
  if (!/^\d+\.\d+\.\d+$/.test(product.version)) throw new Error("metadata.product.version must be X.Y.Z");
  for (const key of ["keywords", "fundingUrls"]) {
    if (!Array.isArray(product[key]) || product[key].length === 0 ||
      product[key].some((value) => typeof value !== "string" || value.length === 0 ||
        (key === "fundingUrls" && !value.startsWith("https://")))) {
      throw new Error(`metadata.product.${key} must be a nonempty array of ${key === "fundingUrls" ? "HTTPS URLs" : "strings"}`);
    }
  }
  return {
    version: product.version,
    description: product.description,
    license: product.license,
    homepage: `https://github.com/${repoSlug}#readme`,
    repository: { type: "git", url: `git+https://github.com/${repoSlug}.git` },
    bugs: { url: `https://github.com/${repoSlug}/issues` },
    funding: product.fundingUrls.map((url) => ({ type: "github", url })),
    keywords: [...product.keywords],
  };
}

function diffFields(pkg, desired) {
  const drift = [];
  for (const [key, value] of Object.entries(desired)) {
    if (JSON.stringify(pkg[key]) !== JSON.stringify(value)) {
      drift.push({ key, current: pkg[key], desired: value });
    }
  }
  return drift;
}

function applyFields(pkg, desired) {
  for (const [key, value] of Object.entries(desired)) {
    pkg[key] = value;
  }
  return pkg;
}

export function checkPackageMetadata(
  packageFile = defaultPackageFile,
  metadataFile = undefined,
) {
  const pkg = JSON.parse(readFileSync(packageFile, "utf8"));
  const metadata = readProjectMetadata(metadataFile);
  const desired = desiredPackageFields(metadata);
  return diffFields(pkg, desired);
}

export function writePackageMetadata(
  packageFile = defaultPackageFile,
  metadataFile = undefined,
) {
  const pkg = JSON.parse(readFileSync(packageFile, "utf8"));
  const metadata = readProjectMetadata(metadataFile);
  const desired = desiredPackageFields(metadata);
  applyFields(pkg, desired);
  writeFileSync(packageFile, `${JSON.stringify(pkg, null, 2)}\n`);
  return desired;
}

function fail(message, code = 1) {
  console.error(message);
  process.exit(code);
}

function main() {
  const { values } = parseArgs({
    options: {
      check: { type: "boolean" }, write: { type: "boolean" },
      help: { type: "boolean", short: "h" },
    },
  });
  if (values.help) {
    console.log(
      `Sync package.json metadata from src/config/metadata.json (the source of truth)\n\nUsage:\n  node src/sync-package-metadata.mjs --check   Fail if package.json is out of sync\n  node src/sync-package-metadata.mjs --write   Update package.json to match metadata.json\n`,
    );
    return;
  }
  if (values.check && values.write) fail("--check and --write are mutually exclusive.", 2);
  if (!existsSync(defaultPackageFile)) {
    fail(`Missing package.json: ${defaultPackageFile}`);
  }

  if (values.write) {
    const desired = writePackageMetadata();
    console.log(
      `Synced package.json from src/config/metadata.json (version ${desired.version}).`,
    );
    return;
  }

  if (!values.write) {
    const drift = checkPackageMetadata();
    if (drift.length > 0) {
      console.error(
        "package.json is out of sync with src/config/metadata.json:",
      );
      for (const { key, current, desired } of drift) {
        console.error(`  ${key}:`);
        console.error(`    current: ${JSON.stringify(current)}`);
        console.error(`    desired: ${JSON.stringify(desired)}`);
      }
      console.error("Run: node src/sync-package-metadata.mjs --write");
      process.exit(1);
    }
    console.log("package.json matches src/config/metadata.json.");
    return;
  }

  fail("Unsupported option. Run with --help for usage.", 2);
}

if (isMainModule(import.meta.url)) {
  try {
    main();
  } catch (error) {
    fail(error.message, error.code?.startsWith("ERR_PARSE_ARGS") ? 2 : 1);
  }
}
