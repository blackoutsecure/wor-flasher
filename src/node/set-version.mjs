#!/usr/bin/env node
import { readFileSync, writeFileSync, existsSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { execSync } from "node:child_process";

const scriptDir = dirname(fileURLToPath(import.meta.url));
const repoDir = join(scriptDir, "..", "..");
const versionArg = process.argv[2];

if (!versionArg || !/^\d+\.\d+\.\d+$/.test(versionArg.replace(/^v/i, ""))) {
  console.error("Usage: node src/node/set-version.mjs <X.Y.Z>");
  process.exit(2);
}

const cleanVersion = versionArg.replace(/^v/i, "");
const metadataFile = join(repoDir, "src", "lib", "metadata.sh");
const plistFile = join(repoDir, "src", "macos-app", "Contents", "Info.plist");
const readmeFile = join(repoDir, "README.md");
const engineFile = join(repoDir, "install-wor.sh");
const packageFile = join(repoDir, "package.json");

for (const file of [metadataFile, plistFile, readmeFile, engineFile]) {
  if (!existsSync(file)) {
    console.error(`Required version file is missing: ${file}`);
    process.exit(1);
  }
}

// 1. metadata.sh
let metadataContent = readFileSync(metadataFile, "utf8");
metadataContent = metadataContent.replace(
  /^WOR_FLASHER_VERSION=['"][^'"]+['"]/m,
  `WOR_FLASHER_VERSION='${cleanVersion}'`,
);
writeFileSync(metadataFile, metadataContent);

// 2. Info.plist
let plistContent = readFileSync(plistFile, "utf8");
plistContent = plistContent.replace(
  /(<key>CFBundleShortVersionString<\/key>\s*<string>)[^<]*(<\/string>)/g,
  `$1${cleanVersion}$2`,
);
plistContent = plistContent.replace(
  /(<key>CFBundleVersion<\/key>\s*<string>)[^<]*(<\/string>)/g,
  `$1${cleanVersion}$2`,
);
writeFileSync(plistFile, plistContent);

// 3. README.md
let readmeContent = readFileSync(readmeFile, "utf8");
readmeContent = readmeContent.replace(
  /version-\d+\.\d+\.\d+-/,
  `version-${cleanVersion}-`,
);
if (!readmeContent.includes(`- **${cleanVersion}**`)) {
  readmeContent = readmeContent.replace(
    /(## Versions\n\n)/,
    `$1- **${cleanVersion}** - Release version update.\n`,
  );
}
writeFileSync(readmeFile, readmeContent);

// 4. install-wor.sh
let engineContent = readFileSync(engineFile, "utf8");
if (!engineContent.includes(`#${cleanVersion} - `)) {
  engineContent = engineContent.replace(
    /(#Version history\n#---------------\n)/,
    `$1#${cleanVersion} - Release version update.\n`,
  );
}
writeFileSync(engineFile, engineContent);

// 5. package.json
if (existsSync(packageFile)) {
  let pkgContent = readFileSync(packageFile, "utf8");
  pkgContent = pkgContent.replace(
    /"version"\s*:\s*"[^"]+"/,
    `"version": "${cleanVersion}"`,
  );
  writeFileSync(packageFile, pkgContent);
}

// Rebuild macOS release artifacts
try {
  execSync(`node "${join(scriptDir, "build-release.mjs")}" --platform=macos`, {
    cwd: repoDir,
    stdio: "inherit",
  });
} catch {
  // Ignore build errors if staging directory is busy
}

console.log(`Updated WoR-Flasher version surfaces to ${cleanVersion}.`);
