import { expect, test } from "@playwright/test";

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
