import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import {
  existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync,
  rmSync, statSync, writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";

const root = fileURLToPath(new URL("../", import.meta.url));
const engine = readFileSync(join(root, "install-wor.sh"), "utf8");
const original = "original cached installation image\n";

function fixture({
  model = 3, compression = "LZMS", chunk = 131072, solid = true,
  failure = "", cacheMode = 1, dryRun = 0, freeBytes = "107374182400", mode = 1,
  host = "Darwin", imageBytes = "",
} = {}) {
  const directory = mkdtempSync(join(tmpdir(), "wor-pi3-lzx-"));
  const image = join(directory, "cached image", "install.wim");
  try {
    mkdirSync(join(directory, "cached image"));
    writeFileSync(image, original);
    writeFileSync(join(directory, "cached image", "alldone"), "already extracted\n");
    const result = spawnSync("bash", ["-c", `
      source "$DIRECTORY/install-wor.sh" source >/dev/null || exit 90
      RPI_MODEL="$TEST_MODEL" BID=19045.3803
      RUN_MODE=cli CAN_INSTALL_ON_SAME_DRIVE="$TEST_MODE" USE_CACHE="$TEST_CACHE" DRY_RUN="$TEST_DRY_RUN"
      WOR_GUI_PROGRESS_FILE="$TEST_DIR/progress"
      is_macos() { [ "$TEST_HOST" == Darwin ]; }
      get_space_free() { printf '%s\\n' "$TEST_FREE"; }
      if [ -n "$TEST_IMAGE_BYTES" ];then get_file_size() { printf '%s\\n' "$TEST_IMAGE_BYTES"; };fi
      wiminfo() {
        printf 'info\\n' >> "$TEST_DIR/calls"
        [ "$TEST_FAILURE" != source-info ] || { printf 'fixture source inspection failure\\n' >&2; return 31; }
        local converted=0
        [ "$1" == "$TEST_IMAGE" ] || converted=1
        if [ "\${2:-}" == --blobs ];then
          [ "$TEST_FAILURE" != blobs ] || { printf 'fixture blob inspection failure\\n' >&2; return 32; }
          printf 'Uncompressed size = 10000 bytes\\n'
          if { [ "$converted" == 0 ] && [ "$TEST_SOLID" == 1 ]; } || { [ "$converted" == 1 ] && [ "$TEST_FAILURE" == solid-output ]; };then
            printf 'Flags = WIM_RESHDR_FLAG_SOLID\\n'
          else
            printf 'Flags = WIM_RESHDR_FLAG_COMPRESSED\\n'
          fi
          return
        fi
        local compression="$TEST_COMPRESSION" chunk="$TEST_CHUNK" name=First
        if [ "$converted" == 1 ];then compression=LZX; chunk=32768;fi
        if [ "$converted" == 1 ] && [ "$TEST_FAILURE" == chunk-output ];then chunk=65536;fi
        if [ "$converted" == 1 ] && [ "$TEST_FAILURE" == metadata-output ];then name=Changed;fi
        printf 'WIM Information:\\nPath: %s\\nImage Count: 2\\nCompression: %s\\nChunk Size: %s bytes\\nPart Number: 1/1\\nBoot Index: 0\\n\\nAvailable Images:\\nIndex: 1\\nName: %s\\nArchitecture: ARM64\\nIndex: 2\\nName: Second\\nArchitecture: ARM64\\n' "$1" "$compression" "$chunk" "$name"
      }
      wimexport() {
        printf 'export\\n' >> "$TEST_DIR/calls"
        printf '%s\\0' "$@" > "$TEST_DIR/export-args"
        [ ! -e "$3" ] || { printf 'Output already existed\\n' >&2; return 99; }
        printf 'converted fixture\\n' > "$3"
        if [ "$TEST_FAILURE" == interrupt ];then /bin/sh -c 'kill -TERM "$PPID"'; sleep 0.1;fi
        [ "$TEST_FAILURE" != export ] || { printf 'fixture export failure\\n' >&2; return 33; }
      }
      wimverify() {
        printf 'verify\\n' >> "$TEST_DIR/calls"
        [ "$TEST_FAILURE" != verify ] || { printf 'fixture integrity failure\\n' >&2; return 34; }
      }
      mv() {
        [ "$TEST_FAILURE" != promote ] || { printf 'fixture rename failure\\n' >&2; return 35; }
        command mv "$@"
      }
      prepare_pi3_install_wim "$TEST_IMAGE"
    `], {
      cwd: directory, encoding: "utf8", timeout: 10000,
      env: {
        ...process.env, NO_UPDATE: "1", DIRECTORY: root, WOR_CACHE_DIR: join(directory, "tool-cache"),
        TEST_DIR: directory, TEST_IMAGE: image, TEST_MODEL: String(model),
        TEST_COMPRESSION: compression, TEST_CHUNK: String(chunk), TEST_SOLID: solid ? "1" : "0",
        TEST_FAILURE: failure, TEST_CACHE: String(cacheMode), TEST_DRY_RUN: String(dryRun),
        TEST_FREE: freeBytes, TEST_MODE: String(mode),
        TEST_HOST: host, TEST_IMAGE_BYTES: String(imageBytes),
      },
    });
    const read = (name) => existsSync(join(directory, name)) ? readFileSync(join(directory, name), "utf8") : "";
    return {
      ...result, calls: read("calls").trim().split("\n").filter(Boolean),
      exportArgs: read("export-args").split("\0").slice(0, -1),
      image: readFileSync(image, "utf8"), files: readdirSync(join(directory, "cached image")).sort(),
      progress: read("progress"),
    };
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
}

describe("Pi 3 low-memory installation-image preparation", () => {
  for (const options of [
    {}, { compression: "LZX", chunk: 32768, solid: true },
    { compression: "LZX", chunk: 65536, solid: false },
    { compression: "XPRESS", chunk: 4096, solid: false },
    { cacheMode: 2 }, { dryRun: 1 }, { mode: 0 },
  ]) {
    it(`converts safely without skipping cache/preview paths ${JSON.stringify(options)}`, () => {
      const result = fixture(options);
      assert.equal(result.status, 0, result.stdout + result.stderr);
      assert.equal(result.image, "converted fixture\n");
      assert.equal(result.exportArgs[1], "all");
      for (const arg of ["--compress=LZX", "--chunk-size=32768", "--recompress", "--check"]) {
        assert.ok(result.exportArgs.includes(arg), `Missing export argument ${arg}`);
      }
      assert.doesNotMatch(result.exportArgs.join(" "), /--solid/);
      assert.equal(result.calls.filter((call) => call === "export").length, 1);
      assert.ok(result.calls.indexOf("verify") > result.calls.indexOf("export"));
      assert.deepEqual(result.files, ["alldone", "install.wim"]);
      assert.match(result.stdout + result.stderr, /non-solid LZX/);
      assert.match(result.progress, /Windows 10/);
    });
  }

  it("reuses an already compatible image without rewriting or recompressing it", () => {
    const result = fixture({ compression: "LZX", chunk: 32768, solid: false });
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.image, original);
    assert.ok(!result.calls.includes("export"));
    assert.match(result.stdout + result.stderr, /already.*non-solid LZX|Reusing.*non-solid LZX/);
  });

  for (const options of [{ model: 4 }, { model: 5 }]) {
    it(`leaves other target routes untouched ${JSON.stringify(options)}`, () => {
      const result = fixture(options);
      assert.equal(result.status, 0, result.stderr);
      assert.equal(result.image, original);
      assert.deepEqual(result.calls, []);
    });
  }

  for (const failure of ["source-info", "blobs", "export", "verify", "solid-output", "chunk-output", "metadata-output", "promote", "interrupt"]) {
    it(`retains the original image and removes staging after ${failure} failure`, () => {
      const result = fixture({ failure });
      assert.notEqual(result.status, 0, "A conversion failure must stop before disk writing");
      assert.equal(result.image, original);
      assert.deepEqual(result.files, ["alldone", "install.wim"]);
      assert.doesNotMatch(result.stdout + result.stderr, /Pi 3 installation image ready/);
      if (failure !== "interrupt") assert.match(result.stderr, /failed|Failed|invalid|match|LZX|solid|preserved|unchanged/i);
    });
  }

  for (const freeBytes of ["1024", "unavailable"]) {
    it(`fails explicitly before export when free space is ${freeBytes}`, () => {
      const result = fixture({ freeBytes });
      assert.notEqual(result.status, 0);
      assert.equal(result.image, original);
      assert.ok(!result.calls.includes("export"));
      assert.match(result.stderr, /space|free/i);
    });
  }

  it("requires the measured uncompressed blob space plus 512 MiB without rounding down", () => {
    const required = 10000 + 512 * 1024 * 1024;
    const insufficient = fixture({ freeBytes: String(required - 1) });
    assert.notEqual(insufficient.status, 0);
    assert.ok(!insufficient.calls.includes("export"));
    const exact = fixture({ freeBytes: String(required) });
    assert.equal(exact.status, 0, exact.stderr);
    assert.ok(exact.calls.includes("export"));
  });

  for (const host of ["Darwin", "Linux"]) {
    for (const mode of [0, 1]) {
      it(`checks exact prepared-image capacity before replacing the cache for ${host}, mode ${mode}`, () => {
        const limit = (mode === 1 ? 18000 : 6000) * (host === "Darwin" ? 1024 * 1024 : 1000000) - 64 * 1024 * 1024;
        const exact = fixture({ host, mode, imageBytes: limit });
        assert.equal(exact.status, 0, exact.stderr);
        const oversized = fixture({ host, mode, imageBytes: limit + 1 });
        assert.notEqual(oversized.status, 0);
        assert.equal(oversized.image, original);
        assert.deepEqual(oversized.files, ["alldone", "install.wim"]);
        assert.match(oversized.stderr, /does not fit/);
      });
    }
  }

  it("runs after every preparation path and before dry-run exit or any real disk writes", () => {
    const prepEnd = engine.indexOf("fi #end of the preparation half a password retry skips");
    const normalize = engine.indexOf('prepare_pi3_install_wim "$PWD/$winfiles/install.wim"', prepEnd);
    const dryRun = engine.indexOf('if [ "$DRY_RUN" == 1 ];then', prepEnd);
    const write = engine.indexOf("  darwin_flash_device", prepEnd);
    assert.ok(prepEnd >= 0 && prepEnd < normalize && normalize < dryRun && dryRun < write);
    assert.match(engine.slice(normalize, dryRun), /prepare_pi3_install_wim .* \|\| exit 1/);
  });

  for (const resume of [0, 1]) {
    it(`routes ${resume ? "password-retry" : "normal"} prepared media through normalization and stops on failure`, () => {
      const directory = mkdtempSync(join(tmpdir(), "wor-pi3-prewrite-"));
      try {
        const start = engine.indexOf("fi #end of the preparation half a password retry skips");
        const end = engine.indexOf("#now that downloads are complete", start);
        assert.ok(start >= 0 && end > start);
        const handoff = engine.slice(engine.indexOf("\n", start) + 1, end);
        for (const status of [0, 37]) {
          const result = spawnSync("bash", ["-c", `
            RESUME_AT_FLASH=${resume} WOR_RESUME_AT_FLASH=${resume} DRY_RUN=1
            winfiles=winfiles_cached
            prepare_pi3_install_wim() { printf 'normalize:%s\\n' "$1"; return ${status}; }
            status() { printf 'dry-run complete\\n'; }
            cli_pause() { :; }
            ${handoff}
          `], { cwd: directory, encoding: "utf8", timeout: 5000 });
          assert.match(result.stdout, /normalize:.*\/winfiles_cached\/install\.wim/);
          if (status === 0) {
            assert.equal(result.status, 0, result.stderr);
            assert.match(result.stdout, /dry-run complete/);
          } else {
            assert.equal(result.status, 1);
            assert.doesNotMatch(result.stdout, /dry-run complete/);
          }
        }
      } finally {
        rmSync(directory, { recursive: true, force: true });
      }
    });
  }
});

const wimlibAvailable = ["wimcapture", "wimappend", "wimexport", "wiminfo", "wimverify", "wimextract"].every(
  (tool) => spawnSync("bash", ["-c", 'command -v "$1"', "_", tool], { encoding: "utf8" }).status === 0,
);

describe("Real wimlib Pi 3 conversion", { skip: !wimlibAvailable }, () => {
  function command(tool, args, options = {}) {
    const result = spawnSync(tool, args, { encoding: "utf8", timeout: 30000, ...options });
    assert.equal(result.status, 0, `${tool} failed:\n${result.stdout}\n${result.stderr}`);
    return result.stdout;
  }

  for (const solidCompression of ["LZMS", "LZX"]) {
    it(`converts real solid ${solidCompression} data with all editions and files intact`, () => {
      const directory = mkdtempSync(join(tmpdir(), "wor-pi3-real-wim-"));
      try {
        const image = join(directory, "install.wim");
        for (const edition of ["First", "Second"]) {
          const source = join(directory, edition);
          mkdirSync(source);
          writeFileSync(join(source, "edition.txt"), edition);
          writeFileSync(join(source, "shared.bin"), Buffer.alloc(100000, 23));
          const tool = edition === "First" ? "wimcapture" : "wimappend";
          command(tool, [source, image, edition, "--solid", `--solid-compress=${solidCompression}`, "--solid-chunk-size=1048576", "--threads=1", "--check"]);
        }
        const before = command("wiminfo", [image]);
        assert.match(command("wiminfo", [image, "--blobs"]), /WIM_RESHDR_FLAG_SOLID/);
        const env = { ...process.env, NO_UPDATE: "1", DIRECTORY: root, WOR_CACHE_DIR: join(directory, "cache"),
          TEST_IMAGE: image, RPI_MODEL: "3", BID: "19045.3803", RUN_MODE: "cli", CAN_INSTALL_ON_SAME_DRIVE: "1" };
        const invoke = () => command("bash", ["-c", 'source "$DIRECTORY/install-wor.sh" source >/dev/null || exit 90; prepare_pi3_install_wim "$TEST_IMAGE"'], { cwd: directory, env });
        invoke();
        const after = command("wiminfo", [image]);
        assert.match(after, /^Compression:\s+LZX$/m);
        assert.match(after, /^Chunk Size:\s+32768 bytes$/m);
        assert.equal(after.slice(after.indexOf("Available Images:")), before.slice(before.indexOf("Available Images:")));
        assert.doesNotMatch(command("wiminfo", [image, "--blobs"]), /WIM_RESHDR_FLAG_SOLID/);
        command("wimverify", [image]);
        for (const [index, edition] of [[1, "First"], [2, "Second"]]) {
          const output = join(directory, `extracted-${index}`);
          command("wimextract", [image, String(index), "edition.txt", "shared.bin", `--dest-dir=${output}`]);
          assert.equal(readFileSync(join(output, "edition.txt"), "utf8"), edition);
          assert.deepEqual(readFileSync(join(output, "shared.bin")), Buffer.alloc(100000, 23));
        }
        const digest = createHash("sha256").update(readFileSync(image)).digest("hex");
        const mtime = statSync(image).mtimeMs;
        invoke();
        assert.equal(createHash("sha256").update(readFileSync(image)).digest("hex"), digest);
        assert.equal(statSync(image).mtimeMs, mtime);
        assert.ok(!readdirSync(directory).some((name) => name.startsWith(".pi3-lzx.")));
      } finally {
        rmSync(directory, { recursive: true, force: true });
      }
    });
  }
});
