import { createEffect, createSignal } from "solid-js";
import { render } from "solid-js/web";
import PianoRoll from "../../src/pianoroll/PianoRoll";
import type { MidiClip } from "../../src/pianoroll/model";

/**
 * Isolated PianoRoll mount for the click-create smoke spec.
 *
 * The main shell only shows a piano-roll stub panel, so e2e mounts the real
 * FL-style canvas here. `onChange` mirrors the clip into `__e2eNotes` and a
 * `data-testid` counter the spec asserts on — no IPC involved (note edits are
 * UI-local; persistence is the caller's job per `pianoroll/store.ts`).
 */
function Fixture() {
  const [clip, setClip] = createSignal<MidiClip>({ length_beats: 8, notes: [] });

  createEffect(() => {
    (window as unknown as { __e2eNotes: unknown }).__e2eNotes = clip().notes;
  });

  return (
    <div style={{ width: "640px", padding: "16px", background: "#1a1a1a" }}>
      <div data-testid="note-count">{clip().notes.length}</div>
      <div data-testid="note-data">{JSON.stringify(clip().notes)}</div>
      <PianoRoll clip={clip()} onChange={setClip} />
    </div>
  );
}

const root = document.getElementById("root");
if (!root) throw new Error("missing #root element");
render(() => <Fixture />, root);
