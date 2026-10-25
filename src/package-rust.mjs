#!/usr/bin/env node
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  appendFileSync, chmodSync, existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync,
  rmSync, writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import {
  copyTree, formatRuntimeManifest, generateRuntimeManifest, isMainModule, readProjectMetadata,
} from "./lib/node-runtime.mjs";
import { verifyPE } from "./build-pi3-boot-refresh.mjs";

const root = fileURLToPath(new URL("../", import.meta.url));
const args = process.argv.slice(2);
let temporary;
const macosTargets = ["aarch64-apple-darwin", "x86_64-apple-darwin"];

function run(program, arguments_, options = {}) {
  const result = spawnSync(program, arguments_, {
    cwd: root, encoding: "utf8", maxBuffer: 64 * 1024 * 1024, ...options,
  });
  if (result.error) throw result.error;
  if (result.status !== 0) {
    throw new Error(`${program} failed (${result.status ?? result.signal}): ${result.stderr || ""}`);
  }
  return result.stdout;
}

function xml(value) {
  return String(value).replaceAll("&", "&amp;").replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;").replaceAll('"', "&quot;");
}

function runtime(directory, product) {
  mkdirSync(directory, { recursive: true });
  const paths = readProjectMetadata().nativeRuntimePaths;
  if (!Array.isArray(paths) || paths.length === 0) throw new Error("Missing native runtime inventory.");
  const manifest = generateRuntimeManifest(join(directory, "runtime"), product.version, paths);
  writeFileSync(join(directory, "runtime-manifest.json"), formatRuntimeManifest(manifest));
}

export function verifyUniversalMachO(binary) {
  if (binary.length < 8 || binary.readUInt32BE(0) !== 0xcafebabe ||
      binary.readUInt32BE(4) !== 2 || binary.length < 48) {
    throw new Error("Expected a universal Mach-O with exactly two architecture slices.");
  }
  const slices = [];
  for (let index = 0; index < 2; index++) {
    const entry = 8 + index * 20;
    const cpu = binary.readUInt32BE(entry);
    const offset = binary.readUInt32BE(entry + 8);
    const size = binary.readUInt32BE(entry + 12);
    if (offset < 48 || size < 32 || offset + size > binary.length ||
        binary.readUInt32LE(offset) !== 0xfeedfacf ||
        binary.readUInt32LE(offset + 4) !== cpu) {
      throw new Error("Invalid universal Mach-O slice.");
    }
    slices.push({ cpu, offset, size });
  }
  const [first, second] = [...slices].sort((a, b) => a.offset - b.offset);
  if (first.offset + first.size > second.offset ||
      !slices.some(slice => slice.cpu === 0x01000007) ||
      !slices.some(slice => slice.cpu === 0x0100000c)) {
    throw new Error("Universal Mach-O must contain separate Intel x86_64 and Apple Silicon arm64 slices.");
  }
}

export function stageBinary(name, target, targetDirectory, universal = false) {
  if (universal) {
    const sources = macosTargets.map(triple => join(targetDirectory, triple, "release", name));
    for (const source of sources) {
      if (!existsSync(source)) throw new Error(`Missing compiled Rust executable: ${source}`);
    }
    run("/usr/bin/lipo", ["-create", ...sources, "-output", target]);
    verifyUniversalMachO(readFileSync(target));
    run("/usr/bin/codesign", ["--force", "--sign", "-", target]);
    run("/usr/bin/codesign", ["--verify", "--strict", target]);
    chmodSync(target, 0o755);
    return;
  }
  const source = join(targetDirectory, "release", name);
  if (!existsSync(source)) throw new Error(`Missing compiled Rust executable: ${name}`);
  copyTree(source, target);
  chmodSync(target, 0o755);
}

function macosApp(destination, product, targetDirectory, universal) {
  const contents = join(destination, "WoR-Flasher.app/Contents");
  const macos = join(contents, "MacOS");
  const resources = join(contents, "Resources");
  mkdirSync(macos, { recursive: true });
  runtime(resources, product);
  stageBinary("wor-flasher", join(macos, "wor-flasher"), targetDirectory, universal);
  stageBinary("wor-desktop", join(macos, "wor-desktop"), targetDirectory, universal);
  copyTree(join(root, "assets", product.iconFilename), join(resources, product.iconFilename));
  writeFileSync(join(contents, "Info.plist"), `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleName</key><string>${xml(product.name)}</string>
<key>CFBundleDisplayName</key><string>${xml(product.name)}</string>
<key>CFBundleIdentifier</key><string>${xml(product.bundleIdentifier)}</string>
<key>CFBundleExecutable</key><string>wor-desktop</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleVersion</key><string>${xml(product.version)}</string>
<key>CFBundleShortVersionString</key><string>${xml(product.version)}</string>
<key>CFBundleIconFile</key><string>${xml(product.iconFilename)}</string>
<key>LSMinimumSystemVersion</key><string>${xml(product.minimumMacosVersion)}</string>
<key>NSHighResolutionCapable</key><true/>
</dict></plist>
`);
  run("/usr/bin/plutil", ["-lint", join(contents, "Info.plist")]);
  return join(macos, "wor-flasher");
}

function notices(destination, metadata) {
  const sysroot = run("rustc", ["--print", "sysroot"]).trim();
  copyTree(join(sysroot, "share/doc/rust/COPYRIGHT-library.html"),
    join(destination, "licenses/rust-toolchain/COPYRIGHT-library.html"));
  const lines = [
    "Third-party Rust dependencies",
    "License expressions and original license/notice files from the locked Cargo packages.",
    "Rust standard-library runtime notices are supplied under licenses/rust-toolchain/COPYRIGHT-library.html.",
    "WoR-Flasher's own LICENSE and NOTICE remain in the runtime.",
    "",
  ];
  for (const pkg of metadata.packages.filter((pkg) => pkg.source).sort((a, b) => a.id.localeCompare(b.id))) {
    lines.push(`${pkg.name} ${pkg.version}: ${pkg.license ?? "See supplied license file"}`);
    if (pkg.repository) lines.push(pkg.repository);
    const directory = dirname(pkg.manifest_path);
    const licenseFiles = readdirSync(directory, { withFileTypes: true })
      .filter((entry) => entry.isFile() && /^(licen[cs]e|copying|notice)([._-].*)?$/i.test(entry.name))
      .map((entry) => entry.name);
    if (pkg.license_file && !licenseFiles.includes(pkg.license_file)) {
      if (pkg.license_file.split(/[\\/]/).some((part) => part === "..")) {
        throw new Error(`Unsafe dependency license path for ${pkg.name}`);
      }
      licenseFiles.push(pkg.license_file);
    }
    for (const file of licenseFiles) {
      const target = join(destination, "licenses", `${pkg.name}-${pkg.version}`, file);
      copyTree(join(directory, file), target);
    }
    lines.push("");
  }
  writeFileSync(join(destination, "THIRD-PARTY-NOTICES.txt"), `${lines.join("\n")}\n`);
}

export function verifyRustPi3Candidate(binary, output) {
  verifyPE(binary);
  const pe = binary.readUInt32LE(0x3c);
  if (binary.readUInt32LE(pe + 8) !== 0) {
    throw new Error("Rust Pi 3 candidate must have a deterministic PE timestamp.");
  }
  if ((binary.readUInt16LE(pe + 22) & 0x2002) !== 2 ||
      (binary.readUInt16LE(pe + 94) & 0x160) !== 0x160) {
    throw new Error("Rust Pi 3 candidate must retain executable, ASLR and DEP protections.");
  }
  const allowed = new Set([
    "kernel32.dll", "ntdll.dll", "advapi32.dll", "api-ms-win-core-synch-l1-2-0.dll",
    ...["stdio", "runtime", "locale", "heap", "private", "string", "math", "environment"]
      .map((part) => `api-ms-win-crt-${part}-l1-1-0.dll`),
  ]);
  const imports = [];
  let active = false;
  let name;
  for (const line of output.split(/\r?\n/)) {
    if (line === "Import {") {
      if (active) throw new Error("Malformed Windows import inspection.");
      active = true;
      name = undefined;
    } else if (active && line.startsWith("  Name: ")) {
      if (name !== undefined) throw new Error("Duplicate Windows import name.");
      name = line.slice(8).toLowerCase();
      if (!allowed.has(name)) throw new Error(`Non-system or unsupported Windows dependency: ${name}`);
    } else if (active && line === "}") {
      if (name === undefined) throw new Error("Missing Windows import name.");
      imports.push(name);
      active = false;
    }
  }
  if (active || imports.length !== new Set(imports).size ||
      !["kernel32.dll", "ntdll.dll", "advapi32.dll"].every((name) => imports.includes(name))) {
    throw new Error("Incomplete Windows import inspection.");
  }
  return imports.sort();
}

function preparePi3Candidate(toolchain, targetDirectory, product) {
  const target = "aarch64-pc-windows-gnullvm";
  const compiler = join(toolchain, "bin/aarch64-w64-mingw32-clang");
  const flags = [
    "-C", "target-cpu=generic", "-C", "target-feature=+crt-static",
    "-C", "link-arg=-Wl,--no-insert-timestamp",
  ];
  const env = {
    ...process.env,
    CARGO_TARGET_AARCH64_PC_WINDOWS_GNULLVM_LINKER: compiler,
    CARGO_ENCODED_RUSTFLAGS: [...flags, `--remap-path-prefix=${root}=.`].join("\x1f"),
  };
  delete env.RUSTFLAGS;
  const licenses = [
    { source: join(toolchain, "LICENSE.TXT"), name: "LLVM-LICENSE.TXT" },
    ...["COPYING", "COPYING.MinGW-w64.txt", "COPYING.MinGW-w64-runtime.txt",
      "COPYING.winpthreads.txt", "COPYING.winstorecompat.txt"].map((name) => ({
      source: join(toolchain, "aarch64-w64-mingw32/share/mingw32", name), name,
    })),
  ].map(({ source, name }) => ({
    path: `licenses/llvm-mingw/${name}`, content: readFileSync(source),
  }));
  run("cargo", ["build", "--locked", "--release", "-p", "wor-pi3-boot-refresh", "--target", target], {
    env, stdio: "inherit",
  });
  const path = join(targetDirectory, target, "release/wor-pi3-boot-refresh.exe");
  const binary = readFileSync(path);
  const imports = verifyRustPi3Candidate(binary, run(join(toolchain, "bin/llvm-readobj"), ["--coff-imports", path]));
  const digest = (content) => createHash("sha256").update(content).digest("hex");
  return {
    binary, licenses,
    manifest: {
      schemaVersion: 1, productVersion: product.version, target,
      qualification: "not-assessed", installedByDefault: false,
      compiler: run(compiler, ["--version"]).split(/\r?\n/)[0],
      rustc: run("rustc", ["--version"]), rustflags: flags,
      sourcePathsRemapped: true, binarySha256: digest(binary), imports,
      licenses: licenses.map(({ path, content }) => ({ path, sha256: digest(content) })),
    },
  };
}

const sourceRoots = [
    "Cargo.toml", "Cargo.lock", "rust-toolchain.toml", "rustfmt.toml", "crates", "apps", "src",
    "config-templates", "assets", "docs", "tests", ".github", ".editorconfig",
    ".gitignore", ".gitattributes", "AGENTS.md", "package.json", "package-lock.json",
    "playwright.config.mjs", "README.md", "CONTRIBUTING.md",
    "CODE_OF_CONDUCT.md", "SECURITY.md", "LICENSE", "NOTICE",
];

export function sourcePaths(directory = root) {
  const inventory = join(directory, "source-manifest.json");
  let paths;
  const fromInventory = existsSync(inventory);
  if (fromInventory) {
    const manifest = JSON.parse(readFileSync(inventory, "utf8"));
    if (manifest.formatVersion !== 1 || !Array.isArray(manifest.files)) throw new Error("Invalid source inventory.");
    paths = manifest.files;
  } else {
    const result = spawnSync("git", ["ls-files", "--cached", "--others", "--exclude-standard", "-z", "--", ...sourceRoots], {
      cwd: directory, encoding: "utf8",
    });
    if (result.error) throw result.error;
    if (result.status !== 0) throw new Error("Cannot enumerate reviewable source files; use a checkout or the supplied source inventory.");
    paths = result.stdout.split("\0").filter(Boolean);
  }
  const selected = new Set();
  for (const path of paths) {
    if (typeof path !== "string" || path.startsWith("/") || path.includes("\\")
        || path.split("/").some((part) => !part || part === "." || part === "..")
        || !sourceRoots.some((allowed) => path === allowed || path.startsWith(`${allowed}/`))) {
      throw new Error("Unsafe source inventory path.");
    }
    if (path.split("/").some((part) => ["target", "node_modules", ".git", ".venv", "__pycache__", ".cache"].includes(part))
        || /(?:\.pyc|\.log|\.rs\.bk)$/.test(path) || path.endsWith("/.DS_Store")) continue;
    if (!existsSync(join(directory, path))) {
      if (fromInventory) throw new Error(`Missing supplied source file: ${path}`);
      continue;
    }
    selected.add(path);
  }
  if (selected.size === 0) throw new Error("The source inventory is empty.");
  return [...selected].sort();
}

function sourceSnapshot(destination) {
  const files = sourcePaths();
  for (const path of files) copyTree(join(root, path), join(destination, "source", path));
  writeFileSync(join(destination, "source/source-manifest.json"), `${JSON.stringify({ formatVersion: 1, files }, null, 2)}\n`);
}

function checksums(directory) {
  const files = [];
  function walk(current) {
    for (const entry of readdirSync(current, { withFileTypes: true })) {
      const path = join(current, entry.name);
      if (entry.isDirectory()) walk(path);
      else if (entry.isFile()) files.push(path);
      else throw new Error(`Unsupported staged entry: ${path}`);
    }
  }
  walk(directory);
  const lines = files.sort().map((path) =>
    `${createHash("sha256").update(readFileSync(path)).digest("hex")}  ${relative(directory, path).replaceAll("\\", "/")}`);
  writeFileSync(join(directory, "SHA256SUMS"), `${lines.join("\n")}\n`);
}

if (isMainModule(import.meta.url)) {
try {
  for (const arg of args) {
    if (!["--desktop", "--universal", "--pi3-helper", "--check", "--clean", "--help"].includes(arg)) throw new Error(`Unknown option: ${arg}`);
  }
  if (args.includes("--clean") && args.length !== 1) throw new Error("--clean cannot be combined with build options.");
  if (args.includes("--help")) {
    console.log("Usage: node src/package-rust.mjs [--desktop] [--universal] [--pi3-helper] [--check] [--clean]\nStages a complete host-native Rust runtime; --check uses temporary staging.\n--universal requires macOS and --desktop; builds Intel and Apple Silicon binaries.\n--pi3-helper also builds an unqualified Windows ARM64 candidate using LLVM_MINGW_ROOT.\n--clean removes only release/rust staging output.");
  } else if (args.includes("--clean")) {
    rmSync(join(root, "release/rust"), { recursive: true, force: true });
    console.log("Removed release/rust staging output.");
  } else {
    if (!["darwin", "linux"].includes(process.platform)) {
      throw new Error("Rust frontend packaging supports macOS and Linux only.");
    }
    const desktop = args.includes("--desktop");
    const universal = args.includes("--universal");
    if (universal && (process.platform !== "darwin" || !desktop)) {
      throw new Error("--universal requires a macOS host and --desktop.");
    }
    if (args.includes("--pi3-helper") && !process.env.LLVM_MINGW_ROOT?.trim()) {
      throw new Error("Set LLVM_MINGW_ROOT to an installed open-source LLVM-MinGW toolchain; no SDK is downloaded or licensed automatically.");
    }
    const build = ["build", "--locked", "--release", "-p", "wor-flasher"];
    if (desktop) build.push("-p", "wor-desktop");
    const { product } = readProjectMetadata();
    if (universal) build.push("--target", macosTargets[0]);
    const buildOptions = { stdio: "inherit", env: { ...process.env,
      ...(process.platform === "darwin" ? { MACOSX_DEPLOYMENT_TARGET: product.minimumMacosVersion } : {}) } };
    run("cargo", build, buildOptions);
    if (universal) run("cargo", [...build.slice(0, -1), macosTargets[1]], buildOptions);
    const cargoMetadata = JSON.parse(run("cargo", ["metadata", "--locked", "--format-version=1"]));
    const candidate = args.includes("--pi3-helper")
      ? preparePi3Candidate(resolve(process.env.LLVM_MINGW_ROOT), cargoMetadata.target_directory, product) : undefined;
    const target = `${process.platform}-${universal ? "universal" : process.arch}`;
    const destination = args.includes("--check")
      ? (temporary = mkdtempSync(join(tmpdir(), "wor-rust-package-")))
      : join(root, "release/rust", target);
    if (!temporary) rmSync(destination, { recursive: true, force: true });
    mkdirSync(join(destination, "bin"), { recursive: true });
    runtime(destination, product);
    const cli = join(destination, "bin/wor-flasher");
    const binaryDirectory = cargoMetadata.target_directory;
    stageBinary("wor-flasher", cli, binaryDirectory, universal);
    if (desktop) stageBinary("wor-desktop", join(destination, "bin/wor-desktop"), binaryDirectory, universal);
    const appCli = desktop && process.platform === "darwin" ? macosApp(destination, product, binaryDirectory, universal) : undefined;
    notices(destination, cargoMetadata);
    if (candidate) {
      const directory = join(destination, "candidates/pi3-boot-refresh");
      mkdirSync(directory, { recursive: true });
      writeFileSync(join(directory, "Pi3BootRefresh.exe"), candidate.binary);
      writeFileSync(join(directory, "manifest.json"), `${JSON.stringify(candidate.manifest, null, 2)}\n`);
      for (const { path, content } of candidate.licenses) {
        mkdirSync(dirname(join(destination, path)), { recursive: true });
        writeFileSync(join(destination, path), content);
      }
      appendFileSync(join(destination, "THIRD-PARTY-NOTICES.txt"), "\nOptional Rust Pi 3 qualification candidate: statically linked LLVM libunwind and MinGW-w64 runtime.\nOriginal toolchain licenses are supplied under licenses/llvm-mingw/; Windows system DLLs are not redistributed.\n");
    }
    sourceSnapshot(destination);
    if (appCli) {
      const resources = join(dirname(dirname(appCli)), "Resources");
      for (const path of ["THIRD-PARTY-NOTICES.txt", "licenses", "source", ...(candidate ? ["candidates"] : [])]) {
        copyTree(join(destination, path), join(resources, path));
      }
      if (universal) {
        const app = join(destination, "WoR-Flasher.app");
        run("/usr/bin/codesign", ["--force", "--sign", "-", app]);
        run("/usr/bin/codesign", ["--verify", "--deep", "--strict", app]);
      }
    }
    writeFileSync(join(destination, "README.txt"), [
      `WoR-Flasher ${product.version} (${target})`,
      "",
      "Verify SHA256SUMS before use. Keep bin/, runtime/ and runtime-manifest.json together.",
      "CLI: bin/wor-flasher --help",
      "Desktop: bin/wor-flasher wizard, or open WoR-Flasher.app on macOS.",
      "The desktop uses the guided Dioxus workflow over the same native backend as the CLI.",
      "The host backend, configuration, image preparation and imaging helpers are native Rust.",
      "No Bash, Python or Node host runtime is included. Native image/OS tools are still required.",
      "Physical native writes are experimental and require explicit --allow-native-write consent.",
      "The Rust Pi 3 on-device helper is not promoted to installation media pending hardware qualification.",
      ...(candidate ? ["candidates/pi3-boot-refresh contains the cross-linked Rust helper, its import/qualification manifest and matching source/licenses. It is not installed by the writer."] : []),
      "Host flashing dependencies are still required. Windows and WSL hosts are unsupported.",
      ...(universal ? ["Both CLI and desktop contain verified Intel x86_64 and Apple Silicon arm64 slices."] : []),
      "macOS applications have no Developer ID signature or notarization. Linux desktops require the matching WebKitGTK runtime.",
      "This is the native Rust application. It has no self-update mechanism.",
      "The exact first-party build source is in source/; third-party license notices are supplied alongside it.",
      "Botspot and the WoR-Flasher contributors; Blackout Secure. See runtime/LICENSE and runtime/NOTICE.",
      "",
    ].join("\n"));
    const expectedVersion = `WoR-Flasher ${product.version}`;
    if (run(cli, ["--version"]).trim() !== expectedVersion) throw new Error("Native version drift.");
    run(cli, ["check-runtime"], { stdio: "inherit" });
    run(cli, ["run", "--version"], { stdio: "inherit", env: { ...process.env, NO_UPDATE: "1" } });
    if (appCli) run(appCli, ["check-runtime"], { stdio: "inherit" });
    checksums(destination);
    console.log(temporary ? "Rust package validation passed." : `Rust package staged in ${relative(root, destination)}.`);
  }
} catch (error) {
  console.error(error instanceof Error ? error.message : String(error));
  process.exitCode = 1;
} finally {
  if (temporary) rmSync(temporary, { recursive: true, force: true });
}
}
