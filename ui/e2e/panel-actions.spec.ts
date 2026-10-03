import { expect, test } from "@playwright/test";
import { mockTauri } from "./tauri-mock";

/**
 * The last silent actions: `groove.apply` and `branch.merge` reach their
 * panels' real commit paths, and guidance actions explain themselves in
 * the palette instead of closing on an inert descriptor.
 */
test.beforeEach(async ({ page }) => {
  await mockTauri(page);
  await page.goto("/");
});

async function runPalette(page: import("@playwright/test").Page, text: string) {
  await page.keyboard.press("Control+k");
  await page.getByPlaceholder("Type a command…").fill(text);
  await page.keyboard.press("Enter");
}

test("palette Groove Apply reaches the panel (empty pool says so)", async ({ page }) => {
  await runPalette(page, "Groove: Apply");
  await expect(page.getByTestId("groove-status")).toContainText("pool is empty");
});

test("palette Branch Merge reaches the panel (nothing to merge)", async ({ page }) => {
  await runPalette(page, "Branch: Merge");
  await expect(page.getByTestId("branch-status")).toContainText("nothing to merge");
});

test("palette Clip Delete explains itself and stays open", async ({ page }) => {
  await runPalette(page, "Clip: Delete");
  const dialog = page.getByRole("dialog", { name: "Command palette" });
  await expect(dialog, "palette stays open on guidance").toBeVisible();
  await expect(dialog).toContainText("needs a selected clip");
});
