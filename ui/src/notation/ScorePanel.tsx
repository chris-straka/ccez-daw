import { createEffect, createMemo, createSignal, For, Show } from "solid-js";
import type { Clip, Op, Project } from "../generated/project";
import { decodeClip, type MidiClip } from "../pianoroll/model";
import { asset_load, asset_store } from "../tauri/commands";
import ScoreView from "./ScoreView";
import type { Clef } from "./pitch";
import {
  emptyStaffFor,
  midiClips,
  readScoreClip,
  resolveScoreClip,
  writeScoreClip,
} from "./score";
import {
  createNoteAtStep,
  deleteSelection,
  pruneSelection,
  type NoteSelection,
} from "./selection";

/**
 * Score panel: the selected MIDI clip as notation, in the shell.
 *
 * - The clip picker lists the project's frozen MIDI clips; the staff shows
 *   the selected one (`resolveScoreClip`, first clip by default).
 * - Note bytes come through the existing `MidiClip` asset path
 *   (`readScoreClip`/`writeScoreClip` over `decodeClip`/`encodeClip`); clips
 *   with no saved bytes yet show an empty staff of the clip's length.
 * - Staff gestures commit through the piano-roll store (`createNoteAtStep`
 *   keeps the same `note_id` sequence, no schema change) and report through
 *   `onEdit`; Commit persists the working note bytes to the asset store —
 *   frozen metadata alone would drop every edit. Content commits are
 *   file-durable, not op-log undoable: there is no note-level op kind.
 * - Pass `assets` to model the engine asset store in tests/hosts; pass
 *   `applyOp` to override the transport (tests).
 */
export default function ScorePanel(props: {
  project: Project | null;
  assets?: Record<string, string>;
  initialClipId?: string;
  applyOp?: (op: Op) => Promise<unknown>;
  onEdit?: (clip: MidiClip, frozen: Clip) => void;
}) {
  const [picked, setPicked] = createSignal<string | null>(props.initialClipId ?? null);
  const [overrides, setOverrides] = createSignal<Record<string, MidiClip>>({});
  const [selection, setSelection] = createSignal<NoteSelection>(new Set<number>());
  const [status, setStatus] = createSignal("pick a MIDI clip — the staff follows it");

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
      setStatus(`score asset refused: ${(e as Error).message}`);
      return { clip: emptyStaffFor(f), frozen: f };
    }
  }

  function commit(next: MidiClip, f: Clip, note: string): void {
    // Round-trip through the asset bytes so what the panel holds is always
    // exactly what the store would persist (byte-stable, same ids).
    const bytes = writeScoreClip(next);
    const stored = readScoreClip(f.source, { [f.source]: bytes }) ?? next;
    setOverrides((o) => ({ ...o, [f.id]: stored }));
    setSelection((s) => pruneSelection(stored, s));
    props.onEdit?.(stored, f);
    setStatus(note);
  }

  function handleSelect(next: NoteSelection): void {
    const w = working();
    setSelection(w ? pruneSelection(w.clip, next) : next);
    setStatus(next.size === 0 ? "selection cleared" : `${next.size} note${next.size === 1 ? "" : "s"} selected`);
  }

  function handleCreate(at: { step: number; clef: Clef; beat: number }): void {
    const w = working();
    if (!w) return;
    try {
      const next = createNoteAtStep(w.clip, at.step, at.clef, at.beat);
      const id = next.notes[next.notes.length - 1]?.note_id;
      commit(next, w.frozen, `added note ${id ?? "?"} (${w.frozen.name})`);
    } catch (e) {
      setStatus(`create refused: ${(e as Error).message}`);
    }
  }

  function removeSelected(): void {
    const w = working();
    if (!w) return;
    const sel = pruneSelection(w.clip, selection());
    if (sel.size === 0) {
      setStatus("nothing selected");
      return;
    }
    commit(deleteSelection(w.clip, sel), w.frozen, `deleted ${sel.size} note${sel.size === 1 ? "" : "s"}`);
  }

  async function commitOp(): Promise<void> {
    const w = working();
    if (!w) return;
    try {
      const bytes = Array.from(new TextEncoder().encode(writeScoreClip(w.clip)));
      await asset_store({ key: assetKey(w.frozen), kind: "midi", bytes });
      const n = w.clip.notes.length;
      setStatus(`Committed ${n} note${n === 1 ? "" : "s"} (saved)`);
    } catch (e) {
      setStatus(`commit refused: ${String(e)}`);
    }
  }

  return (
    <div data-testid="score-panel" class="session-view">
      <Show
        when={props.project}
        fallback={<div class="session-dim">No project open.</div>}
      >
        <Show
          when={clips().length > 0}
          fallback={<div class="session-dim">No MIDI clips in this project.</div>}
        >
          <div class="session-controls">
            <label>
              Clip
              <select
                data-testid="score-clip"
                value={frozen()?.id ?? ""}
                onInput={(e) => {
                  setPicked(e.currentTarget.value || null);
                  setSelection(new Set<number>());
                  setStatus("pick a MIDI clip — the staff follows it");
                }}
                style={{ "margin-left": "4px" }}
              >
                <For each={clips()}>
                  {(c) => <option value={c.id}>{c.name}</option>}
                </For>
              </select>
            </label>
            <span class="clip-chip clip-midi daw-numeric" data-testid="score-meta">
              {frozen()?.name} · {working()?.clip.notes.length ?? 0} notes ·{" "}
              {selection().size} selected
            </span>
            <span class="transport-spacer" />
            <button data-testid="score-delete" onClick={removeSelected}>
              Delete selected
            </button>
            <button data-testid="score-commit" onClick={() => void commitOp()}>
              Commit
            </button>
          </div>
          <Show when={working()}>
            {(w) => (
              <div class="score-sheet">
                <ScoreView
                  clip={w().clip}
                  selected={selection()}
                  onSelect={handleSelect}
                  onCreate={handleCreate}
                />
              </div>
            )}
          </Show>
          <Show when={status()}>
            <div data-testid="score-status" class="meter-readout">
              {status()}
            </div>
          </Show>
        </Show>
      </Show>
    </div>
  );
}
