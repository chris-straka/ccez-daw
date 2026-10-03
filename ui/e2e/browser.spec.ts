import { expect, test } from "@playwright/test";
import { mockTauri } from "./tauri-mock";

/**
 * Browser picks never fail silently: presets apply to a matching device
 * when one exists, everything else says what happened in one line.
 */
test.beforeEach(async ({ page }) => {
  await mockTauri(page);
  await page.goto("/");
});

test("picking a preset with no matching device says so", async ({ page }) => {
  await page.locator('input[aria-label="Browser search"]').fill("Casino Random");
  await page.getByTestId("browser-row-arp_casino_random").click();
  await expect(page.getByTestId("browser-note")).toContainText(
    "No arpeggiator device in this project.",
  );
});

test("picking a sample says it carries no audio", async ({ page }) => {
  await page.locator('input[aria-label="Browser search"]').fill("Boomy 808 Kick");
  await page.getByTestId("browser-row-sample_boomy_kick").click();
  await expect(page.getByTestId("browser-note")).toContainText("no audio");
});
