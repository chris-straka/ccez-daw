//! ACE-Step / multi-stem audio take cutting, loop-seam closure, and
//! `audioforge` interactive-music package export (`ccez-daw-3`).
//!
//! Workflow:
//! 1. Import decoded [`wavread::Audio`] or raw WAV bytes for each `(section, layer)` take.
//! 2. Validate sample/beat cut bounds, resample non-48 kHz sources to `48_000` Hz,
//!    and upmix mono takes to stereo.
//! 3. Close loop wrap boundaries using post-loop or pre-loop crossfade margins
//!    when available (falling back to symmetric raised-cosine edge fades when no
//!    margin exists), and apply optional edge fades on one-shot intros, outros,
//!    and exact-length jingles.
//! 4. Export a deterministic `audioforge-music` v1 package (`manifest.json`,
//!    `score.json`, `mix.json`, `events/*.json`, `stems/*.wav`,
//!    `scenes/preview.json`) and verify every exported WAV loop seam.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::audioforge::{
    build_preview_scene_json, write_audioforge_package, PackageManifestMeta, PackageSection,
    PackageStem,
};
use super::render::{db_to_gain, PlanStep, RenderedStem};
use super::{valid_id, wavread, LayerDef, Role, MAX_INTENSITY, SAMPLE_RATE};
use crate::loopseam::{conform_loop_seam, resample_linear, DEFAULT_CLICK_THRESHOLD};

/// Role of a section built from audio takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TakeRole {
    Intro,
    Loop,
    Outro,
    Jingle,
}

impl TakeRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Intro => "intro",
            Self::Loop => "loop",
            Self::Outro => "outro",
            Self::Jingle => "jingle",
        }
    }

    pub fn is_once(self) -> bool {
        !matches!(self, Self::Loop)
    }

    fn to_stem_role(self) -> Role {
        match self {
            Self::Intro => Role::Intro,
            Self::Loop => Role::Loop,
            Self::Outro | Self::Jingle => Role::Outro,
        }
    }
}

/// Cut & loop bounds inside a source take (end exclusive).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "unit", rename_all = "lowercase")]
pub enum CutRange {
    /// Source sample frames (`trim_start <= loop_start < loop_end <= source_frames`).
    Frames {
        trim_start: usize,
        loop_start: usize,
        loop_end: usize,
    },
    /// Musical beats at the pack's `tempo` (`trim_start_beats <= loop_start_beats < loop_end_beats`).
    Beats {
        trim_start_beats: f64,
        loop_start_beats: f64,
        loop_end_beats: f64,
    },
}

impl CutRange {
    pub fn frames(loop_start: usize, loop_end: usize) -> Self {
        Self::Frames {
            trim_start: loop_start,
            loop_start,
            loop_end,
        }
    }

    pub fn frames_with_pickup(trim_start: usize, loop_start: usize, loop_end: usize) -> Self {
        Self::Frames {
            trim_start,
            loop_start,
            loop_end,
        }
    }

    pub fn beats(loop_start_beats: f64, loop_end_beats: f64) -> Self {
        Self::Beats {
            trim_start_beats: loop_start_beats,
            loop_start_beats,
            loop_end_beats,
        }
    }

    pub fn beats_with_pickup(
        trim_start_beats: f64,
        loop_start_beats: f64,
        loop_end_beats: f64,
    ) -> Self {
        Self::Beats {
            trim_start_beats,
            loop_start_beats,
            loop_end_beats,
        }
    }
}

/// Crossfade and edge-fade lengths in target 48 kHz frames.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct FadeMargin {
    #[serde(default)]
    pub crossfade_frames: usize,
    #[serde(default)]
    pub fade_in_frames: usize,
    #[serde(default)]
    pub fade_out_frames: usize,
}

impl FadeMargin {
    pub fn crossfade(frames: usize) -> Self {
        Self {
            crossfade_frames: frames,
            fade_in_frames: 0,
            fade_out_frames: 0,
        }
    }

    pub fn crossfade_ms(ms: f64) -> Self {
        Self::crossfade(ms_to_frames(ms))
    }

    pub fn jingle_edges_ms(in_ms: f64, out_ms: f64) -> Self {
        Self {
            crossfade_frames: 0,
            fade_in_frames: ms_to_frames(in_ms),
            fade_out_frames: ms_to_frames(out_ms),
        }
    }
}

fn ms_to_frames(ms: f64) -> usize {
    if ms.is_finite() && ms > 0.0 {
        (ms * 0.001 * SAMPLE_RATE as f64).round() as usize
    } else {
        0
    }
}

/// Source take audio provided either pre-decoded or as WAV file bytes.
#[derive(Debug, Clone, PartialEq)]
pub enum TakeAudio {
    Decoded(wavread::Audio),
    WavBytes(Vec<u8>),
}

/// One `(section, layer)` stem cut from an imported take.
#[derive(Debug, Clone, PartialEq)]
pub struct TakeStemSpec {
    pub section: String,
    pub layer: String,
    pub audio: TakeAudio,
    pub cut: CutRange,
    pub fades: FadeMargin,
    pub gain_db: f32,
    pub source_label: Option<String>,
}

/// Section definition for an audio-take package.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TakeSectionSpec {
    pub id: String,
    pub role: TakeRole,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub length_beats: Option<f32>,
}

/// Full multi-stem audio-take export specification.
#[derive(Debug, Clone, PartialEq)]
pub struct TakePackSpec {
    pub name: String,
    pub tempo: f32,
    pub beats_per_bar: u32,
    pub key: String,
    pub xfade_beats: f32,
    pub layers: Vec<LayerDef>,
    pub sections: Vec<TakeSectionSpec>,
    pub stems: Vec<TakeStemSpec>,
    pub normalize_peak_dbfs: Option<f32>,
}

/// JSON-serializable take-pack file spec for CLI `ccez-compose export-takes`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TakePackFileSpec {
    pub name: String,
    pub tempo: f32,
    #[serde(default = "four")]
    pub beats_per_bar: u32,
    #[serde(default)]
    pub key: String,
    #[serde(default = "one_f32")]
    pub xfade_beats: f32,
    pub layers: Vec<LayerDef>,
    pub sections: Vec<TakeSectionSpec>,
    pub stems: Vec<TakeFileStemSpec>,
    #[serde(default = "default_norm_db")]
    pub normalize_peak_dbfs: Option<f32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TakeFileStemSpec {
    pub section: String,
    pub layer: String,
    pub wav_path: String,
    pub cut: CutRange,
    #[serde(default)]
    pub fades: FadeMargin,
    #[serde(default)]
    pub gain_db: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_label: Option<String>,
}

fn four() -> u32 {
    4
}
fn one_f32() -> f32 {
    1.0
}
fn default_norm_db() -> Option<f32> {
    Some(-1.0)
}

impl TakePackFileSpec {
    /// Load WAV bytes relative to `base_dir` into an in-memory [`TakePackSpec`].
    pub fn load_from(&self, base_dir: &Path) -> Result<TakePackSpec, String> {
        let mut stems = Vec::with_capacity(self.stems.len());
        for s in &self.stems {
            let p = Path::new(&s.wav_path);
            let full = if p.is_absolute() {
                p.to_path_buf()
            } else {
                base_dir.join(p)
            };
            let bytes = std::fs::read(&full).map_err(|e| format!("{}: {e}", full.display()))?;
            stems.push(TakeStemSpec {
                section: s.section.clone(),
                layer: s.layer.clone(),
                audio: TakeAudio::WavBytes(bytes),
                cut: s.cut,
                fades: s.fades,
                gain_db: s.gain_db,
                source_label: s.source_label.clone(),
            });
        }
        Ok(TakePackSpec {
            name: self.name.clone(),
            tempo: self.tempo,
            beats_per_bar: self.beats_per_bar,
            key: self.key.clone(),
            xfade_beats: self.xfade_beats,
            layers: self.layers.clone(),
            sections: self.sections.clone(),
            stems,
            normalize_peak_dbfs: self.normalize_peak_dbfs,
        })
    }
}

/// Loop-seam check metrics across `[loop_start_frame, loop_end_frame)`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TakeSeamCheck {
    pub section: String,
    pub layer: String,
    pub loop_start_frame: usize,
    pub loop_end_frame: usize,
    pub max_boundary_step: f32,
    pub boundary_rms_step: f32,
    pub window_rms_delta: f32,
    pub p999_step: f32,
    pub threshold: f32,
    pub seamless: bool,
}

/// Result of exporting a [`TakePackSpec`].
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TakeExportReport {
    pub dir: String,
    pub files: Vec<String>,
    pub stems: usize,
    pub trim_db: f32,
    pub loop_seams: Vec<TakeSeamCheck>,
    pub warnings: Vec<String>,
}

struct PreparedStem {
    section: String,
    layer: String,
    role: TakeRole,
    min_intensity: u8,
    layer_gain_db: f32,
    rendered: RenderedStem,
    loop_start_frame: usize,
    loop_end_frame: usize,
    length_beats: f32,
    loop_start_beats: Option<f32>,
    loop_end_beats: Option<f32>,
    source_label: Option<String>,
}

/// Default audition plan for a [`TakePackSpec`]: intro (if any), each loop at
/// intensity 1 then max intensity, then outro/jingle (if any).
pub fn default_take_plan(spec: &TakePackSpec) -> Vec<PlanStep> {
    let mut plan = Vec::new();
    let max_i = spec
        .layers
        .iter()
        .map(|l| l.min_intensity)
        .max()
        .unwrap_or(1)
        .clamp(1, MAX_INTENSITY);
    let step = |s: &TakeSectionSpec, intensity: u8| PlanStep {
        section: s.id.clone(),
        bars: None,
        intensity,
    };
    if let Some(intro) = spec.sections.iter().find(|s| s.role == TakeRole::Intro) {
        plan.push(step(intro, 1));
    }
    for s in spec.sections.iter().filter(|s| s.role == TakeRole::Loop) {
        plan.push(step(s, 1));
        if max_i > 1 {
            plan.push(step(s, max_i));
        }
    }
    for end in spec
        .sections
        .iter()
        .filter(|s| matches!(s.role, TakeRole::Outro | TakeRole::Jingle))
    {
        plan.push(step(end, 1));
    }
    plan
}

fn validate_spec(spec: &TakePackSpec) -> Result<(), String> {
    if !valid_id(&spec.name) {
        return Err(format!(
            "pack name {:?}: want lowercase [a-z0-9_]+",
            spec.name
        ));
    }
    if !spec.tempo.is_finite() || !(40.0..=300.0).contains(&spec.tempo) {
        return Err(format!("tempo {}: want 40..=300 bpm", spec.tempo));
    }
    if !(1..=16).contains(&spec.beats_per_bar) {
        return Err(format!("beats_per_bar {}: want 1..=16", spec.beats_per_bar));
    }
    if !spec.xfade_beats.is_finite() || spec.xfade_beats <= 0.0 || spec.xfade_beats > 64.0 {
        return Err(format!("xfade_beats {}: want (0, 64]", spec.xfade_beats));
    }
    if let Some(db) = spec.normalize_peak_dbfs {
        if !db.is_finite() || !(-60.0..=0.0).contains(&db) {
            return Err(format!("normalize_peak_dbfs {db}: want [-60, 0]"));
        }
    }
    if spec.layers.is_empty() {
        return Err("pack has no layers".to_string());
    }
    if spec.sections.is_empty() {
        return Err("pack has no sections".to_string());
    }
    if spec.stems.is_empty() {
        return Err("pack has no stems".to_string());
    }
    let mut layer_ids = HashSet::new();
    for l in &spec.layers {
        if !valid_id(&l.id) {
            return Err(format!("layer id {:?}: want lowercase [a-z0-9_]+", l.id));
        }
        if !layer_ids.insert(l.id.as_str()) {
            return Err(format!("layer id {:?} is defined twice", l.id));
        }
        if !(1..=MAX_INTENSITY).contains(&l.min_intensity) {
            return Err(format!(
                "layer {:?}: min_intensity {} want 1..={MAX_INTENSITY}",
                l.id, l.min_intensity
            ));
        }
        if !l.gain_db.is_finite() || !(-60.0..=12.0).contains(&l.gain_db) {
            return Err(format!(
                "layer {:?}: gain_db {} want -60..=12",
                l.id, l.gain_db
            ));
        }
    }
    let mut section_ids = HashSet::new();
    for s in &spec.sections {
        if !valid_id(&s.id) {
            return Err(format!("section id {:?}: want lowercase [a-z0-9_]+", s.id));
        }
        if !section_ids.insert(s.id.as_str()) {
            return Err(format!("section id {:?} is defined twice", s.id));
        }
        if let Some(len) = s.length_beats {
            if !len.is_finite() || len <= 0.0 || len > 4096.0 {
                return Err(format!(
                    "section {:?}: length_beats {len} want (0, 4096]",
                    s.id
                ));
            }
        }
    }
    for s in &spec.sections {
        match (s.role, &s.next) {
            (TakeRole::Intro, None) => {
                return Err(format!("section {:?}: intro requires next section", s.id));
            }
            (TakeRole::Intro, Some(next)) => {
                if !section_ids.contains(next.as_str()) {
                    return Err(format!(
                        "section {:?}: next {:?} is not a defined section",
                        s.id, next
                    ));
                }
            }
            (_, Some(_)) => {
                return Err(format!(
                    "section {:?}: only intro sections may set next",
                    s.id
                ));
            }
            _ => {}
        }
    }
    let mut pairs = HashSet::new();
    for st in &spec.stems {
        if !section_ids.contains(st.section.as_str()) {
            return Err(format!("stem references unknown section {:?}", st.section));
        }
        if !layer_ids.contains(st.layer.as_str()) {
            return Err(format!("stem references unknown layer {:?}", st.layer));
        }
        if !pairs.insert((st.section.as_str(), st.layer.as_str())) {
            return Err(format!(
                "duplicate stem for section {:?}, layer {:?}",
                st.section, st.layer
            ));
        }
        if !st.gain_db.is_finite() || !(-60.0..=12.0).contains(&st.gain_db) {
            return Err(format!(
                "stem ({}, {}): gain_db {} want -60..=12",
                st.section, st.layer, st.gain_db
            ));
        }
    }
    for s in &spec.sections {
        if s.role == TakeRole::Intro {
            if let Some(next) = &s.next {
                if !spec.stems.iter().any(|st| st.section == *next) {
                    return Err(format!("section {:?}: next {:?} has no stems", s.id, next));
                }
            }
        }
    }
    Ok(())
}

fn validate_audio(audio: &wavread::Audio, label: &str) -> Result<usize, String> {
    if audio.sample_rate == 0 {
        return Err(format!("{label}: sample rate must be > 0"));
    }
    if audio.channels.is_empty() {
        return Err(format!("{label}: audio has zero channels"));
    }
    let n = audio.channels[0].len();
    if n == 0 {
        return Err(format!("{label}: audio is empty (zero frames)"));
    }
    if audio.channels.iter().any(|ch| ch.len() != n) {
        return Err(format!("{label}: audio channel lengths differ"));
    }
    if audio
        .channels
        .iter()
        .any(|ch| ch.iter().any(|s| !s.is_finite()))
    {
        return Err(format!("{label}: audio contains non-finite samples"));
    }
    Ok(n)
}

fn resolve_source_bounds(
    cut: CutRange,
    once: bool,
    tempo: f32,
    sample_rate: u32,
    src_frames: usize,
    label: &str,
) -> Result<(usize, usize, usize), String> {
    let (trim_start, loop_start, loop_end) = match cut {
        CutRange::Frames {
            trim_start,
            loop_start,
            loop_end,
        } => {
            if once && trim_start != loop_start {
                return Err(format!(
                    "{label}: trim_start ({trim_start}) must equal loop_start ({loop_start}) for once cues"
                ));
            }
            (trim_start, loop_start, loop_end)
        }
        CutRange::Beats {
            trim_start_beats,
            loop_start_beats,
            loop_end_beats,
        } => {
            if !trim_start_beats.is_finite()
                || !loop_start_beats.is_finite()
                || !loop_end_beats.is_finite()
            {
                return Err(format!("{label}: beat bounds must be finite"));
            }
            if trim_start_beats < 0.0 {
                return Err(format!(
                    "{label}: trim_start_beats ({trim_start_beats}) must be >= 0"
                ));
            }
            if once && (trim_start_beats - loop_start_beats).abs() > 1e-9 {
                return Err(format!(
                    "{label}: trim_start_beats ({trim_start_beats}) must equal loop_start_beats ({loop_start_beats}) for once cues"
                ));
            }
            if trim_start_beats > loop_start_beats {
                return Err(format!(
                    "{label}: trim_start_beats ({trim_start_beats}) must be <= loop_start_beats ({loop_start_beats})"
                ));
            }
            if loop_end_beats <= loop_start_beats {
                return Err(format!(
                    "{label}: invalid loop bounds: loop_end_beats ({loop_end_beats}) must be > loop_start_beats ({loop_start_beats})"
                ));
            }
            let fpb = 60.0 / tempo as f64 * sample_rate as f64;
            (
                (trim_start_beats * fpb).round() as usize,
                (loop_start_beats * fpb).round() as usize,
                (loop_end_beats * fpb).round() as usize,
            )
        }
    };
    if trim_start > loop_start {
        return Err(format!(
            "{label}: trim_start ({trim_start}) must be <= loop_start ({loop_start})"
        ));
    }
    if loop_end <= loop_start {
        return Err(format!(
            "{label}: invalid loop bounds: loop_end ({loop_end}) must be > loop_start ({loop_start})"
        ));
    }
    if loop_end > src_frames {
        return Err(format!(
            "{label}: loop bounds out of range: loop_end ({loop_end}) exceeds source frames ({src_frames})"
        ));
    }
    Ok((trim_start, loop_start, loop_end))
}

fn to_target_stereo(audio: &wavread::Audio) -> Result<([Vec<f32>; 2], f64), String> {
    let ch0 = &audio.channels[0];
    let ch1 = audio.channels.get(1).unwrap_or(ch0);
    if audio.sample_rate == SAMPLE_RATE {
        return Ok(([ch0.clone(), ch1.clone()], 1.0));
    }
    let ratio = audio.sample_rate as f64 / SAMPLE_RATE as f64;
    let l = resample_linear(ch0, ratio)?;
    let r = resample_linear(ch1, ratio)?;
    Ok(([l, r], SAMPLE_RATE as f64 / audio.sample_rate as f64))
}

fn apply_raised_cosine_edge_fades(buf: &mut [f32], fade_in: usize, fade_out: usize) {
    let n = buf.len();
    for (k, s) in buf.iter_mut().take(fade_in).enumerate() {
        let t = k as f32 / fade_in as f32;
        *s *= 0.5 * (1.0 - (std::f32::consts::PI * t).cos());
    }
    for k in 0..fade_out {
        let t = k as f32 / fade_out as f32;
        let g = 0.5 * (1.0 - (std::f32::consts::PI * t).cos());
        buf[n - 1 - k] *= g;
    }
}

fn crossfade_correlation(a: &[f32], b: &[f32]) -> f32 {
    let mut aa = 0.0f64;
    let mut bb = 0.0f64;
    let mut ab = 0.0f64;
    for (&x, &y) in a.iter().zip(b.iter()) {
        let xf = x as f64;
        let yf = y as f64;
        aa += xf * xf;
        bb += yf * yf;
        ab += xf * yf;
    }
    let denom = (aa * bb).sqrt();
    if denom <= 1e-12 {
        0.0
    } else {
        (ab / denom).clamp(-0.5, 1.0) as f32
    }
}

fn apply_loop_crossfade(
    out: &mut [Vec<f32>; 2],
    src: &[Vec<f32>; 2],
    rel_loop_start: usize,
    loop_start: usize,
    loop_end: usize,
    xf: usize,
) {
    let stem_len = out[0].len();
    let total_src = src[0].len();
    if loop_end + xf <= total_src {
        // Post-loop margin: correlation-compensated equal-power blend of
        // [loop_end .. loop_end + xf) into [rel_loop_start .. rel_loop_start + xf)
        // so out[stem_len - 1] = src[loop_end - 1] wraps directly into
        // out[rel_loop_start] = src[loop_end] without a coherent +3 dB bump or dip.
        for ch in 0..2 {
            let rho = crossfade_correlation(
                &out[ch][rel_loop_start..rel_loop_start + xf],
                &src[ch][loop_end..loop_end + xf],
            );
            for k in 0..xf {
                let theta = std::f32::consts::FRAC_PI_2 * (k as f32 / xf as f32);
                let (w_in, w_out) = theta.sin_cos();
                let gain = 1.0 / (1.0 + 2.0 * rho * w_in * w_out).max(0.25).sqrt();
                let idx = rel_loop_start + k;
                out[ch][idx] = (out[ch][idx] * w_in + src[ch][loop_end + k] * w_out) * gain;
            }
        }
    } else if loop_start >= xf {
        // Pre-loop margin: correlation-compensated equal-power blend of
        // [loop_start - xf .. loop_start) into the loop tail so
        // out[stem_len - 1] = src[loop_start - 1] wraps into out[rel_loop_start] = src[loop_start].
        for ch in 0..2 {
            let tail = stem_len - xf;
            let rho = crossfade_correlation(
                &out[ch][tail..stem_len],
                &src[ch][loop_start - xf..loop_start],
            );
            for k in 0..xf {
                let theta = std::f32::consts::FRAC_PI_2 * ((k + 1) as f32 / xf as f32);
                let (w_in, w_out) = theta.sin_cos();
                let gain = 1.0 / (1.0 + 2.0 * rho * w_in * w_out).max(0.25).sqrt();
                let dst = tail + k;
                out[ch][dst] = (out[ch][dst] * w_out + src[ch][loop_start - xf + k] * w_in) * gain;
            }
        }
    } else {
        // No margin outside [trim_start .. loop_end): symmetric raised-cosine edge fade.
        for ch in out.iter_mut() {
            conform_loop_seam(&mut ch[rel_loop_start..stem_len], xf);
        }
    }
}

fn frames_to_beats(frames: usize, tempo: f32) -> f32 {
    (frames as f64 * tempo as f64 / (60.0 * SAMPLE_RATE as f64)) as f32
}

fn prepare_stem(
    spec: &TakePackSpec,
    sec: &TakeSectionSpec,
    layer: &LayerDef,
    st: &TakeStemSpec,
) -> Result<PreparedStem, String> {
    let label = format!("stem ({}, {})", st.section, st.layer);
    let decoded = match &st.audio {
        TakeAudio::Decoded(a) => a.clone(),
        TakeAudio::WavBytes(bytes) => {
            wavread::decode(bytes).map_err(|e| format!("{label}: {e}"))?
        }
    };
    let src_frames = validate_audio(&decoded, &label)?;
    let (src_trim, src_lstart, src_lend) = resolve_source_bounds(
        st.cut,
        sec.role.is_once(),
        spec.tempo,
        decoded.sample_rate,
        src_frames,
        &label,
    )?;
    let (src_stereo, scale) = to_target_stereo(&decoded)?;
    let resampled_len = src_stereo[0].len();
    let t_start = ((src_trim as f64 * scale).round() as usize).min(resampled_len);
    let l_start = ((src_lstart as f64 * scale).round() as usize).clamp(t_start, resampled_len);
    let l_end = ((src_lend as f64 * scale).round() as usize).min(resampled_len);
    if l_end <= l_start {
        return Err(format!(
            "{label}: invalid loop bounds after resample ({l_end} <= {l_start})"
        ));
    }
    let stem_len = l_end - t_start;
    let rel_loop_start = l_start - t_start;
    let loop_len = l_end - l_start;
    if !sec.role.is_once() && loop_len < 2 {
        return Err(format!("{label}: loop region must be at least 2 frames"));
    }
    if !sec.role.is_once() && st.fades.crossfade_frames * 2 > loop_len {
        return Err(format!(
            "{label}: crossfade ({}) exceeds half the loop length ({})",
            st.fades.crossfade_frames,
            loop_len / 2
        ));
    }
    if st.fades.fade_in_frames + st.fades.fade_out_frames > stem_len {
        return Err(format!(
            "{label}: fade_in ({}) + fade_out ({}) exceeds stem length ({stem_len})",
            st.fades.fade_in_frames, st.fades.fade_out_frames
        ));
    }

    let mut out = [
        src_stereo[0][t_start..l_end].to_vec(),
        src_stereo[1][t_start..l_end].to_vec(),
    ];
    if !sec.role.is_once() && st.fades.crossfade_frames > 0 {
        apply_loop_crossfade(
            &mut out,
            &src_stereo,
            rel_loop_start,
            l_start,
            l_end,
            st.fades.crossfade_frames,
        );
    }
    if st.fades.fade_in_frames > 0 || st.fades.fade_out_frames > 0 {
        for ch in &mut out {
            apply_raised_cosine_edge_fades(ch, st.fades.fade_in_frames, st.fades.fade_out_frames);
        }
    }
    let stem_gain = db_to_gain(st.gain_db);
    for ch in &mut out {
        for s in ch.iter_mut() {
            *s *= stem_gain;
        }
    }

    let stem_beats = frames_to_beats(stem_len, spec.tempo);
    let (length_beats, loop_start_beats, loop_end_beats) = if sec.role.is_once() {
        let len_b = sec.length_beats.unwrap_or(stem_beats);
        if !len_b.is_finite() || len_b <= 0.0 || len_b > 4096.0 {
            return Err(format!(
                "{label}: section length_beats {len_b} want (0, 4096]"
            ));
        }
        (len_b, None, None)
    } else {
        let ls_b = frames_to_beats(rel_loop_start, spec.tempo);
        let le_b = frames_to_beats(stem_len, spec.tempo);
        if !le_b.is_finite() || le_b <= ls_b || le_b > 4096.0 {
            return Err(format!(
                "{label}: loop beat range [{ls_b}, {le_b}) outside (0, 4096]"
            ));
        }
        (le_b - ls_b, Some(ls_b), Some(le_b))
    };

    let [left, right] = out;
    Ok(PreparedStem {
        section: st.section.clone(),
        layer: st.layer.clone(),
        role: sec.role,
        min_intensity: layer.min_intensity,
        layer_gain_db: layer.gain_db,
        rendered: RenderedStem {
            section: st.section.clone(),
            layer: st.layer.clone(),
            role: sec.role.to_stem_role(),
            min_intensity: layer.min_intensity,
            left,
            right,
            length_frames: stem_len,
        },
        loop_start_frame: rel_loop_start,
        loop_end_frame: stem_len,
        length_beats,
        loop_start_beats,
        loop_end_beats,
        source_label: st.source_label.clone(),
    })
}

fn check_section_alignment(prepared: &[PreparedStem]) -> Result<(), String> {
    let mut loop_anchors: BTreeMap<&str, (usize, usize)> = BTreeMap::new();
    for p in prepared {
        if p.role.is_once() {
            continue;
        }
        let entry = loop_anchors
            .entry(&p.section)
            .or_insert((p.loop_start_frame, p.loop_end_frame));
        let ds = entry.0.abs_diff(p.loop_start_frame);
        let de = entry.1.abs_diff(p.loop_end_frame);
        if ds > 2 || de > 2 {
            return Err(format!(
                "section {:?}: layer {:?} loop frames ({}..{}) mismatch earlier layer ({}..{})",
                p.section, p.layer, p.loop_start_frame, p.loop_end_frame, entry.0, entry.1
            ));
        }
    }
    Ok(())
}

fn apply_shared_trim(prepared: &mut [PreparedStem], target_peak_dbfs: Option<f32>) -> f32 {
    let Some(target_db) = target_peak_dbfs else {
        return 0.0;
    };
    let mut worst = 0.0f32;
    let mut by_section: BTreeMap<&str, Vec<&PreparedStem>> = BTreeMap::new();
    for p in prepared.iter() {
        by_section.entry(&p.section).or_default().push(p);
    }
    for stems in by_section.values() {
        let mut sum = [Vec::new(), Vec::new()];
        for st in stems {
            let g = db_to_gain(st.layer_gain_db);
            for (ch, src) in [&st.rendered.left, &st.rendered.right].iter().enumerate() {
                if sum[ch].len() < src.len() {
                    sum[ch].resize(src.len(), 0.0);
                }
                for (a, b) in sum[ch].iter_mut().zip(*src) {
                    *a += *b * g;
                }
            }
        }
        for ch in &sum {
            worst = worst.max(ch.iter().fold(0.0f32, |m, s| m.max(s.abs())));
        }
    }
    let target = db_to_gain(target_db);
    let trim = if worst > 0.0 { target / worst } else { 1.0 };
    for st in prepared.iter_mut() {
        for s in st
            .rendered
            .left
            .iter_mut()
            .chain(st.rendered.right.iter_mut())
        {
            *s *= trim;
        }
    }
    20.0 * trim.log10()
}

fn rms(samples: &[f32]) -> f64 {
    (samples
        .iter()
        .map(|&s| (s as f64) * (s as f64))
        .sum::<f64>()
        / samples.len().max(1) as f64)
        .sqrt()
}

/// Verify the loop wrap boundary `[loop_start, loop_end)` of a decoded WAV.
pub fn verify_wav_loop_seam(
    section: &str,
    layer: &str,
    audio: &wavread::Audio,
    loop_start: usize,
    loop_end: usize,
    threshold: f32,
) -> Result<TakeSeamCheck, String> {
    if !threshold.is_finite() || threshold <= 0.0 {
        return Err(format!("seam threshold {threshold} must be finite and > 0"));
    }
    let frames = validate_audio(audio, &format!("stem ({section}, {layer})"))?;
    if loop_end <= loop_start {
        return Err(format!(
            "invalid loop bounds for ({section}, {layer}): loop_end ({loop_end}) must be > loop_start ({loop_start})"
        ));
    }
    if loop_end > frames {
        return Err(format!(
            "loop bounds out of range for ({section}, {layer}): loop_end ({loop_end}) exceeds frames ({frames})"
        ));
    }
    let loop_len = loop_end - loop_start;
    if loop_len < 2 {
        return Err(format!(
            "loop bounds for ({section}, {layer}) need at least 2 frames"
        ));
    }
    let win = (loop_len / 4).clamp(1, 2400);
    let mut max_boundary_step = 0.0f32;
    let mut sum_sq_step = 0.0f64;
    let mut window_rms_delta = 0.0f32;
    let mut steps = Vec::with_capacity(loop_len * audio.channels.len());
    for ch in &audio.channels {
        let slice = &ch[loop_start..loop_end];
        let step = (slice[slice.len() - 1] - slice[0]).abs();
        max_boundary_step = max_boundary_step.max(step);
        sum_sq_step += (step as f64) * (step as f64);
        let head_rms = rms(&slice[..win]);
        let tail_rms = rms(&slice[slice.len() - win..]);
        window_rms_delta = window_rms_delta.max((tail_rms - head_rms).abs() as f32);
        steps.extend(slice.windows(2).map(|w| (w[1] - w[0]).abs()));
    }
    let boundary_rms_step = (sum_sq_step / audio.channels.len() as f64).sqrt() as f32;
    steps.sort_by(|a, b| a.total_cmp(b));
    let p999_step = if steps.is_empty() {
        0.0
    } else {
        steps[(steps.len() as f64 * 0.999) as usize % steps.len()]
    };
    let effective_limit = p999_step.max(threshold);
    let seamless = max_boundary_step <= effective_limit && boundary_rms_step <= effective_limit;
    Ok(TakeSeamCheck {
        section: section.to_string(),
        layer: layer.to_string(),
        loop_start_frame: loop_start,
        loop_end_frame: loop_end,
        max_boundary_step,
        boundary_rms_step,
        window_rms_delta,
        p999_step,
        threshold,
        seamless,
    })
}

/// Cut, crossfade, normalize, export an `audioforge-music` v1 package, and
/// verify every exported loop stem on disk.
pub fn export_takes(
    spec: &TakePackSpec,
    plan: &[PlanStep],
    dir: &Path,
) -> Result<TakeExportReport, String> {
    validate_spec(spec)?;
    let mut prepared = Vec::with_capacity(spec.stems.len());
    for st in &spec.stems {
        let sec = spec
            .sections
            .iter()
            .find(|s| s.id == st.section)
            .ok_or_else(|| format!("unknown section {:?}", st.section))?;
        let layer = spec
            .layers
            .iter()
            .find(|l| l.id == st.layer)
            .ok_or_else(|| format!("unknown layer {:?}", st.layer))?;
        prepared.push(prepare_stem(spec, sec, layer, st)?);
    }
    check_section_alignment(&prepared)?;
    let trim_db = apply_shared_trim(&mut prepared, spec.normalize_peak_dbfs);

    let mut sections_meta = Vec::new();
    let mut section_json_list = Vec::new();
    for s in &spec.sections {
        let Some(first) = prepared.iter().find(|p| p.section == s.id) else {
            continue;
        };
        sections_meta.push(PackageSection {
            id: &s.id,
            role_name: s.role.as_str(),
            once: s.role.is_once(),
            next: s.next.as_deref(),
            length_beats: first.length_beats,
            loop_start_beats: first.loop_start_beats,
            loop_end_beats: first.loop_end_beats,
        });
        let mut entry = json!({
            "id": s.id,
            "role": s.role.as_str(),
            "length_beats": first.length_beats,
        });
        if let Some(next) = &s.next {
            entry["next"] = json!(next);
        }
        section_json_list.push(entry);
    }
    let stems_meta: Vec<PackageStem<'_>> = prepared
        .iter()
        .map(|p| {
            let looped = !p.role.is_once();
            PackageStem {
                section: &p.section,
                layer: &p.layer,
                layer_gain_db: p.layer_gain_db,
                min_intensity: p.min_intensity,
                stem: &p.rendered,
                loop_start_frame: if looped { p.loop_start_frame as u32 } else { 0 },
                loop_end_frame: if looped { p.loop_end_frame as u32 } else { 0 },
                loop_start_beats: p.loop_start_beats,
                loop_end_beats: p.loop_end_beats,
                length_beats: p.length_beats,
            }
        })
        .collect();

    let outro_tail_secs = plan
        .last()
        .and_then(|step| {
            prepared
                .iter()
                .find(|p| p.section == step.section && p.role.is_once())
        })
        .map(|p| {
            let stem_secs = p.rendered.frames() as f64 / SAMPLE_RATE as f64;
            let len_secs = p.length_beats as f64 * (60.0 / spec.tempo as f64);
            (stem_secs - len_secs).max(0.0)
        })
        .unwrap_or(0.0);

    let scene = build_preview_scene_json(
        spec.tempo,
        spec.beats_per_bar,
        &sections_meta,
        plan,
        outro_tail_secs,
    );
    let credits: Vec<Value> = prepared
        .iter()
        .map(|p| {
            json!({
                "id": format!("{}_{}", p.section, p.layer),
                "kind": "take",
                "source": p.source_label.as_deref().unwrap_or("ccez-daw take"),
                "licence": "owner-provided take",
            })
        })
        .collect();
    let meta = PackageManifestMeta {
        name: &spec.name,
        tempo: spec.tempo,
        beats_per_bar: spec.beats_per_bar,
        key: &spec.key,
        xfade_beats: spec.xfade_beats,
        trim_db,
        instruments: credits,
        sections_json: Value::Array(section_json_list),
        layers_json: serde_json::to_value(&spec.layers).map_err(|e| e.to_string())?,
        composition_json: None,
    };
    let (files, warnings) =
        write_audioforge_package(dir, &meta, &sections_meta, &stems_meta, &scene, Vec::new())?;
    let loop_seams = verify_exported_package(dir, DEFAULT_CLICK_THRESHOLD)?;
    Ok(TakeExportReport {
        dir: dir.display().to_string(),
        files,
        stems: prepared.len(),
        trim_db,
        loop_seams,
        warnings,
    })
}

/// Validate an exported `audioforge-music` package directory on disk and verify
/// every exported loop WAV stem's wrap boundary against `threshold`.
pub fn verify_exported_package(dir: &Path, threshold: f32) -> Result<Vec<TakeSeamCheck>, String> {
    let read_json = |rel: &str| -> Result<Value, String> {
        let p = dir.join(rel);
        let text = std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))?;
        serde_json::from_str(&text).map_err(|e| format!("{}: {e}", p.display()))
    };
    let manifest = read_json("manifest.json")?;
    let score = read_json("score.json")?;
    let mix = read_json("mix.json")?;
    let _preview = read_json("scenes/preview.json")?;

    if manifest["format"].as_str() != Some(super::audioforge::FORMAT) {
        return Err(format!(
            "manifest format {:?} != audioforge-music",
            manifest["format"]
        ));
    }
    if manifest["format_version"].as_u64() != Some(super::audioforge::FORMAT_VERSION as u64) {
        return Err(format!(
            "manifest format_version {:?} != 1",
            manifest["format_version"]
        ));
    }
    if mix["sample_rate"].as_u64() != Some(SAMPLE_RATE as u64) {
        return Err(format!(
            "mix sample_rate {:?} != {SAMPLE_RATE}",
            mix["sample_rate"]
        ));
    }
    let bpm = score["bpm"]
        .as_f64()
        .ok_or_else(|| "score.json: missing bpm".to_string())?;
    if !bpm.is_finite() || !(40.0..=300.0).contains(&bpm) {
        return Err(format!("score.json: invalid bpm {bpm}"));
    }
    let sections = score["sections"]
        .as_object()
        .ok_or_else(|| "score.json: missing sections object".to_string())?;
    let initial = score["initial"]
        .as_str()
        .ok_or_else(|| "score.json: missing initial".to_string())?;
    if !sections.contains_key(initial) {
        return Err(format!("score.json: initial {initial:?} not in sections"));
    }
    for (sec_id, sec_val) in sections {
        let once = sec_val["once"].as_bool().unwrap_or(false);
        if once {
            let len = sec_val["length_beats"].as_f64().unwrap_or(0.0);
            if !len.is_finite() || len <= 0.0 || len > 4096.0 {
                return Err(format!(
                    "score.json: once section {sec_id:?} invalid length_beats"
                ));
            }
            if sec_val.get("loop_start_beats").is_some() || sec_val.get("loop_end_beats").is_some()
            {
                return Err(format!(
                    "score.json: once section {sec_id:?} must not set loop points"
                ));
            }
            if let Some(next) = sec_val.get("next").and_then(|v| v.as_str()) {
                if !sections.contains_key(next) {
                    return Err(format!(
                        "score.json: section {sec_id:?} next {next:?} missing"
                    ));
                }
            }
        } else {
            if sec_val.get("next").is_some() {
                return Err(format!(
                    "score.json: loop section {sec_id:?} must not set next"
                ));
            }
            let ls = sec_val["loop_start_beats"].as_f64().unwrap_or(0.0);
            let le = sec_val["loop_end_beats"].as_f64().unwrap_or(0.0);
            if !ls.is_finite() || !le.is_finite() || ls < 0.0 || le <= ls || le > 4096.0 {
                return Err(format!(
                    "score.json: loop section {sec_id:?} invalid loop bounds ({ls}..{le})"
                ));
            }
        }
    }

    let stems = manifest["stems"]
        .as_array()
        .ok_or_else(|| "manifest.json: missing stems array".to_string())?;
    let mut checks = Vec::new();
    for st in stems {
        let rel = st["path"]
            .as_str()
            .ok_or_else(|| "manifest stem missing path".to_string())?;
        let ev = st["event"]
            .as_str()
            .ok_or_else(|| "manifest stem missing event".to_string())?;
        let sec = st["section"]
            .as_str()
            .ok_or_else(|| "manifest stem missing section".to_string())?;
        let layer = st["layer"]
            .as_str()
            .ok_or_else(|| "manifest stem missing layer".to_string())?;
        let expected_frames = st["frames"]
            .as_u64()
            .ok_or_else(|| "manifest stem missing frames".to_string())?
            as usize;

        let ev_json = read_json(&format!("events/{ev}.json"))?;
        if ev_json["kind"].as_str() != Some("loop")
            || ev_json["sources"][0]["wav"].as_str() != Some(format!("../{rel}").as_str())
        {
            return Err(format!("events/{ev}.json: invalid source or kind"));
        }

        let decoded = wavread::read(&dir.join(rel))?;
        if decoded.sample_rate != SAMPLE_RATE || decoded.channels.len() != 2 {
            return Err(format!(
                "{rel}: expected {SAMPLE_RATE} Hz stereo WAV, got {} Hz {} ch",
                decoded.sample_rate,
                decoded.channels.len()
            ));
        }
        if decoded.frames() != expected_frames {
            return Err(format!(
                "{rel}: frame count {} != manifest {expected_frames}",
                decoded.frames()
            ));
        }

        let sec_val = sections
            .get(sec)
            .ok_or_else(|| format!("manifest stem {rel} section {sec:?} missing in score"))?;
        let once = sec_val["once"].as_bool().unwrap_or(false);
        if once {
            if decoded.sample_loop.is_some() {
                return Err(format!(
                    "{rel}: once cue should not carry a smpl loop chunk"
                ));
            }
        } else {
            let fpb = 60.0 / bpm * SAMPLE_RATE as f64;
            let ls = sec_val["loop_start_beats"]
                .as_f64()
                .map(|b| (b * fpb).round() as usize)
                .unwrap_or(0);
            let le = sec_val["loop_end_beats"]
                .as_f64()
                .map(|b| (b * fpb).round() as usize)
                .unwrap_or(decoded.frames())
                .min(decoded.frames());
            if decoded.sample_loop != Some((ls, le)) {
                return Err(format!(
                    "{rel}: WAV smpl loop {:?} does not match score ({ls}, {le})",
                    decoded.sample_loop
                ));
            }
            let check = verify_wav_loop_seam(sec, layer, &decoded, ls, le, threshold)?;
            if !check.seamless {
                return Err(format!(
                    "{rel}: loop seam clicks (max_step {:.4}, rms_step {:.4} > {:.4})",
                    check.max_boundary_step, check.boundary_rms_step, check.threshold
                ));
            }
            checks.push(check);
        }
    }
    Ok(checks)
}
