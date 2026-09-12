# GA-1 primer: adaptive music is stems mixed by game state

New to adaptive music? Start here. This note teaches the one big idea behind
GA-1's vertical slice — **the mix is a function of the game state, evaluated
per sample** — then lists exactly what the vertical-layer engine (`core/src/
adaptive/engine.rs`) does so the transport slice, simulator, and exporter can
build on it without forking it.

## 1. The idea

Linear music asks "what plays at bar 33?". A game can't answer that — the
player might be picking flowers or fighting a boss at bar 33. Vertical
layering sidesteps the question: instead of one fixed mix, you ship a stack
of **stems** (bed, drums, brass...) and a rulebook saying which stems sound
in which game state:

- `explore` → bed only. `combat` → bed + drums + brass.
- The always-on underscore is a layer with **empty `states`** — the contract's
  way of saying "never drops" (`contracts/adaptive-cue-schema.md`).
- A state change with **no written rule cuts cleanly**. Authors score only
  the transitions players will hear (one `Fade` for explore→combat), not an
  NxN matrix. Sparseness is a feature, not a gap.

The engine's whole job is turning that rulebook into per-sample gains:
`out[t] = Σ stems × layer volume × state gain × mute/solo`. Three of those
terms are constant for a whole render; only the state gain moves, and only
during a fade.

## 2. Sample-accurate fades (why not just crossfade blocks?)

A 4-beat fade at 120 BPM lasts 96,000 samples at 48 kHz. If the fade starts
"at the start of the next audio block", its timing wobbles by up to a block
(~a millisecond or three) depending on where the game thread posted the
snapshot. Players hear that as flammy, inconsistent transitions.

`AdaptiveEngine::post_snapshot_at(snapshot, at)` pins the transition to an
**exact sample**: every layer gets a linear ramp `[at, at + fade_samples)`
computed from its gain *at that sample* (an interrupted fade re-ramps from
mid-flight, never jumps). Rendering before the boundary still yields the old
mix; the boundary sample starts the ramp; the end sample lands exactly on
the new mix. The render test asserts all three points, so the guarantee is
checked, not hoped for.

Tempo matters here: `fade_beats` convert to samples at the *cue's own* tempo
(`beats × 60 / tempo × sample_rate`), because each cue carries its BPM.

## 3. The pieces (all GA-1 vertical slice)

- `core/src/adaptive/engine.rs` — `AdaptiveEngine`: owns one cue, starts in
  `default_state` with settled gains, mixes caller-supplied mono stems
  (looped by wrapping; a missing stem is silence, never an error — dangling
  ids fail the *export validator*, per `contracts/export-package.md`).
- `post_snapshot_at` → `Degraded::No` for `Cut`/`Fade` and the no-rule cut
  fallback; `BarWait`/`Stinger` degrade to an honest `Degraded::Yes` cut —
  the transport slice (`core/src/adaptive/reseq.rs`, GA-1 agent 2) owns bar
  grids and stinger one-shots, so the vertical engine reports the fallback
  instead of faking it.
- `set_override` — mute/solo audition flags per layer. Solo forces its layer
  audible even when the state says otherwise (isolate a stem without posting
  snapshots); solo-any wins, so non-soloed layers go quiet while a solo is
  armed. Unknown ids are ignored (audition UI may race cue edits).
- Five `cargo test adaptive::engine` tests: state-change remix (audible sets
  + fade midpoint + exact-sample cut), fade linearity, mute/solo, BarWait/
  Stinger degradation, missing-stem silence.

## 4. Extend it (rules)

- New transition flavors go in `reseq.rs` (planning *when*) and the engine
  (executing gains) — never as special cases in the simulator or exporter.
- New per-trigger or per-state behavior that needs storing must clear the
  frozen v1 bar: breaking schema change = new version + migration note.
  Runtime-only params (like SFX timing spread) stay out of the schema.
- The engine never touches v0 `Project`, the op log, or generated TS: stems
  arrive as plain `Vec<f32>` maps, so `bun run check` drift gates stay green.

## 5. Horizontal resequencing: what happens *when the state changes*

The vertical engine answers "what sounds now?". The transport slice
(`core/src/adaptive/reseq.rs`, GA-1 agent 2) answers the follow-up: "**when**
does the new section take over, and what plays in between?" A cue section in
v1 is just a named state rendered through the cue's layers — the `explore`
section *is* `layers_for_state("explore")` at the cue tempo — so resequencing
is moving from one state-section to another under a `TransitionRule`.

Four transition flavors, one rulebook (`contracts/adaptive-cue-schema.md`):

- `Cut` — swap layers immediately. Also the fallback: a state change with
  **no written rule cuts cleanly**, so authors score only the moments players
  will hear.
- `Fade` — start a crossfade now over `fade_beats`; per-buffer gains come
  from `fade_gains` (linear `(out, in)`, clamped: full-old before the ramp,
  full-new after). A `Fade` with non-positive `fade_beats` degrades to a cut.
- `BarWait` — hold the old layers until the **next bar line**, then cut.
  `next_bar_line` does the grid math in absolute transport beats at the cue
  tempo (a request landing exactly on the downbeat fires at once — zero wait).
  Cutting mid-bar feels like tripping; waiting for the downbeat feels
  intentional. That is the whole musical reason this flavor exists.
- `Stinger` — fire the `stinger_cue_id` one-shot now, then cut on its
  downbeat (`request beat + stinger_beats`, the stinger length the engine
  knows). The sting masks the edit point: players hear a brass hit, not a
  splice.

Rules match **exactly** on `(from_state, to_state)` — continuous snapshot
values (`threat`, `health`) never gate a transition; they belong to RTPC and
layer matching. Time is always **beats at the cue's own tempo**
(`AdaptiveCue::tempo`); seconds conversion happens at the engine edge.

`Resequencer` is the state machine tying it together: the engine calls
`request` from its snapshot handler (immediate plans apply at once; `BarWait`
/ `Stinger` arm a pending switch) and `poll` from its per-buffer transport
callback (fires the armed switch when its beat arrives). A new request always
**supersedes** an armed-but-unfired switch — the player changed their mind
mid-bar, latest game state wins — and re-requesting the already-armed target
is a no-op so stingers never double-fire. Nine `cargo test
adaptive::reseq` tests pin this down, including the transition-on-bar test
(beat 5 of 4/4 holds combat until beat 8, then cuts on the downbeat).

Division of labor: `reseq.rs` plans *when* (beats, pending switches),
`engine.rs` executes *how loud* (per-sample gains). New flavors land in both
halves — never as special cases in the simulator or exporter.
