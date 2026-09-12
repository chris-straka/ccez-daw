# S-Godot primer: playing a GA-4 package in Godot 4.7.1

New to the game side of the DAW? Start here. This note teaches the one big
idea behind `godot-template/` — **the DAW ships data, the game ships
behavior, and a fail-loud loader stands between them** — then walks a
five-minute runbook from package to audible state switches.

## 1. The idea

Nothing in the DAW project plays in a game. The export package
(`contracts/export-package.md`) is just rendered WAVs plus JSON: stems the
engine loops (`stems/<cue>_<layer>.wav`, loop points in beats) or one-shots
(`sfx/<event>_<n>.wav`), one bank file per bank (`bank_<id>.json`), and a
manifest (`package.json`) tying them together.

The template's job is the ~200 lines of GDScript every Godot game needs to
turn that directory into music:

- **Load once, fail loud** (`scripts/bank_loader.gd`): parse the manifest,
  reject wrong schema/validator versions, malformed banks, id mismatches,
  empty pools, duplicate events, bad loop ranges, missing files, and a stale
  title-side cue map. A package that passed `export-validator` already
  satisfies all of this; the loader re-checks because packages travel
  through pipelines that corrupt things.
- **Music never restarts** (`scripts/music_player.gd`): one looping
  `AudioStreamPlayer` per stem; a state change only moves volumes. New state
  layers fade in over the transition's `fade_beats` (converted at the **cue**
  tempo), old ones fade out. No rule for the pair means a Cut. Loops are set
  on the `AudioStreamWAV` (`LOOP_FORWARD`, beats→samples at the cue tempo),
  so Godot itself wraps the seam — the whole file loops when start is 0.
- **SFX is an event, not a file** (`scripts/sfx_player.gd`): game code calls
  `trigger("player.footstep", values)`. The player picks a pool clip
  uniformly, applies per-trigger volume/pitch humanization from a seeded RNG
  (default seed 2026, the DAW export default, so replays are deterministic),
  drops bursts inside `cooldown_ms`, steals the oldest voice past
  `max_polyphony`, and bends the hit with RTPC bindings (clamp → normalize
  → linear map; unknown params ignored, missing values read as default).
  Unknown event ids are dropped with a warning — playback-time failures stay
  silent-by-design, exactly like the audition runtime.

One deliberate v1 gap: the manifest does **not** ship the `AdaptiveCue`
(tempo, per-layer state lists, transitions). That stays authoring-side data,
so the title carries a small cue map (`demo/cue_overworld.json`) and the
loader cross-checks its layer ids against the manifest — a stale map is a
load error, not a silent wrong mix. `BarWait`/`Stinger` transitions fall back
to `Fade` with a one-time warning (v1 scope; the DAW still authors them).

## 2. The pieces

- `godot-template/project.godot` — Godot 4.7 project (GL Compatibility, so
  it also runs on Switch-class hardware profiles); main scene `demo/main.tscn`.
- `godot-template/scripts/` — the three players above. GDScript only, no C#,
  so the template runs on the mono build and the standard build alike.
- `godot-template/demo/main.tscn` + `main.gd` — test scene. The `.tscn` is
  one node; `main.gd` builds the UI in code: six state buttons
  (field/combat/dungeon/boss/village/night), four param sliders
  (`threat` 0–1, `time_of_day` 0–24h, `health_low` 0–1, `mounted` 0–1), four
  SFX buttons, and a status label showing the posted snapshot and the
  audible layer set. `main.gd` is the integration example: copy its
  load → setup → `set_state` / `trigger` flow into your game.
- `godot-template/demo/overworld-v1/` — fixture package built by the real
  DAW export path (`build_package` + `write_package` at 48 kHz, seed 2026),
  approved by `export-validator`. Cue `cue_overworld` at 100 BPM, six layers
  (bed always-on; drums on combat/boss; drone on dungeon; brass on boss; pad
  on village/night; bell on night), bank `bank_adventure` with four events.
  Measured bytes: 48 kHz mono 16-bit PCM; music lengths exactly
  `loop_beats * 60 / tempo * 48000` frames (bed 230400); SFX exactly 48000.
  `context.json` next to it is provenance for re-validation, ignored by the loader.
- `godot-template/tools/self_test.py` — 60-check headless self-test mirroring
  the GDScript logic (loop math, `layers_for_state`, transitions, RTPC).
- `godot-template/tools/regen_fixture.rs` — the generator source; see the
  template README for the copy-run-delete recipe (it lives outside `core/` so
  this track touches no shared dirs).

## 3. Five-minute runbook

1. **Minute 0–1 — self-test.** `cd godot-template && python3 tools/self_test.py`.
   Expect `all checks passed`. This proves the fixture satisfies the loader
   spec without needing a Godot binary.
2. **Minute 1–2 — open.** Open `godot-template/` in Godot 4.7.1 (mono or
   standard). Let the importer finish scanning `demo/overworld-v1/`.
3. **Minute 2–3 — run.** Run the main scene (`demo/main.tscn`). The status
   label reads `state 'field' audible layers: ["bed"]`.
4. **Minute 3–4 — switch.** Click `combat`: drums fade in over 2 beats
   (1.2 s at 100 BPM) without restarting the bed. Click `night`: pad + bell
   fade in over 4 beats. Drag `threat` to 1, then trigger `player.footstep`
   — the hit plays louder via the threat→volume RTPC binding.
5. **Minute 4–5 — break it (optional).** Rename a stem file and re-run: the
   loader rejects the package with a named error instead of playing a wrong
   mix. Restore the name to go green again.

## 4. Ship your own music (the real import)

1. In the DAW, export a GA-4 package for your cues + banks; validate it with
   `scripts/validate-export.sh <pkg> <context.json>`.
2. Copy the package directory into your Godot project (e.g. `res://audio/`).
3. Copy the cue's tempo, per-layer `states`, volumes, and transitions from
   the DAW into a cue-map JSON shaped like `demo/cue_overworld.json`.
4. Point the `PACKAGE_DIR` / `CUE_ID` / `BANK_ID` constants at your content
   (or lift the load → setup → `set_state` / `trigger` flow into your own
   state machine) and run the same five minutes above.

Game states here are the LoZ:TP-like set the template authors against
(field/combat/dungeon/boss/village/night;
`threat`/`time_of_day`/`health_low`/`mounted`). The game is early, so names
may evolve — when they do, update the cue map and the DAW cue together; the
loader's cross-check catches a half-updated pair.

## 5. Validation (what was actually run)

- `scripts/validate-export.sh godot-template/demo/overworld-v1
  godot-template/demo/overworld-v1/context.json` →
  `export-validator: ... approved (validator version 1)`.
- `python3 tools/self_test.py` → all 60 checks pass (shapes, banks, WAV
  headers, loop-length math per stem, all six `layers_for_state` sets,
  transition lookup + fade seconds, two RTPC spot checks).
- In-engine runbook (steps 2–4 above) is **not yet run**: no Godot binary
  exists on this machine (`godot: command not found`). Headless procedure
  when one is available:
  `godot --headless --path godot-template --quit-after 300 res://demo/main.tscn`
  (renders 300 frames, exercises `_ready` + load path), plus
  `godot --headless --check-only --script res://scripts/bank_loader.gd`
  for a parse check. The highest-risk validation remains the **real user
  import** — run the runbook against your build and report back any state
  names the template should adopt.

## 6. Rules (frozen schema — read before extending)

- This track adds no contracts and no generated types: `bun run typegen --
  --check` is unaffected, and nothing under `ui/src/generated/*` is touched.
- The fixture is data, not a contract change: stems + JSON produced by the
  frozen v1 builder. If a shape ever needs to change, it arrives as a new
  `schema_version` + migration note — never as a silent reinterpretation.
- New dirs only (`godot-template/`, this note). The temporary generator was
  run from `core/examples/` and deleted afterward; the tree carries no
  changes outside this track's dirs.
