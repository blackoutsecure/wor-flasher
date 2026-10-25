import { test, expect } from "@playwright/test";
import { capture, contrastAudit, layoutAudit, openFixture, palette, scenes } from "./helpers.mjs";

for (const theme of ["light", "dark"]) {
  for (const width of [320, 375, 768, 1280, 1920]) {
    test(`${theme} layout and contrast at ${width}px across every rendered screen`, async ({ page }, testInfo) => {
      await page.setViewportSize({ width, height: 900 });
      for (const scene of scenes) {
        await openFixture(page, scene, theme);
        await palette(page, theme);
        expect(await layoutAudit(page), `${scene} layout`).toEqual([]);
        expect(await contrastAudit(page), `${scene} contrast`).toEqual([]);
        await expect(page.locator('[aria-current="step"]')).toHaveCount(1);
        if ([375, 1280].includes(width)) await capture(page, testInfo, `${scene}-${theme}-${width}`);
      }
    });
  }
}

test("keyboard focus, zoom-sized viewport and high-density rendering remain usable", async ({ browser, baseURL }) => {
  const context = await browser.newContext({
    baseURL, viewport: { width: 640, height: 480 }, deviceScaleFactor: 2, reducedMotion: "reduce",
  });
  try {
    const page = await context.newPage();
    await openFixture(page, "image", "dark");
    expect(await page.evaluate(() => window.devicePixelRatio)).toBe(2);
    await page.keyboard.press("Tab");
    await page.keyboard.press("Tab");
    const focus = page.locator(":focus");
    await expect(focus).toBeVisible();
    expect(await focus.evaluate(element => getComputedStyle(element).outlineStyle)).not.toBe("none");
    expect(await layoutAudit(page)).toEqual([]);
  } finally {
    await context.close();
  }
});
