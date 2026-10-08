//! `ccez-compose`: render and export game-music compositions headless.
//!
//! ```text
//! ccez-compose validate <composition.json>
//! ccez-compose preview  <composition.json> --out <file.wav|file.mp3> [--plan <plan.json>]
//! ccez-compose export   <composition.json> --out <dir> [--plan <plan.json>]
//! ccez-compose patches
//! ```
//!
//! Every command prints one JSON object on stdout (the MCP `compose_*`
//! tools parse it) and exits non-zero with a plain-text reason on stderr.
//! `--plan` is a JSON array of `{ "section", "bars"?, "intensity"? }`
//! steps; the default plays intro, each loop at intensity 1 then 5, outro.
//! MP3 previews need `ffmpeg` on PATH.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use ccez_core::compose::render::{default_plan, render_preview, render_stems, wav_bytes, PlanStep};
use ccez_core::compose::{audioforge, synth, Composition};
use serde_json::json;

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
        "patches" => Ok(json!(synth::PATCHES
            .iter()
            .map(|p| json!({ "id": p.id, "description": p.description, "range": [p.range.0, p.range.1] }))
            .collect::<Vec<_>>())),
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
            let (stems, _) = render_stems(&c);
            let (mix, stats) = render_preview(&c, &stems, &plan)?;
            write_audio(&out, &wav_bytes(&mix, 1))?;
            Ok(json!({ "out": out.display().to_string(), "plan": plan, "stats": stats }))
        }
        "export" => {
            let c = load(args.get(1).ok_or("export: missing <composition.json>")?)?;
            let out = PathBuf::from(flag(&args, "--out").ok_or("export: missing --out <dir>")?);
            let plan = plan_for(&c, &args)?;
            let report = audioforge::export(&c, &plan, &out)?;
            Ok(serde_json::to_value(report).map_err(|e| e.to_string())?)
        }
        _ => Err(format!("unknown command {cmd:?} (want validate | preview | export | patches)")),
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
