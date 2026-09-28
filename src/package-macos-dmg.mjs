import { spawnSync } from "node:child_process";
import { existsSync, lstatSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { copyTree, verifyRuntimeManifest } from "./lib/node-runtime.mjs";

export function packageMacosDmg(appRoot, destination, version) {
  if (process.platform !== "darwin") throw new Error("Creating a macOS DMG requires hdiutil on macOS.");
  if (!/^\d+\.\d+\.\d+$/.test(version)) throw new Error("Invalid macOS DMG version.");
  if (!existsSync(appRoot) || !lstatSync(appRoot).isDirectory() || !verifyRuntimeManifest(appRoot, version)) {
    throw new Error("The macOS app must contain a verified current runtime before DMG packaging.");
  }
  const plist = readFileSync(join(appRoot, "Contents/Info.plist"), "utf8");
  for (const key of ["CFBundleShortVersionString", "CFBundleVersion"]) {
    const value = plist.match(new RegExp(`<key>${key}</key>\\s*<string>([^<]*)</string>`))?.[1];
    if (value !== version) throw new Error(`The app ${key} does not match the DMG version.`);
  }
  if (lstatSync(destination, { throwIfNoEntry: false })) throw new Error(`Refusing to replace an existing DMG: ${destination}`);
  mkdirSync(dirname(destination), { recursive: true });
  const stage = mkdtempSync(join(tmpdir(), "wor-macos-dmg-"));
  let started = false;
  let verified = false;
  try {
    const source = join(stage, "contents");
    copyTree(appRoot, join(source, "WoR-Flasher.app"));
    writeFileSync(join(source, "README.txt"), [
      `WoR-Flasher ${version} for macOS`,
      "",
      "Copy WoR-Flasher.app to a writable folder such as ~/Applications before opening it.",
      "Keep the complete app bundle together. Eject this disk image after copying.",
      "The app is unsigned and unnotarized; verify the DMG against the release SHA256SUMS.",
      "README, LICENSE, and NOTICE are included in the app's embedded runtime.",
      "Flashing erases the selected target disk. Verify the target and back it up first.",
      "",
    ].join("\n"));
    started = true;
    const result = spawnSync("/usr/bin/hdiutil", [
      "create", "-volname", `WoR-Flasher ${version}`, "-srcfolder", source,
      "-fs", "HFS+", "-format", "UDZO", "-nospotlight", destination,
    ], { encoding: "utf8", timeout: 180000 });
    if (result.error || result.status !== 0) {
      throw new Error(`DMG creation failed: ${result.error?.message || result.stderr || result.stdout}`);
    }
    const verify = spawnSync("/usr/bin/hdiutil", ["verify", destination], { encoding: "utf8", timeout: 120000 });
    if (verify.error || verify.status !== 0) {
      throw new Error(`DMG verification failed: ${verify.error?.message || verify.stderr || verify.stdout}`);
    }
    verified = true;
  } finally {
    if (started && !verified) rmSync(destination, { force: true });
    rmSync(stage, { recursive: true, force: true });
  }
  return destination;
}
