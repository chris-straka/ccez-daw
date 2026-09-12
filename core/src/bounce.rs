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
//! loop-clean reference tone — because engine assets are opaque blobs with
//! no decoder yet. Real sample/MIDI decoding lands behind this same API.
//! Device chains contribute only `gain`/`volume` params; full DSP lands with
//! the device track. Pan is ignored: v1 stems are mono, the standard shape
//! for looped game deliverables.
//!
//! This module adds no IPC or project-schema surface (like `branch.rs`), so
//! the typegen drift gate (`bun run typegen -- --check`) is unaffected.

use crate::engine::Engine;
use crate::model::{Clip, Project};

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

/// Render one track's subtree (its clips x device gains x track volume).
/// Per-track stems ignore mute/solo: a stem is the track's raw material,
/// not its mixer state. The mix (`render_mix`) honors both.
pub fn render_track(project: &Project, track_id: &str, config: &BounceConfig) -> Result<Stem> {
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
        render_clip_into(clip, project.tempo, config, &mut samples);
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
            render_clip_into(clip, project.tempo, config, &mut buf);
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
    project
        .tracks
        .iter()
        .map(|t| render_track(project, &t.id, config))
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
    let stem = render_track(project, track_id, config)?;
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
fn render_clip_into(clip: &Clip, tempo: f64, config: &BounceConfig, out: &mut [f32]) {
    let sr = config.sample_rate as f64;
    let beats_per_sec = tempo / 60.0;
    let win_end = config.start_beat + config.length_beats;
    let clip_end = clip.start_beats + clip.length_beats;
    if clip_end <= config.start_beat || clip.start_beats >= win_end {
        return;
    }
    let is_click = clip.kind == crate::model::ClipKind::Audio && clip.source == "builtin:click";
    // Loop-clean reference tone: exactly 220 cycles per beat at any tempo,
    // so whole-beat windows wrap without a click (A440 at 120 BPM).
    let tone_hz = 220.0 * beats_per_sec;
    for (i, dst) in out.iter_mut().enumerate() {
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
        *dst += s;
    }
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
