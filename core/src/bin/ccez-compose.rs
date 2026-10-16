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
//! ccez-compose listen    <composition.json|audio> [--reference <audio>] [--spectrogram <out.jpg>] [--plan <plan.json>]
//! ccez-compose compare   <a audio> <b audio>
//! ccez-compose progression <request.json>
//! ccez-compose reharmonise <request.json>
//! ccez-compose motif     <request.json>
//! ccez-compose arrange   <composition.json> <request.json>
//! ccez-compose ensemble  <music.md>
//! ccez-compose master    <in audio> --out <out.wav|.mp3> [--lufs -16] [--ceiling -1] [--loop]
//! ccez-compose transcribe <audio|.mid|.csv> --out <composition.json> [--midi <out.mid>] [--tempo <bpm>]
//!                        [--bars 8] [--start-bar 0] [--key "D major"] [--name take] [--from <s>] [--to <s>]
//! ccez-compose daw       <composition.json> --out <project dir>
//! ```
//!
//! `listen` is an agent's ears: tempo, key, density, loudness (curve and
//! range), brightness, width, and for a composition each layer's weight,
//! section likeness, register clashes and stray notes; `--reference`
//! adds how far it is from another track, aspect by aspect.
//! `progression`/`reharmonise`/`motif`/`arrange` are the writing help
//! (`core/src/compose/theory.rs`, `arrange.rs`); `master` the loop-safe
//! mastering pass (or `$CCEZ_MASTER_CMD`, see `master`); `transcribe`
//! turns audio into an editable composition through basic-pitch; `daw`
//! writes a project bundle the DAW opens (Open project).
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
use ccez_core::compose::theory::{self, Key, MotifOp, Pattern, VoicingOpts};
use ccez_core::compose::{arrange, audioforge, daw, fx, library, listen, synth, transcribe, Composition, Note};
use serde::Deserialize;
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

fn read_json<T: for<'de> Deserialize<'de>>(path: &str) -> Result<T, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    serde_json::from_str(&text).map_err(|e| format!("{path}: {e}"))
}

fn num_flag(args: &[String], name: &str) -> Result<Option<f64>, String> {
    flag(args, name)
        .map(|v| v.parse::<f64>().map_err(|_| format!("{name} {v:?}: want a number")))
        .transpose()
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
            let (mut mix, stats) = render_preview(&c, &rendered, &plan)?;
            // The full mastering chain on the preview when the composition asks.
            let master = match &c.master {
                Some(m) => {
                    let [l, r] = &mut mix;
                    Some(master_audio(l, r, m, false)?)
                }
                None => None,
            };
            write_audio(&out, &wav_bytes(&interleave(&mix[0], &mix[1]), 2))?;
            Ok(json!({ "out": out.display().to_string(), "plan": plan, "stats": stats,
                       "master": master, "stem_master_gain": rendered.master }))
        }
        "export" => {
            let c = load(args.get(1).ok_or("export: missing <composition.json>")?)?;
            let out = PathBuf::from(flag(&args, "--out").ok_or("export: missing --out <dir>")?);
            let plan = plan_for(&c, &args)?;
            let report = audioforge::export(&c, &plan, &out)?;
            Ok(serde_json::to_value(report).map_err(|e| e.to_string())?)
        }
        "listen" => cmd_listen(&args),
        "compare" => {
            let a = args.get(1).ok_or("compare: missing <a audio>")?;
            let b = args.get(2).ok_or("compare: missing <b audio>")?;
            let fa = audio_features(Path::new(a))?;
            let fb = audio_features(Path::new(b))?;
            let cmp = listen::compare(&fa, &fb, "A", "B");
            Ok(json!({ "a": { "file": a, "summary": listen::describe(&fa), "features": fa },
                       "b": { "file": b, "summary": listen::describe(&fb), "features": fb },
                       "comparison": cmp }))
        }
        "progression" => cmd_progression(&read_json(args.get(1).ok_or("progression: missing <request.json>")?)?),
        "reharmonise" => cmd_reharmonise(&read_json(args.get(1).ok_or("reharmonise: missing <request.json>")?)?),
        "motif" => {
            let r: MotifReq = read_json(args.get(1).ok_or("motif: missing <request.json>")?)?;
            let key = Key::parse(&r.key)?;
            let notes = theory::transform(&r.notes, &key, &r.ops)?;
            Ok(json!({ "key": key.name(), "notes": notes_json(&notes) }))
        }
        "arrange" => {
            let c = load(args.get(1).ok_or("arrange: missing <composition.json>")?)?;
            let req: arrange::Request = read_json(args.get(2).ok_or("arrange: missing <request.json>")?)?;
            let a = arrange::arrange(&c, &req)?;
            Ok(serde_json::to_value(a).map_err(|e| e.to_string())?)
        }
        "ensemble" => {
            let p = args.get(1).ok_or("ensemble: missing <music.md>")?;
            let md = std::fs::read_to_string(p).map_err(|e| format!("{p}: {e}"))?;
            Ok(json!({ "ensemble": arrange::ensemble_from_markdown(&md)? }))
        }
        "master" => cmd_master(&args),
        "transcribe" => cmd_transcribe(&args),
        "daw" => {
            let c = load(args.get(1).ok_or("daw: missing <composition.json>")?)?;
            let out = flag(&args, "--out").ok_or("daw: missing --out <project dir>")?;
            Ok(serde_json::to_value(daw::write_project(&c, Path::new(&out))?).map_err(|e| e.to_string())?)
        }
        _ => Err(format!(
            "unknown command {cmd:?} (want validate | preview | export | patches | instrument | fetch | samples | listen | compare | progression | reharmonise | motif | arrange | ensemble | master | transcribe | daw)"
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

fn audio_features(path: &Path) -> Result<listen::Features, String> {
    let a = listen::load_audio(path)?;
    Ok(listen::analyse(&a.left, &a.right, a.sample_rate))
}

fn cmd_listen(args: &[String]) -> Result<Value, String> {
    let input = args.get(1).ok_or("listen: missing <composition.json|audio>")?;
    let reference = flag(args, "--reference");
    let spectrogram = flag(args, "--spectrogram");
    let mut out = serde_json::Map::new();
    // The audio an agent "hears": a composition's preview mix, or the file.
    let (left, right, sr, feats) = if input.ends_with(".json") {
        let c = load(input)?;
        let errors = c.validate();
        if !errors.is_empty() {
            return Err(format!("composition is not renderable:\n- {}", errors.join("\n- ")));
        }
        let (report, rendered) = listen::listen_composition(&c)?;
        let plan = plan_for(&c, args)?;
        let (mut mix, _) = render_preview(&c, &rendered, &plan)?;
        if let Some(m) = &c.master {
            let [l, r] = &mut mix;
            master_audio(l, r, m, false)?;
        }
        let [l, r] = mix;
        let f = listen::analyse(&l, &r, ccez_core::compose::SAMPLE_RATE);
        out.insert("preview".into(), json!({ "plan": plan, "summary": listen::describe(&f), "features": f }));
        out.insert("composition".into(), serde_json::to_value(&report).map_err(|e| e.to_string())?);
        (l, r, ccez_core::compose::SAMPLE_RATE, f)
    } else {
        let a = listen::load_audio(Path::new(input))?;
        let f = listen::analyse(&a.left, &a.right, a.sample_rate);
        out.insert("summary".into(), json!(listen::describe(&f)));
        out.insert("features".into(), serde_json::to_value(&f).map_err(|e| e.to_string())?);
        (a.left, a.right, a.sample_rate, f)
    };
    if let Some(r) = reference {
        let fr = audio_features(Path::new(&r))?;
        let cmp = listen::compare(&fr, &feats, "the reference", "this");
        out.insert(
            "reference".into(),
            json!({ "file": r, "summary": listen::describe(&fr), "comparison": cmp,
                    "distance": 1.0 - cmp.overall }),
        );
    }
    if let Some(p) = spectrogram {
        listen::spectrogram_jpeg(&left, &right, sr, Path::new(&p))?;
        out.insert("spectrogram".into(), json!(p));
    }
    Ok(Value::Object(out))
}

fn notes_json(notes: &[Note]) -> Value {
    json!(notes
        .iter()
        .map(|n| json!({ "pitch": theory::pitch_name(n.pitch), "start": n.start, "length": n.length, "velocity": n.velocity }))
        .collect::<Vec<_>>())
}

#[derive(Deserialize)]
struct MotifReq {
    key: String,
    notes: Vec<Note>,
    #[serde(default)]
    ops: Vec<MotifOp>,
}

fn d_every() -> f32 {
    4.0
}
fn d_bpb() -> u32 {
    4
}

#[derive(Deserialize)]
struct ProgressionReq {
    key: String,
    /// Numerals or symbols; or `preset`.
    #[serde(default)]
    chords: Vec<String>,
    #[serde(default)]
    preset: Option<String>,
    /// Beats per chord.
    #[serde(default = "d_every")]
    every: f32,
    #[serde(default = "d_bpb")]
    beats_per_bar: u32,
    #[serde(default)]
    voicing: Option<VoicingOpts>,
    #[serde(default)]
    pattern: Option<Pattern>,
    #[serde(default)]
    subdivision: Option<f32>,
    #[serde(default)]
    velocity: Option<f32>,
    #[serde(default)]
    bass_pattern: Option<Pattern>,
}

fn voiced_json(key: &Key, chords: &[(theory::Chord, f32, f32)], r_voicing: Option<VoicingOpts>, pattern: Option<Pattern>, bass: Option<Pattern>, sub: Option<f32>, vel: Option<f32>, bpb: u32) -> Result<Value, String> {
    let opts = r_voicing.unwrap_or_default();
    let v = theory::voice_lead(chords, &opts)?;
    let upper = theory::realise(&v, pattern.unwrap_or(Pattern::Block), sub.unwrap_or(0.5), vel.unwrap_or(0.55), bpb);
    let bass = theory::realise(&v, bass.unwrap_or(Pattern::RootFifth), sub.unwrap_or(0.5), vel.unwrap_or(0.55) + 0.1, bpb);
    Ok(json!({
        "key": key.name(),
        "chords": v.iter().map(|c| json!({ "start": c.start, "length": c.length, "symbol": c.chord.symbol, "numeral": c.chord.numeral,
            "bass": theory::pitch_name(c.bass), "voices": c.voices.iter().map(|p| theory::pitch_name(*p)).collect::<Vec<_>>() })).collect::<Vec<_>>(),
        "parallel_fifths_octaves": theory::parallels(&v),
        "upper": notes_json(&upper),
        "bass": notes_json(&bass),
    }))
}

fn cmd_progression(r: &ProgressionReq) -> Result<Value, String> {
    let key = Key::parse(&r.key)?;
    let names: Vec<String> = match (&r.preset, r.chords.is_empty()) {
        (Some(p), true) => theory::preset(p)
            .ok_or_else(|| format!("unknown preset {p:?} (known: {})", theory::PRESETS.iter().map(|x| x.0).collect::<Vec<_>>().join(", ")))?
            .split_whitespace()
            .map(String::from)
            .collect(),
        (_, false) => r.chords.clone(),
        (None, true) => {
            return Ok(json!({ "presets": theory::PRESETS.iter().map(|(n, c, d)| json!({ "name": n, "chords": c, "feel": d })).collect::<Vec<_>>() }))
        }
    };
    let chords: Vec<(theory::Chord, f32, f32)> = names
        .iter()
        .enumerate()
        .map(|(i, s)| theory::parse_chord(s, &key).map(|c| (c, i as f32 * r.every, r.every)))
        .collect::<Result<_, _>>()?;
    voiced_json(&key, &chords, r.voicing, r.pattern, r.bass_pattern, r.subdivision, r.velocity, r.beats_per_bar)
}

#[derive(Deserialize)]
struct ReharmReq {
    key: String,
    melody: Vec<Note>,
    #[serde(default)]
    beats: Option<f32>,
    #[serde(default = "d_every")]
    every: f32,
    #[serde(default = "d_bpb")]
    beats_per_bar: u32,
    #[serde(default)]
    palette: theory::Palette,
    #[serde(default)]
    voicing: Option<VoicingOpts>,
    #[serde(default)]
    pattern: Option<Pattern>,
}

fn cmd_reharmonise(r: &ReharmReq) -> Result<Value, String> {
    let key = Key::parse(&r.key)?;
    let end = r.melody.iter().map(|n| n.start + n.length).fold(0.0f32, f32::max);
    let beats = r.beats.unwrap_or((end / r.beats_per_bar as f32).ceil().max(1.0) * r.beats_per_bar as f32);
    let chords = theory::reharmonise(&r.melody, &key, beats, r.every, r.beats_per_bar, r.palette)?;
    voiced_json(&key, &chords, r.voicing, r.pattern, None, None, None, r.beats_per_bar)
}

/// Master in place: `$CCEZ_MASTER_CMD` (e.g. genforge's pass: a command
/// template with `{in}`, `{out}`, `{lufs}`, `{ceiling}`) when set, else
/// the built-in chain.
fn master_audio(l: &mut [f32], r: &mut [f32], m: &fx::Master, looped: bool) -> Result<fx::MasterReport, String> {
    let Ok(tpl) = std::env::var("CCEZ_MASTER_CMD") else {
        return Ok(fx::master_stereo(l, r, ccez_core::compose::SAMPLE_RATE as f32, m, looped));
    };
    let dir = std::env::temp_dir();
    let (inp, outp) = (dir.join(format!("ccez-master-in-{}.wav", std::process::id())), dir.join(format!("ccez-master-out-{}.wav", std::process::id())));
    std::fs::write(&inp, ccez_core::compose::wavread::encode_pcm16(ccez_core::compose::SAMPLE_RATE, &[l.to_vec(), r.to_vec()]))
        .map_err(|e| e.to_string())?;
    let cmdline = tpl
        .replace("{in}", &inp.display().to_string())
        .replace("{out}", &outp.display().to_string())
        .replace("{lufs}", &m.lufs.to_string())
        .replace("{ceiling}", &m.ceiling_dbtp.to_string());
    let (lb, tpb) = (
        ccez_core::meter::lufs::analyse_offline(&[l, r], ccez_core::compose::SAMPLE_RATE as f32).0,
        fx::true_peak_dbtp(&[l, r]),
    );
    let st = Command::new("sh").arg("-c").arg(&cmdline).status().map_err(|e| format!("CCEZ_MASTER_CMD: {e}"))?;
    if !st.success() {
        return Err(format!("CCEZ_MASTER_CMD failed ({st}): {cmdline}"));
    }
    let a = ccez_core::compose::wavread::read(&outp)?;
    let _ = std::fs::remove_file(&inp);
    let _ = std::fs::remove_file(&outp);
    let n = l.len().min(a.frames());
    l[..n].copy_from_slice(&a.left()[..n]);
    r[..n].copy_from_slice(&a.right()[..n]);
    Ok(fx::MasterReport {
        engine: format!("external: {cmdline}"),
        lufs_before: lb,
        lufs_after: ccez_core::meter::lufs::analyse_offline(&[l, r], ccez_core::compose::SAMPLE_RATE as f32).0,
        true_peak_before_dbtp: tpb,
        true_peak_after_dbtp: fx::true_peak_dbtp(&[l, r]),
        gain_db: 0.0,
        max_reduction_db: 0.0,
        limited_share: 0.0,
        looped,
        seam_step: None,
        seamless: None,
    })
}

fn cmd_master(args: &[String]) -> Result<Value, String> {
    let input = args.get(1).ok_or("master: missing <in audio>")?;
    let out = PathBuf::from(flag(args, "--out").ok_or("master: missing --out <file>")?);
    let m = fx::Master {
        lufs: num_flag(args, "--lufs")?.map_or(-16.0, |v| v as f32),
        ceiling_dbtp: num_flag(args, "--ceiling")?.map_or(-1.0, |v| v as f32),
    };
    if let Some(e) = m.validate().first() {
        return Err(e.clone());
    }
    let mut a = listen::load_audio(Path::new(input))?;
    if a.sample_rate != ccez_core::compose::SAMPLE_RATE {
        return Err(format!("{input}: {} Hz; master works at 48 kHz (convert first)", a.sample_rate));
    }
    let looped = args.iter().any(|x| x == "--loop");
    let report = master_audio(&mut a.left, &mut a.right, &m, looped)?;
    write_audio(&out, &wav_bytes(&interleave(&a.left, &a.right), 2))?;
    Ok(json!({ "out": out.display().to_string(), "master": report }))
}

fn cmd_transcribe(args: &[String]) -> Result<Value, String> {
    let input = args.get(1).ok_or("transcribe: missing <audio|.mid|.csv>")?;
    let out = PathBuf::from(flag(args, "--out").ok_or("transcribe: missing --out <composition.json>")?);
    let lower = input.to_ascii_lowercase();
    let (events, file_tempo, heard) = if lower.ends_with(".mid") || lower.ends_with(".midi") {
        let bytes = std::fs::read(input).map_err(|e| format!("{input}: {e}"))?;
        let (ev, bpm) = transcribe::read_midi(&bytes)?;
        (ev, bpm, None)
    } else if lower.ends_with(".csv") {
        let text = std::fs::read_to_string(input).map_err(|e| format!("{input}: {e}"))?;
        (transcribe::read_basic_pitch_csv(&text)?, None, None)
    } else {
        let from = num_flag(args, "--from")?;
        let to = num_flag(args, "--to")?;
        let midi = flag(args, "--midi").map(PathBuf::from);
        let ev = transcribe::run_basic_pitch(Path::new(input), from, to, midi.as_deref())?;
        // Tempo from our own ears on the same stretch of audio.
        let a = listen::load_audio(Path::new(input))?;
        let sr = a.sample_rate as f64;
        let (s, e) = ((from.unwrap_or(0.0) * sr) as usize, to.map_or(a.left.len(), |t| ((t * sr) as usize).min(a.left.len())));
        let f = listen::analyse(&a.left[s.min(e)..e], &a.right[s.min(e)..e], a.sample_rate);
        (ev, None, Some(f))
    };
    let tempo = match num_flag(args, "--tempo")? {
        Some(t) => t as f32,
        None => file_tempo.or(heard.as_ref().map(|f| f.tempo.bpm)).ok_or("transcribe: pass --tempo (no tempo in the input)")?,
    };
    let o = transcribe::Options {
        name: flag(args, "--name").unwrap_or_else(|| "take".into()),
        tempo: (tempo * 2.0).round() / 2.0,
        beats_per_bar: 4,
        key: flag(args, "--key"),
        bars: num_flag(args, "--bars")?.map_or(8, |v| v as u32),
        start_bar: num_flag(args, "--start-bar")?.map_or(0, |v| v as u32),
        every: num_flag(args, "--every")?.map(|v| v as f32),
    };
    let (c, t) = transcribe::to_composition(&events, &o)?;
    if let Some(p) = out.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
    }
    std::fs::write(&out, serde_json::to_string_pretty(&c).map_err(|e| e.to_string())? + "\n").map_err(|e| format!("{}: {e}", out.display()))?;
    Ok(json!({ "out": out.display().to_string(), "transcription": t,
               "heard": heard.as_ref().map(listen::describe), "midi": flag(args, "--midi") }))
}
