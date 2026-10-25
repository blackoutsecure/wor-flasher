import { expect } from "@playwright/test";
import { readFileSync } from "node:fs";
import { unlink } from "node:fs/promises";

export const contract = JSON.parse(readFileSync(new URL("./design-contract.json", import.meta.url), "utf8"));
export const scenes = ["image", "drive", "options", "review", "progress", "result",
  "boot-editor", "boot-advanced", "iot-options", "iot-result", "error"];

export async function openFixture(page, scene, theme) {
  await page.goto(`/${scene}-${theme}.html`);
  await expect(page.locator(".app")).toHaveAttribute("data-fixture-ready", "true");
  await page.evaluate(() => document.fonts.ready);
  await expect(page.getByRole("heading", { name: "WoR-Flasher", exact: true })).toBeVisible();
  const partnership = page.getByRole("group", { name: contract.branding.label, exact: true });
  await expect(partnership).toBeVisible();
  await expect(partnership.getByText("Original creator", { exact: true })).toBeVisible();
  await expect(partnership.getByText("Community maintainer", { exact: true })).toBeVisible();
  await expect.poll(() => page.locator(".partnership-art").evaluate(image => ({
    source: image.getAttribute("src"), width: image.naturalWidth, height: image.naturalHeight,
  }))).toEqual({ source: contract.branding.artwork, width: contract.branding.width, height: contract.branding.height });
}

export async function palette(page, theme) {
  const colors = await page.locator(".app").evaluate(element => {
    const style = getComputedStyle(element);
    const card = element.querySelector(".card");
    const muted = element.querySelector(".muted");
    return {
      canvas: style.backgroundColor, text: style.color,
      surface: getComputedStyle(card).backgroundColor,
      muted: getComputedStyle(muted).color,
      creator: getComputedStyle(element.querySelector(".brand-creator")).color,
      maintainer: getComputedStyle(element.querySelector(".brand-maintainer")).color,
      future: getComputedStyle(element.querySelector(".brand-future")).color,
      accent: getComputedStyle(element.querySelector(".steps .active")).backgroundColor,
    };
  });
  expect(colors).toEqual(contract.themes[theme]);
}

export async function layoutAudit(page) {
  return page.locator(".app").evaluate((root, minimum) => {
    const failures = [];
    const width = document.documentElement.clientWidth;
    if (document.documentElement.scrollWidth > width + 1) failures.push(`Horizontal overflow: ${document.documentElement.scrollWidth} > ${width}`);
    for (const element of root.querySelectorAll("button,input,select,textarea,summary")) {
      if (!element.getClientRects().length) continue;
      const box = element.getBoundingClientRect();
      if (box.left < -1 || box.right > width + 1) failures.push(`Clipped ${element.tagName}: ${element.id || element.textContent.slice(0, 60)}`);
      const target = element.matches('input[type="checkbox"]') ? element.closest("label") : element;
      if (target && target.getBoundingClientRect().height < minimum - .5) failures.push(`Small target: ${element.id || element.tagName}`);
      if (element.matches("input,select,textarea") && !element.getAttribute("aria-label") &&
          !element.getAttribute("aria-labelledby") && !element.labels?.length) failures.push(`Unlabelled control: ${element.id || element.tagName}`);
    }
    return failures;
  }, contract.minimumControlHeight);
}

export async function contrastAudit(page) {
  return page.locator(".app").evaluate((root, limits) => {
    function rgb(value) { return value.match(/[\d.]+/g).map(Number); }
    function luminance(color) {
      const linear = color.slice(0, 3).map(value => value / 255)
        .map(value => value <= .04045 ? value / 12.92 : ((value + .055) / 1.055) ** 2.4);
      return linear[0] * .2126 + linear[1] * .7152 + linear[2] * .0722;
    }
    function background(element) {
      for (let current = element; current; current = current.parentElement) {
        const color = rgb(getComputedStyle(current).backgroundColor);
        if (color.length === 3 || color[3] === 1) return color;
      }
      throw new Error("No opaque application background");
    }
    const failures = [];
    for (const element of root.querySelectorAll("p,label,small,h1,h2,button,summary,pre,footer,.steps li,.brand-creator,.brand-maintainer,.brand-future")) {
      if (!element.getClientRects().length || element.matches(":disabled") ||
          !element.textContent.trim() || getComputedStyle(element).opacity !== "1") continue;
      const style = getComputedStyle(element);
      if (Number.parseFloat(style.fontSize) < limits.minimumTextSize - .1) failures.push(`Small text: ${element.tagName} ${style.fontSize}`);
      const foreground = luminance(rgb(style.color));
      const back = luminance(background(element));
      const ratio = (Math.max(foreground, back) + .05) / (Math.min(foreground, back) + .05);
      const large = Number.parseFloat(style.fontSize) >= 24 ||
        (Number.parseFloat(style.fontSize) >= 18.66 && Number.parseInt(style.fontWeight) >= 700);
      if (ratio + .01 < (large ? 3 : limits.normalTextContrast)) failures.push(`${element.tagName} contrast ${ratio.toFixed(2)}: ${element.textContent.trim().slice(0, 55)}`);
    }
    return failures;
  }, contract);
}

export async function capture(page, testInfo, name) {
  const path = testInfo.outputPath(`${name}.png`);
  await page.locator(".app").screenshot({ path, animations: "disabled" });
  await testInfo.attach(name, { path, contentType: "image/png" });
  // attach() has copied the image into the retained artifact directory.
  await unlink(path);
}
