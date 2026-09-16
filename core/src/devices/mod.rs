//! Track D agent 1: native devices + containers.
//!
//! A native device is DSP you can run anywhere: a pure-Rust kernel over
//! mono `f32` slices plus a frozen [`Node`](crate::model::Node) carrying
//! its params. No hardware, no downloaded binary, no new schema — the
//! project file just holds `Node`s, and this module knows how they sound.
//!
//! The four ideas, each in its file:
//!
//! - [`kernel`]: portable DSP kernels (gain, lowpass, highpass, delay,
//!   distortion). No allocation inside `process`, state in explicit structs
//!   the caller owns — the same shape a future WASM embedding calls.
//! - [`class`]: the device-type convention. A frozen `Param` cannot hold a
//!   string, so the class rides as a numeric code in the
//!   [`DEVICE_CLASS_PARAM`](class::DEVICE_CLASS_PARAM) param (missing or
//!   unknown = [`DeviceClass::Foreign`](class::DeviceClass), which renders
//!   as pass-through — the same tolerance the graph shows half-specified
//!   chains). Same trick Track C used for the oversampling flag.
//! - [`rack`]: nesting, splits, macros, presets, and the renderer.
//!   Structure the flat `Track.device_ids` order cannot express lives in
//!   the [`Rack`](rack::Rack) sidecar (the Track G `VcaGroup` precedent:
//!   UI-local data riding *alongside* the project, never in it).
//! - [`midifx`]: native MIDI FX (arpeggiator, chord generator,
//!   velocity/humanize). Note transforms over [`MidiClip`](crate::midi::MidiClip),
//!   param'd by the same frozen-`Node` class convention; audio-transparent
//!   in the rack renderer, applied in chain order via `apply_midi_chain`.
//! - `Spatial` (`DeviceClass::Spatial`, code 11): first-order ambisonic
//!   placement. DSP lives in [`crate::spatial`] (encode `WXYZ`, HRTF-free
//!   stereo decode); the rack insert holds the mono `(L+R)` projection and
//!   the mixer monitor path renders the full stereo pair.
//! - [`sampler`]: the Simpler-style sampler model. The device `Node`
//!   holds play params (`transpose`, `gain`, `attack`, `release`,
//!   `cutoff`); the audio lives in a [`SampleBank`](sampler::SampleBank)
//!   beside the project, and [`DrumRack`](sampler::DrumRack) maps 16
//!   drum pads to `(track, note)` strike targets.
//! - [`wasm`]: the devices-to-WASM seam. Named, refused, documented —
//!   untrusted DSP runs behind a sandbox boundary when it lands.
//! - [`presets`]: the curated native preset library. Named param maps per
//!   device class, stamped through the existing param/op paths (undoable),
//!   frozen to [`DevicePreset`](rack::DevicePreset) JSON, and surfaced in
//!   the browser search as preset [`LibraryItem`](crate::library::LibraryItem)s.
//!
//! Reads the frozen `Node` / `Param` / `Track` types only and registers
//! nothing in `emit.rs`, so the typegen drift gate is unaffected. The
//! Track C chain conventions are reused, not reinvented: per-device
//! wet/dry comes from [`crate::plugins::chain::wet_dry`], the 2x flag from
//! [`crate::plugins::chain::is_oversampled`].

pub mod class;
pub mod kernel;
pub mod midifx;
pub mod presets;
pub mod rack;
pub mod sampler;
pub mod wasm;

pub use class::{DeviceClass, DEVICE_CLASS_PARAM};
pub use kernel::DeviceError;
pub use rack::{Macro, MacroBinding, Rack, RackError, RackNode, RackState, Split};
pub use midifx::{apply_midi_chain, arp_pattern, arp_render};
pub use presets::{apply_preset, find_preset, native_presets, preset_library_items, preset_op_payloads, to_device_preset, NativePreset, NATIVE_PRESETS};
pub use sampler::{DrumPad, DrumRack, SampleBank, SampleBuffer, DRUM_PAD_COUNT};
pub use wasm::{WasmError, WasmNote, load_wasm_module};
#[cfg(feature = "wasm-runtime")]
pub use wasm::{
    WasmClip, WasmGain, WasmKDelay, WASM_CLIP_CODE, WASM_CLIP_DEFAULT, WASM_CLIP_MAX,
    WASM_GAIN_CODE, WASM_GAIN_MAX, WASM_KDELAY_CODE_DELAY, WASM_KDELAY_CODE_FEEDBACK,
    WASM_KDELAY_MAX_DELAY,
};
