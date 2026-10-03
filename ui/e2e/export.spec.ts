import { expect, test } from "@playwright/test";
import { ipcCalls, mockTauri } from "./tauri-mock";

/**
 * Normalize is gated on a valid bounce range: an invalid config refuses
 * with the validator message instead of enqueueing an item named after
 * the error.
 */
test.beforeEach(async ({ page }) => {
  await mockTauri(page);
  await page.goto("/");
});

test("normalize with an empty range refuses and enqueues nothing", async ({ page }) => {
  const length = page.getByTestId("export-bounce-length");
  await length.scrollIntoViewIfNeeded();
  await length.fill("0");
  await page.getByTestId("export-measured-lufs").fill("-23");
  await page.getByTestId("export-measured-peak").fill("0.5");
  await page.getByRole("button", { name: "Normalize bounce" }).click();
  await expect(page.getByTestId("export-bounce-status")).toContainText("normalize refused");
  await expect(page.locator('[data-testid^="export-item-"]')).toHaveCount(0);
  const bounces = (await ipcCalls(page)).filter((c) => c.cmd === "op_apply");
  expect(bounces).toHaveLength(0);
});
