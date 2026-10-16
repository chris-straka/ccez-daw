//! Render a [`Composition`] to stereo stems and to a preview mixdown.
//!
//! Teaching note: a loop stem must join end-to-start without a click or a
//! gap in the reverb. The trick: render the section once with its full
//! release/reverb tail (`len + tail` samples), then *fold* the tail back
//! onto the start (`stem[i] += buf[len + i]`). That is exactly what a
//! listener hears on the second and every later pass, when the previous
//! pass is still ringing. Intros and outros play once, so they keep their
//! tail (trimmed where it falls silent) and ring out.
//!
//! Every instrument is placed on a stereo stage (`pan`, default: its
//! orchestral seat) and sends into one shared hall ([`super::reverb`]) per
//! stem, so the layers sound like one room when audioforge sums them.

use std::collections::{BTreeMap, HashMap};

use super::library;
use super::sampler::{pan_gains, SampledInstrument};
use super::synth::render_note;
use super::{Composition, LayerDef, PatchKind, Role, SectionDef, SAMPLE_RATE};

/// Seconds of tail rendered past a section (releases + reverb).
pub const TAIL_SECS: f32 = 8.0;

/// One rendered stem.
#[derive(Debug, Clone, PartialEq)]
pub struct RenderedStem {
    pub section: String,
    pub layer: String,
    pub role: Role,
    pub min_intensity: u8,
    /// Stereo at `SAMPLE_RATE`; `left.len() == right.len()`.
    pub left: Vec<f32>,
    pub right: Vec<f32>,
    /// Musical length in frames (loops: `left.len()`; once: the hand-off
    /// point, before the ring-out tail).
    pub length_frames: usize,
}

impl RenderedStem {
    pub fn frames(&self) -> usize {
        self.left.len()
    }
    /// Interleaved L/R.
    pub fn interleaved(&self) -> Vec<f32> {
        interleave(&self.left, &self.right)
    }
}

pub fn interleave(l: &[f32], r: &[f32]) -> Vec<f32> {
    l.iter().zip(r).flat_map(|(a, b)| [*a, *b]).collect()
}

/// Stems plus what the render noticed.
#[derive(Debug, Clone, PartialEq)]
pub struct Rendered {
    pub stems: Vec<RenderedStem>,
    /// Shared trim (dB) that puts the loudest full-intensity section near
    /// -1 dBFS.
    pub trim_db: f32,
    /// Plain-language problems (notes no recording covers, ...).
    pub warnings: Vec<String>,
    /// Each instrument's own dry level (dBFS rms, after the trim) while it
    /// sounds: balance a mix without ears.
    pub instrument_rms_dbfs: BTreeMap<String, f32>,
    /// The composition's `master`: static gain on every stem so the
    /// loudest loop at full intensity sits at the loudness target
    /// (capped so peaks stay under the ceiling).
    pub master: Option<MasterGain>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct MasterGain {
    pub target_lufs: f32,
    /// The loudest full-intensity loop before the gain.
    pub reference_lufs: f32,
    pub gain_db: f32,
    /// True when the ceiling, not the target, set the gain.
    pub capped_by_peak: bool,
}

pub fn db_to_gain(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

fn beats_to_frames(c: &Composition, beats: f64) -> usize {
    (beats * c.secs_per_beat() * SAMPLE_RATE as f64).round() as usize
}

/// A playable instrument: built-in synth or loaded samples.
enum Voice {
    Synth(&'static super::synth::Patch),
    Sampled(Box<SampledInstrument>),
}

/// Instruments loaded once per render (sampled ones download on demand).
#[derive(Default)]
struct Voices {
    by_id: HashMap<String, (Voice, f32)>,
    /// instrument id -> notes no region covered.
    silent: BTreeMap<String, usize>,
    /// instrument id -> (dry energy, frames where it sounds).
    energy: BTreeMap<String, (f64, usize)>,
}

impl Voices {
    fn get(&mut self, c: &Composition, id: &str) -> Result<Option<&mut (Voice, f32)>, String> {
        if !self.by_id.contains_key(id) {
            let Some(inst) = c.instrument(id) else {
                return Ok(None);
            };
            let (voice, seat) = match super::patch_kind(&inst.patch) {
                Some(PatchKind::Synth(p)) => (Voice::Synth(p), 0.0),
                Some(PatchKind::Sampled(e)) => (
                    Voice::Sampled(Box::new(
                        library::load(e).map_err(|m| format!("instrument {id:?} ({}): {m}", e.id))?,
                    )),
                    e.pan,
                ),
                Some(PatchKind::Local) => {
                    let (regions, style) = library::local_spec(&inst.patch)
                        .expect("local patch")
                        .map_err(|m| format!("instrument {id:?}: {m}"))?;
                    (Voice::Sampled(Box::new(SampledInstrument::new(regions, style))), 0.0)
                }
                None => return Ok(None),
            };
            self.by_id
                .insert(id.to_string(), (voice, inst.pan.unwrap_or(seat)));
        }
        Ok(self.by_id.get_mut(id))
    }
}

/// Seeded value in [0, 1) per (part, note): picks random-range regions.
fn note_rand(pi: usize, ni: usize) -> f32 {
    let mut x = (pi as u32).wrapping_mul(0x9E37_79B9) ^ (ni as u32).wrapping_mul(0x85EB_CA6B);
    x ^= x >> 16;
    x = x.wrapping_mul(0x7FEB_352D);
    x ^= x >> 15;
    (x >> 8) as f32 / (1u32 << 24) as f32
}

/// Dry + reverb render of one (section, layer): `len + tail` frames.
fn render_pair(
    c: &Composition,
    sec: &SectionDef,
    layer: &LayerDef,
    voices: &mut Voices,
) -> Result<([Vec<f32>; 2], usize), String> {
    let len = beats_to_frames(c, c.section_beats(sec) as f64);
    let tail = (TAIL_SECS * SAMPLE_RATE as f32) as usize;
    let mut dry = [vec![0.0f32; len + tail], vec![0.0f32; len + tail]];
    let mut send = [vec![0.0f32; len + tail], vec![0.0f32; len + tail]];
    for (pi, part) in c
        .parts
        .iter()
        .enumerate()
        .filter(|(_, p)| p.section == sec.id && p.layer == layer.id)
    {
        let Some(inst) = c.instrument(&part.instrument) else {
            continue;
        };
        let g = db_to_gain(inst.gain_db);
        let rev = (inst.reverb * layer.reverb.unwrap_or(1.0)).clamp(0.0, 2.0);
        let Some((voice, pan)) = voices.get(c, &part.instrument)? else {
            continue;
        };
        let (pl, pr) = pan_gains(*pan);
        let mut silent = 0usize;
        let mut part_buf = [vec![0.0f32; len + tail], vec![0.0f32; len + tail]];
        for (ni, n) in part.notes.iter().enumerate() {
            let start = beats_to_frames(c, n.start as f64);
            let dur = (n.length as f64 * c.secs_per_beat()) as f32;
            let (l, r) = match voice {
                Voice::Synth(p) => {
                    let seed = (pi as u32) << 16 | ni as u32;
                    let m = render_note(p, n.pitch, n.velocity, dur, SAMPLE_RATE, seed);
                    (m.iter().map(|s| s * pl).collect(), m.iter().map(|s| s * pr).collect())
                }
                Voice::Sampled(s) => {
                    let (l, r) = s.render_note(n.pitch, n.velocity, dur, SAMPLE_RATE, note_rand(pi, ni))?;
                    if l.is_empty() {
                        silent += 1;
                    }
                    (l, r)
                }
            };
            for (ch, buf) in [l, r].into_iter().enumerate() {
                for (d, s) in part_buf[ch].iter_mut().skip(start).zip(&buf) {
                    *d += s * g;
                }
            }
        }
        // Level while it plays: frames above -70 dBFS count.
        let (mut energy, mut active) = (0.0f64, 0usize);
        for k in 0..len + tail {
            let (a, b) = (part_buf[0][k], part_buf[1][k]);
            for (ch, s) in [a, b].into_iter().enumerate() {
                dry[ch][k] += s * (1.0 - 0.5 * rev);
                send[ch][k] += s * rev;
            }
            let p = 0.5 * ((a as f64).powi(2) + (b as f64).powi(2));
            if p > 1e-7 {
                energy += p;
                active += 1;
            }
        }
        let e = voices.energy.entry(part.instrument.clone()).or_default();
        e.0 += energy;
        e.1 += active;
        if silent > 0 {
            *voices.silent.entry(part.instrument.clone()).or_default() += silent;
        }
    }
    if send.iter().any(|ch| ch.iter().any(|s| *s != 0.0)) {
        let (wl, wr) = super::reverb::process(&send[0], &send[1], SAMPLE_RATE, &c.layer_room(layer));
        for (d, w) in dry[0].iter_mut().zip(&wl) {
            *d += w;
        }
        for (d, w) in dry[1].iter_mut().zip(&wr) {
            *d += w;
        }
    }
    // Recordings carry room rumble below the lowest note an orchestra
    // plays; a 30 Hz high-pass (linear, before the fold) clears it.
    for ch in &mut dry {
        highpass(ch, RUMBLE_HZ);
        if let Some(eq) = &layer.eq {
            eq.apply(ch, SAMPLE_RATE as f32);
        }
    }
    Ok((dry, len))
}

/// Rumble filter corner (Hz).
pub const RUMBLE_HZ: f32 = 30.0;

/// 2nd-order Butterworth high-pass, in place.
fn highpass(x: &mut [f32], hz: f32) {
    let w = std::f32::consts::TAU * hz / SAMPLE_RATE as f32;
    let (sn, cs) = w.sin_cos();
    let alpha = sn / std::f32::consts::SQRT_2;
    let a0 = 1.0 + alpha;
    let b0 = (1.0 + cs) / 2.0 / a0;
    let b1 = -(1.0 + cs) / a0;
    let a1 = -2.0 * cs / a0;
    let a2 = (1.0 - alpha) / a0;
    let (mut x1, mut x2, mut y1, mut y2) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
    for v in x.iter_mut() {
        let x0 = *v;
        let y0 = b0 * x0 + b1 * x1 + b0 * x2 - a1 * y1 - a2 * y2;
        x2 = x1;
        x1 = x0;
        y2 = y1;
        y1 = y0;
        *v = y0;
    }
}

/// Fold everything past `len` back onto the loop (seamless loop stem).
pub fn fold_tail(buf: &[f32], len: usize) -> Vec<f32> {
    let mut out = vec![0.0f32; len];
    for (i, s) in buf.iter().enumerate() {
        out[i % len] += s;
    }
    out
}

/// Trim trailing near-silence (below -80 dBFS on both sides), keeping at
/// least `min` frames, with a 5 ms fade at the cut.
fn trim_tail(mut buf: [Vec<f32>; 2], min: usize) -> [Vec<f32>; 2] {
    let floor = 1e-4;
    let end = buf
        .iter()
        .map(|ch| ch.iter().rposition(|s| s.abs() > floor).map(|i| i + 1).unwrap_or(0))
        .max()
        .unwrap_or(0);
    for ch in &mut buf {
        let n = end.max(min).min(ch.len());
        ch.truncate(n);
        let fade = (SAMPLE_RATE as usize / 200).min(n.saturating_sub(min));
        for k in 0..fade {
            ch[n - 1 - k] *= k as f32 / fade as f32;
        }
    }
    buf
}

/// Render every stem (one per (section, layer) with notes), with one
/// shared trim so the loudest full-intensity section peaks near -1 dBFS
/// once audioforge sums the layers. Sampled instruments are downloaded
/// into the cache first if missing (an error, never a silent stand-in).
pub fn render_stems(c: &Composition) -> Result<Rendered, String> {
    let mut voices = Voices::default();
    let mut stems = Vec::new();
    for (sec, layer) in c.stem_pairs() {
        let (buf, len) = render_pair(c, sec, layer, &mut voices)?;
        let [left, right] = match sec.role {
            Role::Loop => [fold_tail(&buf[0], len), fold_tail(&buf[1], len)],
            Role::Intro | Role::Outro => trim_tail(buf, len),
        };
        stems.push(RenderedStem {
            section: sec.id.clone(),
            layer: layer.id.clone(),
            role: sec.role,
            min_intensity: layer.min_intensity,
            left,
            right,
            length_frames: len,
        });
    }
    // Worst-case peak: every layer of a section at full intensity, summed
    // with its layer gain (what audioforge plays at intensity 5).
    let mut worst = 0.0f32;
    let mut loop_mixes: Vec<[Vec<f32>; 2]> = Vec::new();
    for sec in &c.sections {
        let mut sum: [Vec<f32>; 2] = [Vec::new(), Vec::new()];
        for st in stems.iter().filter(|s| s.section == sec.id) {
            let g = c
                .layer(&st.layer)
                .map(|l| db_to_gain(l.gain_db))
                .unwrap_or(1.0);
            for (ch, src) in [&st.left, &st.right].into_iter().enumerate() {
                if sum[ch].len() < src.len() {
                    sum[ch].resize(src.len(), 0.0);
                }
                for (a, b) in sum[ch].iter_mut().zip(src) {
                    *a += b * g;
                }
            }
        }
        for ch in &sum {
            worst = worst.max(ch.iter().fold(0.0f32, |m, s| m.max(s.abs())));
        }
        if sec.role == Role::Loop && !sum[0].is_empty() {
            loop_mixes.push(sum);
        }
    }
    let target = db_to_gain(-1.0);
    let mut trim = if worst > 0.0 { target / worst } else { 1.0 };
    let mut master = None;
    let mut master_warning = None;
    if let Some(m) = &c.master {
        // Loudest loop (integrated LUFS) and worst true peak, after the trim.
        let mut reference: Option<f32> = None;
        let mut tp = f32::MIN;
        for mix in &loop_mixes {
            let (l, r): (Vec<f32>, Vec<f32>) = (mix[0].iter().map(|v| v * trim).collect(), mix[1].iter().map(|v| v * trim).collect());
            if let Some(x) = crate::meter::lufs::analyse_offline(&[&l, &r], SAMPLE_RATE as f32).0 {
                reference = Some(reference.map_or(x, |y: f32| y.max(x)));
            }
            tp = tp.max(super::fx::true_peak_dbtp(&[&l, &r]));
        }
        if let Some(reference) = reference {
            let want = m.lufs - reference;
            let room = m.ceiling_dbtp - tp;
            let gain_db = want.min(room);
            let capped_by_peak = room < want;
            if capped_by_peak && want - room > 1.0 {
                master_warning = Some(format!(
                    "master: peaks hold the stems {:.1} dB under the {:.0} LUFS target (the preview's limiter closes the gap; for the stems, soften the loudest hits or lower the target)",
                    want - room,
                    m.lufs
                ));
            }
            trim *= db_to_gain(gain_db);
            master = Some(MasterGain { target_lufs: m.lufs, reference_lufs: reference, gain_db, capped_by_peak });
        }
    }
    for st in &mut stems {
        for s in st.left.iter_mut().chain(st.right.iter_mut()) {
            *s *= trim;
        }
    }
    let warnings = voices
        .silent
        .iter()
        .map(|(id, n)| {
            let span = match voices.by_id.get(id) {
                Some((Voice::Sampled(s), _)) => s
                    .key_span()
                    .map(|(lo, hi)| format!(" (its recordings cover MIDI {lo}..={hi})"))
                    .unwrap_or_default(),
                _ => String::new(),
            };
            format!("instrument {id:?}: {n} note(s) have no recording at that pitch/velocity and are silent{span}")
        })
        .chain(master_warning)
        .collect();
    let instrument_rms_dbfs = voices
        .energy
        .iter()
        .map(|(id, (e, n))| (id.clone(), dbfs(((e / (*n).max(1) as f64).sqrt() as f32) * trim)))
        .collect();
    Ok(Rendered {
        stems,
        trim_db: 20.0 * trim.log10(),
        warnings,
        instrument_rms_dbfs,
        master,
    })
}

/// Loop-seam check for one stem: the wrap-around step against the
/// largest step the music itself makes (99.9th percentile).
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct SeamStat {
    pub section: String,
    pub layer: String,
    /// `max(|last - first|)` over both channels.
    pub seam_step: f32,
    /// 99.9th-percentile sample-to-sample step inside the loop.
    pub p999_step: f32,
    /// The seam is no bigger a jump than the music already makes.
    pub seamless: bool,
}

pub fn seam_stats(stems: &[RenderedStem]) -> Vec<SeamStat> {
    stems
        .iter()
        .filter(|s| s.role == Role::Loop && s.frames() > 1)
        .map(|s| {
            let mut steps: Vec<f32> = Vec::with_capacity(s.frames() * 2);
            let mut seam = 0.0f32;
            for ch in [&s.left, &s.right] {
                steps.extend(ch.windows(2).map(|w| (w[1] - w[0]).abs()));
                seam = seam.max(crate::loopseam::seam_step(ch));
            }
            steps.sort_by(|a, b| a.total_cmp(b));
            let p999 = steps[(steps.len() as f64 * 0.999) as usize % steps.len()];
            SeamStat {
                section: s.section.clone(),
                layer: s.layer.clone(),
                seam_step: seam,
                p999_step: p999,
                seamless: seam <= p999.max(crate::loopseam::DEFAULT_CLICK_THRESHOLD),
            }
        })
        .collect()
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
    /// Per loop stem: does the wrap-around join cleanly?
    pub loop_seams: Vec<SeamStat>,
    /// Each instrument's own dry level while it sounds (dBFS rms).
    pub instrument_rms_dbfs: BTreeMap<String, f32>,
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
/// over one beat without restarting them. Stereo `(left, right)`.
pub fn render_preview(
    c: &Composition,
    rendered: &Rendered,
    plan: &[PlanStep],
) -> Result<([Vec<f32>; 2], PreviewStats), String> {
    let stems = &rendered.stems;
    let beat = beats_to_frames(c, 1.0).max(1);
    let mut out: [Vec<f32>; 2] = [Vec::new(), Vec::new()];
    let mut steps = Vec::new();
    let mut warnings = rendered.warnings.clone();
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
        let mut step_energy = vec![0.0f64; len];
        let mut step_sum = [vec![0.0f32; len], vec![0.0f32; len]];
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
            let span = if once { st.frames() } else { len };
            let mut energy = 0.0f64;
            for o in out.iter_mut() {
                if o.len() < at + span {
                    o.resize(at + span, 0.0);
                }
            }
            let next_changes = plan.get(si + 1).is_some_and(|n| n.section != step.section);
            for i in 0..span {
                let idx = if once {
                    i
                } else {
                    (phase_offset + i) % st.frames()
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
                let xout = if !once && next_changes && i + beat > len {
                    ((len - i) as f32 / beat as f32).clamp(0.0, 1.0)
                } else {
                    1.0
                };
                let gain = lg * ig * xin * xout;
                for (ch, src) in [&st.left, &st.right].into_iter().enumerate() {
                    let v = src[idx] * gain;
                    out[ch][at + i] += v;
                    if i < len {
                        energy += (v as f64) * (v as f64) * 0.5;
                        step_sum[ch][i] += v;
                    }
                }
            }
            if on {
                layer_rms.insert(
                    st.layer.clone(),
                    dbfs((energy / len.max(1) as f64).sqrt() as f32),
                );
            }
        }
        for i in 0..len {
            step_energy[i] = 0.5
                * ((step_sum[0][i] as f64).powi(2) + (step_sum[1][i] as f64).powi(2));
        }
        let rms = (step_energy.iter().sum::<f64>() / len.max(1) as f64).sqrt() as f32;
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
    let peak = out
        .iter()
        .flat_map(|ch| ch.iter())
        .fold(0.0f32, |m, s| m.max(s.abs()));
    if peak >= 1.0 {
        warnings.push(format!("clips: peak {:.1} dBFS", dbfs(peak)));
    }
    for l in &c.layers {
        if !stems.iter().any(|s| s.layer == l.id) {
            warnings.push(format!("layer {:?} has no notes in any section", l.id));
        }
    }
    let loop_seams = seam_stats(stems);
    for s in loop_seams.iter().filter(|s| !s.seamless) {
        warnings.push(format!(
            "loop {}/{}: seam step {:.4} is bigger than the music's own ({:.4})",
            s.section, s.layer, s.seam_step, s.p999_step
        ));
    }
    let n = out[0].len();
    let rms = ((out[0].iter().chain(&out[1]).map(|x| (x * x) as f64).sum::<f64>())
        / (2 * n.max(1)) as f64)
        .sqrt() as f32;
    Ok((
        out,
        PreviewStats {
            duration_secs: n as f32 / SAMPLE_RATE as f32,
            peak_dbfs: dbfs(peak),
            rms_dbfs: dbfs(rms),
            steps,
            loop_seams,
            instrument_rms_dbfs: rendered.instrument_rms_dbfs.clone(),
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

    #[test]
    fn loop_stems_are_exact_length_stereo_and_seamless() {
        let c = tiny();
        let r = render_stems(&c).unwrap();
        assert!(r.trim_db.is_finite());
        let calm = r
            .stems
            .iter()
            .find(|s| s.section == "calm" && s.layer == "bed")
            .unwrap();
        // 2 bars of 4 beats at 120 bpm = 4 s.
        assert_eq!(calm.frames(), 4 * 48_000);
        assert_eq!(calm.right.len(), calm.left.len());
        assert_eq!(calm.length_frames, calm.frames());
        // The F4 at beat 6 rings past the loop end: its tail must be folded
        // onto the start, so the loop start is not silent...
        assert!(calm.left[..2_000].iter().any(|s| s.abs() > 1e-3));
        // ...and the wrap-around seam is no bigger a step than the music
        // itself makes (no click).
        let seams = seam_stats(&r.stems);
        assert!(seams.iter().all(|s| s.seamless), "{seams:?}");
        // The shared hall makes the two sides differ (a stereo room).
        let diff: f32 = calm.left.iter().zip(&calm.right).map(|(a, b)| (a - b).abs()).sum();
        assert!(diff > 1.0, "{diff}");
    }

    #[test]
    fn pan_places_an_instrument_on_the_stage() {
        let mut c = tiny();
        c.instruments[0].pan = Some(-1.0);
        c.instruments[0].reverb = 0.0;
        let r = render_stems(&c).unwrap();
        let calm = r.stems.iter().find(|s| s.section == "calm" && s.layer == "bed").unwrap();
        let e = |x: &[f32]| x.iter().map(|v| v * v).sum::<f32>();
        assert!(e(&calm.right) < 1e-9 * e(&calm.left).max(1.0), "hard left, dry");
    }

    #[test]
    fn layer_eq_and_reverb_shape_only_their_layer() {
        let base = render_stems(&tiny()).unwrap();
        let mut c = tiny();
        c.layers[0].eq = Some(super::super::fx::LayerEq { high_cut_hz: Some(800.0), ..Default::default() });
        c.layers[0].reverb = Some(0.0);
        let r = render_stems(&c).unwrap();
        let get = |r: &Rendered, s: &str, l: &str| r.stems.iter().find(|x| x.section == s && x.layer == l).unwrap().clone();
        // High-frequency content of the bed drops (first difference = a crude high-pass).
        let hf = |x: &[f32]| x.windows(2).map(|w| (w[1] - w[0]).powi(2)).sum::<f32>();
        let lf = |x: &[f32]| x.iter().map(|v| v * v).sum::<f32>();
        let (b0, b1) = (get(&base, "calm", "bed"), get(&r, "calm", "bed"));
        assert!(hf(&b1.left) / lf(&b1.left) < 0.5 * hf(&b0.left) / lf(&b0.left), "low-pass darkens the bed");
        // No reverb on the bed: with the hall gone, the two sides only
        // differ by pan (a mono synth panned centre is identical L/R).
        let diff: f32 = b1.left.iter().zip(&b1.right).map(|(a, b)| (a - b).abs()).sum();
        assert!(diff < 1e-3, "dry bed is centred mono: {diff}");
        let bdiff: f32 = b0.left.iter().zip(&b0.right).map(|(a, b)| (a - b).abs()).sum();
        assert!(bdiff > 1.0, "with its send the bed is in the stereo hall: {bdiff}");
        assert!(seam_stats(&r.stems).iter().all(|s| s.seamless));
    }

    #[test]
    fn master_gain_brings_the_loudest_loop_to_the_target() {
        let mut c = tiny();
        c.master = Some(super::super::fx::Master { lufs: -30.0, ceiling_dbtp: -1.0 });
        let r = render_stems(&c).unwrap();
        let m = r.master.clone().unwrap();
        assert!(m.gain_db < 0.0 && !m.capped_by_peak, "{m:?}");
        let (mix, _) = render_preview(&c, &r, &[PlanStep { section: "calm".into(), bars: None, intensity: 5 }]).unwrap();
        let lufs = crate::meter::lufs::analyse_offline(&[&mix[0], &mix[1]], 48_000.0).0.unwrap();
        assert!((lufs - -30.0).abs() < 1.0, "{lufs}");
        // A target the peaks can't reach is capped, and says so.
        c.master = Some(super::super::fx::Master { lufs: -8.0, ceiling_dbtp: -1.0 });
        let r = render_stems(&c).unwrap();
        assert!(r.master.as_ref().unwrap().capped_by_peak);
        assert!(r.warnings.iter().any(|w| w.starts_with("master:")), "{:?}", r.warnings);
    }

    #[test]
    fn once_stems_ring_out_past_their_length() {
        let r = render_stems(&tiny()).unwrap();
        let intro = r.stems.iter().find(|s| s.section == "intro").unwrap();
        assert_eq!(intro.length_frames, 2 * 48_000);
        assert!(intro.frames() > intro.length_frames, "tail kept");
        assert_eq!(*intro.left.last().unwrap(), 0.0);
        assert_eq!(*intro.right.last().unwrap(), 0.0);
    }

    #[test]
    fn full_intensity_mix_peaks_below_full_scale() {
        let c = tiny();
        let r = render_stems(&c).unwrap();
        let (mix, stats) = render_preview(&c, &r, &default_plan(&c)).unwrap();
        assert!(stats.peak_dbfs <= 0.0, "{stats:?}");
        assert!(!mix[0].is_empty() && mix[0].len() == mix[1].len());
        assert!(stats.warnings.is_empty(), "{:?}", stats.warnings);
        assert_eq!(stats.loop_seams.len(), 2);
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
        let r = render_stems(&c).unwrap();
        let bad = [PlanStep {
            section: "nope".into(),
            bars: None,
            intensity: 1,
        }];
        assert!(render_preview(&c, &r, &bad).is_err());
        let mut c2 = c.clone();
        c2.layers[0].min_intensity = 2; // bed now needs intensity 2
        let r2 = render_stems(&c2).unwrap();
        let plan = [PlanStep {
            section: "calm".into(),
            bars: None,
            intensity: 1,
        }];
        let (_, stats) = render_preview(&c2, &r2, &plan).unwrap();
        assert!(
            stats.warnings.iter().any(|w| w.contains("no layer sounds")),
            "{:?}",
            stats.warnings
        );
    }

    /// A sampled (SFZ) instrument renders through the same path: audible,
    /// repitched, seamless, and a note no recording covers is reported.
    #[test]
    fn sampled_instruments_render_and_report_uncovered_notes() {
        let dir = std::env::temp_dir().join(format!("ccez-render-sfz-{}", std::process::id()));
        super::super::sampler::tests::sine_wav(&dir, "c4.wav", 261.63, 2.0, 44_100);
        let sfz = dir.join("t.sfz");
        std::fs::write(&sfz, "<global> ampeg_release=0.5\n<region> sample=c4.wav lokey=48 hikey=84 pitch_keycenter=60\n").unwrap();
        let mut c = tiny();
        c.instruments[0].patch = format!("sfz:{}", sfz.display());
        assert_eq!(c.validate(), Vec::<String>::new());
        c.parts[1].notes[0].pitch = 30; // below the region
        let r = render_stems(&c).unwrap();
        let calm = r.stems.iter().find(|s| s.section == "calm" && s.layer == "bed").unwrap();
        assert!(calm.left.iter().any(|s| s.abs() > 0.05), "the F4 sounds");
        assert!(seam_stats(&r.stems).iter().all(|s| s.seamless));
        assert_eq!(r.warnings.len(), 1, "{:?}", r.warnings);
        assert!(r.warnings[0].contains("1 note(s) have no recording"), "{:?}", r.warnings);
        c.instruments[0].patch = format!("sfz:{}", dir.join("missing.sfz").display());
        assert!(c.validate().iter().any(|e| e.contains("no file at")));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
