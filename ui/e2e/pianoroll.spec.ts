import { expect, test } from "@playwright/test";
import { ipcCalls, mockTauri } from "./tauri-mock";

/**
 * Piano-roll smoke: clicking empty canvas space click-creates a note.
 *
 * Runs against the isolated fixture (the shell only shows a piano-roll stub
 * panel) — the same `PianoRoll` component, `store.createNote` path, and
 * pointer-event flow as the real editor.
 */
test("piano roll click-creates a note", async ({ page }) => {
  await page.goto("/e2e/fixtures/pianoroll.html");

  // The canvas exposes aria-label="Piano roll" (no landmark role).
  const labeled = page.locator('canvas[aria-label="Piano roll"]');
  await expect(labeled).toBeVisible();

  const count = page.getByTestId("note-count");
  await expect(count).toHaveText("0");

  const box = await labeled.boundingBox();
  expect(box, "canvas has layout size").not.toBeNull();
  // Click empty space mid-canvas: FL-style click-create commits on pointerup.
  await page.mouse.click(box!.x + box!.width / 2, box!.y + box!.height / 2);

  await expect(count).toHaveText("1");
  const data = await page.getByTestId("note-data").textContent();
  const notes = JSON.parse(data ?? "[]") as Array<{ pitch: number; start_beats: number }>;
  expect(notes).toHaveLength(1);
  expect(notes[0].pitch).toBeGreaterThanOrEqual(0);
  expect(notes[0].start_beats).toBeGreaterThanOrEqual(0);
});

test("ctrl+wheel zooms the octave range readout", async ({ page }) => {
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
  await page.getByTestId("tab-Piano roll").click();
  const range = page.getByTestId("piano-range");
  await expect(range).toHaveText("C-1–G9");
  const roll = page.locator('canvas[aria-label="Piano roll"]');
  const box = await roll.boundingBox();
  expect(box, "canvas has layout size").not.toBeNull();
  await page.mouse.move(box!.x + box!.width / 2, box!.y + box!.height / 2);
  await page.keyboard.down("Control");
  await page.mouse.wheel(0, -240);
  await page.keyboard.up("Control");
  await expect(range).not.toHaveText("C-1–G9");
});

test("preview toggles without committing", async ({ page }) => {
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
  await page.getByTestId("tab-Piano roll").click();
  const roll = page.locator('canvas[aria-label="Piano roll"]');
  const box = await roll.boundingBox();
  expect(box, "canvas has layout size").not.toBeNull();
  await page.mouse.click(box!.x + box!.width / 2, box!.y + box!.height / 2);
  await page.getByTestId("piano-preview").click();
  await expect(page.getByTestId("piano-preview")).toHaveText("Stop preview");
  // Stopping early commits nothing: no asset_store call lands.
  await page.getByTestId("piano-preview").click();
  await expect(page.getByTestId("piano-preview")).toHaveText("Preview");
  const stores = (await ipcCalls(page)).filter((c) => c.cmd === "asset_store");
  expect(stores).toHaveLength(0);
});

test("piano commit persists working note bytes via asset_store", async ({ page }) => {
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
  await page.getByTestId("tab-Piano roll").click();
  const roll = page.locator('canvas[aria-label="Piano roll"]');
  await expect(roll).toBeVisible();
  const box = await roll.boundingBox();
  expect(box, "canvas has layout size").not.toBeNull();
  await page.mouse.click(box!.x + box!.width / 2, box!.y + box!.height / 2);
  await page.getByTestId("piano-commit").click();
  await expect(page.getByTestId("piano-status")).toContainText("Committed 1 note");
  const stores = (await ipcCalls(page)).filter((c) => c.cmd === "asset_store");
  expect(stores).toHaveLength(1);
  expect(JSON.stringify(stores[0].args)).toContain("clip_b.mid");
});
