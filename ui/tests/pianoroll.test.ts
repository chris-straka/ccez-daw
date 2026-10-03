import { describe, expect, test } from "bun:test";
import {
  anchorTop,
  beatToX,
  hitTest,
  notesAt,
  pitchName,
  pitchToY,
  rowH,
  scrollTop,
  snapBeat,
  xToBeat,
  yToPitch,
  zoomRows,
  type ViewConfig,
} from "../src/pianoroll/geometry";
import { decodeClip, encodeClip, midiToFreq, type MidiClip } from "../src/pianoroll/model";
import { createNote, deleteNote, moveNote, resizeNote, sweepDelete } from "../src/pianoroll/store";

const VIEW: ViewConfig = { width: 640, height: 512, beatsVisible: 8, scrollBeats: 0 };

function emptyClip(): MidiClip {
  return { length_beats: 8, notes: [] };
}

describe("piano-roll scripted note-edit (FL mouse sequence, headless)", () => {
  test("full gesture script: create -> resize -> move -> sweep-delete", () => {
    let clip = emptyClip();
    // 1. Left-click empty space creates a snapped note (FL click-create).
    clip = createNote(clip, 60, 0.13, { snap: 0.25 });
    expect(clip.notes).toHaveLength(1);
    expect(clip.notes[0].start_beats).toBe(0.25);
    expect(clip.notes[0].pitch).toBe(60);
    const id = clip.notes[0].note_id;

    // 2. Drag the right edge: resize length (FL drag-length).
    clip = resizeNote(clip, id, 1.5);
    expect(clip.notes[0].len_beats).toBe(1.5);

    // 3. Drag the body: move pitch + time.
    clip = moveNote(clip, id, 64, 2.0);
    expect(clip.notes[0].pitch).toBe(64);
    expect(clip.notes[0].start_beats).toBe(2.0);

    // 4. Right-sweep over the note deletes it (FL right-sweep-delete).
    const victims = notesAt(
      clip.notes,
      beatToX(2.1, VIEW),
      pitchToY(64, VIEW) + 1,
      VIEW,
    ).map((n) => n.note_id);
    expect(victims).toEqual([id]);
    clip = sweepDelete(clip, victims);
    expect(clip.notes).toHaveLength(0);
  });

  test("hit test disambiguates resize handle vs body vs empty", () => {
    let clip = emptyClip();
    clip = createNote(clip, 60, 0, { snap: 0.25, lenBeats: 1 });
    const n = clip.notes[0];
    // Far-left of the note body -> move.
    expect(hitTest(clip.notes, beatToX(0.1, VIEW), pitchToY(60, VIEW) + 1, VIEW).kind).toBe("body");
    // Last pixels of the right edge -> resize.
    expect(
      hitTest(clip.notes, beatToX(n.start_beats + n.len_beats, VIEW) - 2, pitchToY(60, VIEW) + 1, VIEW)
        .kind,
    ).toBe("resize");
    // Far away in time -> empty (left-click would create).
    expect(hitTest(clip.notes, beatToX(6, VIEW), pitchToY(60, VIEW) + 1, VIEW).kind).toBe("empty");
  });

  test("coordinate round-trips and snap quantize", () => {
    expect(snapBeat(0.13, 0.25)).toBe(0.25);
    expect(snapBeat(0.11, 0.25)).toBe(0);
    for (const beat of [0, 0.5, 3.75, 7.9]) {
      expect(xToBeat(beatToX(beat, VIEW), VIEW)).toBeCloseTo(beat, 9);
    }
    for (const pitch of [0, 21, 60, 127]) {
      expect(yToPitch(pitchToY(pitch, VIEW) + 1, VIEW)).toBe(pitch);
    }
  });

  test("delete of a missing id reports absence; bad edits throw, never corrupt", () => {
    let clip = createNote(emptyClip(), 60, 0, { snap: 0.25 });
    const id = clip.notes[0].note_id;
    expect(deleteNote(clip, 999).removed).toBe(false);
    expect(deleteNote(clip, 999).clip.notes).toHaveLength(1);
    expect(deleteNote(clip, id).removed).toBe(true);
    expect(() => moveNote(clip, 999, 60, 0)).toThrow("unknown note_id");
    expect(() => resizeNote(clip, 999, 1)).toThrow("unknown note_id");
    // Resize clamps instead of corrupting: never zero/negative length.
    clip = resizeNote(clip, id, -4);
    expect(clip.notes[0].len_beats).toBeGreaterThan(0);
  });

  test("clip encode/decode round-trips byte-stable and rejects bad payloads", () => {
    let clip = emptyClip();
    clip = createNote(clip, 64, 2.0, { snap: 0.25, lenBeats: 1.5 });
    clip = createNote(clip, 60, 0.0, { snap: 0.25, lenBeats: 0.5 });
    const bytes = encodeClip(clip);
    const back = decodeClip(bytes);
    expect(back.notes.map((n) => n.note_id)).toEqual(
      [...clip.notes].sort((a, b) => a.start_beats - b.start_beats).map((n) => n.note_id),
    );
    expect(encodeClip(back)).toBe(bytes);
    expect(() => decodeClip("not json")).toThrow();
    expect(() => decodeClip(JSON.stringify({ length_beats: 4, notes: [{ bogus: 1 }] }))).toThrow();
  });
});

describe("pitch viewport (scroll/zoom)", () => {
  test("defaults show the whole range exactly like before", () => {
    expect(rowH(VIEW)).toBe(4);
    expect(pitchToY(127, VIEW)).toBe(0);
    expect(yToPitch(0, VIEW)).toBe(127);
    expect(yToPitch(511, VIEW)).toBe(0);
  });

  test("pitch names follow C4 = MIDI 60", () => {
    expect(pitchName(60)).toBe("C4");
    expect(pitchName(0)).toBe("C-1");
    expect(pitchName(127)).toBe("G9");
  });

  test("scroll clamps the window inside 0..127", () => {
    expect(scrollTop(127, 128, -200)).toBe(127);
    expect(scrollTop(100, 32, -90)).toBe(31);
    expect(scrollTop(31, 32, 200)).toBe(127);
  });

  test("zoom doubles/halves rows within 12..128", () => {
    expect(zoomRows(128, -1)).toBe(64);
    expect(zoomRows(64, 1)).toBe(128);
    expect(zoomRows(12, -1)).toBe(12);
    expect(zoomRows(128, 1)).toBe(128);
  });

  test("zoom anchor keeps the cursor pitch under the cursor", () => {
    const v: ViewConfig = { ...VIEW, topPitch: 100, rowsVisible: 64 };
    const p = yToPitch(100, v);
    expect(p).toBe(88);
    const rows2 = zoomRows(64, -1);
    const top2 = anchorTop(p, 100, VIEW.height / rows2, rows2);
    expect(top2).toBe(94);
    const v2: ViewConfig = { ...VIEW, topPitch: top2, rowsVisible: rows2 };
    expect(Math.abs(pitchToY(p, v2) - 100)).toBeLessThanOrEqual(8);
  });

  test("mapping round-trips inside a zoomed window", () => {
    const v: ViewConfig = { ...VIEW, topPitch: 71, rowsVisible: 24 };
    for (const p of [48, 60, 71]) {
      expect(yToPitch(pitchToY(p, v) + 1, v)).toBe(p);
    }
  });
});

describe("preview pitch math", () => {
  test("midiToFreq is equal-tempered at A4 = 440", () => {
    expect(midiToFreq(69)).toBeCloseTo(440, 6);
    expect(midiToFreq(60)).toBeCloseTo(261.626, 3);
    expect(midiToFreq(57)).toBeCloseTo(220, 6);
    expect(midiToFreq(81)).toBeCloseTo(880, 6);
  });
});
