import { createSignal, Show } from "solid-js";
import type { MidiClip } from "../pianoroll/model";
import {
  applyPropsToMidi,
  applyWarpToMidi,
  cleanupMidi,
  ratioToSemitones,
  samplePhrase,
  semitonesToRatio,
  type CleanupOptions,
  type WarpMarker,
} from "./model";

/**
 * Track I: per-clip pitch/warp + cleanup panel, in-timeline.
 *
 * Props are headless-only for now: the panel edits one clip's `MidiClip`
 * bytes (loaded lazily from the engine asset behind the frozen
 * `Clip.source`, per Track F) and reports the edited clip through
 * `onEdit`. Pitch shows both semitones and ratio so the two time/pitch
 * views agree; warp pins and cleanup options preview on the demo phrase
 * until the asset loader lands (the seam is the `clip` prop).
 */
export default function RepairPanel(props: {
  clip: MidiClip | null;
  clipName?: string;
  onEdit?: (clip: MidiClip) => void;
}) {
  const [pitch, setPitch] = createSignal(0);
  const [ratio, setRatio] = createSignal(1);
  const [warpShift, setWarpShift] = createSignal(0);
  const [cleanup, setCleanup] = createSignal<CleanupOptions>({
    quantizeGrid: 0,
    dropMuted: true,
    fixOverlaps: true,
  });
  const [message, setMessage] = createSignal("");

  const source: () => MidiClip = () => props.clip ?? samplePhrase();

  function previewPitch(st: number): void {
    setPitch(st);
    setRatio(Math.round(semitonesToRatio(st) * 1000) / 1000);
  }

  function previewRatio(r: number): void {
    if (!(r > 0)) {
      setMessage("ratio must be > 0");
      return;
    }
    setRatio(r);
    setPitch(Math.round(ratioToSemitones(r) * 100) / 100);
  }

  function applyPitchTime(): void {
    try {
      const out = applyPropsToMidi(source(), pitch(), ratio());
      props.onEdit?.(out);
      setMessage(`pitch ${pitch()} st @ ${ratio()}x: ${out.notes.length} notes`);
    } catch (e) {
      setMessage(`pitch/time refused: ${(e as Error).message}`);
    }
  }

  function applyWarp(): void {
    const markers: WarpMarker[] = [{ at_beats: 0, shift_beats: warpShift() }];
    const out = applyWarpToMidi(source(), markers);
    props.onEdit?.(out);
    setMessage(`warp ${warpShift() >= 0 ? "+" : ""}${warpShift()} beats`);
  }

  function applyCleanup(): void {
    try {
      const out = cleanupMidi(source(), cleanup());
      props.onEdit?.(out);
      setMessage(`cleanup: ${out.notes.length} notes kept`);
    } catch (e) {
      setMessage(`cleanup refused: ${(e as Error).message}`);
    }
  }

  return (
    <div class="session-view">
      <div class="view-label">
        Repair — {props.clipName ?? "demo phrase"} (in-timeline)
      </div>
      <div class="form-row">
        <label>
          Pitch (st)
          <input
            type="number"
            min={-24}
            max={24}
            step={1}
            value={pitch()}
            onInput={(e) => previewPitch(Number(e.currentTarget.value))}
            style={{ width: "64px", "margin-left": "4px" }}
          />
        </label>
        <label>
          Rate (x)
          <input
            type="number"
            min={0.25}
            max={4}
            step={0.05}
            value={ratio()}
            onInput={(e) => previewRatio(Number(e.currentTarget.value))}
            style={{ width: "64px", "margin-left": "4px" }}
          />
        </label>
        <button onClick={applyPitchTime}>Apply pitch/time</button>
      </div>
      <div class="form-row">
        <label>
          Warp shift (beats)
          <input
            type="number"
            step={0.05}
            value={warpShift()}
            onInput={(e) => setWarpShift(Number(e.currentTarget.value))}
            style={{ width: "64px", "margin-left": "4px" }}
          />
        </label>
        <button onClick={applyWarp}>Apply warp</button>
      </div>
      <div class="form-row">
        <label>
          <input
            type="checkbox"
            checked={cleanup().dropMuted}
            onInput={(e) => setCleanup({ ...cleanup(), dropMuted: e.currentTarget.checked })}
          />
          drop muted
        </label>
        <label>
          <input
            type="checkbox"
            checked={cleanup().fixOverlaps}
            onInput={(e) => setCleanup({ ...cleanup(), fixOverlaps: e.currentTarget.checked })}
          />
          fix overlaps
        </label>
        <label>
          Quantize
          <select
            value={String(cleanup().quantizeGrid)}
            onInput={(e) => setCleanup({ ...cleanup(), quantizeGrid: Number(e.currentTarget.value) })}
            style={{ "margin-left": "4px" }}
          >
            <option value="0">off</option>
            <option value="0.5">8th</option>
            <option value="0.25">16th</option>
          </select>
        </label>
        <button onClick={applyCleanup}>Apply cleanup</button>
      </div>
      <Show when={message()}>
        <div class="meter-readout">{message()}</div>
      </Show>
    </div>
  );
}
