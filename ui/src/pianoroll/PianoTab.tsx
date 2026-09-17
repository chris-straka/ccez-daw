import { For, Show, createMemo, createSignal } from "solid-js";
import type { Clip, Op, Project } from "../generated/project";
import type { MidiClip } from "./model";
import PianoRoll from "./PianoRoll";
import { op_apply } from "../tauri/commands";
import {
  emptyStaffFor,
  midiClips,
  readScoreClip,
  resolveScoreClip,
  scoreCommitOp,
  writeScoreClip,
} from "../notation/score";

/**
 * Piano tab: the real `PianoRoll` canvas bound to the project's frozen MIDI
 * clips (the shell's old stub panel is gone). Editing is local until Commit,
 * which lands the clip through one undoable `ClipAdded` op — the same path
 * the score panel uses, so staff and roll always agree byte-for-byte.
 * Pass `applyOp` to override the transport (tests).
 */
export default function PianoTab(props: {
  project: Project | null;
  assets?: Record<string, string>;
  applyOp?: (op: Op) => Promise<unknown>;
}) {
  const [picked, setPicked] = createSignal<string | null>(null);
  const [overrides, setOverrides] = createSignal<Record<string, MidiClip>>({});
  const [status, setStatus] = createSignal<string | null>(null);

  const apply = (op: Op): Promise<unknown> =>
    props.applyOp ? props.applyOp(op) : op_apply({ op });

  const clips = createMemo(() => midiClips(props.project));
  const frozen = createMemo(() => resolveScoreClip(props.project, picked()));

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
      await apply(scoreCommitOp("ui", w.frozen));
      setStatus(`committed ${w.frozen.id} (ClipAdded op, undoable)`);
    } catch (e) {
      setStatus(`commit refused: ${String(e)}`);
    }
  }

  const current = () => working();

  return (
    <div data-testid="piano-tab" class="session-view" style={{ height: "100%" }}>
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
            Commit clip
          </button>
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
              <PianoRoll clip={w().clip} onChange={onChange} />
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
