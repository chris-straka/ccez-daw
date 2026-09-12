#!/bin/sh
# Validate one engine export package directory.
#
# Usage: scripts/validate-export.sh <package-dir> <context.json> [--seed N] [--sample-rate N]
#
# <context.json> bundles the authoring side the package was exported from:
#   { "project": Project, "cues": [...], "banks": [...], "params": [...] }
# Exits 0 when the package is approved to ship, 1 when it is rejected.
set -eu
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PKG="${1:?package dir required}"
CTX="${2:?context json required}"
shift 2
exec cargo run --quiet --manifest-path "$ROOT/core/Cargo.toml" --bin export-validator -- \
  --package "$PKG" --context "$CTX" "$@"
