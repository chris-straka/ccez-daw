import { expect, test } from "@playwright/test";
import { ipcCalls, mockTauri } from "./tauri-mock";

/**
 * Groove tab works on the project's own MIDI clips: pick a target, extract
 * a feel, and `groove.apply` (palette / Clip menu) commits a grooved copy
 * as a ClipAdded op with its notes saved under the copy's key. The pool
 * outlives the tab (the panel remounts on every switch).
 */
test.beforeEach(async ({ page }) => {
  await mockTauri(page, {
    projectName: "groove-fixture",
    tracks: [{ id: "t1", name: "Keys" }],
    clips: [
      { id: "clip_a", name: "Verse", track_id: "t1", start_beats: 0, length_beats: 4, kind: "Midi", source: "" },
      { id: "clip_b", name: "Chorus", track_id: "t1", start_beats: 4, length_beats: 4, kind: "Midi", source: "" },
    ],
  });
  await page.goto("/");
  await page.getByTestId("tab-Groove").click();
  await expect(page.getByTestId("groove-panel")).toBeVisible();
});

async function extract(page: import("@playwright/test").Page, name: string): Promise<void> {
  await page.getByTestId("groove-name").fill(name);
  await page.getByTestId("groove-extract").click();
  await expect(page.getByTestId("groove-status")).toContainText(`kept “${name}”`);
}

test("the pool survives a tab switch", async ({ page }) => {
  await extract(page, "late");
  await page.getByTestId("tab-Timeline").click();
  await page.getByTestId("tab-Groove").click();
  await expect(page.getByTestId("groove-delete-late")).toBeVisible();
});

test("groove.apply from the palette commits a grooved copy of the picked clip", async ({ page }) => {
  await page.getByTestId("groove-target").selectOption("clip_b");
  await extract(page, "late");
  await page.keyboard.press("Control+k");
  await page.getByPlaceholder("Type a command…").fill("Groove: Apply Template to Clip");
  await page.keyboard.press("Enter");
  await expect(page.getByTestId("groove-status")).toContainText("applied “late” to Chorus");

  const calls = await ipcCalls(page);
  const store = calls.find((c) => c.cmd === "asset_store");
  expect(store?.args).toMatchObject({ key: "groove:late+clip_b.mid", kind: "midi" });
  const applied = calls.filter((c) => c.cmd === "op_apply");
  expect(applied).toHaveLength(1);
  const op = (applied[0].args as { op: { kind: string; target: string } }).op;
  expect(op.kind).toBe("ClipAdded");
  expect(op.target).toBe("clip_b~groove-late");
});

test("with no MIDI clips the tab says so", async ({ page }) => {
  await mockTauri(page, { projectName: "empty", tracks: [{ id: "t1", name: "Keys" }], clips: [] });
  await page.goto("/");
  await page.getByTestId("tab-Groove").click();
  await expect(page.getByTestId("groove-panel")).toContainText("No MIDI clips");
});
