/**
 * UI-local audition voice (WebAudio). Fire-and-forget: plays one pitched
 * blip for pad strikes and confirmations. Silent where headless (tests,
 * CI) — and never the engine mix, which only renders committed clips.
 */
export function previewNote(
  pitch: number,
  opts?: { gain?: number; seconds?: number; type?: OscillatorType },
): void {
  try {
    const win = window as unknown as {
      AudioContext?: typeof AudioContext;
      webkitAudioContext?: typeof AudioContext;
    };
    const Ctx = window.AudioContext ?? win.webkitAudioContext;
    if (!Ctx) return;
    const ctx = new Ctx();
    void ctx.resume?.();
    const freq = 440 * Math.pow(2, (pitch - 69) / 12);
    const dur = Math.min(2, Math.max(0.05, opts?.seconds ?? 0.25));
    const t0 = ctx.currentTime + 0.02;
    const osc = ctx.createOscillator();
    osc.type = opts?.type ?? "triangle";
    osc.frequency.value = freq;
    const g = ctx.createGain();
    const peak = Math.min(1, Math.max(0.0002, opts?.gain ?? 0.4));
    g.gain.setValueAtTime(0.0001, t0);
    g.gain.exponentialRampToValueAtTime(peak, t0 + 0.01);
    g.gain.exponentialRampToValueAtTime(0.0001, t0 + dur);
    osc.connect(g);
    g.connect(ctx.destination);
    osc.start(t0);
    osc.stop(t0 + dur + 0.05);
    osc.onended = () => void ctx.close();
  } catch {
    // No audio device (tests, headless): callers already updated UI state.
  }
}
