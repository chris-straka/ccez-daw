# Track J primer: the browser — one palette for every sound

New to DAWs? Start here. This note teaches what a DAW *browser* is, why
Track J built it as one palette over four library kinds, and how its
"semantic search" works today (deterministic local math) versus tomorrow
(a real embedding sidecar with unpinned models).

## 1. What a browser is in a DAW

A DAW accumulates thousands of things: drum **samples**, synth
**presets**, **plugin** effects, and whole **project** templates. The
browser is the panel where you find the right one without breaking flow —
you type what you want ("warm pad", "boomy kick") and audition or insert
it. Without a good browser, a huge library is just a hard drive.

Track J's design decision: **one palette indexes all four kinds.**
Samples, presets, plugins, and projects share a single `LibraryItem`
shape (`id`, `kind`, `name`, `tags`, `text`) and a single search box.
Kind tabs (`All / Sample / Preset / Plugin / Project`) filter; they never
fork the index. One ranking function, one UI, four contents.

## 2. The ideas this track is built on

**Library items are documents, not files (yet).** Each entry carries a
human-readable `name`, machine `tags`, and a one-line `text` description.
Ranking reads all three concatenated (`name + tags + text`). Tags are
what make "808" find a kick drum even when its name says "Boomy"; the
description is what makes "ambient intro" find a pad. When you add a real
entry later, write the description like you'd search for it.

**Similarity search = vectors + cosine.** Every document is converted to
a 64-number vector (*embedding*); the query becomes a vector the same
way; results sort by the angle between vectors (*cosine similarity*, 1 =
same direction, 0 = unrelated). Scores are shown next to each result so
you can see what the machine thinks "similar" means.

**Today's embedding is deliberately dumb — and that's the point.**
`embed_text` is a hashed bag of words: lowercase the text, split on
non-alphanumeric characters, hash each token with FNV-1a into one of 64
bins, count, and L2-normalize. No model, no network, no GPU: it runs
offline, it's fully deterministic, and it already answers concept queries
("cozy warm analog pad for an ambient intro" → *Warm Analog Pad* at
0.71, next best 0.17). It cannot understand true synonyms ("feline" won't
find "cat"), which is exactly why it's the *fallback*, not the ceiling.

**The model is unpinned by design.** Real neural embeddings arrive
behind the `EmbeddingProvider` interface (`ui/src/browser/embeddings.ts`):
`LocalEmbeddingProvider` (today's default, today's math) and
`SidecarEmbeddingProvider` (a future local HTTP server serving whatever
sentence-transformer you choose — the model id is a plain string, nothing
pins a version). Swapping models changes vectors, never ranking or UI
code. Anything that must stay true regardless of model lives in the
deterministic layer and its tests.

**Rust owns the truth, TypeScript mirrors it.** `core/src/library.rs`
defines `LibraryKind`, `LibraryItem`, `LibraryHit`, the embedding math,
and the seed library; `ui/src/browser/library.ts` is a line-by-line port
(`tokenize`, `fnv1a32`, `embedText` must agree bit-for-bit) and
`ui/src/browser/seed.ts` mirrors the seeds item-for-item. The two
concept-query tests (Rust + `ui/tests/browser.test.ts`) pin the same
winner, so drift between the implementations fails loudly on either side.

## 3. What Track J built (file map)

- `core/src/library.rs` — truth: kinds, item/hit shapes, embedding +
  cosine + `search`/`search_filtered`, 8 seeded entries, 5 unit tests.
- `core/src/emit.rs` (+`LibraryKind`, `LibraryItem`, `LibraryHit`,
  appended last) — regenerated `ui/src/generated/project.ts` (+21 lines,
  purely additive; frozen v0 shapes and the IPC table untouched).
- `ui/src/browser/library.ts` — TS mirror of the search math.
- `ui/src/browser/embeddings.ts` — sidecar interface + local/sidecar
  providers + `getDefaultProvider()`.
- `ui/src/browser/seed.ts` — 8 schema-validated seeds (2 per kind).
- `ui/src/browser/Browser.tsx` — the palette: search box, kind tabs,
  ranked results with scores, `onPick` hook for timeline/mixer insertion.
- `ui/src/App.tsx` — the Browser panel now renders the palette.
- `ui/tests/browser.test.ts` — 5 tests: concept query → seeded pad, kind
  filter, seed/schema validity, embedding math, empty-query order.

## 4. How to verify any of this

- `cargo test --manifest-path core/Cargo.toml library` — Rust search tests.
- `cd ui && bun test tests/browser.test.ts` — 5/5 pass.
- `bun run typegen -- --check` — drift gate: `ok project.ts`, `ok ipc.ts`.
- `cd ui && bun run check` — `tsc --noEmit` clean.
- Type "warm ambient pad" in the Browser panel — *Warm Analog Pad* tops.

## 5. Known limits (for the next track)

- Seeds are hardcoded; real file scanning / plugin hosting comes later.
- `onPick` only highlights — insertion into timeline/mixer is unwired.
- No new IPC commands: search is local-first, so the frozen v0 table was
  left alone. A `library_search` command becomes worth it only when the
  index outgrows the frontend.
- Note: as of this writing, two failures come from parallel tracks'
  in-progress work, unrelated to Track J: `ui/tests/contracts.test.ts`
  fails on registry additions (`palette.open`, `vim.*` local-only actions
  with no IPC mapping), and `tsc` reports a filename-casing collision
  between `ui/src/palette/Palette.tsx` and `ui/src/palette/palette.ts`.
  Track J's own gates are green: drift gate, `cargo test library`, and
  `bun test tests/browser.test.ts` (5/5).
