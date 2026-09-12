# r-gaps primer: evidence gaps closed, polish swept, game-import runbook (P-3)

Track P-3 of the v3 plan (`.agents/plans/2026-09-12-v3.md`): close the
Track C evidence gaps as verdicts, sweep the menu/shortcut/e2e polish gaps
(fix the trivial, ticket the rest), and — stopping before the game itself,
which is out of reach — hand the user the exact runbook for importing one
GA-4 export package into their game and hearing state-driven layers.

STOP line, stated up front: the user's game was never touched. No game
import was run. Section 4 is the runbook for the user to run; section 5 is
the report-back template that closes v2 Open Questions 1–2 with real names.

## 1. Track C gaps: verdicts (all three closed)

### 1a. AU hosting-crate choice — hand-rolled FFI (verdict, already in `track-c.md` §6)

The evaluation table in `docs/notes/track-c.md` (§"AU hosting-crate
evaluation") is the closed verdict: `rack` 0.4.8 too heavy (native build
layer, tiny adopter base), `audiounit` 0.3.1 adds a macOS-only dep plus a
Swift-bridge toolchain risk, `audio-unit` does not exist, `audiotoolbox`
adds nothing over direct FFI. v1 ships three `extern` fns against the
always-present `AudioToolbox` framework, zero new deps, behind the
`CCEZ_ENABLE_AU=1` flag (`DisabledByFlag` otherwise). Revisit `audiounit`
only when parameter/render bridging lands (phase 2, on the `AuInstance`
handle). No code changed by P-3; the gap is closed by recording the table,
which was already there — P-3 confirms it and moves on.

### 1b. nih-plug fork choice — neither: nih-plug is for *writing* plugins (verdict, already in `track-c.md` §7a)

`track-c.md` §7a records the finding that changed the dependency plan:
`robbert-vdh/nih-plug` is in maintenance mode and the community fork is
`BillyDM/nih-plug` (Codeberg) — but nih-plug is a plugin-*authoring*
framework and cannot host plugins in a DAW. Hosting CLAP in Rust is
`prokopyl/clack` (`clack-host`), and v1 deliberately adds **zero new cargo
dependencies**: the sandbox + snapshot + recovery machinery is proven with
a mock backend, and loading a `PluginKind::Clap` descriptor returns the
honest seam `HostError::ClapUnimplemented`. The future `ClapBackend`
(clack-host + libloading) runs *inside the worker process* behind the
existing worker protocol, so host/recovery code does not change. Closed as
a verdict; no code changed by P-3.

### 1c. Solid dev/build proof — `vite build` green, recorded here (closed with fresh evidence)

The claim "Solid works in Tauri dev/build" previously rested on tests and
`tsc`. P-3 ran the actual production build this session:

```sh
cd ui && bun run build
# ✓ 116 modules transformed.
# dist/index.html                  0.32 kB │ gzip:  0.23 kB
# dist/assets/index-Dj6j2Sgs.js  117.99 kB │ gzip: 36.29 kB
# ✓ built in 416ms
```

Stack pinned in `ui/package.json`: `solid-js ^1.9` + `vite-plugin-solid
^2.11`; the shell (`ui/src/App.tsx`) imports `solid-js` directly
(`For, Show, createSignal, onCleanup, onMount`). A green `vite build` is
the whole proof: Solid JSX compiles, bundles, and emits a servable `dist/`
— the same `dist/` the Tauri shell wraps and the e2e `webServer` block
serves. Dev-cycle proof comes free with it: `bun run test:e2e` boots that
same Vite server on `127.0.0.1:1420` and drives the real Solid components
(5/5 green this session — see §2). Closed; no code changed.

## 2. Menu / shortcut / e2e polish sweep

Full gate re-run this session (repo root, clean tree):

| Gate | Observed result |
|---|---|
| `bun run test` (ui) | 138 pass, 0 fail |
| `bun run test` (mcp) | 18 pass, 0 fail |
| `bun run test` (core lib + integration binaries) | 313 passed, 0 failed (plus 5/5/5 integration binaries, doc-tests 0) |
| `cargo typegen -- --check` | `ok project.ts`, `ok ipc.ts` (no drift) |
| `tsc --noEmit` (ui) | clean |
| `vite build` (ui) | green (see §1c) |
| `bun run test:e2e` (chromium) | 5 passed (shell ×2, palette, pianoroll, gameaudio) |

### Fixed (trivial, landed by P-3)

- `docs/notes/n-e2e.md` said "Four smoke specs"; `docs/notes/n-integration.md`
  said "5 specs". Truth is 4 spec files / 5 tests (`shell.spec.ts` carries
  two). Both lines corrected. No behavior touched.

### Ticketed (not fixed — owners assigned)

1. **Native menu click → event round-trip never runtime-verified**
   (`n-menu.md` admits it: needs `tauri dev` with a display). The 11 Rust
   menu unit tests + accelerator-vs-muda-source check are the current
   proof. Owner: P-1 release track (fresh-machine install smoke) or the
   user on first `bun run tauri:dev` — click each top-level menu, confirm
   the `menu-action` event hits the registry (palette runs the same id).
2. **No bare-key transport accelerators by design** (`Space`/`s` only work
   when the Solid shell has focus; native menu accelerators are all
   `CmdOrCtrl`-based). If users report "Space does nothing while typing,"
   that is the documented tradeoff, not a bug. Owner: Track K follow-up
   (focus-scope indicator in the shell) if it annoys.
3. **No chord timeout** (a half-typed `g` waits forever until the next key;
   `Esc` clears). Deliberate per `n-shortcuts.md`. Owner: Track K — add a
   timeout only with a visible "pending chord" indicator, else it becomes a
   mystery.
4. **VST3 binary instantiation + AU render/param bridging + CLAP real
   backend** remain phase-2 seams (`NoAudioEffect`, `AuInstance` handle,
   `ClapUnimplemented`). Owners: whoever schedules plugin-hosting phase 2;
   each seam already names its landing site in `track-c.md`.
5. **Hang detection**: a live-but-mute plugin worker blocks the caller
   (deadline + kill is phase-2). Owner: phase-2 sandbox work.

## 3. What the game import needs from the DAW side (already shippable)

- Authoring: cue editor + bank + audition simulator in the UI
  (`ui/src/gameaudio/`: `CueEditor.tsx`, `Audition.tsx`, `export.ts`
  preview/profiler). Audition posts `GameStateSnapshot`s and reports the
  audible layer/event ids — the in-DAW rehearsal of exactly what §4 does
  in-game.
- Export: `core/src/gaexport.rs` (`build_package` → `write_package`) emits
  the directory; `core/src/bin/export-validator.rs` /
  `scripts/validate-export.sh` approves it. MCP seam
  (`gameaudio_audition`, `export_bank`, … in `mcp/src/gameaudio.ts`) mirrors
  the same shapes for agents.
- Loader contract: `docs/notes/ga-export-loader.md` (normative for
  `validator_version: "1"`); frozen shapes in
  `contracts/export-package.md`. Renders are 48 kHz / 24-bit WAV
  (`EXPORT_SAMPLE_RATE_HZ` in `ui/src/gameaudio/export.ts` derives every
  size estimate from that, not a constant).

## 4. Runbook: import one export package into your game (user runs this)

Goal: hear DAW-authored layers switch on *real* game state. Time: ~30 min
plus engine integration. Default answers (until you report back in §5):
raw WAV + JSON, demo states `explore`/`combat` + param `threat`.

### Step 1 — Author and rehearse in the DAW (5 min)

1. Open the game-audio panel, author (or keep) one cue with two layers
   (e.g. `bed` always-on + `drums` on `combat`) and one bank with one event
   (e.g. `player.footstep`).
2. In the Audition simulator, post `explore`, then `combat`: confirm the
   readout flips from the bed stack to bed+drums and the transition log
   shows the expected switch (Cut or Fade). Trigger the SFX event and
   confirm it sounds.
3. Note your cue's **tempo** (per-cue BPM — the engine converts loop beats
   at the *cue* tempo, not the project tempo).

### Step 2 — Export the package (2 min)

Export cue(s) + bank(s) to a directory, e.g. `out/demo-v1/`. Expected
manifest (`out/demo-v1/package.json`):

```text
out/demo-v1/
  package.json            # ExportPackage: schema_version 1, name, cue_ids,
                          # bank_ids, stems[], event_bank_path, validator_version "1"
  bank_<id>.json          # one SfxBank per bank_ids entry, verbatim bytes
  stems/<cue>_<layer>.wav # MusicLayer loops, e.g. stems/cue_fight_bed.wav
  sfx/<event>_<n>.wav     # SfxClip one-shots, <n> = index into clip_ids
```

Music stems loop `0..<beats>` at the cue tempo; SFX stems are `0, 0`
one-shots. (`contracts/export-package.md` §Layout.)

### Step 3 — Validate: package must go green (2 min)

```sh
scripts/validate-export.sh out/demo-v1 context.json
# expect: export-validator: out/demo-v1 approved (validator version 1)
```

`context.json` bundles the authoring side:
`{ "project": …, "cues": […], "banks": […], "params": […] }`.
Exit 1 = rejected: fix the listed rule in the DAW and re-export — never
hand-edit the directory (rule 5 determinism checks re-render under the
export seed; `DEFAULT_EXPORT_SEED` unless you passed one explicitly).
(Rules: `contracts/export-package.md` §Validator rules.)

### Step 4 — Copy into your game project (1 min)

Copy the whole directory as one unit, e.g. `Assets/Audio/demo-v1/`. It is
data — check it in or stream it exactly like any other asset bundle.
Packages travel through pipelines that corrupt things, so the engine
re-checks rules 1–3 at load (see loader spec).

### Step 5 — Integrate the loader (the real work; spec in `docs/notes/ga-export-loader.md`)

Implement once per engine, in load order (fail loud at load, never at
playback):

1. Parse `package.json`; reject when `schema_version != 1` or
   `validator_version != "1"`.
2. Load every `bank_<id>.json`; reject on malformed JSON, id/filename
   mismatch, empty `clip_ids`, duplicate event ids.
3. Load every stem as PCM mono WAV; reject on missing/unparseable files.
4. Precompute per-stem loop samples once, at the **cue** tempo:
   `loop_start = floor(start_beats * 60 / tempo * sample_rate)`,
   `loop_end = round(end_beats * 60 / tempo * sample_rate)`; SFX (`0, 0`)
   plays once, never loops.
5. Per frame: `layers_for_state(snapshot.state)` selects audible music
   stems (empty `states` = always-on bed); `transition_for(from, to)`
   selects Cut/Fade (`fade_beats` at cue tempo). SFX: uniform pool pick
   over `clip_ids`, per-trigger `volume_random`/`pitch_random` from a
   seeded RNG, `cooldown_ms`, `max_polyphony` oldest-steal; RTPC bindings
   clamp → normalize → map (unknown params ignored, missing = default).
   A stem lost *after* load renders silence; unknown event ids drop.

Middleware notes (v2 Open Question 2 defaults to raw first):

- **Raw (default, any engine):** the five steps above *are* the
  integration — ~100 lines plus your mixer voices. Start here.
- **FMOD:** import stems into an audio table / programmer sounds; drive
  layer gains from `layers_for_state` output each state change; SFX events
  map 1:1 to FMOD events with the pool pick done game-side. Loop points:
  set `[loop_start, loop_end)` samples from step 4.
- **Wwise:** stems as External Sources (or streamed SFX objects); music
  layers as Switch-driven mixer gains; loop regions from step 4 via
  `SetLoop` / segment loop overrides. RTPC bindings map onto Wwise RTPCs
  with the same clamp/normalize/map math.

### Step 6 — Hear it (5 min)

1. Boot to the state mapped to the bed only; confirm the bed loops
   seamlessly (no click at the wrap = loop math right).
2. Drive the game into the layered state; confirm the second stem fades or
   cuts in per the cue's transition.
3. Trigger the SFX event in-game; confirm one-shot playback with
   humanization variance across repeats.
4. Kill a stem file *after* load (temp rename) only if you want to verify
   the silence-not-crash policy — then restore it.

### Step 7 — Report back (§5 template)

Paste the §5 block into chat. That reply closes v2 Open Questions 1–2 with
evidence and tells the DAW side what to add (real state names land
additively — never by editing demo content in place).

## 5. Report-back template (paste this filled in)

```text
Engine + version:
Audio middleware (or "raw"):
Package name + validator output (paste the "approved" line):
Cue tempo(s):
REAL STATES (names exactly as the game posts them):
REAL PARAMS (name, min, max, default each):
Layer mapping heard (state -> stems, e.g. explore -> bed; combat -> bed+drums):
Transition heard (Cut / Fade + fade_beats):
SFX triggers tested (event id -> result):
Loop seam clean? (yes/no; if no, stem + where the click is):
Mismatch vs audition simulator? (anything the game did that Audition didn't):
Next package wants (more states? more layers? real middleware decode?):
```

Until this comes back, the recorded defaults stand: raw WAV + JSON,
`explore`/`combat` + `threat` (v2 plan Open Questions 1–2).

## 6. Validation summary for P-3

- `bun run test` green (ui 138/0, mcp 18/0, core 313 + integration
  binaries, 0 failures), `bun run test:e2e` 5/5, `vite build` green,
  `typegen --check` ok ×2, `tsc --noEmit` clean — all observed this
  session on a clean tree.
- Additive-only: two one-line doc corrections (§2); no contract, schema,
  generated-type, or behavior change. `ui/src/generated/*` untouched.
- Deliverable: this primer (`docs/notes/r-gaps.md`) — verdicts (§1),
  polish sweep with owners (§2), runbook (§4), report-back (§5).
