---
type: project
---

# ccez-daw v0.0.0-test handoffs (need the user, 2026-09-12)

1. GitHub secrets: Settings → Secrets → Actions → `TAURI_SIGNING_PRIVATE_KEY` = contents of `/tmp/ccez-daw-updater.key`, then delete the key file. Password secret empty (key has no password).
2. Endpoint: replace `OWNER` with real GitHub org/user in `src-tauri/tauri.conf.json` updater endpoints.
3. Tag: `git push origin v0.0.0-test` runs the full release chain. Expectations in `docs/notes/r-release.md`.
4. Game import (now ~5 min with `godot-template/` + runbook in `docs/notes/s-godot.md`): load `godot-template/demo-package` in Godot 4.7.1, toggle states, hear layers switch. Answered: Godot v4.7.1.stable.mono, raw path (no middleware), TP-like states (field/combat/dungeon/boss/village/night) + `threat`/`time_of_day`/`health_low`/`mounted`. Names may evolve as the game grows — report back changes.
5. Before any PUBLIC tag: decide Apple Developer enrollment (ad-hoc + right-click-Open OK for test tag); re-verify two model pins (OaF-Drums checkpoint, DeepFilterNet license) — see `docs/notes/r-models.md`.
