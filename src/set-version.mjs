#!/usr/bin/env node
import { readFileSync, writeFileSync, existsSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { execSync } from "node:child_process";
import { writePackageMetadata } from "./sync-package-metadata.mjs";

const scriptDir = dirname(fileURLToPath(import.meta.url));
const repoDir = join(scriptDir, "..");
const versionArg = process.argv[2];

if (!versionArg || !/^\d+\.\d+\.\d+$/.test(versionArg.replace(/^v/i, ""))) {
  console.error("Usage: node src/set-version.mjs <X.Y.Z>");
  process.exit(2);
}

const cleanVersion = versionArg.replace(/^v/i, "");
const metadataFile = join(repoDir, "src", "config", "metadata.json");
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

//String.replace is a silent no-op when the pattern stops matching, which would bump some files and
//quietly skip others, shipping a release whose parts disagree about their own version
function replaceOrFail(content, pattern, replacement, label) {
  //test the pattern rather than diffing the result: re-setting the current version changes nothing
  //and is legal, but a pattern that no longer matches must not pass silently
  if (
    !new RegExp(pattern.source, pattern.flags.replace("g", "")).test(content)
  ) {
    console.error(
      `Could not update the version in ${label}; its format has changed.`,
    );
    process.exit(1);
  }
  return content.replace(pattern, replacement);
}

// 1. project metadata
let metadataContent = readFileSync(metadataFile, "utf8");
metadataContent = replaceOrFail(
  metadataContent,
  /("version"\s*:\s*")[^"]+("\s*,)/,
  `$1${cleanVersion}$2`,
  metadataFile,
);
writeFileSync(metadataFile, metadataContent);

// 2. Info.plist
let plistContent = readFileSync(plistFile, "utf8");
plistContent = replaceOrFail(
  plistContent,
  /(<key>CFBundleShortVersionString<\/key>\s*<string>)[^<]*(<\/string>)/g,
  `$1${cleanVersion}$2`,
  `${plistFile} (CFBundleShortVersionString)`,
);
plistContent = replaceOrFail(
  plistContent,
  /(<key>CFBundleVersion<\/key>\s*<string>)[^<]*(<\/string>)/g,
  `$1${cleanVersion}$2`,
  `${plistFile} (CFBundleVersion)`,
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

// 5. package.json - description, license, homepage, repository, bugs, funding, and keywords
//    are all derived from src/config/metadata.json (already bumped in step 1), not hand-patched here
if (existsSync(packageFile)) {
  writePackageMetadata(packageFile);
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
