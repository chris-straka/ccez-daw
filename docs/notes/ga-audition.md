# GA-3 primer: the audition simulator — rehearse the game before the game exists

New to game audio? Start here. This note teaches the rehearsal-room idea
behind `ui/src/gameaudio/` (the GA-3 simulator), then shows exactly how to
post states, bend params, pin a timeline, and fire events without touching
the frozen v1 schemas or the audio hardware.

## 1. The idea

GA-1 built the mixer (`AdaptiveEngine`: stems × state) and GA-2 built the
one-shot path (`SfxRuntime`: events × throttles × RTPC). Both read one
input (`contracts/game-state.md`): a named state plus continuous values.
The problem is iteration speed — nobody wants to rebuild the game to hear
whether explore→combat lands.

The simulator closes that loop: it *pretends to be the game*, posting the
same snapshots the engine will one day receive, and previews what the mix
would do. Three rules keep the pretense honest:

- **Same math, no hardware.** Layer audibility mirrors
  `AdaptiveCue.layers_for_state` (empty `states` = always-on bed),
  transitions mirror `transition_for` (no rule = `Cut`), RTPC mirrors the
  GA-2 clamp → normalize → linear-map rule. If the preview and the engine
  ever disagree, the engine wins and the preview is the bug.
- **Snapshots are evaluated, never stored.** Posting a state writes no op,
  no undo entry — the game-state contract says so explicitly. Timeline pins
  are audition-clock bookmarks (beat → snapshot), not project data.
- **Degradation is labeled, not faked.** `BarWait`/`Stinger` need the
  transport slice (bar grids, stinger one-shots). The simulator plays an
  honest `Cut` and says so in the log (`sim plays Cut`), exactly like the
  engine's `Degraded::Yes`.

## 2. The pieces (all GA-3, all under `ui/src/gameaudio/`)

- `model.ts` — pure functions, zero DOM: `layersForState`,
  `transitionFor`/`effectiveKind`/`isDegraded`, `normalizeParam`/
  `valueFor`/`resolveRtpc`, `previewGains` (linear per-layer ramps over a
  fade window), transport sync (`makeTransport`/`advanceTransport`,
  beat↔second↔sample converters), timeline pins
  (`addTimelineEntry`/`snapshotAt`), and a seeded `fireEvent` mirroring
  the GA-2 throttle shape (uniform pool pick, cooldown drop, oldest-steal
  polyphony, typed rejects).
- `sample.ts` — demo fixtures: a bed + drums + brass cue (explore→combat
  `Fade` over 4 beats, combat→explore `BarWait`), a click + footstep bank
  (footstep bends `bus_sfx:volume` with `threat`), three declared params
  (`threat` 0–1, `speed` m/s, `health` hp).
- `Audition.tsx` — the Solid view: state buttons, live param sliders with
  per-binding RTPC readouts, transport play/pause + tempo with beat/second
  readout, pin-and-jump timeline, per-event fire buttons, a scrolling log —
  and a hot canvas driven by `requestAnimationFrame` that draws per-layer
  preview gains (bar height = gain × layer volume) against the running
  audition clock. The rAF loop advances the transport by wall-clock delta
  (clamped to 0.25 s so backgrounded tabs don't time-travel) and paints
  every frame.
- `index.ts` — re-exports.

## 3. Rehearse a transition (the whole loop in ten lines)

```ts
import { layersForState, effectiveKind, previewGains } from "./ui/src/gameaudio/model";
import { sampleCue } from "./ui/src/gameaudio/sample";

const cue = sampleCue();
console.log(layersForState(cue, "explore").map((l) => l.id)); // ["bed"]
console.log(effectiveKind(cue, "explore", "combat"));          // "Fade"
const mid = previewGains(cue, "explore", "combat", 10, 4, 12); // pin @ beat 10, check beat 12
console.log(mid["drums"].toFixed(2));                          // "0.50" — halfway up the 4-beat ramp
```

Sliders follow the same tolerance the engine uses: drag `threat` past 1
and it clamps; clear a value and the declared `default` sounds; a binding
naming a param nobody declared is skipped quietly (future game params stay
quiet, typo'd mix targets fail loudly at export — GA-4's asymmetry).

## 4. Fire an event (audition throttles included)

```ts
import { fireEvent, makeFireRuntime } from "./ui/src/gameaudio/model";

const rt = makeFireRuntime(2026); // same seed, same voices — reproducible rehearsal
const res = fireEvent(bank, rt, "player.footstep", snapshot, declared, Date.now(), transport.beat);
if (res.ok) console.log(`${res.voice.clip_id} @ ${res.voice.gain.toFixed(2)}`);
else console.log(`silent by rule: ${res.kind}`); // Cooldown, EmptyPool, UnknownEvent, ...
```

The UI RNG is a local stand-in (deterministic per seed, uniform over the
pool, humanization inside the declared spreads) — it rehearses throttle
*behavior*, not bit-identical Rust voices. Cross-language parity was
deliberately not bought: the export path replays through the real
`SfxRuntime`.

## 5. Rules (frozen schema — read before extending)

- Never add a stored field to cue/bank/snapshot shapes in place. Timing
  spread already travels per-trigger (`TriggerOptions` / `timingSpreadMs`)
  precisely because v1 has no timing field — a stored one needs a new
  schema version + migration note.
- New transition flavors belong in `core/src/adaptive/reseq.rs` (planning
  *when*) and the engine (executing gains) — never as special cases in
  this simulator. The simulator degrades and labels.
- `ui/src/generated/*.ts` is hand-off: extend `core/src`, regenerate with
  `bun run typegen`, keep the v0 diff purely additive. This track adds no
  serialized types, so the drift gate is unaffected.
- Validation is `bun test gameaudio-audition` (6 scripted tests: state
  change, preview ramp, params/RTPC, transport sync, event throttles,
  timeline) plus `bun run check` (`tsc --noEmit`) green alongside the full
  95-test UI suite.
