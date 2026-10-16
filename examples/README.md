# Example compositions

- `demo_main.composition.json`: the first agent-composed piece,
  written only through the MCP `compose_*` tools on 2026-10-07 as a
  tooling test (not any game's music). It has an
  intro, calm and tense loops, each with a `bed` (intensity 1) and a
  `lift` (intensity 3) layer, and an outro. Re-export it with:

  ```sh
  cargo build --release --manifest-path core/Cargo.toml --bin ccez-compose
  target/release/ccez-compose export examples/demo_main.composition.json --out /tmp/pkg
  target/release/ccez-compose preview examples/demo_main.composition.json --out /tmp/preview.mp3
  ```

- `demo_main.sampled.composition.json`: the same notes and layers with
  sampled instruments (2026-10-15): piano, choir oohs, contrabass, cello
  pizzicato, vibraphone, violins, xylophone and timpani in place of the
  synth patches, balanced to the original's relative levels (bass pulled
  down: a sine bass measures louder than it sounds). The first render
  downloads about 0.8 GB of samples into the cache
  (`docs/notes/sample-libraries.md`):

  ```sh
  target/release/ccez-compose preview examples/demo_main.sampled.composition.json --out /tmp/sampled.mp3
  ```
