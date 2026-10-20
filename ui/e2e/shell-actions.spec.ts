import { expect, test } from "@playwright/test";
import { ipcCalls, mockTauri } from "./tauri-mock";

/**
 * Palette rows, native menu items, and keys all run through the shell's
 * `runAction`, so they behave like the buttons they mirror: Play flips the
 * header, New / Set Tempo / Add Track carry their arguments. Bridge
 * failures land in the status line instead of the console.
 */
async function palette(page: import("@playwright/test").Page, title: string): Promise<void> {
  await page.keyboard.press("Control+k");
  await page.getByPlaceholder("Type a command…").fill(title);
  await page.keyboard.press("Enter");
}

test.describe("with the bridge", () => {
  test.beforeEach(async ({ page }) => {
    await mockTauri(page);
    await page.goto("/");
  });

  test("palette Play flips the header and Space then stops", async ({ page }) => {
    await palette(page, "Transport: Play");
    await expect(page.getByRole("button", { name: "❚❚" })).toBeVisible();
    await page.keyboard.press("Space");
    await expect(page.getByRole("button", { name: "▶" })).toBeVisible();
    expect((await ipcCalls(page)).map((c) => c.cmd).filter((c) => c.startsWith("engine_"))).toEqual(
      expect.arrayContaining(["engine_play", "engine_stop"]),
    );
  });

  test("palette Add Track sends an auto-name", async ({ page }) => {
    await palette(page, "Track: Add");
    const add = (await ipcCalls(page)).find((c) => c.cmd === "track_add");
    expect(add?.args).toMatchObject({ name: expect.stringMatching(/^Track \d+$/) });
    await expect(page.getByTestId("transport-error")).toBeHidden();
  });

  test("palette New and Set Tempo use the typed fields", async ({ page }) => {
    await page.getByTestId("project-name").fill("palette-song");
    await page.getByTestId("tempo-input").fill("132");
    await page.locator("body").click();
    await palette(page, "Project: New");
    await palette(page, "Engine: Set Tempo");
    const calls = await ipcCalls(page);
    expect(calls.find((c) => c.cmd === "project_new")?.args).toMatchObject({ name: "palette-song" });
    expect(calls.find((c) => c.cmd === "engine_set_tempo")?.args).toMatchObject({ tempo: 132 });
    await expect(page.getByTestId("transport-error")).toBeHidden();
  });

  test("palette Vim motions move the cursor like their keys", async ({ page }) => {
    await expect(page.getByTestId("vim-cursor")).toHaveText("0:0");
    await palette(page, "Vim: Move Down");
    await expect(page.getByTestId("vim-cursor")).toHaveText("1:0");
    await palette(page, "Vim: Move Right");
    await expect(page.getByTestId("vim-cursor")).toHaveText("1:1");
  });

  test("global keys stay quiet while the Shortcuts editor is open", async ({ page }) => {
    await page.keyboard.press("?");
    const editor = page.getByRole("dialog", { name: "Shortcuts editor" });
    await expect(editor).toBeVisible();
    // Focus a button, not the search box: inputs already swallow keys.
    await editor.getByRole("button", { name: "Reset to defaults" }).click();
    await page.keyboard.press("Space");
    await page.keyboard.press("r");
    const cmds = (await ipcCalls(page)).map((c) => c.cmd);
    expect(cmds).not.toContain("engine_play");
    expect(cmds).not.toContain("engine_record");
  });
});

test.describe("without the bridge", () => {
  test("transport and project buttons report in the status line, never throw", async ({ page }) => {
    const errors: string[] = [];
    page.on("pageerror", (e) => errors.push(e.message));
    await page.goto("/");
    await expect(page.getByTestId("transport-error")).toContainText("Tauri bridge unavailable");

    await page.getByRole("button", { name: "▶" }).click();
    await expect(page.getByTestId("transport-error")).toContainText("play failed");
    await page.getByTestId("tempo-set").click();
    await page.keyboard.press("Space");
    await expect(page.getByTestId("transport-error"), "Space reports like the button").toContainText(
      "play failed",
    );
    await page.getByTestId("tempo-set").click();
    await expect(page.getByTestId("transport-error")).toContainText("set tempo failed");
    await page.getByTestId("project-new").click();
    await expect(page.getByTestId("transport-error")).toContainText("new project failed");
    await page.getByTestId("project-path").fill("/tmp/x.ccez");
    await page.getByTestId("project-save").click();
    await expect(page.getByTestId("transport-error")).toContainText("save failed");
    await page.getByTestId("project-open").click();
    await expect(page.getByTestId("transport-error")).toContainText("open failed");
    expect(errors, "no uncaught rejections").toEqual([]);
  });
});
