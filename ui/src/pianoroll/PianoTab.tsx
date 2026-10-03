import { For, Show, createEffect, createMemo, createSignal, onCleanup } from "solid-js";
import type { Clip, Op, Project } from "../generated/project";
import { decodeClip, midiToFreq, type MidiClip } from "./model";
import { pitchName } from "./geometry";
import { previewNote } from "../audio/preview";
import PianoRoll from "./PianoRoll";
import { asset_load, asset_store } from "../tauri/commands";
import {
  emptyStaffFor,
  midiClips,
  readScoreClip,
  resolveScoreClip,
  writeScoreClip,
} from "../notation/score";

/**
 * Piano tab: the real `PianoRoll` canvas bound to the project's frozen MIDI
 * clips (the shell's old stub panel is gone). Editing is local until Commit,
 * which persists the working note bytes to the asset store under the clip's
 * key — committing frozen metadata alone would drop every edit. Content
 * commits are file-durable, not op-log undoable: there is no note-level op
 * kind. Pass `applyOp` to override the transport (tests).
 */
export default function PianoTab(props: {
  project: Project | null;
  assets?: Record<string, string>;
  applyOp?: (op: Op) => Promise<unknown>;
}) {
  const [picked, setPicked] = createSignal<string | null>(null);
  const [overrides, setOverrides] = createSignal<Record<string, MidiClip>>({});
  const [status, setStatus] = createSignal<string | null>(null);
  const [flashed, setFlashed] = createSignal(false);
  const [range, setRange] = createSignal<{ lo: number; hi: number } | null>(null);
  const [previewing, setPreviewing] = createSignal(false);
  let previewCtx: AudioContext | null = null;
  let previewTimers: number[] = [];

  function stopPreview(): void {
    for (const t of previewTimers) window.clearTimeout(t);
    previewTimers = [];
    previewCtx?.close().catch(() => {});
    previewCtx = null;
    setPreviewing(false);
  }
  onCleanup(stopPreview);

  /**
   * Audition the WORKING notes (committed or not) through a UI-local
   * triangle voice at the project tempo. This is a preview, not the
   * engine: the mix only renders committed timeline clips on play.
   */
  function preview(): void {
    if (previewing()) {
      stopPreview();
      return;
    }
    const w = working();
    if (!w || w.clip.notes.length === 0) {
      setStatus("Nothing to preview — draw some notes first.");
      return;
    }
    try {
      const win = window as unknown as {
        AudioContext?: typeof AudioContext;
        webkitAudioContext?: typeof AudioContext;
      };
      const Ctx = window.AudioContext ?? win.webkitAudioContext;
      if (!Ctx) {
        setStatus("Preview unavailable (no audio device).");
        return;
      }
      const ctx = new Ctx();
      previewCtx = ctx;
      void ctx.resume?.();
      const spb = 60 / (props.project?.tempo || 120);
      const t0 = ctx.currentTime + 0.05;
      const master = ctx.createGain();
      master.gain.value = 0.4;
      master.connect(ctx.destination);
      for (const n of w.clip.notes) {
        if (n.muted) continue;
        const start = t0 + n.start_beats * spb;
        const dur = Math.max(0.05, n.len_beats * spb);
        const osc = ctx.createOscillator();
        osc.type = "triangle";
        osc.frequency.value = midiToFreq(n.pitch);
        const g = ctx.createGain();
        g.gain.setValueAtTime(0.0001, start);
        g.gain.exponentialRampToValueAtTime(0.9, start + 0.01);
        g.gain.setValueAtTime(0.9, start + Math.max(0.01, dur - 0.05));
        g.gain.exponentialRampToValueAtTime(0.0001, start + dur);
        osc.connect(g);
        g.connect(master);
        osc.start(start);
        osc.stop(start + dur + 0.05);
      }
      setPreviewing(true);
      previewTimers.push(window.setTimeout(stopPreview, w.clip.length_beats * spb * 1000 + 400));
    } catch {
      setStatus("Preview unavailable (no audio device).");
    }
  }
  let flashTimer: number | undefined;
  onCleanup(() => window.clearTimeout(flashTimer));

  /** Short confirmation blip on commit (UI-local voice; silent where headless). */
  function blip(): void {
    previewNote(81, { gain: 0.2, seconds: 0.12, type: "sine" });
  }

  const clips = createMemo(() => midiClips(props.project));
  const frozen = createMemo(() => resolveScoreClip(props.project, picked()));

  /** Asset key for a clip's note bytes: explicit source, else per-clip default. */
  function assetKey(f: Clip): string {
    return f.source || `${f.id}.mid`;
  }

  // Hydrate the working clip from durable bytes on first view. Local edits
  // (overrides) always win: a load never clobbers them.
  createEffect(() => {
    const f = frozen();
    if (!f || overrides()[f.id]) return;
    const key = assetKey(f);
    asset_load({ key })
      .then((bytes) => {
        if (!bytes || bytes.length === 0) return;
        try {
          const clip = decodeClip(new Uint8Array(bytes));
          setOverrides((o) => (o[f.id] ? o : { ...o, [f.id]: clip }));
        } catch {
          // Corrupt asset: fall back to the empty staff below.
        }
      })
      .catch(() => {});
  });

  function working(): { clip: MidiClip; frozen: Clip } | null {
    const f = frozen();
    if (!f) return null;
    const local = overrides()[f.id];
    if (local) return { clip: local, frozen: f };
    try {
      const stored = readScoreClip(f.source, props.assets ?? {});
      return { clip: stored ?? emptyStaffFor(f), frozen: f };
    } catch (e) {
      setStatus(`clip asset refused: ${(e as Error).message}`);
      return { clip: emptyStaffFor(f), frozen: f };
    }
  }

  function onChange(next: MidiClip): void {
    const w = working();
    if (!w) return;
    try {
      const bytes = writeScoreClip(next);
      const stored = readScoreClip(w.frozen.source, { [w.frozen.source]: bytes }) ?? next;
      setOverrides((o) => ({ ...o, [w.frozen.id]: stored }));
    } catch (e) {
      setStatus(`edit refused: ${(e as Error).message}`);
    }
  }

  async function commit(): Promise<void> {
    const w = working();
    if (!w) return;
    try {
      // Persist the WORKING clip bytes (the frozen metadata alone would
      // drop every edit). Content commits are file-durable, not op-log
      // undoable: there is no note-level op kind.
      const bytes = Array.from(new TextEncoder().encode(writeScoreClip(w.clip)));
      await asset_store({ key: assetKey(w.frozen), kind: "midi", bytes });
      const n = w.clip.notes.length;
      setStatus(`Committed ${n} note${n === 1 ? "" : "s"} (saved)`);
      blip();
      setFlashed(true);
      window.clearTimeout(flashTimer);
      flashTimer = window.setTimeout(() => setFlashed(false), 900);
    } catch (e) {
      setStatus(`commit refused: ${String(e)}`);
    }
  }

  const current = () => working();

  return (
    <div id="piano-view" data-testid="piano-tab" tabIndex={-1} class="session-view" style={{ height: "100%" }}>
      <Show when={props.project} fallback={<div class="session-dim">No project open.</div>}>
        <div class="session-controls">
          <label>
            Clip{" "}
            <select
              data-testid="piano-clip"
              value={picked() ?? ""}
              onInput={(e) => setPicked(e.currentTarget.value || null)}
            >
              <option value="">—</option>
              <For each={clips()}>
                {(c) => <option value={c.id}>{c.name}</option>}
              </For>
            </select>
          </label>
          <button data-testid="piano-commit" onClick={() => void commit()} disabled={!current()}>
            {flashed() ? "Committed" : "Commit clip"}
          </button>
          <button data-testid="piano-preview" onClick={preview} disabled={!current()}>
            {previewing() ? "Stop preview" : "Preview"}
          </button>
          <Show when={range()}>
            <span data-testid="piano-range" class="session-dim" title="Visible octave range (wheel scrolls, ctrl/cmd+wheel zooms)">
              {pitchName(range()!.lo)}–{pitchName(range()!.hi)}
            </span>
          </Show>
          <Show when={status()}>
            <span data-testid="piano-status" class="session-launch-note">
              {status()}
            </span>
          </Show>
        </div>
        <Show
          when={current()}
          fallback={<div class="session-dim">Pick a MIDI clip to edit on the roll.</div>}
        >
          {(w) => (
            <div style={{ position: "relative", display: "flex", "flex-direction": "column", flex: "1 1 auto", "min-height": "0" }}>
              <PianoRoll clip={w().clip} onChange={onChange} onViewport={setRange} />
              <Show when={w().clip.notes.length === 0}>
                <div class="piano-empty-hint">Empty clip — click the grid to create notes, drag to move, right-drag to delete.</div>
              </Show>
            </div>
          )}
        </Show>
      </Show>
    </div>
  );
}
