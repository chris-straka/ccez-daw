import { expect, test } from "@playwright/test";
import { ipcCalls, mockTauri } from "./tauri-mock";

/**
 * WAV import: a PCM16-mono file stores as an `audio` asset and lands as an
 * `asset:` Audio clip through a frozen ClipAdded op. Non-WAV files are
 * refused before anything is stored.
 */
test.beforeEach(async ({ page }) => {
  await mockTauri(page);
  await page.goto("/");
});

/** 800 frames of PCM16 mono silence at 8 kHz with a valid header. */
function wavBuffer(frames = 800): Buffer {
  const dataLen = frames * 2;
  const buf = Buffer.alloc(44 + dataLen);
  buf.write("RIFF", 0);
  buf.writeUInt32LE(36 + dataLen, 4);
  buf.write("WAVE", 8);
  buf.write("fmt ", 12);
  buf.writeUInt32LE(16, 16);
  buf.writeUInt16LE(1, 20);
  buf.writeUInt16LE(1, 22);
  buf.writeUInt32LE(8000, 24);
  buf.writeUInt32LE(16000, 28);
  buf.writeUInt16LE(2, 32);
  buf.writeUInt16LE(16, 34);
  buf.write("data", 36);
  buf.writeUInt32LE(dataLen, 40);
  return buf;
}

test("importing a WAV stores the asset and places an asset: clip", async ({ page }) => {
  await page.getByTestId("tab-Timeline").click();
  await expect(page.getByTestId("timeline-view")).toBeVisible();

  await page
    .getByTestId("timeline-import-file")
    .setInputFiles({ name: "hit.wav", mimeType: "audio/wav", buffer: wavBuffer() });
  await expect(page.getByTestId("timeline-status")).toContainText("imported hit.wav");

  const calls = await ipcCalls(page);
  const stored = calls.find((c) => c.cmd === "asset_store");
  expect(stored, "bytes stored as an audio asset").toBeDefined();
  expect((stored!.args as { kind: string }).kind).toBe("audio");

  const applied = calls.filter((c) => c.cmd === "op_apply");
  expect(applied, "import applied an op").toHaveLength(1);
  const op = (applied[0]!.args as { op: Record<string, unknown> }).op;
  expect(op.kind, "frozen ClipAdded op").toBe("ClipAdded");
  const clip = JSON.parse(op.value_json as string) as { kind: string; source: string };
  expect(clip.kind).toBe("Audio");
  expect(clip.source).toBe("asset:hit.wav");
});

test("a non-WAV file is refused and nothing is stored", async ({ page }) => {
  await page.getByTestId("tab-Timeline").click();

  await page
    .getByTestId("timeline-import-file")
    .setInputFiles({ name: "notes.txt", mimeType: "text/plain", buffer: Buffer.from("hello") });
  await expect(page.getByTestId("timeline-status")).toContainText("import refused");

  const calls = await ipcCalls(page);
  expect(
    calls.filter((c) => c.cmd === "asset_store"),
    "no bytes stored",
  ).toHaveLength(0);
  expect(
    calls.filter((c) => c.cmd === "op_apply"),
    "no op applied",
  ).toHaveLength(0);
});
