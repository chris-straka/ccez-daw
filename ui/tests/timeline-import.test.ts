import { describe, expect, test } from "bun:test";
import {
  beatsForFrames,
  buildImportClip,
  importClipId,
  importClipOp,
  parseWavInfo,
  sanitizeAssetKey,
  ImportError,
} from "../src/timeline/import";

/** Minimal PCM16-mono WAV: 44-byte header + `frames` samples of silence. */
function wavBytes(
  { channels = 1, bits = 16, rate = 8000, frames = 800, extra = "" }: {
    channels?: number;
    bits?: number;
    rate?: number;
    frames?: number;
    extra?: string;
  } = {},
): Uint8Array {
  const dataLen = (frames * channels * bits) / 8;
  const buf = new ArrayBuffer(44 + dataLen);
  const v = new DataView(buf);
  const ascii = (o: number, s: string) => {
    for (let i = 0; i < s.length; i++) v.setUint8(o + i, s.charCodeAt(i));
  };
  ascii(0, "RIFF");
  v.setUint32(4, 36 + dataLen, true);
  ascii(8, "WAVE");
  ascii(12, "fmt ");
  v.setUint32(16, 16, true);
  v.setUint16(20, 1, true);
  v.setUint16(22, channels, true);
  v.setUint32(24, rate, true);
  v.setUint32(28, (rate * channels * bits) / 8, true);
  v.setUint16(32, (channels * bits) / 8, true);
  v.setUint16(34, bits, true);
  ascii(36, "data");
  v.setUint32(40, dataLen, true);
  if (extra) ascii(44, extra);
  return new Uint8Array(buf);
}

describe("wav import", () => {
  test("valid PCM16 mono parses to rate and frames", () => {
    expect(parseWavInfo(wavBytes())).toEqual({ sampleRate: 8000, numFrames: 800 });
  });

  test("stereo, non-PCM, and non-WAV files are refused like the decoder", () => {
    expect(() => parseWavInfo(wavBytes({ channels: 2 }))).toThrow(ImportError);
    expect(() => parseWavInfo(wavBytes({ bits: 8 }))).toThrow(ImportError);
    expect(() => parseWavInfo(new Uint8Array([1, 2, 3]))).toThrow(ImportError);
    expect(() => parseWavInfo(wavBytes({ frames: 0 }))).toThrow(ImportError);
  });

  test("keys are bare filenames; traversal is refused", () => {
    expect(sanitizeAssetKey("kit/kick.wav")).toBe("kick.wav");
    expect(sanitizeAssetKey("C:\\kit\\kick.wav")).toBe("kick.wav");
    expect(sanitizeAssetKey("my hit!.wav")).toBe("my_hit_.wav");
    expect(sanitizeAssetKey("..")).toBe("_");
    expect(() => sanitizeAssetKey("")).toThrow(ImportError);
  });

  test("frames convert to beats at the project tempo", () => {
    expect(beatsForFrames(800, 8000, 120)).toBe(0.2);
    expect(() => beatsForFrames(800, 8000, 0)).toThrow(ImportError);
  });

  test("imported clip names the asset and the op carries it", () => {
    const clip = buildImportClip("trk_drums", 4, "hit.wav", "hit.wav", 0.2);
    expect(clip.kind).toBe("Audio");
    expect(clip.source).toBe("asset:hit.wav");
    expect(clip.id).toBe(importClipId("trk_drums", 4, "hit.wav"));
    const op = importClipOp("ui", clip);
    expect(op.kind).toBe("ClipAdded");
    expect(op.target).toBe(clip.id);
    expect(JSON.parse(op.value_json as string).source).toBe("asset:hit.wav");
  });
});
