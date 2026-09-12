//! Track H: automation + modulation (teaching-first).
//!
//! Two concepts that share one address but must never be confused:
//!
//! - **Automation** is *composed*: a [`AutomationLane`](crate::model::AutomationLane)
//!   (frozen v0 type) stores `(beat, value)` points on the timeline. The
//!   renderer reads it **sample-accurately**: every audio sample maps to an
//!   exact beat, and the lane is evaluated per sample with linear
//!   interpolation (hold outside the point range). An [`AutomationClip`] is
//!   the same shape made reusable: points relative to beat 0 that can be
//!   stamped onto any lane at any offset ([`instantiate_clip`]).
//! - **Modulation** is *performed*: a [`ModMatrix`] of [`ModRoute`]s connects
//!   any source (an LFO, a constant) to any destination through the frozen
//!   [`ParamAddress`](crate::model::ParamAddress) — volume, pan, or any
//!   device param in v1, anything addressable later. Modulation is evaluated
//!   per sample in *seconds* and **added** to the automation value, then
//!   clamped to the destination's range.
//!
//! The combination rule (one line): `out = clamp(auto(beat) + Σ depth·src(secs))`.
//! Automation answers "what value did the composer write at this beat";
//! modulation answers "how much wobble is added at this second".
//!
//! Storage: lanes already live on the frozen `Project.automation` list.
//! Reusable clips and mod routes live in the [`AutomationDoc`] sidecar
//! (serialized beside the project, like `TimelineDoc`), so this track adds
//! no project-schema or IPC surface and the typegen drift gate is
//! unaffected.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::model::{AutomationLane, AutomationPoint, ParamAddress, Project};

#[derive(Debug, Clone, PartialEq)]
pub enum AutomationError {
    UnknownParam(String),
    BadLane(String),
    BadClip(String),
    BadRoute(String),
    BadTime(String),
}

impl std::fmt::Display for AutomationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownParam(s) => write!(f, "unknown param `{s}`"),
            Self::BadLane(m) => write!(f, "bad automation lane: {m}"),
            Self::BadClip(m) => write!(f, "bad automation clip: {m}"),
            Self::BadRoute(m) => write!(f, "bad mod route: {m}"),
            Self::BadTime(m) => write!(f, "bad time base: {m}"),
        }
    }
}

impl std::error::Error for AutomationError {}

pub type Result<T> = std::result::Result<T, AutomationError>;

/// Beats in one sample at `tempo` BPM and `sample_rate` Hz.
pub fn beats_per_sample(tempo: f64, sample_rate: f64) -> Result<f64> {
    if !(tempo > 0.0 && tempo.is_finite()) {
        return Err(AutomationError::BadTime(format!(
            "tempo {tempo} must be finite and > 0"
        )));
    }
    if !(sample_rate > 0.0 && sample_rate.is_finite()) {
        return Err(AutomationError::BadTime(format!(
            "sample_rate {sample_rate} must be finite and > 0"
        )));
    }
    Ok(tempo / 60.0 / sample_rate)
}

/// Absolute beat of sample `i` (sample start) in a block starting at `start_beat`.
pub fn sample_beat(start_beat: f64, tempo: f64, sample_rate: f64, i: usize) -> Result<f64> {
    Ok(start_beat + i as f64 * beats_per_sample(tempo, sample_rate)?)
}

/// Absolute seconds of an absolute beat at one tempo.
pub fn beat_to_seconds(beat: f64, tempo: f64) -> Result<f64> {
    if !(tempo > 0.0 && tempo.is_finite()) {
        return Err(AutomationError::BadTime(format!(
            "tempo {tempo} must be finite and > 0"
        )));
    }
    Ok(beat * 60.0 / tempo)
}

/// Range-check one lane: non-empty id, finite points, beats ascending
/// (strictly — two points on one beat would make interpolation ambiguous).
pub fn validate_lane(lane: &AutomationLane) -> Result<()> {
    if lane.id.is_empty() {
        return Err(AutomationError::BadLane("lane id must be non-empty".into()));
    }
    if lane.target.node.is_empty() || lane.target.param.is_empty() {
        return Err(AutomationError::BadLane(
            "lane target node:param must be non-empty".into(),
        ));
    }
    if lane.points.is_empty() {
        return Err(AutomationError::BadLane("lane needs at least one point".into()));
    }
    let mut prev = f64::NEG_INFINITY;
    for p in &lane.points {
        if !(p.beat.is_finite() && p.value.is_finite()) {
            return Err(AutomationError::BadLane(format!(
                "point ({}, {}) must be finite",
                p.beat, p.value
            )));
        }
        if p.beat <= prev {
            return Err(AutomationError::BadLane(format!(
                "beats must ascend strictly ({} after {})",
                p.beat, prev
            )));
        }
        prev = p.beat;
    }
    Ok(())
}

/// Evaluate one lane at one beat: linear interpolation between the
/// surrounding points, hold-first before the first point, hold-last after
/// the last. Pure function of the points — the sample-accuracy primitive.
pub fn eval_lane(lane: &AutomationLane, beat: f64) -> f64 {
    let pts = &lane.points;
    if pts.is_empty() {
        return 0.0;
    }
    if beat <= pts[0].beat {
        return pts[0].value;
    }
    if beat >= pts[pts.len() - 1].beat {
        return pts[pts.len() - 1].value;
    }
    for w in pts.windows(2) {
        let (a, b) = (&w[0], &w[1]);
        if beat >= a.beat && beat <= b.beat {
            let span = b.beat - a.beat;
            if span <= 0.0 {
                return b.value;
            }
            let t = (beat - a.beat) / span;
            return a.value + t * (b.value - a.value);
        }
    }
    pts[pts.len() - 1].value
}

/// Sample-accurate lane render: one value per sample, each sample mapped to
/// its exact beat ([`sample_beat`]) before [`eval_lane`]. This is what the
/// audio thread would read per frame; the test pins exact floats.
pub fn render_lane_samples(
    lane: &AutomationLane,
    start_beat: f64,
    tempo: f64,
    sample_rate: f64,
    frames: usize,
) -> Result<Vec<f64>> {
    let bps = beats_per_sample(tempo, sample_rate)?;
    let mut out = Vec::with_capacity(frames);
    for i in 0..frames {
        out.push(eval_lane(lane, start_beat + i as f64 * bps));
    }
    Ok(out)
}

/// A reusable automation shape: points relative to beat 0, stamped onto any
/// lane at any offset. Clips are the "copy-paste with a name" of automation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AutomationClip {
    pub id: String,
    pub name: String,
    pub length_beats: f64,
    pub points: Vec<AutomationPoint>,
}

impl AutomationClip {
    pub fn validate(&self) -> Result<()> {
        if self.id.is_empty() {
            return Err(AutomationError::BadClip("clip id must be non-empty".into()));
        }
        if !(self.length_beats.is_finite() && self.length_beats > 0.0) {
            return Err(AutomationError::BadClip(format!(
                "length_beats {} must be finite and > 0",
                self.length_beats
            )));
        }
        if self.points.is_empty() {
            return Err(AutomationError::BadClip("clip needs at least one point".into()));
        }
        let mut prev = f64::NEG_INFINITY;
        for p in &self.points {
            if !(p.beat.is_finite() && p.value.is_finite()) {
                return Err(AutomationError::BadClip(format!(
                    "point ({}, {}) must be finite",
                    p.beat, p.value
                )));
            }
            if p.beat < 0.0 || p.beat > self.length_beats {
                return Err(AutomationError::BadClip(format!(
                    "point beat {} outside clip 0..{}",
                    p.beat, self.length_beats
                )));
            }
            if p.beat <= prev {
                return Err(AutomationError::BadClip(format!(
                    "beats must ascend strictly ({} after {})",
                    p.beat, prev
                )));
            }
            prev = p.beat;
        }
        Ok(())
    }
}

/// Stamp a clip at an absolute offset: relative beats become absolute.
/// Negative offsets are rejected (the timeline starts at beat 0).
pub fn instantiate_clip(clip: &AutomationClip, start_beat: f64) -> Result<Vec<AutomationPoint>> {
    clip.validate()?;
    if !(start_beat.is_finite() && start_beat >= 0.0) {
        return Err(AutomationError::BadClip(format!(
            "start_beat {start_beat} must be finite and >= 0"
        )));
    }
    Ok(clip
        .points
        .iter()
        .map(|p| AutomationPoint {
            beat: start_beat + p.beat,
            value: p.value,
        })
        .collect())
}

/// Merge stamped points into a lane: same-beat points are replaced, new
/// beats are inserted, order is restored. Returns the merged point list
/// (the caller writes it back to the frozen lane + op log).
pub fn merge_points(
    lane: &AutomationLane,
    stamped: &[AutomationPoint],
) -> Vec<AutomationPoint> {
    let mut pts = lane.points.clone();
    for p in stamped {
        match pts.iter().position(|q| q.beat == p.beat) {
            Some(i) => pts[i] = p.clone(),
            None => pts.push(p.clone()),
        }
    }
    pts.sort_by(|a, b| {
        a.beat
            .partial_cmp(&b.beat)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    pts
}

// ---------------------------------------------------------------------------
// Modulation: any-to-any over universal param addressing.
// ---------------------------------------------------------------------------

/// One LFO shape. All shapes are bipolar (-1..+1) over phase 0..1 so `depth`
/// on the route means "peak deviation in param units" for every shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LfoShape {
    Sine,
    Triangle,
    Saw,
    Square,
}

/// A free-running LFO source. `freq_hz` is wall-clock Hz; `phase` is start
/// phase in cycles (0..1, wraps).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Lfo {
    pub shape: LfoShape,
    pub freq_hz: f64,
    pub phase: f64,
}

impl Lfo {
    /// Bipolar (-1..+1) value at absolute time `t_secs`.
    pub fn value_at(&self, t_secs: f64) -> f64 {
        let cycles = (self.freq_hz * t_secs + self.phase).rem_euclid(1.0);
        match self.shape {
            LfoShape::Sine => (cycles * std::f64::consts::TAU).sin(),
            LfoShape::Triangle => 4.0 * (cycles - 0.25 * (2.0 * cycles).floor() - 0.25).abs() - 1.0,
            LfoShape::Saw => 2.0 * cycles - 1.0,
            LfoShape::Square => {
                if cycles < 0.5 {
                    1.0
                } else {
                    -1.0
                }
            }
        }
    }
}

/// One modulation source: an LFO, or a constant (DC offset / test probe).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ModSource {
    Lfo(Lfo),
    Constant(f64),
}

impl ModSource {
    pub fn value_at(&self, t_secs: f64) -> f64 {
        match self {
            Self::Lfo(lfo) => lfo.value_at(t_secs),
            Self::Constant(v) => *v,
        }
    }
}

/// One any-to-any patch: `source` writes to `target` (a universal
/// `node:param` address) scaled by `depth` in param units.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModRoute {
    pub id: String,
    pub source: ModSource,
    pub target: ParamAddress,
    pub depth: f64,
}

impl ModRoute {
    pub fn validate(&self) -> Result<()> {
        if self.id.is_empty() {
            return Err(AutomationError::BadRoute("route id must be non-empty".into()));
        }
        if self.target.node.is_empty() || self.target.param.is_empty() {
            return Err(AutomationError::BadRoute(
                "route target node:param must be non-empty".into(),
            ));
        }
        if !self.depth.is_finite() {
            return Err(AutomationError::BadRoute(format!(
                "depth {} must be finite",
                self.depth
            )));
        }
        if let ModSource::Lfo(lfo) = &self.source {
            if !(lfo.freq_hz.is_finite() && lfo.freq_hz >= 0.0) {
                return Err(AutomationError::BadRoute(format!(
                    "lfo freq {} must be finite and >= 0",
                    lfo.freq_hz
                )));
            }
            if !lfo.phase.is_finite() {
                return Err(AutomationError::BadRoute("lfo phase must be finite".into()));
            }
        }
        Ok(())
    }

    /// Contribution of this route at absolute time `t_secs`.
    pub fn contribution_at(&self, t_secs: f64) -> f64 {
        self.depth * self.source.value_at(t_secs)
    }
}

/// The patch bay: every route, evaluated per sample. v1 sources address
/// track `volume`/`pan` and device params (the same set the engine's
/// `ParamSet` accepts); the addressing generalizes without changes.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ModMatrix {
    pub routes: Vec<ModRoute>,
}

impl ModMatrix {
    pub fn add_route(&mut self, route: ModRoute) -> Result<()> {
        route.validate()?;
        if self.routes.iter().any(|r| r.id == route.id) {
            return Err(AutomationError::BadRoute(format!(
                "duplicate route `{}`",
                route.id
            )));
        }
        self.routes.push(route);
        Ok(())
    }

    /// Summed modulation for one target at one absolute time.
    pub fn mod_sum_at(&self, target: &ParamAddress, t_secs: f64) -> f64 {
        self.routes
            .iter()
            .filter(|r| r.target == *target)
            .map(|r| r.contribution_at(t_secs))
            .sum()
    }
}

/// Current value + range of one address, mirroring the engine's `ParamSet`
/// semantics: track `volume` clamps to [0, 1.5], `pan` to [-1, 1],
/// `muted`/`solo` are 0/1 flags, device params use their own min/max.
pub fn param_base_and_range(project: &Project, target: &ParamAddress) -> Result<(f64, f64, f64)> {
    if let Some(track) = project.tracks.iter().find(|t| t.id == target.node) {
        return match target.param.as_str() {
            "volume" => Ok((track.volume, 0.0, 1.5)),
            "pan" => Ok((track.pan, -1.0, 1.0)),
            "muted" => Ok((if track.muted { 1.0 } else { 0.0 }, 0.0, 1.0)),
            "solo" => Ok((if track.solo { 1.0 } else { 0.0 }, 0.0, 1.0)),
            _ => Err(AutomationError::UnknownParam(format!(
                "{}:{}",
                target.node, target.param
            ))),
        };
    }
    if let Some(dev) = project.devices.iter().find(|d| d.id == target.node) {
        if let Some(p) = dev.params.iter().find(|p| p.id == target.param) {
            return Ok((p.value, p.min, p.max));
        }
    }
    Err(AutomationError::UnknownParam(format!(
        "{}:{}",
        target.node, target.param
    )))
}

/// Sample-accurate combined render for one destination:
/// `out[i] = clamp(auto(beat_i) + mod_sum(secs_i))`, where `beat_i` is the
/// exact beat of sample `i` and `secs_i` its absolute song time. With no
/// lane the automation term is the live base value (the knob still works
/// under modulation); with no routes the output is pure automation.
pub fn resolve_param_samples(
    project: &Project,
    target: &ParamAddress,
    lane: Option<&AutomationLane>,
    matrix: &ModMatrix,
    start_beat: f64,
    tempo: f64,
    sample_rate: f64,
    frames: usize,
) -> Result<Vec<f64>> {
    let (base, min, max) = param_base_and_range(project, target)?;
    let bps = beats_per_sample(tempo, sample_rate)?;
    let secs_per_beat = 60.0 / tempo;
    let mut out = Vec::with_capacity(frames);
    for i in 0..frames {
        let beat = start_beat + i as f64 * bps;
        let auto_val = lane.map(|l| eval_lane(l, beat)).unwrap_or(base);
        let t_secs = beat * secs_per_beat;
        let v = auto_val + matrix.mod_sum_at(target, t_secs);
        out.push(v.clamp(min, max));
    }
    Ok(out)
}

/// Group values by target address for one block (the per-block mixer view).
/// Only lanes present in `lanes` are evaluated; every route target with no
/// lane falls back to its base value under modulation.
pub fn render_block(
    project: &Project,
    lanes: &[AutomationLane],
    matrix: &ModMatrix,
    start_beat: f64,
    tempo: f64,
    sample_rate: f64,
    frames: usize,
) -> Result<BTreeMap<String, Vec<f64>>> {
    let mut keys: Vec<ParamAddress> = lanes.iter().map(|l| l.target.clone()).collect();
    for r in &matrix.routes {
        if !keys.contains(&r.target) {
            keys.push(r.target.clone());
        }
    }
    let mut out = BTreeMap::new();
    for target in keys {
        let lane = lanes.iter().find(|l| l.target == target);
        let key = format!("{}:{}", target.node, target.param);
        out.insert(
            key,
            resolve_param_samples(project, &target, lane, matrix, start_beat, tempo, sample_rate, frames)?,
        );
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Sidecar doc: reusable clips + mod routes beside the frozen project.
// ---------------------------------------------------------------------------

/// Sidecar beside the project (e.g. `automation.json`): reusable clips plus
/// the mod matrix. Lanes themselves stay on the frozen `Project.automation`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AutomationDoc {
    pub clips: Vec<AutomationClip>,
    pub routes: Vec<ModRoute>,
}

impl AutomationDoc {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn add_clip(&mut self, clip: AutomationClip) -> Result<()> {
        clip.validate()?;
        if self.clips.iter().any(|c| c.id == clip.id) {
            return Err(AutomationError::BadClip(format!("duplicate clip `{}`", clip.id)));
        }
        self.clips.push(clip);
        Ok(())
    }

    pub fn add_route(&mut self, route: ModRoute) -> Result<()> {
        route.validate()?;
        if self.routes.iter().any(|r| r.id == route.id) {
            return Err(AutomationError::BadRoute(format!(
                "duplicate route `{}`",
                route.id
            )));
        }
        self.routes.push(route);
        Ok(())
    }

    pub fn clip(&self, id: &str) -> Option<&AutomationClip> {
        self.clips.iter().find(|c| c.id == id)
    }

    pub fn matrix(&self) -> ModMatrix {
        ModMatrix {
            routes: self.routes.clone(),
        }
    }
}

/// Demo sidecar: a 4-beat swell clip plus one sine→volume route.
pub fn sample_automation_doc() -> AutomationDoc {
    AutomationDoc {
        clips: vec![AutomationClip {
            id: "clip_swell".to_string(),
            name: "Swell".to_string(),
            length_beats: 4.0,
            points: vec![
                AutomationPoint { beat: 0.0, value: 0.0 },
                AutomationPoint { beat: 4.0, value: 1.0 },
            ],
        }],
        routes: vec![ModRoute {
            id: "route_trem".to_string(),
            source: ModSource::Lfo(Lfo {
                shape: LfoShape::Sine,
                freq_hz: 5.0,
                phase: 0.0,
            }),
            target: ParamAddress {
                node: "trk_music".to_string(),
                param: "volume".to_string(),
            },
            depth: 0.1,
        }],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Node, NodeKind, Param};

    fn lane() -> AutomationLane {
        AutomationLane {
            id: "lane_vol".to_string(),
            target: ParamAddress {
                node: "trk_music".to_string(),
                param: "volume".to_string(),
            },
            points: vec![
                AutomationPoint { beat: 0.0, value: 0.0 },
                AutomationPoint { beat: 4.0, value: 1.0 },
            ],
        }
    }

    fn project_with_music() -> Project {
        let mut p = Project::sample();
        // Project::sample has no devices; add one for the any-to-any test.
        p.devices.push(Node {
            id: "dev_filter".to_string(),
            kind: NodeKind::Device,
            name: "Filter".to_string(),
            params: vec![Param {
                id: "cutoff".to_string(),
                label: "Cutoff".to_string(),
                value: 1000.0,
                min: 20.0,
                max: 20000.0,
                default: 1000.0,
                unit: "Hz".to_string(),
            }],
        });
        p
    }

    /// The sample-accuracy assertion: tempo 60 (1 beat = 1 s) at 4 Hz sample
    /// rate gives exactly 4 samples per beat, so a 0→1 ramp over 4 beats
    /// must read 0/16, 1/16, … at exact sample indices — no rounding, no
    /// block quantization.
    #[test]
    fn lane_render_is_sample_accurate() {
        let lane = lane();
        // 16 frames cover beats 0..4 (4 samples per beat).
        let out = render_lane_samples(&lane, 0.0, 60.0, 4.0, 17).expect("renders");
        assert_eq!(out.len(), 17);
        // Sample i sits at beat i/4; ramp is beat/4, so value = i/16.
        for (i, &v) in out.iter().enumerate() {
            let want = (i as f64 / 16.0).min(1.0);
            assert!(
                (v - want).abs() < 1e-12,
                "sample {i}: got {v}, want {want}"
            );
        }
        // Pin the headline frames exactly.
        assert_eq!(out[0], 0.0);
        assert_eq!(out[4], 0.25);
        assert_eq!(out[8], 0.5);
        assert_eq!(out[12], 0.75);
        assert_eq!(out[16], 1.0);
    }

    #[test]
    fn lane_render_mid_block_offset_is_exact() {
        // Starting mid-ramp must agree with point evaluation, not snap.
        let lane = lane();
        let out = render_lane_samples(&lane, 1.0, 60.0, 4.0, 4).expect("renders");
        assert_eq!(out, vec![0.25, 0.3125, 0.375, 0.4375]);
    }

    #[test]
    fn eval_holds_outside_point_range() {
        let lane = lane();
        assert_eq!(eval_lane(&lane, -100.0), 0.0);
        assert_eq!(eval_lane(&lane, 100.0), 1.0);
    }

    #[test]
    fn clip_instantiate_and_merge() {
        let clip = AutomationClip {
            id: "c".to_string(),
            name: "C".to_string(),
            length_beats: 2.0,
            points: vec![
                AutomationPoint { beat: 0.0, value: 0.5 },
                AutomationPoint { beat: 2.0, value: 0.9 },
            ],
        };
        let stamped = instantiate_clip(&clip, 8.0).expect("stamps");
        assert_eq!(
            stamped,
            vec![
                AutomationPoint { beat: 8.0, value: 0.5 },
                AutomationPoint { beat: 10.0, value: 0.9 },
            ]
        );
        // Merging onto a lane that already has beat 8 replaces it.
        let mut target = lane();
        target.points.push(AutomationPoint { beat: 8.0, value: 0.0 });
        let merged = merge_points(&target, &stamped);
        let beats: Vec<f64> = merged.iter().map(|p| p.beat).collect();
        assert_eq!(beats, vec![0.0, 4.0, 8.0, 10.0]);
        assert_eq!(merged.iter().find(|p| p.beat == 8.0).unwrap().value, 0.5);
    }

    /// Modulation adds to automation and clamps: constant +10.0 on a lane
    /// holding 0.8 volume must pin at the 1.5 ceiling, not wrap or NaN.
    #[test]
    fn modulation_adds_and_clamps_to_range() {
        let p = project_with_music();
        let target = ParamAddress {
            node: "trk_music".to_string(),
            param: "volume".to_string(),
        };
        let lane = AutomationLane {
            id: "l".to_string(),
            target: target.clone(),
            points: vec![AutomationPoint { beat: 0.0, value: 0.8 }],
        };
        let matrix = ModMatrix {
            routes: vec![ModRoute {
                id: "r".to_string(),
                source: ModSource::Constant(10.0),
                target: target.clone(),
                depth: 1.0,
            }],
        };
        let out =
            resolve_param_samples(&p, &target, Some(&lane), &matrix, 0.0, 120.0, 44100.0, 8)
                .expect("resolves");
        assert!(out.iter().all(|&v| v == 1.5), "must clamp: {out:?}");
    }

    /// Any-to-any: the same machinery drives a device param (cutoff Hz),
    /// addressed identically through `node:param`.
    #[test]
    fn modulation_addresses_device_params() {
        let p = project_with_music();
        let target = ParamAddress {
            node: "dev_filter".to_string(),
            param: "cutoff".to_string(),
        };
        let matrix = ModMatrix {
            routes: vec![ModRoute {
                id: "r".to_string(),
                source: ModSource::Constant(1.0),
                target: target.clone(),
                depth: 100.0,
            }],
        };
        let out =
            resolve_param_samples(&p, &target, None, &matrix, 0.0, 120.0, 44100.0, 4)
                .expect("resolves");
        assert_eq!(out, vec![1100.0; 4]);
    }

    #[test]
    fn sine_lfo_is_bipolar_and_periodic() {
        let lfo = Lfo { shape: LfoShape::Sine, freq_hz: 1.0, phase: 0.0 };
        assert_eq!(lfo.value_at(0.0), 0.0);
        assert!((lfo.value_at(0.25) - 1.0).abs() < 1e-12);
        assert!(lfo.value_at(0.5).abs() < 1e-12);
        assert!((lfo.value_at(0.75) + 1.0).abs() < 1e-12);
        assert!(lfo.value_at(1.0).abs() < 1e-12);
    }

    #[test]
    fn unknown_param_is_an_error_not_silence() {
        let p = project_with_music();
        let target = ParamAddress {
            node: "trk_music".to_string(),
            param: "reverb_size".to_string(),
        };
        let err = resolve_param_samples(&p, &target, None, &ModMatrix::default(), 0.0, 120.0, 44100.0, 4)
            .expect_err("must fail");
        assert!(matches!(err, AutomationError::UnknownParam(_)), "{err}");
    }

    #[test]
    fn bad_lanes_and_routes_rejected() {
        let mut bad = lane();
        bad.points.push(AutomationPoint { beat: 4.0, value: 0.0 });
        assert!(validate_lane(&bad).is_err());
        let doc = &mut AutomationDoc::empty();
        assert!(doc.add_route(ModRoute {
            id: "".to_string(),
            source: ModSource::Constant(1.0),
            target: ParamAddress { node: "n".to_string(), param: "p".to_string() },
            depth: 1.0,
        }).is_err());
        assert!(sample_automation_doc().clips[0].validate().is_ok());
    }

    #[test]
    fn doc_round_trips_through_json() {
        let doc = sample_automation_doc();
        let json = serde_json::to_string(&doc).expect("serialize");
        let back: AutomationDoc = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(doc, back);
    }
}
