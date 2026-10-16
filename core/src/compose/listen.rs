//! Ears for agents: what a piece of music sounds like, in numbers and
//! plain words, without a person listening.
//!
//! Teaching note: everything starts from a short-time Fourier transform
//! (2048-sample frames every 512 samples). From the frames:
//!
//! - **Tempo**: spectral flux (how much new energy each frame adds) is an
//!   onset curve; its autocorrelation peaks at the beat period. A prior
//!   centred on 110 BPM picks between a tempo and its double or half.
//! - **Key**: every bin's energy is folded onto the 12 pitch classes
//!   (chroma); the Krumhansl-Kessler key profiles that correlate best
//!   name the key.
//! - **Loudness**: EBU R128 integrated LUFS, a per-second short-term
//!   curve (does it build?) and the loudness range.
//! - **Timbre**: 13 MFCCs (the spectral envelope's shape, the way speech
//!   and music classifiers describe "what instrument / what colour"),
//!   brightness (spectral centroid), energy per band, stereo width.
//!
//! Two pieces are compared aspect by aspect (harmony, timbre, rhythm,
//! brightness, loudness) so an agent learns *how* a take differs from a
//! reference, not just a single distance. Compositions add what only the
//! notes can tell: density per layer, register clashes between parts,
//! notes outside the key or an instrument's range, and how much each
//! intensity layer changes the sound.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Serialize;

use super::render::{render_preview, render_stems, PlanStep, Rendered};
use super::theory::{self, Key};
use super::{Composition, Role, SAMPLE_RATE};
use crate::meter::lufs::analyse_offline;

const FRAME: usize = 2048;
const HOP: usize = 512;

// ------------------------------------------------------------- loading

/// Decoded stereo audio.
#[derive(Debug, Clone)]
pub struct Stereo {
    pub sample_rate: u32,
    pub left: Vec<f32>,
    pub right: Vec<f32>,
}

/// Read a WAV directly; anything else (mp3, ogg, flac, m4a) through
/// `ffmpeg` to 48 kHz stereo.
pub fn load_audio(path: &Path) -> Result<Stereo, String> {
    let is_wav = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("wav"));
    if is_wav {
        let a = super::wavread::read(path)?;
        return Ok(Stereo { sample_rate: a.sample_rate, left: a.left().to_vec(), right: a.right().to_vec() });
    }
    let tmp = std::env::temp_dir().join(format!(
        "ccez-listen-{}-{}.wav",
        std::process::id(),
        path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default()
    ));
    let st = std::process::Command::new("ffmpeg")
        .args(["-y", "-loglevel", "error", "-i"])
        .arg(path)
        .args(["-ac", "2", "-ar", "48000", "-c:a", "pcm_s16le"])
        .arg(&tmp)
        .status()
        .map_err(|e| format!("{}: reading non-WAV audio needs ffmpeg on PATH ({e})", path.display()))?;
    if !st.success() {
        return Err(format!("ffmpeg could not decode {}", path.display()));
    }
    let a = super::wavread::read(&tmp);
    let _ = std::fs::remove_file(&tmp);
    let a = a?;
    Ok(Stereo { sample_rate: a.sample_rate, left: a.left().to_vec(), right: a.right().to_vec() })
}

// ----------------------------------------------------------------- FFT

fn fft(re: &mut [f32], im: &mut [f32]) {
    let n = re.len();
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let ang = -std::f32::consts::TAU / len as f32;
        let (ws, wc) = ang.sin_cos();
        let mut k = 0;
        while k < n {
            let (mut wr, mut wi) = (1.0f32, 0.0f32);
            for m in 0..len / 2 {
                let (ar, ai) = (re[k + m], im[k + m]);
                let (b_r, b_i) = (re[k + m + len / 2], im[k + m + len / 2]);
                let (vr, vi) = (b_r * wr - b_i * wi, b_r * wi + b_i * wr);
                re[k + m] = ar + vr;
                im[k + m] = ai + vi;
                re[k + m + len / 2] = ar - vr;
                im[k + m + len / 2] = ai - vi;
                let t = wr * wc - wi * ws;
                wi = wr * ws + wi * wc;
                wr = t;
            }
            k += len;
        }
        len <<= 1;
    }
}

/// Magnitude spectra (bins 0..=FRAME/2) of Hann-windowed frames.
fn stft(x: &[f32]) -> Vec<Vec<f32>> {
    let win: Vec<f32> = (0..FRAME)
        .map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / FRAME as f32).cos())
        .collect();
    let mut out = Vec::new();
    let mut s = 0;
    let (mut re, mut im) = (vec![0.0f32; FRAME], vec![0.0f32; FRAME]);
    while s + FRAME <= x.len().max(FRAME) {
        for i in 0..FRAME {
            re[i] = x.get(s + i).copied().unwrap_or(0.0) * win[i];
            im[i] = 0.0;
        }
        fft(&mut re, &mut im);
        out.push((0..=FRAME / 2).map(|k| (re[k] * re[k] + im[k] * im[k]).sqrt() / (FRAME as f32 / 4.0)).collect());
        s += HOP;
        if x.len() < FRAME {
            break;
        }
    }
    out
}

// ------------------------------------------------------------ features

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TempoEstimate {
    pub bpm: f32,
    /// Peak strength over the average (1 = no beat found, 3+ = clear pulse).
    pub confidence: f32,
    /// Other likely tempi (BPM, relative strength 0..1).
    pub alternatives: Vec<(f32, f32)>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct KeyEstimate {
    pub key: String,
    /// Profile correlation, -1..1 (0.7+ = clearly tonal).
    pub confidence: f32,
    pub runner_up: String,
    /// Energy per pitch class C..B, max 1.
    pub chroma: [f32; 12],
}

/// What a piece of audio sounds like.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Features {
    pub duration_secs: f32,
    pub lufs: Option<f32>,
    /// Short-term (3 s) loudness every second: does it build or sit flat?
    pub loudness_curve: Vec<f32>,
    pub loudness_range_lu: f32,
    pub true_peak_dbtp: f32,
    pub tempo: TempoEstimate,
    pub key: KeyEstimate,
    /// Note/hit onsets per second (busy-ness).
    pub onsets_per_sec: f32,
    /// Spectral centroid (Hz): low = mellow/dark, high = bright.
    pub brightness_hz: f32,
    /// Share of energy per band (dB relative to the total).
    pub bands_db: BTreeMap<&'static str, f32>,
    /// Side/mid energy (0 = mono, ~0.3 wide, 1 = out of phase).
    pub stereo_width: f32,
    #[serde(skip)]
    pub mfcc_mean: Vec<f32>,
    #[serde(skip)]
    pub mfcc_std: Vec<f32>,
}

const BANDS: [(&str, f32, f32); 6] = [
    ("sub_<60", 20.0, 60.0),
    ("low_60-250", 60.0, 250.0),
    ("lowmid_250-1k", 250.0, 1000.0),
    ("mid_1k-4k", 1000.0, 4000.0),
    ("high_4k-10k", 4000.0, 10000.0),
    ("air_>10k", 10000.0, 20000.0),
];

fn mel(f: f32) -> f32 {
    2595.0 * (1.0 + f / 700.0).log10()
}

/// 40 triangular mel filters over 40 Hz..min(16 kHz, Nyquist).
fn mel_bank(sr: f32) -> Vec<Vec<(usize, f32)>> {
    let nb = FRAME / 2 + 1;
    let (lo, hi) = (mel(40.0), mel(16000.0f32.min(sr / 2.0)));
    let pts: Vec<f32> = (0..42).map(|i| lo + (hi - lo) * i as f32 / 41.0).collect();
    let hz: Vec<f32> = pts.iter().map(|m| 700.0 * (10f32.powf(m / 2595.0) - 1.0)).collect();
    (0..40)
        .map(|b| {
            let (a, c, d) = (hz[b], hz[b + 1], hz[b + 2]);
            (0..nb)
                .filter_map(|k| {
                    let f = k as f32 * sr / FRAME as f32;
                    let w = if f > a && f <= c {
                        (f - a) / (c - a)
                    } else if f > c && f < d {
                        (d - f) / (d - c)
                    } else {
                        0.0
                    };
                    (w > 0.0).then_some((k, w))
                })
                .collect()
        })
        .collect()
}

pub fn analyse(left: &[f32], right: &[f32], sr: u32) -> Features {
    let srf = sr as f32;
    let n = left.len().min(right.len());
    let mono: Vec<f32> = (0..n).map(|i| 0.5 * (left[i] + right[i])).collect();
    let (lufs, loudness_curve, lra) = analyse_offline(&[&left[..n], &right[..n]], srf);
    let spec = stft(&mono);
    let nb = FRAME / 2 + 1;
    let bin_hz = srf / FRAME as f32;
    // Onset curve: log-magnitude spectral flux.
    let mut flux = Vec::with_capacity(spec.len());
    let mut prev: Option<Vec<f32>> = None;
    let mut chroma = [0.0f64; 12];
    let mut band_e = [0.0f64; 6];
    let mut cent_num = 0.0f64;
    let mut cent_den = 0.0f64;
    let bank = mel_bank(srf);
    let mut mf: Vec<Vec<f32>> = Vec::new();
    let pc_of: Vec<Option<usize>> = (0..nb)
        .map(|k| {
            let f = k as f32 * bin_hz;
            (55.0..=5000.0).contains(&f).then(|| {
                let p = 69.0 + 12.0 * (f / 440.0).log2();
                (p.round() as i32).rem_euclid(12) as usize
            })
        })
        .collect();
    for frame in &spec {
        let lg: Vec<f32> = frame.iter().map(|m| (1.0 + 1000.0 * m).ln()).collect();
        let f = match &prev {
            Some(p) => lg.iter().zip(p).map(|(a, b)| (a - b).max(0.0)).sum::<f32>(),
            None => 0.0,
        };
        flux.push(f);
        prev = Some(lg);
        let energy: f32 = frame.iter().map(|m| m * m).sum();
        if energy < 1e-10 {
            continue;
        }
        let mut fc = [0.0f64; 12];
        for (k, m) in frame.iter().enumerate() {
            let e = (*m as f64) * (*m as f64);
            // Chroma from spectral peaks only, at their interpolated
            // frequency: the window's leakage into neighbouring bins
            // would otherwise smear low notes onto the semitones beside them.
            if pc_of[k].is_some() && k > 0 && k + 1 < frame.len() && *m > frame[k - 1] && *m >= frame[k + 1] {
                let (a, b, c) = ((frame[k - 1] + 1e-9).ln(), (m + 1e-9).ln(), (frame[k + 1] + 1e-9).ln());
                let d = 0.5 * (a - c) / (a - 2.0 * b + c).min(-1e-9);
                let f = (k as f32 + d.clamp(-0.5, 0.5)) * bin_hz;
                let pc = ((69.0 + 12.0 * (f / 440.0).log2()).round() as i32).rem_euclid(12) as usize;
                fc[pc] += e + (frame[k - 1] as f64).powi(2) + (frame[k + 1] as f64).powi(2);
            }
            let fhz = k as f32 * bin_hz;
            cent_num += fhz as f64 * e;
            cent_den += e;
            for (bi, (_, lo, hi)) in BANDS.iter().enumerate() {
                if fhz >= *lo && fhz < *hi {
                    band_e[bi] += e;
                }
            }
        }
        // Each frame's chroma normalised, weighted by its loudness (sqrt
        // so loud passages don't drown the rest).
        let fs: f64 = fc.iter().sum();
        if fs > 0.0 {
            let w = (energy as f64).sqrt();
            for i in 0..12 {
                chroma[i] += fc[i] / fs * w;
            }
        }
        let melv: Vec<f32> = bank
            .iter()
            .map(|filt| 10.0 * (filt.iter().map(|(k, w)| frame[*k] * frame[*k] * w).sum::<f32>() + 1e-10).log10())
            .collect();
        // DCT-II (orthonormal) -> 13 coefficients.
        let m = melv.len() as f32;
        let c: Vec<f32> = (0..13)
            .map(|q| {
                let s: f32 = melv
                    .iter()
                    .enumerate()
                    .map(|(i, v)| v * (std::f32::consts::PI * q as f32 * (i as f32 + 0.5) / m).cos())
                    .sum();
                s * (if q == 0 { (1.0 / m).sqrt() } else { (2.0 / m).sqrt() })
            })
            .collect();
        mf.push(c);
    }
    let fr = srf / HOP as f32;
    let tempo = tempo_from_flux(&flux, fr);
    let onsets = count_onsets(&flux, fr);
    let duration = n as f32 / srf;
    let cmax = chroma.iter().cloned().fold(0.0, f64::max).max(1e-12);
    let mut chroma_n = [0.0f32; 12];
    for i in 0..12 {
        chroma_n[i] = (chroma[i] / cmax) as f32;
    }
    let keys = theory::detect_key_from_weights(&chroma);
    let total_band: f64 = band_e.iter().sum::<f64>().max(1e-20);
    let bands_db = BANDS
        .iter()
        .zip(band_e)
        .map(|((name, _, _), e)| (*name, (10.0 * (e / total_band).max(1e-12).log10()) as f32))
        .collect();
    let (mut mid, mut side) = (0.0f64, 0.0f64);
    for i in 0..n {
        let m = 0.5 * (left[i] + right[i]) as f64;
        let s = 0.5 * (left[i] - right[i]) as f64;
        mid += m * m;
        side += s * s;
    }
    let (mfcc_mean, mfcc_std) = if mf.is_empty() {
        (vec![0.0; 13], vec![0.0; 13])
    } else {
        let k = mf.len() as f32;
        let mean: Vec<f32> = (0..13).map(|q| mf.iter().map(|c| c[q]).sum::<f32>() / k).collect();
        let std: Vec<f32> = (0..13)
            .map(|q| (mf.iter().map(|c| (c[q] - mean[q]).powi(2)).sum::<f32>() / k).sqrt())
            .collect();
        (mean, std)
    };
    Features {
        duration_secs: duration,
        lufs,
        loudness_curve,
        loudness_range_lu: lra,
        true_peak_dbtp: super::fx::true_peak_dbtp(&[&left[..n], &right[..n]]),
        tempo,
        key: KeyEstimate {
            key: keys.first().map(|k| k.0.name()).unwrap_or_default(),
            confidence: keys.first().map_or(0.0, |k| k.1 as f32),
            runner_up: keys.get(1).map(|k| k.0.name()).unwrap_or_default(),
            chroma: chroma_n,
        },
        onsets_per_sec: if duration > 0.0 { onsets as f32 / duration } else { 0.0 },
        brightness_hz: if cent_den > 0.0 { (cent_num / cent_den) as f32 } else { 0.0 },
        bands_db,
        stereo_width: if mid + side > 0.0 { (side / mid.max(1e-20)).min(1.0) as f32 } else { 0.0 },
        mfcc_mean,
        mfcc_std,
    }
}

/// Onset curve with its slow trend removed (half-wave rectified).
fn detrend(flux: &[f32], fr: f32) -> Vec<f32> {
    let w = (fr * 0.5) as usize;
    let n = flux.len();
    let mut pre = vec![0.0f64; n + 1];
    for i in 0..n {
        pre[i + 1] = pre[i] + flux[i] as f64;
    }
    (0..n)
        .map(|i| {
            let (a, b) = (i.saturating_sub(w), (i + w + 1).min(n));
            let m = (pre[b] - pre[a]) / (b - a) as f64;
            (flux[i] as f64 - m).max(0.0) as f32
        })
        .collect()
}

fn tempo_from_flux(flux: &[f32], fr: f32) -> TempoEstimate {
    let o = detrend(flux, fr);
    let n = o.len();
    let lag_of = |bpm: f32| fr * 60.0 / bpm;
    let (lmin, lmax) = (lag_of(240.0).floor() as usize, (lag_of(40.0).ceil() as usize).min(n.saturating_sub(1)));
    if n < 16 || lmax <= lmin + 2 {
        return TempoEstimate { bpm: 0.0, confidence: 0.0, alternatives: vec![] };
    }
    let ac: Vec<f32> = (0..=lmax + 1)
        .map(|l| {
            let s: f32 = (0..n - l).map(|i| o[i] * o[i + l]).sum();
            s / (n - l) as f32
        })
        .collect();
    // Comb over a beat's multiples (a real pulse repeats), times a prior
    // around 110 BPM, one octave wide.
    let score = |l: f32| -> f32 {
        let at = |x: f32| -> f32 {
            let i = x.floor() as usize;
            if i + 1 >= ac.len() {
                return 0.0;
            }
            let f = x - i as f32;
            ac[i] * (1.0 - f) + ac[i + 1] * f
        };
        let comb = at(l) + 0.5 * at(2.0 * l) + 0.33 * at(3.0 * l) + 0.25 * at(4.0 * l);
        let bpm = fr * 60.0 / l;
        let prior = (-0.5 * ((bpm / 110.0).log2() / 0.9).powi(2)).exp();
        comb * prior
    };
    let mut cands: Vec<(f32, f32)> = Vec::new();
    let mut l = lmin as f32;
    while l <= lmax as f32 {
        cands.push((l, score(l)));
        l += 0.25;
    }
    let mean = cands.iter().map(|c| c.1).sum::<f32>() / cands.len() as f32;
    // Local maxima, best first.
    let mut peaks: Vec<(f32, f32)> = (1..cands.len() - 1)
        .filter(|&i| cands[i].1 >= cands[i - 1].1 && cands[i].1 >= cands[i + 1].1)
        .map(|i| cands[i])
        .collect();
    peaks.sort_by(|a, b| b.1.total_cmp(&a.1));
    let Some(&(best_l, best_s)) = peaks.first() else {
        return TempoEstimate { bpm: 0.0, confidence: 0.0, alternatives: vec![] };
    };
    let bpm = |l: f32| (fr * 60.0 / l * 10.0).round() / 10.0;
    let mut alts: Vec<(f32, f32)> = Vec::new();
    for &(l, s) in peaks.iter().skip(1) {
        let b = bpm(l);
        if alts.len() >= 3 {
            break;
        }
        if (b - bpm(best_l)).abs() > 3.0 && alts.iter().all(|a| (a.0 - b).abs() > 3.0) {
            alts.push((b, s / best_s.max(1e-12)));
        }
    }
    TempoEstimate { bpm: bpm(best_l), confidence: if mean > 0.0 { best_s / mean } else { 0.0 }, alternatives: alts }
}

fn count_onsets(flux: &[f32], fr: f32) -> usize {
    let o = detrend(flux, fr);
    let n = o.len();
    if n < 3 {
        return 0;
    }
    let mean = o.iter().sum::<f32>() / n as f32;
    let sd = (o.iter().map(|v| (v - mean).powi(2)).sum::<f32>() / n as f32).sqrt();
    let thr = mean + 0.5 * sd;
    let gap = (0.06 * fr).max(1.0) as usize;
    let mut last: Option<usize> = None;
    let mut count = 0;
    for i in 1..n - 1 {
        if o[i] > thr && o[i] >= o[i - 1] && o[i] >= o[i + 1] && last.is_none_or(|l| i - l >= gap) {
            count += 1;
            last = Some(i);
        }
    }
    count
}

// ---------------------------------------------------------- comparing

/// How alike two pieces are, aspect by aspect (0 = nothing alike, 1 = same).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Comparison {
    pub overall: f32,
    pub harmony: f32,
    pub timbre: f32,
    pub rhythm: f32,
    pub brightness: f32,
    pub loudness: f32,
    /// Plain-language differences, biggest first.
    pub differences: Vec<String>,
}

fn centred_cos(a: &[f32], b: &[f32]) -> f32 {
    let ma = a.iter().sum::<f32>() / a.len() as f32;
    let mb = b.iter().sum::<f32>() / b.len() as f32;
    let (mut num, mut da, mut db) = (0.0, 0.0, 0.0);
    for (x, y) in a.iter().zip(b) {
        num += (x - ma) * (y - mb);
        da += (x - ma).powi(2);
        db += (y - mb).powi(2);
    }
    if da <= 0.0 || db <= 0.0 {
        return 0.0;
    }
    num / (da * db).sqrt()
}

fn ratio_sim(a: f32, b: f32, width: f32) -> f32 {
    if a <= 0.0 || b <= 0.0 {
        return if a == b { 1.0 } else { 0.0 };
    }
    (-(a / b).ln().abs() / width).exp()
}

/// Compare `b` against `a` (differences read "B is ... than A").
pub fn compare(a: &Features, b: &Features, a_name: &str, b_name: &str) -> Comparison {
    let harmony = ((centred_cos(&a.key.chroma, &b.key.chroma) + 1.0) / 2.0).clamp(0.0, 1.0);
    let d: f32 = a.mfcc_mean[1..]
        .iter()
        .zip(&b.mfcc_mean[1..])
        .map(|(x, y)| (x - y).powi(2))
        .sum::<f32>()
        .sqrt()
        / 12f32.sqrt();
    let ds: f32 = a.mfcc_std[1..]
        .iter()
        .zip(&b.mfcc_std[1..])
        .map(|(x, y)| (x - y).powi(2))
        .sum::<f32>()
        .sqrt()
        / 12f32.sqrt();
    let timbre = (-(d + 0.5 * ds) / 6.0).exp();
    let tempo_sim = [1.0f32, 2.0, 0.5]
        .iter()
        .map(|k| ratio_sim(a.tempo.bpm * k, b.tempo.bpm, 0.08))
        .fold(0.0, f32::max);
    let rhythm = 0.5 * tempo_sim + 0.5 * ratio_sim(a.onsets_per_sec.max(0.05), b.onsets_per_sec.max(0.05), 0.6);
    let brightness = ratio_sim(a.brightness_hz, b.brightness_hz, 0.5);
    let dl = match (a.lufs, b.lufs) {
        (Some(x), Some(y)) => y - x,
        _ => 0.0,
    };
    let loudness = (-(dl.abs()) / 6.0).exp() * (-((a.loudness_range_lu - b.loudness_range_lu).abs()) / 10.0).exp();
    let overall = 0.25 * harmony + 0.3 * timbre + 0.2 * rhythm + 0.1 * brightness + 0.15 * loudness;
    let mut diffs: Vec<(f32, String)> = Vec::new();
    if a.key.key != b.key.key {
        diffs.push((1.0 - harmony, format!("{b_name} reads as {}, {a_name} as {}", b.key.key, a.key.key)));
    }
    if tempo_sim < 0.8 {
        diffs.push((1.0 - tempo_sim, format!("{b_name} pulses at ~{:.0} BPM, {a_name} at ~{:.0}", b.tempo.bpm, a.tempo.bpm)));
    }
    let dens = b.onsets_per_sec / a.onsets_per_sec.max(0.05);
    if !(0.75..=1.33).contains(&dens) {
        diffs.push((
            dens.ln().abs(),
            format!(
                "{b_name} is {} ({:.1} vs {:.1} onsets/s)",
                if dens > 1.0 { "busier" } else { "sparser" },
                b.onsets_per_sec,
                a.onsets_per_sec
            ),
        ));
    }
    let br = b.brightness_hz / a.brightness_hz.max(1.0);
    if !(0.8..=1.25).contains(&br) {
        diffs.push((
            br.ln().abs(),
            format!(
                "{b_name} is {} (centroid {:.0} vs {:.0} Hz)",
                if br > 1.0 { "brighter" } else { "darker / mellower" },
                b.brightness_hz,
                a.brightness_hz
            ),
        ));
    }
    if dl.abs() >= 2.0 {
        diffs.push((dl.abs() / 6.0, format!("{b_name} is {:.1} LU {}", dl.abs(), if dl > 0.0 { "louder" } else { "quieter" })));
    }
    let dr = b.loudness_range_lu - a.loudness_range_lu;
    if dr.abs() >= 2.0 {
        diffs.push((
            dr.abs() / 6.0,
            format!(
                "{b_name} {} (loudness range {:.1} vs {:.1} LU)",
                if dr > 0.0 { "moves more between quiet and loud" } else { "holds one level more" },
                b.loudness_range_lu,
                a.loudness_range_lu
            ),
        ));
    }
    let dw = b.stereo_width - a.stereo_width;
    if dw.abs() >= 0.08 {
        diffs.push((dw.abs(), format!("{b_name} is {} ({:.2} vs {:.2})", if dw > 0.0 { "wider" } else { "narrower" }, b.stereo_width, a.stereo_width)));
    }
    if timbre < 0.6 {
        diffs.push((1.0 - timbre, format!("the instruments' colour differs (timbre {:.0}% alike)", timbre * 100.0)));
    }
    diffs.sort_by(|x, y| y.0.total_cmp(&x.0));
    Comparison {
        overall,
        harmony,
        timbre,
        rhythm,
        brightness,
        loudness,
        differences: diffs.into_iter().map(|d| d.1).collect(),
    }
}

/// One sentence describing a piece.
pub fn describe(f: &Features) -> String {
    let dens = match f.onsets_per_sec {
        x if x < 1.5 => "sparse",
        x if x < 3.5 => "moderately busy",
        x if x < 6.0 => "busy",
        _ => "very busy",
    };
    let colour = match f.brightness_hz {
        x if x < 900.0 => "dark",
        x if x < 1800.0 => "mellow",
        x if x < 3200.0 => "bright",
        _ => "very bright",
    };
    let keyc = if f.key.confidence >= 0.75 {
        "clearly"
    } else if f.key.confidence >= 0.55 {
        "probably"
    } else {
        "loosely"
    };
    let pulse = if f.tempo.confidence >= 2.0 { "a clear pulse" } else { "a soft pulse" };
    let shape = {
        let c = &f.loudness_curve;
        if c.len() >= 6 {
            let third = c.len() / 3;
            let a = c[..third].iter().sum::<f32>() / third as f32;
            let z = c[c.len() - third..].iter().sum::<f32>() / third as f32;
            if z - a > 3.0 {
                "builds over time"
            } else if a - z > 3.0 {
                "winds down"
            } else if f.loudness_range_lu < 3.0 {
                "holds one level"
            } else {
                "moves between quieter and louder passages"
            }
        } else {
            "is short"
        }
    };
    format!(
        "{:.0} s, {keyc} {} (runner-up {}), ~{:.0} BPM with {pulse}, {dens} ({:.1} onsets/s), {colour} (centroid {:.0} Hz), {} LUFS, peaks {:.1} dBTP, {}; it {shape} (range {:.1} LU)",
        f.duration_secs,
        f.key.key,
        f.key.runner_up,
        f.tempo.bpm,
        f.onsets_per_sec,
        f.brightness_hz,
        f.lufs.map_or("silent".to_string(), |l| format!("{l:.1}")),
        f.true_peak_dbtp,
        if f.stereo_width > 0.25 { "wide stereo" } else if f.stereo_width > 0.08 { "some stereo width" } else { "near mono" },
        f.loudness_range_lu
    )
}

// ------------------------------------------------------- compositions

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Clash {
    pub section: String,
    pub beat: f32,
    pub kind: &'static str,
    pub a: String,
    pub b: String,
    pub overlap_beats: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LayerReport {
    pub section: String,
    pub layer: String,
    pub min_intensity: u8,
    /// This layer alone (LUFS).
    pub lufs: Option<f32>,
    /// How much louder the section gets when this layer joins (LU).
    pub adds_lu: f32,
    pub notes_per_bar: f32,
    /// Lowest..highest MIDI note.
    pub register: Option<(u8, u8)>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SectionListen {
    pub section: String,
    pub intensity: u8,
    pub summary: String,
    pub features: Features,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CompositionListen {
    /// Plain-language overview, one line per finding.
    pub summary: Vec<String>,
    pub sections: Vec<SectionListen>,
    pub layers: Vec<LayerReport>,
    /// Section pairs at full intensity: (a, b, overall, harmony, rhythm).
    pub similarity: Vec<(String, String, f32, f32, f32)>,
    pub clashes: Vec<Clash>,
    pub clash_count: usize,
    pub out_of_key: Vec<String>,
    pub out_of_range: Vec<String>,
}

fn mix_of(c: &Composition, rendered: &Rendered, section: &str, intensity: u8, skip_layer: Option<&str>) -> Result<[Vec<f32>; 2], String> {
    if let Some(skip) = skip_layer {
        let mut r = rendered.clone();
        r.stems.retain(|s| !(s.section == section && s.layer == skip));
        let (mix, _) = render_preview(c, &r, &[PlanStep { section: section.into(), bars: None, intensity }])?;
        return Ok(mix);
    }
    let (mix, _) = render_preview(c, rendered, &[PlanStep { section: section.into(), bars: None, intensity }])?;
    Ok(mix)
}

/// Pairs of simultaneous notes from different instruments that rub
/// (minor 2nds/9ths in the middle and top) or muddy (close intervals low down).
pub fn clashes(c: &Composition) -> Vec<Clash> {
    let mut out = Vec::new();
    for s in &c.sections {
        let notes: Vec<(&str, &super::Note)> = c
            .parts
            .iter()
            .filter(|p| p.section == s.id)
            .flat_map(|p| p.notes.iter().map(move |n| (p.instrument.as_str(), n)))
            .collect();
        for i in 0..notes.len() {
            for j in i + 1..notes.len() {
                let ((ia, a), (ib, b)) = (notes[i], notes[j]);
                if ia == ib {
                    continue;
                }
                let ov = (a.start + a.length).min(b.start + b.length) - a.start.max(b.start);
                if ov < 0.5 {
                    continue;
                }
                let d = (a.pitch as i32 - b.pitch as i32).abs();
                let low = a.pitch.max(b.pitch) < 52;
                let kind = if low && (1..=4).contains(&d) {
                    "mud (close interval below E3)"
                } else if d == 1 || d == 13 {
                    "minor-second rub"
                } else {
                    continue;
                };
                out.push(Clash {
                    section: s.id.clone(),
                    beat: a.start.max(b.start),
                    kind,
                    a: format!("{} {}", ia, theory::pitch_name(a.pitch)),
                    b: format!("{} {}", ib, theory::pitch_name(b.pitch)),
                    overlap_beats: ov,
                });
            }
        }
    }
    out.sort_by(|x, y| y.overlap_beats.total_cmp(&x.overlap_beats));
    out
}

/// Listen to a composition: render it, then report each section at low
/// and full intensity, each layer's weight, section likeness, clashes,
/// and stray notes.
pub fn listen_composition(c: &Composition) -> Result<(CompositionListen, Rendered), String> {
    let rendered = render_stems(c)?;
    let mut sections = Vec::new();
    let mut feats: BTreeMap<String, Features> = BTreeMap::new();
    let mut summary = Vec::new();
    for s in &c.sections {
        let ints: Vec<u8> = if s.role == Role::Loop {
            let lo = c.layers.iter().map(|l| l.min_intensity).min().unwrap_or(1).max(1);
            if lo < super::MAX_INTENSITY { vec![lo, super::MAX_INTENSITY] } else { vec![lo] }
        } else {
            vec![super::MAX_INTENSITY]
        };
        for &i in &ints {
            let mix = mix_of(c, &rendered, &s.id, i, None)?;
            let f = analyse(&mix[0], &mix[1], SAMPLE_RATE);
            let line = describe(&f);
            sections.push(SectionListen { section: s.id.clone(), intensity: i, summary: line, features: f.clone() });
            if i == *ints.last().expect("one") {
                feats.insert(s.id.clone(), f);
            }
        }
        if ints.len() == 2 {
            let (a, b) = (&sections[sections.len() - 2].features, &sections[sections.len() - 1].features);
            let dl = b.lufs.unwrap_or(-70.0) - a.lufs.unwrap_or(-70.0);
            summary.push(format!(
                "{}: intensity {} -> {} adds {:+.1} LU, density {:.1} -> {:.1} onsets/s, brightness {:.0} -> {:.0} Hz{}",
                s.id,
                ints[0],
                ints[1],
                dl,
                a.onsets_per_sec,
                b.onsets_per_sec,
                a.brightness_hz,
                b.brightness_hz,
                if dl < 1.5 { " (a player may not notice the change)" } else { "" }
            ));
        }
    }
    // Layers: alone, and what they add.
    let mut layers = Vec::new();
    for st in &rendered.stems {
        let Some(layer) = c.layer(&st.layer) else { continue };
        let Some(sec) = c.section(&st.section) else { continue };
        let alone = analyse_offline(&[&st.left[..st.length_frames.min(st.frames())], &st.right[..st.length_frames.min(st.frames())]], SAMPLE_RATE as f32).0;
        let full = mix_of(c, &rendered, &st.section, super::MAX_INTENSITY, None)?;
        let without = mix_of(c, &rendered, &st.section, super::MAX_INTENSITY, Some(&st.layer))?;
        let lf = analyse_offline(&[&full[0], &full[1]], SAMPLE_RATE as f32).0;
        let lw = analyse_offline(&[&without[0], &without[1]], SAMPLE_RATE as f32).0;
        let notes: Vec<&super::Note> = c
            .parts
            .iter()
            .filter(|p| p.section == st.section && p.layer == st.layer)
            .flat_map(|p| p.notes.iter())
            .collect();
        layers.push(LayerReport {
            section: st.section.clone(),
            layer: st.layer.clone(),
            min_intensity: layer.min_intensity,
            lufs: alone,
            adds_lu: match (lf, lw) {
                (Some(f), Some(w)) => f - w,
                (Some(_), None) => 99.0,
                _ => 0.0,
            },
            notes_per_bar: notes.len() as f32 / sec.bars.max(1) as f32,
            register: notes.iter().map(|n| n.pitch).min().zip(notes.iter().map(|n| n.pitch).max()),
        });
    }
    for l in &layers {
        if l.adds_lu < 0.5 && l.adds_lu != 99.0 {
            summary.push(format!("layer {}/{} barely changes the sound (+{:.1} LU): louder, brighter or a new register would make it count", l.section, l.layer, l.adds_lu));
        }
    }
    // Section likeness at full intensity.
    let mut similarity = Vec::new();
    let ids: Vec<&String> = feats.keys().collect();
    for i in 0..ids.len() {
        for j in i + 1..ids.len() {
            let cmp = compare(&feats[ids[i]], &feats[ids[j]], ids[i], ids[j]);
            similarity.push((ids[i].clone(), ids[j].clone(), cmp.overall, cmp.harmony, cmp.rhythm));
            if cmp.overall > 0.9 {
                summary.push(format!("{} and {} sound nearly the same ({:.0}% alike)", ids[i], ids[j], cmp.overall * 100.0));
            }
        }
    }
    let all_clashes = clashes(c);
    let clash_count = all_clashes.len();
    if clash_count > 0 {
        summary.push(format!(
            "{clash_count} held note pair(s) rub or muddy; worst: {} beat {}: {} vs {} ({})",
            all_clashes[0].section, all_clashes[0].beat, all_clashes[0].a, all_clashes[0].b, all_clashes[0].kind
        ));
    }
    // Notes outside the written key / an instrument's range.
    let mut out_of_key = Vec::new();
    if let Ok(k) = Key::parse(&c.key) {
        let mut by: BTreeMap<String, usize> = BTreeMap::new();
        for p in &c.parts {
            if super::patch_kind(&c.instrument(&p.instrument).map(|i| i.patch.clone()).unwrap_or_default())
                .is_some_and(|pk| matches!(pk, super::PatchKind::Sampled(e) if matches!(e.family, "percussion" | "hand_drums")))
            {
                continue;
            }
            let n = p.notes.iter().filter(|n| !k.contains(n.pitch)).count();
            if n > 0 {
                *by.entry(format!("{}/{}/{}", p.section, p.layer, p.instrument)).or_default() += n;
            }
        }
        out_of_key = by.into_iter().map(|(k2, n)| format!("{k2}: {n} note(s) outside {}", k.name())).collect();
    }
    let mut out_of_range = Vec::new();
    for p in &c.parts {
        if let Some((lo, hi)) = super::arrange::range_of(c, &p.instrument) {
            let n = p.notes.iter().filter(|n| n.pitch < lo || n.pitch > hi).count();
            if n > 0 {
                out_of_range.push(format!("{}/{}/{}: {n} note(s) outside {lo}..={hi}", p.section, p.layer, p.instrument));
            }
        }
    }
    if !out_of_range.is_empty() {
        summary.push(format!("{} part(s) leave their instrument's range (may be silent)", out_of_range.len()));
    }
    if let Some(first) = sections.iter().find(|s| s.intensity == super::MAX_INTENSITY) {
        summary.insert(0, format!("{} at full intensity: {}", first.section, first.summary));
    }
    Ok((
        CompositionListen { summary, sections, layers, similarity, clashes: all_clashes.into_iter().take(12).collect(), clash_count, out_of_key, out_of_range },
        rendered,
    ))
}

// ---------------------------------------------------------- spectrogram

/// A log-frequency spectrogram (40 Hz..16 kHz, -90..0 dB) as a JPEG, so an
/// agent that reads images can *see* density, register and build-ups.
pub fn spectrogram_jpeg(left: &[f32], right: &[f32], sr: u32, out: &Path) -> Result<(), String> {
    let (w, h) = (1200usize, 360usize);
    let n = left.len().min(right.len());
    let mono: Vec<f32> = (0..n).map(|i| 0.5 * (left[i] + right[i])).collect();
    let spec = stft(&mono);
    if spec.is_empty() {
        return Err("spectrogram: no audio".into());
    }
    let srf = sr as f32;
    let (flo, fhi) = (40.0f32, 16000.0f32.min(srf / 2.0));
    let mut rgb = vec![0u8; w * h * 3];
    let colour = |t: f32| -> [u8; 3] {
        // black -> purple -> orange -> pale yellow
        let stops = [(0.0, [0.0, 0.0, 4.0]), (0.35, [87.0, 16.0, 110.0]), (0.65, [227.0, 89.0, 51.0]), (1.0, [252.0, 255.0, 164.0])];
        let t = t.clamp(0.0, 1.0);
        for k in 1..stops.len() {
            if t <= stops[k].0 {
                let (a, b) = (stops[k - 1], stops[k]);
                let f = (t - a.0) / (b.0 - a.0);
                return [0, 1, 2].map(|c| (a.1[c] + (b.1[c] - a.1[c]) * f) as u8);
            }
        }
        [252, 255, 164]
    };
    for x in 0..w {
        let fi = (x * spec.len() / w).min(spec.len() - 1);
        let fj = (((x + 1) * spec.len() / w).max(fi + 1)).min(spec.len());
        for y in 0..h {
            let f0 = flo * (fhi / flo).powf((h - 1 - y) as f32 / h as f32);
            let f1 = flo * (fhi / flo).powf((h - y) as f32 / h as f32);
            let (k0, k1) = ((f0 * FRAME as f32 / srf) as usize, ((f1 * FRAME as f32 / srf) as usize).max((f0 * FRAME as f32 / srf) as usize + 1));
            let mut m = 0.0f32;
            for frame in &spec[fi..fj] {
                for k in k0..k1.min(frame.len()) {
                    m = m.max(frame[k]);
                }
            }
            let dbv = 20.0 * m.max(1e-9).log10();
            let c = colour((dbv + 90.0) / 90.0);
            let o = (y * w + x) * 3;
            rgb[o..o + 3].copy_from_slice(&c);
        }
    }
    // Faint octave lines at each C (C2..C8) to read register.
    for oct in 2..=8 {
        let f = 440.0 * 2f32.powf((12.0 * (oct as f32 + 1.0) - 69.0) / 12.0);
        if f < flo || f > fhi {
            continue;
        }
        let y = h - 1 - (((f / flo).ln() / (fhi / flo).ln()) * h as f32) as usize;
        for x in (0..w).step_by(4) {
            let o = (y.min(h - 1) * w + x) * 3;
            rgb[o..o + 3].copy_from_slice(&[90, 90, 90]);
        }
    }
    let mut bytes = Vec::new();
    let enc = jpeg_encoder::Encoder::new(&mut bytes, 85);
    enc.encode(&rgb, w as u16, h as u16, jpeg_encoder::ColorType::Rgb).map_err(|e| e.to_string())?;
    if let Some(p) = out.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(p).map_err(|e| format!("{}: {e}", p.display()))?;
    }
    std::fs::write(out, bytes).map_err(|e| format!("{}: {e}", out.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Plucked notes of a D-major arpeggio at `bpm`, one per beat.
    fn arpeggio(bpm: f32, secs: f32, bright: bool) -> Vec<f32> {
        let sr = 48_000.0;
        let n = (secs * sr) as usize;
        let beat = (60.0 / bpm * sr) as usize;
        let pitches = [62, 66, 69, 74, 69, 66];
        let mut x = vec![0.0f32; n];
        for (k, start) in (0..n).step_by(beat).enumerate() {
            let f = 440.0 * 2f32.powf((pitches[k % pitches.len()] as f32 - 69.0) / 12.0);
            for i in 0..(beat * 2).min(n - start) {
                let t = i as f32 / sr;
                let env = (-t * 4.0).exp();
                let mut v = (std::f32::consts::TAU * f * t).sin();
                if bright {
                    for h in 2..8 {
                        v += (std::f32::consts::TAU * f * h as f32 * t).sin() / h as f32;
                    }
                }
                x[start + i] += 0.25 * env * v;
            }
        }
        x
    }

    #[test]
    fn hears_tempo_key_and_density() {
        let x = arpeggio(100.0, 12.0, false);
        let f = analyse(&x, &x, 48_000);
        assert!((f.tempo.bpm - 100.0).abs() < 3.0, "{:?}", f.tempo);
        assert_eq!(f.key.key, "D major", "{:?}", f.key);
        assert!((f.onsets_per_sec - 100.0 / 60.0).abs() < 0.5, "{}", f.onsets_per_sec);
        assert!(f.stereo_width < 0.01);
        assert!(f.lufs.is_some());
        assert!(describe(&f).contains("D major"));
    }

    #[test]
    fn comparison_names_what_differs() {
        let a = arpeggio(100.0, 10.0, false);
        let b = arpeggio(100.0, 10.0, true);
        let c = arpeggio(150.0, 10.0, false);
        let fa = analyse(&a, &a, 48_000);
        let fb = analyse(&b, &b, 48_000);
        let fc = analyse(&c, &c, 48_000);
        let same = compare(&fa, &fa, "A", "A");
        assert!(same.overall > 0.99, "{same:?}");
        let ab = compare(&fa, &fb, "A", "B");
        assert!(ab.brightness < 0.9 && ab.harmony > 0.8, "{ab:?}");
        assert!(ab.differences.iter().any(|d| d.contains("brighter")), "{ab:?}");
        let ac = compare(&fa, &fc, "A", "C");
        assert!(ac.rhythm < same.rhythm, "{ac:?}");
        assert!(ac.differences.iter().any(|d| d.contains("BPM") || d.contains("busier")), "{ac:?}");
    }

    #[test]
    fn listens_to_a_composition_and_finds_clashes() {
        let mut c = super::super::tests::tiny();
        c.key = "D major".into(); // the bed's F4 is outside it
        // A held E4 against the bed's F4 (beats 6..8) is a minor-second rub.
        c.parts[2].notes.push(super::super::Note { pitch: 64, start: 6.0, length: 2.0, velocity: 0.5 });
        let (l, _) = listen_composition(&c).unwrap();
        assert_eq!(l.clash_count, 1, "{:?}", l.clashes);
        assert!(l.clashes[0].kind.contains("rub"));
        // calm at intensity 1 and 5, intro and outro once each.
        assert_eq!(l.sections.len(), 4);
        let pulse = l.layers.iter().find(|x| x.layer == "pulse").unwrap();
        assert!(pulse.adds_lu > 0.0);
        assert!(l.summary.iter().any(|s| s.contains("calm: intensity 1 -> 5")), "{:?}", l.summary);
        assert!(l.out_of_key.iter().any(|s| s.contains("outside D major")), "{:?}", l.out_of_key);
    }

    #[test]
    fn spectrogram_writes_a_jpeg() {
        let x = arpeggio(100.0, 3.0, true);
        let out = std::env::temp_dir().join(format!("ccez-spec-{}.jpg", std::process::id()));
        spectrogram_jpeg(&x, &x, 48_000, &out).unwrap();
        let b = std::fs::read(&out).unwrap();
        assert_eq!(&b[..2], &[0xFF, 0xD8]);
        let _ = std::fs::remove_file(out);
    }
}
