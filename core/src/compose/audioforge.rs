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
//!   stems/<section>_<layer>.wav                  # 16-bit mono 48 kHz
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

use super::render::{render_stems, PlanStep, RenderedStem};
use super::{Composition, Role, SAMPLE_RATE};
use crate::bounce::{encode_wav, Stem};

pub const FORMAT: &str = "audioforge-music";
pub const FORMAT_VERSION: u32 = 1;

pub fn event_name(c: &Composition, section: &str, layer: &str) -> String {
    format!("music.{}.{}.{}", c.name, section, layer)
}

pub fn stem_path(section: &str, layer: &str) -> String {
    format!("stems/{section}_{layer}.wav")
}

/// The audioforge `score.json` for `c` (sections without notes are left
/// out: audioforge needs at least one layer per section).
pub fn score_json(c: &Composition, stems: &[RenderedStem]) -> Value {
    let mut sections = serde_json::Map::new();
    for sec in &c.sections {
        let layers: Vec<Value> = stems
            .iter()
            .filter(|s| s.section == sec.id)
            .map(|s| {
                let l = c.layer(&s.layer).expect("stem layer exists");
                json!({
                    "event": event_name(c, &sec.id, &s.layer),
                    "gain_db": l.gain_db,
                    "bus": "music",
                    "min_intensity": l.min_intensity,
                })
            })
            .collect();
        if layers.is_empty() {
            continue;
        }
        let beats = c.section_beats(sec);
        let mut v = json!({ "layers": layers, "length_beats": beats });
        match sec.role {
            Role::Intro => {
                v["once"] = json!(true);
                v["next"] = json!(sec.next);
            }
            Role::Outro => v["once"] = json!(true),
            Role::Loop => {
                v["loop_start_beats"] = json!(0.0);
                v["loop_end_beats"] = json!(beats);
            }
        }
        sections.insert(sec.id.clone(), v);
    }
    let initial = c
        .sections
        .iter()
        .find(|s| s.role == Role::Intro && sections.contains_key(&s.id))
        .or_else(|| {
            c.sections
                .iter()
                .find(|s| s.role == Role::Loop && sections.contains_key(&s.id))
        })
        .map(|s| s.id.clone())
        .unwrap_or_default();
    json!({
        "name": c.name,
        "bpm": c.tempo,
        "beats_per_bar": c.beats_per_bar,
        "quantize": "bar",
        "xfade_beats": 1.0,
        "initial": initial,
        "initial_intensity": 1,
        "sections": sections,
    })
}

/// audioforge scene that plays `plan` (section switches + intensity) so
/// `af render scenes/preview.json --events events -o preview.wav` hears
/// exactly what the game will.
pub fn preview_scene_json(c: &Composition, plan: &[PlanStep]) -> Value {
    let spb = c.secs_per_beat();
    let mut states = Vec::new();
    let mut t = 0.0f64;
    let mut last_section: Option<&str> = None;
    for (i, step) in plan.iter().enumerate() {
        let Some(sec) = c.section(&step.section) else {
            continue;
        };
        // Request just before the boundary so the bar quantizer lands it
        // on the step's first downbeat. A plan that starts on the intro
        // needs no request (it is the score's initial section), and the
        // loop an intro hands off to arrives on its own.
        let auto = i == 0 && sec.role == Role::Intro
            || last_section.and_then(|l| c.section(l)).is_some_and(|p| {
                p.role == Role::Intro && p.next.as_deref() == Some(sec.id.as_str())
            });
        let at = (t - 0.05).max(0.0);
        let mut st = json!({ "at_secs": at, "intensity": step.intensity });
        if !auto && last_section != Some(sec.id.as_str()) {
            st["section"] = json!(sec.id);
        }
        states.push(st);
        let bars = if sec.role == Role::Loop {
            step.bars.unwrap_or(sec.bars)
        } else {
            sec.bars
        };
        t += (bars * c.beats_per_bar) as f64 * spb;
        last_section = Some(sec.id.as_str());
    }
    // Let a final outro ring out.
    let tail = if last_section
        .and_then(|l| c.section(l))
        .is_some_and(|s| s.role == Role::Outro)
    {
        super::render::TAIL_SECS as f64
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

/// What [`export`] wrote.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ExportReport {
    pub dir: String,
    pub files: Vec<String>,
    pub stems: usize,
    pub trim_db: f32,
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
    let (stems, trim_db) = render_stems(c);
    if stems.is_empty() {
        return Err("composition has no notes: nothing to export".into());
    }
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
    let pretty = |v: &Value| (serde_json::to_string_pretty(v).expect("json") + "\n").into_bytes();
    let mut manifest_stems = Vec::new();
    for s in &stems {
        let rel = stem_path(&s.section, &s.layer);
        let looped = s.role == Role::Loop;
        let stem = Stem {
            name: rel.clone(),
            sample_rate: SAMPLE_RATE,
            samples: s.samples.clone(),
            loop_start: 0,
            loop_end: if looped { s.samples.len() as u32 } else { 0 },
        };
        write(rel.clone(), encode_wav(&stem))?;
        let ev = event_name(c, &s.section, &s.layer);
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
        write(format!("events/{ev}.json"), pretty(&event))?;
        let sec = c.section(&s.section).expect("section");
        manifest_stems.push(json!({
            "path": rel,
            "event": ev,
            "section": s.section,
            "role": sec.role,
            "layer": s.layer,
            "min_intensity": s.min_intensity,
            "length_beats": c.section_beats(sec),
            "loop_start_beats": if looped { json!(0.0) } else { Value::Null },
            "loop_end_beats": if looped { json!(c.section_beats(sec)) } else { Value::Null },
            "frames": s.samples.len(),
        }));
    }
    write("score.json".into(), pretty(&score_json(c, &stems)))?;
    let mix = json!({
        "sample_rate": SAMPLE_RATE,
        "seed": 1,
        "buses": [
            { "name": "master", "gain": 1.0 },
            { "name": "music", "gain": 1.0, "parent": "master" }
        ],
        "score": "score.json"
    });
    write("mix.json".into(), pretty(&mix))?;
    write(
        "scenes/preview.json".into(),
        pretty(&preview_scene_json(c, plan)),
    )?;
    write(
        "composition.json".into(),
        pretty(&serde_json::to_value(c).map_err(|e| e.to_string())?),
    )?;
    let manifest = json!({
        "format": FORMAT,
        "format_version": FORMAT_VERSION,
        "generator": concat!("ccez-daw ", env!("CARGO_PKG_VERSION")),
        "name": c.name,
        "tempo": c.tempo,
        "beats_per_bar": c.beats_per_bar,
        "key": c.key,
        "sample_rate": SAMPLE_RATE,
        "score": "score.json",
        "mix": "mix.json",
        "events_dir": "events",
        "preview_scene": "scenes/preview.json",
        "trim_db": trim_db,
        "sections": c.sections,
        "layers": c.layers,
        "stems": manifest_stems,
    });
    write("manifest.json".into(), pretty(&manifest))?;
    Ok(ExportReport {
        dir: dir.display().to_string(),
        stems: stems.len(),
        files,
        trim_db,
    })
}

#[cfg(test)]
mod tests {
    use super::super::render::default_plan;
    use super::super::tests::tiny;
    use super::*;

    #[test]
    fn score_maps_roles_intensity_and_loop_points() {
        let c = tiny();
        let (stems, _) = render_stems(&c);
        let s = score_json(&c, &stems);
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
