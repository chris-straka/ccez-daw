//! `wasm32-wasip2` guest: the ported delay kernel behind a C ABI.
//!
//! Teaching note: this crate exists so the sandbox can run DSP it does
//! not trust. It contains no DSP of its own — it includes
//! `core/src/wasmdevices/guest.rs` **by path**, so the native reference
//! and the sandboxed guest are the same file and can never drift. The
//! wrappers below are the entire guest/host contract:
//!
//! - `delay_init(sample_rate, time_ms, feedback, mix)` — construct (and
//!   reset) the single guest instance with validated params.
//! - `delay_param(code, value) -> code` — scalar set; `0` ok, `1`
//!   unknown id, `2` uninitialized. Never traps.
//! - `delay_reset()` — clear the line. Never traps.
//! - `delay_sample(x) -> y` — one sample through the loop.
//!
//! The guest imports *nothing* (no WASI, no host functions), so the host
//! instantiates it with an empty `Linker`: the four signatures above are
//! the whole aperture. One instance per module instantiation —
//! polyphony means multiple instances, which is the pooling-allocator
//! follow-up, not a second device kind.

// `guest.rs` also carries host-side helpers (`process`, code/id maps,
// latency probe) the narrow C ABI below does not call — allowed here,
// warned-about in the host crate where they are used.
#[allow(dead_code)]
#[path = "../../src/wasmdevices/guest.rs"]
mod guest;

use guest::GuestDelay;
use std::ptr;

/// Uninitialized-guest return code for `delay_param`.
const GUEST_UNINIT: i32 = 2;

static mut INST: *mut GuestDelay = ptr::null_mut();

fn inst() -> Option<&'static mut GuestDelay> {
    // Single-threaded guest: the host drives one call at a time, so a
    // bare pointer is sufficient (no `Sync` needed, no lock to poison).
    unsafe { INST.as_mut() }
}

/// Construct (or reconstruct) the guest instance. Re-calling resets
/// state and re-applies params — init is idempotent by construction.
#[no_mangle]
pub extern "C" fn delay_init(sample_rate: f32, time_ms: f32, feedback: f32, mix: f32) {
    unsafe {
        if !INST.is_null() {
            drop(Box::from_raw(INST));
            INST = ptr::null_mut();
        }
        let mut g = GuestDelay::new(sample_rate);
        let _ = g.apply_param(guest::PARAM_TIME_MS, time_ms as f64);
        let _ = g.apply_param(guest::PARAM_FEEDBACK, feedback as f64);
        let _ = g.apply_param(guest::PARAM_MIX, mix as f64);
        INST = Box::into_raw(Box::new(g));
    }
}

/// Scalar param set. Returns `0` ok / `1` unknown id / `2` uninit.
#[no_mangle]
pub extern "C" fn delay_param(code: i32, value: f32) -> i32 {
    match inst() {
        Some(g) => g.apply_code(code, value),
        None => GUEST_UNINIT,
    }
}

/// Clear the delay line. Safe to call before init (no-op).
#[no_mangle]
pub extern "C" fn delay_reset() {
    if let Some(g) = inst() {
        g.reset();
    }
}

/// One sample through the loop. Before init, returns silence (never a
/// trap — a half-wired host gets quiet, not a crash).
#[no_mangle]
pub extern "C" fn delay_sample(x: f32) -> f32 {
    match inst() {
        Some(g) => g.step(x),
        None => 0.0,
    }
}
