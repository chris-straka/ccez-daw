# p-v3plan primer: what v3 is and why

New to the project? Start here. v1 built the DAW, v2 taught it game audio.
**v3 ships it**: cut a real test release, pin the AI models v1 deferred, and
close the questions v1/v2 left open. No new features — v3 is the
release-and-close-out milestone.

## 1. The idea

A green build is not a shipped build. Right now everything passes
(`bun run check`, unit suites, Playwright e2e) but three things are still
true:

- **No release has ever been cut.** The pipeline (`.github/workflows/release.yml`,
  strict-sequential per-arch builds copied from the sibling `ccezstudio`
  after it proved parallel uploads silently lose files) exists since Track 0
  — yet `git tag` is empty, the updater key is literally `TODO`, and the
  endpoint says `OWNER`. Until a `v0.0.0-test` tag runs the full chain
  (build → sign → verify → publish → prune), release is a theory.
- **The AI has no brain picked out.** Track M built sidecars the right way —
  deterministic offline baselines behind a one-interface seam, so any model
  slots in without changing op shapes or UI. But v1 Open Question 1 ("which
  local models?") is still unanswered: no names, sizes, or licenses chosen.
  v3 gives every sidecar a verdict: a pinned model or an explicit
  baselines-only deferral.
- **The game hasn't heard it yet.** Game-audio v2 ships demo content
  (`explore`/`combat` + `threat`) and an export package (stems + JSON +
  five-rule validator), but the real proof — importing one package into the
  user's actual game and hearing layers switch on real game state — hasn't
  happened. That demo closes v2's open questions (engine/middleware shape,
  real state names) with evidence instead of defaults.

## 2. The pieces (this plan)

- `.agents/plans/2026-09-12-v3.md` — the v3 plan in the established style:
  Goal, Success Criteria, Context, Constraints, Key Decisions, Work Plan
  (three parallel tracks P-1/P-2/P-3 with validations), Validation Plan,
  Risks, Open Questions, Sources. Three independent build tracks in separate
  new directories: **P-1** release hardening + the test tag, **P-2** sidecar
  model pins, **P-3** open questions + UX polish.
- The rules every track obeys (unchanged since Track 0): contracts in
  `contracts/` are append-only, types regenerate via `bun run typegen`,
  never hand-edit `ui/src/generated/*`, `bun run check` stays green. The
  full gate (test counts, order, e2e setup) lives in
  `docs/notes/n-integration.md`.

## 3. How to tell v3 is done

A friend can install the `v0.0.0-test` build, the docs name every AI model
(or say why none is pinned), and the user's game plays adaptive music
driven by its own state. Anything that misses the tag becomes a written
follow-up with an owner — not a silent gap.
