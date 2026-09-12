import { describe, expect, test } from "bun:test";
import {
  beatToX,
  hitTest,
  notesAt,
  pitchToY,
  snapBeat,
  xToBeat,
  yToPitch,
  type ViewConfig,
} from "../src/pianoroll/geometry";
import { decodeClip, encodeClip, type MidiClip } from "../src/pianoroll/model";
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
