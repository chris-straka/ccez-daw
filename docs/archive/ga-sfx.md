# GA-2 primer: SFX events — the game says *what happened*, the bank decides *what it sounds like*

New to game SFX? Start here. This note teaches the event concept behind
`core/src/sfx/` (the GA-2 runtime), then shows exactly how to trigger,
humanize, throttle, modulate, and audition one-shots without touching the
frozen v1 bank schema.

## 1. The idea

Game code should never say "play `footstep_gravel_03.wav` at volume 0.7".
It says `player.footstep` — a named **event** — and data decides the rest.
That split is the whole Wwise/FMOD event concept in four mechanisms:

- **Pool**: each event names several clips; every trigger picks one
  uniformly at random. Repeats stop machine-gunning because the *sample*
  changes, not just the timing.
- **Humanization**: per trigger, volume wobbles by +/- `volume_random`,
  pitch by +/- `pitch_random` semitones, onset by a few ms of timing
  spread. Small numbers — humans notice identical repeats far more than
  they notice detune.
- **Throttles**: `cooldown_ms` drops triggers that arrive too fast
  (twenty collision callbacks in one frame = one thud), and
  `max_polyphony` steals the oldest voice instead of stacking
  unboundedly (ten explosions at once = N loud ones, not a clipped wall).
- **RTPC**: a live game param (speed, threat, health) bends the mix
  continuously — louder, brighter, harsher — through bindings that map a
  declared param range onto any `target_node:target_param` mix address,
  the same universal addressing v0 automation uses.

All four read one input (`contracts/game-state.md`): a named state plus
continuous values. The game posts snapshots; audio reacts. Snapshots are
evaluated, never stored — no ops, no undo.

## 2. The pieces

- `contracts/sfx-bank-schema.md` — the frozen v1 shapes (`SfxBank`,
  `SfxEvent`, `RtpcBinding`; machine truth in `core/src/game_audio.rs`).
- `core/src/sfx/runtime.rs` — `SfxRuntime::trigger`: pool pick,
  humanization, cooldown/polyphony, RTPC resolution. Seeded RNG
  (`SeededRng`, xorshift64*), so identical seeds give identical voices —
  exports reproduce exactly.
- `core/src/sfx/audition.rs` — `render_voice`: deterministic offline
  preview. Banks reference clips by id, so audition synthesizes a
  per-clip stand-in tone shaped by the voice (gain, detune, timing
  silence) plus `volume`/`pitch`/`cutoff`-family RTPC targets.
- `core/src/sfx/mod.rs` — module docs and re-exports.
- `core/src/sfx/bank.rs` — `build_bank`: assemble a bank from the v0
  project's clip inventory (`BankEventDraft` per event), failing with one
  message per violation (empty pool, duplicate event id, dangling clip
  id — validator rule 2). `bank_to_json` / `bank_from_json` round-trip
  the verbatim `bank_<id>.json` the export package ships.

## 3. Trigger an event (the whole API in ten lines)

```rust
use ccez_core::game_audio::GameStateSnapshot;
use ccez_core::sfx::{SfxRuntime, TriggerOptions};

let mut rt = SfxRuntime::new(2026); // seed: same seed, same voices
let voice = rt.trigger(
    &bank,            // &SfxBank: frozen v1 data, read-only
    "player.footstep",// event id
    &snapshot,        // &GameStateSnapshot: what the game just posted
    &declared,        // &[GameStateParam]: the author-declared param surface
    now_ms,           // u64: caller clock, drives cooldown/polyphony
    &TriggerOptions { timing_spread_ms: 12.0 },
);
match voice {
    Ok(v) => println!("{} @ {:+.1} st, {:.0} ms late", v.clip_id, v.pitch_semitones, v.delay_ms),
    Err(reject) => println!("silent by rule: {:?}", reject.kind), // Cooldown, EmptyPool, ...
}
```

RTPC math per binding: clamp the game value into the declared
`[min, max]`, normalize to 0–1, map linearly onto the binding's
`[min, max]`. Unknown param names are skipped quietly (the game ships
params the mix doesn't use yet); a declared-but-missing value reads as
the param `default`.

## 4. Audition it (the GA-2 validation loop)

```rust
use ccez_core::sfx::{AuditionConfig, render_voice};

let cfg = AuditionConfig::default(); // 44.1 kHz, 1 s
let frames = (cfg.sample_rate as f32 * cfg.seconds) as usize;
let samples: Vec<f32> = render_voice(&voice?, cfg.sample_rate, frames);
```

Same voice renders bit-identical samples every run; different seeds (or
later triggers in one stream) render audibly different ones. That is
what `cargo test -p ccez-core --lib sfx` proves: pool coverage,
spread bounds, cooldown/polyphony behavior, RTPC clamp-map, filter
darkening, leading-silence timing, and seed determinism.

## 5. Build a bank from the project (no hand-written JSON)

Banks reference v0 clips by string id, so the builder checks your drafts
against the live clip inventory instead of trusting them:

```rust
use ccez_core::sfx::{BankEventDraft, build_bank};

let mut step = BankEventDraft::new("player.footstep", "Footstep",
    vec!["clip_step_a".into(), "clip_step_b".into()]);
step.pitch_random = 2.0;   // +/- semitones per trigger
step.cooldown_ms = 90;     // bursts inside 90 ms drop

match build_bank("bank_demo", "Demo", &project, vec![step]) {
    Ok(bank) => println!("{} event(s) ready", bank.events.len()),
    Err(errors) => eprintln!("bank rejected:\n- {}", errors.join("\n- ")),
}
```

Two validation halves, matching the export validator's rule 2:
`validate_bank` (no project needed — empty pools, duplicate event ids)
plus `resolve_against_project` (needs the v0 clip list — dangling ids).
`build_bank` runs both and returns every violation at once, so one
fix-and-retry pass clears the whole list. Malformed JSON on the way back
in is an `Err(String)`, never a panic — banks arrive from disk and from
other tools, and a corrupt file should read as red text, not a crash.

## 6. Rules (frozen schema — read before extending)

- Never add a field to `SfxEvent`/`SfxBank`/`RtpcBinding` in place.
  Timing spread lives in `TriggerOptions` (per-trigger, unstored)
  precisely because v1 has no timing field — a stored timing field
  needs a new schema version + migration note, not a quiet struct edit.
- Empty `clip_ids` is an authoring bug: the runtime rejects the trigger
  (`EmptyPool`) and the export validator fails the bank. Silence must be
  a mix decision, never a missing file.
- Unknown game params stay quiet; unknown `target_node:target_param`
  mix addresses must fail loudly at export (GA-4) — a typo'd mix target
  is the bug this asymmetry exists to catch.
- No new types were added to the typegen surface, so
  `ui/src/generated/*.ts` is untouched by this track. If you add a
  serialized contract type later, extend `core/src`, regenerate with
  `bun run typegen`, and confirm the v0 diff is purely additive.
