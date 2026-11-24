# ccez-daw Documentation

This index separates **active technical documentation** (architecture primers, UI/input workflows, audio/video subsystems, and game-music composition) from **archived milestone plans and removed prototypes** under [`archive/`](./archive).

## Active Documentation

### Core Architecture & DAW Tracks

| Topic | Document | Notes |
|---|---|---|
| Scaffold, typegen & release pipeline | [track-0.md](./notes/track-0.md) | Workspace wiring, Rust-to-TS contract emission, workflows |
| Project model & op-log engine | [track-a.md](./notes/track-a.md) | Event-sourced `ops.jsonl`, snapshots, assets, portable bundles |
| Audio graph & offline freeze/bounce | [track-b.md](./notes/track-b.md) | Lock-free graph swap, cpal/null output, `smpl` WAV stems |
| Plugin hosting (CLAP, VST3, AU, WASM, ARA) | [track-c.md](./notes/track-c.md) | Out-of-process worker sandbox, latency compensation, crash recovery |
| MIDI, piano roll & expressive tools | [track-d.md](./notes/track-d.md) | Note model, MPE, humanizer, arpeggiator, chord generator |
| Timeline, Session View & comping | [track-e.md](./notes/track-e.md) | Clips, sections, launcher quantization, overlap & take comping |
| Warp, pitch, groove & time-stretch | [track-f.md](./notes/track-f.md) | Audio warping, transient alignment, groove templates |
| Mixer, snapshots & reference tracks | [track-g.md](./notes/track-g.md) | Buses, sends, automation lanes, AB reference routing |
| Devices, racks & native presets | [track-h.md](./notes/track-h.md) | 59 curated presets, sampler, drum rack, filter/delay/distortion |
| Branches, diff & project merge | [track-i.md](./notes/track-i.md) | Op-log branch creation, structural diff, merge resolution |
| Semantic browser & search | [track-j.md](./notes/track-j.md) | Deterministic FNV/cosine embeddings mirrored in Rust and TS |
| Command palette, vim & actions | [track-k.md](./notes/track-k.md) | Unified action registry, vim motions, keybindings, scripting |
| MCP server & natural-language ops | [track-l.md](./notes/track-l.md) | Official TS MCP server + sidecar-pluggable NL command compiler |
| AI editing sidecars | [track-m.md](./notes/track-m.md) | Pluggable transcription, separation, and audio cleanup seams |

### Runtime Performance, Sync & Compute Extensions

- [rt-perf.md](./notes/rt-perf.md) — Real-time audio thread benchmarks and callback latency budgets
- [link-sync.md](./notes/link-sync.md) — Tempo/phase sync (`LinkBus`, optional `link-net` UDP session)
- [p-wasm.md](./notes/p-wasm.md) — WASM DSP guest runtime (`wasmtime` + devices-side deterministic sandbox)
- [p-gpu.md](./notes/p-gpu.md) — Optional `wgpu` spectral/convolution compute offload (`gpu` feature)
- [p-distributed.md](./notes/p-distributed.md) — Remote DSP node scheduler & failover seam (`remotenodes`)

### Desktop UI, Recording, Notation, Metering & Video

- [n-menu.md](./notes/n-menu.md) — Native OS menus wired to the frontend action registry
- [n-shortcuts.md](./notes/n-shortcuts.md) — Keyboard shortcut editor and focus scoping rules
- [n-e2e.md](./notes/n-e2e.md) — Playwright web-mode Tauri mock and E2E spec suite (`ui/e2e/`)
- [q-formats.md](./notes/q-formats.md) — Audio file import/export codecs and sample-rate conversion
- [q-metering.md](./notes/q-metering.md) — BS.1770 integrated LUFS, 4x true-peak, and FFT spectrum taps
- [q-notation-core.md](./notes/q-notation-core.md) — Core score/staff model, beaming, clefs, and MusicXML export
- [q-notation.md](./notes/q-notation.md) — Interactive SVG notation editor and note-entry workflow
- [q-record.md](./notes/q-record.md) — Count-in, punch-in/out, multi-take lanes, and comp commit
- [q-video.md](./notes/q-video.md) — SMPTE timecode, beat-to-video sync, drift monitor, ffmpeg thumbnails
- [r-models.md](./notes/r-models.md) — Optional AI sidecar model cache and unpinned provider policy

### Game Music Composition & Export

- [ga-compose.md](./notes/ga-compose.md) — Agent-driven `ccez-compose` CLI and `compose_*` MCP workflow
- [sample-libraries.md](./notes/sample-libraries.md) — On-demand CC0/MIT orchestral sample fetch & SHA-256 verification
- [ga-0.md](./notes/ga-0.md) — Shared game-audio state/cue/event schema types (`core/src/game_audio.rs`)
- [ga-music.md](./notes/ga-music.md) — Vertical layering & horizontal resequencing adaptive cue planner (`core/src/adaptive/`)
- [s-conform.md](./notes/s-conform.md) — Platform loudness conformance presets (PC, consoles, mobile) in `core/src/meter/conform.rs`

### Contracts & Examples

- [contracts/README.md](../contracts/README.md) — Frozen schema, op-log, IPC, MCP, and `audioforge-music` export contracts
- [examples/README.md](../examples/README.md) — Sample composition fixtures and preview workflow

---

## Archived Documentation (`docs/archive/`)

Historical milestone plans, release/integration logs, and pre-cleanup notes whose prototype modules (`gaexport`, `batchexport`, `sfx`, `godot-template`, `ga-fixture`) were removed from the repository in `e31557f` and `9d046be`. Retained for historical context; do not treat as active API specs.

- **Removed Game-Audio Export & Godot Prototype Notes:**
  - [ga-audition.md](./archive/ga-audition.md)
  - [ga-export.md](./archive/ga-export.md)
  - [ga-export-loader.md](./archive/ga-export-loader.md)
  - [ga-mcp.md](./archive/ga-mcp.md)
  - [ga-sfx.md](./archive/ga-sfx.md)
  - [s-batch.md](./archive/s-batch.md)
  - [s-fixture.md](./archive/s-fixture.md)
  - [s-godot.md](./archive/s-godot.md)
  - [s-loop.md](./archive/s-loop.md) *(Note: `core/src/loopseam.rs` itself remains active for bounce and compose loop-seam verification.)*
- **Milestone Plans & Session Close-out Logs:**
  - [track0.md](./archive/track0.md) *(superseded by [track-0.md](./notes/track-0.md))*
  - [n-integration.md](./archive/n-integration.md)
  - [p-v3plan.md](./archive/p-v3plan.md)
  - [r-gaps.md](./archive/r-gaps.md)
  - [r-release.md](./archive/r-release.md)

Design-stage execution plans also live under `.agents/plans/` as historical records.
