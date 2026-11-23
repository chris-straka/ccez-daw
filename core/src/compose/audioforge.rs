//! Export a [`Composition`] as an audioforge interactive-music package.
//!
//! Layout (every path relative to the package dir; contract:
//! `contracts/audioforge-music.md`):
//!
//! ```text
//! <dir>/
//!   manifest.json          # what is here: stems, sections, loop points
//!   score.json             # audioforge Score (sections, layers, intensity)
//!   mix.json               # audioforge RuntimeConfig (buses + score)
//!   events/music.<name>.<section>.<layer>.json   # loop-kind events
//!   stems/<section>_<layer>.wav                  # 16-bit stereo 48 kHz
//!   scenes/preview.json    # audioforge scene playing the preview plan
//!   composition.json       # the source document (re-open / re-export)
//! ```
//!
//! Mapping: intro → `once` + `length_beats` + `next`; outro → `once` with
//! no `next`; loop → `loop_start_beats` 0 / `loop_end_beats` = section
//! length; layer `min_intensity` and `gain_db` copy across. audioforge
//! reads only `score.json`, `mix.json`, `events/` and `stems/`; the rest is
//! for people, agents and re-export.

use std::path::Path;

use serde_json::{json, Value};

use super::render::{render_stems, seam_stats, PlanStep, RenderedStem, SeamStat};
use super::{Composition, Role, SAMPLE_RATE};

pub const FORMAT: &str = "audioforge-music";
pub const FORMAT_VERSION: u32 = 1;

pub fn event_name(c: &Composition, section: &str, layer: &str) -> String {
    format!("music.{}.{}.{}", c.name, section, layer)
}

pub fn stem_path(section: &str, layer: &str) -> String {
    format!("stems/{section}_{layer}.wav")
}

/// Package-level section metadata shared by composition and take-cut exports.
pub(crate) struct PackageSection<'a> {
    pub id: &'a str,
    pub role_name: &'a str,
    pub once: bool,
    pub next: Option<&'a str>,
    pub length_beats: f32,
    pub loop_start_beats: Option<f32>,
    pub loop_end_beats: Option<f32>,
}

/// Package-level stem metadata shared by composition and take-cut exports.
pub(crate) struct PackageStem<'a> {
    pub section: &'a str,
    pub layer: &'a str,
    pub layer_gain_db: f32,
    pub min_intensity: u8,
    pub stem: &'a RenderedStem,
    pub loop_start_frame: u32,
    pub loop_end_frame: u32,
    pub loop_start_beats: Option<f32>,
    pub loop_end_beats: Option<f32>,
    pub length_beats: f32,
}

/// Metadata passed to the shared audioforge package writer.
pub(crate) struct PackageManifestMeta<'a> {
    pub name: &'a str,
    pub tempo: f32,
    pub beats_per_bar: u32,
    pub key: &'a str,
    pub xfade_beats: f32,
    pub trim_db: f32,
    pub instruments: Vec<Value>,
    pub sections_json: Value,
    pub layers_json: Value,
    pub composition_json: Option<Value>,
}

pub(crate) fn build_score_json(
    name: &str,
    tempo: f32,
    beats_per_bar: u32,
    xfade_beats: f32,
    sections_meta: &[PackageSection<'_>],
    stems_meta: &[PackageStem<'_>],
) -> Value {
    let mut sections = serde_json::Map::new();
    for sec in sections_meta {
        let layers: Vec<Value> = stems_meta
            .iter()
            .filter(|s| s.section == sec.id)
            .map(|s| {
                json!({
                    "event": format!("music.{name}.{}.{}", sec.id, s.layer),
                    "gain_db": s.layer_gain_db,
                    "bus": "music",
                    "min_intensity": s.min_intensity,
                })
            })
            .collect();
        if layers.is_empty() {
            continue;
        }
        let mut v = json!({ "layers": layers, "length_beats": sec.length_beats });
        if sec.once {
            v["once"] = json!(true);
            if let Some(next) = sec.next {
                v["next"] = json!(next);
            }
        } else {
            v["loop_start_beats"] = json!(sec.loop_start_beats.unwrap_or(0.0));
            v["loop_end_beats"] = json!(sec.loop_end_beats.unwrap_or(sec.length_beats));
        }
        sections.insert(sec.id.to_string(), v);
    }
    let initial = sections_meta
        .iter()
        .find(|s| s.role_name == "intro" && sections.contains_key(s.id))
        .or_else(|| {
            sections_meta
                .iter()
                .find(|s| s.role_name == "loop" && sections.contains_key(s.id))
        })
        .or_else(|| sections_meta.iter().find(|s| sections.contains_key(s.id)))
        .map(|s| s.id.to_string())
        .unwrap_or_default();
    json!({
        "name": name,
        "bpm": tempo,
        "beats_per_bar": beats_per_bar,
        "quantize": "bar",
        "xfade_beats": xfade_beats,
        "initial": initial,
        "initial_intensity": 1,
        "sections": sections,
    })
}

pub(crate) fn build_preview_scene_json(
    tempo: f32,
    beats_per_bar: u32,
    sections_meta: &[PackageSection<'_>],
    plan: &[PlanStep],
    outro_tail_secs: f64,
) -> Value {
    let spb = 60.0 / tempo as f64;
    let find_sec = |id: &str| sections_meta.iter().find(|s| s.id == id);
    let mut states = Vec::new();
    let mut t = 0.0f64;
    let mut last_section: Option<&str> = None;
    for (i, step) in plan.iter().enumerate() {
        let Some(sec) = find_sec(&step.section) else {
            continue;
        };
        let auto = (i == 0 && sec.role_name == "intro")
            || last_section
                .and_then(find_sec)
                .is_some_and(|p| p.role_name == "intro" && p.next == Some(sec.id));
        let at = (t - 0.05).max(0.0);
        let mut st = json!({ "at_secs": at, "intensity": step.intensity });
        if !auto && last_section != Some(sec.id) {
            st["section"] = json!(sec.id);
        }
        states.push(st);
        let step_beats = if !sec.once {
            step.bars
                .map(|b| (b * beats_per_bar) as f64)
                .unwrap_or(sec.length_beats as f64)
        } else {
            sec.length_beats as f64
        };
        t += step_beats * spb;
        last_section = Some(sec.id);
    }
    let tail = if last_section.and_then(find_sec).is_some_and(|s| s.once) {
        outro_tail_secs
    } else {
        0.0
    };
    json!({
        "sample_rate": SAMPLE_RATE,
        "duration_secs": t + tail,
        "seed": 1,
        "buses": [
            { "name": "master", "gain": 1.0 },
            { "name": "music", "gain": 1.0, "parent": "master" }
        ],
        "music": { "score": "../score.json", "states": states }
    })
}

pub(crate) fn write_audioforge_package(
    dir: &Path,
    meta: &PackageManifestMeta<'_>,
    sections_meta: &[PackageSection<'_>],
    stems_meta: &[PackageStem<'_>],
    scene: &Value,
    initial_warnings: Vec<String>,
) -> Result<(Vec<String>, Vec<String>), String> {
    let io = |e: std::io::Error| format!("{}: {e}", dir.display());
    for sub in ["events", "stems", "scenes"] {
        std::fs::create_dir_all(dir.join(sub)).map_err(io)?;
    }
    let mut files = Vec::new();
    let mut write = |rel: String, bytes: Vec<u8>| -> Result<(), String> {
        std::fs::write(dir.join(&rel), bytes).map_err(|e| format!("{rel}: {e}"))?;
        files.push(rel);
        Ok(())
    };
    let pretty = |v: &Value| -> Result<Vec<u8>, String> {
        serde_json::to_string_pretty(v)
            .map(|s| (s + "\n").into_bytes())
            .map_err(|e| e.to_string())
    };
    let mut manifest_stems = Vec::new();
    for s in stems_meta {
        let rel = stem_path(s.section, s.layer);
        write(
            rel.clone(),
            encode_stem_wav_with_loop(s.stem, s.loop_start_frame, s.loop_end_frame),
        )?;
        let ev = format!("music.{}.{}.{}", meta.name, s.section, s.layer);
        let event = json!({
            "name": ev,
            "kind": "loop",
            "bus": "music",
            "priority": 60,
            "max_voices": 2,
            "cooldown_secs": 0.0,
            "volume_db": [0.0, 0.0],
            "pitch": [1.0, 1.0],
            "pan": [0.0, 0.0],
            "sources": [{ "wav": format!("../{rel}"), "weight": 1.0 }]
        });
        write(format!("events/{ev}.json"), pretty(&event)?)?;
        let sec = sections_meta
            .iter()
            .find(|sec| sec.id == s.section)
            .ok_or_else(|| format!("unknown section {:?}", s.section))?;
        manifest_stems.push(json!({
            "path": rel,
            "event": ev,
            "section": s.section,
            "role": sec.role_name,
            "layer": s.layer,
            "min_intensity": s.min_intensity,
            "length_beats": s.length_beats,
            "loop_start_beats": s.loop_start_beats.map(|b| json!(b)).unwrap_or(Value::Null),
            "loop_end_beats": s.loop_end_beats.map(|b| json!(b)).unwrap_or(Value::Null),
            "frames": s.stem.frames(),
            "channels": STEM_CHANNELS,
        }));
    }
    let score = build_score_json(
        meta.name,
        meta.tempo,
        meta.beats_per_bar,
        meta.xfade_beats,
        sections_meta,
        stems_meta,
    );
    write("score.json".into(), pretty(&score)?)?;
    let mix = json!({
        "sample_rate": SAMPLE_RATE,
        "seed": 1,
        "buses": [
            { "name": "master", "gain": 1.0 },
            { "name": "music", "gain": 1.0, "parent": "master" }
        ],
        "score": "score.json"
    });
    write("mix.json".into(), pretty(&mix)?)?;
    let mut warnings = initial_warnings;
    let scene_secs = scene["duration_secs"].as_f64().unwrap_or(0.0);
    if scene_secs > AF_SCENE_MAX_SECS {
        warnings.push(format!(
            "scenes/preview.json runs {scene_secs:.1} s; audioforge renders scenes up to {AF_SCENE_MAX_SECS} s, so pass a shorter plan to preview it there"
        ));
    }
    write("scenes/preview.json".into(), pretty(scene)?)?;
    if let Some(comp) = &meta.composition_json {
        write("composition.json".into(), pretty(comp)?)?;
    }
    let manifest = json!({
        "format": FORMAT,
        "format_version": FORMAT_VERSION,
        "generator": concat!("ccez-daw ", env!("CARGO_PKG_VERSION")),
        "name": meta.name,
        "tempo": meta.tempo,
        "beats_per_bar": meta.beats_per_bar,
        "key": meta.key,
        "sample_rate": SAMPLE_RATE,
        "channels": STEM_CHANNELS,
        "instruments": meta.instruments,
        "score": "score.json",
        "mix": "mix.json",
        "events_dir": "events",
        "preview_scene": "scenes/preview.json",
        "trim_db": meta.trim_db,
        "sections": meta.sections_json,
        "layers": meta.layers_json,
        "stems": manifest_stems,
    });
    write("manifest.json".into(), pretty(&manifest)?)?;
    Ok((files, warnings))
}

/// The audioforge `score.json` for `c` (sections without notes are left
/// out: audioforge needs at least one layer per section).
pub fn score_json(c: &Composition, stems: &[RenderedStem]) -> Value {
    let sections_meta: Vec<PackageSection<'_>> = c
        .sections
        .iter()
        .map(|s| PackageSection {
            id: &s.id,
            role_name: match s.role {
                Role::Intro => "intro",
                Role::Loop => "loop",
                Role::Outro => "outro",
            },
            once: s.role != Role::Loop,
            next: s.next.as_deref(),
            length_beats: c.section_beats(s),
            loop_start_beats: (s.role == Role::Loop).then_some(0.0),
            loop_end_beats: (s.role == Role::Loop).then(|| c.section_beats(s)),
        })
        .collect();
    let stems_meta: Vec<PackageStem<'_>> = stems
        .iter()
        .filter_map(|s| {
            let l = c.layer(&s.layer)?;
            let sec = c.section(&s.section)?;
            let beats = c.section_beats(sec);
            let looped = s.role == Role::Loop;
            Some(PackageStem {
                section: &s.section,
                layer: &s.layer,
                layer_gain_db: l.gain_db,
                min_intensity: l.min_intensity,
                stem: s,
                loop_start_frame: 0,
                loop_end_frame: if looped { s.frames() as u32 } else { 0 },
                loop_start_beats: looped.then_some(0.0),
                loop_end_beats: looped.then_some(beats),
                length_beats: beats,
            })
        })
        .collect();
    build_score_json(
        &c.name,
        c.tempo,
        c.beats_per_bar,
        1.0,
        &sections_meta,
        &stems_meta,
    )
}

/// audioforge scene that plays `plan` (section switches + intensity) so
/// `af render scenes/preview.json --events events -o preview.wav` hears
/// exactly what the game will.
pub fn preview_scene_json(c: &Composition, plan: &[PlanStep]) -> Value {
    let sections_meta: Vec<PackageSection<'_>> = c
        .sections
        .iter()
        .map(|s| PackageSection {
            id: &s.id,
            role_name: match s.role {
                Role::Intro => "intro",
                Role::Loop => "loop",
                Role::Outro => "outro",
            },
            once: s.role != Role::Loop,
            next: s.next.as_deref(),
            length_beats: c.section_beats(s),
            loop_start_beats: (s.role == Role::Loop).then_some(0.0),
            loop_end_beats: (s.role == Role::Loop).then(|| c.section_beats(s)),
        })
        .collect();
    build_preview_scene_json(
        c.tempo,
        c.beats_per_bar,
        &sections_meta,
        plan,
        super::render::TAIL_SECS as f64,
    )
}

/// Longest scene `af render` accepts (audioforge's scene loader).
pub const AF_SCENE_MAX_SECS: f64 = 120.0;

/// Stems are stereo (audioforge reads 1-2 channel WAVs; as of 2026-10
/// it downmixes music stems to mono on load).
pub const STEM_CHANNELS: u16 = 2;

/// 16-bit PCM stereo WAV with a `smpl` loop chunk (`loop_end` 0 = none).
pub fn encode_stem_wav(stem: &RenderedStem, loop_end: u32) -> Vec<u8> {
    encode_stem_wav_with_loop(stem, 0, loop_end)
}

/// 16-bit PCM stereo WAV with a `smpl` loop chunk (`[loop_start, loop_end)`
/// or `loop_end == 0` for non-looping one-shot cues).
pub fn encode_stem_wav_with_loop(stem: &RenderedStem, loop_start: u32, loop_end: u32) -> Vec<u8> {
    let frames = stem.interleaved();
    let mut data = Vec::with_capacity(frames.len() * 2);
    for s in &frames {
        data.extend_from_slice(&((s.clamp(-1.0, 1.0) * 32767.0).round() as i16).to_le_bytes());
    }
    let mut smpl = Vec::new();
    for v in [0u32, 0, 1_000_000_000 / SAMPLE_RATE, 60, 0, 0, 0, 1, 0] {
        smpl.extend_from_slice(&v.to_le_bytes());
    }
    for v in [0u32, 0, loop_start, loop_end, 0, 0] {
        smpl.extend_from_slice(&v.to_le_bytes());
    }
    let ch = STEM_CHANNELS;
    let mut out = Vec::with_capacity(data.len() + 128);
    out.extend_from_slice(b"RIFF");
    let riff_len = 4 + 8 + 16 + 8 + smpl.len() + 8 + data.len();
    out.extend_from_slice(&(riff_len as u32).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&ch.to_le_bytes());
    out.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    out.extend_from_slice(&(SAMPLE_RATE * 2 * ch as u32).to_le_bytes());
    out.extend_from_slice(&(2 * ch).to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out.extend_from_slice(&data);
    out.extend_from_slice(b"smpl");
    out.extend_from_slice(&(smpl.len() as u32).to_le_bytes());
    out.extend_from_slice(&smpl);
    out
}

/// What [`export`] wrote.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ExportReport {
    pub dir: String,
    pub files: Vec<String>,
    pub stems: usize,
    pub trim_db: f32,
    /// Per loop stem: the wrap-around join against the music's own steps.
    pub loop_seams: Vec<SeamStat>,
    pub warnings: Vec<String>,
}

/// Validate, render and write the package to `dir` (created if missing).
pub fn export(c: &Composition, plan: &[PlanStep], dir: &Path) -> Result<ExportReport, String> {
    let errs = c.validate();
    if !errs.is_empty() {
        return Err(format!(
            "composition is not exportable:\n- {}",
            errs.join("\n- ")
        ));
    }
    let rendered = render_stems(c)?;
    let (stems, trim_db) = (&rendered.stems, rendered.trim_db);
    if stems.is_empty() {
        return Err("composition has no notes: nothing to export".into());
    }
    let sections_meta: Vec<PackageSection<'_>> = c
        .sections
        .iter()
        .map(|s| PackageSection {
            id: &s.id,
            role_name: match s.role {
                Role::Intro => "intro",
                Role::Loop => "loop",
                Role::Outro => "outro",
            },
            once: s.role != Role::Loop,
            next: s.next.as_deref(),
            length_beats: c.section_beats(s),
            loop_start_beats: (s.role == Role::Loop).then_some(0.0),
            loop_end_beats: (s.role == Role::Loop).then(|| c.section_beats(s)),
        })
        .collect();
    let mut stems_meta = Vec::with_capacity(stems.len());
    for s in stems {
        let l = c
            .layer(&s.layer)
            .ok_or_else(|| format!("unknown layer {:?}", s.layer))?;
        let sec = c
            .section(&s.section)
            .ok_or_else(|| format!("unknown section {:?}", s.section))?;
        let looped = s.role == Role::Loop;
        let beats = c.section_beats(sec);
        stems_meta.push(PackageStem {
            section: &s.section,
            layer: &s.layer,
            layer_gain_db: l.gain_db,
            min_intensity: l.min_intensity,
            stem: s,
            loop_start_frame: 0,
            loop_end_frame: if looped { s.frames() as u32 } else { 0 },
            loop_start_beats: looped.then_some(0.0),
            loop_end_beats: looped.then_some(beats),
            length_beats: beats,
        });
    }
    let scene = preview_scene_json(c, plan);
    let meta = PackageManifestMeta {
        name: &c.name,
        tempo: c.tempo,
        beats_per_bar: c.beats_per_bar,
        key: &c.key,
        xfade_beats: 1.0,
        trim_db,
        instruments: instrument_credits(c),
        sections_json: serde_json::to_value(&c.sections).map_err(|e| e.to_string())?,
        layers_json: serde_json::to_value(&c.layers).map_err(|e| e.to_string())?,
        composition_json: Some(serde_json::to_value(c).map_err(|e| e.to_string())?),
    };
    let (files, warnings) = write_audioforge_package(
        dir,
        &meta,
        &sections_meta,
        &stems_meta,
        &scene,
        rendered.warnings.clone(),
    )?;
    Ok(ExportReport {
        dir: dir.display().to_string(),
        stems: stems.len(),
        files,
        trim_db,
        loop_seams: seam_stats(stems),
        warnings,
    })
}

/// Per instrument: what it is and the licence of any samples in it, so a
/// game's credits/licence page can be built from the package.
pub fn instrument_credits(c: &Composition) -> Vec<Value> {
    c.instruments
        .iter()
        .map(|i| match super::patch_kind(&i.patch) {
            Some(super::PatchKind::Sampled(e)) => {
                let s = super::library::source(e.source).expect("catalog source");
                json!({ "id": i.id, "patch": i.patch, "kind": "sampled",
                        "source": s.name, "licence": s.licence, "licence_url": s.licence_url })
            }
            Some(super::PatchKind::Local) => {
                json!({ "id": i.id, "patch": i.patch, "kind": "local", "licence": "unknown (local file)" })
            }
            _ => json!({ "id": i.id, "patch": i.patch, "kind": "synth", "licence": "ccez-daw built-in synth (MIT)" }),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::super::render::default_plan;
    use super::super::tests::tiny;
    use super::*;

    #[test]
    fn score_maps_roles_intensity_and_loop_points() {
        let c = tiny();
        let r = render_stems(&c).unwrap();
        let s = score_json(&c, &r.stems);
        assert_eq!(s["initial"], "intro");
        assert_eq!(s["sections"]["intro"]["once"], true);
        assert_eq!(s["sections"]["intro"]["next"], "calm");
        assert_eq!(s["sections"]["intro"]["length_beats"], 4.0);
        assert_eq!(s["sections"]["end"]["once"], true);
        assert!(s["sections"]["end"].get("next").is_none());
        assert_eq!(s["sections"]["calm"]["loop_end_beats"], 8.0);
        let layers = s["sections"]["calm"]["layers"].as_array().unwrap();
        assert_eq!(layers[1]["event"], "music.t.calm.pulse");
        assert_eq!(layers[1]["min_intensity"], 3);
    }

    #[test]
    fn export_writes_a_complete_package() {
        let c = tiny();
        let dir = std::env::temp_dir().join(format!("ccez-af-export-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let r = export(&c, &default_plan(&c), &dir).unwrap();
        assert_eq!(r.stems, 4);
        assert!(r.warnings.is_empty(), "{:?}", r.warnings);
        let long = vec![
            PlanStep {
                section: "calm".into(),
                bars: Some(64),
                intensity: 1
            };
            1
        ];
        let r2 = export(&c, &long, &dir).unwrap();
        assert!(r2.warnings[0].contains("up to 120 s"), "{:?}", r2.warnings);
        assert!(
            r.loop_seams.iter().all(|s| s.seamless),
            "{:?}",
            r.loop_seams
        );
        // Stems are stereo 16-bit with the loop in a smpl chunk.
        let wav = std::fs::read(dir.join("stems/calm_bed.wav")).unwrap();
        let a = super::super::wavread::decode(&wav).unwrap();
        assert_eq!(
            (a.channels.len(), a.sample_rate, a.frames()),
            (2, 48_000, 4 * 48_000)
        );
        assert_eq!(a.sample_loop, Some((0, 4 * 48_000)));
        let m: Value =
            serde_json::from_slice(&std::fs::read(dir.join("manifest.json")).unwrap()).unwrap();
        assert_eq!(m["channels"], 2);
        assert_eq!(m["instruments"][0]["kind"], "synth");
        for f in [
            "manifest.json",
            "score.json",
            "mix.json",
            "scenes/preview.json",
            "composition.json",
            "stems/calm_bed.wav",
            "events/music.t.calm.bed.json",
        ] {
            assert!(dir.join(f).exists(), "{f} missing");
        }
        let ev: Value = serde_json::from_slice(
            &std::fs::read(dir.join("events/music.t.calm.pulse.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(ev["kind"], "loop");
        assert_eq!(ev["sources"][0]["wav"], "../stems/calm_pulse.wav");
        // The round-trip document re-validates.
        let back: Composition =
            serde_json::from_slice(&std::fs::read(dir.join("composition.json")).unwrap()).unwrap();
        assert_eq!(back.validate(), Vec::<String>::new());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn preview_scene_requests_switches_before_the_bar() {
        let c = tiny();
        let scene = preview_scene_json(&c, &default_plan(&c));
        let states = scene["music"]["states"].as_array().unwrap();
        // intro (initial) -> calm (auto hand-off) -> calm @5 -> end
        assert!(states[0].get("section").is_none());
        assert!(
            states[1].get("section").is_none(),
            "intro hands off by itself"
        );
        assert_eq!(states[2]["intensity"], 5);
        assert_eq!(states[3]["section"], "end");
        // end is requested 50 ms before beat 4 + 8 + 8 = 20 (10 s at 120 bpm)
        assert!((states[3]["at_secs"].as_f64().unwrap() - 9.95).abs() < 1e-6);
    }

    #[test]
    fn invalid_compositions_do_not_export() {
        let mut c = tiny();
        c.parts[0].instrument = "nope".into();
        let err = export(&c, &[], &std::env::temp_dir().join("never")).unwrap_err();
        assert!(err.contains("unknown instrument"), "{err}");
    }
}
