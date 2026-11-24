# S-2 — Loop-seam audition: catch the click before the game does

## The problem in one paragraph

A game music loop is a circle, but a WAV file is a line. When the engine
reaches the last sample it jumps back to the first — and if those two
samples disagree, the speaker cone is ordered to *teleport*. You hear that
teleport as a tick, once per revolution, forever. It is the most common
"the mix was fine in the DAW" bug in game audio, because the DAW plays the
line while the game plays the circle. S-2 makes the DAW play the circle
too, and makes the ship-gate reject lines that do not close.

## The mental model: the seam is one number

Forget spectra and zero-crossing editors. For a loop that must wrap
gaplessly, exactly one number matters:

> **boundary discontinuity = |last sample − first sample|**, on `[-1, 1]` floats.

A whole-beat reference tone at the cue tempo with short edge fades wraps
near zero — our clean stems measure around 1e-4. Anything above **0.02**
(2% of full scale) is a real seam, not analysis noise. That is the whole
metric, in both implementations:

- In-DAW preview meter: `ui/src/gameaudio/loop.ts` (`seamStep`, `analyzeSeam`)
- Ship-gate: `core/src/loopseam.rs` (`seam_step`, `analyze_seam`)

Keep these two definitions in sync — if you change one, change the other
and say so in both module docs.

## Auditioning a loop gaplessly (any cue)

`ui/src/gameaudio/loopAudition.tsx` (`LoopAudition`) takes any
`AdaptiveCue` and offers one layer at a time through a real looped
`AudioBufferSourceNode` (`loop = true`) — the same wrap the engine
performs, so there is nowhere for a gap to hide. Next to the transport it
shows the seam meter (step vs. threshold slider) and a wrap readout proving
the audition clock folds into the loop window (`wrapBeatIntoLoop`: beat 9.5
of a 0–8 loop plays at 1.5, never as silence-plus-restart).

The preview tone is a whole-cycle stand-in, not your mix: this panel
auditions the *seam*. Your real stem bytes are judged at export time.

## The gate: validator rule 6

`scripts/validate-export.sh` runs six rules; rule 6 checks every
**music-loop** stem's boundary step against the threshold. One-shots are
exempt (they never loop). A clicking stem fails with its measurement and
two exits:

1. **Fix** — re-export with loop-clean edges: a whole-beat loop at the cue
   tempo with short fades on both ends (what `build_package` renders, and
   what the preview's *Fix: fade edges* button demonstrates via
   `applyEdgeFade`). Then the step drops back to ~1e-4.
2. **Waive** — record the stem in `loop-waivers.json` with a *reason*:
   `{ "waivers": [{ "path": "stems/cue_bed.wav", "reason": "…" }] }`,
   passed as `--waive <file>`. Empty reasons are rejected, and a waiver
   naming no package stem fails as *stale* — a renamed stem must never
   silently lose its gate. A waiver is a human decision on record, never
   an accident.

Tuning the gate: `--click-threshold F` overrides 0.02 per run; `0`
disables the rule (SFX-only packages). Threshold 0.02 sits far above PCM16
quantization (~3e-5) and far below an audible tick — move it only with a
listening test behind you.

## Where the code lives

| Piece | File |
|---|---|
| Seam metric + waivers + fix text (gate) | `core/src/loopseam.rs` (new) |
| Rule 6 wiring (`validate_package_full`) | `core/src/gaexport.rs` (additive) |
| CLI flags `--click-threshold`, `--waive` | `core/src/bin/export-validator.rs` (additive) |
| Script docs + flag pass-through | `scripts/validate-export.sh` (additive) |
| Seam metric + wrap + fix + waivers (DAW) | `ui/src/gameaudio/loop.ts` (new) |
| Gapless preview + meter + fix-or-waive UX | `ui/src/gameaudio/loopAudition.tsx` (new) |
| Barrel re-exports | `ui/src/gameaudio/index.ts` (additive) |

Contracts untouched: no new serialized types, `ui/src/generated/*`
unmodified. Game states for authored content: field / combat / dungeon /
boss / village / night with `threat`, `time_of_day`, `health_low`,
`mounted` (v4 plan) — the seam gate is state-agnostic and covers every
cue's layers equally.

## Validation (how we know it works)

- `cargo test` — `loopseam` unit tests (clean passes, seeded click fails,
  waived passes, stale waiver fails, waiver file round-trips) plus the
  `gaexport` rule-6 integration test: a real built package passes, the
  same package with a seeded click injected into one stem is rejected by
  name with fix-or-waive guidance, a reasoned waiver clears the seam gate.
- `bun test tests/gameaudio-loop.test.ts` — the TS mirror: seeded click
  detected, clean passes, edge-fade fix clears it, wrap math, waiver
  round-trip.
- `bun run check` green (typegen `--check`, `tsc --noEmit`, full
  `cargo test`).
