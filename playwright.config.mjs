import { defineConfig } from "@playwright/test";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("./", import.meta.url));
export default defineConfig({
  testDir: "./tests/visual",
  testMatch: "**/*.spec.mjs",
  fullyParallel: true,
  forbidOnly: Boolean(process.env.CI),
  retries: 0,
  workers: 2,
  timeout: 90_000,
  outputDir: "./release/ui-evidence/results",
  reporter: [
    ["list"],
    ["json", { outputFile: "./release/ui-evidence/results.json" }],
    ["html", { outputFolder: "./release/ui-evidence/report", open: "never" }],
  ],
  use: {
    baseURL: "http://127.0.0.1:4179",
    viewport: { width: 1280, height: 900 },
    reducedMotion: "reduce",
    colorScheme: "light",
    locale: "en-US",
    timezoneId: "UTC",
    screenshot: "only-on-failure",
    video: "retain-on-failure",
    trace: "retain-on-failure",
  },
  projects: [
    { name: "chromium", use: { browserName: "chromium" } },
    { name: "firefox", use: { browserName: "firefox" } },
    { name: "webkit", use: { browserName: "webkit" } },
  ],
  webServer: {
    command: "node tests/visual/server.mjs",
    cwd: root,
    url: "http://127.0.0.1:4179/fixtures.json",
    reuseExistingServer: false,
    timeout: 15_000,
  },
});
