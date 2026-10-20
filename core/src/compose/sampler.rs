//! Multi-sample instrument player: SFZ/SF2 regions in, stereo notes out.
//!
//! Teaching note: a sampled instrument is a set of *regions*, each one
//! recording mapped to a key range and a velocity range (soft, medium and
//! loud strikes sound different, not just quieter: *velocity layers*), and
//! sometimes several recordings of the same note taken in turn (*round
//! robins*) so repeated notes don't sound like a machine gun. Playing a
//! note picks the matching regions, repitches each recording from its root
//! key to the note (resampling with 4-point Hermite interpolation), shapes
//! it with an attack/decay/sustain/release envelope and sums them.
//!
//! Everything is deterministic: round robins advance per key in note
//! order and random-range regions draw from a seeded value, so an export
//! is bit-identical every time.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use super::wavread::{self, Audio};

/// Where a region's audio lives.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SampleRef {
    /// A WAV file.
    Wav(PathBuf),
    /// A slice of a SoundFont's 16-bit sample pool (frames, end exclusive).
    Sf2 {
        file: PathBuf,
        start: u32,
        end: u32,
        rate: u32,
        loop_start: u32,
        loop_end: u32,
    },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LoopMode {
    /// Play the recording once; note-off starts the release.
    NoLoop,
    /// Play the whole recording regardless of note length (drums).
    OneShot,
    /// Loop `loop_points` while the note sounds.
    Loop,
    /// SFZ's default (no `loop_mode`): loop if the recording carries its
    /// own loop (a WAV `smpl` chunk), else play once.
    FromFile,
}

/// Amplitude envelope, seconds (sustain is a level 0..1).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Env {
    pub attack: f32,
    pub hold: f32,
    pub decay: f32,
    pub sustain: f32,
    pub release: f32,
}

impl Default for Env {
    fn default() -> Self {
        Env {
            attack: 0.0,
            hold: 0.0,
            decay: 0.0,
            sustain: 1.0,
            release: 0.3,
        }
    }
}

/// One recording and where it sits on the keyboard.
#[derive(Debug, Clone, PartialEq)]
pub struct Region {
    pub sample: SampleRef,
    pub lokey: u8,
    pub hikey: u8,
    /// Root key the recording sounds at (fractional: SF2 pitch correction).
    pub keycenter: f32,
    /// Extra tuning in cents.
    pub tune: f32,
    /// Cents per key (100 = normal; 0 = same pitch on every key).
    pub keytrack: f32,
    pub lovel: u8,
    pub hivel: u8,
    /// Velocity cross-fade (xfin_lo, xfin_hi, xfout_lo, xfout_hi).
    pub xfade: Option<(u8, u8, u8, u8)>,
    pub volume_db: f32,
    /// -1 (left) ..= 1 (right).
    pub pan: f32,
    /// Round robin: this region plays on pass `seq_position` of `seq_length`.
    pub seq_length: u32,
    pub seq_position: u32,
    /// Random range [lorand, hirand).
    pub lorand: f32,
    pub hirand: f32,
    /// First frame played / end (exclusive) within the sample.
    pub offset: usize,
    pub end: Option<usize>,
    pub loop_mode: LoopMode,
    pub loop_points: Option<(usize, usize)>,
    pub env: Env,
    /// 0..1: how much velocity scales loudness (SFZ amp_veltrack / 100).
    pub veltrack: f32,
}

impl Region {
    pub fn new(sample: SampleRef) -> Self {
        Region {
            sample,
            lokey: 0,
            hikey: 127,
            keycenter: 60.0,
            tune: 0.0,
            keytrack: 100.0,
            lovel: 1,
            hivel: 127,
            xfade: None,
            volume_db: 0.0,
            pan: 0.0,
            seq_length: 1,
            seq_position: 1,
            lorand: 0.0,
            hirand: 1.0,
            offset: 0,
            end: None,
            loop_mode: LoopMode::NoLoop,
            loop_points: None,
            env: Env::default(),
            veltrack: 1.0,
        }
    }

    fn matches(&self, key: u8, vel: u8, pass: u32, rand: f32) -> bool {
        key >= self.lokey
            && key <= self.hikey
            && vel >= self.lovel
            && vel <= self.hivel
            && (self.seq_length <= 1 || pass % self.seq_length + 1 == self.seq_position)
            && rand >= self.lorand
            && rand < self.hirand
    }

    fn xfade_gain(&self, vel: u8) -> f32 {
        let Some((in_lo, in_hi, out_lo, out_hi)) = self.xfade else {
            return 1.0;
        };
        let v = vel as f32;
        let mut g = 1.0f32;
        if in_hi > in_lo && v < in_hi as f32 {
            g *= ((v - in_lo as f32) / (in_hi - in_lo) as f32).clamp(0.0, 1.0).sqrt();
        }
        if out_hi > out_lo && v > out_lo as f32 {
            g *= (1.0 - (v - out_lo as f32) / (out_hi - out_lo) as f32)
                .clamp(0.0, 1.0)
                .sqrt();
        }
        g
    }
}

/// How the composer plays an instrument's regions.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlayStyle {
    /// Sustained recordings without loop points (neither the region's
    /// nor the WAV's own `smpl` loop) get a cross-faded loop over 35-85%
    /// of the recording when a note outlasts it (strings, winds, choir).
    pub auto_loop: bool,
    /// Floor on the release time (s), so short library releases don't
    /// chop chords dry.
    pub min_release: f32,
    /// Hits ignore note length and ring out (drums, mallets).
    pub one_shot: bool,
}

impl Default for PlayStyle {
    fn default() -> Self {
        PlayStyle {
            auto_loop: false,
            min_release: 0.0,
            one_shot: false,
        }
    }
}

/// Regions plus the decoded recordings they use (loaded on first use).
pub struct SampledInstrument {
    pub regions: Vec<Region>,
    pub style: PlayStyle,
    cache: HashMap<SampleRef, Arc<Audio>>,
    /// Frames of near-silence before each recording starts (skipped so
    /// notes land on the beat; regions with an explicit offset keep it).
    lead_in: HashMap<SampleRef, usize>,
    /// Per-key round-robin pass counter.
    passes: HashMap<u8, u32>,
}

/// A rendered note: left and right at the output rate.
pub type Stereo = (Vec<f32>, Vec<f32>);

pub fn db_gain(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

/// Constant-power pan law, unity at centre: (left gain, right gain).
pub fn pan_gains(pan: f32) -> (f32, f32) {
    let a = (pan.clamp(-1.0, 1.0) + 1.0) * std::f32::consts::FRAC_PI_4;
    (
        std::f32::consts::SQRT_2 * a.cos(),
        std::f32::consts::SQRT_2 * a.sin(),
    )
}

/// An explicit loop `[start, end)` as the interpolator sees it: taps at or
/// past `end` read from `start` on, and once the loop has wrapped, taps
/// before `start` read the loop's tail (what precedes them on every
/// later pass).
#[derive(Clone, Copy)]
struct Wrap {
    start: usize,
    end: usize,
    wrapped: bool,
}

fn hermite(x: &[f32], pos: f64, wrap: Option<Wrap>) -> f32 {
    let i = pos.floor() as isize;
    let t = (pos - i as f64) as f32;
    let at = |mut k: isize| -> f32 {
        if let Some(w) = wrap {
            let (a, b) = (w.start as isize, w.end as isize);
            if k >= b {
                k -= b - a;
            } else if w.wrapped && k < a {
                k += b - a;
            }
        }
        if k < 0 || k as usize >= x.len() {
            0.0
        } else {
            x[k as usize]
        }
    };
    let (xm1, x0, x1, x2) = (at(i - 1), at(i), at(i + 1), at(i + 2));
    let c1 = 0.5 * (x1 - xm1);
    let c2 = xm1 - 2.5 * x0 + 2.0 * x1 - 0.5 * x2;
    let c3 = 0.5 * (x2 - xm1) + 1.5 * (x0 - x1);
    ((c3 * t + c2) * t + c1) * t + x0
}

impl SampledInstrument {
    pub fn new(regions: Vec<Region>, style: PlayStyle) -> Self {
        SampledInstrument {
            regions,
            style,
            cache: HashMap::new(),
            lead_in: HashMap::new(),
            passes: HashMap::new(),
        }
    }

    /// Every sample file the regions reference (for download checks).
    pub fn files(&self) -> Vec<PathBuf> {
        let mut out: Vec<PathBuf> = self
            .regions
            .iter()
            .map(|r| match &r.sample {
                SampleRef::Wav(p) => p.clone(),
                SampleRef::Sf2 { file, .. } => file.clone(),
            })
            .collect();
        out.sort();
        out.dedup();
        out
    }

    /// The keys that sound (lowest, highest) over every region.
    pub fn key_span(&self) -> Option<(u8, u8)> {
        let lo = self.regions.iter().map(|r| r.lokey).min()?;
        let hi = self.regions.iter().map(|r| r.hikey).max()?;
        Some((lo, hi))
    }

    fn audio(&mut self, s: &SampleRef) -> Result<Arc<Audio>, String> {
        if let Some(a) = self.cache.get(s) {
            return Ok(a.clone());
        }
        let a = Arc::new(match s {
            SampleRef::Wav(p) => wavread::read(p)?,
            SampleRef::Sf2 {
                file,
                start,
                end,
                rate,
                loop_start,
                loop_end,
            } => super::sf2::read_slice(file, *start, *end, *rate, *loop_start, *loop_end)?,
        });
        self.lead_in.insert(s.clone(), lead_in(&a));
        self.cache.insert(s.clone(), a.clone());
        Ok(a)
    }

    /// Render one note at `sr` Hz: `dur` seconds held, then the release.
    /// `rand` in [0, 1) picks among random-range regions. Returns silence
    /// (empty buffers) when no region covers the key and velocity.
    pub fn render_note(
        &mut self,
        pitch: u8,
        velocity: f32,
        dur: f32,
        sr: u32,
        rand: f32,
    ) -> Result<Stereo, String> {
        let vel = ((velocity.clamp(0.0, 1.0) * 127.0).round() as u8).max(1);
        let pass = {
            let p = self.passes.entry(pitch).or_insert(0);
            let v = *p;
            *p += 1;
            v
        };
        let picked: Vec<Region> = self
            .regions
            .iter()
            .filter(|r| r.matches(pitch, vel, pass, rand))
            .cloned()
            .collect();
        let mut out: Stereo = (Vec::new(), Vec::new());
        for r in &picked {
            let audio = self.audio(&r.sample)?;
            let mut r = r.clone();
            if r.offset == 0 {
                r.offset = self.lead_in[&r.sample];
            }
            let (l, rr) = render_region(&r, &audio, self.style, pitch, vel, dur, sr);
            if out.0.len() < l.len() {
                out.0.resize(l.len(), 0.0);
                out.1.resize(l.len(), 0.0);
            }
            for (o, s) in out.0.iter_mut().zip(&l) {
                *o += s;
            }
            for (o, s) in out.1.iter_mut().zip(&rr) {
                *o += s;
            }
        }
        Ok(out)
    }
}

/// Leading frames below -40 dB of the recording's peak, minus 2 ms of
/// headroom so the attack itself is never cut.
fn lead_in(a: &Audio) -> usize {
    let peak = a
        .channels
        .iter()
        .flat_map(|c| c.iter())
        .fold(0.0f32, |m, s| m.max(s.abs()));
    if peak <= 0.0 {
        return 0;
    }
    let floor = peak * 0.01;
    let first = (0..a.frames())
        .find(|&i| a.channels.iter().any(|c| c[i].abs() > floor))
        .unwrap_or(0);
    first.saturating_sub(a.sample_rate as usize / 500)
}

/// Render one region for one note.
fn render_region(
    r: &Region,
    audio: &Audio,
    style: PlayStyle,
    pitch: u8,
    vel: u8,
    dur: f32,
    sr: u32,
) -> Stereo {
    let n_src = r.end.unwrap_or(usize::MAX).min(audio.frames());
    let start = r.offset.min(n_src);
    if n_src <= start + 4 {
        return (Vec::new(), Vec::new());
    }
    let cents = (pitch as f32 - r.keycenter) * r.keytrack + r.tune;
    let step = 2f64.powf(cents as f64 / 1200.0) * audio.sample_rate as f64 / sr as f64;
    let one_shot = style.one_shot || r.loop_mode == LoopMode::OneShot;
    // SFZ defaults: unset loop points are the file's own loop, and an
    // unset loop mode loops when the file has one (hits still ring once).
    let loop_points = r.loop_points.or(audio.sample_loop);
    let loop_mode = match r.loop_mode {
        LoopMode::FromFile if audio.sample_loop.is_some() && !one_shot => LoopMode::Loop,
        LoopMode::FromFile => LoopMode::NoLoop,
        m => m,
    };
    let release = r.env.release.max(style.min_release).max(0.005);
    let vel_db = 40.0 * (vel as f32 / 127.0).log10() * r.veltrack;
    let gain = db_gain(r.volume_db + vel_db) * r.xfade_gain(vel);
    let (pl, pr) = pan_gains(r.pan);
    let held = (dur.max(0.0) * sr as f32) as usize;
    // Loop region: explicit points, else an auto cross-fade loop when a
    // sustained note outlasts its recording.
    let src_needed = (held as f64 + release as f64 * sr as f64) * step;
    let looping = match (loop_mode, loop_points) {
        (LoopMode::Loop, Some((a, b))) if b > a + 64 && b <= n_src => Some((a, b, 0usize)),
        _ if style.auto_loop && !one_shot && src_needed > (n_src - start) as f64 => {
            let len = n_src - start;
            let a = start + len * 35 / 100;
            let b = start + len * 85 / 100;
            let xf = ((0.15 * audio.sample_rate as f32) as usize).min((b - a) / 3);
            (b > a + 64).then_some((a, b, xf))
        }
        _ => None,
    };
    let rel_frames = (release * sr as f32) as usize;
    let max_frames = if one_shot || looping.is_none() {
        // Recording length at this pitch (one-shots ring out whole).
        let src_len = ((n_src - start) as f64 / step) as usize;
        if one_shot {
            src_len
        } else {
            src_len.min(held + rel_frames)
        }
    } else {
        held + rel_frames
    };
    let mut l = Vec::with_capacity(max_frames);
    let mut rt = Vec::with_capacity(max_frames);
    let (left, right) = (audio.left(), audio.right());
    let env = r.env;
    let srf = sr as f32;
    let rel_k = (-6.9 / (release * srf)).exp(); // -60 dB over `release`
    // Attack, hold, decay to sustain: the level `t` seconds into the note.
    let level = |t: f32| -> f32 {
        if env.attack > 0.0 && t < env.attack {
            t / env.attack
        } else if t < env.attack + env.hold {
            1.0
        } else if env.decay > 0.0 {
            let d = (t - env.attack - env.hold) / env.decay;
            env.sustain + (1.0 - env.sustain) * (-4.6 * d).exp()
        } else {
            env.sustain
        }
    };
    // The release fades from wherever the envelope was at note-off (mid-
    // attack included), not from a still-running attack or decay.
    let off_level = level(held as f32 / srf);
    let mut rel_level = 1.0f32;
    let mut pos = start as f64;
    for i in 0..max_frames {
        let g = if !one_shot && i >= held {
            rel_level *= rel_k;
            if rel_level < 1e-4 {
                break;
            }
            off_level * rel_level
        } else {
            level(i as f32 / srf)
        };
        // Read (with the loop cross-fade when inside it).
        let (sl, sr2) = match looping {
            Some((a, b, xf)) => {
                let len = (b - a) as f64;
                let mut p = pos;
                if p >= b as f64 {
                    p = a as f64 + (p - a as f64) % len;
                }
                // Explicit loops wrap their taps; the cross-faded auto loop
                // reads the real audio around its ends (the blend below
                // makes that join continuous).
                let wrap = (xf == 0).then_some(Wrap {
                    start: a,
                    end: b,
                    wrapped: pos >= b as f64,
                });
                let (mut sl, mut sr2) = (hermite(left, p, wrap), hermite(right, p, wrap));
                // Approaching the loop end, blend in the audio before the
                // loop start so the wrap is continuous.
                let to_end = b as f64 - p;
                if xf > 0 && pos >= a as f64 && to_end < xf as f64 && (p - len) >= 0.0 {
                    let w = (1.0 - to_end / xf as f64) as f32;
                    let (wa, wb) = ((1.0 - w).sqrt(), w.sqrt());
                    sl = sl * wa + hermite(left, p - len, None) * wb;
                    sr2 = sr2 * wa + hermite(right, p - len, None) * wb;
                }
                (sl, sr2)
            }
            None => {
                if pos >= n_src as f64 {
                    break;
                }
                (hermite(left, pos, None), hermite(right, pos, None))
            }
        };
        let gg = g * gain;
        l.push(sl * gg * pl);
        rt.push(sr2 * gg * pr);
        pos += step;
    }
    // Short fade where the recording was cut (never a click at the end).
    let fade = (sr as usize / 500).min(l.len());
    let n = l.len();
    for k in 0..fade {
        let w = k as f32 / fade as f32;
        l[n - 1 - k] *= w;
        rt[n - 1 - k] *= w;
    }
    (l, rt)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A sine "recording" at `hz` on disk: what the parsers point at.
    pub(crate) fn sine_wav(dir: &std::path::Path, name: &str, hz: f32, secs: f32, sr: u32) -> PathBuf {
        let n = (secs * sr as f32) as usize;
        let s: Vec<f32> = (0..n)
            .map(|i| 0.5 * (std::f32::consts::TAU * hz * i as f32 / sr as f32).sin())
            .collect();
        let p = dir.join(name);
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(&p, wavread::encode_pcm16(sr, &[s.clone(), s])).unwrap();
        p
    }

    fn zero_crossings_hz(x: &[f32], sr: u32) -> f32 {
        let c = x.windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count();
        c as f32 * sr as f32 / x.len() as f32
    }

    #[test]
    fn repitches_picks_velocity_layers_and_round_robins() {
        let dir = std::env::temp_dir().join(format!("ccez-sampler-{}", std::process::id()));
        let a4 = sine_wav(&dir, "a4.wav", 440.0, 1.0, 44_100);
        let soft = sine_wav(&dir, "soft.wav", 220.0, 1.0, 44_100);
        let mut loud = Region::new(SampleRef::Wav(a4.clone()));
        loud.keycenter = 69.0;
        loud.lovel = 64;
        let mut quiet = Region::new(SampleRef::Wav(soft));
        quiet.keycenter = 57.0;
        quiet.hivel = 63;
        let mut inst = SampledInstrument::new(vec![loud, quiet], PlayStyle::default());
        // A5 from the A4 recording: an octave up, resampled 44.1 -> 48 kHz.
        let (l, r) = inst.render_note(81, 0.9, 0.3, 48_000, 0.0).unwrap();
        assert_eq!(l.len(), r.len());
        let hz = zero_crossings_hz(&l[..12_000], 48_000);
        assert!((hz - 880.0).abs() < 15.0, "{hz}");
        // Soft velocity picks the other layer (220 Hz root at A3 = 57).
        let (l, _) = inst.render_note(57, 0.2, 0.3, 48_000, 0.0).unwrap();
        let hz = zero_crossings_hz(&l[..12_000], 48_000);
        assert!((hz - 220.0).abs() < 8.0, "{hz}");
        // Round robins alternate per key.
        let mut a = Region::new(SampleRef::Wav(a4.clone()));
        a.seq_length = 2;
        a.seq_position = 1;
        let mut b = a.clone();
        b.seq_position = 2;
        b.volume_db = -20.0;
        let mut rr = SampledInstrument::new(vec![a, b], PlayStyle::default());
        let peak = |s: &Stereo| s.0.iter().fold(0f32, |m, x| m.max(x.abs()));
        let p1 = peak(&rr.render_note(60, 0.8, 0.2, 48_000, 0.0).unwrap());
        let p2 = peak(&rr.render_note(60, 0.8, 0.2, 48_000, 0.0).unwrap());
        let p3 = peak(&rr.render_note(60, 0.8, 0.2, 48_000, 0.0).unwrap());
        assert!(p1 > p2 * 5.0 && (p1 - p3).abs() < 1e-6, "{p1} {p2} {p3}");
        // No region for this key: silence, not a tone.
        let mut narrow = Region::new(SampleRef::Wav(a4));
        narrow.lokey = 60;
        narrow.hikey = 60;
        let mut n = SampledInstrument::new(vec![narrow], PlayStyle::default());
        assert!(n.render_note(40, 0.8, 0.5, 48_000, 0.0).unwrap().0.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn leading_silence_is_skipped_so_notes_land_on_time() {
        let dir = std::env::temp_dir().join(format!("ccez-sampler-lead-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // 100 ms of silence, then a tone.
        let mut s = vec![0.0f32; 4_800];
        s.extend((0..9_600).map(|i| 0.5 * (i as f32 * 0.06).sin()));
        let p = dir.join("late.wav");
        std::fs::write(&p, wavread::encode_pcm16(48_000, &[s])).unwrap();
        let mut r = Region::new(SampleRef::Wav(p));
        r.keycenter = 60.0;
        let mut inst = SampledInstrument::new(vec![r], PlayStyle::default());
        let (l, _) = inst.render_note(60, 0.8, 0.1, 48_000, 0.0).unwrap();
        let first = l.iter().position(|x| x.abs() > 0.01).unwrap();
        assert!(first < 200, "sound starts {first} frames in (want < 200)");
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn peak(x: &[f32]) -> f32 {
        x.iter().fold(0f32, |m, s| m.max(s.abs()))
    }

    /// A note released mid-attack fades from where the attack got to; it
    /// never keeps swelling after note-off.
    #[test]
    fn release_mid_attack_decays_from_the_note_off_level() {
        let dir = std::env::temp_dir().join(format!("ccez-sampler-rel-{}", std::process::id()));
        let p = sine_wav(&dir, "s.wav", 440.0, 2.0, 48_000);
        let mut r = Region::new(SampleRef::Wav(p));
        r.keycenter = 69.0;
        r.env.attack = 1.0;
        r.env.release = 5.0;
        let mut inst = SampledInstrument::new(vec![r], PlayStyle::default());
        let (l, _) = inst.render_note(69, 1.0, 0.2, 48_000, 0.0).unwrap();
        let off = (0.2 * 48_000.0) as usize;
        let before = peak(&l[off - 480..off]);
        let after = peak(&l[off..]);
        assert!(after <= before * 1.01, "swelled after note-off: {before} -> {after}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// While an explicit loop plays, the interpolator's taps wrap from the
    /// loop end to the loop start: no click each pass, whether the loop
    /// ends at the last frame or audio follows it.
    #[test]
    fn interpolation_wraps_inside_explicit_loops() {
        let dir = std::env::temp_dir().join(format!("ccez-sampler-wrap-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // 100 cycles of a 1 kHz cosine (seamless over 0..4800), then
        // `trailer` frames of silence.
        for trailer in [0usize, 200] {
            let mut s: Vec<f32> = (0..4_800)
                .map(|i| 0.5 * (std::f32::consts::TAU * i as f32 / 48.0).cos())
                .collect();
            s.resize(4_800 + trailer, 0.0);
            let p = dir.join(format!("cos{trailer}.wav"));
            std::fs::write(&p, wavread::encode_pcm16(48_000, &[s])).unwrap();
            let mut r = Region::new(SampleRef::Wav(p));
            r.keycenter = 69.0;
            r.tune = 13.0; // fractional read positions
            r.loop_mode = LoopMode::Loop;
            r.loop_points = Some((0, 4_800));
            let mut inst = SampledInstrument::new(vec![r], PlayStyle::default());
            let (l, _) = inst.render_note(69, 1.0, 1.0, 48_000, 0.0).unwrap();
            // The cosine itself never steps more than 0.5 * 2π/48 ≈ 0.066.
            let held = &l[..48_000];
            let worst = held.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0f32, f32::max);
            assert!(worst < 0.07, "trailer {trailer}: step {worst} at the loop seam");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `wav` with a `smpl` chunk carrying one forward loop (end inclusive,
    /// as the spec stores it).
    fn with_smpl_loop(mut wav: Vec<u8>, start: u32, end_incl: u32) -> Vec<u8> {
        let mut body = vec![0u8; 36];
        body[28..32].copy_from_slice(&1u32.to_le_bytes());
        for v in [0u32, 0, start, end_incl, 0, 0] {
            body.extend_from_slice(&v.to_le_bytes());
        }
        wav.extend_from_slice(b"smpl");
        wav.extend_from_slice(&(body.len() as u32).to_le_bytes());
        wav.extend(body);
        let riff = (wav.len() - 8) as u32;
        wav[4..8].copy_from_slice(&riff.to_le_bytes());
        wav
    }

    /// SFZ defaults: with no `loop_mode`/`loop_start`/`loop_end`, a WAV's
    /// own `smpl` loop plays (`loop_continuous`), and it wins over the
    /// sustain style's 35-85% auto-loop. One-shot hits ignore it.
    #[test]
    fn a_files_own_loop_is_the_default() {
        let dir = std::env::temp_dir().join(format!("ccez-sampler-smpl-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // 50 ms of 1 kHz (seamless, looped by the file), then 50 ms of
        // silence the auto-loop would cycle through.
        let mut s: Vec<f32> = (0..2_400)
            .map(|i| 0.5 * (std::f32::consts::TAU * i as f32 / 48.0).cos())
            .collect();
        s.resize(4_800, 0.0);
        let p = dir.join("looped.wav");
        std::fs::write(&p, with_smpl_loop(wavread::encode_pcm16(48_000, &[s]), 0, 2_399)).unwrap();
        let regions = super::super::sfz::parse("<region> sample=looped.wav key=69", &dir).unwrap();
        assert_eq!(regions[0].loop_mode, LoopMode::FromFile);
        let sus = PlayStyle {
            auto_loop: true,
            min_release: 0.0,
            one_shot: false,
        };
        let mut inst = SampledInstrument::new(regions.clone(), sus);
        let (l, _) = inst.render_note(69, 1.0, 1.0, 48_000, 0.0).unwrap();
        assert!(l.len() >= 48_000, "{}", l.len());
        let quietest = l[4_800..45_600]
            .chunks(480)
            .map(|w| (w.iter().map(|x| x * x).sum::<f32>() / w.len() as f32).sqrt())
            .fold(f32::MAX, f32::min);
        assert!(quietest > 0.3, "the file loop sounds throughout (quietest 10 ms: {quietest})");
        // A hit rings its recording once: no loop.
        let ring = PlayStyle {
            one_shot: true,
            ..PlayStyle::default()
        };
        let mut hit = SampledInstrument::new(regions, ring);
        let (l, _) = hit.render_note(69, 1.0, 1.0, 48_000, 0.0).unwrap();
        assert!(peak(&l[2_500..]) < 1e-3, "one-shot plays the silence after the loop");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn long_notes_loop_and_pan_is_constant_power() {
        let dir = std::env::temp_dir().join(format!("ccez-sampler-loop-{}", std::process::id()));
        let p = sine_wav(&dir, "s.wav", 440.0, 1.0, 48_000);
        let mut r = Region::new(SampleRef::Wav(p));
        r.keycenter = 69.0;
        r.pan = 1.0;
        let style = PlayStyle {
            auto_loop: true,
            min_release: 0.1,
            one_shot: false,
        };
        let mut inst = SampledInstrument::new(vec![r], style);
        // A 3 s note from a 1 s recording keeps sounding to the end.
        let (l, rr) = inst.render_note(69, 0.8, 3.0, 48_000, 0.0).unwrap();
        assert!(l.len() >= 3 * 48_000, "{}", l.len());
        let late = &rr[2 * 48_000..2 * 48_000 + 4_800];
        let rms = (late.iter().map(|x| x * x).sum::<f32>() / late.len() as f32).sqrt();
        assert!(rms > 0.1, "late rms {rms}");
        assert!(l.iter().all(|x| x.abs() < 1e-6), "hard right pan leaves left silent");
        let (gl, gr) = pan_gains(0.0);
        assert!((gl - 1.0).abs() < 1e-6 && (gr - 1.0).abs() < 1e-6);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
