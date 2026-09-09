#!/usr/bin/env node
import { writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import {
  checkReleaseUpdate,
  compareVersions,
  formatRuntimeManifest,
  generateRuntimeManifest,
  readProjectMetadata,
  readRuntimePaths,
  verifyRuntimeManifest,
} from "./lib/node-runtime.mjs";

const scriptDir = dirname(fileURLToPath(import.meta.url));
const repoDir = join(scriptDir, "..");

async function main() {
  const args = process.argv.slice(2);
  if (args.length === 0 || args.includes("--help") || args.includes("-h")) {
    console.log(
      `WoR-Flasher Updater Tooling\n\n` +
        `Usage: node src/updater.mjs [options]\n\n` +
        `Options:\n` +
        `  --check-release                    Check for a newer published release\n` +
        `  --build-manifest <stage> <out> [v] Build runtime-manifest.json\n` +
        `  --verify-manifest <appRoot>        Verify embedded runtime manifest\n` +
        `  --compare-versions <v1> <v2>       Compare two version strings\n` +
        `  --repo-dir=<path>                  Repository root directory\n` +
        `  --repo-slug=<owner/name>           Release source repository\n` +
        `  --version=<version>                Version to compare against\n` +
        `  --json                             Emit machine-readable JSON\n`,
    );
    return;
  }

  const valueFor = (prefix) => {
    const found = args.find((a) => a.startsWith(`${prefix}=`));
    return found ? found.split("=", 2)[1] : undefined;
  };

  const customRepoDir = valueFor("--repo-dir") || repoDir;
  const projectVersion = () => {
    try {
      return readProjectMetadata(
        join(customRepoDir, "src", "config", "metadata.json"),
      ).product?.version;
    } catch {
      return undefined;
    }
  };

  if (args.includes("--check-release")) {
    const res = await checkReleaseUpdate({
      currentVersion: valueFor("--version") || projectVersion(),
      repoSlug: valueFor("--repo-slug"),
      repoPath: customRepoDir,
    });
    if (args.includes("--json")) {
      console.log(JSON.stringify(res, null, 2));
      return;
    }
    //Single stable line so the shell engine can branch on it without parsing JSON.
    if (res.updateAvailable) {
      console.log(`UPDATE_AVAILABLE ${res.latestVersion} ${res.htmlUrl}`);
    } else if (res.error !== undefined) {
      console.log("UNKNOWN");
    } else {
      console.log("CURRENT");
    }
    return;
  }

  const buildIdx = args.indexOf("--build-manifest");
  if (buildIdx !== -1 && args[buildIdx + 1] && args[buildIdx + 2]) {
    const stageRoot = args[buildIdx + 1];
    const manifestOut = args[buildIdx + 2];
    const ver = args[buildIdx + 3] || projectVersion();
    if (!ver) throw new Error("Could not resolve a version for the manifest");
    const manifest = generateRuntimeManifest(
      stageRoot,
      ver,
      readRuntimePaths(),
      customRepoDir,
    );
    writeFileSync(manifestOut, formatRuntimeManifest(manifest));
    console.log(`Generated manifest at ${manifestOut}`);
    return;
  }

  const verifyIdx = args.indexOf("--verify-manifest");
  if (verifyIdx !== -1 && args[verifyIdx + 1]) {
    const appRoot = args[verifyIdx + 1];
    if (!verifyRuntimeManifest(appRoot)) process.exit(1);
    console.log("Manifest is valid");
    return;
  }

  const compIdx = args.indexOf("--compare-versions");
  if (compIdx !== -1 && args[compIdx + 1] && args[compIdx + 2]) {
    console.log(compareVersions(args[compIdx + 1], args[compIdx + 2]));
  }
}

if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) {
  main().catch((err) => {
    console.error(err);
    process.exit(1);
  });
}
