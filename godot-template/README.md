# godot-template — GA-4 playback layer for Godot 4.7.1 (GDScript only)

> C# port for HLL lives in the game repo: `~/SWE/games/hll/audio/Ccez*.cs`
> (CcezBankLoader, CcezMusicPlayer, CcezSfxPlayer). Logic mirrors
> `scripts/*.gd` line for line; behavior deltas are bugs.

Drop-in audio playback for a CCEZ game-audio export package. Full teaching
primer + 5-minute runbook: `docs/notes/s-godot.md`.

## Layout

```text
godot-template/
  project.godot            # Godot 4.7 project, main scene = demo/main.tscn
  scripts/
    bank_loader.gd         # CcezBankLoader: validates + loads a v1 package
    music_player.gd        # CcezMusicPlayer: looping stems, crossfaded states
    sfx_player.gd          # CcezSfxPlayer: event pools, humanization, RTPC
  demo/
    main.tscn / main.gd    # test scene: 6 state buttons + 4 param sliders
    cue_overworld.json     # title-side cue map (tempo, layer states, rules)
    overworld-v1/          # fixture package built by the real DAW export path
  tools/
    self_test.py           # headless self-test (no Godot binary needed)
    regen_fixture.rs       # fixture generator source (copy to core/examples/)
```

## Quick start

1. `python3 tools/self_test.py` — validates the fixture without Godot.
2. Open this dir in Godot 4.7.1 (mono or standard; no C# used), run the
   main scene, click states, drag sliders.
3. Ship your own music: export a GA-4 package from the DAW, copy the cue's
   tempo/layer-states/transitions into a cue map, point `main.gd` at it.

## Regenerating the fixture

```sh
cp tools/regen_fixture.rs core/examples/s1_fixture_gen.rs
cargo run --manifest-path core/Cargo.toml --example s1_fixture_gen -- godot-template/demo/overworld-v1
scripts/validate-export.sh godot-template/demo/overworld-v1 godot-template/demo/overworld-v1/context.json
rm core/examples/s1_fixture_gen.rs
```
