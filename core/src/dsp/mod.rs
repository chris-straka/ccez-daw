//! Track D (agent 2): native DSP suite — pure-Rust kernels.
//!
//! Teaching note: every device here is a *kernel*, not a plugin. A kernel
//! is a plain struct with a `reset()` and a `process()` that turns one
//! block of mono samples into another. No threads, no allocation inside
//! `process` (callers pass the output slice), no hardware — so the same
//! code runs in the realtime callback, the offline renderer, and the
//! unit tests below. Each kernel also speaks the frozen v0 schema:
//! [`default_node`](crate::dsp::synth::Synth::default_node)-style
//! constructors return an ordinary [`Node`](crate::model::Node) of kind
//! `Device` whose `params` carry the knob values, so chains
//! (`core/src/plugins/chain.rs`) and the op log (`ParamSet`) can drive
//! them with zero schema change — the typegen drift gate stays green.
//!
//! Map: [`synth`] (source oscillator + AR envelope), [`eq`] (3-band
//! biquad stack), [`comp`] (feedforward peak compressor),
//! [`reverb`] (Schroeder comb+allpass), [`delay`] (feedback delay line).
//! Every device reports [`latency_samples`](Where::latency_samples) = 0
//! (no lookahead), so Track B's compensation machinery needs no update.

pub mod comp;
pub mod delay;
pub mod eq;
pub mod reverb;
pub mod synth;

pub use comp::Compressor;
pub use delay::Delay;
pub use eq::Eq;
pub use reverb::Reverb;
pub use synth::Synth;
