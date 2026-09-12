import { describe, expect, test } from "bun:test";
import {
  applyPropsToMidi,
  applyWarpToMidi,
  cleanupMidi,
  notchSpectrum,
  ratioToSemitones,
  samplePhrase,
  semitonesToRatio,
  transposeClip,
} from "../src/repair/model";

describe("time/pitch round-trips", () => {
  test("+7 then -7 restores every note (the Track I validation)", () => {
    const clip = samplePhrase();
    const up = transposeClip(clip, 7);
    expect(up.notes.map((n) => n.pitch)).toEqual([67, 71, 74]);
    const back = transposeClip(up, -7);
    expect(back).toEqual(clip);
  });

  test("props edit then exact inverse restores timing and pitch", () => {
    const clip = samplePhrase();
    const edited = applyPropsToMidi(clip, 12, 2);
    expect(edited.length_beats).toBe(2);
    const back = applyPropsToMidi(edited, -12, 0.5);
    expect(back.notes.map((n) => n.pitch)).toEqual([60, 64, 67]);
    for (let i = 0; i < back.notes.length; i++) {
      expect(Math.abs(back.notes[i].start_beats - clip.notes[i].start_beats)).toBeLessThan(1e-9);
    }
  });

  test("fractional pitch rides bend, ratio agrees with semitones", () => {
    const clip = samplePhrase();
    const edited = applyPropsToMidi(clip, 0.3, 1);
    expect(edited.notes.map((n) => n.pitch)).toEqual([60, 64, 67]);
    expect(edited.notes.every((n) => Math.abs(n.pitch_bend - 0.3) < 1e-12)).toBe(true);
    expect(Math.abs(ratioToSemitones(semitonesToRatio(7)) - 7)).toBeLessThan(1e-9);
  });

  test("empty warp is identity; small shift negates back", () => {
    const clip = samplePhrase();
    expect(applyWarpToMidi(clip, [])).toEqual(clip);
    const warped = applyWarpToMidi(clip, [{ at_beats: 0, shift_beats: 0.1 }]);
    expect(warped.notes.map((n) => n.len_beats)).toEqual(clip.notes.map((n) => n.len_beats));
    const back = applyWarpToMidi(warped, [{ at_beats: 0.1, shift_beats: -0.1 }]);
    for (let i = 0; i < back.notes.length; i++) {
      expect(Math.abs(back.notes[i].start_beats - clip.notes[i].start_beats)).toBeLessThan(1e-9);
    }
  });

  test("out-of-range edits are refused loudly", () => {
    const clip = samplePhrase();
    expect(() => transposeClip(clip, 25)).toThrow();
    expect(() => applyPropsToMidi(clip, 0, 0)).toThrow();
  });
});

describe("repair cleanup", () => {
  test("cleanup converges: second pass keeps everything", () => {
    const clip = {
      ...samplePhrase(),
      notes: [
        { ...samplePhrase().notes[0], start_beats: 0.02, timing_offset_beats: 0 },
        { ...samplePhrase().notes[1], muted: true },
      ],
    };
    const once = cleanupMidi(clip, { quantizeGrid: 0.5, dropMuted: true, fixOverlaps: true });
    expect(once.notes.length).toBe(1);
    expect(once.notes[0].start_beats).toBe(0);
    const twice = cleanupMidi(once, { quantizeGrid: 0.5, dropMuted: true, fixOverlaps: true });
    expect(twice).toEqual(once);
  });

  test("spectral notch cuts one band and validates range", () => {
    const frames = [
      [0.01, 0.9, 0.02],
      [0.03, 0.8, 0.005],
    ];
    const out = notchSpectrum(frames, { band: 1, width: 0, cut: 0 });
    expect(out[0][1]).toBe(0);
    expect(out[1][0]).toBe(0.03);
    expect(() => notchSpectrum(frames, { band: 9, width: 0, cut: 0 })).toThrow();
  });
});
