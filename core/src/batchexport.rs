//! S-4 batch stem variants: one-pass export of every layer/state mix.
//!
//! Teaching note: a GA-4 package ships one stem per cue layer, and the game
//! mixes layers live per state. That is flexible but means N mixes to get
//! right by hand. The batch export renders **one mix WAV per (cue, state)**
//! for the whole TP-like state set (`field/combat/dungeon/boss/village/`
//! `night`) in a single pass, next to the base package, under one manifest
//! (`batch.json`). The game can drop a state mix in directly; the per-layer
//! stems stay available for live crossfades.
//!
//! Reuse, not reinvention:
//!
//! - The base package is built by [`gaexport`](crate::gaexport) — same
//!   request shape, same deterministic renders, same validator.
//! - Variant mixes are **sums of the shipped layer stems** (decoded with the
//!   [`bounce`](crate::bounce) WAV codec, summed in f32, re-encoded with the
//!   same codec). A variant can never disagree with its layers: the bytes
//!   the game loops are arithmetic over the bytes the validator approved.
//! - Shorter layers wrap (repeat) to the mix length, which is the longest
//!   audible layer's loop. Layers are whole-beat loops of the same tempo,
//!   so wrapping stays on the grid.
//! - A state with no audible layers still gets a variant: digital silence of
//!   [`DEFAULT_LOOP_BEATS`](crate::gaexport::DEFAULT_LOOP_BEATS) at the cue
//!   tempo. The manifest stays complete (every cue × every state), and the
//!   game treats "no music here" as data, not a missing file.
//!
//! Frozen-shape discipline: `BatchManifest`/`BatchVariant` are caller-side
//! documents (like `ExportRequest`), never added to the `emit` typegen
//! surface, so `ui/src/generated/*.ts` is untouched and
//! `bun run typegen -- --check` stays green.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::bounce::{decode_wav, encode_wav, Stem};
use crate::gaexport::{
    self, beats_to_samples, layer_loop_beats, BuiltPackage, ExportOptions, ExportRequest,
    DEFAULT_LOOP_BEATS, VALIDATOR_VERSION,
};
use crate::game_audio::{AdaptiveCue, GameStateParam, SfxBank, GAME_AUDIO_SCHEMA_VERSION};
use crate::model::Project;
use serde::{Deserialize, Serialize};

/// The TP-like game state set batch exports cover by default.
pub const BATCH_STATES: [&str; 6] = [
    "field", "combat", "dungeon", "boss", "village", "night",
];
/// Batch manifest filename (package-relative, next to `package.json`).
pub const BATCH_MANIFEST_FILENAME: &str = "batch.json";
/// Package-relative directory holding one mix WAV per (cue, state).
pub const BATCH_VARIANTS_DIR: &str = "variants";

/// Default batch state list (a fresh `Vec` over [`BATCH_STATES`]).
pub fn default_states() -> Vec<String> {
    BATCH_STATES.iter().map(|s| s.to_string()).collect()
}

/// Package-relative mix path for one (cue, state) variant.
pub fn variant_path_for(cue_id: &str, state: &str) -> String {
    format!("{BATCH_VARIANTS_DIR}/{cue_id}_{state}.wav")
}

/// What to batch-export: a name plus the cue/bank ids (mirroring the frozen
/// `gameaudio_export` input) plus the states to render a mix per cue for.
#[derive(Debug, Clone, PartialEq)]
pub struct BatchRequest {
    pub name: String,
    pub cue_ids: Vec<String>,
    pub bank_ids: Vec<String>,
    pub states: Vec<String>,
}

impl BatchRequest {
    pub fn new(
        name: &str,
        cue_ids: Vec<String>,
        bank_ids: Vec<String>,
        states: Vec<String>,
    ) -> Self {
        Self {
            name: name.to_string(),
            cue_ids,
            bank_ids,
            states,
        }
    }

    /// Request over the default TP-like state set.
    pub fn with_default_states(name: &str, cue_ids: Vec<String>, bank_ids: Vec<String>) -> Self {
        Self::new(name, cue_ids, bank_ids, default_states())
    }
}

/// One state mix inside a batch manifest: which cue/state it renders, where
/// the WAV lives, which layers were audible (cue order), and the mix loop.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BatchVariant {
    pub cue_id: String,
    pub state: String,
    pub path: String,
    pub layer_ids: Vec<String>,
    pub loop_end_beats: f64,
}

/// The batch manifest (`batch.json`): the base GA-4 package verbatim plus
/// the per-(cue, state) variant list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BatchManifest {
    pub schema_version: u32,
    pub name: String,
    pub states: Vec<String>,
    pub package: crate::game_audio::ExportPackage,
    pub variants: Vec<BatchVariant>,
    pub validator_version: String,
}

/// One rendered variant mix plus the exact WAV bytes written for it.
#[derive(Debug, Clone, PartialEq)]
pub struct RenderedVariant {
    pub variant: BatchVariant,
    pub wav_bytes: Vec<u8>,
}

/// A fully built (not yet written) batch: manifest + base GA-4 package +
/// variant mixes.
#[derive(Debug, Clone, PartialEq)]
pub struct BuiltBatch {
    pub manifest: BatchManifest,
    pub base: BuiltPackage,
    pub variants: Vec<RenderedVariant>,
}

/// Layer ids audible in `state` for one cue, in cue order.
fn audible_layer_ids(cue: &AdaptiveCue, state: &str) -> Vec<String> {
    cue.layers_for_state(state)
        .into_iter()
        .map(|l| l.id.clone())
        .collect()
}

/// Mix loop for one variant: the longest audible layer loop, or
/// [`DEFAULT_LOOP_BEATS`] when no layer is audible (silent variant).
fn variant_loop_beats(cue: &AdaptiveCue, state: &str, project: &Project) -> f64 {
    let mut beats = 0.0f64;
    for layer in cue.layers_for_state(state) {
        beats = beats.max(layer_loop_beats(layer, project));
    }
    if beats > 0.0 {
        beats
    } else {
        DEFAULT_LOOP_BEATS
    }
}

/// Sum decoded layer stems into one mix of `n` samples. Shorter layers wrap
/// (repeat from their own start); an empty input renders silence.
fn mix_layers(stems: &[Vec<f32>], n: usize) -> Vec<f32> {
    let mut out = vec![0.0f32; n];
    if n == 0 {
        return out;
    }
    for stem in stems {
        if stem.is_empty() {
            continue;
        }
        for (i, dst) in out.iter_mut().enumerate() {
            *dst += stem[i % stem.len()];
        }
    }
    for s in out.iter_mut() {
        *s = s.clamp(-1.0, 1.0);
    }
    out
}

/// Build a batch: validate + render the base GA-4 package, then render one
/// mix per (cue, state) as the sample-sum of that variant's audible layer
/// stems. Returns one message per violation (no partial batch).
pub fn build_batch(
    project: &Project,
    cues: &[AdaptiveCue],
    banks: &[SfxBank],
    params: &[GameStateParam],
    request: &BatchRequest,
    options: ExportOptions,
) -> Result<BuiltBatch, Vec<String>> {
    if request.states.is_empty() {
        return Err(vec!["batch request needs at least one state".to_string()]);
    }
    let base_request = ExportRequest::new(&request.name, request.cue_ids.clone(), request.bank_ids.clone());
    let base = gaexport::build_package(project, cues, banks, params, &base_request, options)?;
    let cue_by_id: HashMap<&str, &AdaptiveCue> =
        cues.iter().map(|c| (c.id.as_str(), c)).collect();
    // Index base WAV bytes by manifest stem path so mixes sum shipped bytes.
    let wav_by_path: HashMap<&str, &[u8]> = base
        .stems
        .iter()
        .map(|s| (s.stem.path.as_str(), s.wav_bytes.as_slice()))
        .collect();

    let mut variants = Vec::new();
    for cue_id in &request.cue_ids {
        let cue = cue_by_id
            .get(cue_id.as_str())
            .expect("base build resolved every cue id");
        for state in &request.states {
            let layer_ids = audible_layer_ids(cue, state);
            let loop_beats = variant_loop_beats(cue, state, project);
            let n = beats_to_samples(loop_beats, cue.tempo, options.sample_rate) as usize;
            let mut decoded: Vec<Vec<f32>> = Vec::with_capacity(layer_ids.len());
            for layer_id in &layer_ids {
                let path = gaexport::stem_path_for_layer(&cue.id, layer_id);
                let bytes = wav_by_path.get(path.as_str()).expect("base build renders every layer");
                let stem = decode_wav(bytes)
                    .map_err(|e| vec![format!("base stem '{path}' does not decode: {e}")])?;
                decoded.push(stem.samples);
            }
            let samples = mix_layers(&decoded, n);
            let loop_end = beats_to_samples(loop_beats, cue.tempo, options.sample_rate) as u32;
            let wav_bytes = encode_wav(&Stem {
                name: format!("{cue_id}_{state}"),
                sample_rate: options.sample_rate,
                samples,
                loop_start: 0,
                loop_end,
            });
            variants.push(RenderedVariant {
                variant: BatchVariant {
                    cue_id: cue_id.clone(),
                    state: state.clone(),
                    path: variant_path_for(cue_id, state),
                    layer_ids,
                    loop_end_beats: loop_beats,
                },
                wav_bytes,
            });
        }
    }

    let manifest = BatchManifest {
        schema_version: GAME_AUDIO_SCHEMA_VERSION,
        name: request.name.clone(),
        states: request.states.clone(),
        package: base.package.clone(),
        variants: variants.iter().map(|v| v.variant.clone()).collect(),
        validator_version: VALIDATOR_VERSION.to_string(),
    };
    Ok(BuiltBatch {
        manifest,
        base,
        variants,
    })
}

/// Write a built batch to `dir`: the base GA-4 package files, every variant
/// mix at its manifest path, and `batch.json`.
pub fn write_batch(dir: &Path, built: &BuiltBatch) -> std::io::Result<()> {
    gaexport::write_package(dir, &built.base)?;
    for rendered in &built.variants {
        let path = dir.join(&rendered.variant.path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, &rendered.wav_bytes)?;
    }
    std::fs::write(
        dir.join(BATCH_MANIFEST_FILENAME),
        serde_json::to_string_pretty(&built.manifest).expect("BatchManifest serializes"),
    )?;
    Ok(())
}

/// Read a written batch manifest back; mis-shaped JSON is an error string,
/// never a panic.
pub fn read_batch_manifest(dir: &Path) -> Result<BatchManifest, String> {
    let bytes = std::fs::read(dir.join(BATCH_MANIFEST_FILENAME))
        .map_err(|e| format!("cannot read {BATCH_MANIFEST_FILENAME}: {e}"))?;
    let text = String::from_utf8(bytes)
        .map_err(|e| format!("{BATCH_MANIFEST_FILENAME} is not UTF-8: {e}"))?;
    serde_json::from_str(&text).map_err(|e| format!("invalid {BATCH_MANIFEST_FILENAME}: {e}"))
}

/// The batch validator's verdict: every rule violation as one message.
/// Empty `errors` = approved to ship.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchReport {
    pub errors: Vec<String>,
}

impl BatchReport {
    pub fn is_ok(&self) -> bool {
        self.errors.is_empty()
    }
}

/// Validate a written batch directory:
/// base GA-4 package approves clean, then batch completeness —
/// every (cue × state) variant listed exactly once with the audible layer
/// set, every file present and parsing as WAV at the export rate with the
/// loop-length sample count, and the first variant re-mixed byte-identical
/// from the shipped layer stems.
pub fn validate_batch(
    dir: &Path,
    project: &Project,
    cues: &[AdaptiveCue],
    banks: &[SfxBank],
    params: &[GameStateParam],
    options: ExportOptions,
) -> BatchReport {
    let mut errors = Vec::new();

    // Base package first: a batch over a broken base is red, attributed.
    for e in gaexport::validate_package(dir, project, cues, banks, params, options).errors {
        errors.push(format!("base package: {e}"));
    }

    let manifest = match read_batch_manifest(dir) {
        Ok(m) => m,
        Err(e) => {
            errors.push(e);
            return BatchReport { errors };
        }
    };
    if manifest.schema_version != GAME_AUDIO_SCHEMA_VERSION {
        errors.push(format!(
            "batch schema_version is {}, want {}",
            manifest.schema_version, GAME_AUDIO_SCHEMA_VERSION
        ));
    }
    if manifest.validator_version != VALIDATOR_VERSION {
        errors.push(format!(
            "batch validator_version is '{}', want '{VALIDATOR_VERSION}'",
            manifest.validator_version
        ));
    }

    let cue_by_id: HashMap<&str, &AdaptiveCue> =
        cues.iter().map(|c| (c.id.as_str(), c)).collect();
    let mut expected: HashSet<(String, String)> = HashSet::new();
    for cue_id in &manifest.package.cue_ids {
        for state in &manifest.states {
            expected.insert((cue_id.clone(), state.clone()));
        }
    }
    let mut seen: HashSet<(String, String)> = HashSet::new();
    for v in &manifest.variants {
        let key = (v.cue_id.clone(), v.state.clone());
        if !seen.insert(key.clone()) {
            errors.push(format!(
                "batch lists variant '{}:{}' twice",
                v.cue_id, v.state
            ));
        }
        if !expected.contains(&key) {
            errors.push(format!(
                "batch lists unexpected variant '{}:{}' (no cue × state renders it)",
                v.cue_id, v.state
            ));
        }
        // Layer set must equal the audible set in cue order.
        match cue_by_id.get(v.cue_id.as_str()) {
            Some(cue) => {
                let want = audible_layer_ids(cue, &v.state);
                if v.layer_ids != want {
                    errors.push(format!(
                        "variant '{}:{}' names layers [{}], want [{}]",
                        v.cue_id,
                        v.state,
                        v.layer_ids.join(", "),
                        want.join(", ")
                    ));
                }
                if !(v.loop_end_beats.is_finite() && v.loop_end_beats > 0.0) {
                    errors.push(format!(
                        "variant '{}:{}' has bad loop_end_beats {} (want finite > 0)",
                        v.cue_id, v.state, v.loop_end_beats
                    ));
                }
            }
            None => errors.push(format!(
                "variant '{}:{}' names unknown cue '{}'",
                v.cue_id, v.state, v.cue_id
            )),
        }
        // Path must be the canonical one for the pair.
        if v.path != variant_path_for(&v.cue_id, &v.state) {
            errors.push(format!(
                "variant '{}:{}' lives at '{}', want '{}'",
                v.cue_id,
                v.state,
                v.path,
                variant_path_for(&v.cue_id, &v.state)
            ));
        }
    }
    for (cue_id, state) in &expected {
        if !seen.contains(&(cue_id.clone(), state.clone())) {
            errors.push(format!("batch omits expected variant '{cue_id}:{state}'"));
        }
    }

    // Files: present, parse as WAV at the export rate, loop-length samples.
    for v in &manifest.variants {
        let bytes = match std::fs::read(dir.join(&v.path)) {
            Ok(b) => b,
            Err(_) => {
                errors.push(format!("missing variant file '{}'", v.path));
                continue;
            }
        };
        let decoded = match decode_wav(&bytes) {
            Ok(d) => d,
            Err(e) => {
                errors.push(format!("variant '{}' does not parse as WAV: {e}", v.path));
                continue;
            }
        };
        if decoded.sample_rate != options.sample_rate {
            errors.push(format!(
                "variant '{}' is {} Hz, want {} Hz",
                v.path, decoded.sample_rate, options.sample_rate
            ));
        }
        if let Some(cue) = cue_by_id.get(v.cue_id.as_str()) {
            let want = beats_to_samples(v.loop_end_beats, cue.tempo, options.sample_rate) as usize;
            if decoded.samples.len() != want {
                errors.push(format!(
                    "variant '{}' holds {} samples, loop {} beats at {} BPM wants {want}",
                    v.path, decoded.samples.len(), v.loop_end_beats, cue.tempo
                ));
            }
        }
    }

    // Determinism: re-mix the first variant from the shipped layer stems;
    // bytes must match the shipped variant file.
    if let Some(v) = manifest.variants.first() {
        if let Some(cue) = cue_by_id.get(v.cue_id.as_str()) {
            let mut layers: Vec<Vec<f32>> = Vec::new();
            let mut remix_ok = true;
            for layer_id in &v.layer_ids {
                let path = gaexport::stem_path_for_layer(&v.cue_id, layer_id);
                match std::fs::read(dir.join(&path))
                    .ok()
                    .and_then(|b| decode_wav(&b).ok())
                {
                    Some(d) => layers.push(d.samples),
                    None => {
                        remix_ok = false;
                        break;
                    }
                }
            }
            if remix_ok {
                let n =
                    beats_to_samples(v.loop_end_beats, cue.tempo, options.sample_rate) as usize;
                let loop_end =
                    beats_to_samples(v.loop_end_beats, cue.tempo, options.sample_rate) as u32;
                let want = encode_wav(&Stem {
                    name: String::new(),
                    sample_rate: options.sample_rate,
                    samples: mix_layers(&layers, n),
                    loop_start: 0,
                    loop_end,
                });
                match std::fs::read(dir.join(&v.path)) {
                    Ok(have) if have == want => {}
                    Ok(_) => errors.push(format!(
                        "variant '{}' is not deterministic: re-mix from layer stems differs",
                        v.path
                    )),
                    Err(_) => {} // missing file already reported above
                }
            }
        }
    }

    BatchReport { errors }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game_audio::{CueLayer, GAME_AUDIO_SCHEMA_VERSION};
    use crate::model::{Clip, ClipKind};
    use std::sync::atomic::{AtomicU64, Ordering};

    static BATCH_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn scratch(name: &str) -> std::path::PathBuf {
        let n = BATCH_COUNTER.fetch_add(1, Ordering::SeqCst);
        std::env::temp_dir().join(format!("ccez-batch-{name}-{}-{n}", std::process::id()))
    }

    fn options() -> ExportOptions {
        ExportOptions {
            sample_rate: 8000,
            seed: gaexport::DEFAULT_EXPORT_SEED,
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

    /// TP-like demo: an always-on bed, a combat-only drums layer, and a
    /// night-only pad — state gating the completeness check can observe.
    fn demo_project() -> Project {
        let mut p = Project::new("proj_batch", "Batch demo");
        p.tempo = 120.0;
        p.clips.push(clip("clip_bed", 8.0));
        p.clips.push(clip("clip_drums", 4.0));
        p.clips.push(clip("clip_pad", 2.0));
        p
    }

    fn demo_cue() -> AdaptiveCue {
        let mut cue = AdaptiveCue::new("cue_overworld", "Overworld");
        cue.tempo = 120.0;
        cue.default_state = "field".to_string();
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
            states: vec!["combat".to_string(), "boss".to_string()],
            volume: 0.9,
        });
        cue.layers.push(CueLayer {
            id: "pad".to_string(),
            name: "Night pad".to_string(),
            clip_ids: vec!["clip_pad".to_string()],
            states: vec!["night".to_string()],
            volume: 0.6,
        });
        cue
    }

    fn demo_request() -> BatchRequest {
        BatchRequest::with_default_states("overworld-batch", vec!["cue_overworld".to_string()], vec![])
    }

    fn clean_batch(dir: &std::path::Path, options: ExportOptions) -> BatchManifest {
        let project = demo_project();
        let cues = vec![demo_cue()];
        let banks = vec![];
        let built = build_batch(&project, &cues, &banks, &[], &demo_request(), options)
            .expect("demo batch builds");
        write_batch(dir, &built).expect("write batch");
        built.manifest
    }

    #[test]
    fn batch_covers_every_cue_times_state_with_one_manifest() {
        let dir = scratch("cover");
        let _ = std::fs::remove_dir_all(&dir);
        let manifest = clean_batch(&dir, options());
        assert_eq!(manifest.states, default_states());
        assert_eq!(manifest.schema_version, GAME_AUDIO_SCHEMA_VERSION);
        assert_eq!(manifest.validator_version, VALIDATOR_VERSION);
        // 1 cue × 6 TP states.
        assert_eq!(manifest.variants.len(), 6);
        assert!(dir.join(BATCH_MANIFEST_FILENAME).exists());
        // Base package files ride along untouched.
        assert!(dir.join("package.json").exists());
        assert!(dir.join("stems/cue_overworld_bed.wav").exists());
    }

    #[test]
    fn variants_carry_the_audible_layer_set_per_state() {
        let dir = scratch("layers");
        let _ = std::fs::remove_dir_all(&dir);
        let manifest = clean_batch(&dir, options());
        let layers = |state: &str| {
            manifest
                .variants
                .iter()
                .find(|v| v.state == state)
                .expect("variant")
                .layer_ids
                .clone()
        };
        assert_eq!(layers("field"), vec!["bed".to_string()]);
        assert_eq!(
            layers("combat"),
            vec!["bed".to_string(), "drums".to_string()]
        );
        assert_eq!(layers("boss"), vec!["bed".to_string(), "drums".to_string()]);
        assert_eq!(layers("night"), vec!["bed".to_string(), "pad".to_string()]);
        assert_eq!(layers("dungeon"), vec!["bed".to_string()]);
        assert_eq!(layers("village"), vec!["bed".to_string()]);
    }

    #[test]
    fn combat_mix_is_the_sample_sum_of_its_layers() {
        let dir = scratch("sum");
        let _ = std::fs::remove_dir_all(&dir);
        clean_batch(&dir, options());
        let bed = decode_wav(&std::fs::read(dir.join("stems/cue_overworld_bed.wav")).expect("bed"))
            .expect("decode bed");
        let drums =
            decode_wav(&std::fs::read(dir.join("stems/cue_overworld_drums.wav")).expect("drums"))
                .expect("decode drums");
        let mix = decode_wav(
            &std::fs::read(dir.join("variants/cue_overworld_combat.wav")).expect("mix"),
        )
        .expect("decode mix");
        // Mix loop is the longest audible layer (bed: 8 beats).
        assert_eq!(mix.samples.len(), bed.samples.len());
        // Drums (4 beats) wraps exactly twice into the 8-beat bed.
        assert_eq!(bed.samples.len(), 2 * drums.samples.len());
        for (i, s) in mix.samples.iter().enumerate() {
            let want = (bed.samples[i] + drums.samples[i % drums.samples.len()]).clamp(-1.0, 1.0);
            // i16 quantization on three codec passes: tolerance is one LSB
            // plus summation rounding, well under 1e-3.
            assert!((s - want).abs() < 1e-3, "sample {i}: got {s}, want {want}");
        }
    }

    #[test]
    fn build_then_validate_approves_a_clean_batch() {
        let dir = scratch("clean");
        let _ = std::fs::remove_dir_all(&dir);
        clean_batch(&dir, options());
        let report = validate_batch(&dir, &demo_project(), &[demo_cue()], &[], &[], options());
        assert!(report.is_ok(), "must approve: {:?}", report.errors);
    }

    #[test]
    fn validator_rejects_a_missing_variant_and_a_wrong_layer_set() {
        let dir = scratch("tamper");
        let _ = std::fs::remove_dir_all(&dir);
        clean_batch(&dir, options());
        // Delete one variant file: the file check fires (manifest still lists it).
        std::fs::remove_file(dir.join("variants/cue_overworld_dungeon.wav")).expect("remove");
        let report = validate_batch(&dir, &demo_project(), &[demo_cue()], &[], &[], options());
        assert!(
            report.errors.iter().any(|e| e.contains("missing variant file")),
            "want a missing-file error, got {:?}",
            report.errors
        );
        // Rewrite the manifest with a wrong layer set for field: the
        // completeness check fires even though every file exists.
        let mut manifest = read_batch_manifest(&dir).expect("read manifest");
        manifest
            .variants
            .iter_mut()
            .find(|v| v.state == "field")
            .expect("field")
            .layer_ids = vec!["bed".to_string(), "drums".to_string()];
        std::fs::write(
            dir.join(BATCH_MANIFEST_FILENAME),
            serde_json::to_string_pretty(&manifest).expect("serialize"),
        )
        .expect("rewrite");
        // Restore the deleted file so only the layer-set error remains under test.
        clean_batch(&dir, options());
        let mut manifest = read_batch_manifest(&dir).expect("read manifest");
        manifest
            .variants
            .iter_mut()
            .find(|v| v.state == "field")
            .expect("field")
            .layer_ids = vec!["bed".to_string(), "drums".to_string()];
        std::fs::write(
            dir.join(BATCH_MANIFEST_FILENAME),
            serde_json::to_string_pretty(&manifest).expect("serialize"),
        )
        .expect("rewrite");
        let report = validate_batch(&dir, &demo_project(), &[demo_cue()], &[], &[], options());
        assert!(
            report.errors.iter().any(|e| e.contains("names layers")),
            "want a layer-set error, got {:?}",
            report.errors
        );
    }

    #[test]
    fn empty_states_is_a_request_error_not_a_silent_empty_batch() {
        let project = demo_project();
        let cues = vec![demo_cue()];
        let request = BatchRequest::new("empty", vec!["cue_overworld".to_string()], vec![], vec![]);
        let err = build_batch(&project, &cues, &[], &[], &request, options()).expect_err("empty");
        assert!(err.iter().any(|e| e.contains("at least one state")));
    }

    #[test]
    fn silent_variant_covers_a_state_with_no_audible_layers() {
        // A cue whose only layer is combat-gated: field renders silence.
        let mut project = demo_project();
        let _ = &mut project;
        let mut cue = AdaptiveCue::new("cue_fight", "Fight");
        cue.tempo = 120.0;
        cue.layers.push(CueLayer {
            id: "drums".to_string(),
            name: "Drums".to_string(),
            clip_ids: vec!["clip_drums".to_string()],
            states: vec!["combat".to_string()],
            volume: 0.9,
        });
        let request = BatchRequest::new(
            "fight-batch",
            vec!["cue_fight".to_string()],
            vec![],
            vec!["field".to_string(), "combat".to_string()],
        );
        let built = build_batch(&project, &[cue.clone()], &[], &[], &request, options())
            .expect("builds");
        let field = built
            .variants
            .iter()
            .find(|v| v.variant.state == "field")
            .expect("field variant");
        assert!(field.variant.layer_ids.is_empty());
        assert_eq!(field.variant.loop_end_beats, DEFAULT_LOOP_BEATS);
        let decoded = decode_wav(&field.wav_bytes).expect("decode");
        assert!(decoded.samples.iter().all(|s| *s == 0.0));
        let dir = scratch("silent");
        let _ = std::fs::remove_dir_all(&dir);
        write_batch(&dir, &built).expect("write");
        let report = validate_batch(&dir, &project, &[cue], &[], &[], options());
        assert!(report.is_ok(), "silent variant validates: {:?}", report.errors);
    }
}
