import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { readProjectMetadata } from "../src/lib/node-runtime.mjs";

const root = fileURLToPath(new URL("../", import.meta.url));
const release = readFileSync(`${root}/.github/workflows/release.yml`, "utf8");
const rust = readFileSync(`${root}/.github/workflows/rust.yml`, "utf8");

describe("Rust-only release workflow", () => {
  it("resolves release versions from canonical native metadata", () => {
    const lookup = `node -p 'require("./src/config/metadata.json").product.version'`;
    assert.equal(release.split(lookup).length - 1, 3);
    assert.match(readProjectMetadata().product.version, /^\d+\.\d+\.\d+$/);
    assert.match(release, /Release tag must be vX\.Y\.Z/);
  });

  it("limits release version commits to metadata and derived package metadata", () => {
    assert.match(release, /^permissions:\n  contents: read$/m);
    const prepare = release.slice(release.indexOf("  prepare:"), release.indexOf("  validate-linux:"));
    assert.match(prepare, /    permissions:\n      contents: write/);
    assert.match(prepare, /git add src\/config\/metadata\.json package\.json/);
    assert.match(prepare, /Co-authored-by: Copilot/);
    assert.doesNotMatch(prepare, /install-wor|src\/macos-app|metadata\.sh/);
  });

  it("validates both native host builds and publishes only their packages", () => {
    for (const command of [
      "cargo fmt --all -- --check",
      "cargo test --locked --workspace --exclude wor-desktop",
      "cargo clippy --locked --workspace --all-targets -- -D warnings",
    ]) {
      assert.equal(release.split(command).length - 1, 2, command);
    }
    for (const suffix of ["", " --universal"]) {
      assert.equal(release.split(`node src/package-rust.mjs --desktop${suffix} --check\n`).length - 1, 1);
      assert.equal(release.split(`node src/package-rust.mjs --desktop${suffix}\n`).length - 1, 1);
    }
    assert.match(release, /actions\/upload-artifact@/);
    assert.match(release, /actions\/download-artifact@/);
    assert.match(release, /wor-flasher-\$\{RELEASE_VERSION#v\}-\$\{target\}\.tar\.gz/);
    assert.match(release, /Expected the validated Linux x64, Linux ARM64 and universal macOS Rust packages/);
    assert.match(release, /for target in linux-x64 linux-arm64 darwin-universal/);
    assert.match(release, /shasum -a 256 wor-flasher-/);
    assert.match(release, /WoR-Flasher\.app/);
    assert.doesNotMatch(release, /install-wor|shellcheck|run-tests\.sh|\.dmg|linux-cli\.tar\.gz/);
  });

  it("covers native Linux architectures, both Macs and explicitly limited Windows scope", () => {
    for (const runner of ["ubuntu-24.04", "ubuntu-24.04-arm", "macos-latest", "macos-15-intel"]) {
      assert.ok(rust.includes(runner), runner);
    }
    assert.match(release, /os: ubuntu-22\.04-arm/);
    assert.match(release, /name: wor-flasher-rust-linux-\$\{\{ matrix\.arch \}\}/);
    assert.match(release, /target=darwin-universal/);
    assert.match(rust, /cargo build --locked -p wor-flasher --target armv7-unknown-linux-gnueabihf/);
    assert.match(rust, /cargo test --locked -p wor-domain -p wor-application/);
    assert.match(rust, /not a Windows host app/);
    assert.match(rust, /runner\.os == 'macOS' \|\| runner\.arch == 'X64'/);
  });

  it("runs native Rust and Node contracts when Rust source or package inputs change", () => {
    for (const path of ["crates/**", "apps/**", "src/**", "tests/**", "README.md"]) {
      assert.match(rust, new RegExp(path.replaceAll("*", "\\*")));
    }
    assert.match(rust, /npm run check/);
    assert.match(rust, /cargo test --locked --workspace --exclude wor-desktop/);
    assert.doesNotMatch(rust, /install-wor|shellcheck|run-tests\.sh/);
  });
});
