import { expect, test } from "@playwright/test";
import { ipcCalls, mockTauri } from "./tauri-mock";

/**
 * Mixer faders: dragging previews locally and a single `ParamSet` op
 * commits on release (no per-pixel backend round-trip mid-gesture).
 */
test.beforeEach(async ({ page }) => {
  await mockTauri(page);
  await page.goto("/");
});

test("volume slider commits one ParamSet on release", async ({ page }) => {
  await page.getByTestId("mixer-vol-t1").fill("1");
  const patches = (await ipcCalls(page)).filter((c) => c.cmd === "op_apply");
  const hit = patches.find((c) => JSON.stringify(c.args).includes("t1:volume"));
  expect(hit).toBeDefined();
  expect(JSON.stringify(hit?.args)).toContain("ParamSet");
});

test("mute button sends a muted ParamSet", async ({ page }) => {
  await page.getByTestId("mixer-mute-t1").click();
  const patches = (await ipcCalls(page)).filter((c) => c.cmd === "op_apply");
  const hit = patches.find((c) => JSON.stringify(c.args).includes("t1:muted"));
  expect(hit).toBeDefined();
  expect(JSON.stringify(hit?.args)).toContain("true");
});

test("pan slider commits one ParamSet on release", async ({ page }) => {
  await page.getByTestId("mixer-pan-t1").fill("-0.5");
  const patches = (await ipcCalls(page)).filter((c) => c.cmd === "op_apply");
  const hit = patches.find((c) => JSON.stringify(c.args).includes("t1:pan"));
  expect(hit).toBeDefined();
});

test.describe("vim strip selection", () => {
  test.beforeEach(async ({ page }) => {
    await mockTauri(page, {
      projectName: "mixer-fixture",
      tracks: [
        { id: "t1", name: "Drums" },
        { id: "t2", name: "Bass" },
      ],
    });
    await page.goto("/");
  });

  test("the strip m / M act on is highlighted and follows j", async ({ page }) => {
    await page.keyboard.press("g");
    await page.keyboard.press("m");
    await expect(page.getByTestId("mixer-strip-t1")).toHaveAttribute("data-selected", "true");
    await page.keyboard.press("j");
    await expect(page.getByTestId("mixer-strip-t2")).toHaveAttribute("data-selected", "true");
    await expect(page.getByTestId("mixer-strip-t1")).not.toHaveAttribute("data-selected", "true");

    await page.keyboard.press("m");
    await expect
      .poll(async () =>
        (await ipcCalls(page)).some((c) => c.cmd === "op_apply" && JSON.stringify(c.args).includes("t2:muted")),
      )
      .toBe(true);
  });
});
