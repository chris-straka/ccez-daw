//! `typegen` binary: render Rust contracts to `ui/src/generated/*.ts`.
//!
//! - `bun run typegen`         — rewrite the generated files.
//! - `bun run typegen -- --check` — exit nonzero if committed files drift.

use std::path::PathBuf;
use std::process::ExitCode;

fn out_dir() -> PathBuf {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest.join("..").join("ui").join("src").join("generated")
}

fn write_or_check(name: &str, fresh: &str, check: bool) -> Result<bool, String> {
    let dir = out_dir();
    let path = dir.join(name);
    if check {
        let committed = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(format!("{}: {e}", path.display())),
        };
        if committed == fresh {
            println!("ok   {name}");
            Ok(true)
        } else {
            eprintln!("DRIFT {name}: committed file differs from Rust source.");
            eprintln!("Run `bun run typegen` to regenerate, then review the diff.");
            Ok(false)
        }
    } else {
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        std::fs::write(&path, fresh).map_err(|e| format!("{}: {e}", path.display()))?;
        println!("wrote {}", path.display());
        Ok(true)
    }
}

fn main() -> ExitCode {
    let check = std::env::args().any(|a| a == "--check");
    let files = [
        ("project.ts", ccez_core::emit::render_project_ts()),
        ("ipc.ts", ccez_core::emit::render_ipc_ts()),
    ];
    let mut ok = true;
    for (name, content) in files {
        match write_or_check(name, &content, check) {
            Ok(matches) => ok &= matches,
            Err(e) => {
                eprintln!("typegen: {e}");
                return ExitCode::FAILURE;
            }
        }
    }
    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
