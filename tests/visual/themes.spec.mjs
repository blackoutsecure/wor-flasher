import { test, expect } from "@playwright/test";
import { capture, openFixture, palette } from "./helpers.mjs";

test.use({ video: {mode:"on",size:{width:1280,height:900}}, trace: "on" });

test("system appearance follows the OS; explicit light/dark overrides survive fixture navigation", async ({ page }, testInfo) => {
  await page.emulateMedia({ colorScheme: "light" });
  await openFixture(page, "image", "system");
  await palette(page, "light");
  await page.emulateMedia({ colorScheme: "dark" });
  await palette(page, "dark");
  await page.getByRole("combobox", { name: "Appearance", exact: true }).selectOption("light");
  await expect(page).toHaveURL(/image-light\.html$/);
  await expect(page.locator(".app")).toHaveAttribute("data-fixture-ready", "true");
  await palette(page, "light");
  await capture(page, testInfo, "explicit-light");
  await page.waitForTimeout(350); // Make the selected palette visible in the recording.
  await page.getByRole("combobox", { name: "Appearance", exact: true }).selectOption("dark");
  await expect(page).toHaveURL(/image-dark\.html$/);
  await expect(page.locator(".app")).toHaveAttribute("data-fixture-ready", "true");
  await palette(page, "dark");
  await page.getByRole("combobox", { name: "Fixture screen" }).selectOption("drive");
  await expect(page).toHaveURL(/drive-dark\.html$/);
  await expect(page.getByRole("heading", { name: "Choose the whole drive to erase" })).toBeVisible();
  await palette(page, "dark");
  await page.reload();
  await expect(page.getByRole("combobox", { name: "Appearance", exact: true })).toHaveValue("dark");
  await capture(page, testInfo, "explicit-dark-drive");
  await page.waitForTimeout(350);
});
