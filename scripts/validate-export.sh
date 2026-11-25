#!/bin/sh
# Validate one engine export package directory.
#
# Usage: scripts/validate-export.sh <package-dir> <context.json> [--seed N] [--sample-rate N]
#        [--click-threshold F] [--waive <loop-waivers.json>]
#
# <context.json> bundles the authoring side the package was exported from:
#   { "project": Project, "cues": [...], "banks": [...], "params": [...] }
# Rule 6 (S-2 loop seam) rejects music-loop stems whose boundary step
# |last - first| exceeds --click-threshold (default 0.02; 0 disables). A
# clicking stem ships two ways — fix (re-export with loop-clean edges) or
# waive (--waive points at `{ "waivers": [{ "path", "reason" }] }`; every
# entry needs a non-empty reason, stale entries fail). Extra flags pass
# through to the export-validator binary verbatim.
# Exits 0 when the package is approved to ship, 1 when it is rejected.
set -eu
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PKG="${1:?package dir required}"
CTX="${2:?context json required}"
shift 2
exec cargo run --quiet --manifest-path "$ROOT/core/Cargo.toml" --bin export-validator -- \
  --package "$PKG" --context "$CTX" "$@"
