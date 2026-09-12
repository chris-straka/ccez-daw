# Q-notation-core primer: from played MIDI to written score (and back)

New to notation? Start here. This note teaches the one big idea behind
`core/src/notation/` — **MIDI records performance, notation records
intent** — then lists exactly what the quantizer, score model, and
MusicXML exporter do so players, editors, and exporters can build on the
written side without re-deriving it.

> Filename note: `docs/notes/q-notation.md` already belongs to the UI score
> editor (`ui/src/notation/`, Agent 4). This primer covers the *core model*
> (`core/src/notation/`) and lives here so neither doc clobbers the other.
> The two tracks are independent: the UI renders its own SVG from the
> piano-roll model; this module quantizes `MidiClip`s and exports MusicXML.

## 1. The idea

A `MidiClip` says "note 66 started at beat 3.97 and lasted 0.48 beats".
A score says "an eighth note on F# on the off-beat of 3". The first is what
the fingers did; the second is what the composer meant. Quantization is the
performance → intent map, and it needs two pieces of context to be
musically literate:

- **Meter** tells it where the bar lines are (`Meter::bar_beats`: 4/4 → 4,
  3/4 and 6/8 → 3). Rests fill gaps *to the bar line*, and notes that cross
  a bar line split into tied notes — so the bar always adds up.
- **Key** tells it how to spell (`SpelledPitch::from_midi`): MIDI 66 is F#
  in G major, Gb in F major, because sharp-side keys (`fifths >= 0`) spell
  chromatics sharp and flat-side keys spell them flat. Naturals stay natural
  on both sides.

The reverse map (`to_midi`) renders the *written* part — a swung
performance quantized to straight eighths stays straight. `to_midi` is how
the engine plays a score; it is not how you recover the original feel.

## 2. The pieces (`core/src/notation/`)

- `model.rs` — the written-score types. A `Score` is one part / one staff /
  one voice: `title` + `Meter` + `KeySig` + `Measure`s. Each measure holds
  `MeasureEvent`s (`Note`, `Chord` for simultaneous onsets, `Rest`) plus
  `BeamGroup`s (index lists into that measure's events). Everything is
  symbolic — values are `NoteValue` + dot count (max 1), never floats — so
  scores compare with `==`, which is what makes the round-trip tests exact.
  `Measure::validate` checks the bar adds up and every beam names
  consecutive beamable (eighth-or-shorter) sounding events.
- `quantize.rs` — `quantize(clip, opts)` and `to_midi(score)`. The quantizer
  snaps onsets/lengths to `grid_beats` (default 16ths; must divide the bar,
  never finer than a 32nd), fuses onsets within half a grid step into
  `Chord`s, fills gaps with rests (greedy whole → 32nd, single dots),
  splits cross-bar notes with `tie_start`/`tie_stop`, and auto-beams runs of
  ≥ 2 beamable notes inside one quarter-beat group. Muted notes are skipped
  (they never sound, same as `midi::expand`); `probability` is ignored —
  notation writes the part, not the take gate.
- `musicxml.rs` — `export_xml` writes score-partwise MusicXML 4.0 (one
  part/staff/voice, 480 divisions, per-measure attributes, pitch/rest,
  type, dots, ties, `begin/continue/end` beams, `<chord/>` joins).
  `parse_xml` reads back *exactly what the exporter emits* so
  `parse(export(score)) == score` is testable. It is **not** a general
  importer (no multi-part/voice, no backup/forward) — import is out of
  scope for this track.

## 3. Scope edges (one voice, on purpose)

The model is one melodic line plus block chords. Two consequences:

- **Overlaps truncate.** A note still sounding when the next onset arrives
  is cut to that onset (legato piano input quantizes to back-to-back
  notes). Held-against-melody polyphony cannot fit one staff/one voice and
  is out of scope — documented in `quantize`, pinned by the
  `overlapping_notes_truncate_to_next_onset` test.
- **No import required.** `parse_xml` exists only to prove the exporter
  round-trips; depending on it for user files would be a category error.

Like `midi/`, this module reads only the frozen v0 `Clip` shape through the
opaque MIDI-asset convention: no `contracts/` change, no `model.rs`/`ipc.rs`
/`emit.rs` change, no `ui/src/generated/*` change — `bun run check` drift
gates stay green by construction.

## 4. Verify

- `cargo test --manifest-path core/Cargo.toml --lib notation` — 16 pass:
  meter bar lengths, key-sided spelling (+ full 21–108 round-trip both
  sides), snap-to-grid onsets, 3/4 + 6/8 bars, chord fusion, per-beat beams,
  overlap truncation, option rejection, MIDI round-trip
  (`quantize(to_midi(score)) == score`), MusicXML export shapes +
  exact round-trip (major 4/4, minor 3/4, escaped titles) + rejection of
  non-exporter XML.
- Full `cargo test --lib`: 292 pass; the 6 `meter::*` failures belong to the
  parallel metering track (`core/src/meter/`, untouched here) — not this
  module. The one build warning (`meter/lufs.rs`) is theirs too.
