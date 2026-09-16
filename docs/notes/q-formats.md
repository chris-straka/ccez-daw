# Q-formats primer: AAX, LV2, and ARA — what we ship, flag, and defer (Agent 8)

New to plugin-format work? Start here. This note teaches the one idea
behind each of the three formats outside the current CLAP/VST3/AU
coverage, evaluates what hosting it would cost this repo, and records a
verdict per format: **ship** (build now), **flag** (feasible, blocked on a
named condition), or **defer** (do not build; revisit only when the named
blocker clears). There is no heavy implementation in this track: the only
code is the ARA host seam in [`ara.rs`](../../core/src/plugins/ara.rs)
(unimplemented hooks, like the other phase-2 seams), and this doc.

Out of scope (do not build here): surround/Atmos, hardware surfaces,
cloud/collab.

Research method note: the Context7/DeepWiki MCP tools named in the brief
were not granted to this agent, so the evaluation below rests on the
primary sources themselves, fetched directly: Avid's
[AAX SDK page](https://developer.avid.com/aax/), the
[`livi-rs` host library](https://github.com/wmedrano/livi-rs) plus its
[docs.rs dependency listing](https://docs.rs/livi/0.7.5/settings.html),
and Celemony's [`ARA_SDK`](https://github.com/Celemony/ARA_SDK) /
[`ara_api`](https://github.com/celemony/ara_api) repositories.

## 1. AAX — the Pro Tools format lives behind Avid's door: DEFER

A plugin format is two things: a **binary contract** (how the host finds
and talks to the plugin) and a **license gate** (what you must sign to use
the SDK). AAX's binary contract is ordinary — a C++ SDK with plugin
classes much like VST3's factory model. Its license gate is the whole
story, and Avid states it plainly on the
[AAX SDK page](https://developer.avid.com/aax/):

- The SDK downloads only after a **click-through license agreement**, via
  an **Avid account** — it cannot be vendored, fetched by CI, or
  redistributed. Nobody without that account can even build the code.
- Running and testing needs an **iLok account** (Pro Tools itself), and
  commercial AAX products need an **iLok USB key** as part of PACE digital
  signing — unsigned AAX plugins do not load in Pro Tools.
- Commercialization goes through `audiosdk@avid.com` for "the necessary
  tools and license": a human relationship, not a `cargo add`.

So the blocker is not engineering effort, it is access: no SDK in the
repo, no SDK in CI, no way to test, and a signing step we cannot perform.
(A 2024 ADC talk notes the SDK is dual-licensed GPL3, but that changes
none of the above — signing is still mandatory.) **Verdict: DEFER.**
Revisit only when the project holds an Avid developer account *and* a
signing path; until then no code, no dependency, no directory — a seam
would be a promise we cannot test.

## 2. LV2 — the open Linux-first format, hostable from Rust: FLAG

LV2 is the community-open plugin standard (RDF/Turtle metadata +
shared-object DSP, dominant on Linux). Unlike AAX there is no license
gate at all — the question is purely which Rust crate hosts it, and the
answer is [`livi`](https://github.com/wmedrano/livi-rs) ("An LV2 host
library for Rust"), layered on
[`lilv-rs`](https://github.com/wmedrano/lilv-rs) (safe bindings to the C
[lilv](http://drobilla.net/software/lilv) host library, targeting Lilv
0.24.2). Two cautions from the same sources:

- **System dependency.** `livi 0.7.5` depends on `lilv ^0.2`,
  `lv2-sys ^2`, `lv2_raw ^0.2`: hosting LV2 links the C `lilv` library,
  so every build machine needs `liblilv-dev`/`lv2-dev` (a `pkg-config`
  failure, not a Rust error, when missing). That breaks today's
  dependency-free `core/` build story the CLAP track deliberately kept
  (offline-friendly, no supply-chain surface).
- **Maturity.** `livi` describes itself as work-in-progress, "not yet
  fully tested"; the sibling `lv2` crate family (`lv2-core`, …) is for
  *authoring* plugins, not hosting them — do not confuse the two.

Neither caution is fatal: LV2 matters most on Linux where `lilv` is one
`apt install` away, and the sandbox story from Track C transfers directly
(untrusted C DSP already lives in the worker process). **Verdict: FLAG**
(feasible, Linux-first). Ship condition, in order: (1) a Linux CI image
with `liblilv-dev`/`lv2-dev` pinned, (2) `livi` behind an optional cargo
feature so default builds stay dependency-free, (3) the adapter living in
the worker process behind the existing
[`SandboxedPlugin`](../../core/src/plugins/sandbox.rs) protocol, exactly
as the `vst3.rs` module doc prescribes for its own phase-2 adapter.

## 3. ARA — random access for editing plugins, open SDK: SHIP (as a seam now, host later)

VST3/CLAP/AU stream audio *through* the plugin buffer-by-buffer; the
plugin never sees the whole file. ARA (Celemony + PreSonus, now at spec
2.0) flips that for editing-style plugins (Melodyne-style pitch/time):
the **host owns a document model** — audio sources, regions on a musical
timeline, tempo/key context — and the plugin gets **random access** to the
audio, analyzes whole phrases, and follows later timeline edits. The
plugin then renders its edited audio back through **playback rendering**
roles. That is the whole mental model: *document in, edits out*.

The license gate is open, which is why this is the one format of the
three to scope: since May 2021 Celemony publishes the full
[ARA SDK](https://github.com/Celemony/ARA_SDK) under **Apache 2.0**
(spec in [`ara_api`](https://github.com/celemony/ara_api),
buildable library code plus a JUCE integration example). No account, no
signing, no fee — the cost is engineering, and it is large but separable:

1. **Document model** (host side): audio sources + regions + musical
   context built from the timeline. The seam's [`AraRegion` and
   `AraAudioSourceId`](../../core/src/plugins/ara.rs) name the first two;
   tempo/key following is the follow-up.
2. **Plugin discovery/bridging**: ARA plugins ship *inside* VST3/AU
   binaries, so ARA hosting reuses the VST3/AU loaders, then negotiates
   the ARA extension — it composes with existing tracks instead of
   replacing them.
3. **Playback renderer + edit loop**: drive the plugin's renderer over a
   region range and invalidate on timeline edits (the hardest part; needs
   the engine's render path, not just the plugin registry).

**Verdict: SHIP as phase-2 design.** Step 0 (this track's original seam:
[`AraHost`](../../core/src/plugins/ara.rs) constructs, reports
`is_available() == false`, and refuses `share_source` / `render_region` /
`detach_clip` with
[`AraError::AraUnimplemented`](../../core/src/plugins/ara.rs)) has since
landed as a live document model — the follow-up arrived in `core/` only,
`ui/` untouched:

1. **Document model** (live): [`AraDocument`](../../core/src/plugins/ara.rs)
   — [`MusicalContext`](../../core/src/plugins/ara.rs) (tempo/key/meter,
   beats↔seconds mapping, `from_project`), [`AraAudioSource`](../../core/src/plugins/ara.rs)
   with random sample access (`sample_at` / `read_range`), and region
   sequences mapped from arrangement clips (`from_project` /
   `add_region_for_clip`, invalid spans skipped by the same
   [`AraRegion::new`](../../core/src/plugins/ara.rs) rule as before).
   [`AraHost`](../../core/src/plugins/ara.rs) owns the document:
   `is_available()` is `true`, `share_source` registers clip audio,
   `render_region` slices a clip's region through a mock effect, and
   `detach_clip` is idempotent.
2. **Offline bounce wiring** (live): [`ara_document_for_track`,
   `render_track_with_ara`,
   `render_mix_with_ara`](../../core/src/bounce.rs) — a bound effect
   transforms exactly its clip's window span before the stem sums; no
   binding (or an effect with no region in the document) renders
   bit-identical to the plain path, and the mix honors mute/solo.
3. **Mock effect + round-trip proof** (live, tested):
   [`MockAraEffect`](../../core/src/plugins/ara.rs) — zero-crossing
   analysis into editable [`notes`](../../core/src/plugins/ara.rs),
   per-note/region gain plus a length-preserving naive transpose.
   Hand-rolled on purpose (~40 lines, zero new cargo deps): an ARA SDK
   crate would drag C++ bindings and license weight for test-only value;
   a real pitch engine arrives with real binary hosting.
4. **Real third-party binaries** (follow-up, no code): exactly two pieces
   are missing — (a) **factory entry hosting**: ARA plug-ins ship inside
   VST3/AU binaries, so the host must load the binary through the existing
   VST3/AU worker backends and perform the ARA factory handshake
   (`ARAFactory::createDocumentController` negotiation over the plug-in
   extension); (b) **document controller lifecycle**: create/bind/destroy
   the `ARADocumentController`, drive its analysis + playback-renderer
   roles, and invalidate on timeline edits. [`AraHost::load_binary_plugin`](../../core/src/plugins/ara.rs)
   names this seam and refuses with it.

## Why no contract changes

All three evaluations add zero frozen surface: no `contracts/*` edits, no
`core/src/model.rs` or `ipc.rs` changes, no new cargo dependencies (AAX
needs an account, LV2 needs a system lib, ARA needs engine work — none of
them is a line in `Cargo.toml` today). The ARA seam types are
addressing scaffolding like [`Vst3Descriptor`](../../core/src/plugins/vst3.rs):
deliberately unregistered from `emit.rs`, never written to project files.

## Validation

```sh
cargo test -p ccez-core plugins::ara   # 8 tests: region, context, sources, mapping, host, binary seam, round-trip, transpose
cargo test -p ccez-core bounce         # 13 tests: 10 pre-existing + 3 ARA bounce wiring
cargo test --workspace                 # full workspace stays green (this track's gate)
bun run check                          # typegen drift gate unaffected (no contract surface touched)
```
