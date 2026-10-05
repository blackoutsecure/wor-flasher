import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { cpSync, existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";
import { verifyPi3BootRefresh } from "../src/build-pi3-boot-refresh.mjs";

const root = fileURLToPath(new URL("../", import.meta.url));
const engine = readFileSync(join(root, "install-wor.sh"), "utf8");
const batch = readFileSync(join(root, "config-templates/prefinalize.cmd"), "utf8");

function temporary(callback) {
  const directory = mkdtempSync(join(tmpdir(), "wor-pi3-finalize-"));
  try {
    return callback(directory);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
}

function inEngine(directory, command, env = {}) {
  return spawnSync("bash", ["-c", `
    source "$DIRECTORY/install-wor.sh" source >/dev/null || exit 90
    RPI_MODEL=3 WOR_IMAGE_FAMILY=desktop OOBE_NETWORK_BYPASS=0 PI4_AUTO_DISABLE_3GB=0 PI4_UEFI_SHELL_UNLOCK=0
    WINDOWS_ACCOUNT_SETUP=0 WINDOWS_LOCALE_SETUP=0
    ${command}
  `], {
    cwd: directory, encoding: "utf8", timeout: 30000,
    env: { ...process.env, DIRECTORY: root, NO_UPDATE: "1", WOR_CACHE_DIR: join(directory, "cache"), ...env },
  });
}

describe("Native Pi 3 finalizer packaging and staging", () => {
  it("ships a source-matched ARM64 executable and its Go license", () => {
    assert.doesNotThrow(() => verifyPi3BootRefresh());
  });

  for (const changed of ["source", "binary", "license"]) {
    it(`rejects a changed ${changed} rather than packaging a stale helper`, () => temporary((directory) => {
      for (const path of ["src/pi3-boot-refresh", "src/lib/pi3-boot-refresh"]) {
        cpSync(join(root, path), join(directory, path), { recursive: true });
      }
      const path = {
        source: "src/pi3-boot-refresh/bootstrap.go",
        binary: "src/lib/pi3-boot-refresh/Pi3BootRefresh.exe",
        license: "src/lib/pi3-boot-refresh/GO-LICENSE.txt",
      }[changed];
      const original = readFileSync(join(directory, path));
      writeFileSync(join(directory, path), Buffer.concat([original, Buffer.from("\nchanged\n")]));
      assert.throws(() => verifyPi3BootRefresh(directory), /manifest is stale/);
    }));
  }

  for (const version of [10, 11]) {
    for (const mode of [0, 1]) {
      for (const cache of [0, 1, 2]) {
        it(`stages mandatory repair without personalization for Windows ${version}, mode ${mode}, cache ${cache}`, () => temporary((directory) => {
          mkdirSync(join(directory, "peinstaller/winpe/2"), { recursive: true });
          const result = inEngine(directory, `
            BID=${version === 10 ? "19045.3803" : "22631.2861"}
            CAN_INSTALL_ON_SAME_DRIVE=${mode} USE_CACHE=${cache}
            mark_cache "$PWD/peinstaller" fixture
            configure_pe_prefinalize || exit 91
            if [ "$USE_CACHE" == 0 ] && cache_is_current "$PWD/peinstaller" fixture;then exit 92;fi
            USE_CACHE=1 cache_is_current "$PWD/peinstaller" fixture || exit 93
          `);
          assert.equal(result.status, 0, result.stdout + result.stderr);
          const scripts = join(directory, "peinstaller/winpe/2/scripts");
          assert.deepEqual(readdirSync(scripts).sort(), ["GO-LICENSE.txt", "Pi3BootRefresh.exe", "prefinalize.cmd"]);
          assert.deepEqual(readFileSync(join(scripts, "Pi3BootRefresh.exe")),
            readFileSync(join(root, "src/lib/pi3-boot-refresh/Pi3BootRefresh.exe")));
          const staged = readFileSync(join(scripts, "prefinalize.cmd"), "utf8");
          assert.match(staged, /\r\n/);
          assert.doesNotMatch(staged, /(?<!\r)\n|\r\r/);
        }));
      }
    }
  }

  for (const model of [4, 5]) {
    it(`removes cached Pi 3 repair when switching to Pi ${model}`, () => temporary((directory) => {
      mkdirSync(join(directory, "peinstaller/winpe/2"), { recursive: true });
      const result = inEngine(directory, `
        configure_pe_prefinalize || exit 91
        RPI_MODEL=${model}
        configure_pe_prefinalize || exit 92
      `);
      assert.equal(result.status, 0, result.stdout + result.stderr);
      assert.equal(existsSync(join(directory, "peinstaller/winpe/2/scripts")), false);
    }));
  }

  it("does not add the desktop finalizer to an IoT Core route", () => temporary((directory) => {
    mkdirSync(join(directory, "peinstaller/winpe/2"), { recursive: true });
    const result = inEngine(directory, `
      WOR_IMAGE_FAMILY=iot-core
      configure_pe_prefinalize || exit 91
    `);
    assert.equal(result.status, 0, result.stdout + result.stderr);
    assert.equal(existsSync(join(directory, "peinstaller/winpe/2/scripts/Pi3BootRefresh.exe")), false);
  }));

  for (const damage of ["missing", "tampered", "manifest"]) {
    it(`fails closed for a ${damage} native helper even with USE_CACHE=2`, () => temporary((directory) => {
      const runtime = join(directory, "runtime");
      cpSync(join(root, "src/lib/pi3-boot-refresh"), join(runtime, "src/lib/pi3-boot-refresh"), { recursive: true });
      const helper = join(runtime, "src/lib/pi3-boot-refresh");
      if (damage === "missing") rmSync(join(helper, "Pi3BootRefresh.exe"));
      if (damage === "tampered") writeFileSync(join(helper, "Pi3BootRefresh.exe"), "not a boot finalizer");
      if (damage === "manifest") writeFileSync(join(helper, "manifest.json"), "{}");
      const result = inEngine(directory, `
        DIRECTORY="$TEST_RUNTIME" USE_CACHE=2
        validate_pi3_boot_refresh
      `, { TEST_RUNTIME: runtime });
      assert.notEqual(result.status, 0);
      assert.match(result.stderr + result.stdout, /finalizer.*(missing|invalid|damaged)/i);
    }));
  }

  it("requires the installer payload before staging a Pi 3 hook", () => temporary((directory) => {
    const result = inEngine(directory, "configure_pe_prefinalize");
    assert.notEqual(result.status, 0);
    assert.match(result.stderr + result.stdout, /mandatory Pi 3 boot finalizer cannot be staged/);
  }));

  it("injects the verified finalizer into boot.wim index 2", (context) => {
    if (spawnSync("wimcapture", ["--version"]).error) {
      context.skip("wimlib is unavailable");
      return;
    }
    temporary((directory) => {
      mkdirSync(join(directory, "peinstaller/winpe/2"), { recursive: true });
      mkdirSync(join(directory, "base"));
      writeFileSync(join(directory, "base/startnet.cmd"), "@echo off\r\n");
      const result = inEngine(directory, `
        configure_pe_prefinalize || exit 91
        wimcapture base boot.wim WinPE --compress=LZX >/dev/null || exit 92
        wimappend base boot.wim Setup >/dev/null || exit 93
        wimupdate boot.wim 2 --command="add peinstaller/winpe/2 /" >/dev/null || exit 94
        wimextract boot.wim 2 /scripts --dest-dir=extracted >/dev/null || exit 95
      `);
      assert.equal(result.status, 0, result.stdout + result.stderr);
      const scripts = join(directory, "extracted/scripts");
      assert.deepEqual(readFileSync(join(scripts, "Pi3BootRefresh.exe")),
        readFileSync(join(root, "src/lib/pi3-boot-refresh/Pi3BootRefresh.exe")));
      assert.match(readFileSync(join(scripts, "prefinalize.cmd"), "utf8"), /:refresh_pi3/);
      assert.equal(existsSync(join(scripts, "GO-LICENSE.txt")), true);
    });
  });

  it("performs critical boot repair before optional answer-file exits", () => {
    assert.ok(batch.indexOf('"%~dp0Pi3BootRefresh.exe"\n') < batch.indexOf("set answerSource="));
    assert.match(batch, /:refresh_pi3\s+"%~dp0Pi3BootRefresh\.exe"\s+if not "%errorlevel%"=="0" \([\s\S]*?LogFatal[\s\S]*?exit \/b 1/);
    assert.match(batch, /finalizer is missing[\s\S]*?exit \/b 1/);
    assert.match(batch, /:end\s+:: Optional[^\n]*\nexit \/b 0/);
    assert.ok(engine.indexOf('validate_pi3_boot_refresh || error "The required Pi 3') < engine.indexOf('if [ "$DRY_RUN" == 1 ];then'));
    assert.equal((engine.match(/configure_pe_prefinalize \|\| error/g) ?? []).length, 2);
  });
});
