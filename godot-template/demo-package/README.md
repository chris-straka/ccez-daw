# demo-package (S-1 fixture handoff)

Approved GA-4 export package for the Godot template track. Built by
`ga-fixture` through the frozen export path (`gaexport::build_package`);
approved by `scripts/validate-export.sh` (validator version 1). Do not
hand-edit these files — regenerate with:

```sh
cargo run --manifest-path ga-fixture/Cargo.toml -- \
  --out godot-template/demo-package --context ga-fixture/context.json
scripts/validate-export.sh godot-template/demo-package ga-fixture/context.json
```

Authoring side (clips, cues, bank, params): `ga-fixture/context.json`.
Primer: `docs/notes/s-fixture.md`.

## Contents (`demo-tp-v1`, 48 kHz mono WAV)

| Stem | Kind | Loop (beats) | Cue tempo |
|---|---|---|---|
| `stems/cue_field_bed.wav` | MusicLayer | 0–8 | 104 |
| `stems/cue_field_strings.wav` | MusicLayer | 0–8 | 104 |
| `stems/cue_field_drums.wav` | MusicLayer | 0–4 | 104 |
| `stems/cue_field_nightpad.wav` | MusicLayer | 0–8 | 104 |
| `stems/cue_boss_bed.wav` | MusicLayer | 0–8 | 132 |
| `stems/cue_boss_brass.wav` | MusicLayer | 0–8 | 132 |
| `stems/cue_boss_choir.wav` | MusicLayer | 0–4 | 132 |
| `stems/cue_stinger_boss_hit.wav` | MusicLayer | 0–2 | 132 |
| `sfx/player.footstep_{0,1}.wav` | SfxClip | one-shot | — |
| `sfx/sword.swing_{0,1}.wav` | SfxClip | one-shot | — |
| `sfx/ui.confirm_0.wav` | SfxClip | one-shot | — |
| `sfx/horse.gallop_{0,1}.wav` | SfxClip | one-shot | — |

Plus `package.json` (manifest) and `bank_bank_tp.json` (event bank, verbatim).

## Game surface

- States: `field`, `combat`, `boss`, `village`, `night`.
- Params: `threat` 0–1, `time_of_day` 0–24 h (default 12), `health_low` 0–1, `mounted` 0–1.
- Audible layers: field → bed+strings; village → bed+strings; combat → bed+drums; night → bed+nightpad; boss (cue_boss) → bed+brass+choir.
- Transitions: field↔combat Fade (2 / 4 beats), night→field Fade 2, boss→field Fade 4, field→boss Stinger (`cue_stinger_boss`), everything else Cut fallback.
- SFX: trigger `player.footstep`, `sword.swing`, `ui.confirm`, `horse.gallop` by id. RTPC: `threat` → `bus_sfx:volume`, `mounted` → `trk_sfx:volume`.
- Loop math (loader spec): `samples = beats * 60 / tempo * 48000`, whole file loops from 0.
