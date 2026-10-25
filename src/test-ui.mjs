#!/usr/bin/env node
import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { extname, join, relative, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";
import { isMainModule } from "./lib/node-runtime.mjs";

const root = fileURLToPath(new URL("../", import.meta.url));
const evidence = join(root, "release/ui-evidence");
const escape = value => String(value).replaceAll("&", "&amp;").replaceAll("<", "&lt;").replaceAll(">", "&gt;").replaceAll('"', "&quot;");

export function gallery(report, directory = evidence) {
  const media = new Map();
  function visit(value) {
    if (!value || typeof value !== "object") return;
    if (value.path && [".png", ".webm"].includes(extname(value.path))) {
      const path = resolve(value.path);
      if (!path.startsWith(`${resolve(directory)}${sep}`)) throw new Error("Evidence attachment escapes its output directory");
      media.set(path, relative(directory, path).replaceAll("\\", "/"));
    }
    for (const child of Object.values(value)) {
      if (Array.isArray(child)) child.forEach(visit);
      else if (child && typeof child === "object") visit(child);
    }
  }
  visit(report);
  const cards = [...media.values()].map(path => {
    const source = escape(path);
    return `<figure>${path.endsWith(".webm") ? `<video controls preload="metadata" src="${source}"></video>` : `<a href="${source}"><img loading="lazy" src="${source}" alt="${source}"></a>`}<figcaption>${source}</figcaption></figure>`;
  }).join("\n");
  const coverage = existsSync(join(directory, "coverage.json")) ? JSON.parse(readFileSync(join(directory, "coverage.json"), "utf8")) : undefined;
  const limitation = coverage?.unavailable?.length
    ? `<p><strong>Not Assessed:</strong> ${escape(coverage.unavailable.join(" "))}</p>` : "";
  return `<!doctype html><html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>WoR-Flasher UI consistency evidence</title><style>:root{color-scheme:light dark;font:16px system-ui}body{max-width:1600px;margin:auto;padding:24px}section{display:grid;grid-template-columns:repeat(auto-fit,minmax(min(100%,320px),1fr));gap:20px}figure{margin:0;border:1px solid GrayText;padding:12px;border-radius:12px}img,video{width:100%;max-height:680px;object-fit:contain;object-position:top}figcaption{overflow-wrap:anywhere;font-size:12px;margin-top:12px}</style><h1>WoR-Flasher UI consistency evidence</h1><p>Actual production Dioxus markup and shared CSS rendered with deterministic fixture data. These are presentation recordings, not physical flashes or native OS-dialog recordings.</p><p>Browser projects executed: ${escape(coverage?.projects?.join(", ") ?? "see detailed report")}.</p>${limitation}<p>Results: expected ${report.stats?.expected ?? 0}, unexpected ${report.stats?.unexpected ?? 0}, skipped ${report.stats?.skipped ?? 0}. <a href="report/index.html">Detailed Playwright report</a></p><section>${cards}</section></html>`;
}

export function browserCoverage(platform, systemVersion, playwrightVersion, explicit = []) {
  const all = ["chromium", "firefox", "webkit"];
  if (explicit.length) return { projects: explicit, unavailable: [] };
  if (platform === "darwin" && systemVersion.startsWith("27.") && playwrightVersion === "1.64.1") {
    return {
      projects: ["chromium", "webkit"],
      unavailable: ["Firefox on macOS 27 with Playwright 1.64.1 cannot create/load a profile (Could not find profile folder). Default, canonical temp, private HOME profiles and the preceding Firefox 155 build all failed before UI execution. Linux CI still requires Firefox; explicit --project=firefox retries it. https://github.com/Moltenship/time/issues/182"],
    };
  }
  return { projects: all, unavailable: [] };
}

function run(program, args) {
  const result = spawnSync(program, args, { cwd: root, stdio: "inherit" });
  if (result.error) throw result.error;
  return result.status ?? 1;
}

if (isMainModule(import.meta.url)) {
  try {
    if (!existsSync(join(root, "node_modules/@playwright/test/cli.js"))) throw new Error("Run npm ci and npx playwright install before visual tests.");
    const build = run("cargo", ["run", "--locked", "-p", "wor-desktop", "--features", "visual-fixtures", "--",
      "--runtime-dir", root, "--export-ui-fixtures", join(root, "release/ui-fixtures")]);
    if (build !== 0) process.exitCode = build;
    else {
      mkdirSync(evidence, { recursive: true });
      rmSync(join(evidence, "results.json"), { force: true });
      rmSync(join(evidence, "index.html"), { force: true });
      const args = process.argv.slice(2);
      const explicit = args.flatMap((arg, index) => arg.startsWith("--project=")
        ? [arg.slice(10)] : arg === "--project" ? [args[index + 1]] : []);
      const system = process.platform === "darwin"
        ? spawnSync("/usr/bin/sw_vers", ["-productVersion"], { encoding: "utf8" }) : undefined;
      if (system && (system.error || system.status !== 0)) throw new Error("Cannot determine macOS browser compatibility.");
      const version = JSON.parse(readFileSync(join(root, "node_modules/@playwright/test/package.json"), "utf8")).version;
      const coverage = browserCoverage(process.platform, system?.stdout.trim() ?? "", version, explicit);
      writeFileSync(join(evidence, "coverage.json"), `${JSON.stringify(coverage, null, 2)}\n`);
      for (const message of coverage.unavailable) console.warn(`Not Assessed: ${message}`);
      const selected = explicit.length ? [] : coverage.projects.map(name => `--project=${name}`);
      const status = run(process.execPath, [join(root, "node_modules/@playwright/test/cli.js"), "test", ...selected, ...args]);
      const report = join(evidence, "results.json");
      if (existsSync(report)) writeFileSync(join(evidence, "index.html"), gallery(JSON.parse(readFileSync(report, "utf8"))));
      else throw new Error("Visual test runner did not produce its evidence report.");
      process.exitCode = status;
    }
  } catch (error) {
    console.error(error instanceof Error ? error.message : String(error));
    process.exitCode = 1;
  }
}
