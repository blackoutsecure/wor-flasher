import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";

const root = fileURLToPath(new URL("../", import.meta.url));
const engine = readFileSync(join(root, "install-wor.sh"), "utf8");
const firmwareStart = engine.indexOf('phase "Preparing Pi${RPI_MODEL} UEFI firmware"');
const firmwareEnd = engine.indexOf("{ #Download Windows ESD", firmwareStart);
const driverStart = engine.indexOf('if [ "$RPI_MODEL" != 5 ];then\n  phase "Preparing $(windows_version_label) ARM64 drivers"');
assert.ok(driverStart >= 0 && firmwareStart > driverStart && firmwareEnd > firmwareStart);

function release(tag, prefix = "RPi3_UEFI_Firmware_", extra = {}) {
  return {
    tag_name: tag, draft: false, prerelease: false,
    assets: [{ name: `${prefix}${tag}.zip`, state: "uploaded" }], ...extra,
  };
}

function fixture(pages, body = 'list_release_versions "$TEST_KIND"', {
  model = 3, kind = "uefi", fail = false, latest = null,
} = {}) {
  const directory = mkdtempSync(join(tmpdir(), "wor-release-catalogue-"));
  try {
    pages.forEach((page, index) => writeFileSync(join(directory, `page-${index + 1}.json`), JSON.stringify(page)));
    writeFileSync(join(directory, "latest.json"), JSON.stringify(latest));
    const result = spawnSync("bash", ["-c", `
      source "$DIRECTORY/install-wor.sh" source >/dev/null || exit 90
      wget() {
        { printf '%s ' "$@"; printf '\\n'; } >> "$TEST_DIR/requests"
        [ "$TEST_FAIL" != 1 ] || return 8
        local url argument
        for argument;do url="$argument";done
        case "$url" in
          "https://api.github.com/repos/"*"/releases/latest") cat "$TEST_DIR/latest.json"; return ;;
          "https://api.github.com/repos/"*"/releases?per_page=100&page="*) ;;
          *) printf 'Unexpected network request\\n' >&2; return 99 ;;
        esac
        local page="\${url##*page=}"
        if [ -f "$TEST_DIR/page-$page.json" ];then
          cat "$TEST_DIR/page-$page.json"
        else
          printf '[]\\n'
        fi
      }
      ${body}
    `], {
      cwd: directory, encoding: "utf8", timeout: 20000,
      env: {
        ...process.env, NO_UPDATE: "1", DIRECTORY: root, WOR_CACHE_DIR: join(directory, "cache"),
        TEST_DIR: directory, TEST_KIND: kind, TEST_FAIL: fail ? "1" : "0", RPI_MODEL: String(model),
      },
    });
    return {
      ...result,
      requests: existsSync(join(directory, "requests")) ? readFileSync(join(directory, "requests"), "utf8").trim().split("\n").map((line) => line.trim()) : [],
    };
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
}

describe("Compatible tagged release catalogue", () => {
  it("filters draft, prerelease, wrong-model, unavailable and unsafe assets", () => {
    const result = fixture([[
      release("v1.53.1"), release("v1.39"), release("v1.39"),
      release("v9.0", undefined, { draft: true }),
      release("v8.0", undefined, { prerelease: true }),
      release("v7.0", "RPi4_UEFI_Firmware_"),
      release("v6.0", undefined, { assets: [] }),
      release("v5.0", undefined, { assets: [{ name: "RPi3_UEFI_Firmware_v5.0.zip", state: "new" }] }),
      release("../outside"), release("tag!other"), release("tag\nother"),
    ]]);
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.stdout, "v1.53.1\nv1.39\n");
    assert.equal(result.requests.length, 1);
    assert.match(result.requests[0], /--timeout=8 --tries=1/);
    assert.match(result.requests[0], /repos\/pftf\/RPi3\/releases\?per_page=100&page=1/);
  });

  for (const [model, kind, prefix, repo] of [
    [4, "uefi", "RPi4_UEFI_Firmware_", "pftf/RPi4"],
    [5, "uefi", "RPi5_UEFI_Release_", "worproject/rpi5-uefi"],
    [3, "drivers", "RPi3_Windows_ARM64_Drivers_", "worproject/RPi-Windows-Drivers"],
    [4, "drivers", "RPi4_Windows_ARM64_Drivers_", "worproject/RPi-Windows-Drivers"],
  ]) {
    it(`selects the matching ${kind} package for Pi ${model}`, () => {
      const result = fixture([[release("v0.17", prefix), release("v1.39")]], undefined, { model, kind });
      assert.equal(result.status, 0, result.stderr);
      assert.equal(result.stdout, "v0.17\n");
      assert.ok(result.requests[0].includes(`/repos/${repo}/releases?`));
    });
  }

  it("reads every page and deduplicates versions across pages", () => {
    const page = Array.from({ length: 100 }, () => release("v1.53.1"));
    const result = fixture([page, [release("v1.53.1"), release("v1.39")]]);
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.stdout, "v1.53.1\nv1.39\n");
    assert.equal(result.requests.length, 2);
    assert.match(result.requests[1], /page=2$/);
  });

  it("reuses a fresh catalogue without another API call", () => {
    const result = fixture([[release("v1.39")]], `
      list_release_versions uefi || exit 91
      TEST_FAIL=1
      list_release_versions uefi
    `);
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.stdout, "v1.39\nv1.39\n");
    assert.equal(result.requests.length, 1);
  });

  it("refreshes an expired catalogue instead of silently accepting stale tags", () => {
    const result = fixture([[release("v1.39")]], `
      list_release_versions uefi >/dev/null || exit 91
      cache="$WOR_CACHE_DIR/github-releases/pftf/RPi3/pi3/uefi.json"
      jq '.fetched = 0' "$cache" > "$TEST_DIR/expired.json"
      mv "$TEST_DIR/expired.json" "$cache"
      list_release_versions uefi
    `);
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.requests.length, 2);
  });

  for (const payload of [{ message: "API limit" }, [null], [{ tag_name: "v1", assets: [] }]]) {
    it(`rejects malformed metadata ${JSON.stringify(payload)}`, () => {
      const result = fixture([payload]);
      assert.notEqual(result.status, 0);
      assert.equal(result.stdout, "");
      assert.match(result.stderr, /invalid release metadata/);
    });
  }

  it("reports a network failure without returning a partial list", () => {
    const result = fixture([], undefined, { fail: true });
    assert.notEqual(result.status, 0);
    assert.equal(result.stdout, "");
    assert.match(result.stderr, /Could not retrieve uefi releases/);
  });

  it("returns no partial tags when a later page is malformed", () => {
    const result = fixture([Array.from({ length: 100 }, () => release("v1.39")), { message: "failed page" }]);
    assert.notEqual(result.status, 0);
    assert.equal(result.stdout, "");
    assert.match(result.stderr, /invalid release metadata/);
  });

  it("reports an empty compatible catalogue", () => {
    const result = fixture([[release("v1", "OtherModel_")]]);
    assert.notEqual(result.status, 0);
    assert.equal(result.stdout, "");
    assert.match(result.stderr, /No published uefi releases with a Pi 3 package/);
  });

  it("refuses to silently truncate a release history at its pagination limit", () => {
    const page = Array.from({ length: 100 }, () => release("v1.39"));
    const result = fixture(Array.from({ length: 10 }, () => page));
    assert.notEqual(result.status, 0);
    assert.equal(result.stdout, "");
    assert.match(result.stderr, /refusing an incomplete list/);
    assert.equal(result.requests.length, 10);
  });

  it("does not query a driver repository for Pi 5", () => {
    const result = fixture([], undefined, { model: 5, kind: "drivers" });
    assert.notEqual(result.status, 0);
    assert.equal(result.requests.length, 0);
  });
});

describe("Authoritative latest release tags", () => {
  it("uses the latest endpoint even when the first catalogue release differs", () => {
    const result = fixture([[release("v1.54"), release("v1.53.1")]], `
      list_release_versions uefi || exit 91
      list_release_versions uefi latest
    `, { latest: release("v1.53.1") });
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.stdout, "v1.54\nv1.53.1\nv1.53.1\n");
    assert.equal(result.requests.length, 2);
    assert.match(result.requests[1], /\/pftf\/RPi3\/releases\/latest$/);
  });

  it("uses a separate five-minute cache for the latest tag", () => {
    const result = fixture([[release("v1.54")]], `
      list_release_versions uefi || exit 91
      list_release_versions uefi latest || exit 92
      TEST_FAIL=1
      list_release_versions uefi latest
    `, { latest: release("v1.53.1") });
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.stdout, "v1.54\nv1.53.1\nv1.53.1\n");
    assert.equal(result.requests.length, 2);
  });

  for (const latest of [
    { message: "Rate limit exceeded" },
    release("v1.53.1", "RPi4_UEFI_Firmware_"),
    release("v1.53.1", undefined, { draft: true }),
    release("v1.53.1", undefined, { prerelease: true }),
    release("../v1.53.1"),
    [release("v1.53.1")],
  ]) {
    it(`rejects an incompatible latest response ${JSON.stringify(latest)}`, () => {
      const result = fixture([], "list_release_versions uefi latest", { latest });
      assert.notEqual(result.status, 0);
      assert.equal(result.stdout, "");
      assert.match(result.stderr, /invalid release metadata|No published uefi releases/);
    });
  }

  it("reports a latest lookup failure rather than calling the catalogue head latest", () => {
    const result = fixture([[release("v1.54")]], `
      list_release_versions uefi >/dev/null || exit 91
      TEST_FAIL=1
      list_release_versions uefi latest
    `);
    assert.notEqual(result.status, 0);
    assert.equal(result.stdout, "");
    assert.match(result.stderr, /Could not retrieve uefi releases/);
  });

  it("checks the ARM64 asset for the selected Pi in the latest driver release", () => {
    const result = fixture([], "list_release_versions drivers latest", {
      kind: "drivers", model: 4, latest: release("v0.18", "RPi4_Windows_ARM64_Drivers_"),
    });
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.stdout, "v0.18\n");
    assert.match(result.requests[0], /\/worproject\/RPi-Windows-Drivers\/releases\/latest$/);
  });
});

describe("Selected release forwarding", () => {
  it("honors a configured driver version with an explicit false latest preference", () => {
    const result = fixture([], `
      printf '%s\\n' '{"customization":{"driversUseLatest":false},"system":{"driverVer":"v0.16"}}' > "$TEST_DIR/config.json"
      unset DRIVERS_USE_LATEST DRIVER_VER
      load_config_json "$TEST_DIR/config.json"
      printf '%s|%s' "$DRIVERS_USE_LATEST" "$DRIVER_VER"
    `);
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.stdout, "0|v0.16");
  });

  for (const [kind, tag, source, url] of [
    ["uefi", "v1.20", engine.slice(firmwareStart, firmwareEnd), "https://github.com/pftf/RPi3/releases/download/v1.20/RPi3_UEFI_Firmware_v1.20.zip"],
    ["drivers", "v0.16", engine.slice(driverStart, firmwareStart), "https://github.com/worproject/RPi-Windows-Drivers/releases/download/v0.16/RPi3_Windows_ARM64_Drivers_v0.16.zip"],
  ]) {
    it(`uses the selected ${kind} tag in the actual package preparation path`, () => {
      const result = fixture([], `
        UEFI_USE_LATEST=0 DRIVERS_USE_LATEST=0 USE_CACHE=1
        set_selected_release_version ${kind} ${tag} || exit 91
        phase() { :; }
        cache_is_current() { printf 'SELECTED %s\\n' "$2"; return 0; }
        ${source}
      `);
      assert.equal(result.status, 0, result.stderr);
      assert.ok(result.stdout.includes(`SELECTED ${url}\n`));
      assert.equal(result.requests.length, 0);
    });
  }

  it("exports the exact firmware and driver selections into the installer", () => {
    const result = fixture([], `
      UEFI_USE_LATEST=0 DRIVERS_USE_LATEST=0
      set_selected_release_version uefi v1.20 || exit 91
      set_selected_release_version drivers v0.16 || exit 92
      export_installer_settings
      bash -c 'source "$DIRECTORY/install-wor.sh" source >/dev/null || exit 93; printf "%s|%s|%s|%s\\n" "$(uefi_pinned_version)" "$DRIVER_VER" "$(uefi_use_latest)" "$DRIVERS_USE_LATEST"'
    `);
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.stdout, "v1.20|v0.16|0|0\n");
  });

  it("keeps firmware selections separate for each model", () => {
    const result = fixture([], `
      set_selected_release_version uefi v1.20
      RPI_MODEL=4; set_selected_release_version uefi v1.48
      RPI_MODEL=5; set_selected_release_version uefi v0.2
      for RPI_MODEL in 3 4 5;do uefi_pinned_version;done
    `);
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.stdout, "v1.20\nv1.48\nv0.2\n");
  });

  for (const tag of ["", "../v1", "v1\nv2", "tag!other", "v1;echo bad"]) {
    it(`rejects unsafe tag ${JSON.stringify(tag)} without changing the current pin`, () => {
      const result = fixture([], `
        UEFI_VER_PI3=v1.39
        if set_selected_release_version uefi '${tag}';then exit 91;fi
        printf '%s' "$UEFI_VER_PI3"
      `);
      assert.equal(result.status, 0, result.stderr);
      assert.equal(result.stdout, "v1.39");
      assert.match(result.stderr, /Invalid uefi release version/);
    });
  }
});
