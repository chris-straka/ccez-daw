# ga-compose: agents compose game music, audioforge plays it (2026-10-07)

The game-music path, end to end: an agent writes notes through the MCP
`compose_*` tools; core renders them with built-in synth patches into
seamless loop stems; `ccez-compose export` writes an audioforge package
(`contracts/audioforge-music.md`); audioforge plays it in a Bevy game with
`MusicSection` / `MusicIntensity`.

## Pieces

- `core/src/compose/` — `Composition` (sections: intro/loop/outro;
  layers with `min_intensity` 1-5; instruments; parts of notes), `synth`
  (8 patches: felt_piano, warm_pad, strings, soft_bass, pizzicato,
  celesta, clock_tick, low_drum), `render` (stems with release/reverb
  tails folded onto the loop start; a preview mix with per-step and
  per-layer loudness), `audioforge` (the package writer).
- `core/src/bin/ccez-compose.rs` — `validate | preview | export | patches`,
  one JSON object on stdout. MP3 previews go through `ffmpeg`.
- `mcp/src/compose.ts` — the tools; they shell out to the CLI. Agents
  without an MCP client: `bun run start:http` then
  `bun scripts/call.ts <tool> '<json>'` (or `@file.json`).
- `contracts/fixtures/audioforge-tiny.composition.json` — shared fixture;
  `scripts/export-audioforge-fixture.sh` regenerates audioforge's copy,
  which `audioforge/crates/af-runtime/tests/ccez_daw_export.rs` plays.

## First agent session: rough edges hit, and what happened to them

Composing The Interpreter's main theme test (intro, calm + tense loops with
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
7. Open: 8 synth patches are the whole palette (no samples or
   soundfonts), which caps how good it can sound.
8. Open: stems are mono (audioforge downmixes; pan is per layer).
9. Open: each mutation re-runs the Rust validator as a subprocess (fast
   normally, seconds on a loaded box).
