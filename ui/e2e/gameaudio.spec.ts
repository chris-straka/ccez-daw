import { expect, test } from "@playwright/test";

/**
 * Game-audio simulator smoke: posting a state updates the audition room.
 *
 * Runs against the isolated Audition fixture (demo cue/bank/params from
 * `gameaudio/sample.ts`). Clicking the `combat` state button must log the
 * `explore -> combat` Fade transition and flip the audible-layer readout
 * from the explore bed to the combat stack.
 */
test("game-audio simulator sets a state", async ({ page }) => {
  await page.goto("/e2e/fixtures/audition.html");

  await expect(page.getByText(/audible:/)).toContainText("bed");
  await page.getByRole("button", { name: "combat", exact: true }).click();

  // Transition log proves postState ran with the authored Fade rule.
  await expect(page.getByText(/explore -> combat: Fade/)).toBeVisible();
  // Combat stack (drums + brass over the bed) is now audible.
  await expect(page.getByText(/audible:/)).toContainText("drums");
});
