import { expect, test } from "@playwright/test";
import { ipcCalls, mockTauri } from "./tauri-mock";

/**
 * Timeline clips drag horizontally on the linear lanes: the chip follows
 * the pointer and one frozen `ClipMoved` op commits on release.
 */
test.beforeEach(async ({ page }) => {
  await mockTauri(page, {
    projectName: "e2e-fixture",
    tracks: [{ id: "t1", name: "Music" }],
    clips: [
      {
        id: "clip_a",
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

test("dragging a clip commits one ClipMoved op", async ({ page }) => {
  const chip = page.getByTestId("clip-clip_a");
  await expect(chip).toBeVisible();
  const box = await chip.boundingBox();
  expect(box, "chip has layout size").not.toBeNull();
  const x = box!.x + box!.width / 2;
  const y = box!.y + box!.height / 2;
  await page.mouse.move(x, y);
  await page.mouse.down();
  // 40px at 4px/beat = +10 beats (1-beat snap).
  await page.mouse.move(x + 40, y, { steps: 5 });
  await page.mouse.up();
  await expect(page.getByTestId("timeline-status")).toContainText("Moved clip_a → beat 10");
  const moves = (await ipcCalls(page)).filter((c) => c.cmd === "op_apply");
  const hit = moves.find((c) => JSON.stringify(c.args).includes("ClipMoved"));
  expect(hit).toBeDefined();
  const args = hit?.args as { op?: { kind?: string; target?: string; value_json?: string } };
  expect(args.op?.kind).toBe("ClipMoved");
  expect(args.op?.target).toBe("clip_a");
  expect(JSON.parse(args.op?.value_json ?? "{}")).toMatchObject({ startBeats: 10 });
});
