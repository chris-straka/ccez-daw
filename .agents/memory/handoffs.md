---
type: project
---

# ccez-daw v0.0.0-test handoffs (historical checklist, 2026-09-12)

1. GitHub secrets: Settings → Secrets → Actions → `TAURI_SIGNING_PRIVATE_KEY` = contents of local updater private key, then delete any temporary copy. Password secret empty (key has no password).
2. ~~Endpoint: replace `OWNER` in `src-tauri/tauri.conf.json` updater endpoints.~~ Done 2026-10-03: points at `chris-straka/ccez-daw`.
3. Tag: `git push origin v0.0.0-test` runs the full release chain. Expectations in `docs/archive/r-release.md`.
4. Archived GA-4/Godot prototype import checklist (the prototype modules were removed in `e31557f`/`9d046be`; see `docs/archive/s-godot.md`). Action-adventure states (`field`/`combat`/`dungeon`/`boss`/`village`/`night`) + params (`threat`/`time_of_day`/`health_low`/`mounted`).
5. Before any PUBLIC tag: decide Apple Developer enrollment (ad-hoc + right-click-Open OK for test tag); re-verify two model pins (OaF-Drums checkpoint, DeepFilterNet license) — see `docs/notes/r-models.md`.
