#!/bin/sh
# Build the wasmdevices delay guest and stage it for tests.
#
# Usage: scripts/build-wasm-guest.sh
#
# Compiles core/wasm-guest (which includes core/src/wasmdevices/guest.rs
# by path, so guest and native reference cannot drift) for two targets:
#
# - wasm32-unknown-unknown (staged): a core module importing NOTHING, so
#   the sandbox instantiates it with an empty Linker — the narrowest
#   possible aperture. This is the artifact the `--features wasm-runtime`
#   tests load via include_bytes!. Commit the result so the gate is
#   hermetic.
# - wasm32-wasip2 (compile proof only): the same source compiles to the
#   WASI P2 target, but rustc emits a Component Model component whose
#   std pulls WASI imports (clocks/cli/io — inspect with
#   `strings delay_guest_wasip2.wasm | grep wasi:`). Hosting that needs a
#   full WASI context (wasmtime-wasi), which widens the aperture this
#   track deliberately keeps at zero imports — so the component-API
#   wiring is the production follow-up, not this script's output.
set -eu
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
rustup target add wasm32-unknown-unknown >/dev/null 2>&1 || true
rustup target add wasm32-wasip2 >/dev/null 2>&1 || true
# panic=abort: a wasm guest cannot unwind; a panicking guest aborts the
# instance (a caught trap host-side) instead of corrupting the stack.
export CARGO_PROFILE_DEV_PANIC=abort CARGO_PROFILE_RELEASE_PANIC=abort
cargo build --quiet --manifest-path "$ROOT/Cargo.toml" -p ccez-wasm-delay \
  --target wasm32-unknown-unknown --release
mkdir -p "$ROOT/core/src/wasmdevices/testdata"
cp "$ROOT/target/wasm32-unknown-unknown/release/ccez_wasm_delay.wasm" \
  "$ROOT/core/src/wasmdevices/testdata/delay_guest.wasm"
cargo build --quiet --manifest-path "$ROOT/Cargo.toml" -p ccez-wasm-delay \
  --target wasm32-wasip2 --release
ls -l "$ROOT/core/src/wasmdevices/testdata/delay_guest.wasm" \
  "$ROOT/target/wasm32-wasip2/release/ccez_wasm_delay.wasm"
