#!/usr/bin/env node
import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { execFileSync } from "node:child_process";
import { desiredPackageFields } from "./sync-package-metadata.mjs";

const scriptDir = dirname(fileURLToPath(import.meta.url));
const repoDir = join(scriptDir, "..");
const args = process.argv.slice(2);

if (args.length !== 1 || !/^v?\d+\.\d+\.\d+$/i.test(args[0])) {
  console.error("Usage: node src/set-version.mjs <X.Y.Z>");
  process.exit(2);
}

function replaceOrFail(content, pattern, replacement, label) {
  if (!pattern.test(content)) {
    throw new Error(`Could not update the version in ${label}; its format has changed.`);
  }
  return content.replace(pattern, replacement);
}

try {
  const version = args[0].replace(/^v/i, "");
  const metadataFile = join(repoDir, "src/config/metadata.json");
  const plistFile = join(repoDir, "src/macos-app/Contents/Info.plist");
  const readmeFile = join(repoDir, "README.md");
  const engineFile = join(repoDir, "install-wor.sh");
  const packageFile = join(repoDir, "package.json");

  //Validate every surface before writing any of them; a missing anchor must not leave a partial bump.
  const metadata = JSON.parse(readFileSync(metadataFile, "utf8"));
  if (typeof metadata.product?.version !== "string") throw new Error("metadata.product.version is missing.");
  metadata.product.version = version;
  const desired = desiredPackageFields(metadata);
  const pkg = JSON.parse(readFileSync(packageFile, "utf8"));
  if (!pkg || typeof pkg !== "object" || Array.isArray(pkg)) throw new Error("package.json must be an object.");

  let plist = readFileSync(plistFile, "utf8");
  for (const key of ["CFBundleShortVersionString", "CFBundleVersion"]) {
    plist = replaceOrFail(plist, new RegExp(`(<key>${key}</key>\\s*<string>)[^<]*(</string>)`),
      (_match, prefix, suffix) => `${prefix}${version}${suffix}`, `${plistFile} (${key})`);
  }
  let readme = replaceOrFail(readFileSync(readmeFile, "utf8"), /version-\d+\.\d+\.\d+-/,
    `version-${version}-`, `${readmeFile} (version badge)`);
  const readmeHistory = /^## Versions\n\n/m;
  if (!readmeHistory.test(readme)) throw new Error("README.md is missing its Versions heading.");
  if (!readme.includes(`- **${version}**`)) {
    readme = readme.replace(readmeHistory, (heading) => `${heading}- **${version}** - Release version update.\n`);
  }
  let engine = readFileSync(engineFile, "utf8");
  const engineHistory = /^#Version history\n/m;
  if (!engineHistory.test(engine)) throw new Error("install-wor.sh is missing its version history heading.");
  if (!engine.includes(`#${version} - `)) {
    engine = engine.replace(engineHistory, (heading) => `${heading}#${version} - Release version update.\n`);
  }

  const updates = new Map([
    [metadataFile, `${JSON.stringify(metadata, null, 2)}\n`],
    [plistFile, plist], [readmeFile, readme], [engineFile, engine],
    [packageFile, `${JSON.stringify({ ...pkg, ...desired }, null, 2)}\n`],
  ]);
  for (const [path, content] of updates) writeFileSync(path, content);

  execFileSync(process.execPath, [join(scriptDir, "build-release.mjs"), "--platform=macos"], {
    cwd: repoDir, stdio: "inherit",
  });
  console.log(`Updated WoR-Flasher version surfaces to ${version}.`);
} catch (error) {
  console.error(error instanceof Error ? error.message : String(error));
  process.exitCode = 1;
}
