import { For, Show, createSignal } from "solid-js";
import type { Project } from "../generated/project";
import {
  captureSnapshot,
  effectiveTrackGain,
  loudnessDb,
  makeGroup,
  matchGainFor,
  recallSnapshot,
  referenceTracks,
  snapshotDiff,
  strips,
  type MixerSnapshot,
  type VcaGroup,
} from "./model";

/**
 * Track G: mixer console view.
 *
 * Strips are a view over the project's tracks + `Audio` edges (same list
 * the audio graph renders). Fader/mute/solo edits emit through `onPatch`
 * so the host can fan them out as frozen `ParamSet` ops; snapshots and
 * groups are UI-local sidecars mirrored from `core/src/mixer.rs`.
 */
export default function Mixer(props: {
  project: Project;
  onPatch: (trackId: string, field: "volume" | "pan" | "muted" | "solo", value: number | boolean) => void;
}) {
  const [groups, setGroups] = createSignal<VcaGroup[]>([
    makeGroup("g_all", "All", props.project.tracks.map((t) => t.id), 0),
  ]);
  const [slotA, setSlotA] = createSignal<MixerSnapshot | null>(null);
  const [slotB, setSlotB] = createSignal<MixerSnapshot | null>(null);
  const [ab, setAb] = createSignal<"A" | "B">("A");
  const [note, setNote] = createSignal("");

  const list = () => strips(props.project);
  const refs = () => referenceTracks(props.project);

  function take(which: "A" | "B") {
    const snap = captureSnapshot(props.project, groups(), `slot-${which}`);
    if (which === "A") setSlotA(snap);
    else setSlotB(snap);
    setNote(`Captured ${which} (${Object.keys(snap.volumes).length} strips)`);
  }

  function flip(which: "A" | "B") {
    const snap = which === "A" ? slotA() : slotB();
    if (!snap) {
      setNote(`Slot ${which} is empty — capture it first`);
      return;
    }
    const live = captureSnapshot(props.project, groups(), "live");
    const changed = snapshotDiff(live, snap);
    const draft = structuredClone(props.project);
    const g = structuredClone(groups());
    recallSnapshot(draft, g, snap);
    for (const t of draft.tracks) {
      const before = props.project.tracks.find((x) => x.id === t.id);
      if (!before) continue;
      if (t.volume !== before.volume) props.onPatch(t.id, "volume", t.volume);
      if (t.pan !== before.pan) props.onPatch(t.id, "pan", t.pan);
      if (t.muted !== before.muted) props.onPatch(t.id, "muted", t.muted);
      if (t.solo !== before.solo) props.onPatch(t.id, "solo", t.solo);
    }
    setGroups(g);
    setAb(which);
    setNote(`Recalled ${which}: ${changed.length} strip(s) changed (${changed.join(", ") || "null — no-op"})`);
  }

  return (
    <div style={{ display: "flex", "flex-direction": "column", gap: "8px" }}>
      <div style={{ display: "flex", gap: "8px", "align-items": "center", "flex-wrap": "wrap" }}>
        <button onClick={() => take("A")}>Capture A</button>
        <button onClick={() => take("B")}>Capture B</button>
        <button onClick={() => flip("A")} disabled={!slotA()}>Recall A{ab() === "A" ? " ●" : ""}</button>
        <button onClick={() => flip("B")} disabled={!slotB()}>Recall B{ab() === "B" ? " ●" : ""}</button>
        <Show when={note()}>
          <span style={{ color: "#8cf", "font-size": "12px" }}>{note()}</span>
        </Show>
      </div>
      <Show when={refs().length > 0}>
        <div style={{ "font-size": "12px", color: "#fa0" }}>
          REF (never in mix): {refs().join(", ")} — cue solo-listens the reference, mix stays muted.
        </div>
      </Show>
      <div style={{ display: "flex", gap: "8px", overflow: "auto" }}>
        <For each={list()}>
          {(s) => (
            <div
              style={{
                border: "1px solid #555",
                "border-radius": "4px",
                padding: "6px",
                "min-width": "110px",
                background: s.isReference ? "#2a2200" : "#222",
              }}
            >
              <div style={{ "font-size": "12px", "font-weight": "bold" }}>{s.name}</div>
              <div style={{ "font-size": "11px", color: "#888" }}>
                → {s.out ?? "master"}
                {s.isReference ? " · REF" : ""}
              </div>
              <label style={{ display: "block", "font-size": "11px" }}>
                vol
                <input
                  type="range"
                  min="0"
                  max="1.5"
                  step="0.01"
                  value={s.volume}
                  onInput={(e) => props.onPatch(s.id, "volume", Number(e.target.value))}
                />
                {s.volume.toFixed(2)} (eff {effectiveTrackGain(props.project, groups(), s.id).toFixed(2)})
              </label>
              <label style={{ display: "block", "font-size": "11px" }}>
                pan
                <input
                  type="range"
                  min="-1"
                  max="1"
                  step="0.01"
                  value={s.pan}
                  onInput={(e) => props.onPatch(s.id, "pan", Number(e.target.value))}
                />
                {s.pan.toFixed(2)}
              </label>
              <div style={{ display: "flex", gap: "4px" }}>
                <button
                  style={{ background: s.muted ? "#a33" : undefined }}
                  onClick={() => props.onPatch(s.id, "muted", !s.muted)}
                >
                  M
                </button>
                <button
                  style={{ background: s.solo ? "#3a3" : undefined }}
                  onClick={() => props.onPatch(s.id, "solo", !s.solo)}
                >
                  S
                </button>
              </div>
            </div>
          )}
        </For>
      </div>
      <div style={{ "font-size": "11px", color: "#888" }}>
        Loudness-matched A/B: compare buffers at equal RMS so louder never wins by default.
        (match gain = rms(A)/rms(B); silence matches at 1.0.)
      </div>
    </div>
  );
}

export { loudnessDb, matchGainFor };
