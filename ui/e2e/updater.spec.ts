import { expect, test } from "@playwright/test";
import { emitToPage, ipcCalls, mockTauri } from "./tauri-mock";

/**
 * The updater is reachable: palette and native menu both ask the backend
 * (`app_update_check`) and the status line shows its answer.
 */
test.beforeEach(async ({ page }) => {
  await mockTauri(page);
  await page.goto("/");
});

test("palette Check for Updates asks the release feed and shows the answer", async ({ page }) => {
  await page.keyboard.press("Control+k");
  await page.getByPlaceholder("Type a command…").fill("App: Check for Updates");
  await page.keyboard.press("Enter");
  await expect(page.getByTestId("transport-error")).toContainText("update: 0.1.2 available");
  expect((await ipcCalls(page)).map((c) => c.cmd)).toContain("app_update_check");
});

test("menu Check for Updates reaches the same command", async ({ page }) => {
  await emitToPage(page, "menu-action", { action: "app.update.check" });
  await expect(page.getByTestId("transport-error")).toContainText("update: 0.1.2 available");
});

test("palette Install Update reports the backend's status", async ({ page }) => {
  await page.keyboard.press("Control+k");
  await page.getByPlaceholder("Type a command…").fill("App: Install Update");
  await page.keyboard.press("Enter");
  await expect(page.getByTestId("transport-error")).toContainText("update: up to date (0.1.2)");
  expect((await ipcCalls(page)).map((c) => c.cmd)).toContain("app_update_install");
});
