#!/usr/bin/env bash
# Regenerate audioforge's ccez-daw playback fixture from the shared tiny
# composition. audioforge's tests/ccez_daw_export.rs plays it through the
# real runtime, so a format change on either side shows up there.
#   scripts/export-audioforge-fixture.sh [path/to/audioforge]
set -euo pipefail
here="$(cd "$(dirname "$0")/.." && pwd)"
af="${1:-$here/../audioforge}"
out="$af/crates/af-runtime/tests/fixtures/ccez-daw-tiny"
cargo build --release --manifest-path "$here/core/Cargo.toml" --bin ccez-compose
rm -rf "$out"
"$here/target/release/ccez-compose" export \
  "$here/contracts/fixtures/audioforge-tiny.composition.json" --out "$out"
echo "wrote $out"
