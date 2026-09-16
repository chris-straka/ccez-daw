import { expect, test } from "@playwright/test";
import { ipcCalls, mockTauri } from "./tauri-mock";

/**
 * Automation journey: clicking a lane strip in the Automation panel flows
 * through a real `AutomationPointSet` op to the backend (asserted on the
 * recorded invoke call, not just markup).
 */
test.beforeEach(async ({ page }) => {
  await mockTauri(page);
  await page.goto("/");
});

test("lane click appends an AutomationPointSet op", async ({ page }) => {
  const lane = page.getByTestId("lane-t1:volume");
  await expect(lane).toBeVisible();
  await lane.click({ position: { x: 150, y: 24 } });
  const calls = await ipcCalls(page);
  const op = calls.find((c) => c.cmd === "op_apply");
  expect(op).toBeDefined();
  const payload = (op?.args as { op?: { kind?: string; target?: string } })?.op;
  expect(payload?.kind).toBe("AutomationPointSet");
  expect(payload?.target).toBe("t1:volume");
  await expect(page.getByTestId("automation-status")).toContainText("t1:volume");
});
