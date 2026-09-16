//! S-2 loop-seam audition: gapless-loop click detection (additive; v0/v1 frozen).
//!
//! Teaching note: a loop clicks when the last sample does not meet the first
//! one — the speaker cone is ordered to teleport, and the ear hears the
//! teleport as a tick once per revolution. The metric here is deliberately
//! the simplest honest one: the **boundary discontinuity**
//! `|x[n-1] - x[0]|` on the decoded `[-1, 1]` float samples. Whole-beat
//! reference tones at the cue tempo wrap near zero (see
//! [`crate::gaexport`] edge fades), so a clean export stem measures ~1e-4
//! and anything above the threshold is a real seam, not analysis noise.
//!
//! The same definition ships in `ui/src/gameaudio/loop.ts` (in-DAW preview
//! meter) and here (ship-gate). The validator's rule 6 runs this check over
//! every music-loop stem; waivers are explicit, per-stem, and reasoned, so a
//! waived click is a decision, never an accident.

use std::collections::{HashMap, HashSet};

/// Default click threshold in full-scale float units: 2% FS. PCM16
/// quantization alone is ~3e-5 and faded loop points sit near zero, so 0.02
/// is far above the noise floor and far below an audible tick.
pub const DEFAULT_CLICK_THRESHOLD: f32 = 0.02;

/// Filename (package-relative) of the optional waiver file. The validator
/// reads `<package>/loop-waivers.json` only when `--waive` points at it (or
/// the caller passes the path); the name below is the convention the error
/// text suggests.
pub const WAIVER_FILENAME: &str = "loop-waivers.json";

/// One stem's seam report: the raw step plus the peak for context.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SeamReport {
    /// `|last - first|` on `[-1, 1]` floats.
    pub step: f32,
    /// Peak `|x|` over the stem (0 for an empty stem).
    pub peak: f32,
    /// The threshold this report was judged against.
    pub threshold: f32,
}

impl SeamReport {
    pub fn is_click(&self) -> bool {
        self.step > self.threshold
    }
}

/// Boundary discontinuity of one loop stem: `|x[n-1] - x[0]|`.
/// Empty stems read as 0 (rule 4 already rejects wrong-length stems; the
/// seam check stays silent rather than double-reporting).
pub fn seam_step(samples: &[f32]) -> f32 {
    match (samples.first(), samples.last()) {
        (Some(&first), Some(&last)) => (last - first).abs(),
        _ => 0.0,
    }
}

/// Peak absolute amplitude; context for judging a step.
pub fn peak(samples: &[f32]) -> f32 {
    samples.iter().fold(0.0f32, |m, &s| m.max(s.abs()))
}

/// Full seam report for one decoded stem.
pub fn analyze_seam(samples: &[f32], threshold: f32) -> SeamReport {
    SeamReport {
        step: seam_step(samples),
        peak: peak(samples),
        threshold,
    }
}

/// Validator-side options for the loop-seam rule (rule 6). Caller-side only,
/// never serialized contract surface.
#[derive(Debug, Clone, PartialEq)]
pub struct LoopSeamOptions {
    /// Boundary step above this is a click. Non-positive disables the rule.
    pub threshold: f32,
    /// Stem paths (package-relative, e.g. `stems/cue_bed.wav`) excused from
    /// the rule, each with the human reason recorded in the waiver file.
    pub waivers: HashMap<String, String>,
}

impl Default for LoopSeamOptions {
    fn default() -> Self {
        Self {
            threshold: DEFAULT_CLICK_THRESHOLD,
            waivers: HashMap::new(),
        }
    }
}

impl LoopSeamOptions {
    /// Options with the seam rule disabled (SFX-only packages, legacy runs).
    pub fn disabled() -> Self {
        Self {
            threshold: 0.0,
            waivers: HashMap::new(),
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.threshold > 0.0
    }
}

/// One parsed waiver entry: which stem, and why a human excused it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Waiver {
    pub path: String,
    pub reason: String,
}

/// Parse a waiver file body: `{ "waivers": [{ "path": ..., "reason": ... }] }`.
/// Empty reasons and duplicate paths are errors — a waiver without a *why*
/// is an accident waiting to happen.
pub fn parse_waivers(json: &str) -> Result<Vec<Waiver>, String> {
    let value: serde_json::Value =
        serde_json::from_str(json).map_err(|e| format!("invalid waiver file: {e}"))?;
    let list = value
        .get("waivers")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "invalid waiver file: want { \"waivers\": [{ \"path\", \"reason\" }] }".to_string())?;
    let mut out = Vec::with_capacity(list.len());
    let mut seen = HashSet::new();
    for entry in list {
        let path = entry
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "invalid waiver file: every entry needs a string \"path\"".to_string())?;
        let reason = entry
            .get("reason")
            .and_then(|v| v.as_str())
            .ok_or_else(|| format!("invalid waiver file: '{path}' needs a string \"reason\""))?;
        if path.is_empty() {
            return Err("invalid waiver file: waiver path must not be empty".to_string());
        }
        if reason.trim().is_empty() {
            return Err(format!("invalid waiver file: '{path}' needs a non-empty reason"));
        }
        if !seen.insert(path.to_string()) {
            return Err(format!("invalid waiver file: '{path}' waived twice"));
        }
        out.push(Waiver {
            path: path.to_string(),
            reason: reason.to_string(),
        });
    }
    Ok(out)
}

/// Serialize waivers back to the file format (what the in-DAW "waive" button
/// writes next to the package).
pub fn waivers_to_json(waivers: &[Waiver]) -> String {
    let entries: Vec<serde_json::Value> = waivers
        .iter()
        .map(|w| serde_json::json!({ "path": w.path, "reason": w.reason }))
        .collect();
    serde_json::to_string_pretty(&serde_json::json!({ "waivers": entries }))
        .expect("waivers serialize")
}

/// Read a waiver file from disk into [`LoopSeamOptions`]-ready entries.
pub fn load_waivers(path: &std::path::Path) -> Result<Vec<Waiver>, String> {
    let bytes =
        std::fs::read(path).map_err(|e| format!("cannot read waiver file {}: {e}", path.display()))?;
    let text =
        String::from_utf8(bytes).map_err(|_| format!("waiver file {} is not UTF-8", path.display()))?;
    parse_waivers(&text)
}

/// The fix-or-waive error for one clicking stem. The fix names the cause
/// (edges that do not meet); the waiver names the file and the convention.
pub fn click_error(stem_path: &str, report: SeamReport) -> String {
    format!(
        "stem '{stem_path}' clicks at the loop seam: boundary step {:.4} > threshold {:.4} (peak {:.3}). \
Fix: re-export with loop-clean edges (whole-beat loop at the cue tempo with edge fades), \
or waive with a reason in {WAIVER_FILENAME} (--waive <file>)",
        report.step, report.threshold, report.peak
    )
}

/// Check one decoded music-loop stem against the seam rule. Returns the error
/// string when the stem clicks and is not waived, `None` otherwise.
pub fn check_stem(stem_path: &str, samples: &[f32], options: &LoopSeamOptions) -> Option<String> {
    if !options.is_enabled() {
        return None;
    }
    let report = analyze_seam(samples, options.threshold);
    if !report.is_click() {
        return None;
    }
    if options.waivers.contains_key(stem_path) {
        return None;
    }
    Some(click_error(stem_path, report))
}

/// Linear-interpolation resample under a playback-rate `ratio` (> 0):
/// output length is [`crate::timepitch::resampled_len`] and each output
/// frame reads source position `i * ratio`. Ratio 1 is bit-stable (every
/// read lands on an integer source frame); ratio > 1 shortens (fitting a
/// loop up to a faster tempo), ratio < 1 lengthens. Empty in = empty out.
pub fn resample_linear(samples: &[f32], ratio: f64) -> Result<Vec<f32>, String> {
    if !(ratio > 0.0 && ratio.is_finite()) {
        return Err(format!("time ratio {ratio} must be finite and > 0"));
    }
    if samples.is_empty() {
        return Ok(Vec::new());
    }
    let n = samples.len();
    let out_len = crate::timepitch::resampled_len(n, ratio)?;
    let mut out = Vec::with_capacity(out_len.max(1));
    for i in 0..out_len.max(1) {
        let pos = i as f64 * ratio;
        let i0 = pos.floor() as usize;
        let frac = (pos - i0 as f64) as f32;
        let a = samples[i0.min(n - 1)];
        let b = samples[(i0 + 1).min(n - 1)];
        out.push(a + frac * (b - a));
    }
    out.truncate(out_len);
    Ok(out)
}

/// Seam conform: short raised-cosine edge fade (same construction as the
/// export renderer) pulling both loop edges toward zero so the boundary
/// step closes. `fade_frames` clamps to `[1, n/2]`; buffers shorter than 2
/// frames are untouched. Deterministic and idempotent in effect (a second
/// pass keeps an already-quiet seam quiet).
pub fn conform_loop_seam(samples: &mut [f32], fade_frames: usize) {
    let n = samples.len();
    if n < 2 {
        return;
    }
    let f = fade_frames.clamp(1, n / 2).max(1);
    let smooth = |x: f32| x * x * (3.0 - 2.0 * x);
    for t in 0..f {
        let g = smooth(t as f32 / f as f32);
        samples[t] *= g;
        samples[n - 1 - t] *= g;
    }
}

/// Auto-fit report: the resample factor applied, the fitted length, the
/// seam judged against the click threshold, and whether the conform fade
/// ran to close the seam.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LoopFit {
    /// `target_tempo / source_tempo` (> 0).
    pub ratio: f64,
    /// Output frame count (`resampled_len` of the input).
    pub out_frames: usize,
    /// Seam of the (possibly conformed) output against `threshold`.
    pub seam: SeamReport,
    /// True when [`conform_loop_seam`] ran (output clicked before conform).
    pub conformed: bool,
}

/// Auto-fit an audio loop recorded at `source_tempo` BPM to `target_tempo`
/// BPM: resample by `target / source` (same ratio convention as
/// [`crate::timepitch`], so the sample path and the MIDI path agree), then
/// judge the fitted seam against `threshold`. When the fitted loop clicks,
/// a short edge-fade conform closes the seam and the returned report is the
/// post-conform seam — the output always conforms to the threshold unless
/// the buffer is empty (silent, like [`check_stem`]) or the threshold is
/// non-positive (rule disabled: fit only, never conform).
pub fn auto_fit_loop_to_tempo(
    samples: &[f32],
    source_tempo: f64,
    target_tempo: f64,
    threshold: f32,
) -> Result<(Vec<f32>, LoopFit), String> {
    let ratio = crate::timepitch::loop_fit_ratio(source_tempo, target_tempo)?;
    let mut out = resample_linear(samples, ratio)?;
    let mut seam = analyze_seam(&out, threshold);
    let mut conformed = false;
    if threshold > 0.0 && !out.is_empty() && seam.is_click() {
        let fade = (out.len() / 64).clamp(1, out.len() / 2).max(1);
        conform_loop_seam(&mut out, fade);
        seam = analyze_seam(&out, threshold);
        conformed = true;
    }
    let out_frames = out.len();
    Ok((out, LoopFit { ratio, out_frames, seam, conformed }))
}

/// Waiver entries that name no stem in `stem_paths` are stale (a renamed
/// stem would silently lose its gate), so they fail loudly.
pub fn stale_waiver_errors(waivers: &HashMap<String, String>, stem_paths: &HashSet<String>) -> Vec<String> {
    let mut errors: Vec<String> = waivers
        .keys()
        .filter(|p| !stem_paths.contains(p.as_str()))
        .map(|p| format!("waiver names unknown stem '{p}' (no package stem renders it)"))
        .collect();
    errors.sort();
    errors
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic stand-in loop tone: whole cycles per window (mirrors the
    /// export renderer construction) plus the renderer's raised-cosine edge
    /// fade. The fade is what closes the seam: a raw whole-cycle tone still
    /// misses by one sample of slope at the endpoint, which is exactly the
    /// click an unfaded cut would ship.
    fn clean_loop(n: usize, cycles: f64) -> Vec<f32> {
        let f = (n / 64).max(1);
        let smooth = |x: f32| x * x * (3.0 - 2.0 * x);
        (0..n)
            .map(|t| {
                let s = (0.5 * (2.0 * std::f64::consts::PI * cycles * t as f64 / n as f64).sin()) as f32;
                let g = if t < f {
                    smooth(t as f32 / f as f32)
                } else if t >= n - f {
                    smooth((n - 1 - t) as f32 / f as f32)
                } else {
                    1.0
                };
                s * g
            })
            .collect()
    }

    /// Seeded click injection: noise over the last `len` samples plus a
    /// pinned final sample exactly `height` above the first, deterministic
    /// per seed (mirrors the TS test helper). The pin guarantees the seam
    /// step reads `height` no matter what the noise draws.
    fn inject_click(samples: &mut [f32], seed: u64, len: usize, height: f32) {
        let mut state = seed;
        let mut next = || {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((state >> 33) as f64) / (u32::MAX as f64)
        };
        let start = samples.len().saturating_sub(len);
        for s in &mut samples[start..] {
            *s += (next() as f32 * 2.0 - 1.0) * height;
        }
        let first = samples.first().copied().unwrap_or(0.0);
        if let Some(last) = samples.last_mut() {
            *last = first + height;
        }
    }

    #[test]
    fn clean_loop_passes_seeded_click_fails() {
        let clean = clean_loop(8000, 220.0);
        assert!(analyze_seam(&clean, DEFAULT_CLICK_THRESHOLD).step < 1e-3);
        assert!(check_stem("stems/a.wav", &clean, &LoopSeamOptions::default()).is_none());

        let mut clicked = clean.clone();
        inject_click(&mut clicked, 2026, 32, 0.3);
        let report = analyze_seam(&clicked, DEFAULT_CLICK_THRESHOLD);
        assert!(report.step > DEFAULT_CLICK_THRESHOLD, "step={}", report.step);
        let err = check_stem("stems/a.wav", &clicked, &LoopSeamOptions::default())
            .expect("seeded click must be detected");
        assert!(err.contains("clicks at the loop seam"), "{err}");
        assert!(err.contains("Fix:"), "{err}");
        assert!(err.contains("--waive"), "{err}");
    }

    #[test]
    fn raw_whole_cycle_tone_still_clicks_without_the_fade() {
        // Whole cycles do NOT close the seam by themselves: the endpoint
        // misses by one sample of slope. This is why the renderer fades.
        let raw: Vec<f32> = (0..8000)
            .map(|t| (0.5 * (2.0 * std::f64::consts::PI * 220.0 * t as f64 / 8000.0).sin()) as f32)
            .collect();
        assert!(analyze_seam(&raw, DEFAULT_CLICK_THRESHOLD).is_click());
    }

    #[test]
    fn waived_click_passes_stale_waiver_fails() {
        let mut clicked = clean_loop(8000, 220.0);
        inject_click(&mut clicked, 7, 32, 0.3);
        let mut options = LoopSeamOptions::default();
        options.waivers.insert(
            "stems/a.wav".to_string(),
            "intentional vinyl crackle tail".to_string(),
        );
        assert!(check_stem("stems/a.wav", &clicked, &options).is_none());

        let stems: HashSet<String> = ["stems/a.wav".to_string()].into_iter().collect();
        assert!(stale_waiver_errors(&options.waivers, &stems).is_empty());
        options.waivers.insert("stems/ghost.wav".to_string(), "stale".to_string());
        let stale = stale_waiver_errors(&options.waivers, &stems);
        assert_eq!(stale.len(), 1);
        assert!(stale[0].contains("stems/ghost.wav"), "{stale:?}");
    }

    #[test]
    fn waiver_file_round_trips_and_rejects_empty_reasons() {
        let waivers = vec![Waiver {
            path: "stems/a.wav".to_string(),
            reason: "field recording with real tape splice".to_string(),
        }];
        let json = waivers_to_json(&waivers);
        assert_eq!(parse_waivers(&json).expect("round-trip"), waivers);
        assert!(parse_waivers(r#"{"waivers": []}"#).expect("empty ok").is_empty());
        assert!(parse_waivers(r#"{"waivers": [{"path": "s.wav", "reason": "  "}]}"#).is_err());
        assert!(parse_waivers(r#"{"nope": []}"#).is_err());
    }

    #[test]
    fn resample_ratio_one_is_bit_stable_and_rejects_bad_ratio() {
        let clean = clean_loop(512, 8.0);
        assert_eq!(resample_linear(&clean, 1.0).unwrap(), clean);
        assert_eq!(
            resample_linear(&clean, 2.0).unwrap().len(),
            crate::timepitch::resampled_len(512, 2.0).unwrap()
        );
        assert!(resample_linear(&clean, 0.0).is_err());
        assert!(resample_linear(&clean, f64::INFINITY).is_err());
        assert!(resample_linear(&[], 2.0).unwrap().is_empty());
    }

    #[test]
    fn conform_closes_a_click_and_keeps_clean_quiet() {
        let mut clicked = clean_loop(8000, 220.0);
        inject_click(&mut clicked, 9, 32, 0.3);
        assert!(analyze_seam(&clicked, DEFAULT_CLICK_THRESHOLD).is_click());
        conform_loop_seam(&mut clicked, 128);
        assert!(
            !analyze_seam(&clicked, DEFAULT_CLICK_THRESHOLD).is_click(),
            "step={}",
            analyze_seam(&clicked, DEFAULT_CLICK_THRESHOLD).step
        );
        let mut clean = clean_loop(8000, 220.0);
        conform_loop_seam(&mut clean, 128);
        assert!(!analyze_seam(&clean, DEFAULT_CLICK_THRESHOLD).is_click());
    }

    #[test]
    fn auto_fit_same_tempo_passes_through_and_bad_tempo_errors() {
        let clean = clean_loop(8000, 220.0);
        let (out, fit) =
            auto_fit_loop_to_tempo(&clean, 120.0, 120.0, DEFAULT_CLICK_THRESHOLD).unwrap();
        assert!((fit.ratio - 1.0).abs() < 1e-12);
        assert_eq!(out.len(), fit.out_frames);
        assert!(!fit.conformed, "clean loop at ratio 1 must not need conform");
        assert!(!fit.seam.is_click());
        assert!(auto_fit_loop_to_tempo(&clean, 0.0, 120.0, DEFAULT_CLICK_THRESHOLD).is_err());
        assert!(auto_fit_loop_to_tempo(&clean, 120.0, f64::NAN, DEFAULT_CLICK_THRESHOLD).is_err());
    }

    #[test]
    fn auto_fit_conforms_a_fitted_click_to_threshold() {
        let mut clicked = clean_loop(8000, 220.0);
        inject_click(&mut clicked, 11, 32, 0.3);
        let (out, fit) =
            auto_fit_loop_to_tempo(&clicked, 120.0, 140.0, DEFAULT_CLICK_THRESHOLD).unwrap();
        assert!((fit.ratio - 140.0 / 120.0).abs() < 1e-12);
        assert_eq!(out.len(), crate::timepitch::resampled_len(8000, fit.ratio).unwrap());
        assert!(fit.conformed, "fitted click must trigger the conform fade");
        assert!(!fit.seam.is_click(), "post-conform seam must meet threshold");
        assert!(check_stem("stems/a.wav", &out, &LoopSeamOptions::default()).is_none());
    }

    #[test]
    fn disabled_rule_and_empty_stems_stay_silent() {
        let mut clicked = clean_loop(64, 4.0);
        inject_click(&mut clicked, 1, 8, 0.5);
        assert!(check_stem("s.wav", &clicked, &LoopSeamOptions::disabled()).is_none());
        assert!(check_stem("s.wav", &[], &LoopSeamOptions::default()).is_none());
    }
}
