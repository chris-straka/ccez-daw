import { expect, test } from "@playwright/test";
import { ipcCalls, mockTauri } from "./tauri-mock";

/**
 * Transport-bar workflow: undo/redo, tempo set, project new/open/save all
 * reach the backend through the mocked bridge (calls asserted, not just
 * markup rendered).
 */
test.beforeEach(async ({ page }) => {
  await mockTauri(page);
  await page.goto("/");
});

test("play button toggles play then stop", async ({ page }) => {
  await page.getByRole("button", { name: "▶" }).click();
  expect((await ipcCalls(page)).map((c) => c.cmd)).toContain("engine_play");
  await expect(page.getByRole("button", { name: "❚❚" })).toBeVisible();
  await page.getByRole("button", { name: "❚❚" }).click();
  expect((await ipcCalls(page)).map((c) => c.cmd)).toContain("engine_stop");
  await expect(page.getByRole("button", { name: "▶" })).toBeVisible();
});

test("space toggles play then stop", async ({ page }) => {
  await page.keyboard.press("Space");
  expect((await ipcCalls(page)).map((c) => c.cmd)).toContain("engine_play");
  await expect(page.getByRole("button", { name: "❚❚" })).toBeVisible();
  await page.keyboard.press("Space");
  expect((await ipcCalls(page)).map((c) => c.cmd)).toContain("engine_stop");
  await expect(page.getByRole("button", { name: "▶" })).toBeVisible();
});

test("undo button appends op_undo", async ({ page }) => {
  await page.getByTestId("undo-btn").click();
  const calls = await ipcCalls(page);
  expect(calls.map((c) => c.cmd)).toContain("op_undo");
});

test("tempo set sends the typed BPM", async ({ page }) => {
  await page.getByTestId("tempo-input").fill("140");
  await page.getByTestId("tempo-set").click();
  const calls = await ipcCalls(page);
  const tempo = calls.find((c) => c.cmd === "engine_set_tempo");
  expect(tempo).toBeDefined();
  expect(tempo?.args).toMatchObject({ tempo: 140 });
});

test("save sends the typed path", async ({ page }) => {
  await page.getByTestId("project-path").fill("/tmp/e2e-proj");
  await page.getByTestId("project-save").click();
  const calls = await ipcCalls(page);
  const save = calls.find((c) => c.cmd === "project_save");
  expect(save).toBeDefined();
  expect(save?.args).toMatchObject({ path: "/tmp/e2e-proj" });
});

test("new sends the typed name", async ({ page }) => {
  await page.getByTestId("project-name").fill("e2e-song");
  await page.getByTestId("project-new").click();
  const calls = await ipcCalls(page);
  const created = calls.find((c) => c.cmd === "project_new");
  expect(created).toBeDefined();
  expect(created?.args).toMatchObject({ name: "e2e-song" });
});
