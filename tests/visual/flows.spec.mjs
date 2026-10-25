import { test, expect } from "@playwright/test";
import { capture, contrastAudit, layoutAudit, openFixture } from "./helpers.mjs";

test.use({
  viewport: {width:375,height:812},
  video: {mode:"on",size:{width:376,height:812}}, // The encoder needs an even width; the UI viewport remains 375px.
  trace: "on",
});

test("wizard presentation preserves navigation, readable options and explicit safety messaging", async ({ page }, testInfo) => {
  await page.setViewportSize({ width: 375, height: 812 });
  await openFixture(page, "image", "light");
  for (const scene of ["drive", "options", "review", "progress", "result"]) {
    await page.getByRole("combobox", { name: "Fixture screen" }).selectOption(scene);
    await expect(page).toHaveURL(new RegExp(`/${scene}-light\\.html$`));
    await expect(page.locator(".app")).toHaveAttribute("data-fixture-ready", "true");
    expect(await layoutAudit(page)).toEqual([]);
    await capture(page, testInfo, `phone-${scene}`);
    await page.waitForTimeout(350); // Recording review time; readiness is asserted above.
  }
  await expect(page.getByText("Hardware boot: Not Assessed.", { exact: false })).toBeVisible();
  await expect(page.getByText("Visual fixture - no imaging operation was executed.", { exact: true })).toBeVisible();
});

test("protected boot editor and advanced controls stay labelled, readable and cancelable", async ({ page }, testInfo) => {
  await openFixture(page, "options", "dark");
  const menus = page.locator(".app details");
  for (let index=0; index<await menus.count(); index++) {
    const menu=menus.nth(index);
    if(!await menu.evaluate(element=>element.open)) await menu.locator("summary").press("Enter");
    expect(await menu.evaluate(element=>element.open)).toBe(true);
  }
  expect(await layoutAudit(page)).toEqual([]);
  expect(await contrastAudit(page)).toEqual([]);
  await capture(page, testInfo, "expanded-advanced-menus");
  await page.waitForTimeout(350);
  await menus.first().locator("summary").press("Enter");
  expect(await menus.first().evaluate(element=>element.open)).toBe(false);
  await openFixture(page, "boot-editor", "dark");
  const editor = page.getByRole("region", { name: "Protected Boot Config Editor" });
  await expect(editor).toBeVisible();
  await expect(editor.getByRole("button", { name: "Cancel", exact: true })).toBeVisible();
  await expect(editor.getByText("Required firmware entries remain protected.", { exact: false })).toBeVisible();
  await page.getByRole("combobox", { name: "Fixture screen" }).selectOption("boot-advanced");
  await expect(page.getByRole("textbox", { name: "Advanced boot configuration" })).toBeVisible();
  expect(await layoutAudit(page)).toEqual([]);
  await capture(page, testInfo, "protected-advanced-editor");
  await page.waitForTimeout(350);
});

test("IoT credentials stay masked and failure/pending states cannot resemble verified success", async ({ page }, testInfo) => {
  await openFixture(page, "iot-result", "dark");
  const password = page.locator('input[type="password"]').first();
  await expect(password).toBeVisible();
  await password.fill("Another_Visual_Only_Secret42!");
  await expect(page.locator(".app")).not.toContainText("Another_Visual_Only_Secret42!");
  await expect(page.locator(".app")).not.toContainText("VisualFixture_Only42!");
  await expect(page.locator(".app")).toContainText("pending-reboot");
  await expect(page.getByRole("button", { name: "Apply and verify requested personalization" })).toBeDisabled();
  await capture(page, testInfo, "masked-iot-pending");
  await page.waitForTimeout(350);
  await page.getByRole("combobox", { name: "Fixture screen" }).selectOption("error");
  await expect(page.getByRole("alert")).toContainText("No operation started");
  await expect(page.getByRole("alert")).toContainText(/no drive was written/i);
  await capture(page, testInfo, "explicit-error");
});
