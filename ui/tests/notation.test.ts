import { describe, expect, test } from "bun:test";
import { decodeClip, encodeClip, type MidiClip } from "../src/pianoroll/model";
import { createNote } from "../src/pianoroll/store";
import {
  accidentalGlyph,
  diatonicIndex,
  layoutClip,
  ledgerLines,
  naturalPitchAtStep,
  pitchName,
  staffStep,
  stepForY,
  systemTop,
  xForBeat,
  yForStep,
} from "../src/notation";
import {
  createNoteAtStep,
  deleteSelection,
  pruneSelection,
  selectOnly,
  setSelectedDuration,
  shiftSelectionBeats,
  toggleSelect,
  transposeSelection,
} from "../src/notation";

function emptyClip(): MidiClip {
  return { length_beats: 8, notes: [] };
}

describe("notation pitch mapping", () => {
  test("names, clef split, and ledger counts", () => {
    expect(pitchName(60)).toBe("C4");
    expect(pitchName(69)).toBe("A4");
    expect(pitchName(64)).toBe("E4");
    // Middle C sings treble with one ledger line below; low C is bass.
    expect(staffStep(60)).toEqual({ clef: "treble", step: -2 });
    expect(ledgerLines(-2)).toBe(1);
    expect(staffStep(59).clef).toBe("bass");
    expect(staffStep(64)).toEqual({ clef: "treble", step: 0 });
    expect(staffStep(77)).toEqual({ clef: "treble", step: 8 });
    expect(ledgerLines(8)).toBe(0);
    expect(accidentalGlyph(61)).toBe("♯");
    expect(accidentalGlyph(60)).toBe("");
  });

  test("natural steps round-trip for every natural pitch 0..127", () => {
    for (let p = 0; p <= 127; p++) {
      const { clef, step } = staffStep(p);
      const natural = naturalPitchAtStep(step, clef);
      // Same staff slot: identical diatonic index (accidentals fold onto it).
      expect(diatonicIndex(natural)).toBe(diatonicIndex(p));
      // Naturals are exact fixed points.
      if (accidentalGlyph(p) === "") expect(natural).toBe(p);
    }
  });
});

describe("notation layout", () => {
  test("y inverts to step; x orders beats within measures", () => {
    const { layout } = layoutClip(emptyClip());
    for (const step of [-4, -2, 0, 3, 8, 11]) {
      const y = yForStep(step, systemTop(0, layout), layout);
      expect(stepForY(y, systemTop(0, layout), layout)).toBe(step);
    }
    // Beats sort left-to-right inside one measure; next measure wraps right.
    expect(xForBeat(0, layout)).toBeLessThan(xForBeat(1, layout));
    expect(xForBeat(1, layout)).toBeLessThan(xForBeat(3.9, layout));
  });
});

describe("notation <-> piano-roll edit round-trip", () => {
  test("create in roll -> select/transpose/shift/resize in score -> encode stable", () => {
    // 1. Piano-roll gesture creates the notes (FL click-create).
    let clip = emptyClip();
    clip = createNote(clip, 60, 0, { snap: 0.25, lenBeats: 1 });
    clip = createNote(clip, 64, 2, { snap: 0.25, lenBeats: 0.5 });
    expect(clip.notes).toHaveLength(2);

    // 2. Score lays both out on known steps (C4 ledger-below, E4 bottom line).
    const before = layoutClip(clip);
    expect(before.notes.map((p) => p.step).sort((a, b) => a - b)).toEqual([-2, 0]);

    // 3. Score selection edits go through the same store as the roll.
    const low = clip.notes.find((n) => n.pitch === 60)!;
    let sel = selectOnly([low.note_id]);
    sel = toggleSelect(sel, 9999); // stray id toggles harmlessly…
    sel = toggleSelect(sel, 9999); // …and back off, selection unchanged.
    clip = transposeSelection(clip, sel, 2); // C4 -> D4
    expect(clip.notes.find((n) => n.note_id === low.note_id)!.pitch).toBe(62);
    const afterTranspose = layoutClip(clip);
    expect(afterTranspose.notes.find((p) => p.note.note_id === low.note_id)!.step).toBe(-1);

    clip = shiftSelectionBeats(clip, sel, 1);
    expect(clip.notes.find((n) => n.note_id === low.note_id)!.start_beats).toBe(1);
    clip = setSelectedDuration(clip, low.note_id, 2);
    expect(clip.notes.find((n) => n.note_id === low.note_id)!.len_beats).toBe(2);

    // 4. Opaque-asset round-trip is byte-stable after score edits.
    const bytes = encodeClip(clip);
    const back = decodeClip(bytes);
    expect(encodeClip(back)).toBe(bytes);
    expect(back.notes.find((n) => n.note_id === low.note_id)!.pitch).toBe(62);

    // 5. Score delete prunes the selection; clip empties note by note.
    clip = deleteSelection(clip, sel);
    expect(pruneSelection(clip, sel).has(low.note_id)).toBe(false);
    expect(clip.notes).toHaveLength(1);
  });

  test("staff click-create maps to the same pitch the roll would make", () => {
    let clip = emptyClip();
    // Click the bottom line (step 0, treble) at beat 1: E4, like the roll.
    clip = createNoteAtStep(clip, 0, "treble", 1, { snap: 0.25, lenBeats: 1 });
    expect(clip.notes[0].pitch).toBe(64);
    // Sharp step click with accidental +1 spells the black key.
    clip = createNoteAtStep(clip, 0, "treble", 2, { snap: 0.25, accidental: 1 });
    expect(clip.notes.find((n) => n.start_beats === 2)!.pitch).toBe(65);
    const bytes = encodeClip(clip);
    expect(encodeClip(decodeClip(bytes))).toBe(bytes);
  });
});
