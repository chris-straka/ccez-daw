import { expect, test } from "@playwright/test";
import { emitToPage, ipcCalls, mockTauri } from "./tauri-mock";

/**
 * Native menu items arrive as `menu-action` events carrying registry ids
 * and run exactly like palette actions (same ids, same handlers).
 */
test.beforeEach(async ({ page }) => {
  await mockTauri(page);
  await page.goto("/");
});

test("menu Save behaves like the Save button (typed path)", async ({ page }) => {
  await page.getByTestId("project-path").fill("/tmp/e2e-proj");
  await emitToPage(page, "menu-action", { action: "project.save" });
  const save = (await ipcCalls(page)).find((c) => c.cmd === "project_save");
  expect(save?.args).toMatchObject({ path: "/tmp/e2e-proj" });
});

test("menu Save with no path says so instead of rejecting", async ({ page }) => {
  await emitToPage(page, "menu-action", { action: "project.save" });
  await expect(page.getByTestId("transport-error")).toContainText("save needs a file path");
  const saves = (await ipcCalls(page)).filter((c) => c.cmd === "project_save");
  expect(saves).toHaveLength(0);
});

test("menu Play toggles the transport", async ({ page }) => {
  await emitToPage(page, "menu-action", { action: "transport.play" });
  expect((await ipcCalls(page)).map((c) => c.cmd)).toContain("engine_play");
  await expect(page.getByRole("button", { name: "❚❚" })).toBeVisible();
});

test("menu Open with no path says so instead of rejecting", async ({ page }) => {
  await emitToPage(page, "menu-action", { action: "project.open" });
  await expect(page.getByTestId("transport-error")).toContainText("open needs a file path");
  const opens = (await ipcCalls(page)).filter((c) => c.cmd === "project_open");
  expect(opens).toHaveLength(0);
});

test("menu Add Track auto-names instead of rejecting", async ({ page }) => {
  await emitToPage(page, "menu-action", { action: "track.add" });
  const add = (await ipcCalls(page)).find((c) => c.cmd === "track_add");
  expect(add?.args).toMatchObject({ name: "Track 2" });
  await expect(page.getByTestId("transport-error")).toBeHidden();
});

test("menu Add Clip says what it needs instead of rejecting", async ({ page }) => {
  await emitToPage(page, "menu-action", { action: "clip.add" });
  await expect(page.getByTestId("transport-error")).toContainText("Add Clip needs clip details");
});
