import { expect, test } from "@playwright/test";
import { mockTauri } from "./tauri-mock";

/**
 * Dead-shortcuts cluster: `?` opens the shortcuts editor (Escape closes),
 * and `g m` moves focus to the mixer so its view-scoped keys apply.
 */
test.beforeEach(async ({ page }) => {
  await mockTauri(page);
  await page.goto("/");
});

test("shortcuts editor opens on ? and closes on Escape", async ({ page }) => {
  const dialog = page.getByRole("dialog", { name: "Shortcuts editor" });
  await expect(dialog, "editor closed initially").toBeHidden();

  await page.keyboard.press("?");
  await expect(dialog, "editor opens on ?").toBeVisible();
  await expect(
    page.getByPlaceholder("Search keys or actions…"),
    "search takes focus",
  ).toBeFocused();

  await page.keyboard.press("Escape");
  await expect(dialog, "editor closes on Escape").toBeHidden();
});

test("g m focuses the mixer view", async ({ page }) => {
  await page.keyboard.press("g");
  await page.keyboard.press("m");
  await expect(
    page.getByTestId("mixer-view"),
    "mixer takes focus after g m",
  ).toBeFocused();
});
