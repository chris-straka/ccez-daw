import { expect, test } from "@playwright/test";
import { ipcCalls, mockTauri } from "./tauri-mock";

/**
 * Score commit persists the working staff bytes through `asset_store`
 * (frozen metadata alone would drop every edit).
 */
test.beforeEach(async ({ page }) => {
  await mockTauri(page, {
    projectName: "e2e-fixture",
    tracks: [{ id: "t1", name: "Music" }],
    clips: [
      {
        id: "clip_b",
        name: "Sketch",
        track_id: "t1",
        start_beats: 0,
        length_beats: 4,
        kind: "Midi",
        source: "",
      },
    ],
  });
  await page.goto("/");
});

test("score commit persists working note bytes via asset_store", async ({ page }) => {
  await page.getByTestId("tab-Score").click();
  await expect(page.getByTestId("score-panel")).toBeVisible();
  await page.getByTestId("score-commit").click();
  await expect(page.getByTestId("score-status")).toContainText("Committed 0 notes");
  const stores = (await ipcCalls(page)).filter((c) => c.cmd === "asset_store");
  expect(stores).toHaveLength(1);
  expect(JSON.stringify(stores[0].args)).toContain("clip_b.mid");
});
