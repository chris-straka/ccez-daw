# ga-compose: agents compose game music, audioforge plays it (2026-10-07)

The game-music path, end to end: an agent writes notes through the MCP
`compose_*` tools; core renders them with sampled orchestral instruments
(or the built-in synth patches) into seamless stereo loop stems;
`ccez-compose export` writes an audioforge package
(`contracts/audioforge-music.md`); audioforge plays it in a Bevy game with
`MusicSection` / `MusicIntensity`.

## Pieces

- `core/src/compose/` — `Composition` (sections: intro/loop/outro;
  layers with `min_intensity` 1-5; instruments with `pan`; parts of
  notes; an optional shared `room`), `library` (45 sampled instruments
  from CC0/MIT libraries, downloaded on demand and checked by SHA-256:
  `docs/notes/sample-libraries.md`), `sampler` + `sfz` + `sf2` (our own
  multi-sample player: velocity layers, round robins, loops), `synth`
  (8 patches: felt_piano, warm_pad, strings, soft_bass, pizzicato,
  celesta, clock_tick, low_drum), `reverb` (the shared stereo hall: early
  reflections + an 8-line FDN), `render` (stereo stems with
  release/reverb tails folded onto the loop start; a preview mix with
  per-step, per-layer and per-instrument loudness and loop-seam checks),
  `audioforge` (the package writer).
- `core/src/bin/ccez-compose.rs` — `validate | preview | export | patches
  | instrument | fetch | samples`, one JSON object on stdout. MP3 previews
  go through `ffmpeg`, sample downloads through `curl`.
- `mcp/src/compose.ts` — the tools; they shell out to the CLI. Agents
  without an MCP client: `bun run start:http` then
  `bun scripts/call.ts <tool> '<json>'` (or `@file.json`).
- `contracts/fixtures/audioforge-tiny.composition.json` — shared fixture;
  `scripts/export-audioforge-fixture.sh` regenerates audioforge's copy,
  which `audioforge/crates/af-runtime/tests/ccez_daw_export.rs` plays.

## First agent session: rough edges hit, and what happened to them

Composing a test main theme (intro, calm + tense loops with
a `bed` layer at intensity 1 and a `lift` layer at intensity 3, outro)
through the MCP server over HTTP:

1. **HTTP mode returned empty bodies** for every call: the per-request
   transport was closed in `finally` before its SSE body streamed. Fixed:
   JSON responses (`enableJsonResponse`).
2. **No way in without an MCP client config.** Fixed: `mcp/scripts/call.ts`.
3. **Every call during build-up said `ok: false`** ("need a loop
   section", an intro's `next` not defined yet), so real mistakes were
   hidden in noise. Fixed: validation splits `errors` (wrong now) from
   `pending` (referenced, not defined yet); `ok` still means exportable.
4. **Note arrays were impractical by hand** (a 64-note ostinato is 64 JSON
   objects); the agent ended up scripting the payloads. Fixed:
   `compose_write_notes` takes `text` (`A4/1.5 G4/0.5 F4 D3+F3+A3/4 r/2`,
   sticky lengths, `@vel`) with `start` and `repeat`.
5. **No ears.** Preview stats gave only per-step loudness, so whether an
   intensity layer is noticeable was a guess. Fixed: per-layer loudness
   per step (`layer_rms_dbfs`). Still open: no spectrogram/picture tool.
6. Open: the composition is not a DAW project. It doesn't show in the
   timeline, piano roll or mixer, and there is no palette action for it.
7. Closed (2026-10-15): sampled instruments. 45 of them (string
   sections sustained and pizzicato, solo violin, flute, piccolo, oboe,
   clarinet, bassoon, horn, trumpet, trombone, tuba, Kawai grand piano,
   harp, kalimba, glockenspiel, marimba, xylophone, vibraphone, tubular
   bells, wordless choir aahs/oohs, timpani, orchestral and hand drums)
   from VSCO 2 CE and VCSL (CC0) and FluidR3_GM (MIT), with their
   velocity layers and round robins; our own SFZ/SF2 sampler; local
   `sfz:`/`sf2:` files work too. Proof: the example and a game theme
   re-rendered (`examples/demo_main.sampled.composition.json`).
8. Closed (2026-10-15): stems are stereo. Each instrument sits at a pan
   position (default: its orchestral seat) and sends into one shared
   stereo hall per stem; the hall's tail folds onto the loop start like
   the releases, so loops stay seamless (`loop_seams` in preview and
   export reports). audioforge loads the stereo stems (it still
   downmixes music stems to mono when it plays them).
   Fixed 2026-10-20: the stage pan was only applied to synth voices;
   sampled instruments (catalog seats and `pan` overrides alike) now
   take the same constant-power law, as a balance for stereo
   recordings (see `sample-libraries.md`, "Pan law").
9. Open: each mutation re-runs the Rust validator as a subprocess (fast
   normally, seconds on a loaded box).
