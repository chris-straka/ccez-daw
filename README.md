# ccez-daw

A fully featured desktop DAW as a Tauri 2 app (Rust backend + SolidJS/TypeScript
frontend), with native plugin support (CLAP + VST3 + AU), an MCP server,
vim-style keybindings, and AI-assisted editing.

The approved plan lives at
`.agents/plans/2026-09-12-tauri-daw.md`. Frozen v0 contracts live in
[`contracts/`](./contracts/README.md) (one additive extension since the
freeze: the `AutomationPointSet` op — see `contracts/op-log-format.md`).

## What it does

- **Play & record** — realtime cpal transport (null-device fallback for
  headless/CI), audio input capture, punch in/out, count-in, comp takes,
  groove pool.
- **Write & arrange** — timeline + Session View clip launcher with jam
  recording, piano roll, score notation, automation lanes, branch
  compare/merge, all event-sourced through the op log (infinite undo).
- **Sound** — native sampler, drum rack, filter/delay/distortion kernels,
  arpeggiator, chord generator, humanizer, 58 curated presets, WASM guest
  devices (sample-identical to native kernels), Link-style tempo sync.
- **Mix & finish** — mixer with snapshots and reference tracks,
  loudness-normalized bounce, batch + game-audio export, metering bridge.
- **Hosts everything** — sandboxed CLAP/VST3/AU loading with latency
  compensation, plugin scan lists, crash recovery with watchdog, ARA
  document model wired to bounce.
- **Shell** — command palette, vim keybindings, shortcut editor, one action
  registry driving palette/vim/scripting/menu/MCP alike, dark studio theme.

## Layout

- `src-tauri/` — Tauri 2 app shell (Rust). Thin: invokes into `core`.
- `core/` — Rust crate owning project/engine truth (project model, op log, IPC
  shapes) plus the Rust→TS typegen step.
- `ui/` — SolidJS + TypeScript (strict) + Vite + Bun frontend. Never hand-writes
  contract types; imports them from `ui/src/generated/`.
- `mcp/` — Bun/TS MCP server on the official MCP TS SDK. Tools mirror the
  action registry (every feature since v0 has palette/vim/menu/MCP parity).
- `contracts/` — frozen v0 human-readable contracts (IPC table, op-log format,
  action registry, MCP tool list).

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
bun run test:e2e       # Playwright web-mode specs (first: cd ui && bunx playwright install chromium)
bun run tauri:dev      # launch the DAW shell (proves Solid-in-Tauri cycle)
cargo test --manifest-path core/Cargo.toml   # Rust core tests
```

`bun run check` regenerates the bindings from `core/` and diffs them against the
committed files: if Rust types and TS bindings drift, the build fails.
