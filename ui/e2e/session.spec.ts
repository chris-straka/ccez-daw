import { expect, test } from "@playwright/test";
import { ipcCalls, mockTauri } from "./tauri-mock";

/**
 * Session View smoke: clip launch, scene launch, and jam-record.
 *
 * Runs against the live shell with a canned project whose clips overlap the
 * demo Verse (`0..16`) / Chorus (`16..32`) sections from `sampleTimelineDoc`,
 * so the same `planSlotLaunch` / `planSceneLaunch` / `jamRecordOps` path as
 * the real launcher fires. Jam-record must write frozen `ClipAdded` ops
 * through the existing `op_apply` surface (no new IPC).
 */
test.beforeEach(async ({ page }) => {
  await mockTauri(page, {
    projectName: "session-fixture",
    tracks: [
      { id: "trk_drums", name: "Drums" },
      { id: "trk_bass", name: "Bass" },
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
        id: "clip_chorus_1",
        name: "Chorus-1",
        track_id: "trk_drums",
        start_beats: 16,
        length_beats: 8,
        kind: "Midi",
        source: "take:clip_chorus_1",
      },
    ],
  });
  await page.goto("/");
});

test("session view launches a clip, a scene, and jam-records", async ({ page }) => {
  await page.getByTestId("tab-Session").click();
  const session = page.getByTestId("session-view");
  await expect(session, "session panel visible").toBeVisible();
  await expect(page.getByTestId("session-quant")).toBeVisible();

  // Launch one clip slot: the plan readout names the triggered clip and the
  // press joins the pending jam.
  await page.getByTestId("slot-launch-sec_verse-trk_drums").click();
  await expect(page.getByTestId("session-last-launch")).toContainText("clip_verse_1");
  await expect(page.getByTestId("jam-status")).toContainText("1 pending");

  // Launch a whole scene: the pending jam grows to two events.
  await page.getByTestId("scene-launch-sec_chorus").click();
  await expect(page.getByTestId("jam-status")).toContainText("2 pending");

  // Jam-record writes frozen ClipAdded ops through the existing op surface.
  await page.getByTestId("jam-record").click();
  await expect(page.getByTestId("jam-status")).toContainText("recorded 2 clips");

  const calls = await ipcCalls(page);
  const applied = calls.filter((c) => c.cmd === "op_apply");
  expect(applied, "jam-record applied ops").toHaveLength(2);
  for (const c of applied) {
    const op = (c.args as { op: Record<string, unknown> }).op;
    expect(op.kind, "frozen ClipAdded op").toBe("ClipAdded");
    expect(op.seq, "seq placeholder the engine replaces").toBe(0);
  }
});

test("session view is reachable from the palette", async ({ page }) => {
  await page.keyboard.press("Control+k");
  const dialog = page.getByRole("dialog", { name: "Command palette" });
  await expect(dialog).toBeVisible();
  await page.getByPlaceholder("Type a command…").fill("Focus Session");
  await expect(page.getByRole("button", { name: /Focus Session/ })).toBeVisible();
  await page.keyboard.press("Enter");
  await expect(dialog, "palette closes after running").toBeHidden();
  await expect(page.getByTestId("session-view")).toBeFocused();
});
