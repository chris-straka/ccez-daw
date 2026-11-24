# ccez-daw

A fully featured desktop DAW as a Tauri 2 app (Rust backend + SolidJS/TypeScript
frontend), with native plugin support (CLAP + VST3 + AU), an MCP server,
vim-style keybindings, and AI-assisted editing.

The architecture index lives in [`docs/`](./docs/README.md) (active technical
primers in [`docs/notes/`](./docs/notes) and historical milestone/prototype
archive in [`docs/archive/`](./docs/archive)). The initial design plan is in
`.agents/plans/2026-09-12-tauri-daw.md`, and frozen contracts live in
[`contracts/`](./contracts/README.md). Post-v0 changes are additive: two op kinds
(`AutomationPointSet` and `GameAudioExported`,
[`contracts/op-log-format.md`](./contracts/op-log-format.md)), additive IPC
commands ([`contracts/ipc-table.md`](./contracts/ipc-table.md)), and additive
MCP/game-music contracts ([`contracts/mcp-tools.md`](./contracts/mcp-tools.md),
[`contracts/audioforge-music.md`](./contracts/audioforge-music.md)).

## What it does

- **Play & record** — realtime cpal transport (null-device fallback for
  headless/CI), audio input capture, punch in/out, count-in, comp takes,
  groove pool.
- **Write & arrange** — timeline + Session View clip launcher with jam
  recording, piano roll, score notation, automation lanes, branch
  compare/merge, all event-sourced through the op log (infinite undo).
- **Sound** — native sampler, drum rack, filter/delay/distortion kernels,
  arpeggiator, chord generator, humanizer, 59 curated presets, WASM guest
  devices (sample-identical to native kernels), Link-style tempo sync.
- **Mix & finish** — mixer with snapshots and reference tracks,
  loudness-normalized bounce with platform presets, metering bridge.
- **Game music** — compose via MCP `compose_*` tools or `ccez-compose`,
  render seamless loop stems with intro/loop/outro sections and intensity
  1-5 layers, and export an `audioforge-music` package
  ([`docs/notes/ga-compose.md`](./docs/notes/ga-compose.md),
  [`contracts/audioforge-music.md`](./contracts/audioforge-music.md)).
- **Hosts everything** — sandboxed CLAP/VST3/AU loading with latency
  compensation, plugin scan lists, crash recovery with watchdog, ARA
  document model wired to bounce.
- **Shell** — command palette, vim keybindings, shortcut editor, one action
  registry driving palette/vim/scripting/menu (MCP tools mirror a subset of
  it), dark studio theme.

## Layout

- `src-tauri/` — Tauri 2 app shell (Rust). Thin: invokes into `core`.
- `core/` — Rust crate (`ccez-core`) owning project/engine truth, DSP, plugin
  hosting, compose rendering, and binaries (`ccez-compose`, `plugin-worker`,
  `typegen`).
- `ui/` — SolidJS + TypeScript (strict) + Vite + Bun frontend. Never hand-writes
  contract types; imports them from `ui/src/generated/`.
- `mcp/` — Bun/TS MCP server on the official MCP TS SDK (28 tools: 14
  registry-mirrored DAW tools + 14 `compose_*` game-music tools). Not every
  registry action has an MCP tool; palette/vim/menu-only actions are documented
  in [`contracts/mcp-tools.md`](./contracts/mcp-tools.md).
- `docs/` — [Documentation index](./docs/README.md) splitting active primers
  from archived plans and removed prototypes.
- `contracts/` — Contract specifications (`ipc-table`, `project-schema`,
  `op-log-format`, `action-registry`, `mcp-tools`, `audioforge-music`, etc.).
- `examples/` — Sample composition fixtures and preview assets.

## Prerequisites

- Bun >= 1.1, Rust stable, Tauri 2 system deps
  ([prerequisites](https://v2.tauri.app/start/prerequisites/)).
- Linux (Debian/Ubuntu) also needs ALSA headers for the audio engine; the
  full list CI installs is:

  ```sh
  sudo apt-get install -y libwebkit2gtk-4.1-dev build-essential curl wget file \
    libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev libasound2-dev
  ```
- Optional: `ffmpeg` on `PATH` for video thumbnails (without it, filmstrip
  cells render as placeholders).

## Commands

```sh
bun install            # install ui + mcp workspaces
bun run typegen        # regenerate TS bindings from Rust (core is source of truth)
bun run check          # typegen drift gate + tsc + core tests (FAILS on contract drift)
bun run test           # all TS + Rust tests
cargo fmt --all --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test -p ccez-daw --lib # native shell command regression tests
bun run test:e2e       # Playwright web-mode specs (first: cd ui && bunx playwright install chromium)
bun run tauri dev      # launch the DAW shell (starts Vite + Tauri; tauri:dev also works)
cargo test --manifest-path core/Cargo.toml   # Rust core tests
```

`bun run check` regenerates the bindings from `core/` and diffs them against the
committed files: if Rust types and TS bindings drift, the build fails.
