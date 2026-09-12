//! `export-validator` binary: approve one engine export package directory.
//!
//! ```text
//! export-validator --package <dir> --context <json> [--seed N] [--sample-rate N]
//!                [--click-threshold F] [--waive <waivers.json>]
//! ```
//!
//! `--context` is a JSON document bundling the authoring side the package was
//! exported from: `{ "project": Project, "cues": [...], "banks": [...],
//! "params": [...] }`. Prints every validator error (one per line) and exits
//! nonzero when the package must not ship. `--seed` / `--sample-rate` must
//! match the export options (defaults: 2026 / 48000 Hz).
//!
//! `--click-threshold` sets the S-2 loop-seam gate (boundary `|last - first|`
//! above this fails; default 0.02; 0 disables). `--waive` points at a
//! `loop-waivers.json` file (`{ "waivers": [{ "path", "reason" }] }`) that
//! excuses named stems — every entry needs a non-empty reason, and entries
//! naming no package stem fail as stale.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use ccez_core::game_audio::{AdaptiveCue, GameStateParam, SfxBank};
use ccez_core::gaexport::{self, ExportOptions};
use ccez_core::loopseam;
use ccez_core::model::Project;

#[derive(Debug, Deserialize, Serialize)]
struct Context {
    project: Project,
    #[serde(default)]
    cues: Vec<AdaptiveCue>,
    #[serde(default)]
    banks: Vec<SfxBank>,
    #[serde(default)]
    params: Vec<GameStateParam>,
}

fn usage() -> ! {
    eprintln!(
        "usage: export-validator --package <dir> --context <json> [--seed N] [--sample-rate N] [--click-threshold F] [--waive <waivers.json>]"
    );
    std::process::exit(2);
}

fn flag_value(args: &[String], flag: &str) -> Option<String> {
    args.windows(2)
        .find(|w| w[0] == flag)
        .map(|w| w[1].clone())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let package = flag_value(&args, "--package").unwrap_or_else(|| usage());
    let context_path = flag_value(&args, "--context").unwrap_or_else(|| usage());
    let seed: u64 = flag_value(&args, "--seed")
        .map(|s| s.parse().unwrap_or_else(|_| usage()))
        .unwrap_or(gaexport::DEFAULT_EXPORT_SEED);
    let sample_rate: u32 = flag_value(&args, "--sample-rate")
        .map(|s| s.parse().unwrap_or_else(|_| usage()))
        .unwrap_or(gaexport::EXPORT_SAMPLE_RATE);
    let options = ExportOptions { sample_rate, seed };
    let threshold: f32 = flag_value(&args, "--click-threshold")
        .map(|s| s.parse().unwrap_or_else(|_| usage()))
        .unwrap_or(loopseam::DEFAULT_CLICK_THRESHOLD);
    let mut loop_opts = loopseam::LoopSeamOptions {
        threshold,
        waivers: std::collections::HashMap::new(),
    };
    if let Some(waive_path) = flag_value(&args, "--waive") {
        match loopseam::load_waivers(std::path::Path::new(&waive_path)) {
            Ok(entries) => {
                for w in entries {
                    loop_opts.waivers.insert(w.path, w.reason);
                }
            }
            Err(e) => {
                eprintln!("export-validator: {e}");
                std::process::exit(2);
            }
        }
    }

    let text = std::fs::read_to_string(&context_path).unwrap_or_else(|e| {
        eprintln!("cannot read context {context_path}: {e}");
        std::process::exit(2);
    });
    let context: Context = serde_json::from_str(&text).unwrap_or_else(|e| {
        eprintln!("invalid context JSON: {e}");
        std::process::exit(2);
    });

    let report = gaexport::validate_package_full(
        &PathBuf::from(&package),
        &context.project,
        &context.cues,
        &context.banks,
        &context.params,
        options,
        &loop_opts,
    );
    if report.is_ok() {
        println!("export-validator: {package} approved (validator version {})", gaexport::VALIDATOR_VERSION);
    } else {
        println!(
            "export-validator: {package} REJECTED ({} error{}):",
            report.errors.len(),
            if report.errors.len() == 1 { "" } else { "s" }
        );
        for e in &report.errors {
            println!("- {e}");
        }
        std::process::exit(1);
    }
}
