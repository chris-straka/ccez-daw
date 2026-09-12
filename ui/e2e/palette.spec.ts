import { expect, test } from "@playwright/test";
import { ipcCalls, mockTauri } from "./tauri-mock";

/**
 * Palette smoke: Ctrl+K opens the command palette and running an action
 * dispatches through the registry to the (mocked) backend.
 */
test.beforeEach(async ({ page }) => {
  await mockTauri(page);
  await page.goto("/");
});

test("palette opens via shortcut and runs an action", async ({ page }) => {
  const dialog = page.getByRole("dialog", { name: "Command palette" });
  await expect(dialog, "palette closed initially").toBeHidden();

  await page.keyboard.press("Control+k");
  await expect(dialog, "palette opens on Ctrl+K").toBeVisible();

  const input = page.getByPlaceholder("Type a command…");
  await expect(input).toBeFocused();
  await input.fill("Transport: Play");
  await expect(page.getByRole("button", { name: /Transport: Play/ })).toBeVisible();

  await page.keyboard.press("Enter");
  await expect(dialog, "palette closes after running").toBeHidden();

  const calls = await ipcCalls(page);
  expect(
    calls.map((c) => c.cmd),
    "transport.play reached the backend",
  ).toContain("engine_play");
});
