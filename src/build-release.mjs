#!/usr/bin/env node
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  existsSync,
  mkdtempSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, relative } from "node:path";
import { fileURLToPath } from "node:url";
import {
  copyTree,
  formatRuntimeManifest,
  generateRuntimeManifest,
  isSafeRepoSlug,
  readProjectMetadata,
  readRuntimePaths,
} from "./lib/node-runtime.mjs";
import { checkPackageMetadata } from "./sync-package-metadata.mjs";
import { packageStandalone } from "./package-standalone.mjs";
import { packageMacosDmg } from "./package-macos-dmg.mjs";
import { verifyPi3BootRefresh } from "./build-pi3-boot-refresh.mjs";

const scriptDir = dirname(fileURLToPath(import.meta.url));
const repoDir = join(scriptDir, "..");
const releaseDir = join(repoDir, "release");
const macosAppTemplate = join(repoDir, "src", "macos-app");
const runtimePaths = Object.freeze(readRuntimePaths());

const platformPlans = Object.freeze({
  macos: {
    directory: "macos",
    description: "macOS app bundle and disk image",
    build: buildMacos,
  },
  linux: {
    directory: "linux",
    description: "Linux shell distribution",
    build: buildLinux,
  },
  standalone: {
    directory: "standalone",
    description: "Standalone Linux and macOS shell client",
    build: buildStandalone,
  },
  windows: {
    directory: "windows",
    description: "Windows placeholder",
    build: buildWindowsPlaceholder,
  },
});

const args = process.argv.slice(2);
const checkOnly = args.includes("--check");
const cleanOnly = args.includes("--clean");
const platform = valueFor("--platform") ?? "all";
let checkDir;
let standaloneClient;

function valueFor(name) {
  const arg = args.find((item) => item.startsWith(`${name}=`));
  return arg ? arg.split("=", 2)[1] : undefined;
}

function fail(message) {
  throw new Error(message);
}

function run(command, commandArgs, env = {}) {
  const result = spawnSync(command, commandArgs, {
    cwd: repoDir,
    env: { ...process.env, ...env },
    stdio: "inherit",
  });
  if (result.status !== 0) fail(`${command} ${commandArgs.join(" ")} failed`);
}

function assertExists(path, message) {
  if (!existsSync(path)) fail(message);
}

function walk(root) {
  const entries = [];
  for (const name of readdirSync(root)) {
    const path = join(root, name);
    const info = statSync(path);
    if (info.isDirectory()) entries.push(...walk(path));
    else if (info.isFile()) entries.push(path);
  }
  return entries.sort();
}

function sha256(path) {
  return createHash("sha256").update(readFileSync(path)).digest("hex");
}

function writeChecksums(root) {
  const lines = walk(root)
    .filter((file) => relative(root, file) !== "SHA256SUMS")
    .map((file) => `${sha256(file)}  ${relative(root, file)}`);
  writeFileSync(join(root, "SHA256SUMS"), `${lines.join("\n")}\n`);
}

function writeReadme(root, lines) {
  writeFileSync(join(root, "README.txt"), `${lines.join("\n")}\n`);
}

function stageStandalone(target) {
  if (standaloneClient) copyTree(standaloneClient, target);
  else {
    packageStandalone(target);
    standaloneClient = target;
  }
}

function createLinuxArchive(root, version, flavor, entrypoint) {
  const archive = join(root, `wor-flasher-${version}-linux-${flavor}.tar.gz`);
  const files = [entrypoint, "README.txt", "LICENSE", "NOTICE"];
  run("tar", ["-czf", archive, "-C", root, ...files], {
    COPYFILE_DISABLE: "1", COPY_EXTENDED_ATTRIBUTES_DISABLE: "1",
  });
  const listing = spawnSync("tar", ["-tzf", archive], {
    cwd: repoDir,
    encoding: "utf8",
  });
  if (listing.status !== 0) fail(`Unable to verify ${archive}`);
  const actual = listing.stdout.trim().split("\n").sort();
  if (JSON.stringify(actual) !== JSON.stringify(files.sort())) fail(`Unexpected files in ${archive}`);
}

function selectedPlatforms() {
  if (platform === "all") return ["macos", "linux", "standalone", "windows"];
  if (!Object.hasOwn(platformPlans, platform)) {
    fail(
      `Unsupported platform '${platform}'. Supported values: all, macos, linux, standalone, windows.`,
    );
  }
  return [platform];
}

function preparePlatformDir(name) {
  const base = checkDir ?? releaseDir;
  const root = join(base, platformPlans[name].directory);
  rmSync(root, { recursive: true, force: true });
  mkdirSync(root, { recursive: true });
  return root;
}

function verifyShellRuntime() {
  verifyPi3BootRefresh();
  for (const runtimePath of runtimePaths)
    assertExists(join(repoDir, runtimePath), `Missing ${runtimePath}`);

  const requiredTemplates = [
    "config.schema.json",
    "config.json",
    "pi3.config.txt",
    "pi4.config.txt",
    "pi5.config.txt",
    "pi4-ram-unlock.ps1",
    "pi4-ram-unlock-specialize.xml",
    "oobe-network-bypass.xml",
    "prefinalize.cmd",
  ];

  for (const templateFile of requiredTemplates) {
    const fullPath = join(repoDir, "config-templates", templateFile);
    assertExists(fullPath, `Missing config-templates/${templateFile}`);
    if (statSync(fullPath).size === 0) {
      fail(`Template config-templates/${templateFile} is empty`);
    }
  }

  const schemaPath = join(repoDir, "config-templates", "config.schema.json");
  const configJsonPath = join(repoDir, "config-templates", "config.json");
  try {
    JSON.parse(readFileSync(schemaPath, "utf8"));
    JSON.parse(readFileSync(configJsonPath, "utf8"));
  } catch (err) {
    fail(`Invalid JSON in configuration templates: ${err.message}`);
  }

  //src/config/metadata.json is the source of truth for package.json's description, license,
  //homepage, repository, bugs, funding, and keywords; a release must not ship them stale.
  const packageDrift = checkPackageMetadata();
  if (packageDrift.length > 0) {
    fail(
      `package.json is out of sync with src/config/metadata.json (${packageDrift.map((d) => d.key).join(", ")}); run: node src/sync-package-metadata.mjs --write`,
    );
  }
}

function buildMacos(root) {
  verifyShellRuntime();
  assertExists(macosAppTemplate, "src/macos-app is missing");
  assertExists(
    join(macosAppTemplate, "Contents", "Info.plist"),
    "src/macos-app/Contents/Info.plist is missing",
  );
  assertExists(
    join(macosAppTemplate, "Contents", "MacOS", "WoR-Flasher"),
    "src/macos-app/Contents/MacOS/WoR-Flasher is missing",
  );
  assertExists(
    join(macosAppTemplate, "Contents", "Resources", "WoR-Flasher.icns"),
    "src/macos-app/Contents/Resources/WoR-Flasher.icns is missing",
  );
  const metadata = readProjectMetadata();
  const assetsDirname = metadata.product?.assetsDirname;
  const logoFilename = metadata.product?.logoFilename;
  if (!assetsDirname || !logoFilename) {
    fail(
      "product.assetsDirname or product.logoFilename is missing from src/config/metadata.json",
    );
  }
  const logoSource = join(repoDir, assetsDirname, logoFilename);
  assertExists(logoSource, `Missing ${assetsDirname}/${logoFilename}`);
  const appTarget = join(root, "WoR-Flasher.app");
  copyTree(macosAppTemplate, appTarget);
  // The app bundle template does not check in its own logo copy; it is always
  // staged from the single canonical source in assets/ to prevent drift.
  copyTree(logoSource, join(appTarget, "Contents", "Resources", logoFilename));
  const pkg = JSON.parse(readFileSync(join(repoDir, "package.json"), "utf8"));
  const runtimeTarget = join(appTarget, "Contents", "Resources", "runtime");
  const manifestTarget = join(
    appTarget,
    "Contents",
    "Resources",
    "runtime-manifest.json",
  );
  const manifest = generateRuntimeManifest(
    runtimeTarget,
    pkg.version,
    runtimePaths,
    repoDir,
  );
  writeFileSync(manifestTarget, formatRuntimeManifest(manifest));
  if (process.platform === "darwin") {
    packageMacosDmg(appTarget, join(root, `WoR-Flasher-${pkg.version}-macos.dmg`), pkg.version);
  } else {
    console.log("macOS app staged; DMG creation requires macOS and runs on the macOS release publisher.");
  }
  writeReadme(root, [
    "WoR-Flasher macOS app and DMG release artifacts",
    "",
    "This directory is regenerated by npm run build.",
    "Publish WoR-Flasher-<version>-macos.dmg; it contains the complete WoR-Flasher.app.",
    "The unpacked WoR-Flasher.app remains here for local use.",
    "Creating the DMG requires macOS; other hosts stage and validate the app only.",
    "Copy the app out of the DMG to a writable folder such as ~/Applications before launching.",
    "The app is unsigned and unnotarized; review signing/notarization before distribution.",
    "Disk flashing logic remains in install-wor.sh; Node only stages release artifacts.",
    "",
    "Verify SHA256SUMS before publishing.",
  ]);
  writeChecksums(root);
}

function buildLinux(root) {
  verifyShellRuntime();
  const metadata = readProjectMetadata();
  const version = metadata.product.version;
  const repository = metadata.systemDefaults.repoSlug;
  if (!isSafeRepoSlug(repository)) fail("Invalid repository for the Linux GUI bootstrap.");
  const client = join(root, "install-wor.sh");
  stageStandalone(client);
  let gui = readFileSync(join(repoDir, "install-wor-gui.sh"), "utf8");
  for (const [name, value] of [
    ["WOR_GUI_BOOTSTRAP_BASE_URL", `https://github.com/${repository}/releases/download/v${version}`],
    ["WOR_GUI_BOOTSTRAP_SHA256", sha256(client)],
  ]) {
    const pattern = new RegExp(`^${name}='[^']*'$`, "gm");
    if ((gui.match(pattern) || []).length !== 1) fail(`Missing Linux GUI release marker: ${name}`);
    gui = gui.replace(pattern, () => `${name}='${value}'`);
  }
  writeFileSync(join(root, "install-wor-gui.sh"), gui, { mode: 0o755 });
  for (const notice of ["LICENSE", "NOTICE"]) copyTree(join(repoDir, notice), join(root, notice));
  writeReadme(root, [
    `WoR-Flasher ${version} Linux clients`,
    "",
    "Verify your archive against the published SHA256SUMS before extracting.",
    "Extract into a new directory. Files are at the archive root, without a wrapper folder.",
    "Extract both archives into the same directory to put the GUI and CLI next to each other.",
    "CLI: bash install-wor.sh --help (runtime is bundled; Git and Node.js are not needed).",
    "GUI: bash install-wor-gui.sh (uses a complete nearby runtime or a nearby standalone install-wor.sh).",
    "Without a local runtime, the GUI downloads the matching checksum-verified release client over HTTPS.",
    "That download supplies the engine, libraries, templates, and artwork in one consistent version.",
    "The GUI needs curl or wget only for a missing download, plus shasum or sha256sum to verify it.",
    "Verified downloads are cached privately; existing scripts and config.json are not overwritten.",
    "Use --config with the CLI or WOR_CONFIG_FILE for the GUI to select a configuration file.",
    "Normal flashing dependencies and drive-safety confirmations still apply.",
  ]);
  createLinuxArchive(root, version, "cli", "install-wor.sh");
  createLinuxArchive(root, version, "gui", "install-wor-gui.sh");
  writeChecksums(root);
}

function buildWindowsPlaceholder(root) {
  writeReadme(root, [
    "WoR-Flasher Windows release placeholder",
    "",
    "Windows packaging is intentionally not implemented yet.",
    "A Windows UI must get its own reviewed device-safety, elevation and removable-media design before this project ships one.",
    "Until then, Windows users should use the official Windows on Raspberry Imager.",
  ]);
  writeChecksums(root);
}

function buildStandalone(root) {
  verifyShellRuntime();
  const launcher = join(root, "install-wor.sh");
  stageStandalone(launcher);
  run("bash", ["-n", launcher]);
  writeReadme(root, [
    "WoR-Flasher standalone Linux and macOS client",
    "",
    "Verify install-wor.sh against SHA256SUMS, then run: bash install-wor.sh --help",
    "This one file embeds the canonical runtime, configuration templates, artwork, README, LICENSE, and NOTICE.",
    "No Git checkout or Node.js is needed to unpack it. Normal flashing dependencies are still required.",
    "The verified runtime is cached under ${XDG_CACHE_HOME:-$HOME/.cache}/wor-flasher/standalone.",
    "Use --config with a file in your working directory, or the usual environment variables.",
    "The standalone launcher never replaces itself and keeps existing disk safety checks.",
  ]);
  writeChecksums(root);
}

try {
  if (args.includes("--help") || args.includes("-h")) {
    console.log(
      `WoR-Flasher Release Builder\n\nUsage: node src/build-release.mjs [options]\n\nOptions:\n  --platform=all|macos|linux|standalone|windows  Target platform (default: all)\n  --check                                      Validate release packaging without writing artifacts\n  --clean                                      Remove release/ staging directory\n  --help, -h                                   Show this help message\n`,
    );
    process.exit(0);
  }

  const unknownArg = args.find(
    (arg) =>
      arg !== "--check" && arg !== "--clean" && !arg.startsWith("--platform="),
  );
  if (unknownArg) fail(`Unsupported option '${unknownArg}'.`);
  if (checkOnly && cleanOnly)
    fail("--check and --clean cannot be used together.");

  if (cleanOnly) {
    rmSync(releaseDir, { recursive: true, force: true });
    console.log("Removed release");
  } else {
    checkDir = checkOnly
      ? mkdtempSync(join(tmpdir(), "wor-flasher-release-check-"))
      : undefined;
    for (const name of selectedPlatforms()) {
      const root = preparePlatformDir(name);
      platformPlans[name].build(root);
    }

    if (checkOnly) console.log("Release packaging plan is valid");
    else
      console.log(
        `Release artifacts staged in ${relative(repoDir, releaseDir)}`,
      );
  }
} catch (error) {
  console.error(error instanceof Error ? error.message : String(error));
  process.exitCode = 1;
} finally {
  if (checkDir) rmSync(checkDir, { recursive: true, force: true });
}
