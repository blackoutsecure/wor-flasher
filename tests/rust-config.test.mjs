import { describe, it } from "node:test";
import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";

const read = file => readFileSync(file, "utf8");
const workspace = () => read("Cargo.toml");
function table(source, name) {
  const header = `[${name}]`;
  const index = source.indexOf(header);
  assert.notEqual(index, -1, `Missing ${header}`);
  return source.slice(index + header.length).split(/\n\[/, 1)[0];
}

describe("Rust-first configuration contracts", () => {
  it("uses Rust 2024's MSRV-aware resolver and a matching pinned toolchain", () => {
    assert.match(table(workspace(), "workspace"), /resolver\s*=\s*"3"/);
    assert.match(table(workspace(), "workspace.package"), /edition\s*=\s*"2024"/);
    const minimum = table(workspace(), "workspace.package").match(/rust-version\s*=\s*"([^"]+)"/)?.[1];
    const toolchain = read("rust-toolchain.toml");
    const pinned = toolchain.match(/channel\s*=\s*"(\d+\.\d+\.\d+)"/)?.[1];
    assert.ok(pinned, "Pin an exact stable compiler release");
    assert.equal(pinned.split(".").slice(0, 2).join("."), minimum);
    assert.match(toolchain, /profile\s*=\s*"minimal"/);
    assert.match(toolchain, /components\s*=\s*\["clippy", "rustfmt"\]/);
  });
  it("centralizes repeated native dependencies without lifting frontend features into the workspace base", () => {
    const dependencies = table(workspace(), "workspace.dependencies");
    for (const dependency of ["libc", "fs2", "signal-hook", "dioxus-ssr"]) {
      assert.match(dependencies, new RegExp(`^${dependency}\\s*=`, "m"));
    }
    const dioxus = dependencies.split("\n").find(line => line.startsWith("dioxus ="));
    assert.match(dioxus, /default-features\s*=\s*false/);
    assert.doesNotMatch(dioxus, /"desktop"|"web"/);
    const frontend = read("apps/desktop/Cargo.toml");
    assert.match(frontend, /dioxus\s*=\s*\{[^}]*workspace\s*=\s*true[^}]*"desktop"[^}]*"document"/);
    assert.match(frontend, /dioxus-ssr\s*=\s*\{[^}]*workspace\s*=\s*true[^}]*optional\s*=\s*true/);
    for (const file of ["crates/ffu/Cargo.toml", "crates/iot-media/Cargo.toml", "crates/iot-account/Cargo.toml"]) {
      assert.match(read(file), /libc\.workspace\s*=\s*true/);
    }
    for (const file of ["crates/infrastructure/Cargo.toml", "crates/desktop-media/Cargo.toml"]) assert.match(read(file), /fs2\.workspace\s*=\s*true/);
    for (const file of ["crates/iot-media/Cargo.toml", "crates/iot-account/Cargo.toml"]) assert.match(read(file), /signal-hook\.workspace\s*=\s*true/);
  });
  it("keeps all workspace members private, lint-inheriting and the headless defaults UI-free", () => {
    const members = [...table(workspace(), "workspace").match(/^\s*members\s*=\s*\[([\s\S]*?)\]/m)[1].matchAll(/"([^"]+)"/g)].map(match => match[1]);
    assert.equal(new Set(members).size, members.length);
    for (const required of ["crates/domain", "crates/application", "apps/cli", "apps/desktop"]) assert.ok(members.includes(required));
    for (const member of members) {
      const manifest = read(`${member}/Cargo.toml`);
      for (const key of ["version", "edition", "rust-version", "license", "publish"]) assert.match(manifest, new RegExp(`${key}\\.workspace\\s*=\\s*true`), member);
      assert.match(table(manifest, "lints"), /workspace\s*=\s*true/, member);
    }
    const defaults = table(workspace(), "workspace").match(/default-members\s*=\s*\[([\s\S]*?)\]/)[1];
    assert.doesNotMatch(defaults, /apps\/desktop/);
    assert.match(table(workspace(), "workspace.package"), /publish\s*=\s*false/);
  });
  it("shares Rust formatting with editor rules and prevents placeholder/debug macros", () => {
    assert.ok(existsSync("rustfmt.toml"), "Commit the shared Rust formatter policy");
    const formatter = read("rustfmt.toml");
    assert.match(formatter, /edition\s*=\s*"2024"/);
    assert.match(formatter, /style_edition\s*=\s*"2024"/);
    assert.match(formatter, /newline_style\s*=\s*"Unix"/);
    assert.match(read(".editorconfig"), /\[\*\.rs\]\s+indent_size\s*=\s*4/);
    for (const lint of ["dbg_macro", "todo", "unimplemented"]) assert.match(table(workspace(), "workspace.lints.clippy"), new RegExp(`${lint}\\s*=\\s*"deny"`));
    assert.match(table(workspace(), "workspace.lints.rust"), /unsafe_op_in_unsafe_fn\s*=\s*"deny"/);
  });
  it("limits development/test debug data without changing release unwinding or portable CPU policy", () => {
    assert.match(table(workspace(), "profile.dev"), /debug\s*=\s*1/);
    assert.match(table(workspace(), "profile.test"), /debug\s*=\s*1/);
    const release = table(workspace(), "profile.release");
    assert.match(release, /lto\s*=\s*"thin"/);
    assert.match(release, /strip\s*=\s*true/);
    assert.doesNotMatch(workspace(), /panic\s*=\s*"abort"|target-cpu=native/);
  });
  it("watches and ships formatter/editor config and reports native Rust interfaces", () => {
    const workflow = read(".github/workflows/rust.yml");
    for (const path of ["rustfmt.toml", ".editorconfig", ".gitattributes"]) {
      assert.equal(workflow.split(`"${path}"`).length - 1, 2, path);
    }
    assert.match(read("src/package-rust.mjs"), /"rustfmt\.toml"/);
    const issue = read(".github/ISSUE_TEMPLATE/bug_report.yml");
    assert.match(issue, /Dioxus/);
    assert.match(issue, /Native Rust CLI/);
    assert.match(issue, /macOS/);
    assert.doesNotMatch(JSON.parse(read("package.json")).description, /flashing remains in the shell/);
  });
  it("preserves canonical JSON and safety defaults rather than embedding Rust-only write bypasses", () => {
    const config = JSON.parse(read("config-templates/config.json"));
    const schema = JSON.parse(read("config-templates/config.schema.json"));
    assert.equal(config.execution.dryRun, false);
    assert.equal(config.execution.skipImageVerification, false);
    assert.equal(config.execution.verifyTls, true);
    assert.equal(config.execution.checkForUpdates, true);
    assert.equal(config.media.useCache, 1);
    assert.equal(config.target.canInstallOnSameDrive, true);
    assert.equal(schema.properties.execution.properties.allowNativeWrite, undefined);
    assert.equal(config.appearance, undefined);
    assert.match(schema.description, /Rust/);
  });
});
