import { createHash } from "node:crypto";
import {
  chmodSync,
  copyFileSync,
  existsSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  statSync,
} from "node:fs";
import { dirname, isAbsolute, join, relative } from "node:path";
import { fileURLToPath } from "node:url";

const scriptDir = dirname(fileURLToPath(import.meta.url));
const repoDir = join(scriptDir, "..", "..");
const defaultMetadataFile = join(repoDir, "src", "config", "metadata.json");

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

export function readProjectMetadata(metadataFile = defaultMetadataFile) {
  if (!existsSync(metadataFile)) {
    throw new Error(`Missing project metadata: ${metadataFile}`);
  }
  return JSON.parse(readFileSync(metadataFile, "utf8"));
}

export function readRuntimePaths(metadataFile = defaultMetadataFile) {
  const metadata = readProjectMetadata(metadataFile);
  const runtimePaths = metadata.runtimePaths;
  if (!Array.isArray(runtimePaths) || runtimePaths.length === 0) {
    throw new Error("src/config/metadata.json must define runtimePaths");
  }
  for (const relPath of runtimePaths) {
    if (
      typeof relPath !== "string" ||
      relPath.length === 0 ||
      isAbsolute(relPath) ||
      relPath.split(/[\\/]/).includes("..")
    ) {
      throw new Error(`Invalid runtime path in package.json: ${relPath}`);
    }
  }
  return runtimePaths;
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

export function formatRuntimeManifest(manifest) {
  const files = manifest.files
    .map(
      (entry) =>
        `    {"path": ${JSON.stringify(entry.path)}, "sha256": "${entry.sha256}", "mode": "${entry.mode}"}`,
    )
    .join(",\n");
  return `{
  "schemaVersion": ${manifest.schemaVersion},
  "version": ${JSON.stringify(manifest.version)},
  "files": [
${files}
  ]
}
`;
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
  const metadataPath = join(repoPath, "src", "config", "metadata.json");
  const configPath = join(repoPath, "config-templates", "config.json");
  let system = {};
  if (existsSync(metadataPath)) {
    try {
      const defaults = readProjectMetadata(metadataPath).systemDefaults || {};
      system = {
        peInstallerUrl: defaults.peInstallerUrl,
        peInstallerSha256: defaults.peInstallerSha256,
        uefiVerPi3: defaults.uefiVerPi3,
        uefiVerPi4: defaults.uefiVerPi4,
        uefiVerPi5: defaults.uefiVerPi5,
        driverVer: defaults.driverVer,
        armv80MaxBuild: defaults.armv80MaxBuild,
        win11MinBuild: defaults.win11MinBuild,
        win10OldestBuild: defaults.win10OldestBuild,
        exampleBid: defaults.exampleBid,
        armv80SafeBid: defaults.armv80SafeBid,
        repoSlug: defaults.repoSlug,
      };
      system = Object.fromEntries(
        Object.entries(system).filter(([, value]) => value !== undefined),
      );
    } catch {
      system = {};
    }
  }
  if (!existsSync(configPath)) return { system };
  try {
    const config = JSON.parse(readFileSync(configPath, "utf8"));
    return { ...config, system: { ...system, ...(config.system || {}) } };
  } catch {
    return { system };
  }
}

//The slug is interpolated into the update URL, so it must match the exact owner/name shape
//GitHub allows. Anything looser could point the check at an attacker-chosen host.
export function isSafeRepoSlug(slug) {
  return (
    typeof slug === "string" &&
    /^[A-Za-z0-9._-]+\/[A-Za-z0-9._-]+$/.test(slug) &&
    !slug.split("/").includes("..")
  );
}

//Read-only release check: it never writes, never executes anything, and never mutates the
//installation. The caller is told a newer release exists and decides what to do about it.
export async function checkReleaseUpdate({
  currentVersion,
  repoSlug,
  repoPath = repoDir,
  timeoutMs = 5000,
  fetchImpl = fetch,
} = {}) {
  const systemConfig = loadSystemConfig(repoPath)?.system || {};
  const finalRepoSlug = repoSlug || systemConfig.repoSlug;
  const version =
    currentVersion || readProjectMetadata().product?.version || "0.0.0";

  if (!isSafeRepoSlug(finalRepoSlug)) {
    return { updateAvailable: false, error: "unsafe-repo-slug" };
  }

  try {
    const response = await fetchImpl(
      `https://api.github.com/repos/${finalRepoSlug}/releases/latest`,
      {
        headers: {
          "User-Agent": "WoR-Flasher-Updater",
          Accept: "application/vnd.github+json",
        },
        redirect: "error",
        signal: AbortSignal.timeout(timeoutMs),
      },
    );
    if (!response.ok) return { updateAvailable: false, error: response.status };

    const data = await response.json();
    if (data.draft || data.prerelease) {
      return { updateAvailable: false, currentVersion: version };
    }

    const latestVersion = data.tag_name || data.name || "";
    const htmlUrl = data.html_url || "";

    return {
      updateAvailable: compareVersions(latestVersion, version) > 0,
      currentVersion: version,
      latestVersion,
      htmlUrl: htmlUrl.startsWith("https://") ? htmlUrl : "",
    };
  } catch (err) {
    return { updateAvailable: false, error: err.message };
  }
}
