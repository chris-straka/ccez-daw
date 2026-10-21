import { expect, test } from "@playwright/test";
import { emitToPage, mockTauri } from "./tauri-mock";

/**
 * Palette Edit rows act on the last text field through the clipboard and
 * report what happened. The native Edit menu's OS roles already did the
 * work, so their `menu-action` sync event must not do it twice.
 */
test.beforeEach(async ({ page, context }) => {
  await context.grantPermissions(["clipboard-read", "clipboard-write"]);
  await mockTauri(page);
  await page.goto("/");
});

async function palette(page: import("@playwright/test").Page, title: string): Promise<void> {
  await page.getByTestId("palette-open").click();
  await page.getByPlaceholder("Type a command…").fill(title);
  await page.keyboard.press("Enter");
}

test("palette Select All, Copy, then Paste into another field", async ({ page }) => {
  await page.getByTestId("project-name").fill("hello world");
  await palette(page, "Edit: Select All");
  await expect(page.getByTestId("transport-error")).toHaveText("edit: selected 11 chars in project-name");
  await palette(page, "Edit: Copy");
  await expect(page.getByTestId("transport-error")).toHaveText("edit: copied 11 chars");

  await page.getByTestId("project-path").fill("/tmp/");
  await palette(page, "Edit: Paste");
  await expect(page.getByTestId("transport-error")).toHaveText("edit: pasted 11 chars into project-path");
  await expect(page.getByTestId("project-path")).toHaveValue("/tmp/hello world");
});

test("palette Copy with no text field says so", async ({ page }) => {
  await palette(page, "Edit: Copy");
  await expect(page.getByTestId("transport-error")).toHaveText("edit: copy needs a text field — click one first");
});

test("the native Edit menu's sync event does not paste a second time", async ({ page }) => {
  await page.evaluate(() => navigator.clipboard.writeText("x"));
  await page.getByTestId("project-path").fill("/tmp/");
  await emitToPage(page, "menu-action", { action: "edit.paste" });
  await page.waitForTimeout(200);
  await expect(page.getByTestId("project-path")).toHaveValue("/tmp/");
});
