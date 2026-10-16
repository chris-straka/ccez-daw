import { expect, test } from "@playwright/test";
import { ipcCalls, mockTauri } from "./tauri-mock";

/**
 * A game-music composition opens in the DAW: the path field + "Open
 * composition" (or the palette's "Composition: Open in the DAW") runs
 * `compose_open`, the timeline shows its rendered and MIDI tracks laid out
 * by section, the status line says what opened, and the piano roll edits
 * the notes. The canned project mirrors what `core::compose::daw` writes.
 */
const note = (id: number, pitch: number, start: number, len: number) => ({
  note_id: id,
  pitch,
  velocity: 100,
  start_beats: start,
  len_beats: len,
  channel: 0,
  probability: 1,
  ratchets: 1,
  timing_offset_beats: 0,
  pitch_bend: 0,
  pressure: 0,
  timbre: 0,
  muted: false,
});

test.beforeEach(async ({ page }) => {
  await mockTauri(page, {
    projectName: "e2e-fixture",
    tracks: [{ id: "t1", name: "Drums" }],
    composition: {
      name: "island_travel",
      tempo: 104,
      tracks: [
        { id: "trk_audio_calm", name: "calm (rendered, intensity 1+)", muted: false },
        { id: "trk_midi_calm_clarinet", name: "calm · clarinet (clarinet)", muted: true },
      ],
      clips: [
        {
          id: "clip_travel_calm_audio",
          name: "travel · calm",
          track_id: "trk_audio_calm",
          start_beats: 0,
          length_beats: 96,
          kind: "Audio",
          source: "asset:stem_travel_calm.wav",
        },
        {
          id: "clip_travel_calm_clarinet",
          name: "travel · clarinet",
          track_id: "trk_midi_calm_clarinet",
          start_beats: 0,
          length_beats: 96,
          kind: "Midi",
          source: "midi_travel_calm_clarinet.json",
        },
      ],
    },
    assets: {
      "midi_travel_calm_clarinet.json": JSON.stringify({
        length_beats: 96,
        notes: [note(1, 78, 0, 0.75), note(2, 76, 0.75, 0.25), note(3, 74, 1, 0.5), note(4, 69, 1.5, 0.5)],
      }),
    },
  });
  await page.goto("/");
});

test("a composition opens on the timeline and in the piano roll", async ({ page }) => {
  await page.getByTestId("project-path").fill("/music/island_travel.composition.json");
  await page.getByTestId("composition-open").click();

  await expect(page.getByTestId("composition-status")).toHaveText(
    "composition: opened island_travel → 2 tracks (1 rendered clips, 1 MIDI clips) at 104 BPM",
  );
  await expect(page.getByTestId("tempo-input")).toHaveValue("104");
  const calls = await ipcCalls(page);
  expect(calls.find((c) => c.cmd === "compose_open")?.args).toEqual({
    path: "/music/island_travel.composition.json",
  });
  // Timeline: the rendered stem and the notes, side by side.
  await expect(page.getByTestId("clip-clip_travel_calm_audio")).toBeVisible();
  await expect(page.getByTestId("clip-clip_travel_calm_clarinet")).toBeVisible();

  // Piano roll: the MIDI clip loads its notes (no empty-clip hint).
  await page.getByTestId("tab-Piano roll").click();
  await page.getByTestId("piano-clip").selectOption("clip_travel_calm_clarinet");
  await expect(page.locator('canvas[aria-label="Piano roll"]')).toBeVisible();
  await expect(page.locator(".piano-empty-hint")).toHaveCount(0);
  await expect(page.getByTestId("piano-range")).toBeVisible();
});

test("the palette opens a composition from the path field, and a bad path says why", async ({ page }) => {
  await page.getByTestId("project-path").fill("/music/notes.txt");
  await page.getByTestId("composition-open").click();
  await expect(page.getByTestId("transport-error")).toContainText("open composition failed");
  await expect(page.getByTestId("composition-status")).toHaveCount(0);

  await page.getByTestId("project-path").fill("/music/island_travel.composition.json");
  // Shortcuts don't fire while typing in a field.
  await page.getByTestId("project-path").blur();
  await page.keyboard.press("Control+k");
  const input = page.getByPlaceholder("Type a command…");
  await input.fill("Composition: Open in the DAW");
  await page.keyboard.press("Enter");
  await expect(page.getByTestId("composition-status")).toContainText("opened island_travel");
  const opens = (await ipcCalls(page)).filter((c) => c.cmd === "compose_open");
  expect(opens).toHaveLength(2);
});
