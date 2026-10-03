import { expect, test } from "@playwright/test";
import { ipcCalls, mockTauri } from "./tauri-mock";

/**
 * Device sliders drag locally and commit one `ParamSet` op on release.
 */
test.beforeEach(async ({ page }) => {
  await mockTauri(page, {
    projectName: "e2e-fixture",
    tracks: [{ id: "t1", name: "Music" }],
    devices: [
      {
        id: "smp1",
        name: "Voice",
        classCode: 7,
        params: [
          { id: "transpose", min: -48, max: 48, value: 0 },
          { id: "gain", min: 0, max: 4, value: 1 },
          { id: "attack", min: 0, max: 10, value: 0.005 },
          { id: "release", min: 0, max: 10, value: 0.1 },
          { id: "cutoff", min: 20, max: 20000, value: 20000 },
        ],
      },
    ],
  });
  await page.goto("/");
});

test("pad strike selects the pad and reports its mapping", async ({ page }) => {
  const pad = page.getByTestId("drumrack-pad-0");
  await pad.scrollIntoViewIfNeeded();
  await pad.click();
  await expect(page.getByTestId("drumrack-status")).toContainText("pad 0 →");
});

test("sampler transpose commits one ParamSet on release", async ({ page }) => {
  const slider = page.getByTestId("sampler-transpose");
  await slider.scrollIntoViewIfNeeded();
  await slider.fill("12");
  const patches = (await ipcCalls(page)).filter((c) => c.cmd === "op_apply");
  const hit = patches.find((c) => JSON.stringify(c.args).includes("smp1:transpose"));
  expect(hit).toBeDefined();
  await expect(page.getByTestId("sampler-status")).toContainText("transpose → 12");
});
