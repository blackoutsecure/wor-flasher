import { before, describe, it } from "node:test";
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import {
  copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync,
  readdirSync, realpathSync, rmSync, symlinkSync, writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";
import { generateRuntimeManifest, readProjectMetadata, readRuntimePaths } from "../src/lib/node-runtime.mjs";

const root = fileURLToPath(new URL("../", import.meta.url));
const sourceGui = readFileSync(join(root, "install-wor-gui.sh"), "utf8");
const tools = new Map();
const digest = (data) => createHash("sha256").update(data).digest("hex");
const clientScript = `#!/bin/bash
#WOR_STANDALONE_CLIENT
printf 'runtime\\n' >> "$BOOT_EXECUTIONS"
printf '%s\\0' "$PWD" "\${WOR_CONFIG_FILE:-}" "$@"
`;

before(() => {
  for (const tool of ["dirname", "grep", "awk", "mkdir", "chmod", "mktemp", "rm", "mv", "cp", "sha256sum", "shasum"]) {
    const result = spawnSync("/bin/bash", ["-c", 'command -v "$1"', "bash", tool], { encoding: "utf8" });
    if (result.status === 0) tools.set(tool, result.stdout.trim());
    else if (tool !== "sha256sum" && tool !== "shasum") assert.fail(`Missing test utility: ${tool}`);
  }
});

function sandbox(callback, { pinned = true, transport = "curl", payload = clientScript, hashes = true } = {}) {
  const directory = mkdtempSync(join(tmpdir(), "wor-linux-gui-"));
  try {
    const client = join(directory, "client with spaces");
    const home = join(directory, "home");
    const bin = join(directory, "bin");
    const fixture = join(directory, "fixture");
    for (const path of [client, home, bin, fixture]) mkdirSync(path);
    const expected = digest(payload);
    const url = "https://github.com/blackoutsecure/wor-flasher/releases/download/v2.0.0";
    const gui = pinned
      ? sourceGui.replace(/^WOR_GUI_BOOTSTRAP_BASE_URL='[^']*'$/m, `WOR_GUI_BOOTSTRAP_BASE_URL='${url}'`)
        .replace(/^WOR_GUI_BOOTSTRAP_SHA256='[^']*'$/m, `WOR_GUI_BOOTSTRAP_SHA256='${expected}'`)
      : sourceGui;
    const script = join(client, "install-wor-gui.sh");
    writeFileSync(script, gui);
    writeFileSync(join(fixture, "install-wor.sh"), payload);
    writeFileSync(join(fixture, "SHA256SUMS"), `${expected}  install-wor.sh\n`);
    for (const [name, path] of tools) {
      if (hashes || (name !== "sha256sum" && name !== "shasum")) symlinkSync(path, join(bin, name));
    }
    writeFileSync(join(bin, "uname"), '#!/bin/bash\nprintf "%s\\n" "${BOOT_OS:-Linux}"\n', { mode: 0o755 });
    if (transport) {
      writeFileSync(join(bin, transport), `#!/bin/bash
printf '%s\\n' "$*" >> "$BOOT_DOWNLOADS"
[ "\${BOOT_OFFLINE:-0}" != 1 ] || { printf 'Mock download unavailable\\n' >&2; exit 22; }
output='' url=''
while [ "$#" -gt 0 ];do
  case "$1" in
    --output|-O) output="$2"; shift 2 ;;
    --) shift; url="$1"; break ;;
    *) shift ;;
  esac
done
[ -n "$output" ] && [ -n "$url" ] || exit 99
case "$url" in
  https://github.com/blackoutsecure/wor-flasher/releases/*/SHA256SUMS)
    cp "$BOOT_FIXTURE/SHA256SUMS" "$output" ;;
  https://github.com/blackoutsecure/wor-flasher/releases/*/install-wor.sh)
    cp "$BOOT_FIXTURE/install-wor.sh" "$output" ;;
  *) printf 'Unexpected download URL\\n' >&2; exit 99 ;;
esac
`, { mode: 0o755 });
    }
    const env = {
      HOME: home, PATH: bin, XDG_CACHE_HOME: join(directory, "cache"),
      NO_UPDATE: "1", BOOT_FIXTURE: fixture,
      BOOT_DOWNLOADS: join(directory, "downloads"), BOOT_EXECUTIONS: join(directory, "executions"),
    };
    const cache = join(env.XDG_CACHE_HOME, "wor-flasher/gui-client");
    callback({
      directory, client, fixture, script, env, bin, cache, cached: join(cache, `${expected}.sh`), expected,
      run(args = [], environment = {}) {
        return spawnSync("/bin/bash", [script, ...args], {
          cwd: home, env: { ...env, ...environment }, encoding: "utf8", timeout: 8000,
        });
      },
      downloads() { return existsSync(env.BOOT_DOWNLOADS) ? readFileSync(env.BOOT_DOWNLOADS, "utf8").trim().split("\n") : []; },
      executed() { return existsSync(env.BOOT_EXECUTIONS); },
    });
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
}

function populateRuntime(client) {
  const files = generateRuntimeManifest(root, readProjectMetadata().product.version, readRuntimePaths(), root).files;
  for (const { path } of files) {
    if (path === "install-wor-gui.sh") continue;
    const destination = join(client, path);
    mkdirSync(dirname(destination), { recursive: true });
    copyFileSync(join(root, path), destination);
  }
  writeFileSync(join(client, "install-wor.sh"), 'printf "complete local runtime\\n"; exit 0\n');
}

describe("Minimal Linux GUI bootstrap", () => {
  it("uses a complete nearby runtime without needing Git or a download tool", () => sandbox((test) => {
    populateRuntime(test.client);
    const result = test.run();
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.stdout, "complete local runtime\n");
    assert.equal(existsSync(test.cache), false);
  }, { transport: null }));

  it("uses a nearby standalone CLI and preserves arguments and local configuration", () => sandbox((test) => {
    writeFileSync(join(test.client, "install-wor.sh"), clientScript);
    writeFileSync(join(test.client, "config.json"), '{"custom":true}\n');
    const result = test.run(["--config", "site config.json"]);
    assert.equal(result.status, 0, result.stderr);
    assert.deepEqual(result.stdout.split("\0").slice(0, -1), [
      realpathSync(test.env.HOME), join(realpathSync(test.client), "config.json"),
      "--gui", "--config", "site config.json",
    ]);
    assert.equal(existsSync(test.cache), false);
    assert.equal(readFileSync(join(test.client, "config.json"), "utf8"), '{"custom":true}\n');
  }, { transport: null }));

  for (const missing of ["install-wor.sh", "src/lib/gui.sh", "config-templates/pi4.config.txt", "assets/next-steps.png"]) {
    it(`obtains one verified runtime when ${missing} is missing, without overwriting local edits`, () => sandbox((test) => {
      populateRuntime(test.client);
      rmSync(join(test.client, missing));
      writeFileSync(join(test.client, "config.json"), '{"site":"preserve"}\n');
      const before = existsSync(join(test.client, "install-wor.sh")) ? readFileSync(join(test.client, "install-wor.sh"), "utf8") : null;
      const result = test.run();
      assert.equal(result.status, 0, result.stderr);
      assert.equal(test.executed(), true);
      assert.equal(test.downloads().length, 1);
      assert.match(test.downloads()[0], /--proto =https --proto-redir =https/);
      assert.match(test.downloads()[0], /\/releases\/download\/v2\.0\.0\/install-wor\.sh$/);
      assert.ok(existsSync(test.cached));
      assert.equal(readFileSync(join(test.client, "config.json"), "utf8"), '{"site":"preserve"}\n');
      if (before !== null) assert.equal(readFileSync(join(test.client, "install-wor.sh"), "utf8"), before);
      else assert.equal(existsSync(join(test.client, "install-wor.sh")), false);
      assert.deepEqual(readdirSync(test.cache), [`${test.expected}.sh`]);
    }));
  }

  it("looks up exactly one checksum for an unversioned source GUI", () => sandbox((test) => {
    const result = test.run();
    assert.equal(result.status, 0, result.stderr);
    assert.equal(test.downloads().length, 2);
    assert.match(test.downloads()[0], /releases\/latest\/download\/SHA256SUMS$/);
    assert.match(test.downloads()[1], /releases\/latest\/download\/install-wor\.sh$/);
  }, { pinned: false }));

  it("reuses a pinned cached client when the network becomes unavailable", () => sandbox((test) => {
    assert.equal(test.run().status, 0);
    const result = test.run([], { BOOT_OFFLINE: "1" });
    assert.equal(result.status, 0, result.stderr);
    assert.equal(test.downloads().length, 1);
    assert.equal(readFileSync(test.env.BOOT_EXECUTIONS, "utf8"), "runtime\nruntime\n");
  }));

  it("supports HTTPS-only wget when curl is absent", () => sandbox((test) => {
    const result = test.run();
    assert.equal(result.status, 0, result.stderr);
    assert.match(test.downloads()[0], /--https-only/);
  }, { transport: "wget" }));

  it("fails explicitly with no downloader instead of executing an incomplete engine", () => sandbox((test) => {
    const result = test.run();
    assert.equal(result.status, 1);
    assert.match(result.stderr, /curl or wget is required/);
    assert.equal(test.executed(), false);
    assert.deepEqual(readdirSync(test.cache), []);
  }, { transport: null }));

  it("fails before download if no SHA-256 verifier is installed", () => sandbox((test) => {
    const result = test.run();
    assert.equal(result.status, 1);
    assert.match(result.stderr, /verification requires sha256sum or shasum/);
    assert.equal(test.downloads().length, 0);
    assert.equal(test.executed(), false);
  }, { hashes: false }));

  it("preserves local files and cleans staging when a download fails", () => sandbox((test) => {
    writeFileSync(join(test.client, "install-wor.sh"), "local incomplete engine\n");
    const result = test.run([], { BOOT_OFFLINE: "1" });
    assert.equal(result.status, 1);
    assert.match(result.stderr, /Could not download the GUI runtime/);
    assert.equal(readFileSync(join(test.client, "install-wor.sh"), "utf8"), "local incomplete engine\n");
    assert.equal(test.executed(), false);
    assert.deepEqual(readdirSync(test.cache), []);
  }));

  it("rejects downloaded content that does not match the release digest", () => sandbox((test) => {
    writeFileSync(join(test.fixture, "install-wor.sh"), clientScript + "#tampered\n");
    const result = test.run();
    assert.equal(result.status, 1);
    assert.match(result.stderr, /checksum mismatch/);
    assert.equal(test.executed(), false);
    assert.deepEqual(readdirSync(test.cache), []);
  }));

  for (const checksums of ["", "invalid  install-wor.sh\n", `${"a".repeat(64)}  install-wor.sh\n${"a".repeat(64)}  install-wor.sh\n`]) {
    it(`rejects missing, invalid, or duplicate release checksums (${checksums.length} bytes)`, () => sandbox((test) => {
      writeFileSync(join(test.fixture, "SHA256SUMS"), checksums);
      const result = test.run();
      assert.equal(result.status, 1);
      assert.match(result.stderr, /checksum/i);
      assert.equal(test.downloads().length, 1);
      assert.equal(test.executed(), false);
    }, { pinned: false }));
  }

  it("refuses to execute a raw engine advertised as a standalone client", () => sandbox((test) => {
    const result = test.run();
    assert.equal(result.status, 1);
    assert.match(result.stderr, /not a standalone client/);
    assert.equal(test.executed(), false);
    assert.deepEqual(readdirSync(test.cache), []);
  }, { payload: clientScript.replace("#WOR_STANDALONE_CLIENT\n", "") }));

  it("rejects cached tampering without replacing the cached file", () => sandbox((test) => {
    assert.equal(test.run().status, 0);
    rmSync(test.env.BOOT_EXECUTIONS);
    const modified = clientScript + "#modified cached client\n";
    writeFileSync(test.cached, modified);
    const result = test.run();
    assert.equal(result.status, 1);
    assert.match(result.stderr, /Cached GUI runtime failed verification/);
    assert.equal(test.executed(), false);
    assert.equal(readFileSync(test.cached, "utf8"), modified);
    assert.equal(test.downloads().length, 1);
  }));

  it("rejects a cached symlink even when its bytes match the pinned digest", () => sandbox((test) => {
    assert.equal(test.run().status, 0);
    rmSync(test.env.BOOT_EXECUTIONS);
    rmSync(test.cached);
    symlinkSync(join(test.fixture, "install-wor.sh"), test.cached);
    const result = test.run();
    assert.equal(result.status, 1);
    assert.match(result.stderr, /unsafe cached GUI runtime/);
    assert.equal(test.executed(), false);
  }));

  it("refuses a non-HTTPS bootstrap URL before invoking a downloader", () => sandbox((test) => {
    writeFileSync(test.script, readFileSync(test.script, "utf8").replace(
      /^WOR_GUI_BOOTSTRAP_BASE_URL='[^']*'$/m, "WOR_GUI_BOOTSTRAP_BASE_URL='http://example.invalid/releases/v2.0.0'"));
    const result = test.run();
    assert.equal(result.status, 1);
    assert.match(result.stderr, /Refusing a non-HTTPS/);
    assert.equal(test.downloads().length, 0);
    assert.equal(test.executed(), false);
  }));

  for (const tool of ["mkdir", "mktemp"]) {
    it(`stops before downloading if ${tool} cannot create private staging`, () => sandbox((test) => {
      rmSync(join(test.bin, tool));
      writeFileSync(join(test.bin, tool), '#!/bin/bash\nexit 73\n', { mode: 0o755 });
      const result = test.run();
      assert.equal(result.status, 1);
      assert.match(result.stderr, /Could not create GUI/);
      assert.equal(test.downloads().length, 0);
      assert.equal(test.executed(), false);
    }));
  }

  it("rejects a linked cache rather than writing through it", () => sandbox((test) => {
    const outside = join(test.directory, "outside");
    mkdirSync(outside);
    mkdirSync(dirname(test.cache), { recursive: true });
    symlinkSync(outside, test.cache);
    const result = test.run();
    assert.equal(result.status, 1);
    assert.match(result.stderr, /unsafe GUI runtime cache/);
    assert.equal(test.downloads().length, 0);
    assert.deepEqual(readdirSync(outside), []);
  }));

  for (const environment of [{ WSL_DISTRO_NAME: "mock-WSL" }, { BOOT_OS: "Darwin" }, { WOR_GUI_BOOTSTRAPPED: "1" }]) {
    it(`does not bootstrap an unsupported host or recurse (${JSON.stringify(environment)})`, () => sandbox((test) => {
      const result = test.run([], environment);
      assert.equal(result.status, 1);
      assert.equal(test.downloads().length, 0);
      assert.equal(test.executed(), false);
    }));
  }
});
