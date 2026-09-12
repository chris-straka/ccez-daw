//! `typegen` binary: render Rust contracts to `ui/src/generated/*.ts`.
//!
//! - `bun run typegen`         — rewrite the generated files.
//! - `bun run typegen -- --check` — exit nonzero if committed files drift.

use std::path::PathBuf;

fn out_dir() -> PathBuf {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest
        .join("..")
        .join("ui")
        .join("src")
        .join("generated")
}

fn write_or_check(name: &str, fresh: &str, check: bool) -> bool {
    let path = out_dir().join(name);
    if check {
        let committed = std::fs::read_to_string(&path).unwrap_or_default();
        if committed == fresh {
            println!("ok   {name}");
            true
        } else {
            eprintln!("DRIFT {name}: committed file differs from Rust source.");
            eprintln!("Run `bun run typegen` to regenerate, then review the diff.");
            false
        }
    } else {
        std::fs::create_dir_all(out_dir()).expect("create generated dir");
        std::fs::write(&path, fresh).expect("write generated file");
        println!("wrote {}", path.display());
        true
    }
}

fn main() {
    let check = std::env::args().any(|a| a == "--check");
    let mut ok = true;
    ok &= write_or_check("project.ts", &ccez_core::emit::render_project_ts(), check);
    ok &= write_or_check("ipc.ts", &ccez_core::emit::render_ipc_ts(), check);
    if !ok {
        std::process::exit(1);
    }
}
