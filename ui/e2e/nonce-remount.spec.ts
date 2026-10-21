import { expect, test } from "@playwright/test";
import { ipcCalls, mockTauri } from "./tauri-mock";

/**
 * Bump-and-show actions fire once per press. Workspace panels remount on
 * every tab switch, and a remount used to replay the last handled nonce:
 * clicking back to Record started another take, back to Comp committed
 * again, back to Session relaunched the slot.
 */
test.beforeEach(async ({ page }) => {
  await mockTauri(page, {
    projectName: "nonce-fixture",
    tracks: [
      { id: "trk_drums", name: "Drums" },
      { id: "trk_vox", name: "Vox" },
    ],
    clips: [
      {
        id: "clip_verse_1",
        name: "Verse-1",
        track_id: "trk_drums",
        start_beats: 0,
        length_beats: 8,
        kind: "Audio",
        source: "take:clip_verse_1",
      },
      {
        id: "take_vox_p1",
        name: "Vox p1",
        track_id: "trk_vox",
        start_beats: 0,
        length_beats: 4,
        kind: "Audio",
        source: "take:take_vox_p1",
      },
    ],
  });
  await page.goto("/");
});

async function count(page: import("@playwright/test").Page, cmd: string): Promise<number> {
  return (await ipcCalls(page)).filter((c) => c.cmd === cmd).length;
}

test("returning to Record after r does not punch in again", async ({ page }) => {
  await page.keyboard.press("r");
  await expect(page.getByTestId("record-status")).toContainText("punched in");
  await page.getByTestId("record-punch-out").click();
  await expect(page.getByTestId("record-status")).toContainText("punched out");
  expect(await count(page, "engine_record")).toBe(1);

  await page.getByTestId("tab-Timeline").click();
  await page.getByTestId("tab-Record").click();
  await expect(page.getByTestId("record-panel")).toBeVisible();
  await expect(page.getByTestId("record-status")).not.toContainText("punched in");
  expect(await count(page, "engine_record"), "a tab click records nothing").toBe(1);

  // One more press from another tab toggles exactly once: a replayed nonce
  // on top of the bump would punch straight back out.
  await page.getByTestId("tab-Timeline").click();
  await page.keyboard.press("r");
  await expect(page.getByTestId("record-status")).toContainText("punched in");
});

test("returning to Comp after C does not commit again", async ({ page }) => {
  await page.getByTestId("tab-Comp").click();
  await page.getByTestId("comp-track").selectOption("trk_vox");
  await page.keyboard.press("C");
  await expect(page.getByTestId("comp-status")).toContainText("committed");
  expect(await count(page, "op_apply")).toBe(1);

  await page.getByTestId("tab-Timeline").click();
  await page.getByTestId("tab-Comp").click();
  await expect(page.getByTestId("comp-panel")).toBeVisible();
  expect(await count(page, "op_apply"), "a tab click commits nothing").toBe(1);
});

test("returning to Session after S does not relaunch", async ({ page }) => {
  await page.keyboard.press("S");
  await expect(page.getByTestId("jam-status")).toContainText("1 pending");

  await page.getByTestId("tab-Timeline").click();
  await page.getByTestId("tab-Session").click();
  await expect(page.getByTestId("session-view")).toBeVisible();
  await expect(page.getByTestId("jam-status")).toContainText("0 pending");
});
