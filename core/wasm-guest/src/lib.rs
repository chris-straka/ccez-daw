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

// -- test gain device: the mono-`f32` kernel-contract probe --
//
// Teaching note: delay proves a stateful port; gain proves the *shape*
// every guest device shares — one scalar param in, one sample in/out —
// against the simplest native kernel (`kernel::gain_process`). It is
// stateless on purpose: no init ordering, no line to clear, so the
// `devices::wasm` roundtrip test measures only the boundary (load,
// instantiate, param, process), never DSP state. Param code `0` is
// `gain` (linear 0.0..=4.0, default 1.0 — the frozen `Gain` node range).

/// Param code for `gain` (the only gain-device param).
pub const GAIN_CODE_GAIN: i32 = 0;

static mut GAIN: f32 = 1.0;

/// Set the gain (idempotent; re-calling just re-applies).
#[no_mangle]
pub extern "C" fn gain_init(gain: f32) {
    unsafe {
        GAIN = gain;
    }
}

/// Scalar param set. Returns `0` ok / `1` unknown id. Never traps.
#[no_mangle]
pub extern "C" fn gain_param(code: i32, value: f32) -> i32 {
    if code == GAIN_CODE_GAIN {
        unsafe {
            GAIN = value;
        }
        0
    } else {
        1
    }
}

/// Clear gain state: back to unity. (Stateless device, so this is just
/// the default re-applied — reset must still exist so hosts can treat
/// every guest uniformly.)
#[no_mangle]
pub extern "C" fn gain_reset() {
    unsafe {
        GAIN = 1.0;
    }
}

/// One sample through the gain: `x * gain`. Reads the current gain on
/// every call, so a `gain_param` between samples steers the next sample.
#[no_mangle]
pub extern "C" fn gain_sample(x: f32) -> f32 {
    unsafe { x * GAIN }
}

// -- kernel-delay device: port of `devices::kernel::delay_process` --
//
// Teaching note: unlike the `delay_*` exports above (the `dsp::Delay`
// millisecond device with wet/dry mix), this is the Track D kernel
// delay — a feedback comb in *samples* (`y[n] = x[n] + fb * y[n-D]`)
// with no mix stage. The loop body below mirrors
// `kernel::delay_process` op-for-op (same echo read, same multiply-add
// order, same `(pos + 1) % len` advance), which is what makes the
// host roundtrip test bit-exact rather than tolerance-based. Param
// ids mirror `kernel::{DELAY_SAMPLES_PARAM, FEEDBACK_PARAM}`; `0`
// samples bypasses the line, exactly like the kernel.

/// Param code for `delay_samples` (0..=48000, default 0.0).
pub const KDELAY_CODE_DELAY_SAMPLES: i32 = 0;
/// Param code for `feedback` (0.0..=0.95, default 0.0).
pub const KDELAY_CODE_FEEDBACK: i32 = 1;
/// Longest line the guest will hold (the frozen `Delay` node range).
pub const KDELAY_MAX_DELAY: usize = 48000;

/// Feedback comb state. The line always satisfies
/// `line.len() == delay` (`delay == 0` bypasses with the line kept but
/// untouched — the kernel's `ensure`-then-bypass order, applied at
/// set time instead of per block).
struct KernelDelay {
    delay: usize,
    feedback: f32,
    line: Vec<f32>,
    pos: usize,
}

impl KernelDelay {
    fn new(delay_samples: f32, feedback: f32) -> Self {
        let mut g = Self {
            delay: 0,
            feedback: 0.0,
            line: Vec::new(),
            pos: 0,
        };
        g.apply_code(KDELAY_CODE_DELAY_SAMPLES, delay_samples);
        g.apply_code(KDELAY_CODE_FEEDBACK, feedback);
        g
    }

    fn ensure(&mut self) {
        if self.line.len() != self.delay {
            self.line = vec![0.0; self.delay];
            self.pos = 0;
        }
    }

    /// Numeric-code param set. Same clamps as the kernel path (`0`
    /// floor for the delay, `[0, 0.95]` for feedback); returns a code,
    /// never traps.
    fn apply_code(&mut self, code: i32, value: f32) -> i32 {
        match code {
            KDELAY_CODE_DELAY_SAMPLES => {
                self.delay = (value as f64).max(0.0).clamp(0.0, KDELAY_MAX_DELAY as f64) as usize;
                self.ensure();
                0
            }
            KDELAY_CODE_FEEDBACK => {
                self.feedback = (value as f64).clamp(0.0, 0.95) as f32;
                0
            }
            _ => 1,
        }
    }

    fn reset(&mut self) {
        self.delay = 0;
        self.feedback = 0.0;
        self.line = Vec::new();
        self.pos = 0;
    }

    /// One sample through the comb: the body of `kernel::delay_process`,
    /// factored out for the per-sample WASM boundary.
    fn step(&mut self, x: f32) -> f32 {
        if self.delay == 0 {
            return x;
        }
        let fb = self.feedback.clamp(0.0, 0.95);
        let len = self.line.len();
        let echo = self.line[self.pos];
        let y = x + fb * echo;
        self.line[self.pos] = y;
        self.pos = (self.pos + 1) % len;
        y
    }
}

static mut KINST: *mut KernelDelay = ptr::null_mut();

fn kinst() -> Option<&'static mut KernelDelay> {
    // Single-threaded guest: the host drives one call at a time, so a
    // bare pointer is sufficient (no `Sync` needed, no lock to poison).
    unsafe { KINST.as_mut() }
}

/// Construct (or reconstruct) the kernel-delay instance. Re-calling
/// resets state and re-applies params — init is idempotent.
#[no_mangle]
pub extern "C" fn kdelay_init(delay_samples: f32, feedback: f32) {
    unsafe {
        if !KINST.is_null() {
            drop(Box::from_raw(KINST));
            KINST = ptr::null_mut();
        }
        KINST = Box::into_raw(Box::new(KernelDelay::new(delay_samples, feedback)));
    }
}

/// Scalar param set. Returns `0` ok / `1` unknown id / `2` uninit.
#[no_mangle]
pub extern "C" fn kdelay_param(code: i32, value: f32) -> i32 {
    match kinst() {
        Some(g) => g.apply_code(code, value),
        None => GUEST_UNINIT,
    }
}

/// Back to `delay 0 / feedback 0` with a cleared line. Safe to call
/// before init (no-op).
#[no_mangle]
pub extern "C" fn kdelay_reset() {
    if let Some(g) = kinst() {
        g.reset();
    }
}

/// One sample through the comb. Before init, passes input through
/// (never a trap — a half-wired host hears dry, not a crash).
#[no_mangle]
pub extern "C" fn kdelay_sample(x: f32) -> f32 {
    match kinst() {
        Some(g) => g.step(x),
        None => x,
    }
}

// -- soft-clip device: port of `kernel::distortion_process` --
//
// Teaching note: the stateless sibling of gain — one scalar param in,
// one sample in/out — against `kernel::distortion_process`
// (`y = (1+k)*x / (1+k*|x|)`). Stateless on purpose, like gain: no
// init ordering, no line to clear, so the roundtrip test measures only
// the boundary plus the waveshaper formula. Param code `0` is `drive`
// (unitless 0.0..=10.0, default 1.0 — the frozen `Distortion` range).

/// Param code for `drive` (the only soft-clip param).
pub const CLIP_CODE_DRIVE: i32 = 0;

static mut CLIP: f32 = 1.0;

/// Set the drive (idempotent; re-calling just re-applies).
#[no_mangle]
pub extern "C" fn clip_init(drive: f32) {
    unsafe {
        CLIP = drive;
    }
}

/// Scalar param set. Returns `0` ok / `1` unknown id. Never traps.
#[no_mangle]
pub extern "C" fn clip_param(code: i32, value: f32) -> i32 {
    if code == CLIP_CODE_DRIVE {
        unsafe {
            CLIP = value;
        }
        0
    } else {
        1
    }
}

/// Clear clip state: back to the default drive. (Stateless device, so
/// this is just the default re-applied — reset must still exist so
/// hosts can treat every guest uniformly.)
#[no_mangle]
pub extern "C" fn clip_reset() {
    unsafe {
        CLIP = 1.0;
    }
}

/// One sample through the shaper. Reads the current drive on every
/// call, so a `clip_param` between samples steers the next sample.
#[no_mangle]
pub extern "C" fn clip_sample(x: f32) -> f32 {
    unsafe {
        let k = CLIP.max(0.0);
        (1.0 + k) * x / (1.0 + k * x.abs())
    }
}
