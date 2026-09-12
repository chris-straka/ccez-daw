//! GA-2 SFX event runtime: named game events resolve to sounding voices.
//!
//! Teaching note: the game never triggers audio files — it triggers a *name*
//! (`player.footstep`), and this runtime decides what sounds. That decision
//! has four parts, mirroring the frozen contract in
//! `contracts/sfx-bank-schema.md` (machine truth:
//! `crate::game_audio::{SfxBank, SfxEvent, RtpcBinding}`):
//!
//! 1. **Pool**: pick one `clip_ids` entry uniformly at random per trigger.
//! 2. **Humanization**: +/- `volume_random` (linear) and +/- `pitch_random`
//!    (semitones) drawn from a seeded RNG, so exports are reproducible.
//! 3. **Throttles**: `cooldown_ms` drops bursts; `max_polyphony` steals the
//!    oldest voice instead of stacking unboundedly.
//! 4. **RTPC**: each binding maps a declared game param (clamped to its
//!    declared `[min, max]`, normalized to 0–1) linearly onto the binding's
//!    output `[min, max]` addressed at `target_node:target_param`.
//!
//! Timing spread (a few ms of random onset delay, the third humanization
//! axis) is intentionally *not* a stored event field — the frozen v1
//! `SfxEvent` has no timing field, and breaking that schema needs a new
//! version + migration note. It lives in [`TriggerOptions`], a per-trigger
//! runtime parameter, so audition and gameplay can humanize timing without
//! touching the bank format.
//!
//! Unknown game params in `rtpc` are ignored per trigger (quiet tolerance,
//! same as snapshots); empty `clip_ids` rejects the trigger (loud authoring
//! bug, per contract).

pub mod audition;
pub mod bank;
pub mod runtime;

pub use audition::{AuditionConfig, render_voice};
pub use bank::{BankEventDraft, bank_from_json, bank_to_json, build_bank, validate_bank};
pub use runtime::{
    ResolvedRtpc, SeededRng, SfxRuntime, TriggerOptions, TriggerReject, TriggerRejectKind, Voice,
};
