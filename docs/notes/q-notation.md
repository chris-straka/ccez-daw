# Primer: score editor (`ui/src/notation/`)

Agent 4 owns `ui/src/notation/` — a print-friendly SVG score view over the
same `MidiClip` model the piano-roll canvas edits. There is exactly one
source of truth for notes; the two views are lenses, never forks.

## 1. One model, two lenses

Notes live in the piano-roll model (`ui/src/pianoroll/model.ts`, a Zod v4
mirror of `core/src/midi/clip.rs` — snake_case, sorted by
`(start_beats, pitch, note_id)`, byte-stable `encodeClip`/`decodeClip` behind
a clip's opaque `source` asset key). The frozen v0 `Clip` schema has no
notes field, so nothing in `contracts/` or `ui/src/generated/*` changed for
this track: no typegen run, no drift.

The rule that keeps the views in sync: **notation never mutates notes
directly**. Every edit in `selection.ts` delegates to the piano-roll store
(`createNote`, `moveNote`, `resizeNote`, `sweepDelete`), which re-sorts and
re-validates. So:

- click a notehead in the score → its `note_id` selects; the same id
  highlights on the piano-roll canvas;
- drag a note body on the canvas → the score re-lays-out from the new
  `pitch`/`start_beats` via `stepOfNote`;
- `encodeClip` after any mix of edits is byte-stable (`decode → encode` is
  identity — the round-trip test asserts this).

## 2. Pitch <-> staff steps (`pitch.ts`)

A *step* is a diatonic offset from the clef's bottom line: 0 = bottom line,
8 = top line, odds = spaces, out-of-range = ledger territory. Steps are
enharmonic-blind (C# and Db share one step); `accidentalGlyph` recovers the
spelling side (sharp-only in v1).

- `pitchName(60) === "C4"` (MIDI 60 = C4), `clefForPitch` splits at middle
  C: `>= 60` treble, below bass.
- Bottom lines: E4 (64) treble, G2 (43) bass. So middle C is treble step
  −2 (one ledger line below), top-line F5 is step 8.
- `naturalPitchAtStep(step, clef)` inverts the mapping for staff clicks;
  `createNoteAtStep` adds an optional `accidental: 1` offset, then calls the
  piano roll's own `createNote` — same ids, same snap grid.
- Fixed-point property (tested for all 128 pitches): naturals round-trip
  exactly; accidentals fold onto their natural's step.

## 3. Layout (`layout.ts`) — measures, systems, print sizing

`resolveLayout` (4/4, 4 measures per system, 640 px page by default) feeds
pure mappers: `measureOf` / `systemOf`, `xForBeat` (beat → px inside its
measure cell, bar-pad aware), `yForStep` (step 8 = system top line), with
`beatForX` / `stepForY` inverses for pointer hits. `layoutClip` places every
note; `pageSystems` / `pageHeight` size the page from the clip length, so
printing is just browser print over vector SVG.

`displayDuration` quantizes a length to the nearest `whole | half | quarter
| eighth | 16th` glyph; `hasStem` / `stemDown` (down on/above the middle
line) drive stem rendering. Durations are display-only — the model keeps
exact `len_beats`.

## 4. Selection + the SVG view (`selection.ts`, `ScoreView.tsx`)

Selection is a plain `Set<note_id>` (`selectOnly`, `toggleSelect`,
`pruneSelection` after edits). Ops: `transposeSelection` (semitones),
`shiftSelectionBeats`, `setSelectedDuration`, `deleteSelection`
(`sweepDelete` under the hood), all id-sorted and no-ops on unknown ids.

`ScoreView` renders black-on-white SVG: 5-line staves with bar lines, treble
clef, ledger lines, ♯ signs, filled/hollow heads by duration, stems, blue
selection tint, and a `@media print` rule that hides the screen hint. Click
toggles selection (shift adds); `onCreate` reports `{ step, clef, beat }`
for staff clicks the shell commits via `createNoteAtStep`. v1 renders a
single treble staff even for low notes (steps go negative with correct
ledgers); a grand staff is the natural next step. Surround/Atmos, hardware
surfaces, and cloud/collab are out of scope and untouched.

## 5. Verify

- `cd ui && bun test tests/notation.test.ts` — 5 pass: pitch mapping +
  full-range natural round-trip, layout inversion, roll→score→encode edit
  round-trip, staff click-create identity.
- `cd ui && bun test tests` — 133 pass, 0 fail (no regressions).
- `cd ui && bun run check` — `tsc --noEmit` clean; `generated/` untouched.
