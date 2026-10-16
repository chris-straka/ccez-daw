# audioforge music package (additive, v1)

What `ccez-compose export` (and the MCP `compose_export_audioforge` tool)
writes for audioforge's interactive music
(`~/Games/_audio/audioforge`, `crates/af-music/src/score.rs`). Source of truth:
`core/src/compose/audioforge.rs`. audioforge's
`crates/af-runtime/tests/ccez_daw_export.rs` plays a package built from
`contracts/fixtures/audioforge-tiny.composition.json` through its real
runtime; regenerate it with `scripts/export-audioforge-fixture.sh`.

```
<dir>/
  manifest.json        # format "audioforge-music", format_version 1; stems + sections + instrument licences
  score.json           # audioforge Score
  mix.json             # audioforge RuntimeConfig: buses master + music, score
  events/music.<name>.<section>.<layer>.json   # kind "loop", bus "music", one wav source
  stems/<section>_<layer>.wav                  # 16-bit PCM stereo 48 kHz (+ smpl loop chunk)
  scenes/preview.json  # audioforge scene playing the preview plan
  composition.json     # the source composition (re-open, re-export)
```

A game loads it with `AudioRuntime::from_files(settings, "<dir>/mix.json",
"<dir>/events")` and drives it with `set_music_section` /
`set_music_intensity` (Bevy: `MusicSection`, `MusicIntensity`).

## Mapping

| Composition | score.json |
|---|---|
| section `role: intro`, `bars`, `next` | `once: true`, `length_beats`, `next` (hand-off on the exact beat, no fade) |
| section `role: loop` | `loop_start_beats: 0`, `loop_end_beats` = section beats |
| section `role: outro` | `once: true`, no `next` (silence after; the tail rings out) |
| layer `min_intensity` (1-5), `gain_db` | layer `min_intensity`, `gain_db` |
| first intro (else first loop) | `initial`; `initial_intensity: 1` |
| `tempo`, `beats_per_bar` | `bpm`, `beats_per_bar`; `quantize: "bar"`, `xfade_beats: 1` |

One stem per (section, layer) that has notes; a section with no notes is
left out of the score. Loop stems are exactly the section length with the
release/reverb tail folded onto the start (seamless by construction);
once stems are the section length plus their trimmed ring-out tail. One
shared trim (`manifest.trim_db`) puts the loudest section at full
intensity near -1 dBFS.

## Additive fields (2026-10-15)

Stems became stereo (2 channels, interleaved) without a version bump:
audioforge's WAV reader takes 1-2 channels, and every reader of v1 must
too. The manifest grew, additively:

- `channels`: 2 (top level) and `stems[].channels`.
- `instruments`: one row per composition instrument, `{ id, patch, kind:
  "sampled" | "synth" | "local", source?, licence, licence_url? }`, so a
  game can credit sample libraries (FluidR3's MIT notice must ship with
  choir stems; CC0 asks nothing).
- `composition.json` may carry `instruments[].pan` and a top-level `room`
  (`decay_secs`, `predelay_ms`, `damping`); both are optional.

The export report (stdout of `ccez-compose export`) adds `loop_seams`
(per loop stem: wrap-around step vs the music's 99.9th-percentile step,
`seamless`) and `warnings`.

## Limits (v1)

- audioforge (as of 2026-10-15) downmixes music stems to mono when it
  plays them and pans per layer; the stereo image is in the files for
  when it plays stereo sources.
- One tempo per piece (audioforge's score-level `bpm`).
- No stingers or per-transition rules: section switches cross-fade on the
  next bar; intensity changes fade over one beat from the next beat.
