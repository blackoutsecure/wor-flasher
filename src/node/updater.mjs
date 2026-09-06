#!/usr/bin/env node
import { execSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  chmodSync,
  copyFileSync,
  existsSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { dirname, join, relative } from "node:path";
import { fileURLToPath } from "node:url";

const scriptDir = dirname(fileURLToPath(import.meta.url));
const repoDir = join(scriptDir, "..", "..");

export function parseVersion(versionStr) {
  if (!versionStr || typeof versionStr !== "string") return [0, 0, 0];
  const cleaned = versionStr.replace(/^v/i, "").trim();
  const parts = cleaned.split(".").map((p) => parseInt(p, 10) || 0);
  while (parts.length < 3) parts.push(0);
  return parts.slice(0, 3);
}

export function compareVersions(v1, v2) {
  const p1 = parseVersion(v1);
  const p2 = parseVersion(v2);
  for (let i = 0; i < 3; i++) {
    if (p1[i] > p2[i]) return 1;
    if (p1[i] < p2[i]) return -1;
  }
  return 0;
}

export function readRuntimePaths(
  pathsFile = join(scriptDir, "runtime-paths.json"),
) {
  if (!existsSync(pathsFile)) return [];
  try {
    return JSON.parse(readFileSync(pathsFile, "utf8"));
  } catch {
    return [];
  }
}

export function copyTree(source, target) {
  const info = statSync(source);
  if (info.isDirectory()) {
    mkdirSync(target, { recursive: true });
    for (const name of readdirSync(source)) {
      copyTree(join(source, name), join(target, name));
    }
    return;
  }
  mkdirSync(dirname(target), { recursive: true });
  copyFileSync(source, target);
  chmodSync(target, info.mode);
}

export function generateRuntimeManifest(
  stageRoot,
  version,
  runtimePaths = readRuntimePaths(),
  rootSource = repoDir,
) {
  const files = [];

  for (const relPath of runtimePaths) {
    const srcPath = join(rootSource, relPath);
    if (!existsSync(srcPath)) {
      throw new Error(`Missing required runtime path: ${relPath}`);
    }
    const destPath = join(stageRoot, relPath);
    if (srcPath !== destPath && !existsSync(destPath)) {
      copyTree(srcPath, destPath);
    }
  }

  function processEntry(currentPath) {
    const info = statSync(currentPath);
    if (info.isDirectory()) {
      for (const name of readdirSync(currentPath)) {
        processEntry(join(currentPath, name));
      }
    } else if (info.isFile()) {
      const rel = relative(stageRoot, currentPath).replace(/\\/g, "/");
      const sha256 = createHash("sha256")
        .update(readFileSync(currentPath))
        .digest("hex");
      const mode = (info.mode & 0o777).toString(8);
      files.push({ path: rel, sha256, mode });
    }
  }

  for (const relPath of runtimePaths) {
    processEntry(join(stageRoot, relPath));
  }

  files.sort((a, b) => a.path.localeCompare(b.path));
  return { schemaVersion: 1, version, files };
}

export function verifyRuntimeManifest(appRoot) {
  const runtimeDir = join(appRoot, "Contents", "Resources", "runtime");
  const manifestFile = join(
    appRoot,
    "Contents",
    "Resources",
    "runtime-manifest.json",
  );

  if (!existsSync(runtimeDir) || !existsSync(manifestFile)) return false;

  try {
    const manifest = JSON.parse(readFileSync(manifestFile, "utf8"));
    if (!manifest || !Array.isArray(manifest.files)) return false;

    for (const entry of manifest.files) {
      const filePath = join(runtimeDir, entry.path);
      if (!existsSync(filePath)) return false;
      const info = statSync(filePath);
      if (!info.isFile()) return false;
      const sha256 = createHash("sha256")
        .update(readFileSync(filePath))
        .digest("hex");
      if (sha256 !== entry.sha256) return false;
      const mode = (info.mode & 0o777).toString(8);
      if (mode !== entry.mode) return false;
    }
    return true;
  } catch {
    return false;
  }
}

export function loadSystemConfig(repoPath = repoDir) {
  const configPath = join(repoPath, "config-templates", "config.json");
  if (!existsSync(configPath)) return {};
  try {
    return JSON.parse(readFileSync(configPath, "utf8"));
  } catch {
    return {};
  }
}

export function checkGitUpdate({
  repoPath = repoDir,
  updateUrl,
  updateRef,
} = {}) {
  const systemConfig = loadSystemConfig(repoPath)?.system || {};
  const finalUpdateUrl =
    updateUrl ||
    systemConfig.updateRepoUrl ||
    "https://github.com/blackoutsecure/wor-flasher.git";
  const finalUpdateRef = updateRef || systemConfig.updateRef || "HEAD";

  try {
    const status = execSync("git status --porcelain --untracked-files=no", {
      cwd: repoPath,
      encoding: "utf8",
    }).trim();
    if (status.length > 0) {
      return { status: "DIRTY", localCommit: null, remoteCommit: null };
    }

    const localCommit = execSync("git rev-parse HEAD", {
      cwd: repoPath,
      encoding: "utf8",
    }).trim();

    const remoteLine = execSync(
      `git ls-remote "${finalUpdateUrl}" "${finalUpdateRef}"`,
      {
        cwd: repoPath,
        encoding: "utf8",
      },
    ).trim();

    const remoteCommit = remoteLine.split(/\s+/)[0];
    if (!remoteCommit) {
      return { status: "UNKNOWN", localCommit, remoteCommit: null };
    }

    if (localCommit === remoteCommit) {
      return { status: "CURRENT", localCommit, remoteCommit };
    }

    return { status: "UPDATE_AVAILABLE", localCommit, remoteCommit };
  } catch {
    return { status: "ERROR", localCommit: null, remoteCommit: null };
  }
}

export async function checkReleaseUpdate({
  currentVersion = "1.0.2",
  repoSlug,
  repoPath = repoDir,
} = {}) {
  const systemConfig = loadSystemConfig(repoPath)?.system || {};
  const finalRepoSlug =
    repoSlug || systemConfig.repoSlug || "blackoutsecure/wor-flasher";

  try {
    const url = `https://api.github.com/repos/${finalRepoSlug}/releases/latest`;
    const response = await fetch(url, {
      headers: { "User-Agent": "WoR-Flasher-Updater" },
      signal: AbortSignal.timeout(5000),
    });
    if (!response.ok) return { updateAvailable: false, error: response.status };

    const data = await response.json();
    const latestVersion = data.tag_name || data.name || "";
    const isNewer = compareVersions(latestVersion, currentVersion) > 0;

    return {
      updateAvailable: isNewer,
      currentVersion,
      latestVersion,
      releaseNotes: data.body || "",
      htmlUrl: data.html_url || "",
    };
  } catch (err) {
    return { updateAvailable: false, error: err.message };
  }
}

async function main() {
  const args = process.argv.slice(2);
  if (args.length === 0 || args.includes("--help") || args.includes("-h")) {
    console.log(
      `WoR-Flasher Updater Tooling\n\n` +
        `Usage: node src/node/updater.mjs [options]\n\n` +
        `Options:\n` +
        `  --check-git                        Check for git remote updates\n` +
        `  --check-release                    Check for GitHub release updates\n` +
        `  --build-manifest <stage> <out> [v] Build runtime-manifest.json\n` +
        `  --verify-manifest <appRoot>         Verify embedded runtime manifest\n` +
        `  --compare-versions <v1> <v2>       Compare two version strings\n` +
        `  --repo-dir=<path>                  Repository root directory\n` +
        `  --update-url=<url>                 Remote update repository URL\n` +
        `  --update-ref=<ref>                 Remote update ref\n`,
    );
    return;
  }

  const valueFor = (prefix) => {
    const found = args.find((a) => a.startsWith(`${prefix}=`));
    return found ? found.split("=", 2)[1] : undefined;
  };

  const customRepoDir = valueFor("--repo-dir") || repoDir;
  const sysConfig = loadSystemConfig(customRepoDir)?.system || {};
  const customUrl =
    valueFor("--update-url") ||
    sysConfig.updateRepoUrl ||
    "https://github.com/blackoutsecure/wor-flasher.git";
  const customRef = valueFor("--update-ref") || sysConfig.updateRef || "HEAD";

  if (args.includes("--check-git")) {
    const res = checkGitUpdate({
      repoPath: customRepoDir,
      updateUrl: customUrl,
      updateRef: customRef,
    });
    console.log(
      `${res.status}${res.remoteCommit ? ` ${res.remoteCommit}` : ""}`,
    );
    return;
  }

  if (args.includes("--check-release")) {
    const version = valueFor("--version") || "1.0.2";
    const res = await checkReleaseUpdate({ currentVersion: version });
    console.log(JSON.stringify(res, null, 2));
    return;
  }

  const buildIdx = args.indexOf("--build-manifest");
  if (buildIdx !== -1 && args[buildIdx + 1] && args[buildIdx + 2]) {
    const stageRoot = args[buildIdx + 1];
    const manifestOut = args[buildIdx + 2];
    const ver = args[buildIdx + 3] || "1.0.2";
    const manifest = generateRuntimeManifest(
      stageRoot,
      ver,
      readRuntimePaths(),
      customRepoDir,
    );
    writeFileSync(manifestOut, JSON.stringify(manifest, null, 2) + "\n");
    console.log(`Generated manifest at ${manifestOut}`);
    return;
  }

  const verifyIdx = args.indexOf("--verify-manifest");
  if (verifyIdx !== -1 && args[verifyIdx + 1]) {
    const appRoot = args[verifyIdx + 1];
    const isValid = verifyRuntimeManifest(appRoot);
    if (!isValid) process.exit(1);
    console.log("Manifest is valid");
    return;
  }

  const compIdx = args.indexOf("--compare-versions");
  if (compIdx !== -1 && args[compIdx + 1] && args[compIdx + 2]) {
    const result = compareVersions(args[compIdx + 1], args[compIdx + 2]);
    console.log(result);
    return;
  }
}

if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) {
  main().catch((err) => {
    console.error(err);
    process.exit(1);
  });
}
