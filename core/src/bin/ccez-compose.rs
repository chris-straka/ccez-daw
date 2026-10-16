//! `ccez-compose`: render and export game-music compositions headless.
//!
//! ```text
//! ccez-compose validate <composition.json>
//! ccez-compose preview  <composition.json> --out <file.wav|file.mp3> [--plan <plan.json>]
//! ccez-compose export   <composition.json> --out <dir> [--plan <plan.json>]
//! ccez-compose patches   [--family <family>]
//! ccez-compose instrument <id>
//! ccez-compose fetch     <id>... | all
//! ccez-compose samples   lock --out <sample-lock.json> | verify | calibrate [<id>...]
//! ```
//!
//! `patches` lists every instrument an agent can name: the built-in synth
//! patches and the sampled ones (CC0/MIT libraries, downloaded on demand
//! into `$CCEZ_SAMPLES_DIR` or `~/.cache/ccez-daw/samples` and checked
//! against `core/assets/sample-lock.json`). `instrument` adds velocity
//! layers, round robins and, for drums, which key plays which hit.
//!
//! Every command prints one JSON object on stdout (the MCP `compose_*`
//! tools parse it) and exits non-zero with a plain-text reason on stderr.
//! `--plan` is a JSON array of `{ "section", "bars"?, "intensity"? }`
//! steps; the default plays intro, each loop at intensity 1 then 5, outro.
//! MP3 previews need `ffmpeg` on PATH.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use ccez_core::compose::render::{default_plan, interleave, render_preview, render_stems, wav_bytes, PlanStep};
use ccez_core::compose::sampler::SampledInstrument;
use ccez_core::compose::{audioforge, library, synth, Composition};
use serde_json::{json, Value};

fn main() -> ExitCode {
    match run(std::env::args().skip(1).collect()) {
        Ok(v) => {
            println!("{v}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("ccez-compose: {e}");
            ExitCode::FAILURE
        }
    }
}

fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1).cloned())
}

fn load(path: &str) -> Result<Composition, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    serde_json::from_str(&text).map_err(|e| format!("{path}: {e}"))
}

fn plan_for(c: &Composition, args: &[String]) -> Result<Vec<PlanStep>, String> {
    match flag(args, "--plan") {
        Some(p) => {
            let text = std::fs::read_to_string(&p).map_err(|e| format!("{p}: {e}"))?;
            serde_json::from_str(&text).map_err(|e| format!("{p}: {e}"))
        }
        None => Ok(default_plan(c)),
    }
}

fn run(args: Vec<String>) -> Result<serde_json::Value, String> {
    let cmd = args.first().map(String::as_str).unwrap_or("");
    match cmd {
        "patches" => {
            let family = flag(&args, "--family");
            let root = library::cache_root();
            let lock = library::lock();
            let synths = synth::PATCHES.iter().map(|p| {
                json!({ "id": p.id, "kind": "synth", "family": "synth", "description": p.description,
                        "range": [p.range.0, p.range.1], "pan": 0.0, "licence": "built-in (MIT)", "cached": true })
            });
            let sampled = library::CATALOG.iter().map(|e| {
                let s = library::source(e.source).expect("source");
                let missing = library::missing_bytes(&root, &lock, e);
                json!({ "id": e.id, "kind": "sampled", "family": e.family, "description": e.description,
                        "range": [e.range.0, e.range.1], "pan": e.pan, "source": s.name, "licence": s.licence,
                        "cached": missing == 0, "download_mb": (missing as f64 / 1e6).ceil() })
            });
            Ok(json!(synths
                .chain(sampled)
                .filter(|v| family.as_deref().is_none_or(|f| v["family"] == f))
                .collect::<Vec<_>>()))
        }
        "instrument" => {
            let id = args.get(1).ok_or("instrument: missing <id>")?;
            if let Some(p) = synth::patch(id) {
                return Ok(json!({ "id": p.id, "kind": "synth", "description": p.description,
                                   "range": [p.range.0, p.range.1], "release_secs": p.release_secs }));
            }
            let e = library::entry(id).ok_or_else(|| format!("unknown instrument {id:?} (see `ccez-compose patches`)"))?;
            let root = library::cache_root();
            library::ensure(&root, &library::lock(), e)?;
            let regions = library::regions(&root, e)?;
            let (layers, rr) = library::layer_counts(&regions);
            let span = SampledInstrument::new(regions.clone(), e.style).key_span();
            let s = library::source(e.source).expect("source");
            let mut v = json!({ "id": e.id, "kind": "sampled", "family": e.family, "description": e.description,
                "range": [e.range.0, e.range.1], "keys_with_recordings": span.map(|(a, b)| json!([a, b])),
                "velocity_layers": layers, "round_robins": rr, "regions": regions.len(), "pan": e.pan,
                "one_shot": e.style.one_shot, "source": s.name, "licence": s.licence, "licence_url": s.licence_url,
                "homepage": s.homepage });
            if e.style.one_shot && matches!(e.family, "percussion" | "hand_drums") {
                v["hits"] = json!(library::hits(&regions)
                    .into_iter()
                    .map(|(k, n)| (k.to_string(), Value::String(n)))
                    .collect::<serde_json::Map<_, _>>());
            }
            Ok(v)
        }
        "fetch" => {
            let root = library::cache_root();
            let lock = library::lock();
            let ids: Vec<&str> = if args.get(1).map(String::as_str) == Some("all") {
                library::CATALOG.iter().map(|e| e.id).collect()
            } else {
                args[1..].iter().map(String::as_str).collect()
            };
            if ids.is_empty() {
                return Err("fetch: name instruments (or all)".into());
            }
            let mut out = Vec::new();
            for id in ids {
                let e = library::entry(id).ok_or_else(|| format!("unknown sampled instrument {id:?}"))?;
                let bytes = library::ensure(&root, &lock, e)?;
                out.push(json!({ "id": id, "downloaded_bytes": bytes }));
            }
            Ok(json!({ "cache": root.display().to_string(), "fetched": out }))
        }
        "samples" => samples(&args[1..]),
        "validate" => {
            let c = load(args.get(1).ok_or("validate: missing <composition.json>")?)?;
            let r = c.validate_report();
            Ok(json!({ "ok": r.errors.is_empty() && r.pending.is_empty(), "errors": r.errors, "pending": r.pending }))
        }
        "preview" => {
            let c = load(args.get(1).ok_or("preview: missing <composition.json>")?)?;
            let out = PathBuf::from(flag(&args, "--out").ok_or("preview: missing --out <file>")?);
            let errors = c.validate();
            if !errors.is_empty() {
                return Err(format!("composition is not renderable:\n- {}", errors.join("\n- ")));
            }
            let plan = plan_for(&c, &args)?;
            let rendered = render_stems(&c)?;
            let (mix, stats) = render_preview(&c, &rendered, &plan)?;
            write_audio(&out, &wav_bytes(&interleave(&mix[0], &mix[1]), 2))?;
            Ok(json!({ "out": out.display().to_string(), "plan": plan, "stats": stats }))
        }
        "export" => {
            let c = load(args.get(1).ok_or("export: missing <composition.json>")?)?;
            let out = PathBuf::from(flag(&args, "--out").ok_or("export: missing --out <dir>")?);
            let plan = plan_for(&c, &args)?;
            let report = audioforge::export(&c, &plan, &out)?;
            Ok(serde_json::to_value(report).map_err(|e| e.to_string())?)
        }
        _ => Err(format!(
            "unknown command {cmd:?} (want validate | preview | export | patches | instrument | fetch | samples)"
        )),
    }
}

/// Maintainer commands for the sample cache and lock.
fn samples(args: &[String]) -> Result<Value, String> {
    let root = library::cache_root();
    match args.first().map(String::as_str).unwrap_or("") {
        "lock" => {
            let out = flag(args, "--out").ok_or("samples lock: missing --out <sample-lock.json>")?;
            let lock = library::build_lock(&root)?;
            let text = serde_json::to_string_pretty(&lock).map_err(|e| e.to_string())? + "\n";
            std::fs::write(&out, text).map_err(|e| format!("{out}: {e}"))?;
            let files: usize = lock.sources.values().map(|m| m.len()).sum();
            let bytes: u64 = lock.sources.values().flat_map(|m| m.values()).map(|f| f.bytes).sum();
            Ok(json!({ "out": out, "instruments": lock.instruments.len(), "files": files, "bytes": bytes }))
        }
        "verify" => {
            let lock = library::lock();
            let (mut ok, mut missing, mut bad) = (0, 0, Vec::new());
            for (src, files) in &lock.sources {
                for (rel, lf) in files {
                    let p = library::local_path(&root, src, rel);
                    if !p.exists() {
                        missing += 1;
                        continue;
                    }
                    match ccez_core::compose::sha256::sha256_file(&p) {
                        Ok(h) if h == lf.sha256 => ok += 1,
                        _ => bad.push(format!("{src}/{rel}")),
                    }
                }
            }
            Ok(json!({ "cache": root.display().to_string(), "ok": ok, "not_downloaded": missing, "bad": bad }))
        }
        "calibrate" => {
            // Level of a mezzo-forte note (two beats at 120 bpm) through each
            // instrument, unity gain, for setting catalog gain_db by ear-free
            // numbers. Prints rms dBFS per instrument.
            let lock = library::lock();
            let ids: Vec<&str> = if args.len() > 1 {
                args[1..].iter().map(String::as_str).collect()
            } else {
                library::CATALOG.iter().map(|e| e.id).collect()
            };
            let mut out = serde_json::Map::new();
            for id in ids {
                let e = library::entry(id).ok_or_else(|| format!("unknown {id:?}"))?;
                library::ensure(&root, &lock, e)?;
                let mut regions = library::regions(&root, e)?;
                for r in &mut regions {
                    r.volume_db -= e.gain_db;
                }
                let mut inst = SampledInstrument::new(regions, e.style);
                let keys: Vec<u8> = if e.style.one_shot && matches!(e.family, "percussion" | "hand_drums") {
                    library::hits(&inst.regions).keys().copied().collect()
                } else {
                    let mid = (e.range.0 as u16 + e.range.1 as u16) / 2;
                    vec![mid as u8, (mid as u8).saturating_sub(5), (mid + 4).min(127) as u8]
                };
                let (mut energy, mut frames, mut peak) = (0.0f64, 0usize, 0.0f32);
                for k in keys {
                    let (l, r) = inst.render_note(k, 0.7, 1.0, 48_000, 0.5)?;
                    // Loudness over the first second (what a listener weighs).
                    let n = l.len().min(48_000);
                    for i in 0..n {
                        energy += 0.5 * ((l[i] as f64).powi(2) + (r[i] as f64).powi(2));
                        peak = peak.max(l[i].abs()).max(r[i].abs());
                    }
                    frames += 48_000;
                }
                let rms = (energy / frames.max(1) as f64).sqrt();
                out.insert(id.to_string(), json!({ "rms_dbfs": 20.0 * rms.max(1e-9).log10(), "peak_dbfs": 20.0 * (peak.max(1e-9)).log10() }));
            }
            Ok(Value::Object(out))
        }
        other => Err(format!("samples {other:?}: want lock --out <file> | verify | calibrate [<id>...]")),
    }
}

/// Write WAV bytes, or encode MP3 through ffmpeg when `out` ends in .mp3.
pub fn write_audio(out: &Path, wav: &[u8]) -> Result<(), String> {
    if let Some(parent) = out.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    let is_mp3 = out
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("mp3"));
    if !is_mp3 {
        return std::fs::write(out, wav).map_err(|e| format!("{}: {e}", out.display()));
    }
    let tmp = out.with_extension("tmp.wav");
    std::fs::write(&tmp, wav).map_err(|e| format!("{}: {e}", tmp.display()))?;
    let status = Command::new("ffmpeg")
        .args(["-y", "-loglevel", "error", "-i"])
        .arg(&tmp)
        .args(["-codec:a", "libmp3lame", "-q:a", "2"])
        .arg(out)
        .status();
    let _ = std::fs::remove_file(&tmp);
    match status {
        Ok(s) if s.success() => Ok(()),
        Ok(s) => Err(format!("ffmpeg failed ({s}) encoding {}", out.display())),
        Err(e) => Err(format!(
            "mp3 needs ffmpeg on PATH ({e}); ask for a .wav instead"
        )),
    }
}
