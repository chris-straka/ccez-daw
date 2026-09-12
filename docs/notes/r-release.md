# r-release — release hardening prep (Track P-1, STOP before secrets/tag)

How the `v0.0.0-test` tag release goes from "pipeline that has never run" to
"proven end to end", and what is left for a human with repo-settings access.

## What this track did

1. **Audited `.github/workflows/release.yml` + `check.yml` for rot** (action
   majors, runner images) against the GitHub API on 2026-09-12.
   - `actions/checkout@v4` → `@v7` (10 uses across both files). v5 moved the
     runtime to Node 24 (min runner v2.327.1, satisfied by all hosted
     runners); v6 persists creds to a separate file (internal, no input
     change); v7 blocks fork-PR checkouts for `pull_request_target` /
     `workflow_run` — we use neither (`push` tags + `pull_request`), so the
     bump is safe. Basic `uses: actions/checkout@vX` interface unchanged.
   - `softprops/action-gh-release@v2` → `@v3` (4 uses in release.yml). v3 is
     Node 20 → Node 24 runtime only; none of the inputs we use
     (`tag_name`, `draft`, `fail_on_unmatched_files`, `files`) changed.
   - Left alone deliberately: `tauri-apps/tauri-action@v1` (v1 tag exists,
     `action-v1.0.0` published 2026-06-29 — already current),
     `oven-sh/setup-bun@v2` (latest is v2.2.0 — major current),
     `dtolnay/rust-toolchain@stable` (floating tag by design).
   - Runner images: `macos-latest`, `windows-latest`, `windows-11-arm`,
     `ubuntu-latest` float (fine); `ubuntu-22.04` stays pinned on purpose —
     Tauri links the build host's glibc/WebKitGTK, so the oldest supported
     base keeps binaries runnable on newer systems (comment in the file).
     `windows-11-arm` works for public repos only; if this repo ever goes
     private, drop the ARM64 Windows job (noted in the file).
2. **Generated the updater keypair.** Non-interactive command (no prompts):
   `bunx --package @tauri-apps/cli tauri signer generate -w /tmp/ccez-daw-updater.key --ci -f`
   (generates without a password; `--ci` skips all prompts). The key was
   generated WITHOUT a password, so `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`
   can be left empty/unset — signing accepts an empty password for an
   unencrypted key.
3. **Pasted the public key** into `plugins.updater.pubkey` in
   `src-tauri/tauri.conf.json` (replacing the TODO). Public keys are safe to
   commit; the private key is NOT in the repo.
4. **`OWNER` endpoint left as-is.** `git remote -v` is empty (no remote
   configured), so the real owner could not be resolved. The endpoint
   `https://github.com/OWNER/ccez-daw/releases/latest/download/latest.json`
   still needs `OWNER` replaced once the repo has a remote / is pushed to
   GitHub.

## User handoff (human does this — agent stops here)

**a) Set secrets** — repo Settings → Secrets and variables → Actions → New
repository secret (NEVER commit these):
- `TAURI_SIGNING_PRIVATE_KEY` = full contents of `/tmp/ccez-daw-updater.key`
  (starts with `untrusted comment: rsync...`, ends with `...secret key`
  block — copy the whole file, then `shred -u` / delete it).
- `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` = leave empty (key has no password).
  If you prefer a password later, regenerate with
  `bunx tauri signer generate -w <path> -p '<password>'` and update both
  the secret and this note.

**b) Fix the endpoint** — in `src-tauri/tauri.conf.json`, replace `OWNER` in
`plugins.updater.endpoints` with the real GitHub org/user once known.

**c) Push the tag** (nobody has — `git tag` is still empty):
`git push origin v0.0.0-test`
This triggers `release.yml`: create-draft → macOS x86_64 → macOS aarch64 →
Windows x64 → Windows arm64 → Linux → rename-assets → verify-release →
publish-release → prune-old-releases (strictly sequential, one draft writer
at a time).

**d) Artifacts to expect** on the auto-published release (verify-release
fails the run if any are missing):
`CcezDAW-macos-apple-silicon.dmg`, `CcezDAW-macos-intel.dmg`,
`CcezDAW-linux-x64.deb`, `CcezDAW-linux-x64.AppImage`,
`CcezDAW-windows-x64-setup.exe`, `CcezDAW-windows-arm64-setup.exe`,
each updater bundle + its `.sig`, and `latest.json` covering all five
updater platforms (`darwin-aarch64`, `darwin-x86_64`, `linux-x86_64`,
`windows-x86_64`, `windows-aarch64`).

**e) First-launch note (ad-hoc signing, no Apple Developer account):** the
macOS bundle is NOT notarized, so a fresh Mac shows an unidentified-developer
gate — right-click the app → Open → Open. Windows SmartScreen likewise shows
"Unknown publisher" (no Authenticode cert; updater signing is unrelated and
IS configured). Default stays ad-hoc until someone enrolls in the Apple
Developer Program (v3 Open Question 1).

## Validation

- `python3 -c yaml.safe_load` on both workflow files (lint) — pass.
- `tauri.conf.json` parses + `pubkey` no longer contains TODO — pass.
- `bun run check` (typegen `--check` + `tsc --noEmit` + core tests) — pass.
