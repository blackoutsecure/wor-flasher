import { after, before, describe, it } from "node:test";
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import {
  chmodSync, copyFileSync, existsSync, mkdirSync, mkdtempSync,
  readFileSync, readdirSync, realpathSync, renameSync, rmSync, statSync, symlinkSync, writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { spawn, spawnSync } from "node:child_process";
import { once } from "node:events";
import { packageStandalone } from "../src/package-standalone.mjs";
import { generateRuntimeManifest, readProjectMetadata, readRuntimePaths } from "../src/lib/node-runtime.mjs";

const root = fileURLToPath(new URL("../", import.meta.url));
const build = mkdtempSync(join(tmpdir(), "wor-standalone-tests-"));
const realAsset = join(build, "install-wor.sh");
const fixtureAsset = join(build, "fixture.sh");
const version = readProjectMetadata().product.version;
const tools = [
  "base64", "tar", "gzip", "awk", "find", "wc", "tr", "mktemp", "mkdir",
  "chmod", "mv", "rm", "rmdir", "date", "dirname", "basename", "uname",
  "sed", "readlink", "sleep",
];
const binaries = new Map();

before(() => {
  for (const name of [...tools, "sha256sum", "shasum"]) {
    const command = spawnSync("/bin/bash", ["-c", 'command -v "$1"', "bash", name], { encoding: "utf8" });
    if (command.status === 0) binaries.set(name, command.stdout.trim());
    else if (tools.includes(name)) assert.fail(`Required test tool is missing: ${name}`);
  }
  assert.ok(binaries.has("sha256sum") || binaries.has("shasum"), "A SHA-256 utility is needed.");
  packageStandalone(realAsset);
  const fixture = join(build, "fixture");
  mkdirSync(join(fixture, "src/config"), { recursive: true });
  writeFileSync(join(fixture, "src/config/metadata.json"), JSON.stringify({
    product: { version },
    runtimePaths: ["install-wor.sh", "install-wor-gui.sh", "src/config", "payload.txt"],
  }));
  writeFileSync(join(fixture, "payload.txt"), "bundled payload");
  writeFileSync(join(fixture, "install-wor.sh"), `#!/bin/bash
set -eu
[ "$(cat "$DIRECTORY/payload.txt")" == "bundled payload" ] || exit 99
if [ "\${1:-}" == --gui ];then shift; exec "$DIRECTORY/install-wor-gui.sh" "$@"; fi
if [ "\${1:-}" == umask-test ];then umask; exit 0; fi
if [ "\${1:-}" == signal-test ];then
  trap 'printf "stopped\\n" > "$HOME/stopped"; exit 143' TERM
  printf "ready\\n" > "$HOME/ready"
  while true;do sleep 0.1;done
fi
printf '%s\\0' "$PWD" "\${DL_DIR:-}" "\${DRY_RUN:-}" "$@"
exit "\${FIXTURE_EXIT:-0}"
`, { mode: 0o755 });
  writeFileSync(join(fixture, "install-wor-gui.sh"), '#!/bin/bash\nprintf "mock gui\\n"; printf "%s\\n" "$@"\n', { mode: 0o755 });
  packageStandalone(fixtureAsset, fixture);
});
after(() => rmSync(build, { recursive: true, force: true }));

function client(asset = realAsset, { hash = binaries.has("sha256sum") ? "sha256sum" : "shasum", omit = [] } = {}) {
  const directory = mkdtempSync(join(build, "client "));
  const caller = join(directory, "caller with spaces");
  const home = join(directory, "home");
  const cache = join(directory, "cache");
  const path = join(directory, "tools");
  for (const folder of [caller, home, path]) mkdirSync(folder);
  const cat = spawnSync("/bin/bash", ["-c", "command -v cat"], { encoding: "utf8" }).stdout.trim();
  for (const name of [...tools, hash]) {
    if (!omit.includes(name)) symlinkSync(binaries.get(name), join(path, name));
  }
  symlinkSync(cat, join(path, "cat"));
  //Make an accidental download, install, or alternate checkout fail loudly.
  for (const name of ["git", "node", "curl", "wget", "sudo"]) {
    writeFileSync(join(path, name), `#!/bin/bash\nprintf '${name}\\n' >> "$HOME/unexpected-tools"\nexit 99\n`, { mode: 0o755 });
  }
  const script = join(caller, "install-wor.sh");
  copyFileSync(asset, script);
  chmodSync(script, 0o755);
  const env = {
    PATH: path, HOME: home, XDG_CACHE_HOME: cache, LC_ALL: "C",
    NO_UPDATE: "1", RUN_MODE: "cli",
  };
  const content = readFileSync(script, "utf8");
  const digest = content.match(/^payload_sha256='([0-9a-f]{64})'$/m)?.[1];
  assert.ok(digest);
  const runtime = join(cache, "wor-flasher/standalone", digest);
  return {
    directory, caller, home, script, env, runtime,
    run(args, extraEnv = {}) {
      return spawnSync("/bin/bash", [script, ...args], {
        cwd: caller, env: { ...env, ...extraEnv }, encoding: "utf8", timeout: 10000,
      });
    },
  };
}

describe("Single-file release client", () => {
  it("runs the actual engine from only the downloaded file with no Git, Node, or network use", () => {
    const test = client();
    assert.deepEqual(readdirSync(test.caller), ["install-wor.sh"]);
    const result = test.run(["--version"]);
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.stdout.trim(), `WoR-Flasher ${version}`);
    const help = test.run(["--help"]);
    assert.equal(help.status, 0, help.stderr);
    assert.match(help.stdout, /Usage: install-wor\.sh/);
    assert.match(help.stdout, /--config/);
    assert.match(help.stdout, /--gui/);
    assert.equal(existsSync(join(test.home, "unexpected-tools")), false);
    assert.deepEqual(readdirSync(test.caller), ["install-wor.sh"]);
    assert.deepEqual(readdirSync(test.home), []);
    assert.ok((statSync(test.runtime).mode & 0o077) === 0);
    assert.ok((statSync(join(test.runtime, "runtime/install-wor.sh")).mode & 0o100) !== 0);
  });

  it("contains every canonical runtime file, including templates, docs, and notices", () => {
    const test = client();
    const result = test.run(["--version"]);
    assert.equal(result.status, 0, result.stderr);
    const manifest = generateRuntimeManifest(root, version, readRuntimePaths(), root);
    for (const entry of manifest.files) {
      assert.deepEqual(readFileSync(join(test.runtime, "runtime", entry.path)), readFileSync(join(root, entry.path)), entry.path);
    }
    for (const path of ["README.md", "LICENSE", "NOTICE", "config-templates/pi4.config.txt", "src/lib/macos-disk-alerts.js"]) {
      assert.ok(existsSync(join(test.runtime, "runtime", path)));
    }
  });

  it("reuses the verified cache without rewriting its runtime or the downloaded script", () => {
    const test = client();
    const original = readFileSync(test.script);
    assert.equal(test.run(["--version"]).status, 0);
    const entry = join(test.runtime, "runtime/install-wor.sh");
    const before = statSync(entry).mtimeMs;
    assert.equal(test.run(["--version"]).status, 0);
    assert.equal(statSync(entry).mtimeMs, before);
    assert.deepEqual(readFileSync(test.script), original);
    assert.ok(readdirSync(join(test.env.XDG_CACHE_HOME, "wor-flasher/standalone")).every((name) =>
      name === test.runtime.split("/").at(-1) || name === "responses"));
  });

  it("passes working directory, configuration arguments, and environment unchanged to the bundled engine", () => {
    const test = client(fixtureAsset);
    writeFileSync(join(test.caller, "local config.json"), "{}\n");
    const args = ["--config", "local config.json", "an argument with spaces"];
    const result = test.run(args, { DL_DIR: "downloads with spaces", DRY_RUN: "1", DIRECTORY: "/not-the-runtime" });
    assert.equal(result.status, 0, result.stderr);
    assert.deepEqual(result.stdout.split("\0").slice(0, -1), [
      realpathSync(test.caller), "downloads with spaces", "1", ...args,
    ]);
  });

  it("forwards --gui to the embedded front-end rather than needing another downloaded file", () => {
    const test = client(fixtureAsset);
    const result = test.run(["--gui", "unchanged argument"]);
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.stdout, "mock gui\nunchanged argument\n");
  });

  it("preserves engine errors and does not translate failed execution into success", () => {
    const test = client(fixtureAsset);
    assert.equal(test.run(["--config", "site.json"], { FIXTURE_EXIT: "37" }).status, 37);
    const actual = client();
    const invalid = actual.run(["--not-a-valid-option"]);
    assert.equal(invalid.status, 1);
    assert.match(invalid.stderr, /Unknown argument/);
  });

  it("restores the caller's umask before running the engine", () => {
    const test = client(fixtureAsset);
    const result = test.run(["umask-test"]);
    assert.equal(result.status, 0, result.stderr);
    assert.equal(parseInt(result.stdout.trim(), 8), process.umask());
  });

  it("preserves the engine's WSL safety exclusion", () => {
    const test = client();
    const result = test.run([], { WSL_DISTRO_NAME: "mock-unsupported-host" });
    assert.equal(result.status, 1);
    assert.match(result.stderr, /does not support WSL/);
    assert.equal(existsSync(join(test.home, "unexpected-tools")), false);
  });

  it("uses the default home cache when XDG_CACHE_HOME is unset", () => {
    const test = client();
    const result = test.run(["--version"], { XDG_CACHE_HOME: "" });
    assert.equal(result.status, 0, result.stderr);
    assert.ok(existsSync(join(test.home, ".cache/wor-flasher/standalone")));
  });

  it("execs the engine so its signal handler remains in charge", async () => {
    const test = client(fixtureAsset);
    const process = spawn("/bin/bash", [test.script, "signal-test"], { cwd: test.caller, env: test.env, stdio: "ignore" });
    const finished = once(process, "exit");
    const deadline = setTimeout(() => process.kill("SIGKILL"), 10000);
    try {
      for (let attempt = 0; attempt < 100 && !existsSync(join(test.home, "ready")); attempt++) {
        await new Promise((resolve) => setTimeout(resolve, 50));
      }
      assert.ok(existsSync(join(test.home, "ready")), "The bundled engine did not start.");
      process.kill("SIGTERM");
      const [status, signal] = await finished;
      assert.equal(signal, null);
      assert.equal(status, 143);
      assert.equal(readFileSync(join(test.home, "stopped"), "utf8"), "stopped\n");
      assert.ok(existsSync(join(test.runtime, "runtime/install-wor.sh")));
    } finally {
      clearTimeout(deadline);
      if (process.exitCode === null && process.signalCode === null) {
        process.kill("SIGKILL");
        await finished;
      }
    }
  });

  for (const hash of ["sha256sum", "shasum"]) {
    it(`supports the ${hash} verification path`, (testContext) => {
      if (!binaries.has(hash)) {
        testContext.skip(`${hash} is not installed on this host.`);
        return;
      }
      const test = client(realAsset, { hash });
      const result = test.run(["--version"]);
      assert.equal(result.status, 0, result.stderr);
      assert.match(result.stdout, /WoR-Flasher/);
    });
  }

  it("reports a missing unpacking tool before creating a runtime", () => {
    const test = client(realAsset, { omit: ["base64"] });
    const result = test.run(["--version"]);
    assert.equal(result.status, 1);
    assert.match(result.stderr, /Required unpacking tool is missing: base64/);
    assert.equal(existsSync(test.runtime), false);
  });

  it("never unpacks without an available SHA-256 verifier", () => {
    const test = client(realAsset, { omit: ["sha256sum", "shasum"] });
    const result = test.run(["--version"]);
    assert.equal(result.status, 1);
    assert.match(result.stderr, /SHA-256 verification requires sha256sum or shasum/);
    assert.equal(existsSync(test.runtime), false);
  });

  it("rejects a linked cache root without writing into its destination", () => {
    const test = client();
    const outside = join(test.directory, "outside");
    mkdirSync(outside);
    mkdirSync(join(test.env.XDG_CACHE_HOME, "wor-flasher"), { recursive: true });
    symlinkSync(outside, join(test.env.XDG_CACHE_HOME, "wor-flasher/standalone"));
    const result = test.run(["--version"]);
    assert.equal(result.status, 1);
    assert.match(result.stderr, /Refusing a symbolic-link runtime cache/);
    assert.deepEqual(readdirSync(outside), []);
  });

  it("does not disturb an in-progress preparation lock from another launch", () => {
    const test = client();
    mkdirSync(`${test.runtime}.lock`, { recursive: true });
    const result = test.run(["--version"]);
    assert.equal(result.status, 1);
    assert.match(result.stderr, /already being prepared/);
    assert.ok(existsSync(`${test.runtime}.lock`));
    assert.equal(existsSync(test.runtime), false);
  });

  it("rejects a damaged embedded archive and cleans partial extraction", () => {
    const test = client();
    const content = readFileSync(test.script, "utf8");
    const start = content.indexOf("<<'WOR_STANDALONE_PAYLOAD'\n") + "<<'WOR_STANDALONE_PAYLOAD'\n".length;
    assert.ok(start > 30);
    writeFileSync(test.script, content.slice(0, start) + (content[start] === "A" ? "B" : "A") + content.slice(start + 1));
    const result = test.run(["--version"]);
    assert.equal(result.status, 1);
    assert.match(result.stderr, /Embedded runtime checksum mismatch/);
    assert.equal(existsSync(test.runtime), false);
    assert.deepEqual(readdirSync(join(test.env.XDG_CACHE_HOME, "wor-flasher/standalone")), []);
  });

  for (const mutation of ["file", "manifest", "extra-file", "symlink"]) {
    it(`refuses ${mutation} changes in a cached runtime without overwriting them`, () => {
      const test = client();
      assert.equal(test.run(["--version"]).status, 0);
      const target = join(test.runtime, "runtime/install-wor.sh");
      if (mutation === "file") writeFileSync(target, "changed content\n");
      if (mutation === "manifest") writeFileSync(join(test.runtime, "runtime.sha256"), "");
      if (mutation === "extra-file") writeFileSync(join(test.runtime, "runtime/config.json"), "{}");
      if (mutation === "symlink") {
        const copy = join(test.directory, "matching-source.sh");
        renameSync(target, copy);
        symlinkSync(copy, target);
      }
      const result = test.run(["--version"]);
      assert.equal(result.status, 1);
      assert.match(result.stderr, /Cached runtime failed verification/);
      if (mutation === "file") assert.equal(readFileSync(target, "utf8"), "changed content\n");
    });
  }

  it("refuses sourcing and piped execution without affecting the calling shell or preparing a runtime", () => {
    const test = client();
    const sourced = spawnSync("/bin/bash", ["-c", 'source "$1"; result=$?; printf "still running: %s\\n" "$result"', "bash", test.script], {
      cwd: test.caller, env: test.env, encoding: "utf8",
    });
    assert.equal(sourced.status, 0);
    assert.equal(sourced.stdout, "still running: 2\n");
    assert.match(sourced.stderr, /use the full checkout to source/);
    const piped = spawnSync("/bin/bash", [], { input: readFileSync(test.script), cwd: test.caller, env: test.env, encoding: "utf8" });
    assert.equal(piped.status, 2);
    assert.match(piped.stderr, /do not pipe it into Bash/);
    assert.equal(existsSync(test.runtime), false);
  });

  it("stages an executable standalone release artifact and includes it in the published checksums", () => {
    const result = spawnSync(process.execPath, ["src/build-release.mjs", "--platform=standalone"], { cwd: root, encoding: "utf8" });
    assert.equal(result.status, 0, result.stderr);
    const artifact = join(root, "release/standalone/install-wor.sh");
    const digest = createHash("sha256").update(readFileSync(artifact)).digest("hex");
    assert.match(readFileSync(join(root, "release/standalone/SHA256SUMS"), "utf8"), new RegExp(`^${digest}  install-wor\\.sh$`, "m"));
    assert.ok(statSync(artifact).mode & 0o100);
    const workflow = readFileSync(join(root, ".github/workflows/release.yml"), "utf8");
    assert.match(workflow, /cp release\/standalone\/install-wor\.sh "\$archive_dir\/install-wor\.sh"/);
    assert.match(workflow, /shasum -a 256 install-wor\.sh /);
    assert.match(workflow, /\$\{\{ steps\.package\.outputs\.directory \}\}\/install-wor\.sh/);
  });
});
