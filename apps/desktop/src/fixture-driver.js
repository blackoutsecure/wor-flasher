"use strict";
(() => {
  const scene = document.documentElement.dataset.fixtureScene;
  const root = document.querySelector(".app");
  if (!scene || !root) throw new Error("Missing inert visual-fixture metadata");
  const modes = new Set(["system", "light", "dark"]);
  const appearance = document.getElementById("appearance");
  for (const select of document.querySelectorAll(".app select[value]")) {
    select.value = select.getAttribute("value");
  }
  appearance.value = root.dataset.theme;
  appearance.addEventListener("change", () => {
    if (!modes.has(appearance.value)) throw new Error("Unsupported preview appearance");
    location.assign(`${scene}-${appearance.value}.html`);
  });
  document.getElementById("fixture-scene").addEventListener("change", (event) => {
    const selected = event.target.value;
    if (!/^[a-z-]+$/.test(selected) || !modes.has(root.dataset.theme)) {
      throw new Error("Invalid fixture navigation");
    }
    location.assign(`${selected}-${root.dataset.theme}.html`);
  });
  root.dataset.fixtureReady = "true";
})();
