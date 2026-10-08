//! Instrument patches for [`super::Composition`]: small, deterministic
//! synth voices that render one note to a buffer.
//!
//! Teaching note: each patch is a recipe of three parts: an *oscillator*
//! (sine partials, band-limited saws, a plucked delay line, FM), an
//! *envelope* that shapes loudness over the note, and a gentle *filter*
//! that keeps it from sounding buzzy. `render_note` renders the note plus
//! its release tail; the caller adds it into the part buffer at the note's
//! start. Noise comes from a seeded LCG, so a render is bit-identical
//! every time (export determinism).

use std::f32::consts::TAU;

/// One patch: id, what it sounds like, and the range it sits well in.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Patch {
    pub id: &'static str,
    pub description: &'static str,
    /// Comfortable MIDI range (inclusive); notes outside still render.
    pub range: (u8, u8),
    /// Seconds a note rings after its length ends.
    pub release_secs: f32,
}

pub const PATCHES: &[Patch] = &[
    Patch {
        id: "felt_piano",
        description: "soft felt piano: warm hammer, quick decay, intimate (melody, chords)",
        range: (36, 96),
        release_secs: 0.35,
    },
    Patch {
        id: "warm_pad",
        description: "slow detuned-saw pad, filtered and wide (sustained chords, beds)",
        range: (36, 84),
        release_secs: 1.4,
    },
    Patch {
        id: "strings",
        description: "bowed string section: medium attack, vibrato (long notes, swells)",
        range: (36, 96),
        release_secs: 0.6,
    },
    Patch {
        id: "soft_bass",
        description: "round sine/triangle bass (roots, pedal tones)",
        range: (24, 60),
        release_secs: 0.15,
    },
    Patch {
        id: "pizzicato",
        description: "plucked string (Karplus-Strong): short, woody (ostinatos, tension)",
        range: (36, 88),
        release_secs: 0.1,
    },
    Patch {
        id: "celesta",
        description: "FM bell/celesta: glassy, long decay (motifs, sparkle)",
        range: (60, 108),
        release_secs: 0.8,
    },
    Patch {
        id: "clock_tick",
        description: "dry wooden tick; pitch tints the click (pulse, ticking clock)",
        range: (60, 108),
        release_secs: 0.05,
    },
    Patch {
        id: "low_drum",
        description: "soft timpani-like thud; pitch sets the body (heartbeat, accents)",
        range: (28, 52),
        release_secs: 0.6,
    },
];

pub fn patch(id: &str) -> Option<&'static Patch> {
    PATCHES.iter().find(|p| p.id == id)
}

pub fn midi_hz(pitch: u8) -> f32 {
    440.0 * 2f32.powf((pitch as f32 - 69.0) / 12.0)
}

/// Seeded white noise in [-1, 1] (LCG; deterministic per seed).
struct Noise(u32);
impl Noise {
    fn next(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (self.0 >> 8) as f32 / (1u32 << 23) as f32 - 1.0
    }
}

/// One-pole low-pass coefficient for cutoff `hz`.
fn lp_coef(hz: f32, sr: f32) -> f32 {
    1.0 - (-TAU * hz.min(sr * 0.45) / sr).exp()
}

/// PolyBLEP residual (band-limits saw/square discontinuities).
fn poly_blep(t: f32, dt: f32) -> f32 {
    if t < dt {
        let t = t / dt;
        t + t - t * t - 1.0
    } else if t > 1.0 - dt {
        let t = (t - 1.0) / dt;
        t * t + t + t + 1.0
    } else {
        0.0
    }
}

/// Linear attack, exponential-ish release after `hold` secs. Returns gain.
fn ar_env(t: f32, attack: f32, hold: f32, release: f32) -> f32 {
    let a = if attack > 0.0 {
        (t / attack).min(1.0)
    } else {
        1.0
    };
    if t <= hold {
        a
    } else {
        let r = ((t - hold) / release.max(1e-4)).min(1.0);
        a * (1.0 - r) * (1.0 - r)
    }
}

/// Render one note of `patch`: `dur` seconds held, plus the patch's
/// release tail. `seed` varies noise per note (deterministically).
pub fn render_note(p: &Patch, pitch: u8, velocity: f32, dur: f32, sr: u32, seed: u32) -> Vec<f32> {
    let srf = sr as f32;
    let f = midi_hz(pitch);
    let v = velocity.clamp(0.0, 1.0);
    let total = dur + p.release_secs;
    let n = (total * srf).ceil() as usize + 1;
    let mut out = vec![0.0f32; n];
    let mut noise = Noise(seed.wrapping_mul(2_654_435_761).wrapping_add(pitch as u32));
    match p.id {
        "felt_piano" => {
            // Inharmonic partials, brighter with velocity, higher partials
            // decaying faster; a damper release when the key lifts.
            let b = 0.0004f32;
            let bright = 0.6 + 0.8 * v;
            let mut lp = 0.0f32;
            let c = lp_coef(1800.0 + 3500.0 * v, srf);
            for (i, o) in out.iter_mut().enumerate() {
                let t = i as f32 / srf;
                let mut s = 0.0;
                for k in 1..=8 {
                    let kf = k as f32;
                    let fk = f * kf * (1.0 + b * kf * kf).sqrt();
                    if fk > srf * 0.45 {
                        break;
                    }
                    let amp = bright.powf(kf - 1.0) / kf.powf(1.3);
                    let decay = (-(0.5 + 0.45 * kf) * t * (f / 262.0).sqrt()).exp();
                    s += amp * decay * (TAU * fk * t).sin();
                }
                let env = ar_env(t, 0.004, dur, p.release_secs);
                lp += c * (s - lp);
                *o = lp * env * (0.25 + 0.75 * v) * 0.45;
            }
        }
        "warm_pad" | "strings" => {
            let pad = p.id == "warm_pad";
            let detune = if pad {
                [0.993f32, 1.0, 1.007]
            } else {
                [0.997, 1.0, 1.003]
            };
            let mut phase = [0.0f32, 0.31, 0.67];
            let (attack, cutoff) = if pad {
                (0.7, 900.0 + 900.0 * v)
            } else {
                (0.22, 1600.0 + 1800.0 * v)
            };
            let (mut l1, mut l2) = (0.0f32, 0.0f32);
            let c = lp_coef(cutoff, srf);
            for (i, o) in out.iter_mut().enumerate() {
                let t = i as f32 / srf;
                let vib = if pad {
                    0.0
                } else {
                    0.004 * (TAU * 5.2 * t).sin() * ((t - 0.3) / 0.4).clamp(0.0, 1.0)
                };
                let mut s = 0.0;
                for (ph, d) in phase.iter_mut().zip(detune) {
                    let dt = f * d * (1.0 + vib) / srf;
                    s += 2.0 * *ph - 1.0 - poly_blep(*ph, dt);
                    *ph += dt;
                    if *ph >= 1.0 {
                        *ph -= 1.0;
                    }
                }
                l1 += c * (s / 3.0 - l1);
                l2 += c * (l1 - l2);
                let env = ar_env(t, attack, dur, p.release_secs);
                *o = l2 * env * (0.35 + 0.65 * v) * if pad { 0.3 } else { 0.32 };
            }
        }
        "soft_bass" => {
            for (i, o) in out.iter_mut().enumerate() {
                let t = i as f32 / srf;
                let ph = (f * t).fract();
                let tri = 4.0 * (ph - 0.5).abs() - 1.0;
                let s = (TAU * f * t).sin() + 0.25 * tri;
                let decay = 0.75 + 0.25 * (-t * 3.0).exp();
                let env = ar_env(t, 0.008, dur, p.release_secs) * decay;
                *o = s * env * (0.4 + 0.6 * v) * 0.42;
            }
        }
        "pizzicato" => {
            // Karplus-Strong: a noise burst circulating in a delay line
            // one period long, averaged each pass (string damping).
            let period = (srf / f).max(2.0);
            let len = period.floor() as usize;
            let frac = period - len as f32;
            let mut line: Vec<f32> = (0..len + 1).map(|_| noise.next()).collect();
            // soften the excitation (a finger, not a pick)
            for i in 1..line.len() {
                line[i] = 0.5 * (line[i] + line[i - 1]);
            }
            let damp = 0.994 - 0.004 * (f / 880.0).min(1.0);
            let mut idx = 0usize;
            let mut prev = 0.0f32;
            for (i, o) in out.iter_mut().enumerate() {
                let t = i as f32 / srf;
                let a = line[idx];
                let b = line[(idx + 1) % line.len()];
                let s = a + frac * (b - a);
                let fed = damp * 0.5 * (s + prev);
                prev = s;
                line[idx] = fed;
                idx = (idx + 1) % line.len();
                let env = ar_env(t, 0.0, dur, p.release_secs);
                *o = s * env * (0.3 + 0.7 * v) * 0.55;
            }
        }
        "celesta" => {
            // 2-op FM: modulator at 3.5x, index decaying fast (glassy
            // attack settling to a near-sine), long exponential body.
            for (i, o) in out.iter_mut().enumerate() {
                let t = i as f32 / srf;
                let index = (1.2 + 1.8 * v) * (-t * 6.0).exp();
                let m = index * (TAU * f * 3.5 * t).sin();
                let s =
                    (TAU * f * t + m).sin() + 0.2 * (TAU * f * 2.0 * t).sin() * (-t * 4.0).exp();
                let body = (-t * 1.6).exp();
                let env = ar_env(t, 0.002, dur, p.release_secs) * body;
                *o = s * env * (0.3 + 0.7 * v) * 0.3;
            }
        }
        "clock_tick" => {
            // Band-passed click + a short resonant ping tinted by pitch.
            let fr = f.min(4000.0);
            let mut bp_lo = 0.0f32;
            let mut bp_hi = 0.0f32;
            let c1 = lp_coef(fr * 2.0, srf);
            let c2 = lp_coef(fr * 0.5, srf);
            for (i, o) in out.iter_mut().enumerate() {
                let t = i as f32 / srf;
                let burst = if t < 0.004 { noise.next() } else { 0.0 };
                bp_lo += c1 * (burst - bp_lo);
                bp_hi += c2 * (bp_lo - bp_hi);
                let click = (bp_lo - bp_hi) * 3.0;
                let ping = (TAU * fr * t).sin() * (-t * 90.0).exp();
                *o = (click + 0.5 * ping) * (0.3 + 0.7 * v) * 0.5;
            }
        }
        "low_drum" => {
            let mut phase = 0.0f32;
            let mut lp = 0.0f32;
            let c = lp_coef(400.0, srf);
            for (i, o) in out.iter_mut().enumerate() {
                let t = i as f32 / srf;
                let sweep = f * (1.0 + 0.8 * (-t * 25.0).exp());
                phase += sweep / srf;
                let body = (TAU * phase).sin() * (-t * 4.5).exp();
                let thump = noise.next() * (-t * 60.0).exp();
                lp += c * (thump - lp);
                *o = (body + 0.6 * lp) * (0.3 + 0.7 * v) * 0.6;
            }
        }
        _ => {}
    }
    // De-click the very end (release tails land exactly on zero).
    let fade = ((0.003 * srf) as usize).min(out.len());
    let len = out.len();
    for k in 0..fade {
        out[len - 1 - k] *= k as f32 / fade as f32;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rms(s: &[f32]) -> f32 {
        (s.iter().map(|x| x * x).sum::<f32>() / s.len().max(1) as f32).sqrt()
    }

    #[test]
    fn every_patch_sounds_and_stays_in_range() {
        for p in PATCHES {
            let mid = ((p.range.0 as u16 + p.range.1 as u16) / 2) as u8;
            let s = render_note(p, mid, 0.8, 0.5, 48_000, 1);
            let peak = s.iter().fold(0.0f32, |m, x| m.max(x.abs()));
            assert!(rms(&s) > 1e-3, "{} is silent", p.id);
            assert!(peak < 1.0, "{} clips: {peak}", p.id);
            assert!(s.iter().all(|x| x.is_finite()), "{} NaN", p.id);
            assert_eq!(*s.last().unwrap(), 0.0, "{} tail ends on zero", p.id);
        }
    }

    #[test]
    fn notes_are_deterministic() {
        let p = patch("pizzicato").unwrap();
        assert_eq!(
            render_note(p, 60, 0.7, 0.3, 48_000, 9),
            render_note(p, 60, 0.7, 0.3, 48_000, 9)
        );
    }

    #[test]
    fn pitch_tracks_frequency() {
        // Zero crossings of a soft-bass A2 (110 Hz) over 1 s ≈ 220.
        let s = render_note(patch("soft_bass").unwrap(), 45, 0.8, 1.0, 48_000, 0);
        let body = &s[4_800..52_800];
        let zc = body
            .windows(2)
            .filter(|w| w[0] <= 0.0 && w[1] > 0.0)
            .count();
        assert!((105..=115).contains(&zc), "{zc} upward crossings");
    }
}
