//! GA-1 agent 1: adaptive music runtime — vertical layering.
//!
//! Teaching note: adaptive music answers "what sounds *right now*?" from the
//! game state, not the timeline. Each [`CueLayer`](crate::game_audio::CueLayer)
//! is a pre-rendered mono **stem**; the engine mixes the audible stems every
//! sample. A [`GameStateSnapshot`](crate::game_audio::GameStateSnapshot) flips
//! which layers are audible, and a
//! [`TransitionRule`](crate::game_audio::TransitionRule) decides *how fast*
//! the flip happens (immediate cut vs. linear crossfade). Mute/solo are
//! audition overrides on top of the state mix.
//!
//! Layout: [`engine`] holds the sample-accurate mixer
//! ([`engine::AdaptiveEngine`]); this module re-exports its public surface.
//! Frozen v1 contracts (`contracts/adaptive-cue-schema.md`,
//! `contracts/game-state.md`) are never touched here — this crate only
//! *evaluates* them via `layers_for_state` / `transition_for`.
//!
//! Horizontal resequencing details (bar-quantized `BarWait`, one-shot
//! `Stinger`) belong to the GA-1 agent 2 / transport slice: this engine
//! degrades those kinds to a clean [`Degraded::Yes`] cut so a sparse
//! transition graph can never glitch, and reports the fallback explicitly.

pub mod audition;
pub mod engine;
pub mod reseq;

pub use audition::{
    audible_layers, effective_transition, layer_mix, render_audition, rms as audition_rms,
    AuditionConfig, AuditionTransition,
};
pub use engine::{AdaptiveEngine, Degraded, LayerOverride};
