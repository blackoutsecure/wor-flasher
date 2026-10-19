#!/usr/bin/env node
import { writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";
import {
  checkReleaseUpdate,
  compareVersions,
  formatRuntimeManifest,
  generateRuntimeManifest,
  isMainModule,
  readProjectMetadata,
  readRuntimePaths,
  verifyRuntimeManifest,
} from "./lib/node-runtime.mjs";

const scriptDir = dirname(fileURLToPath(import.meta.url));
const repoDir = join(scriptDir, "..");

async function main() {
  const args = process.argv.slice(2);
  const { values, positionals } = parseArgs({
    args,
    allowPositionals: true,
    options: {
      "check-release": { type: "boolean" },
      "build-manifest": { type: "boolean" },
      "verify-manifest": { type: "boolean" },
      "compare-versions": { type: "boolean" },
      "repo-dir": { type: "string" },
      "repo-slug": { type: "string" },
      version: { type: "string" },
      json: { type: "boolean" },
      help: { type: "boolean", short: "h" },
    },
  });
  if (args.length === 0 || values.help) {
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

  const commands = ["check-release", "build-manifest", "verify-manifest", "compare-versions"]
    .filter((name) => values[name]);
  if (commands.length !== 1) throw new Error("Exactly one updater command is required.");
  const command = commands[0];
  const counts = { "check-release": [0], "build-manifest": [2, 3], "verify-manifest": [1], "compare-versions": [2] };
  if (!counts[command].includes(positionals.length)) {
    throw new Error(`--${command} requires ${counts[command].join(" or ")} positional arguments.`);
  }
  if (command !== "check-release" && (values.json || values["repo-slug"] !== undefined || values.version !== undefined)) {
    throw new Error("--json, --repo-slug and --version require --check-release.");
  }
  const customRepoDir = values["repo-dir"] ?? repoDir;
  const metadataFile = join(customRepoDir, "src/config/metadata.json");

  if (command === "check-release") {
    const res = await checkReleaseUpdate({
      currentVersion: values.version,
      repoSlug: values["repo-slug"],
      repoPath: customRepoDir,
    });
    if (values.json) {
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

  if (command === "build-manifest") {
    const [stageRoot, manifestOut, requestedVersion] = positionals;
    const ver = requestedVersion ?? readProjectMetadata(metadataFile).product?.version;
    if (!ver) throw new Error("Could not resolve a version for the manifest");
    const manifest = generateRuntimeManifest(
      stageRoot,
      ver,
      readRuntimePaths(metadataFile),
      customRepoDir,
    );
    writeFileSync(manifestOut, formatRuntimeManifest(manifest));
    console.log(`Generated manifest at ${manifestOut}`);
    return;
  }

  if (command === "verify-manifest") {
    const [appRoot] = positionals;
    if (!verifyRuntimeManifest(appRoot)) throw new Error("Runtime manifest verification failed.");
    console.log("Manifest is valid");
    return;
  }

  if (command === "compare-versions") {
    if (positionals.some((value) => !/^v?\d+(?:\.\d+){0,2}$/i.test(value))) {
      throw new Error("--compare-versions requires numeric version arguments.");
    }
    console.log(compareVersions(...positionals));
  }
}

if (isMainModule(import.meta.url)) {
  main().catch((err) => {
    console.error(err);
    process.exit(1);
  });
}
