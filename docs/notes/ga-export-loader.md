# Engine-side loader spec: how a game consumes a v1 export package (GA-4)

Normative for v1 packages (`validator_version: "1"`). Read
`docs/notes/ga-export.md` first for the authoring side; read
`contracts/export-package.md` for the frozen shapes. Nothing here changes
those contracts — this is the playback contract for the bytes they describe.

## 1. Load order (fail loud at load, never at playback)

1. Parse `package.json` into `ExportPackage`. Reject the package when
   `schema_version != 1` or `validator_version != "1"`.
2. Load every `bank_<id>.json` in `bank_ids`. Reject on malformed JSON,
   id mismatch with the filename, empty `clip_ids`, or duplicate event ids.
3. Load every stem in `stems` as PCM mono WAV. Reject on missing files or
   parse failures. (A package that passed `export-validator` already
   satisfies 1–3; the loader re-checks because packages travel through
   pipelines that corrupt things.)
4. Precompute per-stem loop samples (section 2) once, at load.

Playback-time failures stay silent-by-design: a stem that goes missing
*after* load renders silence (the GA-1 engine rule — dangling audio is a
validator error, never a realtime failure). Unknown game params in snapshots
are ignored per trigger; unknown event ids on trigger are dropped
(`UnknownEvent`). Both mirror the audition runtime.

## 2. Loop math (beats in, samples out)

Music stems (`kind: MusicLayer`) carry loop points in beats at the cue's own
tempo (`AdaptiveCue.tempo`, shipped alongside the package or known to the
title — the manifest references cues by id):

```text
loop_start_samples = floor(loop_start_beats * 60 / tempo * sample_rate)
loop_end_samples   = round(loop_end_beats * 60 / tempo * sample_rate)
frame[t] wraps into [loop_start_samples, loop_end_samples)
```

The validator guarantees `0 <= start < end` and that the file holds exactly
`loop_end - loop_start` beats of samples, so the wrap is seamless and the
whole file loops when `start` is `0`. Convert at the *cue* tempo, not the
project tempo — cues carry their own BPM.

SFX stems (`kind: SfxClip`) always carry `0, 0`: play once, do not loop.
`source_layer_id` is empty; `<n>` in `sfx/<event>_<n>.wav` is the index into
the event's `clip_ids` (pool pick = uniform index per trigger, round-robin
is a later version).

## 3. Trigger and snapshot evaluation (unchanged runtimes)

- Music: `layers_for_state(snapshot.state)` selects the audible stems
  (empty `states` = always-on bed); `transition_for(from, to)` selects the
  switch, defaulting to `Cut`. Fades convert `fade_beats` at the cue tempo.
- SFX: uniform pool pick from `clip_ids`, per-trigger `volume_random` /
  `pitch_random` humanization from a seeded RNG (deterministic replays use
  the export seed), `cooldown_ms` drops bursts, `max_polyphony` steals the
  oldest voice. RTPC per binding: clamp the game value into the declared
  param `[min, max]`, normalize to 0–1, map linearly onto the binding's
  `[min, max]`; unknown params ignored, missing values read as `default`.
- `event_bank_path` is the first bank's file in v1; engines supporting
  multi-bank titles load every `bank_<id>.json` in `bank_ids` instead.

## 4. Worked example

Package `demo-v1` ships `stems/cue_fight_bed.wav` (`MusicLayer`, loop
`0..8` beats, cue tempo 120) and `sfx/player.footstep_0.wav` (`SfxClip`,
`0, 0`):

- At 48 kHz the bed loop is `0..384000` samples
  (`8 * 60 / 120 * 48000`); the engine wraps the bed voice into that range
  while `explore` is active and keeps wrapping (bed states are empty) when
  `combat` adds the drums stem on top.
- Game code triggers `"player.footstep"`; the engine picks
  `sfx/player.footstep_{0,1}.wav` uniformly, scales by the event volume
  plus the threat-driven RTPC output, and plays it once.

## 5. Versioning

v1 loaders accept only v1 packages. Any shape change (new stem kind, new
loop semantics, new manifest field) arrives as a new `schema_version` plus
a migration note — never as a silent reinterpretation of v1 bytes.
