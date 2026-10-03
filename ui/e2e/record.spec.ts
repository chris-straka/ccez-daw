import { expect, test } from "@playwright/test";
import { ipcCalls, mockTauri } from "./tauri-mock";

/**
 * Record path: arm a track, punch a pass on the rolling Recording
 * transport, and commit the take as a frozen ClipAdded op. The `r` action
 * punches in from any tab.
 */
test.beforeEach(async ({ page }) => {
  await mockTauri(page, {
    projectName: "record-fixture",
    tracks: [{ id: "trk_vox", name: "Vox" }],
    clips: [],
  });
  await page.goto("/");
});

test("arm, punch in/out, and commit cuts a take clip", async ({ page }) => {
  await page.getByTestId("tab-Record").click();
  await expect(page.getByTestId("record-panel")).toBeVisible();

  await page.getByTestId("record-arm-trk_vox").click();
  await expect(page.getByTestId("record-status")).toContainText("armed");

  await page.getByTestId("record-punch-in").click();
  await expect(page.getByTestId("record-status")).toContainText("punched in");
  expect(
    (await ipcCalls(page)).map((c) => c.cmd),
    "punch-in rolls the Recording transport",
  ).toContain("engine_record");

  await page.getByTestId("record-punch-out").click();
  await expect(page.getByTestId("record-status")).toContainText("punched out");

  await page.getByTestId("record-commit").click();
  await expect(page.getByTestId("record-status")).toContainText("take_trk_vox_0_1");

  const applied = (await ipcCalls(page)).filter((c) => c.cmd === "op_apply");
  expect(applied, "take commit applied an op").toHaveLength(1);
  const op = (applied[0].args as { op: Record<string, unknown> }).op;
  expect(op.kind, "frozen ClipAdded op").toBe("ClipAdded");
});

test("r punches in from the Timeline tab", async ({ page }) => {
  await page.getByTestId("tab-Record").click();
  await page.getByTestId("record-arm-trk_vox").click();
  await page.getByTestId("tab-Timeline").click();

  await page.keyboard.press("r");
  await expect(page.getByTestId("record-status")).toContainText("punched in");
  expect(
    (await ipcCalls(page)).map((c) => c.cmd),
    "r rolls the Recording transport",
  ).toContain("engine_record");
});
