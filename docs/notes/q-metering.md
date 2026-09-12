# Q primer: metering + mastering DSP (agent 5)

New to loudness metering? Start here. This note teaches the four ideas
behind `core/src/meter/`, then maps each one to the exact function that
implements it. The frozen rules it obeys live in `contracts/` (this
track adds no schema and no IPC — it only *reads* rendered buffers, so
the typegen drift gate cannot fail); the renderer it taps lives in
`core/src/audio/` (Track B primer: `docs/notes/track-b.md`).

## 1. A meter is a tap, not an insert

An insert rewrites the signal; a slow one clicks. A tap only *watches*
it. So every meter here splits in two halves with opposite budgets —
the same split as Track B's
[`device.rs`](../../../core/src/audio/device.rs):

- **Audio half** (`observe` / `process`): runs on the realtime thread.
  No allocation, no locks, no channel `recv`. It only folds samples
  into pre-sized rings and running sums.
- **UI half** (snapshots, spectra): may allocate freely, but never
  waits for the audio thread. It reads through `try_lock` and takes
  "stale by one block" over "blocked" — exactly like Track B's
  `ParamBank`.

The map: [`MeterTap::observe`](../../../core/src/meter/tap.rs) folds
one rendered block in; [`MeterTap::publish`](../../../core/src/meter/tap.rs)
refreshes the shared frame with `try_lock` (a UI-held lock means the
frame is *skipped*, never waited for); the UI polls
[`MeterReader::snapshot`](../../../core/src/meter/tap.rs) whenever it
repaints. `None` means "repaint the old frame".

```rust
let mut tap = MeterTap::new(48_000.0);
let reader = tap.reader(); // moves to the UI thread
tap.observe_node(&buffers, "mix"); // straight out of RenderGraph::render
tap.publish();
if let Some(snap) = reader.snapshot() {
    println!("{:.1} LUFS int, {:.1} dBTP", snap.integrated_lufs.unwrap_or(f32::NEG_INFINITY), snap.true_peak_dbtp);
}
```

## 2. LUFS: K-weight, then square, then gate

`LufsMeter` answers "how loud does this *feel*?" in three standard
ideas (ITU-R BS.1770 style):

1. **K-weighting**: two biquads — a +4 dB highshelf above ~1.7 kHz,
   then a 60 Hz highpass — model the head/ear. A dull rumble and a
   bright hiss with identical RMS read differently. Retuned per sample
   rate in [`KWeight::design`](../../../core/src/meter/lufs.rs); a DC
   test proves the highpass really eats DC.
2. **Rectangular windows**: mean-square energy over the last 400 ms
   ([`momentary_lufs`](../../../core/src/meter/lufs.rs), a syllable)
   and 3 s ([`short_term_lufs`](../../../core/src/meter/lufs.rs), a
   phrase). Rectangular — not exponential — so two implementations
   agree sample for sample. Each window starts evicting once *it*
   fills (an early version gated both on the 3 s ring and read 7.5x
   hot — a steady-tone test pins both windows equal now).
3. **Gating for the integrated number**: chop the track into 400 ms
   blocks hopping every 100 ms, drop blocks below −70 LUFS, average
   the rest, drop blocks 10 LU below that average, average what
   survives ([`integrated_lufs`](../../../core/src/meter/lufs.rs)
   returns `None` when everything gates out). Silence and quiet
   intros must not drag the number down — that is the whole "gated"
   mystery.

Anchors you can trust: a full-scale 1 kHz sine reads ≈ −3 LUFS, and
a −6 dBFS step reads exactly 6.02 LU lower (the chain is linear, so
the relative test is exact independent of calibration). Storage is
pre-sized for 3 s of windows plus 3 h of gated blocks — `process`
never allocates; `set_sample_rate` reallocates and is UI-thread-only,
like swapping the render graph.

```rust
let mut m = LufsMeter::new(48_000.0);
m.process(&block); // audio thread, never allocates
let integ = m.integrated_lufs(); // None on silence
```

## 3. True peak: the peak *between* the samples

A DAC does not play your samples — it reconstructs a smooth wave
*through* them, and that wave overshoots the samples (up to +3 dB on
hot masters). A sample-peak meter lies exactly at the ceiling.
[`TruePeakMeter`](../../../core/src/meter/true_peak.rs) re-samples the
wave 4x denser (the BS.1770 factor) with Catmull-Rom cubic
interpolation and keeps the biggest absolute value — one running
maximum plus 3 history samples, so block seams lose nothing (an
odd-chunk-size test proves it) and a near-Nyquist tone reads strictly
*above* its sample peak. The first sample seeds the history, so a
stream opening on a constant does not hallucinate a 0 → value step.

```rust
let mut t = TruePeakMeter::new();
t.process(&block);
println!("{:.2} dBTP", t.peak_dbtp()); // −inf on silence
```

## 4. Spectrum: FFT on the UI side, never in the callback

An FFT in the audio callback is a classic dropout bug. So the audio
thread only `memcpy`s into a [`SharedRing`](../../../core/src/meter/spectrum.rs)
(behind `try_lock` — a contended lock *drops* the block; a spectrum
frame is indicative), and the UI thread runs
[`spectrum_magnitudes`](../../../core/src/meter/spectrum.rs): a
Hann-windowed radix-2 FFT over the latest window (~60 lines, no
`rustfft` in the realtime crate), grouped into display bins by
maximum so an off-center tone still reports its true height.
[`dominant_freq`](../../../core/src/meter/spectrum.rs) picks the exact
DFT peak. Display bins are linear fractions of Nyquist — map to log
bands at the UI layer:

```rust
let mags = reader.spectrum(4096, 64).expect("frame"); // UI thread only
let log_band = |lo: f32, hi: f32| {
    let to_bin = |f: f32| ((f / 24_000.0 * 64.0) as usize).min(63);
    mags[to_bin(lo)..=to_bin(hi)].iter().fold(0.0f32, |a, &v| a.max(v))
};
let bass = log_band(40.0, 250.0);
```

## 5. How to verify

- `cargo test --manifest-path core/Cargo.toml --lib meter` — 19
  tests: silence gating, full-scale sine ≈ −3 LUFS, exact −6 dB →
  6.02 LU step, momentary/short-term convergence, DC rejection,
  sine true-peak ≈ amplitude, inter-sample overshoot above sample
  peak, on-bin FFT exactness, tone-bin placement, bass/treble
  separation, tap round-trip, contended-publish non-blocking (holds
  the lock on another handle — completion *is* the assertion),
  missing-node no-op, tap-on-a-real-`RenderGraph` buffer map, resets.
- `cargo test --manifest-path core/Cargo.toml` — full suite, no
  regressions.
- `cargo run --manifest-path core/Cargo.toml --bin typegen -- --check`
  — unaffected (this track reads rendered buffers and registers no
  frozen types, so no drift is possible).

## 6. What agent 5 deliberately leaves out

- Stereo/multi-channel weighting (dual-mono sum + channel gains) is
  one wiring step away: every meter here is mono by construction, and
  the tap observes one buffer.
- Loudness-range (LRA percentile statistics) and momentary-max /
  short-term-max holds are UI-side reductions over the existing
  windows/blocks — no audio-thread change needed.
- A `Proc::Meter` render-graph variant would move `observe` inside
  the renderer; today the engine calls `observe_node` on the returned
  buffer map, which is the same data one block later.
- Spectrum log-banding, smoothing (attack/release per bin), and peak
  holds live in the UI layer over `spectrum()` frames.
- `ui/src/generated/*`, `model.rs`, and `ipc.rs` untouched.
