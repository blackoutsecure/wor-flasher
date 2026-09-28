import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { chmodSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { generateRuntimeManifest, readProjectMetadata, readRuntimePaths } from "./lib/node-runtime.mjs";

const scriptDir = dirname(fileURLToPath(import.meta.url));
const repoDir = join(scriptDir, "..");
const sha256 = (bytes) => createHash("sha256").update(bytes).digest("hex");

export function packageStandalone(outputFile, rootSource = repoDir) {
  const metadataPath = join(rootSource, "src/config/metadata.json");
  const version = readProjectMetadata(metadataPath).product?.version;
  if (!/^\d+\.\d+\.\d+$/.test(version || "")) throw new Error("Invalid standalone release version.");
  const workspace = mkdtempSync(join(tmpdir(), "wor-standalone-package-"));
  try {
    const manifest = generateRuntimeManifest(
      join(workspace, "runtime"), version, readRuntimePaths(metadataPath), rootSource,
    );
    if (manifest.files.some((entry) => /[\r\n\\]/.test(entry.path))) {
      throw new Error("Standalone runtime paths cannot contain newlines or backslashes.");
    }
    const checksums = manifest.files.map((entry) => `${entry.sha256}  runtime/${entry.path}`).join("\n") + "\n";
    writeFileSync(join(workspace, "runtime.sha256"), checksums);
    const archive = join(workspace, "payload.tar.gz");
    const packed = spawnSync("tar", [
      "--format=ustar", "-czf", archive, "-C", workspace, "runtime", "runtime.sha256",
    ], {
      encoding: "utf8",
      env: { ...process.env, COPYFILE_DISABLE: "1", COPY_EXTENDED_ATTRIBUTES_DISABLE: "1" },
    });
    if (packed.error || packed.status !== 0) {
      throw new Error(`Could not package standalone runtime: ${packed.error?.message || packed.stderr}`);
    }
    const payload = readFileSync(archive);
    const substitutions = {
      WOR_VERSION: version,
      WOR_PAYLOAD_SHA256: sha256(payload),
      WOR_MANIFEST_SHA256: sha256(checksums),
      WOR_RUNTIME_FILE_COUNT: String(manifest.files.length),
      WOR_PAYLOAD_BASE64: payload.toString("base64").match(/.{1,76}/g).join("\n"),
    };
    let launcher = readFileSync(join(scriptDir, "standalone-launcher.sh"), "utf8");
    for (const [name, value] of Object.entries(substitutions)) {
      const marker = `@${name}@`;
      if (launcher.split(marker).length !== 2) throw new Error(`Expected one standalone template marker: ${marker}`);
      launcher = launcher.replace(marker, () => value);
    }
    mkdirSync(dirname(outputFile), { recursive: true });
    writeFileSync(outputFile, launcher);
    chmodSync(outputFile, 0o755);
  } finally {
    rmSync(workspace, { recursive: true, force: true });
  }
}
