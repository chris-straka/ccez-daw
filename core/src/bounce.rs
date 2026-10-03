//! Track B (agent 2): background freeze/bounce.
//!
//! Offline render of graph subtrees to stems. The realtime engine renders on
//! a hard deadline; this module renders the same project *without* a clock:
//! it walks tracks/clips in a [`BounceConfig`] beat window, synthesizes each
//! clip deterministically, sums the subtree, and hands back [`Stem`]s — mono
//! f32 audio plus loop points — ready to encode as WAV game deliverables.
//!
//! Teaching note: "freeze" and "bounce" are the same verb with different
//! nouns. Freezing a track renders it to a stem asset inside the project
//! (so the realtime graph can later play the file instead of the devices).
//! Bouncing renders one stem per track, or one mix stem, for export. Both
//! are pure functions over [`Project`](crate::model::Project); only
//! [`freeze_track`] touches the [`Engine`](crate::engine::Engine), and only
//! through its public asset API — so frozen stems ride along in portable
//! bundles for free.
//!
//! v1 limits (honest, not hidden): clip sources render procedurally —
//! `builtin:click` audio clips render metronome clicks, MIDI clips render a
//! loop-clean reference tone — except Audio clips naming `asset:` keys,
//! which render decoded WAV audio from the [`SampleBank`] (a dangling key
//! renders silence, never a tone). MIDI note bytes still have no decoder.
//! Device chains contribute only `gain`/`volume` params; full DSP lands with
//! the device track. Pan is ignored: v1 stems are mono, the standard shape
//! for looped game deliverables.
//!
//! This module adds no IPC or project-schema surface (like `branch.rs`), so
//! the typegen drift gate (`bun run typegen -- --check`) is unaffected.

use std::collections::HashMap;

use crate::engine::Engine;
use crate::loopseam::resample_linear;
use crate::model::{Clip, ClipKind, Project};
use crate::plugins::ara::{AraDocument, MockAraEffect, MusicalContext};

/// Default render rate: CD-adjacent, the game-audio lingua franca.
pub const DEFAULT_SAMPLE_RATE: u32 = 44100;
/// Asset kind used when a frozen stem is stored on the engine.
pub const FROZEN_STEM_KIND: &str = "audio";

#[derive(Debug)]
pub enum BounceError {
    UnknownTrack(String),
    BadRange(String),
    WavParse(String),
    Engine(crate::engine::EngineError),
}

impl std::fmt::Display for BounceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownTrack(t) => write!(f, "unknown bounce track `{t}`"),
            Self::BadRange(m) => write!(f, "bad bounce range: {m}"),
            Self::WavParse(m) => write!(f, "bad wav: {m}"),
            Self::Engine(e) => write!(f, "bounce engine: {e}"),
        }
    }
}

impl std::error::Error for BounceError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Engine(e) => Some(e),
            _ => None,
        }
    }
}

impl From<crate::engine::EngineError> for BounceError {
    fn from(e: crate::engine::EngineError) -> Self {
        Self::Engine(e)
    }
}

pub type Result<T> = std::result::Result<T, BounceError>;

/// What to render: a beat window at a sample rate. Tempo comes from the
/// project itself, so stems always agree with the timeline.
#[derive(Debug, Clone, PartialEq)]
pub struct BounceConfig {
    pub sample_rate: u32,
    pub start_beat: f64,
    pub length_beats: f64,
}

impl BounceConfig {
    pub fn new(sample_rate: u32, start_beat: f64, length_beats: f64) -> Result<Self> {
        if sample_rate == 0 {
            return Err(BounceError::BadRange("sample_rate must be > 0".to_string()));
        }
        if !length_beats.is_finite() || length_beats <= 0.0 {
            return Err(BounceError::BadRange(format!(
                "length_beats must be positive, got {length_beats}"
            )));
        }
        if !start_beat.is_finite() || start_beat < 0.0 {
            return Err(BounceError::BadRange(format!(
                "start_beat must be >= 0, got {start_beat}"
            )));
        }
        Ok(Self {
            sample_rate,
            start_beat,
            length_beats,
        })
    }

    fn sample_count(&self, tempo: f64) -> Result<usize> {
        if !tempo.is_finite() || tempo <= 0.0 {
            return Err(BounceError::BadRange(format!("tempo must be positive, got {tempo}")));
        }
        Ok((self.length_beats * 60.0 / tempo * self.sample_rate as f64).round() as usize)
    }
}

/// One rendered stem: mono f32 samples plus loop points in samples.
///
/// `loop_start`/`loop_end` mark the loopable region (a half-open range:
/// play `[loop_start, loop_end)` and wrap). Full-window renders loop the
/// whole stem, which is what game engines want for ambient/music beds.
#[derive(Debug, Clone, PartialEq)]
pub struct Stem {
    pub name: String,
    pub sample_rate: u32,
    pub samples: Vec<f32>,
    pub loop_start: u32,
    pub loop_end: u32,
}

impl Stem {
    fn silent(name: &str, sample_rate: u32, n: usize) -> Self {
        Self {
            name: name.to_string(),
            sample_rate,
            samples: vec![0.0; n],
            loop_start: 0,
            loop_end: n as u32,
        }
    }

    pub fn peak(&self) -> f32 {
        self.samples.iter().fold(0.0f32, |m, s| m.max(s.abs()))
    }

    pub fn is_silent(&self) -> bool {
        self.samples.iter().all(|s| *s == 0.0)
    }
}

/// Decoded sample audio addressable by asset key: what `asset:` clips
/// render. Built from the engine's `audio`-kind WAV assets (see
/// [`Engine::sample_bank`]); undecodable blobs never enter the bank, so a
/// dangling key renders silence, never an error.
#[derive(Debug, Clone, Default)]
pub struct SampleBank {
    samples: HashMap<String, Sample>,
}

/// One decoded sample: mono frames at their native rate (resampled to the
/// render rate at clip time).
#[derive(Debug, Clone)]
pub struct Sample {
    pub rate: u32,
    pub mono: Vec<f32>,
}

impl SampleBank {
    pub fn empty() -> Self {
        Self { samples: HashMap::new() }
    }

    /// Decode WAV `bytes` under `key`, replacing any previous entry.
    /// Non-WAV/decoding failures are the caller's `Err` — the bank builder
    /// skips those keys instead.
    pub fn insert_wav(&mut self, key: &str, bytes: &[u8]) -> Result<()> {
        let stem = decode_wav(bytes)?;
        self.samples.insert(
            key.to_string(),
            Sample { rate: stem.sample_rate, mono: stem.samples },
        );
        Ok(())
    }

    pub fn get(&self, key: &str) -> Option<&Sample> {
        self.samples.get(key)
    }
}

/// `asset:` source prefix: Audio clips naming a bank key render decoded
/// sample audio; every other source keeps the procedural v1 path.
pub const ASSET_SOURCE_PREFIX: &str = "asset:";

/// Render one track's subtree (its clips x device gains x track volume).
/// Per-track stems ignore mute/solo: a stem is the track's raw material,
/// not its mixer state. The mix (`render_mix`) honors both.
pub fn render_track(project: &Project, track_id: &str, config: &BounceConfig) -> Result<Stem> {
    render_track_with_bank(project, track_id, config, &SampleBank::empty())
}

/// [`render_track`] with decoded sample audio for `asset:` clips.
pub fn render_track_with_bank(
    project: &Project,
    track_id: &str,
    config: &BounceConfig,
    bank: &SampleBank,
) -> Result<Stem> {
    let track = project
        .tracks
        .iter()
        .find(|t| t.id == track_id)
        .ok_or_else(|| BounceError::UnknownTrack(track_id.to_string()))?;
    let n = config.sample_count(project.tempo)?;
    if n == 0 {
        return Ok(Stem::silent(&track.name, config.sample_rate, 0));
    }
    let gain = track_gain(project, track_id) * track.volume as f32;
    let mut samples = vec![0.0f32; n];
    for clip in project.clips.iter().filter(|c| c.track_id == track_id) {
        render_clip_into(clip, project.tempo, config, bank, &mut samples);
    }
    for s in &mut samples {
        *s *= gain;
    }
    Ok(Stem {
        name: track.name.clone(),
        sample_rate: config.sample_rate,
        samples,
        loop_start: 0,
        loop_end: n as u32,
    })
}

/// Render the full mix: every track summed, honoring mute and solo (if any
/// track is soloed, only soloed tracks sound).
pub fn render_mix(project: &Project, config: &BounceConfig) -> Result<Stem> {
    render_mix_with_bank(project, config, &SampleBank::empty())
}

/// [`render_mix`] with decoded sample audio for `asset:` clips.
pub fn render_mix_with_bank(
    project: &Project,
    config: &BounceConfig,
    bank: &SampleBank,
) -> Result<Stem> {
    let n = config.sample_count(project.tempo)?;
    if n == 0 {
        return Ok(Stem::silent("mix", config.sample_rate, 0));
    }
    let any_solo = project.tracks.iter().any(|t| t.solo);
    let mut samples = vec![0.0f32; n];
    for track in &project.tracks {
        if track.muted || (any_solo && !track.solo) {
            continue;
        }
        let gain = track_gain(project, &track.id) * track.volume as f32;
        let mut buf = vec![0.0f32; n];
        for clip in project.clips.iter().filter(|c| c.track_id == track.id) {
            render_clip_into(clip, project.tempo, config, bank, &mut buf);
        }
        for (dst, src) in samples.iter_mut().zip(buf.iter()) {
            *dst += *src * gain;
        }
    }
    Ok(Stem {
        name: "mix".to_string(),
        sample_rate: config.sample_rate,
        samples,
        loop_start: 0,
        loop_end: n as u32,
    })
}

/// Render one stem per track: the game-deliverable batch.
pub fn render_stems(project: &Project, config: &BounceConfig) -> Result<Vec<Stem>> {
    render_stems_with_bank(project, config, &SampleBank::empty())
}

/// [`render_stems`] with decoded sample audio for `asset:` clips.
pub fn render_stems_with_bank(
    project: &Project,
    config: &BounceConfig,
    bank: &SampleBank,
) -> Result<Vec<Stem>> {
    project
        .tracks
        .iter()
        .map(|t| render_track_with_bank(project, &t.id, config, bank))
        .collect()
}

/// Freeze a track: render it and store the WAV as an engine asset so it
/// travels with the portable bundle. Returns the asset key.
pub fn freeze_track(
    engine: &mut Engine,
    project: &Project,
    track_id: &str,
    config: &BounceConfig,
) -> Result<String> {
    let bank = engine.sample_bank();
    let stem = render_track_with_bank(project, track_id, config, &bank)?;
    let wav = encode_wav(&stem);
    let key = format!("freeze-{track_id}.wav");
    engine.store_asset(&key, FROZEN_STEM_KIND, &wav)?;
    Ok(key)
}

// -- internals ---------------------------------------------------------------

/// Effective device-chain gain for a track: the product of every `gain` or
/// `volume` param on its devices (1.0 when the chain has none).
fn track_gain(project: &Project, track_id: &str) -> f32 {
    let Some(track) = project.tracks.iter().find(|t| t.id == track_id) else {
        return 1.0;
    };
    let mut gain = 1.0f32;
    for dev_id in &track.device_ids {
        if let Some(dev) = project.devices.iter().find(|d| d.id == *dev_id) {
            for p in &dev.params {
                if p.id == "gain" || p.id == "volume" {
                    gain *= p.value as f32;
                }
            }
        }
    }
    gain
}

/// Add one clip's contribution into `out`. Time is beats; only the overlap
/// with the bounce window sounds.
fn render_clip_into(
    clip: &Clip,
    tempo: f64,
    config: &BounceConfig,
    bank: &SampleBank,
    out: &mut [f32],
) {
    let buf = clip_window(clip, tempo, config, bank, out.len());
    for (dst, s) in out.iter_mut().zip(buf.iter()) {
        *dst += *s;
    }
}

/// Render one clip alone into a full-window buffer (silence outside the
/// clip's overlap with the window). Same math as [`render_clip_into`],
/// owned per clip so the ARA path can transform a clip before it sums.
fn clip_window(
    clip: &Clip,
    tempo: f64,
    config: &BounceConfig,
    bank: &SampleBank,
    n: usize,
) -> Vec<f32> {
    let mut buf = vec![0.0f32; n];
    let sr = config.sample_rate as f64;
    let beats_per_sec = tempo / 60.0;
    let win_end = config.start_beat + config.length_beats;
    let clip_end = clip.start_beats + clip.length_beats;
    if clip_end <= config.start_beat || clip.start_beats >= win_end {
        return buf;
    }
    // Decoded sample audio: `asset:` Audio clips play the banked WAV from
    // the clip start, truncated to the clip bounds (arrangement wins).
    // A dangling key renders silence — a content hole, never a tone.
    if clip.kind == ClipKind::Audio {
        if let Some(key) = clip.source.strip_prefix(ASSET_SOURCE_PREFIX) {
            if let Some(sample) = bank.get(key) {
                let ratio = sample.rate as f64 / sr;
                if let Ok(audio) = resample_linear(&sample.mono, ratio) {
                    let start = ((clip.start_beats - config.start_beat) * sr / beats_per_sec)
                        .round() as i64;
                    let end = ((clip_end - config.start_beat) * sr / beats_per_sec).round()
                        as i64;
                    for (i, s) in audio.iter().enumerate() {
                        let dst = start + i as i64;
                        if dst >= end {
                            break;
                        }
                        if dst >= 0 && (dst as usize) < buf.len() {
                            buf[dst as usize] += s;
                        }
                    }
                }
            }
            return buf;
        }
    }
    let is_click = clip.kind == crate::model::ClipKind::Audio && clip.source == "builtin:click";
    // Loop-clean reference tone: exactly 220 cycles per beat at any tempo,
    // so whole-beat windows wrap without a click (A440 at 120 BPM).
    let tone_hz = 220.0 * beats_per_sec;
    for (i, dst) in buf.iter_mut().enumerate() {
        let beat = config.start_beat + (i as f64 / sr) * beats_per_sec;
        if beat < clip.start_beats || beat >= clip_end {
            continue;
        }
        let t_sec = (beat - clip.start_beats) / beats_per_sec;
        let s = if is_click {
            click_sample(beat, t_sec)
        } else {
            tone_sample(tone_hz, t_sec, clip.length_beats / beats_per_sec)
        };
        *dst = s;
    }
    buf
}

// -- ARA document model wiring ------------------------------------------------

/// Build the ARA document for one track's clips: each clip's window audio
/// becomes the clip's source, and each valid clip maps to a region entry
/// (beats → seconds at the project tempo). The entry's source offset pins
/// the region's project-timeline seconds to the window-based source
/// buffer (see [`crate::plugins::ara::AraRegionEntry`]).
pub fn ara_document_for_track(
    project: &Project,
    track_id: &str,
    config: &BounceConfig,
) -> Result<AraDocument> {
    if !project.tracks.iter().any(|t| t.id == track_id) {
        return Err(BounceError::UnknownTrack(track_id.to_string()));
    }
    let n = config.sample_count(project.tempo)?;
    let mut doc = AraDocument::new(MusicalContext::from_project(project));
    let window_start_sec = config.start_beat * 60.0 / project.tempo;
    for clip in project.clips.iter().filter(|c| c.track_id == track_id) {
        // ARA documents stay procedural (empty bank): effect analysis on
        // stable synthetic sources, not on decoded samples.
        let buf = clip_window(clip, project.tempo, config, &SampleBank::empty(), n);
        doc.add_source(&clip.id, config.sample_rate, buf)
            .map_err(|e| BounceError::BadRange(e.to_string()))?;
        if let Some(region_id) = doc
            .add_region_for_clip(clip, &clip.id)
            .map_err(|e| BounceError::BadRange(e.to_string()))?
        {
            let entry = doc
                .regions()
                .iter()
                .find(|e| e.id == region_id)
                .expect("just-added region");
            let offset = entry.region.start_sec - window_start_sec;
            doc.set_source_offset(&region_id, offset);
        }
    }
    Ok(doc)
}

/// Active span of one clip inside a full-window buffer: `(first, one_past,
/// region_time_of_first)` where times are seconds from the clip start.
/// `None` when the clip does not overlap the window.
fn clip_span_frames(
    clip: &Clip,
    tempo: f64,
    config: &BounceConfig,
    n: usize,
) -> Option<(usize, usize, f64)> {
    let sr = config.sample_rate as f64;
    let beats_per_sec = tempo / 60.0;
    let clip_end = clip.start_beats + clip.length_beats;
    let mut first: Option<usize> = None;
    let mut last = 0usize;
    for i in 0..n {
        let beat = config.start_beat + (i as f64 / sr) * beats_per_sec;
        if beat >= clip.start_beats && beat < clip_end {
            if first.is_none() {
                first = Some(i);
            }
            last = i;
        } else if first.is_some() {
            break;
        }
    }
    let i0 = first?;
    let t0 = (config.start_beat + (i0 as f64 / sr) * beats_per_sec - clip.start_beats) / beats_per_sec;
    Some((i0, last + 1, t0))
}

/// Render one clip's window buffer through its ARA effect when the document
/// maps the clip to a region; otherwise the buffer passes through
/// untouched. An effect with no region in the document is ignored — the
/// document (not the effect map) decides what is placed, so unplaced edits
/// can never color a bounce.
fn apply_ara_to_clip(
    buf: &mut [f32],
    clip: &Clip,
    tempo: f64,
    config: &BounceConfig,
    ara: &AraDocument,
    effect: &MockAraEffect,
) {
    let Some(entry) = ara.regions_for_clip(&clip.id).into_iter().next() else {
        return;
    };
    let Some((i0, i1, t0)) = clip_span_frames(clip, tempo, config, buf.len()) else {
        return;
    };
    let rendered = effect.render(&buf[i0..i1], config.sample_rate, entry.region.start_sec + t0);
    buf[i0..i1].copy_from_slice(&rendered);
}

/// Render one track's stem with ARA effects applied per clip (same gains,
/// windows, and looping as [`render_track`]; per-track stems still ignore
/// mute/solo).
pub fn render_track_with_ara(
    project: &Project,
    track_id: &str,
    config: &BounceConfig,
    ara: &AraDocument,
    effects: &HashMap<String, MockAraEffect>,
) -> Result<Stem> {
    let track = project
        .tracks
        .iter()
        .find(|t| t.id == track_id)
        .ok_or_else(|| BounceError::UnknownTrack(track_id.to_string()))?;
    let n = config.sample_count(project.tempo)?;
    if n == 0 {
        return Ok(Stem::silent(&track.name, config.sample_rate, 0));
    }
    let gain = track_gain(project, track_id) * track.volume as f32;
    let mut samples = vec![0.0f32; n];
    for clip in project.clips.iter().filter(|c| c.track_id == track_id) {
        let mut buf = clip_window(clip, project.tempo, config, &SampleBank::empty(), n);
        if let Some(fx) = effects.get(&clip.id) {
            apply_ara_to_clip(&mut buf, clip, project.tempo, config, ara, fx);
        }
        for (dst, s) in samples.iter_mut().zip(buf.iter()) {
            *dst += *s;
        }
    }
    for s in &mut samples {
        *s *= gain;
    }
    Ok(Stem {
        name: track.name.clone(),
        sample_rate: config.sample_rate,
        samples,
        loop_start: 0,
        loop_end: n as u32,
    })
}

/// Render the full mix with ARA effects applied per clip (honors mute and
/// solo exactly like [`render_mix`]).
pub fn render_mix_with_ara(
    project: &Project,
    config: &BounceConfig,
    ara: &AraDocument,
    effects: &HashMap<String, MockAraEffect>,
) -> Result<Stem> {
    let n = config.sample_count(project.tempo)?;
    if n == 0 {
        return Ok(Stem::silent("mix", config.sample_rate, 0));
    }
    let any_solo = project.tracks.iter().any(|t| t.solo);
    let mut samples = vec![0.0f32; n];
    for track in &project.tracks {
        if track.muted || (any_solo && !track.solo) {
            continue;
        }
        let gain = track_gain(project, &track.id) * track.volume as f32;
        let mut buf = vec![0.0f32; n];
        for clip in project.clips.iter().filter(|c| c.track_id == track.id) {
            let mut clip_buf = clip_window(clip, project.tempo, config, &SampleBank::empty(), n);
            if let Some(fx) = effects.get(&clip.id) {
                apply_ara_to_clip(&mut clip_buf, clip, project.tempo, config, ara, fx);
            }
            for (dst, s) in buf.iter_mut().zip(clip_buf.iter()) {
                *dst += *s;
            }
        }
        for (dst, src) in samples.iter_mut().zip(buf.iter()) {
            *dst += *src * gain;
        }
    }
    Ok(Stem {
        name: "mix".to_string(),
        sample_rate: config.sample_rate,
        samples,
        loop_start: 0,
        loop_end: n as u32,
    })
}

/// Metronome click: a fast-decaying pulse struck on each beat line.
/// Decay runs in *beat* units so the shape is tempo-independent.
fn click_sample(beat: f64, _t_sec: f64) -> f32 {
    let phase = beat - beat.floor();
    let dt_beats = if phase < 0.5 { phase } else { 0.0 };
    (0.9 * (-dt_beats * 18.0).exp()) as f32
}

/// Reference tone with 5ms raised-cosine fades so clip edges never click.
fn tone_sample(hz: f64, t_sec: f64, clip_sec: f64) -> f32 {
    let fade = 0.005;
    let env = ((t_sec / fade).min(1.0)).min(((clip_sec - t_sec) / fade).max(0.0).min(1.0));
    let shaped = env * env * (3.0 - 2.0 * env);
    (0.5 * (2.0 * std::f64::consts::PI * hz * t_sec).sin() * shaped) as f32
}

// -- WAV (16-bit PCM mono + smpl loop) ----------------------------------------

/// Encode a stem as 16-bit PCM mono WAV with a `smpl` chunk carrying the
/// loop points — the shape game engines import directly.
pub fn encode_wav(stem: &Stem) -> Vec<u8> {
    let mut data = Vec::with_capacity(stem.samples.len() * 2);
    for s in &stem.samples {
        let v = (s.clamp(-1.0, 1.0) * 32767.0).round() as i16;
        data.extend_from_slice(&v.to_le_bytes());
    }
    // smpl chunk body: 9 u32 header + 1 loop (6 u32).
    let mut smpl = Vec::new();
    for v in [0u32, 0, 1_000_000_000 / stem.sample_rate.max(1), 60, 0, 0, 0, 1, 0] {
        smpl.extend_from_slice(&v.to_le_bytes());
    }
    for v in [0u32, 0, stem.loop_start, stem.loop_end, 0, 0] {
        smpl.extend_from_slice(&v.to_le_bytes());
    }
    let mut out = Vec::new();
    out.extend_from_slice(b"RIFF");
    let riff_len = 4 + 8 + 16 + 8 + smpl.len() as usize + 8 + data.len();
    out.extend_from_slice(&(riff_len as u32).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&stem.sample_rate.to_le_bytes());
    out.extend_from_slice(&(stem.sample_rate * 2).to_le_bytes()); // byte rate
    out.extend_from_slice(&2u16.to_le_bytes()); // block align
    out.extend_from_slice(&16u16.to_le_bytes()); // bits
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(&data);
    out.extend_from_slice(b"smpl");
    out.extend_from_slice(&(smpl.len() as u32).to_le_bytes());
    out.extend_from_slice(&smpl);
    out
}

/// Decode our own WAV output (PCM16 mono + optional `smpl` loop).
pub fn decode_wav(bytes: &[u8]) -> Result<Stem> {
    let err = |m: &str| BounceError::WavParse(m.to_string());
    if bytes.len() < 44 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(err("not a RIFF/WAVE file"));
    }
    let mut pos = 12;
    let mut sample_rate = 0u32;
    let mut samples: Vec<f32> = Vec::new();
    let mut loop_start = 0u32;
    let mut loop_end = 0u32;
    let mut saw_smpl = false;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let len = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().unwrap()) as usize;
        let body = bytes.get(pos + 8..pos + 8 + len).ok_or_else(|| err("truncated chunk"))?;
        match id {
            b"fmt " => {
                if len < 16
                    || u16::from_le_bytes([body[0], body[1]]) != 1
                    || u16::from_le_bytes([body[2], body[3]]) != 1
                    || u16::from_le_bytes([body[14], body[15]]) != 16
                {
                    return Err(err("want PCM16 mono"));
                }
                sample_rate = u32::from_le_bytes([body[4], body[5], body[6], body[7]]);
            }
            b"data" => {
                if body.len() % 2 != 0 {
                    return Err(err("odd data length"));
                }
                samples = body
                    .chunks_exact(2)
                    .map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32767.0)
                    .collect();
            }
            b"smpl" => {
                if body.len() >= 36 + 24 {
                    loop_start = u32::from_le_bytes(body[44..48].try_into().unwrap());
                    loop_end = u32::from_le_bytes(body[48..52].try_into().unwrap());
                    saw_smpl = true;
                }
            }
            _ => {}
        }
        pos += 8 + len;
    }
    if sample_rate == 0 {
        return Err(err("missing fmt chunk"));
    }
    if !saw_smpl {
        loop_end = samples.len() as u32;
    }
    Ok(Stem {
        name: "decoded".to_string(),
        sample_rate,
        samples,
        loop_start,
        loop_end,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ClipKind, Node, Param, Track};
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    static BOUNCE_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn scratch(name: &str) -> std::path::PathBuf {
        let n = BOUNCE_COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("ccez-bounce-{name}-{}-{n}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    fn cfg() -> BounceConfig {
        // 4 beats at 120 BPM, 8kHz mono: fast tests, exact sample math.
        BounceConfig::new(8000, 0.0, 4.0).expect("config")
    }

    fn tone_project() -> Project {
        let mut p = Project::new("p", "P");
        p.tempo = 120.0;
        p.tracks.push(Track {
            id: "trk".to_string(),
            name: "Lead".to_string(),
            volume: 0.8,
            pan: 0.0,
            muted: false,
            solo: false,
            clip_ids: vec!["clip".to_string()],
            device_ids: vec![],
        });
        p.clips.push(Clip {
            id: "clip".to_string(),
            track_id: "trk".to_string(),
            name: "Phrase".to_string(),
            start_beats: 0.0,
            length_beats: 4.0,
            kind: ClipKind::Midi,
            source: "take:1".to_string(),
        });
        p
    }

    #[test]
    fn track_stem_has_exact_length_and_full_loop() {
        let p = tone_project();
        let s = render_track(&p, "trk", &cfg()).expect("render");
        // 4 beats at 120 BPM = 2s at 8kHz.
        assert_eq!(s.samples.len(), 16000);
        assert_eq!(s.sample_rate, 8000);
        assert_eq!((s.loop_start, s.loop_end), (0, 16000));
        assert!(s.peak() > 0.1, "tone must sound, peak={}", s.peak());
    }

    #[test]
    fn asset_clip_renders_decoded_wav_through_bank() {
        // A known constant stem, banked as WAV, must come back through an
        // `asset:` clip at track gain (0.8) — this is the sample path the
        // live loop and the stems share.
        let src = Stem {
            name: "hit".to_string(),
            sample_rate: 8000,
            samples: vec![0.5; 800],
            loop_start: 0,
            loop_end: 800,
        };
        let mut bank = SampleBank::empty();
        bank.insert_wav("hit.wav", &encode_wav(&src)).expect("decode");
        let mut p = Project::new("p", "P");
        p.tempo = 120.0;
        p.tracks.push(Track {
            id: "trk".to_string(),
            name: "Kit".to_string(),
            volume: 0.8,
            pan: 0.0,
            muted: false,
            solo: false,
            clip_ids: vec![],
            device_ids: vec![],
        });
        p.clips.push(Clip {
            id: "clip".to_string(),
            track_id: "trk".to_string(),
            name: "Hit".to_string(),
            start_beats: 0.0,
            length_beats: 0.2,
            kind: ClipKind::Audio,
            source: "asset:hit.wav".to_string(),
        });
        let s = render_mix_with_bank(&p, &cfg(), &bank).expect("render");
        assert!(s.peak() > 0.35, "decoded sample must sound, peak={}", s.peak());
        assert!(s.peak() < 0.45, "gain applied once, peak={}", s.peak());
    }

    #[test]
    fn missing_asset_clip_renders_silence_never_tone() {
        // A dangling `asset:` key is a content hole, not a tone cue: honest
        // silence (the sampler panel's contract), while unknown non-asset
        // sources keep today's procedural tone.
        let mut p = Project::new("p", "P");
        p.tempo = 120.0;
        p.tracks.push(Track {
            id: "trk".to_string(),
            name: "Kit".to_string(),
            volume: 0.8,
            pan: 0.0,
            muted: false,
            solo: false,
            clip_ids: vec![],
            device_ids: vec![],
        });
        p.clips.push(Clip {
            id: "clip".to_string(),
            track_id: "trk".to_string(),
            name: "Hole".to_string(),
            start_beats: 0.0,
            length_beats: 1.0,
            kind: ClipKind::Audio,
            source: "asset:nope.wav".to_string(),
        });
        let s = render_mix_with_bank(&p, &cfg(), &SampleBank::empty()).expect("render");
        assert!(s.samples.iter().all(|&x| x == 0.0), "missing asset = silence");
    }

    #[test]
    fn whole_beat_tone_loops_cleanly() {
        let p = tone_project();
        let s = render_track(&p, "trk", &cfg()).expect("render");
        // 220 cycles/beat x 4 beats = integer cycles: end meets start.
        let (first, last) = (s.samples[0], s.samples[s.samples.len() - 1]);
        assert!(first.abs() < 0.01, "fade-in starts at zero, got {first}");
        assert!(last.abs() < 0.02, "loop end meets start, got {last}");
    }

    #[test]
    fn volume_scales_peak_and_mute_kills_mix() {
        let mut p = tone_project();
        let full = render_track(&p, "trk", &cfg()).expect("render");
        p.tracks[0].volume = 0.4; // half of 0.8
        let half = render_track(&p, "trk", &cfg()).expect("render");
        assert!((half.peak() * 2.0 - full.peak()).abs() < 0.02);
        p.tracks[0].muted = true;
        // Per-track stems ignore mute (raw material)...
        assert!(!render_track(&p, "trk", &cfg()).expect("stem").is_silent());
        // ...but the mix honors it.
        assert!(render_mix(&p, &cfg()).expect("mix").is_silent());
    }

    #[test]
    fn solo_isolates_the_soloed_track() {
        let mut p = tone_project();
        p.tracks.push(Track {
            id: "trk2".to_string(),
            name: "Silent".to_string(),
            volume: 0.8,
            pan: 0.0,
            muted: false,
            solo: true,
            clip_ids: vec![],
            device_ids: vec![],
        });
        // trk2 is soloed but has no clips: mix is silent, trk stem is not.
        assert!(render_mix(&p, &cfg()).expect("mix").is_silent());
        assert!(!render_track(&p, "trk", &cfg()).expect("stem").is_silent());
    }

    #[test]
    fn device_gain_param_scales_the_stem() {
        let mut p = tone_project();
        p.devices.push(Node {
            id: "dev".to_string(),
            kind: crate::model::NodeKind::Device,
            name: "Trim".to_string(),
            params: vec![Param {
                id: "gain".to_string(),
                label: "Gain".to_string(),
                value: 0.5,
                min: 0.0,
                max: 2.0,
                default: 1.0,
                unit: "".to_string(),
            }],
        });
        p.tracks[0].device_ids.push("dev".to_string());
        let a = render_track(&p, "trk", &cfg()).expect("render");
        p.devices[0].params[0].value = 1.0;
        let b = render_track(&p, "trk", &cfg()).expect("render");
        assert!((b.peak() - a.peak() * 2.0).abs() < 0.02);
    }

    #[test]
    fn wav_round_trip_preserves_audio_and_loop() {
        let p = tone_project();
        let s = render_track(&p, "trk", &cfg()).expect("render");
        let back = decode_wav(&encode_wav(&s)).expect("decode");
        assert_eq!(back.sample_rate, s.sample_rate);
        assert_eq!((back.loop_start, back.loop_end), (0, 16000));
        assert_eq!(back.samples.len(), s.samples.len());
        for (a, b) in s.samples.iter().zip(back.samples.iter()) {
            assert!((a - b).abs() < 1.0 / 32767.0 + 1e-6, "16-bit quantization only");
        }
    }

    #[test]
    fn freeze_stores_a_decodable_wav_asset() {
        let dir = scratch("freeze");
        let p = tone_project();
        let mut e = Engine::create(&dir, p.clone()).expect("create");
        let key = freeze_track(&mut e, &p, "trk", &cfg()).expect("freeze");
        assert_eq!(key, "freeze-trk.wav");
        let raw = e.load_asset(&key).expect("load frozen stem");
        assert_eq!(&raw[0..4], b"RIFF");
        let stem = decode_wav(&raw).expect("decode frozen stem");
        assert_eq!(stem.samples.len(), 16000);
        assert!(stem.peak() > 0.1);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn clips_outside_the_window_stay_silent() {
        let mut p = tone_project();
        p.clips[0].start_beats = 100.0;
        assert!(render_track(&p, "trk", &cfg()).expect("render").is_silent());
        assert!(render_mix(&p, &cfg()).expect("mix").is_silent());
    }

    #[test]
    fn bad_inputs_error_cleanly() {
        let p = tone_project();
        assert!(matches!(
            render_track(&p, "nope", &cfg()),
            Err(BounceError::UnknownTrack(_))
        ));
        assert!(BounceConfig::new(0, 0.0, 4.0).is_err());
        assert!(BounceConfig::new(8000, 0.0, 0.0).is_err());
        assert!(BounceConfig::new(8000, -1.0, 4.0).is_err());
        assert!(decode_wav(b"nope").is_err());
        assert!(decode_wav(&encode_wav(&Stem::silent("x", 8000, 8))).is_ok());
    }

    #[test]
    fn ara_region_gain_transforms_the_bounce() {
        use crate::plugins::ara::MockAraEffect;
        use std::collections::HashMap;
        let p = tone_project();
        let config = cfg();
        let ara = ara_document_for_track(&p, "trk", &config).expect("document");
        assert_eq!(ara.source_count(), 1);
        assert_eq!(ara.region_count(), 1);
        // No bindings: identical to the plain stem (wiring adds nothing).
        let plain = render_track(&p, "trk", &config).expect("render");
        let unbound = render_track_with_ara(&p, "trk", &config, &ara, &HashMap::new())
            .expect("unbound render");
        assert_eq!(unbound.samples, plain.samples);
        // A 0.5 region gain halves the peak through the ARA region.
        let mut effects = HashMap::new();
        effects.insert("clip".to_string(), MockAraEffect::new(0.5, 0));
        let soft = render_track_with_ara(&p, "trk", &config, &ara, &effects).expect("ara render");
        assert_eq!(soft.samples.len(), plain.samples.len());
        assert!((soft.peak() - plain.peak() * 0.5).abs() < 0.02, "peak halves");
        assert_ne!(soft.samples, plain.samples);
        // Unknown track still errors on the ARA path.
        assert!(matches!(
            render_track_with_ara(&p, "nope", &config, &ara, &effects),
            Err(BounceError::UnknownTrack(_))
        ));
        assert!(matches!(
            ara_document_for_track(&p, "nope", &config),
            Err(BounceError::UnknownTrack(_))
        ));
    }

    #[test]
    fn ara_note_edit_roundtrip_reaches_the_stem() {
        use crate::plugins::ara::MockAraEffect;
        use std::collections::HashMap;
        let p = tone_project(); // 4-beat tone clip = 2s at 8kHz
        let config = cfg();
        let ara = ara_document_for_track(&p, "trk", &config).expect("document");
        // Analyze the shared source audio over the clip's region: the
        // 440Hz reference tone measures as MIDI 69 in every note.
        let entry = ara.regions_for_clip("clip")[0].clone();
        let source = ara.get_source("clip").expect("source");
        let audio = source.read_range(
            entry.source_offset_sec,
            entry.source_offset_sec + entry.region.len_sec(),
        );
        let mut fx =
            MockAraEffect::analyze(&audio, config.sample_rate, entry.region.start_sec, entry.region.len_sec(), 4)
                .expect("analyze");
        assert_eq!(fx.notes.len(), 4);
        assert!(fx.notes.iter().all(|n| n.pitch_midi == 69));
        // Edit: mute the first two notes (the first half of the region).
        fx.set_note_gain("note-0", 0.0);
        fx.set_note_gain("note-1", 0.0);
        let mut effects = HashMap::new();
        effects.insert("clip".to_string(), fx);
        let stem = render_track_with_ara(&p, "trk", &config, &ara, &effects).expect("ara render");
        assert_eq!(stem.samples.len(), 16000);
        assert!(stem.samples[..8000].iter().all(|s| *s == 0.0));
        assert!(stem.samples[8000..].iter().any(|s| s.abs() > 0.1));
        // And the mix path applies the same edit while honoring mute.
        let mix = render_mix_with_ara(&p, &config, &ara, &effects).expect("ara mix");
        assert!(mix.samples[..8000].iter().all(|s| *s == 0.0));
        let mut muted = p.clone();
        muted.tracks[0].muted = true;
        assert!(render_mix_with_ara(&muted, &config, &ara, &effects).expect("mix").is_silent());
    }

    #[test]
    fn ara_effect_without_a_region_never_colors_the_bounce() {
        use crate::plugins::ara::{MockAraEffect, MusicalContext};
        use std::collections::HashMap;
        let p = tone_project();
        let config = cfg();
        // Empty document: no regions mapped, so the binding is unplaced.
        let ara = AraDocument::new(MusicalContext::from_project(&p));
        let mut effects = HashMap::new();
        effects.insert("clip".to_string(), MockAraEffect::new(0.0, 0));
        let plain = render_track(&p, "trk", &config).expect("render");
        let out = render_track_with_ara(&p, "trk", &config, &ara, &effects).expect("ara render");
        assert_eq!(out.samples, plain.samples);
    }

    #[test]
    fn sample_project_bounces_both_tracks() {
        let p = Project::sample(); // click track + MIDI sketch
        let config = BounceConfig::new(8000, 0.0, 12.0).expect("config");
        let stems = render_stems(&p, &config).expect("stems");
        assert_eq!(stems.len(), 2);
        assert!(stems.iter().all(|s| s.peak() > 0.0));
        let mix = render_mix(&p, &config).expect("mix");
        assert_eq!(mix.samples.len(), stems[0].samples.len());
        assert!(mix.peak() > 0.0);
    }
}
