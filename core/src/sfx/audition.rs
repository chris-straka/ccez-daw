//! SFX audition renderer: deterministic offline "what does this trigger
//! sound like?" preview.
//!
//! The bank references v0 clips by string id, so there is no audio to play
//! back yet — audition synthesizes a deterministic stand-in tone per clip
//! (sine whose frequency hashes from the clip id) shaped by the voice:
//!
//! - `gain` scales the tone; resolved RTPC outputs whose `target_param` is
//!   `volume` (or `gain`) multiply it, and `pitch`/`transpose`/`detune`
//!   outputs add semitones on top of the humanized detune.
//! - `target_param` of `cutoff`/`filter`/`lowpass` applies a one-pole
//!   lowpass; the output value is re-normalized to 0–1 through the
//!   binding's own `[min, max]` (see [`ResolvedRtpc::normalized`]), so Hz
//!   ranges and unit ranges both work. Other targets are recorded on the
//!   voice but do not shape the stand-in.
//! - `delay_ms` renders as leading silence (timing-spread auditioning).
//!
//! Pure function of the voice: same voice, same samples, every run.

use super::runtime::{ResolvedRtpc, Voice};

/// Audition render settings.
#[derive(Debug, Clone, PartialEq)]
pub struct AuditionConfig {
    pub sample_rate: u32,
    pub seconds: f32,
}

impl Default for AuditionConfig {
    fn default() -> Self {
        Self {
            sample_rate: 44_100,
            seconds: 1.0,
        }
    }
}

/// FNV-1a 64-bit hash: deterministic clip-id -> tone mapping without tables.
fn clip_hash(clip_id: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8482_22_25;
    for b in clip_id.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// Base frequency for a clip id: 180–740 Hz, deterministic per id.
fn base_freq_hz(clip_id: &str) -> f64 {
    180.0 + (clip_hash(clip_id) % 561) as f64
}

fn is_volume_target(p: &str) -> bool {
    matches!(p, "volume" | "gain")
}

fn is_pitch_target(p: &str) -> bool {
    matches!(p, "pitch" | "transpose" | "detune")
}

fn is_filter_target(p: &str) -> bool {
    matches!(p, "cutoff" | "filter" | "lowpass")
}

/// Fold a voice's RTPC outputs into audition shaping:
/// (gain multiplier, extra semitones, filter openness 0–1 or None).
fn fold_rtpc(rtpc: &[ResolvedRtpc]) -> (f64, f64, Option<f64>) {
    let mut gain_mul = 1.0;
    let mut semi = 0.0;
    let mut openness: Option<f64> = None;
    for r in rtpc {
        if is_volume_target(&r.target_param) {
            gain_mul *= r.value;
        } else if is_pitch_target(&r.target_param) {
            semi += r.value;
        } else if is_filter_target(&r.target_param) {
            openness = Some(openness.unwrap_or(1.0).min(r.normalized()));
        }
    }
    (gain_mul, semi, openness)
}

/// Render one voice to mono samples (leading timing-spread silence plus a
/// shaped stand-in tone). Deterministic: no RNG, no I/O.
pub fn render_voice(voice: &Voice, sample_rate: u32, frames: usize) -> Vec<f32> {
    let mut out = vec![0.0f32; frames];
    if sample_rate == 0 || frames == 0 {
        return out;
    }
    let sr = sample_rate as f64;
    let (gain_mul, rtpc_semi, openness) = fold_rtpc(&voice.rtpc);
    let gain = (voice.gain * gain_mul).max(0.0);
    let total_semi = voice.pitch_semitones + rtpc_semi;
    let freq = base_freq_hz(&voice.clip_id) * 2f64.powf(total_semi / 12.0);
    let delay = ((voice.delay_ms.max(0.0) / 1000.0) * sr) as usize;
    // One-pole lowpass coefficient from openness: closed = dark (0.02),
    // open = transparent (1.0 = no filtering).
    let alpha = openness.map(|o| 0.02 + 0.98 * o.clamp(0.0, 1.0));
    let mut y = 0.0f64;
    for (t, s) in out.iter_mut().enumerate() {
        if t < delay {
            continue;
        }
        let x = (gain * (2.0 * std::f64::consts::PI * freq * (t - delay) as f64 / sr).sin()) as f64;
        if let Some(a) = alpha {
            y += a * (x - y);
            *s = y as f32;
        } else {
            *s = x as f32;
        }
    }
    out
}

/// Root-mean-square level, the audition "did it sound?" scalar.
pub fn rms(buf: &[f32]) -> f32 {
    if buf.is_empty() {
        return 0.0;
    }
    (buf.iter().map(|s| s * s).sum::<f32>() / buf.len() as f32).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game_audio::{GameStateSnapshot, RtpcBinding, SfxEvent, GAME_AUDIO_SCHEMA_VERSION};
    use crate::sfx::runtime::{SfxRuntime, TriggerOptions};

    fn bank() -> crate::game_audio::SfxBank {
        crate::game_audio::SfxBank {
            schema_version: GAME_AUDIO_SCHEMA_VERSION,
            id: "bank_aud".to_string(),
            name: "Aud".to_string(),
            events: vec![SfxEvent {
                id: "ev_hit".to_string(),
                name: "Hit".to_string(),
                clip_ids: vec!["clip_a".to_string(), "clip_b".to_string()],
                volume: 0.8,
                volume_random: 0.1,
                pitch_random: 2.0,
                cooldown_ms: 0,
                max_polyphony: 16,
                rtpc: vec![RtpcBinding {
                    param: "threat".to_string(),
                    target_node: "bus_sfx".to_string(),
                    target_param: "volume".to_string(),
                    min: 0.0,
                    max: 1.0,
                }],
            }],
        }
    }

    fn threat_declared() -> crate::game_audio::GameStateParam {
        crate::game_audio::GameStateParam {
            id: "threat".to_string(),
            label: "Threat".to_string(),
            min: 0.0,
            max: 1.0,
            default: 1.0,
            unit: String::new(),
        }
    }

    /// Full event-audition path: trigger from a bank, render the voice.
    /// This is the GA-2 validation test: an event must audition to
    /// deterministic, non-silent audio.
    #[test]
    fn event_audition_renders_deterministic_non_silent_audio() {
        let bank = bank();
        let declared = vec![threat_declared()];
        let snapshot = GameStateSnapshot {
            state: "combat".to_string(),
            values: vec![],
        };
        let opts = TriggerOptions::default();
        let cfg = AuditionConfig::default();
        let frames = (cfg.sample_rate as f32 * cfg.seconds) as usize;

        // Two runtimes, same seed: identical voices, identical renders.
        let mut a = SfxRuntime::new(2026);
        let mut b = SfxRuntime::new(2026);
        let va = a
            .trigger(&bank, "ev_hit", &snapshot, &declared, 0, &opts)
            .expect("accepted");
        let vb = b
            .trigger(&bank, "ev_hit", &snapshot, &declared, 0, &opts)
            .expect("accepted");
        let (ra, rb) = (
            render_voice(&va, cfg.sample_rate, frames),
            render_voice(&vb, cfg.sample_rate, frames),
        );
        assert_eq!(ra.len(), frames);
        assert_eq!(ra, rb, "audition must be bit-deterministic per seed");
        assert!(rms(&ra) > 0.01, "audition must be audible, rms={}", rms(&ra));

        // Humanization is audible: later triggers in the stream render
        // different (but still deterministic) buffers.
        let mut seen = std::collections::HashSet::new();
        for t in 1..8u64 {
            let v = a
                .trigger(&bank, "ev_hit", &snapshot, &declared, t, &opts)
                .expect("accepted");
            let r = render_voice(&v, cfg.sample_rate, frames);
            assert!(rms(&r) > 0.01);
            seen.insert(format!("{:?}", &r[..64]));
        }
        assert!(seen.len() > 1, "humanized triggers must sound different");
    }

    #[test]
    fn rtpc_volume_and_filter_shape_the_audition() {
        // Volume RTPC at 0.0 must silence what 1.0 makes loud.
        let voice = |volume_value: f64, cutoff: Option<f64>| Voice {
            event_id: "ev".to_string(),
            clip_id: "clip_a".to_string(),
            gain: 0.8,
            pitch_semitones: 0.0,
            delay_ms: 0.0,
            started_ms: 0,
            rtpc: {
                let mut r = vec![ResolvedRtpc {
                    target_node: "bus_sfx".to_string(),
                    target_param: "volume".to_string(),
                    value: volume_value,
                    min: 0.0,
                    max: 1.0,
                }];
                if let Some(c) = cutoff {
                    r.push(ResolvedRtpc {
                        target_node: "bus_sfx".to_string(),
                        target_param: "cutoff".to_string(),
                        value: c,
                        min: 0.0,
                        max: 1.0,
                    });
                }
                r
            },
        };
        let frames = 44_100;
        let loud = rms(&render_voice(&voice(1.0, None), 44_100, frames));
        let quiet = rms(&render_voice(&voice(0.25, None), 44_100, frames));
        assert!((loud - 4.0 * quiet).abs() < 1e-3, "{loud} vs {quiet}");
        assert_eq!(rms(&render_voice(&voice(0.0, None), 44_100, frames)), 0.0);

        // Closing the filter must darken (lower RMS of the raw tone).
        let open = rms(&render_voice(&voice(1.0, Some(1.0)), 44_100, frames));
        let closed = rms(&render_voice(&voice(1.0, Some(0.0)), 44_100, frames));
        assert!(closed < open * 0.5, "closed {closed} vs open {open}");
    }

    #[test]
    fn timing_spread_renders_as_leading_silence() {
        let mut v = Voice {
            event_id: "ev".to_string(),
            clip_id: "clip_a".to_string(),
            gain: 0.8,
            pitch_semitones: 0.0,
            delay_ms: 10.0, // 441 samples at 44.1kHz
            started_ms: 0,
            rtpc: vec![],
        };
        let r = render_voice(&v, 44_100, 1024);
        assert!(r[..441].iter().all(|&s| s == 0.0));
        assert!(rms(&r[441..]) > 0.01);
        v.delay_ms = 0.0;
        assert_ne!(r, render_voice(&v, 44_100, 1024));
    }
}
