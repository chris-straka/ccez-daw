import { describe, expect, test } from "bun:test";
import {
  applyOffsetDrag,
  formatTimecode,
  parseTimecode,
  sampleVideoDoc,
  timecodeForTransport,
  transportBeatsForVideoTime,
  validateVideoClip,
  videoTimeForTransport,
} from "../src/video/model";
import { applySyncDecision, syncDecision } from "../src/video/sync";

const TEMPO = 120; // 1 beat = 0.5s

describe("Agent 2 video sync + offset", () => {
  test("transport maps to media time through tempo and offset", () => {
    const doc = sampleVideoDoc();
    // vid_scene starts at beat 16; at 120bpm beat 20 = 2s of picture.
    expect(videoTimeForTransport(doc, 20, TEMPO)).toBeCloseTo(2.0, 9);
    // Offset +2 beats slips picture later: beat 20 shows 1s.
    const slipped = {
      ...doc,
      clips: doc.clips.map((c) => (c.id === "vid_scene" ? { ...c, offset_beats: 2 } : c)),
    };
    expect(videoTimeForTransport(slipped, 20, TEMPO)).toBeCloseTo(1.0, 9);
    // Gap between clips (beat 10) has no picture.
    expect(videoTimeForTransport(doc, 10, TEMPO)).toBeNull();
  });

  test("offset drag round-trips through transportBeatsForVideoTime", () => {
    const doc = sampleVideoDoc();
    const scene = doc.clips.find((c) => c.id === "vid_scene")!;
    const next = applyOffsetDrag(scene, 1.5);
    expect(next).toBeCloseTo(1.5, 9);
    const moved = { ...scene, offset_beats: next };
    // Media time 0 shows at transport start+offset.
    expect(transportBeatsForVideoTime(moved, 0, TEMPO)).toBeCloseTo(17.5, 9);
    // Drag clamps so picture cannot lead by more than the clip length.
    expect(applyOffsetDrag(scene, -100)).toBe(-scene.length_beats);
    expect(() => applyOffsetDrag(scene, Number.NaN)).toThrow();
  });

  test("timecode formats HH:MM:SS:FF and round-trips", () => {
    expect(formatTimecode(0, 30)).toBe("00:00:00:00");
    expect(formatTimecode(61.5, 30)).toBe("00:01:01:15");
    const doc = sampleVideoDoc();
    expect(timecodeForTransport(doc, 20, TEMPO)).toBe("00:00:02:00");
    expect(timecodeForTransport(doc, 10, TEMPO)).toBeNull();
    const tc = formatTimecode(3661.2, 30);
    expect(parseTimecode(tc, 30)).toBeCloseTo(3661.2, 6);
    expect(() => parseTimecode("nope", 30)).toThrow();
    expect(() => parseTimecode("00:00:00:30", 30)).toThrow();
  });

  test("sync decision: coast when close, seek when drifting, hold when paused", () => {
    const doc = sampleVideoDoc();
    // Playing, element within threshold -> leave it alone.
    expect(
      syncDecision({
        doc,
        transportBeats: 20,
        tempo: TEMPO,
        state: "Playing",
        elementTime: 2.05,
        elementPaused: false,
      }),
    ).toEqual({ kind: "coast" });
    // Playing, element far off -> seek to target (2.0s).
    expect(
      syncDecision({
        doc,
        transportBeats: 20,
        tempo: TEMPO,
        state: "Playing",
        elementTime: 9.0,
        elementPaused: false,
      }),
    ).toEqual({ kind: "seek", toSeconds: 2 });
    // Paused -> pin to exact frame even for tiny drift.
    expect(
      syncDecision({
        doc,
        transportBeats: 20,
        tempo: TEMPO,
        state: "Stopped",
        elementTime: 2.05,
        elementPaused: true,
      }),
    ).toEqual({ kind: "hold", toSeconds: 2 });
    // No clip under playhead while playing -> blank (caller pauses).
    expect(
      syncDecision({
        doc,
        transportBeats: 10,
        tempo: TEMPO,
        state: "Playing",
        elementTime: 1.0,
        elementPaused: false,
      }),
    ).toEqual({ kind: "blank" });
  });

  test("applySyncDecision drives a stub element", async () => {
    const el = {
      currentTime: 9.0,
      paused: false,
      async play() {
        el.paused = false;
      },
      pause() {
        el.paused = true;
      },
    };
    await applySyncDecision(el, { kind: "seek", toSeconds: 2 }, true);
    expect(el.currentTime).toBe(2);
    await applySyncDecision(el, { kind: "hold", toSeconds: 2 }, false);
    expect(el.paused).toBe(true);
  });

  test("sample doc validates", () => {
    for (const clip of sampleVideoDoc().clips) expect(() => validateVideoClip(clip)).not.toThrow();
    expect(() =>
      validateVideoClip({ ...sampleVideoDoc().clips[0], fps: 0 }),
    ).toThrow();
  });
});
