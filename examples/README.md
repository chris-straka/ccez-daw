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

- `island_travel/`: an island-travel theme written with the writing and
  listening tools (2026-10-16): a one-bar motif stated, sequenced down the
  scale for bars 1-3, inverted and augmented for the B section, chords
  voice-led (and found for the B tune by the re-harmoniser), arranged to
  the ensemble in `island_travel/music.md` on `calm` and `lively` layers,
  layer EQ, mastered, then heard against a reference track. The session
  is a script of MCP calls:

  ```sh
  bun mcp/scripts/island-travel-session.ts /tmp/island [reference.ogg] [lufs]
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
