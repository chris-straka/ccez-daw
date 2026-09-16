import { expect, test } from "@playwright/test";
import { ipcCalls, mockTauri } from "./tauri-mock";

/**
 * Shell smoke: browser rail, workspace tabs, and right column render
 * against canned backend data.
 *
 * Panel titles are `h2` headings in `App.tsx`; the Timeline rows come from
 * the mocked `project_get`, so the canned track name proves the invoke path
 * (not just static markup) is live. Only the active workspace tab is
 * visible — each tab is opened and asserted in turn.
 */
test.beforeEach(async ({ page }) => {
  await mockTauri(page);
  await page.goto("/");
});

test("shell shows rail, tabs, and default timeline", async ({ page }) => {
  await expect(page.getByRole("heading", { name: "Browser" })).toBeVisible();
  await expect(page.getByRole("heading", { name: "Mixer" })).toBeVisible();
  await expect(page.getByRole("heading", { name: "Command palette" })).toBeVisible();
  for (const tab of ["Timeline", "Session", "Piano roll", "Score", "Automation", "Groove", "Record", "Comp"]) {
    await expect(page.getByTestId(`tab-${tab}`), `${tab} tab visible`).toBeVisible();
  }
  // Default tab is the timeline with the canned project rows.
  await expect(page.getByRole("heading", { name: "Timeline" })).toBeVisible();
});

test("every workspace tab opens its panel", async ({ page }) => {
  const tabs: Record<string, string> = {
    Session: "Session",
    "Piano roll": "Piano roll",
    Score: "Score",
    Automation: "Automation",
    Groove: "Groove",
    Record: "Record",
    Comp: "Comp",
  };
  for (const [tab, heading] of Object.entries(tabs)) {
    await page.getByTestId(`tab-${tab}`).click();
    await expect(page.getByRole("heading", { name: heading }), `${heading} panel visible`).toBeVisible();
  }
  await page.getByTestId("tab-Timeline").click();
  await expect(page.getByRole("heading", { name: "Timeline" })).toBeVisible();
});

test("timeline shows the canned project", async ({ page }) => {
  await expect(page.getByText("e2e-fixture @ 120 BPM")).toBeVisible();
  // TimelineView lanes show track names with clip blocks (vol/pan live in
  // the mixer now, not the lane row; the default canned project has tracks
  // but no clips, so the lane name is the invoke-path proof).
  await expect(page.getByText("Drums", { exact: true }).first()).toBeVisible();
  const calls = await ipcCalls(page);
  expect(calls.map((c) => c.cmd)).toContain("project_get");
});
