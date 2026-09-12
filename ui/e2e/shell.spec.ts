import { expect, test } from "@playwright/test";
import { ipcCalls, mockTauri } from "./tauri-mock";

/**
 * Shell smoke: the five regions render against canned backend data.
 *
 * Panel titles are `h2` headings in `App.tsx`; the Timeline/Mixer rows come
 * from the mocked `project_get`, so the canned track name proves the
 * invoke path (not just static markup) is live.
 */
test.beforeEach(async ({ page }) => {
  await mockTauri(page);
  await page.goto("/");
});

test("shell loads all five regions", async ({ page }) => {
  for (const name of ["Timeline", "Mixer", "Browser", "Piano roll", "Command palette"]) {
    await expect(page.getByRole("heading", { name }), `${name} region visible`).toBeVisible();
  }
});

test("timeline shows the canned project", async ({ page }) => {
  await expect(page.getByText("e2e-fixture @ 120 BPM")).toBeVisible();
  await expect(page.getByText("Drums — vol 0.8 pan 0")).toBeVisible();
  const calls = await ipcCalls(page);
  expect(calls.map((c) => c.cmd)).toContain("project_get");
});
