# Example compositions

- `interpreter_main.composition.json`: the first agent-composed piece,
  written only through the MCP `compose_*` tools on 2026-10-07 as a
  tooling test for The Interpreter (not the game's music). It has an
  intro, calm and tense loops, each with a `bed` (intensity 1) and a
  `lift` (intensity 3) layer, and an outro. Re-export it with:

  ```sh
  cargo build --release --manifest-path core/Cargo.toml --bin ccez-compose
  target/release/ccez-compose export examples/interpreter_main.composition.json --out /tmp/pkg
  target/release/ccez-compose preview examples/interpreter_main.composition.json --out /tmp/preview.mp3
  ```
