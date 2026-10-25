import { createHash } from "node:crypto";
import {
  chmodSync,
  copyFileSync,
  existsSync,
  lstatSync,
  mkdirSync,
  readFileSync,
  realpathSync,
  readdirSync,
  rmSync,
} from "node:fs";
import { dirname, isAbsolute, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const scriptDir = dirname(fileURLToPath(import.meta.url));
const repoDir = join(scriptDir, "..", "..");
const defaultMetadataFile = join(repoDir, "src", "config", "metadata.json");

export function isMainModule(moduleUrl) {
  const entrypoint = process.argv[1];
  return Boolean(entrypoint && existsSync(entrypoint) &&
    realpathSync(entrypoint) === realpathSync(fileURLToPath(moduleUrl)));
}

export function readProjectMetadata(metadataFile = defaultMetadataFile) {
  if (!existsSync(metadataFile)) {
    throw new Error(`Missing project metadata: ${metadataFile}`);
  }
  const metadata = JSON.parse(readFileSync(metadataFile, "utf8"));
  if (!metadata || typeof metadata !== "object" || Array.isArray(metadata)) {
    throw new Error(`Project metadata must be a JSON object: ${metadataFile}`);
  }
  return metadata;
}

export function isSafeRepoSlug(slug) {
  return typeof slug === "string" &&
    /^[A-Za-z0-9._-]+\/[A-Za-z0-9._-]+$/.test(slug) &&
    !slug.split("/").includes("..");
}

function isSafeRuntimePath(path) {
  return typeof path === "string" && !isAbsolute(path) &&
    path.split(/[\\/]/).every((part) => part !== "" && part !== "." && part !== "..");
}

export function copyTree(source, target) {
  const info = lstatSync(source);
  if (info.isSymbolicLink() || lstatSync(target, { throwIfNoEntry: false })?.isSymbolicLink()) {
    throw new Error(`Symbolic links are not allowed in package copies: ${source}`);
  }
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
  paths,
  rootSource = repoDir,
) {
  if (!Array.isArray(paths) || paths.length === 0) {
    throw new Error("A nonempty Rust runtime inventory is required.");
  }
  const files = [];

  for (const relPath of paths) {
    if (!isSafeRuntimePath(relPath)) {
      throw new Error(`Invalid runtime path: ${relPath}`);
    }
    const srcPath = join(rootSource, relPath);
    if (!existsSync(srcPath)) {
      throw new Error(`Missing required runtime path: ${relPath}`);
    }
    const destPath = join(stageRoot, relPath);
    if (resolve(srcPath) !== resolve(destPath)) {
      let ancestor = stageRoot;
      for (const part of ["", ...relPath.split(/[\\/]/)]) {
        ancestor = join(ancestor, part);
        if (lstatSync(ancestor, { throwIfNoEntry: false })?.isSymbolicLink()) {
          throw new Error(`Symbolic link in staging path: ${ancestor}`);
        }
      }
      rmSync(destPath, { recursive: true, force: true });
      copyTree(srcPath, destPath);
    }
  }

  function addEntry(currentPath) {
    const info = lstatSync(currentPath);
    if (info.isSymbolicLink()) throw new Error(`Symbolic link in runtime: ${currentPath}`);
    if (info.isDirectory()) {
      for (const name of readdirSync(currentPath)) addEntry(join(currentPath, name));
    } else if (info.isFile()) {
      const path = relative(stageRoot, currentPath).replace(/\\/g, "/");
      const sha256 = createHash("sha256").update(readFileSync(currentPath)).digest("hex");
      const mode = (info.mode & 0o777).toString(8);
      files.push({ path, sha256, mode });
    }
  }

  for (const relPath of paths) addEntry(join(stageRoot, relPath));
  files.sort((a, b) => a.path.localeCompare(b.path));
  return { schemaVersion: 1, version, files };
}

export function formatRuntimeManifest(manifest) {
  const files = manifest.files
    .map((entry) =>
      `    {"path": ${JSON.stringify(entry.path)}, "sha256": "${entry.sha256}", "mode": "${entry.mode}"}`)
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
