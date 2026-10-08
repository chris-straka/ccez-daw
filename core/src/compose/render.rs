//! Render a [`Composition`] to stems and to a preview mixdown.
//!
//! Teaching note: a loop stem must join end-to-start without a click or a
//! gap in the reverb. The trick: render the section once with its full
//! release/reverb tail (`len + tail` samples), then *fold* the tail back
//! onto the start (`stem[i] += buf[len + i]`). That is exactly what a
//! listener hears on the second and every later pass, when the previous
//! pass is still ringing. Intros and outros play once, so they keep their
//! tail (trimmed where it falls silent) and ring out.

use super::synth::{patch, render_note};
use super::{Composition, LayerDef, Role, SectionDef, SAMPLE_RATE};
use crate::dsp::reverb::Reverb;

/// Seconds of tail rendered past a section (releases + reverb).
pub const TAIL_SECS: f32 = 4.0;

/// One rendered stem.
#[derive(Debug, Clone, PartialEq)]
pub struct RenderedStem {
    pub section: String,
    pub layer: String,
    pub role: Role,
    pub min_intensity: u8,
    /// Mono, `SAMPLE_RATE`.
    pub samples: Vec<f32>,
    /// Musical length in frames (loops: `samples.len()`; once: the hand-
    /// off point, before the ring-out tail).
    pub length_frames: usize,
}

pub fn db_to_gain(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

fn beats_to_frames(c: &Composition, beats: f64) -> usize {
    (beats * c.secs_per_beat() * SAMPLE_RATE as f64).round() as usize
}

/// Dry + reverb render of one (section, layer): `len + tail` frames.
fn render_pair(c: &Composition, sec: &SectionDef, layer: &LayerDef) -> (Vec<f32>, usize) {
    let len = beats_to_frames(c, c.section_beats(sec) as f64);
    let tail = (TAIL_SECS * SAMPLE_RATE as f32) as usize;
    let mut dry = vec![0.0f32; len + tail];
    let mut send = vec![0.0f32; len + tail];
    for (pi, part) in c
        .parts
        .iter()
        .enumerate()
        .filter(|(_, p)| p.section == sec.id && p.layer == layer.id)
    {
        let Some(inst) = c.instrument(&part.instrument) else {
            continue;
        };
        let Some(p) = patch(&inst.patch) else {
            continue;
        };
        let g = db_to_gain(inst.gain_db);
        for (ni, n) in part.notes.iter().enumerate() {
            let start = beats_to_frames(c, n.start as f64);
            let dur = (n.length as f64 * c.secs_per_beat()) as f32;
            let seed = (pi as u32) << 16 | ni as u32;
            let note = render_note(p, n.pitch, n.velocity, dur, SAMPLE_RATE, seed);
            for (k, s) in note.iter().enumerate() {
                let Some(d) = dry.get_mut(start + k) else {
                    break;
                };
                *d += s * g * (1.0 - 0.5 * inst.reverb);
                send[start + k] += s * g * inst.reverb;
            }
        }
    }
    if send.iter().any(|s| *s != 0.0) {
        let mut verb = Reverb::new(SAMPLE_RATE as f32);
        verb.apply_param("size", 0.78).expect("reverb size");
        verb.apply_param("damping", 0.55).expect("reverb damping");
        verb.apply_param("mix", 1.0).expect("reverb mix");
        let mut wet = vec![0.0f32; send.len()];
        verb.process(&send, &mut wet);
        for (d, w) in dry.iter_mut().zip(&wet) {
            *d += w * 0.8;
        }
    }
    (dry, len)
}

/// Fold everything past `len` back onto the loop (seamless loop stem).
pub fn fold_tail(buf: &[f32], len: usize) -> Vec<f32> {
    let mut out = vec![0.0f32; len];
    for (i, s) in buf.iter().enumerate() {
        out[i % len] += s;
    }
    out
}

/// Trim trailing near-silence (below -80 dBFS), keeping at least `min`.
fn trim_tail(mut buf: Vec<f32>, min: usize) -> Vec<f32> {
    let floor = 1e-4;
    let end = buf
        .iter()
        .rposition(|s| s.abs() > floor)
        .map(|i| i + 1)
        .unwrap_or(0);
    buf.truncate(end.max(min).min(buf.len()));
    let fade = (SAMPLE_RATE as usize / 200).min(buf.len().saturating_sub(min));
    let n = buf.len();
    for k in 0..fade {
        buf[n - 1 - k] *= k as f32 / fade as f32;
    }
    buf
}

/// Render every stem (one per (section, layer) with notes), with one
/// shared trim so the loudest full-intensity section peaks near -1 dBFS
/// once audioforge sums the layers. Returns the stems and the trim (dB).
pub fn render_stems(c: &Composition) -> (Vec<RenderedStem>, f32) {
    let mut stems: Vec<RenderedStem> = c
        .stem_pairs()
        .into_iter()
        .map(|(sec, layer)| {
            let (buf, len) = render_pair(c, sec, layer);
            let samples = match sec.role {
                Role::Loop => fold_tail(&buf, len),
                Role::Intro | Role::Outro => trim_tail(buf, len),
            };
            RenderedStem {
                section: sec.id.clone(),
                layer: layer.id.clone(),
                role: sec.role,
                min_intensity: layer.min_intensity,
                samples,
                length_frames: len,
            }
        })
        .collect();
    // Worst-case peak: every layer of a section at full intensity, summed
    // with its layer gain (what audioforge plays at intensity 5).
    let mut worst = 0.0f32;
    for sec in &c.sections {
        let mut sum: Vec<f32> = Vec::new();
        for st in stems.iter().filter(|s| s.section == sec.id) {
            let g = c
                .layer(&st.layer)
                .map(|l| db_to_gain(l.gain_db))
                .unwrap_or(1.0);
            if sum.len() < st.samples.len() {
                sum.resize(st.samples.len(), 0.0);
            }
            for (a, b) in sum.iter_mut().zip(&st.samples) {
                *a += b * g;
            }
        }
        worst = worst.max(sum.iter().fold(0.0f32, |m, s| m.max(s.abs())));
    }
    let target = db_to_gain(-1.0);
    let trim = if worst > 0.0 { target / worst } else { 1.0 };
    for st in &mut stems {
        for s in &mut st.samples {
            *s *= trim;
        }
    }
    (stems, 20.0 * trim.log10())
}

/// One step of a preview: play `section` for `bars` (default: its own
/// length; intros/outros always play whole) at `intensity`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PlanStep {
    pub section: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bars: Option<u32>,
    #[serde(default = "one")]
    pub intensity: u8,
}

fn one() -> u8 {
    1
}

/// The default preview: intro (if any), each loop at intensity 1 then 5,
/// then the outro (if any).
pub fn default_plan(c: &Composition) -> Vec<PlanStep> {
    let mut plan = Vec::new();
    let step = |s: &SectionDef, i: u8| PlanStep {
        section: s.id.clone(),
        bars: None,
        intensity: i,
    };
    if let Some(intro) = c.sections.iter().find(|s| s.role == Role::Intro) {
        plan.push(step(intro, 1));
    }
    for s in c.sections.iter().filter(|s| s.role == Role::Loop) {
        plan.push(step(s, 1));
        plan.push(step(s, super::MAX_INTENSITY));
    }
    if let Some(outro) = c.sections.iter().find(|s| s.role == Role::Outro) {
        plan.push(step(outro, 1));
    }
    plan
}

/// Loudness/peak numbers an agent can act on without ears.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct PreviewStats {
    pub duration_secs: f32,
    pub peak_dbfs: f32,
    pub rms_dbfs: f32,
    /// Per plan step: (section, intensity, rms dBFS, audible layers).
    pub steps: Vec<StepStats>,
    /// Plain-language warnings (clipping, silent steps, inaudible layers).
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct StepStats {
    pub section: String,
    pub intensity: u8,
    pub start_secs: f32,
    pub rms_dbfs: f32,
    pub layers: Vec<String>,
    /// Each audible layer's own level in this step (dBFS rms): how much an
    /// intensity layer actually adds, for ears-free balancing.
    pub layer_rms_dbfs: std::collections::BTreeMap<String, f32>,
}

fn dbfs(x: f32) -> f32 {
    if x <= 0.0 {
        -120.0
    } else {
        (20.0 * x.log10()).max(-120.0)
    }
}

/// Mix a plan the way audioforge plays it: loop stems wrap, once stems
/// ring out, sections cross-fade over one beat, intensity fades layers
/// over one beat without restarting them. Mono, `SAMPLE_RATE`.
pub fn render_preview(
    c: &Composition,
    stems: &[RenderedStem],
    plan: &[PlanStep],
) -> Result<(Vec<f32>, PreviewStats), String> {
    let beat = beats_to_frames(c, 1.0).max(1);
    let mut out: Vec<f32> = Vec::new();
    let mut steps = Vec::new();
    let mut warnings = Vec::new();
    let mut at = 0usize;
    for (si, step) in plan.iter().enumerate() {
        let sec = c
            .section(&step.section)
            .ok_or_else(|| format!("plan[{si}]: unknown section {:?}", step.section))?;
        if !(1..=super::MAX_INTENSITY).contains(&step.intensity) {
            return Err(format!(
                "plan[{si}]: intensity {} want 1..=5",
                step.intensity
            ));
        }
        let once = sec.role != Role::Loop;
        let bars = if once {
            sec.bars
        } else {
            step.bars.unwrap_or(sec.bars)
        };
        let len = beats_to_frames(c, (bars * c.beats_per_bar) as f64);
        let prev_int = si
            .checked_sub(1)
            .map(|p| plan[p].intensity)
            .unwrap_or(step.intensity);
        let same_section = si > 0 && plan[si - 1].section == step.section;
        let prev_once = si > 0
            && c.section(&plan[si - 1].section)
                .is_some_and(|s| s.role != Role::Loop);
        // Phase continues across consecutive steps of the same section.
        let phase0 = if same_section {
            steps
                .iter()
                .rev()
                .take_while(|s: &&StepStats| s.section == step.section)
                .count()
        } else {
            0
        };
        let phase_offset: usize = if phase0 > 0 {
            plan[si - phase0..si]
                .iter()
                .map(|p| beats_to_frames(c, (p.bars.unwrap_or(sec.bars) * c.beats_per_bar) as f64))
                .sum()
        } else {
            0
        };
        let mut layers = Vec::new();
        let mut layer_rms = std::collections::BTreeMap::new();
        let mut step_sum = vec![0.0f32; len];
        for st in stems.iter().filter(|s| s.section == sec.id) {
            let lg = c
                .layer(&st.layer)
                .map(|l| db_to_gain(l.gain_db))
                .unwrap_or(1.0);
            let on = step.intensity >= st.min_intensity;
            let was_on = prev_int >= st.min_intensity;
            if on {
                layers.push(st.layer.clone());
            }
            if !on && !(same_section && was_on) {
                continue;
            }
            let span = if once { st.samples.len() } else { len };
            let mut energy = 0.0f64;
            if out.len() < at + span {
                out.resize(at + span, 0.0);
            }
            for i in 0..span {
                let s = if once {
                    st.samples[i]
                } else {
                    st.samples[(phase_offset + i) % st.samples.len()]
                };
                // intensity ramp over the first beat of the step
                let t = (i as f32 / beat as f32).min(1.0);
                let ig = match (same_section, was_on, on) {
                    (true, true, false) => 1.0 - t,
                    (true, false, true) => t,
                    (_, _, true) => 1.0,
                    _ => 0.0,
                };
                // section cross-fade in (loops entered from another loop)
                let xin = if !same_section && !prev_once && si > 0 && !once {
                    t
                } else {
                    1.0
                };
                // fade out over the last beat when the next step changes section
                let next_changes = plan.get(si + 1).is_some_and(|n| n.section != step.section);
                let xout = if !once && next_changes && i + beat > len {
                    ((len - i) as f32 / beat as f32).clamp(0.0, 1.0)
                } else {
                    1.0
                };
                let v = s * lg * ig * xin * xout;
                out[at + i] += v;
                if i < len {
                    energy += (v as f64) * (v as f64);
                }
                if i < len {
                    step_sum[i] += v;
                }
            }
            if on {
                layer_rms.insert(
                    st.layer.clone(),
                    dbfs((energy / len.max(1) as f64).sqrt() as f32),
                );
            }
        }
        let rms = (step_sum.iter().map(|x| x * x).sum::<f32>() / len.max(1) as f32).sqrt();
        if layers.is_empty() {
            warnings.push(format!(
                "plan[{si}] {} at intensity {}: no layer sounds (raise intensity or add a min_intensity 1 layer)",
                sec.id, step.intensity
            ));
        } else if dbfs(rms) < -45.0 {
            warnings.push(format!(
                "plan[{si}] {}: very quiet ({:.0} dBFS rms)",
                sec.id,
                dbfs(rms)
            ));
        }
        steps.push(StepStats {
            section: sec.id.clone(),
            intensity: step.intensity,
            start_secs: at as f32 / SAMPLE_RATE as f32,
            rms_dbfs: dbfs(rms),
            layers,
            layer_rms_dbfs: layer_rms,
        });
        at += len;
    }
    let peak = out.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    if peak >= 1.0 {
        warnings.push(format!("clips: peak {:.1} dBFS", dbfs(peak)));
    }
    for l in &c.layers {
        if !stems.iter().any(|s| s.layer == l.id) {
            warnings.push(format!("layer {:?} has no notes in any section", l.id));
        }
    }
    let rms = (out.iter().map(|x| x * x).sum::<f32>() / out.len().max(1) as f32).sqrt();
    Ok((
        out.clone(),
        PreviewStats {
            duration_secs: out.len() as f32 / SAMPLE_RATE as f32,
            peak_dbfs: dbfs(peak),
            rms_dbfs: dbfs(rms),
            steps,
            warnings,
        },
    ))
}

/// 16-bit PCM WAV, `channels` interleaved.
pub fn wav_bytes(samples: &[f32], channels: u16) -> Vec<u8> {
    let mut out = Vec::with_capacity(44 + samples.len() * 2);
    let data_len = (samples.len() * 2) as u32;
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    out.extend_from_slice(&(SAMPLE_RATE * 2 * channels as u32).to_le_bytes());
    out.extend_from_slice(&(2 * channels).to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        out.extend_from_slice(&((s.clamp(-1.0, 1.0) * 32767.0).round() as i16).to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::super::tests::tiny;
    use super::*;
    use crate::loopseam::analyze_seam;

    #[test]
    fn loop_stems_are_exact_length_and_seamless() {
        let c = tiny();
        let (stems, trim) = render_stems(&c);
        assert!(trim.is_finite());
        let calm = stems
            .iter()
            .find(|s| s.section == "calm" && s.layer == "bed")
            .unwrap();
        // 2 bars of 4 beats at 120 bpm = 4 s.
        assert_eq!(calm.samples.len(), 4 * 48_000);
        assert_eq!(calm.length_frames, calm.samples.len());
        // The F4 at beat 6 rings past the loop end: its tail must be folded
        // onto the start, so the loop start is not silent...
        assert!(calm.samples[..2_000].iter().any(|s| s.abs() > 1e-3));
        // ...and the wrap-around seam is no bigger a step than the music
        // itself makes (no click).
        let seam = analyze_seam(&calm.samples, 0.05);
        assert!(!seam.is_click(), "{seam:?}");
    }

    #[test]
    fn once_stems_ring_out_past_their_length() {
        let (stems, _) = render_stems(&tiny());
        let intro = stems.iter().find(|s| s.section == "intro").unwrap();
        assert_eq!(intro.length_frames, 2 * 48_000);
        assert!(intro.samples.len() > intro.length_frames, "tail kept");
        assert_eq!(*intro.samples.last().unwrap(), 0.0);
    }

    #[test]
    fn full_intensity_mix_peaks_below_full_scale() {
        let c = tiny();
        let (stems, _) = render_stems(&c);
        let (mix, stats) = render_preview(&c, &stems, &default_plan(&c)).unwrap();
        assert!(stats.peak_dbfs <= 0.0, "{stats:?}");
        assert!(!mix.is_empty());
        assert!(stats.warnings.is_empty(), "{:?}", stats.warnings);
        // intensity 1 hears the bed only; intensity 5 adds the pulse.
        let calm: Vec<&StepStats> = stats.steps.iter().filter(|s| s.section == "calm").collect();
        assert_eq!(calm[0].layers, vec!["bed"]);
        assert_eq!(calm[1].layers, vec!["bed", "pulse"]);
        let pulse = calm[1].layer_rms_dbfs["pulse"];
        assert!(
            pulse > -90.0 && pulse < calm[1].rms_dbfs,
            "pulse level {pulse}"
        );
    }

    #[test]
    fn preview_rejects_unknown_sections_and_flags_silence() {
        let c = tiny();
        let (stems, _) = render_stems(&c);
        let bad = [PlanStep {
            section: "nope".into(),
            bars: None,
            intensity: 1,
        }];
        assert!(render_preview(&c, &stems, &bad).is_err());
        let mut c2 = c.clone();
        c2.layers[0].min_intensity = 2; // bed now needs intensity 2
        let (stems2, _) = render_stems(&c2);
        let plan = [PlanStep {
            section: "calm".into(),
            bars: None,
            intensity: 1,
        }];
        let (_, stats) = render_preview(&c2, &stems2, &plan).unwrap();
        assert!(
            stats.warnings.iter().any(|w| w.contains("no layer sounds")),
            "{:?}",
            stats.warnings
        );
    }
}
