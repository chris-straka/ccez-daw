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
  | instrument | fetch | samples | listen | compare | progression |
  reharmonise | motif | arrange | ensemble | master | transcribe | daw`,
  one JSON object on stdout. MP3 previews go through `ffmpeg`, sample
  downloads through `curl`.
- `mcp/src/compose.ts` — the tools; they shell out to the CLI. Agents
  without an MCP client: `bun run start:http` then
  `bun scripts/call.ts <tool> '<json>'` (or `@file.json`).
- `contracts/fixtures/audioforge-tiny.composition.json` — shared fixture;
  `scripts/export-audioforge-fixture.sh` regenerates audioforge's copy,
  which `audioforge/crates/af-runtime/tests/ccez_daw_export.rs` plays.

## What agents can do (2026-10-16)

The owner, 2026-10-16: "Do you think the agentic music creation abilities
of the ccez-daw can be expanded?" Five pieces, all through MCP tools:

- **Ears** (`listen.rs`, `compose_listen`, `compose_compare`): tempo
  (mel-band onset curve, autocorrelation with a bar-level comb and a
  110 BPM prior), key (chroma from spectral peaks, Krumhansl-Kessler),
  onsets per second, brightness, energy per band, stereo width, EBU R128
  loudness with a per-second curve and loudness range, MFCC timbre. A
  composition is also heard part by part: each section at low and full
  intensity, how much each layer adds (LU), section likeness, held notes
  that rub (minor 2nds/9ths) or muddy (close intervals low down),
  notes outside the key or an instrument's range. `reference` compares
  against any track aspect by aspect with plain differences ("this is
  brighter", "this moves more between quiet and loud"). A spectrogram
  JPEG closes old open item 5. Checked on real tracks: an ACE-Step
  world-map take (104 BPM, D major per its provenance) reads 103 BPM, D
  major; two composer themes at 126 BPM in C and D major read 126 BPM and
  the right keys. Timbre likeness is scaled on real pairs: excerpts of one
  piece sit 3-5 apart, one tune re-rendered ~8, different pieces 15-25.
  CLAP/MERT embeddings were not added: our own features already explain
  *how* two pieces differ, which is what an agent acts on, without a
  PyTorch install.
- **Writing help** (`theory.rs`, `arrange.rs`): keys and 11 modes;
  numerals read in the key (`ii7`, `V7/V`, `bVII`, `IV/3`; accidentals
  are relative to the major scale) or symbols (`F#m7b5`); named
  progressions (`royal_road`, `journey`, `mixolydian_sail`, ...); voice
  leading that keeps common tones and avoids parallel fifths/octaves;
  patterns (block, arpeggio, broken, pulse, root-fifth); motifs stated
  once (`motifs` in the document) and varied in the key (transpose by
  steps, diatonic inversion, retrograde, augment, sequence, fragment,
  displace); re-harmonising a tune (Viterbi over melody fit and chord
  motion, plain or colour palette). `compose_arrange` gives an ensemble
  roles (melody, double, sparkle, counter, bass, pad, arp, pulse,
  broken, drone, percussion) inside each instrument's range, keeps the
  pad above the bass register and moves accompaniment notes that would
  rub against the tune. The ensemble can come from a game's
  `design/music.md` `## Ensemble` table (Id | Patch | Role | Layer, plus
  optional Velocity, Subdivision, Keys, Density).
- **Sound** (`fx.rs`): per-layer EQ (low/high cut, shelves, bell), a
  per-layer reverb amount and an optional own hall; `master` in the
  document: previews get DC removal, a 25 Hz high-pass, R128 loudness to
  the target and a look-ahead true-peak limiter (all circular for loops,
  so the wrap stays seamless); exported stems get one shared gain so the
  loudest full-intensity loop sits at the target (capped by peaks, and
  it says so). `compose_master_audio` masters any file. genforge's
  mastering pass (job sn5cpdsw) had not landed on 2026-10-16; set
  `CCEZ_MASTER_CMD` to a command template (`{in}`, `{out}`, `{lufs}`,
  `{ceiling}`) to master previews with it once it does.
- **From AI audio to notes** (`transcribe.rs`, `compose_transcribe`):
  basic-pitch 0.4.0 (Apache-2.0) installed on first use into
  `~/.cache/ccez-daw/tools/basic-pitch` with `uv` (Python 3.11; pins
  `setuptools<81` because resampy still imports `pkg_resources`), or any
  MIDI file / basic-pitch CSV. Tempo heard, beat phase and downbeat found,
  notes quantised to sixteenths, melody (top line), bass (bottom line)
  and a chord per bar pulled out, then arranged on `calm` and `lively`
  layers; the melody is kept as motif `transcribed_melody`. 40 s of an
  ACE-Step take transcribes in ~12 s on the Mac mini.
- **In the DAW** (`daw.rs`, IPC `compose_open`, palette "Composition:
  Open in the DAW", `compose_open_in_daw`): closes old open item 6. One
  audio track per layer carries the rendered stems (play sounds the real
  orchestration, the live loop being the pre-rendered mix), one muted
  MIDI track per layer and instrument carries the notes for the piano
  roll; sections are laid end to end. Note edits in the DAW stay in the
  project; `composition.json` (stored in the bundle) is still what the
  composer renders.

`mcp/scripts/island-travel-session.ts` is a whole session as a script
(motif → sequence → inversion → progression → re-harmonised B section →
ensemble → layer EQ → master → ears against a reference → audioforge
package → DAW project).

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
   per step (`layer_rms_dbfs`), then (2026-10-16) `compose_listen` and a
   spectrogram picture (above).
6. Closed (2026-10-16): a composition opens as a DAW project (timeline,
   piano roll, mixer) through `compose_open` / the palette (above).
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
9. Open: each mutation re-runs the Rust validator as a subprocess (fast
   normally, seconds on a loaded box).
10. Open: DAW note edits don't flow back into the composition (the
    composer renders `composition.json`); re-open after editing the
    document instead.
11. Open: the clash check sees held notes only; fast passing tones and
    timbral masking (two instruments in one register) are not judged.
12. Open: genforge's mastering pass is not wired by default (it had not
    landed); `CCEZ_MASTER_CMD` hooks it in.
