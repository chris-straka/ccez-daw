import { describe, expect, test } from "bun:test";
import type { ExportPackage, SfxBank } from "../src/generated/project";
import {
  estimatePackage,
  estimateWavBytes,
  EXPORT_BYTES_PER_SAMPLE,
  EXPORT_SAMPLE_RATE_HZ,
  formatBytes,
  previewPackage,
  profileVoices,
  stemSecondsFromBeats,
  WAV_HEADER_BYTES,
} from "../src/gameaudio/export";

function pkg(): ExportPackage {
  return {
    schema_version: 1,
    name: "demo-v1",
    cue_ids: ["cue_fight"],
    bank_ids: ["bank_ui"],
    stems: [
      {
        path: "stems/cue_fight_bed.wav",
        source_id: "cue_fight",
        source_layer_id: "bed",
        kind: "MusicLayer",
        loop_start_beats: 0,
        loop_end_beats: 16,
      },
      {
        path: "sfx/ev_click_0.wav",
        source_id: "ev_click",
        source_layer_id: "",
        kind: "SfxClip",
        loop_start_beats: 0,
        loop_end_beats: 0,
      },
    ],
    event_bank_path: "bank_bank_ui.json",
    validator_version: "1",
  };
}

function banks(): SfxBank[] {
  return [
    {
      schema_version: 1,
      id: "bank_ui",
      name: "UI",
      events: [
        {
          id: "ev_click",
          name: "Click",
          clip_ids: ["clip_c"],
          volume: 0.7,
          volume_random: 0.05,
          pitch_random: 1.0,
          cooldown_ms: 50,
          max_polyphony: 4,
          rtpc: [],
        },
      ],
    },
  ];
}

describe("export preview", () => {
  test("lists stems, bank files, and counts", () => {
    const p = previewPackage(pkg(), banks());
    expect(p.manifestPath).toBe("package.json");
    expect(p.bankFiles).toEqual(["bank_bank_ui.json"]);
    expect(p.stemRows.map((r) => r.loop)).toEqual(["loop 0\u201316 beats", "one-shot"]);
    expect(p.stemRows[0]?.source).toBe("cue_fight/bed");
    expect(p.stemRows[1]?.source).toBe("ev_click");
    expect(p.counts).toEqual({ stems: 2, cues: 1, banks: 1, events: 1 });
  });

  test("unknown bank ids count zero events, still list the file", () => {
    const p = previewPackage({ ...pkg(), bank_ids: ["bank_missing"] }, banks());
    expect(p.bankFiles).toEqual(["bank_bank_missing.json"]);
    expect(p.counts.events).toBe(0);
  });
});

describe("size estimate sanity", () => {
  test("one mono second is 48000*3 + header", () => {
    expect(estimateWavBytes(1)).toBe(EXPORT_SAMPLE_RATE_HZ * EXPORT_BYTES_PER_SAMPLE + WAV_HEADER_BYTES);
  });

  test("scales linearly and doubles for stereo", () => {
    expect(estimateWavBytes(2)).toBe(2 * (estimateWavBytes(1) - WAV_HEADER_BYTES) + WAV_HEADER_BYTES);
    expect(estimateWavBytes(1, 2) - WAV_HEADER_BYTES).toBe(2 * (estimateWavBytes(1) - WAV_HEADER_BYTES));
  });

  test("negative durations clamp to header-only", () => {
    expect(estimateWavBytes(-5)).toBe(WAV_HEADER_BYTES);
  });

  test("package total is stems + banks + manifest", () => {
    const e = estimatePackage(pkg(), banks(), {
      "stems/cue_fight_bed.wav": 8, // 16 beats @ 120 BPM
      "sfx/ev_click_0.wav": 0.5,
    });
    expect(e.totalBytes).toBe(e.stemBytes + e.bankBytes + e.manifestBytes);
    expect(e.stemBytes).toBeGreaterThan(8 * EXPORT_SAMPLE_RATE_HZ * EXPORT_BYTES_PER_SAMPLE);
    expect(e.bankBytes).toBeGreaterThan(0);
    expect(e.memoryBytes).toBeGreaterThan(e.stemBytes); // f32 decode > 24-bit disk
    expect(e.totalHuman).toContain("MiB");
  });

  test("16 beats at 120 BPM is 8 seconds; one-shots carry no beat length", () => {
    const [loop, oneshot] = pkg().stems;
    expect(stemSecondsFromBeats(loop!, 120)).toBeCloseTo(8);
    expect(stemSecondsFromBeats(oneshot!, 120)).toBe(0);
  });

  test("formatBytes boundaries", () => {
    expect(formatBytes(0)).toBe("0 B");
    expect(formatBytes(1023)).toBe("1023 B");
    expect(formatBytes(1024)).toBe("1.0 KiB");
  });
});

describe("voice profiler", () => {
  test("empty log profiles to zero", () => {
    expect(profileVoices([])).toEqual({ peakTotal: 0, peakByEvent: {}, overPolyphony: [] });
  });

  test("overlapping voices stack; touching voices do not", () => {
    const p = profileVoices([
      { eventId: "ev_hit", startedMs: 0, durationMs: 100 },
      { eventId: "ev_hit", startedMs: 50, durationMs: 100 },
      { eventId: "ev_hit", startedMs: 150, durationMs: 100 }, // touches previous end
    ]);
    expect(p.peakTotal).toBe(2);
    expect(p.peakByEvent).toEqual({ ev_hit: 2 });
  });

  test("flags events over max_polyphony", () => {
    const p = profileVoices(
      [
        { eventId: "ev_hit", startedMs: 0, durationMs: 200 },
        { eventId: "ev_hit", startedMs: 10, durationMs: 200 },
        { eventId: "ev_boom", startedMs: 0, durationMs: 200 },
      ],
      { ev_hit: 1, ev_boom: 4 },
    );
    expect(p.peakTotal).toBe(3);
    expect(p.overPolyphony).toEqual([{ eventId: "ev_hit", peak: 2, max: 1 }]);
  });

  test("zero-duration entries never sound", () => {
    const p = profileVoices([{ eventId: "ev_hit", startedMs: 0, durationMs: 0 }]);
    expect(p.peakTotal).toBe(0);
    expect(p.peakByEvent).toEqual({});
  });
});
