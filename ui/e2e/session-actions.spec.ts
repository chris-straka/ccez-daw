import { expect, test } from "@playwright/test";
import { ipcCalls, mockTauri } from "./tauri-mock";

/**
 * Session/comp/link actions: `S` launches the selected slot, `J` records
 * the pending jam, `C` commits the comp picks, `L` joins the Link clock.
 * Every one used to resolve to an inert descriptor.
 */
test.beforeEach(async ({ page }) => {
  await mockTauri(page, {
    projectName: "actions-fixture",
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
      {
        id: "take_vox_p2",
        name: "Vox p2",
        track_id: "trk_vox",
        start_beats: 0,
        length_beats: 4,
        kind: "Audio",
        source: "take:take_vox_p2",
      },
    ],
  });
  await page.goto("/");
});

test("S launches the selected slot; J records the jam as ClipAdded ops", async ({ page }) => {
  await page.keyboard.press("S");
  await expect(page.getByTestId("jam-status")).toContainText("1 pending");
  await expect(page.getByTestId("session-last-launch")).toContainText("clip_verse_1");

  await page.keyboard.press("J");
  await expect(page.getByTestId("jam-status")).toContainText("recorded");

  const applied = (await ipcCalls(page)).filter((c) => c.cmd === "op_apply");
  // One verse jam event records every clip in the section across tracks.
  expect(applied, "jam-record applied ops").toHaveLength(3);
  for (const c of applied) {
    const op = (c.args as { op: Record<string, unknown> }).op;
    expect(op.kind, "frozen ClipAdded op").toBe("ClipAdded");
  }
});

test("C commits the comp picks as a ClipAdded op", async ({ page }) => {
  await page.getByTestId("tab-Comp").click();
  await expect(page.getByTestId("comp-panel")).toBeVisible();
  await page.getByTestId("comp-track").selectOption("trk_vox");

  await page.keyboard.press("C");
  await expect(page.getByTestId("comp-status")).toContainText("committed clip_comp_trk_vox_0_4");

  const applied = (await ipcCalls(page)).filter((c) => c.cmd === "op_apply");
  expect(applied, "comp commit applied an op").toHaveLength(1);
  const op = (applied[0].args as { op: Record<string, unknown> }).op;
  expect(op.kind, "frozen ClipAdded op").toBe("ClipAdded");
});

test("L joins the Link clock and shows the badge", async ({ page }) => {
  await expect(page.getByTestId("link-badge")).toBeHidden();

  await page.keyboard.press("L");
  await expect(page.getByTestId("link-badge")).toContainText("LINK");
  expect(
    (await ipcCalls(page)).map((c) => c.cmd),
    "L reached the backend",
  ).toContain("link_toggle");

  await page.keyboard.press("L");
  await expect(page.getByTestId("link-badge")).toBeHidden();
});
