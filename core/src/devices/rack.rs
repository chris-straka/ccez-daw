//! Racks: nesting, splits, macros, presets, and the renderer.
//!
//! Teaching note: a flat insert chain (`Track.device_ids` in order) can
//! say "A then B". It cannot say "A then (B beside C, mixed back
//! together)" or "one knob drives B's cutoff and C's drive together".
//! That structure lives here, in the [`Rack`] sidecar — a plain serde
//! struct riding *alongside* the project, the way Track G's `VcaGroup`
//! rides alongside the mixer. The project file never changes shape; the
//! rack file (or op payload, or engine asset) carries the rest.
//!
//! - Nesting: a [`DeviceClass::Container`](super::class::DeviceClass)
//!   device id maps to a child list in `Rack.containers`. Containers nest
//!   by referencing other container ids; cycles are [`RackError::BadRack`].
//! - Splits: [`Split`] renders every branch from the same input and sums
//!   them with per-branch gains (parallel compression's "squash beside
//!   dry", multiband's poor-man's crossover — the idea, not the DSP).
//! - Macros: [`Macro`] maps one 0..=1 knob onto many params at once
//!   (`value -> min + value * (max - min)`, clamped to each param's own
//!   range). Render-time overrides only: the project's stored params are
//!   never rewritten, so a macro tweak cannot corrupt a preset.
//! - Presets: [`DevicePreset`] is one reusable device as JSON — class,
//!   name, full params. Save any tuned node, stamp out copies with
//!   [`DevicePreset::instantiate`].
//!
//! [`render_rack`] is the offline renderer: serial chain with recursive
//! container expansion, per-device wet/dry from the Track C chain
//! conventions, 2x oversampling behind the chain's flag (naive linear
//! resample — the polyphase upgrade is noted below, same as Track C),
//! and explicit [`RackState`] threaded through so filters and delays
//! remember across blocks.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::model::{Node, Project};
use crate::plugins::chain::{apply_wet_dry, is_oversampled, wet_dry};

use super::class::{classify, DeviceClass, DEVICE_CLASS_PARAM};
use super::kernel::{
    delay_process, distortion_process, gain_process, highpass_process, lowpass_process,
    sampler_note_off, sampler_note_on, sampler_process, ATTACK_PARAM, CUTOFF_PARAM,
    DELAY_SAMPLES_PARAM, DRIVE_PARAM, FEEDBACK_PARAM, GAIN_PARAM, RELEASE_PARAM,
    TRANSPOSE_PARAM, DelayState, HighpassState, LowpassState, SamplerState,
};
use super::sampler::SampleBank;
use super::DeviceError;

/// Oversampling factor applied behind the chain's `oversampling` flag.
/// Naive linear 2x (upsample → process → average down). Audibly better
/// than nothing for nonlinear kernels, measurably not polyphase — the
/// real resampler is the same follow-up Track C already names.
pub const RACK_OVERSAMPLE_FACTOR: usize = 2;

#[derive(Debug, Clone, PartialEq)]
pub enum RackError {
    UnknownTrack(String),
    UnknownContainer(String),
    BadRack(String),
    Device(DeviceError),
}

impl std::fmt::Display for RackError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownTrack(t) => write!(f, "unknown rack track `{t}`"),
            Self::UnknownContainer(c) => write!(f, "unknown rack container `{c}`"),
            Self::BadRack(m) => write!(f, "bad rack: {m}"),
            Self::Device(e) => write!(f, "rack device: {e}"),
        }
    }
}

impl std::error::Error for RackError {}

impl From<DeviceError> for RackError {
    fn from(e: DeviceError) -> Self {
        Self::Device(e)
    }
}

pub type Result<T> = std::result::Result<T, RackError>;

/// One child of a container: a leaf device, a nested container, or a split.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum RackNode {
    Device(String),
    Rack(String),
    Split(Split),
}

/// A parallel split: every branch renders from the same input, outputs
/// sum with per-branch `gains` (missing entries read as 1.0; length
/// mismatch with branches is [`RackError::BadRack`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Split {
    pub id: String,
    pub branches: Vec<Vec<RackNode>>,
    pub gains: Vec<f64>,
}

/// One macro binding: `device:param` follows the macro knob across
/// `[min, max]`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MacroBinding {
    pub device: String,
    pub param: String,
    pub min: f64,
    pub max: f64,
}

/// One macro knob: a 0..=1 value driving many params at once.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Macro {
    pub id: String,
    pub label: String,
    pub value: f64,
    pub bindings: Vec<MacroBinding>,
}

impl Macro {
    pub fn new(id: &str, label: &str, value: f64, bindings: Vec<MacroBinding>) -> Result<Self> {
        if id.is_empty() {
            return Err(RackError::BadRack("macro id must be non-empty".to_string()));
        }
        if !value.is_finite() || !(0.0..=1.0).contains(&value) {
            return Err(RackError::BadRack(format!("macro value {value} out of 0..=1")));
        }
        Ok(Self {
            id: id.to_string(),
            label: label.to_string(),
            value,
            bindings,
        })
    }

    pub fn set_value(&mut self, value: f64) -> Result<()> {
        if !value.is_finite() || !(0.0..=1.0).contains(&value) {
            return Err(RackError::BadRack(format!("macro value {value} out of 0..=1")));
        }
        self.value = value;
        Ok(())
    }
}

/// The structure the flat chain order cannot say: container children plus
/// macro knobs. Empty rack = flat chain (every device renders in
/// `device_ids` order) — racks add, never reinterpret.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Rack {
    pub containers: BTreeMap<String, Vec<RackNode>>,
    pub macros: Vec<Macro>,
}

impl Rack {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn to_json(&self) -> Result<String> {
        serde_json::to_string(self).map_err(|e| RackError::BadRack(e.to_string()))
    }

    pub fn from_json(json: &str) -> Result<Self> {
        serde_json::from_str(json).map_err(|e| RackError::BadRack(e.to_string()))
    }

    pub fn set_macro(&mut self, id: &str, value: f64) -> Result<()> {
        match self.macros.iter_mut().find(|m| m.id == id) {
            Some(m) => m.set_value(value),
            None => Err(RackError::BadRack(format!("unknown macro `{id}`"))),
        }
    }
}

/// One reusable device as JSON: class, display name, full params
/// (values *and* ranges, so restore is exact — the chain-snapshot rule).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DevicePreset {
    pub class: String,
    pub name: String,
    pub params: Vec<crate::model::Param>,
}

impl DevicePreset {
    /// Freeze a tuned node as a preset. Foreign nodes are refused — a
    /// preset must know its DSP to be reusable.
    pub fn capture(node: &Node) -> Result<Self> {
        let class = classify(node);
        if class == DeviceClass::Foreign {
            return Err(RackError::BadRack(format!(
                "cannot preset foreign node `{}`",
                node.id
            )));
        }
        Ok(Self {
            class: class_name(class).to_string(),
            name: node.name.clone(),
            params: node.params.clone(),
        })
    }

    /// Stamp out a fresh node (new id, preset's params byte-exact).
    /// Unknown class names are [`RackError::BadRack`] — presets never
    /// silently become a different device.
    pub fn instantiate(&self, id: &str) -> Result<Node> {
        let class = parse_class(&self.class)?;
        let mut node = super::class::instantiate(class, id, &self.name);
        node.params = self.params.clone();
        // The stored tag wins over the factory default (they agree unless
        // the JSON was hand-edited; exact restore beats re-derivation).
        Ok(node)
    }

    pub fn to_json(&self) -> Result<String> {
        serde_json::to_string(self).map_err(|e| RackError::BadRack(e.to_string()))
    }

    pub fn from_json(json: &str) -> Result<Self> {
        serde_json::from_str(json).map_err(|e| RackError::BadRack(e.to_string()))
    }
}

fn class_name(class: DeviceClass) -> &'static str {
    match class {
        DeviceClass::Gain => "gain",
        DeviceClass::Lowpass => "lowpass",
        DeviceClass::Highpass => "highpass",
        DeviceClass::Delay => "delay",
        DeviceClass::Distortion => "distortion",
        DeviceClass::Container => "container",
        DeviceClass::Sampler => "sampler",
        DeviceClass::Arpeggiator => "arpeggiator",
        DeviceClass::Chord => "chord",
        DeviceClass::Humanize => "humanize",
        DeviceClass::Foreign => "foreign",
    }
}

fn parse_class(name: &str) -> Result<DeviceClass> {
    match name {
        "gain" => Ok(DeviceClass::Gain),
        "lowpass" => Ok(DeviceClass::Lowpass),
        "highpass" => Ok(DeviceClass::Highpass),
        "delay" => Ok(DeviceClass::Delay),
        "distortion" => Ok(DeviceClass::Distortion),
        "container" => Ok(DeviceClass::Container),
        "sampler" => Ok(DeviceClass::Sampler),
        "arpeggiator" => Ok(DeviceClass::Arpeggiator),
        "chord" => Ok(DeviceClass::Chord),
        "humanize" => Ok(DeviceClass::Humanize),
        _ => Err(RackError::BadRack(format!("unknown preset class `{name}`"))),
    }
}

/// Per-device kernel state, keyed by device id. The caller owns this and
/// threads it through every block — that is the whole "filters remember"
/// mechanism, and why `render_rack` is `&mut state`, not `&state`.
#[derive(Debug, Clone, PartialEq)]
enum DeviceState {
    Lowpass(LowpassState),
    Highpass(HighpassState),
    Delay(DelayState),
    Sampler(SamplerState),
}

/// Open a sampler voice: restarts its read position and attack envelope.
/// Voices are per-device-id (one voice per sampler, the Simpler
/// mono-voice rule); untriggered voices render silence. A `note_on` on a
/// device that later reclassifies to a non-sampler errors loudly on the
/// next render (the state-mismatch rule), never silently retunes.
pub fn trigger_sampler_voice(state: &mut RackState, device_id: &str) {
    let entry = state
        .states
        .entry(device_id.to_string())
        .or_insert_with(|| DeviceState::Sampler(SamplerState::default()));
    if let DeviceState::Sampler(s) = entry {
        sampler_note_on(s);
    }
}

/// Close a sampler voice: the release ramp starts from the current level.
pub fn release_sampler_voice(state: &mut RackState, device_id: &str) {
    if let Some(DeviceState::Sampler(s)) = state.states.get_mut(device_id) {
        sampler_note_off(s);
    }
}

/// Cross-block DSP memory for one rendered track. Opaque to tests except
/// through its audible effect (an echo landing in the *next* block).
#[derive(Debug, Clone, Default)]
pub struct RackState {
    states: BTreeMap<String, DeviceState>,
}

/// Resolve one node's params with macro overrides applied (macros later
/// in the list win; each mapped value clamps to the param's own range).
/// The node itself is untouched — overrides are render-time only.
fn resolved_params(node: &Node, rack: &Rack) -> Vec<crate::model::Param> {
    let mut params = node.params.clone();
    for macro_ in &rack.macros {
        for b in &macro_.bindings {
            if b.device != node.id {
                continue;
            }
            if let Some(p) = params.iter_mut().find(|p| p.id == b.param) {
                p.value = (b.min + macro_.value * (b.max - b.min)).clamp(p.min, p.max);
            }
        }
    }
    params
}

fn param_of(params: &[crate::model::Param], id: &str, default: f64) -> f64 {
    params.iter().find(|p| p.id == id).map(|p| p.value).unwrap_or(default)
}

/// Render one leaf device: kernel → wet, mixed with dry per the Track C
/// chain convention (`dry * dry_in + wet * wet_in`), 2x-naive when the
/// chain's oversampling flag is set. Foreign classes pass through.
fn render_leaf(
    node: &Node,
    params: &[crate::model::Param],
    state: &mut RackState,
    bank: &SampleBank,
    input: &[f32],
    sample_rate: f64,
) -> Result<Vec<f32>> {
    let class = param_of(params, DEVICE_CLASS_PARAM, 0.0);
    let class = DeviceClass::from_code(class);
    let mut wet = vec![0.0f32; input.len()];
    match class {
        DeviceClass::Gain => {
            let g = param_of(params, GAIN_PARAM, 1.0) as f32;
            if is_oversampled(node) {
                process_oversampled2(input, &mut wet, |inn, out| {
                    gain_process(g, inn, out)
                })?;
            } else {
                gain_process(g, input, &mut wet)?;
            }
        }
        DeviceClass::Lowpass => {
            let fc = param_of(params, CUTOFF_PARAM, 1000.0);
            let entry = state
                .states
                .entry(node.id.clone())
                .or_insert_with(|| DeviceState::Lowpass(LowpassState::default()));
            let DeviceState::Lowpass(s) = entry else {
                return Err(RackError::BadRack(format!(
                    "state mismatch for `{}` (class changed mid-session?)",
                    node.id
                )));
            };
            if is_oversampled(node) {
                // Borrow dance: run the 2x wrapper against a scratch state
                // copy, then write back (state must advance exactly once).
                let mut scratch = s.clone();
                process_oversampled2(input, &mut wet, |inn, out| {
                    // Resampling changes the effective rate: scale cutoff.
                    lowpass_process(fc * 2.0, sample_rate * 2.0, &mut scratch, inn, out)
                })?;
                *s = scratch;
            } else {
                lowpass_process(fc, sample_rate, s, input, &mut wet)?;
            }
        }
        DeviceClass::Highpass => {
            let fc = param_of(params, CUTOFF_PARAM, 1000.0);
            let entry = state
                .states
                .entry(node.id.clone())
                .or_insert_with(|| DeviceState::Highpass(HighpassState::default()));
            let DeviceState::Highpass(s) = entry else {
                return Err(RackError::BadRack(format!(
                    "state mismatch for `{}` (class changed mid-session?)",
                    node.id
                )));
            };
            highpass_process(fc, sample_rate, s, input, &mut wet)?;
        }
        DeviceClass::Delay => {
            let d = param_of(params, DELAY_SAMPLES_PARAM, 0.0).max(0.0) as usize;
            let fb = param_of(params, FEEDBACK_PARAM, 0.0);
            let entry = state
                .states
                .entry(node.id.clone())
                .or_insert_with(|| DeviceState::Delay(DelayState::default()));
            let DeviceState::Delay(s) = entry else {
                return Err(RackError::BadRack(format!(
                    "state mismatch for `{}` (class changed mid-session?)",
                    node.id
                )));
            };
            delay_process(d, fb, s, input, &mut wet)?;
        }
        DeviceClass::Distortion => {
            let k = param_of(params, DRIVE_PARAM, 1.0) as f32;
            if is_oversampled(node) {
                process_oversampled2(input, &mut wet, |inn, out| {
                    distortion_process(k, inn, out)
                })?;
            } else {
                distortion_process(k, input, &mut wet)?;
            }
        }
        DeviceClass::Sampler => {
            // An instrument, not an insert: the voice *is* the wet signal
            // (fully-wet defaults mean a chain-head sampler replaces
            // silence with its voice; wet/dry still mixes like any leaf).
            // Missing or empty buffers render silence — a setup gap, the
            // same tolerance as foreign pass-through.
            let entry = state
                .states
                .entry(node.id.clone())
                .or_insert_with(|| DeviceState::Sampler(SamplerState::default()));
            let DeviceState::Sampler(s) = entry else {
                return Err(RackError::BadRack(format!(
                    "state mismatch for `{}` (class changed mid-session?)",
                    node.id
                )));
            };
            let sample: &[f32] = bank
                .get(&node.id)
                .map(|b| b.frames.as_slice())
                .unwrap_or(&[]);
            let transpose = param_of(params, TRANSPOSE_PARAM, 0.0);
            let gain = param_of(params, GAIN_PARAM, 1.0) as f32;
            let attack = param_of(params, ATTACK_PARAM, 0.005);
            let release = param_of(params, RELEASE_PARAM, 0.05);
            let cutoff = param_of(params, CUTOFF_PARAM, 20000.0);
            sampler_process(
                sample, transpose, gain, attack, release, cutoff, sample_rate, s, &mut wet,
            )?;
        }
        DeviceClass::Container => {
            // A container rendered flat (not expanded via the rack map —
            // e.g. an empty entry) is unity: structure defaults to sound.
            wet.copy_from_slice(input);
        }
        // MIDI FX shape notes before the instrument, never samples after
        // it: on the audio path they pass through (see
        // `devices::midifx::apply_midi_chain` for the note path).
        DeviceClass::Arpeggiator | DeviceClass::Chord | DeviceClass::Humanize => {
            wet.copy_from_slice(input);
        }
        DeviceClass::Foreign => {
            wet.copy_from_slice(input);
        }
    }
    let (w, d) = wet_dry(node);
    Ok(input
        .iter()
        .zip(wet.iter())
        .map(|(dry_in, wet_in)| apply_wet_dry(*dry_in, *wet_in, w, d))
        .collect())
}

/// Naive linear 2x: upsample (linear interp) → process at 2x →
/// downsample (pair average). Buffers are the caller's (`wet` is
/// output-sized); the two scratch lines are this wrapper's — the one
/// place the oversample path allocates, one layer above the kernels.
fn process_oversampled2(
    input: &[f32],
    output: &mut [f32],
    mut process: impl FnMut(&[f32], &mut [f32]) -> std::result::Result<(), DeviceError>,
) -> Result<()> {
    assert_eq!(input.len(), output.len());
    let n = input.len();
    if n == 0 {
        return Ok(());
    }
    let mut up = vec![0.0f32; n * RACK_OVERSAMPLE_FACTOR];
    for i in 0..n {
        let next = input.get(i + 1).copied().unwrap_or(input[i]);
        up[2 * i] = input[i];
        up[2 * i + 1] = 0.5 * (input[i] + next);
    }
    let mut up_out = vec![0.0f32; n * RACK_OVERSAMPLE_FACTOR];
    process(&up, &mut up_out)?;
    for i in 0..n {
        output[i] = 0.5 * (up_out[2 * i] + up_out[2 * i + 1]);
    }
    Ok(())
}

struct RenderCtx<'a> {
    nodes: BTreeMap<&'a str, &'a Node>,
    rack: &'a Rack,
    bank: &'a SampleBank,
    sample_rate: f64,
}

fn render_nodes(
    ctx: &RenderCtx,
    state: &mut RackState,
    items: &[RackNode],
    input: &[f32],
    visiting: &mut BTreeSet<String>,
) -> Result<Vec<f32>> {
    let mut signal = input.to_vec();
    for item in items {
        signal = render_item(ctx, state, item, &signal, visiting)?;
    }
    Ok(signal)
}

fn render_item(
    ctx: &RenderCtx,
    state: &mut RackState,
    item: &RackNode,
    input: &[f32],
    visiting: &mut BTreeSet<String>,
) -> Result<Vec<f32>> {
    match item {
        // A container id expands whether or not its device node exists
        // (structure outlives registry gaps); the node's own wet/dry
        // applies when it does. Missing node + no container =
        // pass-through (the graph's implicit-endpoint rule).
        RackNode::Device(id) => {
            let node = ctx.nodes.get(id.as_str());
            match ctx.rack.containers.get(id.as_str()) {
                Some(children) => {
                    let children = children.clone();
                    if !visiting.insert(id.clone()) {
                        return Err(RackError::BadRack(format!(
                            "container cycle through `{id}`"
                        )));
                    }
                    let out = render_nodes(ctx, state, &children, input, visiting)?;
                    visiting.remove(id);
                    // The container's own wet/dry still applies (a rack
                    // macro bus fades its whole subtree); missing node =
                    // fully wet (unity on the subtree).
                    let (w, d) = node.map(|n| wet_dry(n)).unwrap_or((1.0, 0.0));
                    Ok(input
                        .iter()
                        .zip(out.iter())
                        .map(|(dry_in, wet_in)| apply_wet_dry(*dry_in, *wet_in, w, d))
                        .collect())
                }
                None => match node {
                    Some(leaf) => {
                        let params = resolved_params(leaf, ctx.rack);
                        render_leaf(leaf, &params, state, ctx.bank, input, ctx.sample_rate)
                    }
                    None => Ok(input.to_vec()),
                },
            }
        }
        RackNode::Rack(id) => {
            let children = ctx
                .rack
                .containers
                .get(id.as_str())
                .ok_or_else(|| RackError::UnknownContainer(id.clone()))?
                .clone();
            if !visiting.insert(id.clone()) {
                return Err(RackError::BadRack(format!("container cycle through `{id}`")));
            }
            let out = render_nodes(ctx, state, &children, input, visiting)?;
            visiting.remove(id);
            Ok(out)
        }
        RackNode::Split(split) => {
            if split.branches.is_empty() {
                return Err(RackError::BadRack(format!("split `{}` has no branches", split.id)));
            }
            if !split.gains.iter().all(|g| g.is_finite()) {
                return Err(RackError::BadRack(format!(
                    "split `{}` has non-finite gain",
                    split.id
                )));
            }
            let mut sum = vec![0.0f32; input.len()];
            for (bi, branch) in split.branches.iter().enumerate() {
                let gain = split.gains.get(bi).copied().unwrap_or(1.0) as f32;
                let out = render_nodes(ctx, state, branch, input, visiting)?;
                for (s, o) in sum.iter_mut().zip(out.iter()) {
                    *s += gain * *o;
                }
            }
            Ok(sum)
        }
    }
}

/// Render one track's device chain through the rack: `device_ids` order,
/// containers expanded, splits summed, macros applied, wet/dry + 2x per
/// the Track C conventions. Pure function of its inputs plus `state`
/// (filters/delays remember across calls — pass one `RackState` per
/// rendered track, not one global). Empty input renders empty.
pub fn render_rack(
    project: &Project,
    rack: &Rack,
    state: &mut RackState,
    track_id: &str,
    input: &[f32],
    sample_rate: f64,
) -> Result<Vec<f32>> {
    render_rack_with_bank(project, rack, &SampleBank::new(), state, track_id, input, sample_rate)
}

/// Render one track's device chain with sampler voices fed from `bank`:
/// each sampler device id reads its buffer from the bank (missing =
/// silence). Triggers arrive via [`trigger_sampler_voice`] /
/// [`release_sampler_voice`] — offline renders strike the voice first,
/// then render silence-fed blocks through it.
pub fn render_rack_with_bank(
    project: &Project,
    rack: &Rack,
    bank: &SampleBank,
    state: &mut RackState,
    track_id: &str,
    input: &[f32],
    sample_rate: f64,
) -> Result<Vec<f32>> {
    if !sample_rate.is_finite() || sample_rate <= 0.0 {
        return Err(RackError::BadRack(format!("sample rate must be positive, got {sample_rate}")));
    }
    let track = project
        .tracks
        .iter()
        .find(|t| t.id == track_id)
        .ok_or_else(|| RackError::UnknownTrack(track_id.to_string()))?;
    let ctx = RenderCtx {
        nodes: project.devices.iter().map(|d| (d.id.as_str(), d)).collect(),
        rack,
        bank,
        sample_rate,
    };
    let items: Vec<RackNode> = track.device_ids.iter().map(|id| RackNode::Device(id.clone())).collect();
    render_nodes(&ctx, state, &items, input, &mut BTreeSet::new())
}

/// Convenience: the order actually rendered (containers inline-expanded
/// for display; splits shown once). Debugging aid, not audio.
pub fn rendered_order(project: &Project, rack: &Rack, track_id: &str) -> Result<Vec<String>> {
    let track = project
        .tracks
        .iter()
        .find(|t| t.id == track_id)
        .ok_or_else(|| RackError::UnknownTrack(track_id.to_string()))?;
    let mut order = Vec::new();
    let mut visiting = BTreeSet::new();
    for id in &track.device_ids {
        flatten(id, rack, &mut visiting, &mut order)?;
    }
    Ok(order)
}

fn flatten(
    id: &str,
    rack: &Rack,
    visiting: &mut BTreeSet<String>,
    order: &mut Vec<String>,
) -> Result<()> {
    match rack.containers.get(id) {
        None => {
            order.push(id.to_string());
            Ok(())
        }
        Some(children) => {
            if !visiting.insert(id.to_string()) {
                return Err(RackError::BadRack(format!("container cycle through `{id}`")));
            }
            order.push(format!("{id}:["));
            for child in children.clone() {
                match child {
                    RackNode::Device(d) => flatten(&d, rack, visiting, order)?,
                    RackNode::Rack(r) => {
                        order.push(format!("{r}:"));
                        let kids = rack
                            .containers
                            .get(r.as_str())
                            .ok_or_else(|| RackError::UnknownContainer(r.clone()))?
                            .clone();
                        if !visiting.insert(r.clone()) {
                            return Err(RackError::BadRack(format!("container cycle through `{r}`")));
                        }
                        for k in &kids {
                            if let RackNode::Device(d) = k {
                                flatten(d, rack, visiting, order)?;
                            }
                        }
                        visiting.remove(&r);
                    }
                    RackNode::Split(s) => order.push(format!("split:{}({})", s.id, s.branches.len())),
                }
            }
            order.push("]".to_string());
            visiting.remove(id);
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Track;
    use super::super::class::{instantiate, param_value, set_param_value};

    fn track(id: &str, devices: &[&str]) -> Track {
        Track {
            id: id.to_string(),
            name: id.to_string(),
            volume: 0.8,
            pan: 0.0,
            muted: false,
            solo: false,
            clip_ids: vec![],
            device_ids: devices.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn gain_project() -> (Project, Rack) {
        let mut p = Project::new("p", "Rack");
        p.tracks.push(track("trk", &["g1", "g2"]));
        let mut g1 = instantiate(DeviceClass::Gain, "g1", "G1");
        set_param_value(&mut g1, GAIN_PARAM, 2.0).expect("tune");
        let mut g2 = instantiate(DeviceClass::Gain, "g2", "G2");
        set_param_value(&mut g2, GAIN_PARAM, 3.0).expect("tune");
        p.devices.push(g1);
        p.devices.push(g2);
        (p, Rack::new())
    }

    #[test]
    fn serial_gains_multiply() {
        let (p, rack) = gain_project();
        let mut state = RackState::default();
        let out = render_rack(&p, &rack, &mut state, "trk", &[1.0, 0.5], 44100.0).expect("render");
        assert_eq!(out, vec![6.0, 3.0]);
    }

    #[test]
    fn nested_containers_multiply_through() {
        let (mut p, mut rack) = gain_project();
        // trk: g1 -> C1, where C1 = [g2, C2], C2 = [g3]. Gains clamp to
        // 4.0, so g3 tunes to 4.0 and the chain is 2 * 3 * 4 = 24.
        let mut g3 = instantiate(DeviceClass::Gain, "g3", "G3");
        set_param_value(&mut g3, GAIN_PARAM, 5.0).expect("tune");
        assert_eq!(param_value(&g3, GAIN_PARAM, 0.0), 4.0);
        p.devices.push(g3);
        let c1 = instantiate(DeviceClass::Container, "c1", "Bus");
        let c2 = instantiate(DeviceClass::Container, "c2", "Sub");
        p.devices.push(c1);
        p.devices.push(c2);
        p.tracks[0].device_ids = vec!["g1".to_string(), "c1".to_string()];
        rack.containers.insert(
            "c1".to_string(),
            vec![RackNode::Device("g2".to_string()), RackNode::Rack("c2".to_string())],
        );
        rack.containers.insert("c2".to_string(), vec![RackNode::Device("g3".to_string())]);
        let mut state = RackState::default();
        let out = render_rack(&p, &rack, &mut state, "trk", &[1.0], 44100.0).expect("render");
        assert_eq!(out, vec![24.0]);
        // Rack JSON is the save shape: it must round-trip exactly.
        let json = rack.to_json().expect("to json");
        assert_eq!(Rack::from_json(&json).expect("from json"), rack);
    }

    #[test]
    fn split_sums_weighted_branches() {
        let (mut p, mut rack) = gain_project();
        p.tracks[0].device_ids = vec!["c1".to_string()];
        let c1 = instantiate(DeviceClass::Container, "c1", "Split bus");
        p.devices.push(c1);
        rack.containers.insert(
            "c1".to_string(),
            vec![RackNode::Split(Split {
                id: "s1".to_string(),
                branches: vec![
                    vec![RackNode::Device("g1".to_string())],
                    vec![RackNode::Device("g2".to_string())],
                ],
                gains: vec![0.5, 0.5],
            })],
        );
        let mut state = RackState::default();
        // 0.5*2 + 0.5*3 = 2.5 on a unit input.
        let out = render_rack(&p, &rack, &mut state, "trk", &[1.0], 44100.0).expect("render");
        assert_eq!(out, vec![2.5]);
    }

    #[test]
    fn macro_drives_two_params_at_once() {
        let (p, mut rack) = gain_project();
        rack.macros.push(
            Macro::new(
                "m1",
                "Both gains",
                0.5,
                vec![
                    MacroBinding { device: "g1".to_string(), param: GAIN_PARAM.to_string(), min: 0.0, max: 2.0 },
                    MacroBinding { device: "g2".to_string(), param: GAIN_PARAM.to_string(), min: 0.0, max: 4.0 },
                ],
            )
            .expect("macro"),
        );
        let mut state = RackState::default();
        // g1 = 1.0, g2 = 2.0 -> 2.0 on a unit input. Stored params untouched.
        let out = render_rack(&p, &rack, &mut state, "trk", &[1.0], 44100.0).expect("render");
        assert_eq!(out, vec![2.0]);
        assert_eq!(param_value(&p.devices[0], GAIN_PARAM, 0.0), 2.0);
        assert_eq!(param_value(&p.devices[1], GAIN_PARAM, 0.0), 3.0);
        rack.set_macro("m1", 1.0).expect("full");
        let out = render_rack(&p, &rack, &mut state, "trk", &[1.0], 44100.0).expect("render");
        assert_eq!(out, vec![8.0]);
    }

    #[test]
    fn preset_save_reload_renders_identical() {
        let mut node = instantiate(DeviceClass::Lowpass, "lp", "Softener");
        set_param_value(&mut node, CUTOFF_PARAM, 400.0).expect("tune");
        let preset = DevicePreset::capture(&node).expect("capture");
        let json = preset.to_json().expect("to json");
        assert_eq!(DevicePreset::from_json(&json).expect("from json"), preset);

        // Reload into a fresh project: same DSP, byte-exact params.
        let restored = preset.instantiate("lp2").expect("instantiate");
        assert_eq!(restored.params, node.params);
        let mut p = Project::new("p", "Preset");
        p.tracks.push(track("trk", &["lp2"]));
        p.devices.push(restored);
        let mut state = RackState::default();
        // A one-pole needs ~hundreds of samples to settle at 400 Hz, so
        // render a real block and check the tail, not the attack.
        let dc = vec![1.0f32; 2048];
        let out = render_rack(&p, &Rack::new(), &mut state, "trk", &dc, 44100.0).expect("render");
        assert_eq!(out.len(), 2048);
        assert!(out[2047] > 0.9, "lowpassed dc settles near 1, got {}", out[2047]);
        assert!(DevicePreset::from_json("{bad").is_err());
        assert!(DevicePreset::capture(&crate::model::Node {
            id: "x".into(),
            kind: crate::model::NodeKind::Device,
            name: "x".into(),
            params: vec![],
        })
        .is_err());
    }

    #[test]
    fn oversampled_gain_is_exact_on_constants() {
        let (mut p, rack) = gain_project();
        crate::plugins::chain::set_oversampled(&mut p.devices[0], true);
        let mut state = RackState::default();
        // Constants up/down-sample exactly: rate-independent kernels must
        // be bit-identical through the 2x path.
        let out = render_rack(&p, &rack, &mut state, "trk", &[1.0, 1.0], 44100.0).expect("render");
        assert_eq!(out, vec![6.0, 6.0]);
    }

    #[test]
    fn delay_state_carries_echo_into_the_next_block() {
        let mut p = Project::new("p", "Echo");
        p.tracks.push(track("trk", &["d1"]));
        let mut d = instantiate(DeviceClass::Delay, "d1", "Echo");
        set_param_value(&mut d, DELAY_SAMPLES_PARAM, 2.0).expect("delay");
        set_param_value(&mut d, FEEDBACK_PARAM, 1.0).expect("fb clamps");
        assert_eq!(param_value(&d, FEEDBACK_PARAM, 0.0), 0.95);
        p.devices.push(d);
        let rack = Rack::new();
        let mut state = RackState::default();
        let b1 = render_rack(&p, &rack, &mut state, "trk", &[1.0, 0.0], 44100.0).expect("b1");
        assert_eq!(b1, vec![1.0, 0.0]);
        // Same state, next block: the echo arrives.
        let b2 = render_rack(&p, &rack, &mut state, "trk", &[0.0, 0.0], 44100.0).expect("b2");
        assert_eq!(b2[0], 0.95);
    }

    #[test]
    fn foreign_and_missing_devices_pass_through() {
        let (mut p, rack) = gain_project();
        p.tracks[0].device_ids.push("ghost".to_string());
        p.devices.push(crate::model::Node {
            id: "legacy".to_string(),
            kind: crate::model::NodeKind::Device,
            name: "Legacy".to_string(),
            params: vec![],
        });
        p.tracks[0].device_ids.push("legacy".to_string());
        let mut state = RackState::default();
        let out = render_rack(&p, &rack, &mut state, "trk", &[1.0], 44100.0).expect("render");
        assert_eq!(out, vec![6.0]);
    }

    #[test]
    fn sampler_preset_round_trips() {
        let node = instantiate(DeviceClass::Sampler, "s", "Keys");
        let preset = DevicePreset::capture(&node).expect("capture");
        assert_eq!(preset.class, "sampler");
        let restored = preset.instantiate("s2").expect("instantiate");
        assert_eq!(restored.params, node.params);
        assert_eq!(classify(&restored), DeviceClass::Sampler);
        assert!(DevicePreset::from_json(&preset.to_json().expect("json")).is_ok());
    }

    #[test]
    fn sampler_renders_triggered_voice_and_silence_without_bank() {
        use super::super::sampler::{SampleBank, SampleBuffer};
        let mut p = Project::new("p", "Sampler");
        p.tracks.push(track("trk", &["s1"]));
        p.devices.push(instantiate(DeviceClass::Sampler, "s1", "Voice"));
        let mut bank = SampleBank::new();
        bank.insert("s1", SampleBuffer::new(44100.0, vec![1.0; 8192]).expect("buf"));
        // Untriggered: silence (the gate starts closed).
        let mut state = RackState::default();
        let out = render_rack_with_bank(&p, &Rack::new(), &bank, &mut state, "trk", &[0.0; 8], 44100.0)
            .expect("render");
        assert_eq!(out, vec![0.0; 8]);
        // Triggered: the voice speaks (attack is 5 ms, so frame 0 ramps
        // in but the block average is well above silence).
        trigger_sampler_voice(&mut state, "s1");
        let out = render_rack_with_bank(&p, &Rack::new(), &bank, &mut state, "trk", &[0.0; 2205], 44100.0)
            .expect("render");
        assert!(out[2204] > 0.9, "voice must sustain, got {}", out[2204]);
        // Released with a 50 ms envelope: the tail drains to silence.
        release_sampler_voice(&mut state, "s1");
        let out = render_rack_with_bank(&p, &Rack::new(), &bank, &mut state, "trk", &[0.0; 4410], 44100.0)
            .expect("render");
        assert!(out[4409].abs() < 1e-4, "release tail {}", out[4409]);
        // No bank entry: silence, not an error.
        let mut state = RackState::default();
        trigger_sampler_voice(&mut state, "s1");
        let out = render_rack_with_bank(
            &p, &Rack::new(), &SampleBank::new(), &mut state, "trk", &[0.0; 8], 44100.0,
        )
        .expect("render");
        assert_eq!(out, vec![0.0; 8]);
        // The plain renderer (no bank) treats samplers as silent voices.
        let mut state = RackState::default();
        trigger_sampler_voice(&mut state, "s1");
        let out = render_rack(&p, &Rack::new(), &mut state, "trk", &[0.0; 8], 44100.0)
            .expect("render");
        assert_eq!(out, vec![0.0; 8]);
    }

    #[test]
    fn rack_errors_are_clean() {
        let (p, rack) = gain_project();
        let mut state = RackState::default();
        assert!(matches!(
            render_rack(&p, &rack, &mut state, "nope", &[1.0], 44100.0),
            Err(RackError::UnknownTrack(_))
        ));
        assert!(matches!(
            render_rack(&p, &rack, &mut state, "trk", &[1.0], 0.0),
            Err(RackError::BadRack(_))
        ));
        let mut bad = rack.clone();
        bad.containers.insert(
            "loop".to_string(),
            vec![RackNode::Rack("loop".to_string())],
        );
        let mut p2 = p.clone();
        p2.tracks[0].device_ids = vec!["loop".to_string()];
        assert!(matches!(
            render_rack(&p2, &bad, &mut state, "trk", &[1.0], 44100.0),
            Err(RackError::BadRack(_))
        ));
        assert!(Rack::from_json("{bad").is_err());
        let mut m = Macro::new("m", "M", 0.0, vec![]).expect("macro");
        assert!(m.set_value(2.0).is_err());
    }
}
