//! GA-4 engine export package: stems + JSON banks + validator (additive; v0/v1 frozen).
//!
//! Teaching note: the engine deliverable is a **directory**, not a project
//! file. The DAW renders every adaptive-cue layer to a loopable WAV stem and
//! every SFX event pool to one-shot WAVs, then writes one JSON manifest
//! (`package.json`, the frozen [`ExportPackage`] shape) plus one verbatim
//! [`SfxBank`] JSON per bank (`bank_<id>.json`). A versioned validator
//! approves every package before it ships, so a missing loop, a dangling
//! clip, or a typo'd mix address is a red build, never a silent in-game bug.
//!
//! Layout (all paths package-relative, per `contracts/export-package.md`):
//!
//! ```text
//! <package>/
//!   package.json        # ExportPackage serialized verbatim
//!   bank_<id>.json      # one SfxBank per bank_ids entry, serialized verbatim
//!   stems/<cue>_<layer>.wav
//!   sfx/<event>_<n>.wav
//! ```
//!
//! Rendering is deterministic: music stems are loop-clean reference tones at
//! the cue tempo (the [`bounce`](crate::bounce) philosophy — opaque v0
//! sources have no decoder yet, so clips render procedurally), and SFX stems
//! are per-clip stand-in tones (the [`audition`](crate::sfx) philosophy) with
//! a seed-drawn detune so rule 5 (same project + seed = byte-identical stems)
//! exercises the seeded path. Real sample/MIDI decoding lands behind the same
//! function signatures. WAV bytes reuse the [`bounce`](crate::bounce) PCM16
//! mono + `smpl` loop codec, so anything that reads a frozen stem reads an
//! export stem.
//!
//! Frozen-shape discipline: this module only *evaluates* the v1 contracts —
//! no new serialized types, no `emit` changes, `ui/src/generated/*.ts`
//! untouched. `ExportRequest`/`ExportOptions` are caller-side inputs, never
//! serialized contract surface.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::bounce::{decode_wav, encode_wav, Stem};
use crate::game_audio::{
    AdaptiveCue, ExportPackage, ExportStem, GameStateParam, SfxBank, StemKind, TransitionKind,
    GAME_AUDIO_SCHEMA_VERSION,
};
use crate::model::Project;
use crate::sfx::SeededRng;

/// Production render rate: the game-audio default from `contracts/export-package.md`.
pub const EXPORT_SAMPLE_RATE: u32 = 48_000;
/// Validator version stamped on every approved manifest (`"1"` in v1).
pub const VALIDATOR_VERSION: &str = "1";
/// Default export seed: rule 5 needs a seed, and the frozen manifest has no
/// seed field, so reproducible exports use this unless the caller passes an
/// explicit [`ExportOptions::seed`] (the validator takes the same seed back).
pub const DEFAULT_EXPORT_SEED: u64 = 2026;
/// One-shot SFX stem length in seconds (loop `0, 0` in the manifest).
pub const SFX_ONESHOT_SECONDS: f64 = 1.0;
/// Loop length used when a music layer names no resolvable clips.
pub const DEFAULT_LOOP_BEATS: f64 = 4.0;

// -- caller-side inputs (never serialized) ------------------------------------

/// What to export: a name plus the cue/bank ids (mirrors the frozen
/// `gameaudio_export` input `{ name, cueIds, bankIds }`).
#[derive(Debug, Clone, PartialEq)]
pub struct ExportRequest {
    pub name: String,
    pub cue_ids: Vec<String>,
    pub bank_ids: Vec<String>,
}

impl ExportRequest {
    pub fn new(name: &str, cue_ids: Vec<String>, bank_ids: Vec<String>) -> Self {
        Self {
            name: name.to_string(),
            cue_ids,
            bank_ids,
        }
    }
}

/// How to render: sample rate + seed. The defaults are the shippable shape
/// (48 kHz, seed 2026); tests pass a small rate for speed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ExportOptions {
    pub sample_rate: u32,
    pub seed: u64,
}

impl Default for ExportOptions {
    fn default() -> Self {
        Self {
            sample_rate: EXPORT_SAMPLE_RATE,
            seed: DEFAULT_EXPORT_SEED,
        }
    }
}

// -- built package -------------------------------------------------------------

/// One manifest stem entry plus the exact WAV bytes written for it.
#[derive(Debug, Clone, PartialEq)]
pub struct RenderedStem {
    pub stem: ExportStem,
    pub wav_bytes: Vec<u8>,
}

/// One bank file: its id, filename, and verbatim JSON body.
#[derive(Debug, Clone, PartialEq)]
pub struct BankFile {
    pub id: String,
    pub filename: String,
    pub json: String,
}

/// A fully built (not yet written) export: manifest + stems + bank files.
#[derive(Debug, Clone, PartialEq)]
pub struct BuiltPackage {
    pub package: ExportPackage,
    pub stems: Vec<RenderedStem>,
    pub banks: Vec<BankFile>,
}

/// Package-relative stem path for one cue layer.
pub fn stem_path_for_layer(cue_id: &str, layer_id: &str) -> String {
    format!("stems/{cue_id}_{layer_id}.wav")
}

/// Package-relative stem path for one pooled clip (`<n>` = index into the
/// event's `clip_ids`, matching the contract's `sfx/<event>_<n>.wav` layout).
pub fn stem_path_for_event_clip(event_id: &str, n: usize) -> String {
    format!("sfx/{event_id}_{n}.wav")
}

/// Package-relative filename for one bank's verbatim JSON.
pub fn bank_filename(bank_id: &str) -> String {
    format!("bank_{bank_id}.json")
}

/// Loop length of one layer in beats: the longest resolvable clip in the
/// layer (clips carry `length_beats`; the stem must cover the longest one),
/// defaulting to [`DEFAULT_LOOP_BEATS`] when nothing resolves.
pub fn layer_loop_beats(
    layer: &crate::game_audio::CueLayer,
    project: &Project,
) -> f64 {
    let mut beats = 0.0f64;
    for id in &layer.clip_ids {
        if let Some(clip) = project.clips.iter().find(|c| &c.id == id) {
            if clip.length_beats.is_finite() {
                beats = beats.max(clip.length_beats);
            }
        }
    }
    if beats > 0.0 {
        beats
    } else {
        DEFAULT_LOOP_BEATS
    }
}

/// Whole beats → whole samples at `tempo` BPM. Non-positive input → 0.
pub fn beats_to_samples(beats: f64, tempo: f64, sample_rate: u32) -> u64 {
    if !(beats > 0.0) || !(tempo > 0.0) || sample_rate == 0 {
        return 0;
    }
    (beats * 60.0 / tempo * sample_rate as f64).round() as u64
}

/// FNV-1a 64-bit: deterministic id → tone mapping without tables (same
/// construction as the audition renderer, kept local so this module needs
/// no new dependencies).
fn fnv1a64(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8482_22_25;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// Stand-in tone frequency for one clip id: 180–740 Hz, deterministic per id
/// (same range as the audition renderer so previews and exports agree).
fn clip_tone_freq_hz(clip_id: &str) -> f64 {
    180.0 + (fnv1a64(clip_id) % 561) as f64
}

/// Raised-cosine edge fade (5 ms by default at the caller rate) so stem
/// edges and one-shot tails never click.
fn edge_fade(t: usize, n: usize, fade: usize) -> f32 {
    if fade == 0 || n <= 1 {
        return 1.0;
    }
    let f = fade.min(n / 2).max(1);
    let smooth = |x: f32| x * x * (3.0 - 2.0 * x);
    if t < f {
        smooth(t as f32 / f as f32)
    } else if t >= n - f {
        smooth((n - 1 - t) as f32 / f as f32)
    } else {
        1.0
    }
}

/// Render one music-layer stem: a loop-clean reference tone at the cue tempo
/// (220 cycles per beat, the [`bounce`](crate::bounce) construction, so
/// whole-beat windows wrap without a click), shifted up by a deterministic
/// per-layer 0–4 semitones so layers are distinguishable, scaled by the
/// layer volume.
fn render_music_samples(
    layer_key: &str,
    loop_beats: f64,
    tempo: f64,
    sample_rate: u32,
    volume: f64,
) -> Vec<f32> {
    let n = beats_to_samples(loop_beats, tempo, sample_rate) as usize;
    if n == 0 {
        return Vec::new();
    }
    let semi = (fnv1a64(layer_key) % 5) as f64;
    let hz = 220.0 * (tempo / 60.0) * 2f64.powf(semi / 12.0);
    let sr = sample_rate as f64;
    let fade = (0.005 * sr) as usize;
    let gain = volume.max(0.0) as f32;
    (0..n)
        .map(|t| {
            let s = (0.5 * (2.0 * std::f64::consts::PI * hz * t as f64 / sr).sin()) as f32;
            s * gain * edge_fade(t, n, fade)
        })
        .collect()
}

/// Render one SFX one-shot stem: the per-clip stand-in tone shaped by the
/// event volume, with a seed-drawn +/-0.5 semitone detune so the seeded-RNG
/// path from validator rule 5 is real: same seed = same bytes, new seed =
/// audibly (and byte-) different renders.
fn render_sfx_samples(clip_id: &str, seed: u64, sample_rate: u32, volume: f64) -> Vec<f32> {
    let n = (SFX_ONESHOT_SECONDS * sample_rate as f64).round() as usize;
    if n == 0 {
        return Vec::new();
    }
    let mut rng = SeededRng::new(seed ^ fnv1a64(clip_id));
    let detune_semi = rng.range_f64(-0.5, 0.5);
    let hz = clip_tone_freq_hz(clip_id) * 2f64.powf(detune_semi / 12.0);
    let sr = sample_rate as f64;
    let fade = (0.005 * sr) as usize;
    let gain = volume.max(0.0) as f32;
    (0..n)
        .map(|t| {
            let s = (0.5 * (2.0 * std::f64::consts::PI * hz * t as f64 / sr).sin()) as f32;
            s * gain * edge_fade(t, n, fade)
        })
        .collect()
}

// -- build ---------------------------------------------------------------------

/// Mix addresses the export validator accepts as real
/// `target_node:target_param` values (validator rule 3): every device node's
/// own param ids, plus each track's `volume`/`pan` faders. Anything else is
/// a typo'd mix target and fails loudly (unknown *game params* stay quiet —
/// that tolerance lives in the trigger path, not here).
pub fn known_mix_targets(project: &Project) -> HashSet<(String, String)> {
    let mut known = HashSet::new();
    for dev in &project.devices {
        for p in &dev.params {
            known.insert((dev.id.clone(), p.id.clone()));
        }
    }
    for track in &project.tracks {
        known.insert((track.id.clone(), "volume".to_string()));
        known.insert((track.id.clone(), "pan".to_string()));
    }
    known
}

/// Static (no-files) validation of one export request: validator rules 1–3
/// plus tempo sanity. One message per violation; `Ok` carries the resolved
/// cues/banks in request order.
fn resolve_request<'a>(
    project: &Project,
    cues: &'a [AdaptiveCue],
    banks: &'a [SfxBank],
    params: &[GameStateParam],
    request: &ExportRequest,
) -> Result<(Vec<&'a AdaptiveCue>, Vec<&'a SfxBank>), Vec<String>> {
    let _ = params;
    let mut errors = Vec::new();
    let cue_by_id: HashMap<&str, &AdaptiveCue> =
        cues.iter().map(|c| (c.id.as_str(), c)).collect();
    let bank_by_id: HashMap<&str, &SfxBank> =
        banks.iter().map(|b| (b.id.as_str(), b)).collect();
    let known_clips: HashSet<&str> =
        project.clips.iter().map(|c| c.id.as_str()).collect();

    // Rule 1: every cue_ids / bank_ids entry resolves.
    let mut resolved_cues = Vec::new();
    for id in &request.cue_ids {
        match cue_by_id.get(id.as_str()) {
            Some(cue) => resolved_cues.push(*cue),
            None => errors.push(format!("unknown cue id '{id}'")),
        }
    }
    let mut resolved_banks = Vec::new();
    for id in &request.bank_ids {
        match bank_by_id.get(id.as_str()) {
            Some(bank) => resolved_banks.push(*bank),
            None => errors.push(format!("unknown bank id '{id}'")),
        }
    }

    // Rule 2 (cue half): every layer's clip_ids resolve to real clips.
    for cue in &resolved_cues {
        if !(cue.tempo.is_finite() && cue.tempo > 0.0) {
            errors.push(format!(
                "cue '{}' has non-positive tempo {}",
                cue.id, cue.tempo
            ));
        }
        for layer in &cue.layers {
            for clip in &layer.clip_ids {
                if !known_clips.contains(clip.as_str()) {
                    errors.push(format!(
                        "cue '{}' layer '{}' references unknown clip '{clip}'",
                        cue.id, layer.id
                    ));
                }
            }
        }
        // Rule 3 (cue half): Stinger rules name a real stinger cue, and only
        // Stinger rules may carry one (contract: non-empty iff Stinger).
        for rule in &cue.transitions {
            let is_stinger = rule.kind == TransitionKind::Stinger;
            if is_stinger {
                if rule.stinger_cue_id.is_empty() {
                    errors.push(format!(
                        "cue '{}' rule '{}' is Stinger but names no stinger cue",
                        cue.id, rule.id
                    ));
                } else if !cue_by_id.contains_key(rule.stinger_cue_id.as_str()) {
                    errors.push(format!(
                        "cue '{}' rule '{}' names unknown stinger cue '{}'",
                        cue.id, rule.id, rule.stinger_cue_id
                    ));
                }
            } else if !rule.stinger_cue_id.is_empty() {
                errors.push(format!(
                    "cue '{}' rule '{}' is {:?} but carries stinger cue '{}'",
                    cue.id, rule.id, rule.kind, rule.stinger_cue_id
                ));
            }
        }
    }

    // Rules 2 (bank half) + 3 (bank half): reuse the bank track's own
    // validators for the project-independent half, then check mix targets.
    let mix_targets = known_mix_targets(project);
    for bank in &resolved_banks {
        errors.extend(crate::sfx::bank::validate_bank(bank));
        errors.extend(crate::sfx::bank::resolve_against_project(bank, project));
        for event in &bank.events {
            for binding in &event.rtpc {
                if !mix_targets
                    .contains(&(binding.target_node.clone(), binding.target_param.clone()))
                {
                    errors.push(format!(
                        "bank '{}' event '{}' binds unknown mix target '{}:{}'",
                        bank.id, event.id, binding.target_node, binding.target_param
                    ));
                }
            }
        }
    }

    if errors.is_empty() {
        Ok((resolved_cues, resolved_banks))
    } else {
        Err(errors)
    }
}

/// Build an export package: validate rules 1–3, render every stem, assemble
/// the manifest. Returns one message per violation (no partial package) or
/// the built package ready for [`write_package`].
pub fn build_package(
    project: &Project,
    cues: &[AdaptiveCue],
    banks: &[SfxBank],
    params: &[GameStateParam],
    request: &ExportRequest,
    options: ExportOptions,
) -> Result<BuiltPackage, Vec<String>> {
    if options.sample_rate == 0 {
        return Err(vec!["sample_rate must be > 0".to_string()]);
    }
    let (resolved_cues, resolved_banks) =
        resolve_request(project, cues, banks, params, request)?;

    let mut stems = Vec::new();
    for cue in &resolved_cues {
        for layer in &cue.layers {
            let loop_beats = layer_loop_beats(layer, project);
            let samples = render_music_samples(
                &format!("{}:{}", cue.id, layer.id),
                loop_beats,
                cue.tempo,
                options.sample_rate,
                layer.volume,
            );
            let loop_end = beats_to_samples(loop_beats, cue.tempo, options.sample_rate) as u32;
            let stem = Stem {
                name: format!("{}_{}", cue.id, layer.id),
                sample_rate: options.sample_rate,
                samples,
                loop_start: 0,
                loop_end,
            };
            stems.push(RenderedStem {
                stem: ExportStem {
                    path: stem_path_for_layer(&cue.id, &layer.id),
                    source_id: cue.id.clone(),
                    source_layer_id: layer.id.clone(),
                    kind: StemKind::MusicLayer,
                    loop_start_beats: 0.0,
                    loop_end_beats: loop_beats,
                },
                wav_bytes: encode_wav(&stem),
            });
        }
    }
    for bank in &resolved_banks {
        for event in &bank.events {
            for (n, clip_id) in event.clip_ids.iter().enumerate() {
                let samples =
                    render_sfx_samples(clip_id, options.seed, options.sample_rate, event.volume);
                // One-shot: the manifest `0, 0` carries the loop, so the smpl
                // chunk is unused (encoded as 0, 0).
                let stem = Stem {
                    name: format!("{}_{n}", event.id),
                    sample_rate: options.sample_rate,
                    samples,
                    loop_start: 0,
                    loop_end: 0,
                };
                stems.push(RenderedStem {
                    stem: ExportStem {
                        path: stem_path_for_event_clip(&event.id, n),
                        source_id: event.id.clone(),
                        source_layer_id: String::new(),
                        kind: StemKind::SfxClip,
                        loop_start_beats: 0.0,
                        loop_end_beats: 0.0,
                    },
                    wav_bytes: encode_wav(&stem),
                });
            }
        }
    }

    let bank_files = resolved_banks
        .iter()
        .map(|bank| BankFile {
            id: bank.id.clone(),
            filename: bank_filename(&bank.id),
            json: crate::sfx::bank::bank_to_json(bank),
        })
        .collect::<Vec<_>>();
    let event_bank_path = bank_files
        .first()
        .map(|b| b.filename.clone())
        .unwrap_or_default();
    let package = ExportPackage {
        schema_version: GAME_AUDIO_SCHEMA_VERSION,
        name: request.name.clone(),
        cue_ids: request.cue_ids.clone(),
        bank_ids: request.bank_ids.clone(),
        stems: stems.iter().map(|s| s.stem.clone()).collect(),
        event_bank_path,
        validator_version: VALIDATOR_VERSION.to_string(),
    };
    Ok(BuiltPackage {
        package,
        stems,
        banks: bank_files,
    })
}

/// Write a built package to `dir` (created when missing): `package.json`,
/// every `bank_<id>.json`, and every stem WAV at its manifest path.
pub fn write_package(dir: &Path, built: &BuiltPackage) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    std::fs::write(
        dir.join("package.json"),
        serde_json::to_string_pretty(&built.package).expect("ExportPackage serializes"),
    )?;
    for bank in &built.banks {
        std::fs::write(dir.join(&bank.filename), &bank.json)?;
    }
    for stem in &built.stems {
        let path = dir.join(&stem.stem.path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, &stem.wav_bytes)?;
    }
    Ok(())
}

// -- validate ------------------------------------------------------------------

/// The validator's verdict: every rule violation as one message. Empty
/// `errors` = approved to ship.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationReport {
    pub errors: Vec<String>,
}

impl ValidationReport {
    pub fn is_ok(&self) -> bool {
        self.errors.is_empty()
    }
}

/// Read a written manifest back; mis-shaped JSON is an error string, never
/// a panic (packages arrive from disk and other tools).
pub fn read_manifest(dir: &Path) -> Result<ExportPackage, String> {
    let bytes = std::fs::read(dir.join("package.json"))
        .map_err(|e| format!("cannot read package.json: {e}"))?;
    let text =
        String::from_utf8(bytes).map_err(|e| format!("package.json is not UTF-8: {e}"))?;
    serde_json::from_str(&text).map_err(|e| format!("invalid package.json: {e}"))
}

/// Re-render the WAV bytes one manifest stem entry should hold, so the
/// validator can prove rule 5. Returns `None` when the entry's sources do
/// not resolve (that is already a rule 1/2 error; determinism stays silent
/// rather than double-reporting).
fn rerender_stem_bytes(
    entry: &ExportStem,
    cues: &[AdaptiveCue],
    banks: &[SfxBank],
    project: &Project,
    options: ExportOptions,
) -> Option<Vec<u8>> {
    match entry.kind {
        StemKind::MusicLayer => {
            let cue = cues.iter().find(|c| c.id == entry.source_id)?;
            let layer = cue.layers.iter().find(|l| l.id == entry.source_layer_id)?;
            let loop_beats = layer_loop_beats(layer, project);
            let samples = render_music_samples(
                &format!("{}:{}", cue.id, layer.id),
                loop_beats,
                cue.tempo,
                options.sample_rate,
                layer.volume,
            );
            Some(encode_wav(&Stem {
                name: String::new(),
                sample_rate: options.sample_rate,
                samples,
                loop_start: 0,
                loop_end: beats_to_samples(loop_beats, cue.tempo, options.sample_rate) as u32,
            }))
        }
        StemKind::SfxClip => {
            let bank = banks
                .iter()
                .find(|b| b.events.iter().any(|e| e.id == entry.source_id))?;
            let event = bank.events.iter().find(|e| e.id == entry.source_id)?;
            let n: usize = entry
                .path
                .rsplit('_')
                .next()?
                .strip_suffix(".wav")?
                .parse()
                .ok()?;
            let clip_id = event.clip_ids.get(n)?;
            let samples =
                render_sfx_samples(clip_id, options.seed, options.sample_rate, event.volume);
            Some(encode_wav(&Stem {
                name: String::new(),
                sample_rate: options.sample_rate,
                samples,
                loop_start: 0,
                loop_end: 0,
            }))
        }
    }
}

/// Validate a written package directory against the project it was exported
/// from. Implements every rule in `contracts/export-package.md`:
///
/// 1. `cue_ids`/`bank_ids` resolve; manifest versions are the v1 values.
/// 2. Layer/event clip pools resolve; no empty event pool; no duplicate
///    event ids (via the bank track's validators).
/// 3. Stinger rules name real cues (and only Stinger rules carry one);
///    every RTPC binding names a real `target_node:target_param`.
/// 4. Loop ranges (`0 <= start < end` for loops, `0, 0` for one-shots);
///    every stem file exists, parses as WAV at the expected rate, and music
///    stem lengths match their beat loop points at the cue tempo.
/// 5. Determinism: the first resolvable stem re-renders byte-identical under
///    `options.seed`.
///
/// `options.sample_rate` is the rate the package was rendered at (production
/// packages: [`EXPORT_SAMPLE_RATE`); `options.seed` the export seed.
pub fn validate_package(
    dir: &Path,
    project: &Project,
    cues: &[AdaptiveCue],
    banks: &[SfxBank],
    params: &[GameStateParam],
    options: ExportOptions,
) -> ValidationReport {
    let mut errors = Vec::new();
    let manifest = match read_manifest(dir) {
        Ok(m) => m,
        Err(e) => {
            return ValidationReport {
                errors: vec![e],
            }
        }
    };

    if manifest.schema_version != GAME_AUDIO_SCHEMA_VERSION {
        errors.push(format!(
            "package schema_version is {}, want {}",
            manifest.schema_version, GAME_AUDIO_SCHEMA_VERSION
        ));
    }
    if manifest.validator_version != VALIDATOR_VERSION {
        errors.push(format!(
            "package validator_version is '{}', want '{VALIDATOR_VERSION}'",
            manifest.validator_version
        ));
    }

    // Rules 1–3 run on the manifest's id lists (same resolver as the build).
    let request = ExportRequest::new(
        &manifest.name,
        manifest.cue_ids.clone(),
        manifest.bank_ids.clone(),
    );
    let (resolved_cues, resolved_banks) =
        match resolve_request(project, cues, banks, params, &request) {
            Ok(resolved) => resolved,
            Err(mut errs) => {
                errors.append(&mut errs);
                (Vec::new(), Vec::new())
            }
        };
    let cue_by_id: HashMap<&str, &AdaptiveCue> =
        resolved_cues.iter().map(|c| (c.id.as_str(), *c)).collect();

    // Bank files: one verbatim JSON per bank_ids entry, plus the manifest's
    // event_bank_path (the first bank's file in v1).
    for id in &manifest.bank_ids {
        let bytes = match std::fs::read(dir.join(bank_filename(id))) {
            Ok(b) => b,
            Err(_) => {
                errors.push(format!("missing bank file '{}'", bank_filename(id)));
                continue;
            }
        };
        let text = match String::from_utf8(bytes) {
            Ok(t) => t,
            Err(_) => {
                errors.push(format!("bank file '{}' is not UTF-8", bank_filename(id)));
                continue;
            }
        };
        match crate::sfx::bank::bank_from_json(&text) {
            Ok(parsed) if parsed.id == *id => {
                // Rule 2 on the shipped bytes (not the in-memory bank): the
                // file is what the engine loads, so empty pools, duplicate
                // event ids, and dangling clips fail here even when the
                // caller's bank list is clean.
                errors.extend(
                    crate::sfx::bank::validate_bank(&parsed)
                        .into_iter()
                        .map(|e| format!("bank file '{}': {e}", bank_filename(id))),
                );
                errors.extend(
                    crate::sfx::bank::resolve_against_project(&parsed, project)
                        .into_iter()
                        .map(|e| format!("bank file '{}': {e}", bank_filename(id))),
                );
            }
            Ok(parsed) => errors.push(format!(
                "bank file '{}' holds bank '{}', want '{id}'",
                bank_filename(id),
                parsed.id
            )),
            Err(e) => errors.push(format!("bank file '{}': {e}", bank_filename(id))),
        }
    }
    if !manifest.bank_ids.is_empty() {
        let want = bank_filename(&manifest.bank_ids[0]);
        if manifest.event_bank_path != want {
            errors.push(format!(
                "event_bank_path is '{}', want '{want}'",
                manifest.event_bank_path
            ));
        }
    }
    if !manifest.event_bank_path.is_empty() && !dir.join(&manifest.event_bank_path).exists() {
        errors.push(format!(
            "event_bank_path '{}' names a missing file",
            manifest.event_bank_path
        ));
    }

    // Expected stem set: one per cue layer + one per pooled event clip.
    let mut expected_paths = HashSet::new();
    for cue in &resolved_cues {
        for layer in &cue.layers {
            expected_paths.insert(stem_path_for_layer(&cue.id, &layer.id));
        }
    }
    for bank in &resolved_banks {
        for event in &bank.events {
            for n in 0..event.clip_ids.len() {
                expected_paths.insert(stem_path_for_event_clip(&event.id, n));
            }
        }
    }
    let listed_paths: HashSet<&str> =
        manifest.stems.iter().map(|s| s.path.as_str()).collect();
    for want in &expected_paths {
        if !listed_paths.contains(want.as_str()) {
            errors.push(format!("manifest omits expected stem '{want}'"));
        }
    }
    let mut seen_paths = HashSet::new();
    for entry in &manifest.stems {
        if !seen_paths.insert(entry.path.as_str()) {
            errors.push(format!("manifest lists stem '{}' twice", entry.path));
        }
        if !expected_paths.contains(entry.path.as_str()) {
            errors.push(format!(
                "manifest lists unexpected stem '{}' (no cue layer or event clip renders it)",
                entry.path
            ));
        }
    }

    // Rule 4: loop ranges + files that exist and parse as WAV.
    for entry in &manifest.stems {
        let finite = entry.loop_start_beats.is_finite() && entry.loop_end_beats.is_finite();
        match entry.kind {
            StemKind::MusicLayer => {
                if !finite
                    || !(0.0 <= entry.loop_start_beats && entry.loop_start_beats < entry.loop_end_beats)
                {
                    errors.push(format!(
                        "stem '{}' has bad music loop {}..{} (want 0 <= start < end)",
                        entry.path, entry.loop_start_beats, entry.loop_end_beats
                    ));
                }
            }
            StemKind::SfxClip => {
                if entry.loop_start_beats != 0.0 || entry.loop_end_beats != 0.0 {
                    errors.push(format!(
                        "stem '{}' is a one-shot but carries loop {}..{} (want 0, 0)",
                        entry.path, entry.loop_start_beats, entry.loop_end_beats
                    ));
                }
                if !entry.source_layer_id.is_empty() {
                    errors.push(format!(
                        "stem '{}' is an SFX stem but names layer '{}'",
                        entry.path, entry.source_layer_id
                    ));
                }
            }
        }
        let bytes = match std::fs::read(dir.join(&entry.path)) {
            Ok(b) => b,
            Err(_) => {
                errors.push(format!("missing stem file '{}'", entry.path));
                continue;
            }
        };
        let decoded = match decode_wav(&bytes) {
            Ok(d) => d,
            Err(e) => {
                errors.push(format!("stem '{}' does not parse as WAV: {e}", entry.path));
                continue;
            }
        };
        if decoded.sample_rate != options.sample_rate {
            errors.push(format!(
                "stem '{}' is {} Hz, want {} Hz",
                entry.path, decoded.sample_rate, options.sample_rate
            ));
        }
        // Music stems: the file length must equal the beat loop rendered at
        // the cue tempo; SFX stems: exactly the one-shot second.
        match entry.kind {
            StemKind::MusicLayer => {
                if let Some(cue) = cue_by_id.get(entry.source_id.as_str()) {
                    let want =
                        beats_to_samples(entry.loop_end_beats - entry.loop_start_beats, cue.tempo, options.sample_rate)
                            as usize;
                    if decoded.samples.len() != want {
                        errors.push(format!(
                            "stem '{}' holds {} samples, loop {}..{} beats at {} BPM wants {want}",
                            entry.path,
                            decoded.samples.len(),
                            entry.loop_start_beats,
                            entry.loop_end_beats,
                            cue.tempo
                        ));
                    }
                }
            }
            StemKind::SfxClip => {
                let want = (SFX_ONESHOT_SECONDS * options.sample_rate as f64).round() as usize;
                if decoded.samples.len() != want {
                    errors.push(format!(
                        "stem '{}' holds {} samples, one-shot wants {want}",
                        entry.path,
                        decoded.samples.len()
                    ));
                }
            }
        }
    }

    // Rule 5: re-render the first resolvable stem; bytes must match.
    if let Some(entry) = manifest
        .stems
        .iter()
        .find(|s| rerender_stem_bytes(s, cues, banks, project, options).is_some())
    {
        let want = rerender_stem_bytes(entry, cues, banks, project, options).expect("found");
        match std::fs::read(dir.join(&entry.path)) {
            Ok(have) if have == want => {}
            Ok(_) => errors.push(format!(
                "stem '{}' is not deterministic: re-render with seed {} differs",
                entry.path, options.seed
            )),
            Err(_) => {} // missing file already reported above
        }
    }

    ValidationReport { errors }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game_audio::{
        CueLayer, RtpcBinding, SfxEvent, TransitionRule, GAME_AUDIO_SCHEMA_VERSION,
    };
    use crate::model::{Clip, ClipKind, Node, NodeKind, Param};
    use std::sync::atomic::{AtomicU64, Ordering};

    static GAEXPORT_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn scratch(name: &str) -> std::path::PathBuf {
        let n = GAEXPORT_COUNTER.fetch_add(1, Ordering::SeqCst);
        std::env::temp_dir().join(format!("ccez-gaexport-{name}-{}-{n}", std::process::id()))
    }

    fn options() -> ExportOptions {
        // 8 kHz keeps test renders small; production uses EXPORT_SAMPLE_RATE.
        ExportOptions {
            sample_rate: 8000,
            seed: DEFAULT_EXPORT_SEED,
        }
    }

    fn clip(id: &str, length_beats: f64) -> Clip {
        Clip {
            id: id.to_string(),
            track_id: "trk_1".to_string(),
            name: id.to_string(),
            start_beats: 0.0,
            length_beats,
            kind: ClipKind::Audio,
            source: format!("samples/{id}.wav"),
        }
    }

    fn demo_project() -> Project {
        let mut p = Project::new("proj_demo", "Demo");
        p.tempo = 120.0;
        p.clips.push(clip("clip_bed", 8.0));
        p.clips.push(clip("clip_drums", 4.0));
        p.clips.push(clip("clip_step_a", 1.0));
        p.clips.push(clip("clip_step_b", 1.0));
        // The one real mix target the demo bank drives.
        p.devices.push(Node {
            id: "bus_sfx".to_string(),
            kind: NodeKind::Bus,
            name: "SFX bus".to_string(),
            params: vec![Param {
                id: "volume".to_string(),
                label: "Volume".to_string(),
                value: 0.8,
                min: 0.0,
                max: 1.0,
                default: 0.8,
                unit: String::new(),
            }],
        });
        p
    }

    fn demo_cue() -> AdaptiveCue {
        let mut cue = AdaptiveCue::new("cue_fight", "Fight");
        cue.tempo = 120.0;
        cue.default_state = "explore".to_string();
        cue.layers.push(CueLayer {
            id: "bed".to_string(),
            name: "Bed".to_string(),
            clip_ids: vec!["clip_bed".to_string()],
            states: vec![],
            volume: 0.8,
        });
        cue.layers.push(CueLayer {
            id: "drums".to_string(),
            name: "Drums".to_string(),
            clip_ids: vec!["clip_drums".to_string()],
            states: vec!["combat".to_string()],
            volume: 0.9,
        });
        cue.transitions.push(TransitionRule {
            id: "t1".to_string(),
            from_state: "explore".to_string(),
            to_state: "combat".to_string(),
            kind: TransitionKind::Fade,
            fade_beats: 2.0,
            stinger_cue_id: String::new(),
        });
        cue
    }

    fn demo_bank() -> SfxBank {
        SfxBank {
            schema_version: GAME_AUDIO_SCHEMA_VERSION,
            id: "bank_ui".to_string(),
            name: "UI".to_string(),
            events: vec![SfxEvent {
                id: "player.footstep".to_string(),
                name: "Footstep".to_string(),
                clip_ids: vec!["clip_step_a".to_string(), "clip_step_b".to_string()],
                volume: 0.7,
                volume_random: 0.05,
                pitch_random: 1.0,
                cooldown_ms: 50,
                max_polyphony: 4,
                rtpc: vec![RtpcBinding {
                    param: "threat".to_string(),
                    target_node: "bus_sfx".to_string(),
                    target_param: "volume".to_string(),
                    min: 0.5,
                    max: 1.0,
                }],
            }],
        }
    }

    fn demo_params() -> Vec<GameStateParam> {
        vec![GameStateParam {
            id: "threat".to_string(),
            label: "Threat".to_string(),
            min: 0.0,
            max: 1.0,
            default: 0.0,
            unit: String::new(),
        }]
    }

    fn demo_request() -> ExportRequest {
        ExportRequest::new(
            "demo-v1",
            vec!["cue_fight".to_string()],
            vec!["bank_ui".to_string()],
        )
    }

    fn clean_package(dir: &std::path::Path, options: ExportOptions) -> ExportPackage {
        let project = demo_project();
        let cues = vec![demo_cue()];
        let banks = vec![demo_bank()];
        let built = build_package(&project, &cues, &banks, &demo_params(), &demo_request(), options)
            .expect("demo export builds");
        write_package(dir, &built).expect("write package");
        built.package
    }

    #[test]
    fn export_then_validate_approves_a_clean_package() {
        let dir = scratch("clean");
        let _ = std::fs::remove_dir_all(&dir);
        let options = options();
        let manifest = clean_package(&dir, options);
        assert_eq!(manifest.schema_version, GAME_AUDIO_SCHEMA_VERSION);
        assert_eq!(manifest.validator_version, VALIDATOR_VERSION);
        assert_eq!(manifest.event_bank_path, "bank_bank_ui.json");
        // Two music stems + two pooled SFX one-shots.
        assert_eq!(manifest.stems.len(), 4, "{manifest:?}");

        // Music loop points come from the longest layer clip: bed 8 beats.
        let bed = manifest
            .stems
            .iter()
            .find(|s| s.path == "stems/cue_fight_bed.wav")
            .expect("bed stem");
        assert_eq!((bed.loop_start_beats, bed.loop_end_beats), (0.0, 8.0));
        // SFX stems are one-shots.
        for s in manifest.stems.iter().filter(|s| s.kind == StemKind::SfxClip) {
            assert_eq!((s.loop_start_beats, s.loop_end_beats), (0.0, 0.0));
            assert!(s.source_layer_id.is_empty());
        }

        // Bank JSON is verbatim: re-serializing the parse is byte-stable.
        let bank_text =
            std::fs::read_to_string(dir.join("bank_bank_ui.json")).expect("bank file");
        let parsed: SfxBank = serde_json::from_str(&bank_text).expect("bank parses");
        assert_eq!(parsed, demo_bank());
        assert_eq!(crate::sfx::bank::bank_to_json(&parsed), bank_text);

        // One music WAV decodes to exactly its beat loop at the cue tempo.
        let wav = std::fs::read(dir.join("stems/cue_fight_bed.wav")).expect("bed wav");
        let decoded = decode_wav(&wav).expect("bed parses as WAV");
        assert_eq!(decoded.sample_rate, 8000);
        assert_eq!(decoded.samples.len(), (8.0 * 60.0 / 120.0 * 8000.0) as usize);

        let report = validate_package(
            &dir,
            &demo_project(),
            &[demo_cue()],
            &[demo_bank()],
            &demo_params(),
            options,
        );
        assert!(report.is_ok(), "clean package must validate: {:?}", report.errors);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rule1_unknown_cue_and_bank_ids_fail() {
        let project = demo_project();
        let err = build_package(
            &project,
            &[demo_cue()],
            &[demo_bank()],
            &demo_params(),
            &ExportRequest::new("bad", vec!["cue_nope".to_string()], vec!["bank_nope".to_string()]),
            options(),
        )
        .expect_err("unknown ids must not build");
        assert!(err.iter().any(|e| e.contains("cue_nope")), "{err:?}");
        assert!(err.iter().any(|e| e.contains("bank_nope")), "{err:?}");

        // The written-package validator reports the same rule, not a panic.
        let dir = scratch("rule1");
        let _ = std::fs::remove_dir_all(&dir);
        clean_package(&dir, options());
        let mut manifest = read_manifest(&dir).expect("manifest");
        manifest.cue_ids = vec!["cue_nope".to_string()];
        std::fs::write(
            dir.join("package.json"),
            serde_json::to_string_pretty(&manifest).expect("serialize"),
        )
        .expect("rewrite manifest");
        let report = validate_package(
            &dir,
            &demo_project(),
            &[demo_cue()],
            &[demo_bank()],
            &demo_params(),
            options(),
        );
        assert!(!report.is_ok());
        assert!(report.errors.iter().any(|e| e.contains("cue_nope")), "{:?}", report.errors);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rule2_dangling_clips_empty_pools_and_dupes_fail() {
        // Build side: a dangling layer clip is one message.
        let mut cue = demo_cue();
        cue.layers[0].clip_ids.push("clip_ghost".to_string());
        let err = build_package(
            &demo_project(),
            &[cue],
            &[demo_bank()],
            &demo_params(),
            &demo_request(),
            options(),
        )
        .expect_err("dangling layer clip must not build");
        assert!(err.iter().any(|e| e.contains("clip_ghost")), "{err:?}");

        // Written side: a duplicated event in the bank file fails validation.
        let dir = scratch("rule2");
        let _ = std::fs::remove_dir_all(&dir);
        clean_package(&dir, options());
        let bank_text =
            std::fs::read_to_string(dir.join("bank_bank_ui.json")).expect("bank file");
        let mut value: serde_json::Value = serde_json::from_str(&bank_text).expect("json");
        let first = value["events"][0].clone();
        value["events"].as_array_mut().expect("array").push(first);
        std::fs::write(
            dir.join("bank_bank_ui.json"),
            serde_json::to_string_pretty(&value).expect("serialize"),
        )
        .expect("rewrite bank");
        let report = validate_package(
            &dir,
            &demo_project(),
            &[demo_cue()],
            &[demo_bank()],
            &demo_params(),
            options(),
        );
        assert!(!report.is_ok());
        assert!(report.errors.iter().any(|e| e.contains("duplicate")), "{:?}", report.errors);

        // Empty pools never validate either (checked without a project).
        let mut bank = demo_bank();
        bank.events[0].clip_ids.clear();
        assert!(
            !crate::sfx::bank::validate_bank(&bank).is_empty(),
            "empty pool must fail validate_bank"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rule3_bad_stinger_and_typod_mix_target_fail() {
        // Stinger naming a missing cue.
        let mut cue = demo_cue();
        cue.transitions.push(TransitionRule {
            id: "t_sting".to_string(),
            from_state: "combat".to_string(),
            to_state: "explore".to_string(),
            kind: TransitionKind::Stinger,
            fade_beats: 0.0,
            stinger_cue_id: "cue_missing".to_string(),
        });
        let err = build_package(
            &demo_project(),
            &[cue],
            &[demo_bank()],
            &demo_params(),
            &demo_request(),
            options(),
        )
        .expect_err("unknown stinger cue must not build");
        assert!(err.iter().any(|e| e.contains("cue_missing")), "{err:?}");

        // A non-Stinger rule carrying a stinger id (contract: iff).
        let mut cue = demo_cue();
        cue.transitions[0].stinger_cue_id = "cue_fight".to_string();
        let err = build_package(
            &demo_project(),
            &[cue],
            &[demo_bank()],
            &demo_params(),
            &demo_request(),
            options(),
        )
        .expect_err("stinger id on a Fade must not build");
        assert!(err.iter().any(|e| e.contains("t1")), "{err:?}");

        // RTPC binding to a typo'd mix address.
        let mut bank = demo_bank();
        bank.events[0].rtpc[0].target_node = "bus_typo".to_string();
        let err = build_package(
            &demo_project(),
            &[demo_cue()],
            &[bank],
            &demo_params(),
            &demo_request(),
            options(),
        )
        .expect_err("unknown mix target must not build");
        assert!(err.iter().any(|e| e.contains("bus_typo")), "{err:?}");
    }

    #[test]
    fn rule4_bad_loops_missing_and_unparsable_stems_fail() {
        // A music stem carrying a one-shot loop.
        let dir = scratch("rule4loop");
        let _ = std::fs::remove_dir_all(&dir);
        clean_package(&dir, options());
        let mut manifest = read_manifest(&dir).expect("manifest");
        manifest.stems[0].loop_end_beats = 0.0;
        std::fs::write(
            dir.join("package.json"),
            serde_json::to_string_pretty(&manifest).expect("serialize"),
        )
        .expect("rewrite manifest");
        let report = validate_package(
            &dir,
            &demo_project(),
            &[demo_cue()],
            &[demo_bank()],
            &demo_params(),
            options(),
        );
        assert!(report.errors.iter().any(|e| e.contains("bad music loop")), "{:?}", report.errors);

        // A deleted stem file.
        let dir = scratch("rule4missing");
        let _ = std::fs::remove_dir_all(&dir);
        clean_package(&dir, options());
        let manifest = read_manifest(&dir).expect("manifest");
        std::fs::remove_file(dir.join(&manifest.stems[0].path)).expect("delete stem");
        let report = validate_package(
            &dir,
            &demo_project(),
            &[demo_cue()],
            &[demo_bank()],
            &demo_params(),
            options(),
        );
        assert!(report.errors.iter().any(|e| e.contains("missing stem")), "{:?}", report.errors);

        // A stem that is not a WAV at all.
        let dir = scratch("rule4parse");
        let _ = std::fs::remove_dir_all(&dir);
        clean_package(&dir, options());
        let manifest = read_manifest(&dir).expect("manifest");
        std::fs::write(dir.join(&manifest.stems[1].path), b"definitely not audio")
            .expect("clobber stem");
        let report = validate_package(
            &dir,
            &demo_project(),
            &[demo_cue()],
            &[demo_bank()],
            &demo_params(),
            options(),
        );
        assert!(
            report.errors.iter().any(|e| e.contains("does not parse as WAV")),
            "{:?}",
            report.errors
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rule5_same_seed_is_byte_identical_and_tampering_fails() {
        let project = demo_project();
        let cues = vec![demo_cue()];
        let banks = vec![demo_bank()];
        let a = build_package(&project, &cues, &banks, &demo_params(), &demo_request(), options())
            .expect("build a");
        let b = build_package(&project, &cues, &banks, &demo_params(), &demo_request(), options())
            .expect("build b");
        assert_eq!(a, b, "same project + seed must render identical packages");
        // A new seed audibly changes the SFX stems (the seeded-RNG path).
        let mut other_options = options();
        other_options.seed = 99;
        let c = build_package(&project, &cues, &banks, &demo_params(), &demo_request(), other_options)
            .expect("build c");
        assert_ne!(
            a.stems.iter().find(|s| s.stem.kind == StemKind::SfxClip).map(|s| &s.wav_bytes),
            c.stems.iter().find(|s| s.stem.kind == StemKind::SfxClip).map(|s| &s.wav_bytes),
            "a new seed must change SFX bytes"
        );

        // A single flipped sample byte still parses — but fails determinism.
        let dir = scratch("rule5");
        let _ = std::fs::remove_dir_all(&dir);
        write_package(&dir, &a).expect("write");
        let manifest = read_manifest(&dir).expect("manifest");
        let music_path = manifest
            .stems
            .iter()
            .find(|s| s.kind == StemKind::MusicLayer)
            .map(|s| s.path.clone())
            .expect("music stem");
        let full = dir.join(&music_path);
        let mut bytes = std::fs::read(&full).expect("read stem");
        assert!(bytes.len() > 200, "stem must hold samples");
        bytes[100] ^= 0xFF;
        std::fs::write(&full, &bytes).expect("rewrite stem");
        let report =
            validate_package(&dir, &project, &cues, &banks, &demo_params(), options());
        assert!(
            report.errors.iter().any(|e| e.contains("not deterministic")),
            "{:?}",
            report.errors
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn malformed_manifest_is_an_error_not_a_panic() {
        let dir = scratch("rule0");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        std::fs::write(dir.join("package.json"), "{not json").expect("write");
        assert!(read_manifest(&dir).is_err());
        let report = validate_package(
            &dir,
            &demo_project(),
            &[demo_cue()],
            &[demo_bank()],
            &demo_params(),
            options(),
        );
        assert!(!report.is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
