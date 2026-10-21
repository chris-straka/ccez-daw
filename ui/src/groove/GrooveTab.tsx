import { For, Show, createEffect, createMemo, createSignal } from "solid-js";
import type { Clip, Project } from "../generated/project";
import type { Nonce } from "../input/nonce";
import { decodeClip, type MidiClip } from "../pianoroll/model";
import { emptyStaffFor, midiAssetKey, midiClips, writeScoreClip } from "../notation/score";
import { asset_load, asset_store } from "../tauri/commands";
import GroovePanel from "./GroovePanel";
import type { GroovePool } from "./model";

/**
 * Groove tab: binds `GroovePanel` to the project's MIDI clips. The source
 * is a picked clip (or the demo phrase), the target a picked clip whose
 * notes load from the asset store like the piano roll's. Applying saves the
 * grooved notes under the copy's key and commits it as a `ClipAdded` op.
 * The pool comes from the shell so it outlives the tab.
 */
export default function GrooveTab(props: {
  project: Project | null;
  pool: GroovePool;
  applyNonce?: Nonce;
}) {
  const clips = createMemo(() => midiClips(props.project));
  const [sourceId, setSourceId] = createSignal("");
  const [targetId, setTargetId] = createSignal<string | null>(null);
  const [notes, setNotes] = createSignal<Record<string, MidiClip>>({});

  const find = (id: string | null): Clip | null =>
    clips().find((c) => c.id === id) ?? null;
  const target = createMemo(() => find(targetId()) ?? clips()[0] ?? null);
  const source = createMemo(() => find(sourceId()));

  // Load each picked clip's saved notes once; unsaved clips are empty.
  createEffect(() => {
    for (const c of [source(), target()]) {
      if (!c || notes()[c.id]) continue;
      asset_load({ key: midiAssetKey(c) })
        .then((bytes) => {
          if (!bytes || bytes.length === 0) return;
          const clip = decodeClip(new Uint8Array(bytes));
          setNotes((n) => (n[c.id] ? n : { ...n, [c.id]: clip }));
        })
        .catch(() => {});
    }
  });
  const midiOf = (c: Clip | null): MidiClip | null => (c ? (notes()[c.id] ?? emptyStaffFor(c)) : null);

  async function persist(key: string, clip: MidiClip): Promise<void> {
    const bytes = Array.from(new TextEncoder().encode(writeScoreClip(clip)));
    await asset_store({ key, kind: "midi", bytes });
  }

  return (
    <Show
      when={clips().length > 0}
      fallback={
        <div data-testid="groove-panel" class="session-dim">
          No MIDI clips to groove — add one on the Timeline.
        </div>
      }
    >
      <div class="form-row">
        <label>
          Feel from
          <select
            data-testid="groove-source"
            value={sourceId()}
            onInput={(e) => setSourceId(e.currentTarget.value)}
            style={{ "margin-left": "4px" }}
          >
            <option value="">demo phrase</option>
            <For each={clips()}>{(c) => <option value={c.id}>{c.name}</option>}</For>
          </select>
        </label>
        <label>
          Apply to
          <select
            data-testid="groove-target"
            value={target()?.id ?? ""}
            onInput={(e) => setTargetId(e.currentTarget.value)}
            style={{ "margin-left": "4px" }}
          >
            <For each={clips()}>{(c) => <option value={c.id}>{c.name}</option>}</For>
          </select>
        </label>
      </div>
      <GroovePanel
        source={midiOf(source())}
        sourceName={source()?.name}
        target={midiOf(target())}
        targetName={target()?.name}
        targetClip={target() ?? undefined}
        pool={props.pool}
        persist={persist}
        applyNonce={props.applyNonce}
      />
    </Show>
  );
}
