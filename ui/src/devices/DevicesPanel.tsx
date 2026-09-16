import { For, Show, createSignal } from "solid-js";
import type { Node, Project } from "../generated/project";
import DrumRackPanel from "./DrumRackPanel";
import { ArpPanel, ChordPanel, HumanizePanel } from "./MidiFxPanels";
import SamplerPanel from "./SamplerPanel";
import { deviceClassOf, type DeviceClassName } from "./model";

/**
 * Devices shelf: one picker per native device class plus the drum rack.
 *
 * Device lists are a view over the project's frozen `devices` (same list
 * the rack renderer reads); every param edit inside the sub-panels lands
 * as a frozen `ParamSet` op through the existing `op_apply` surface, so
 * device tweaks undo/redo like any other op. Empty classes show their
 * stub text — the shelf never invents devices.
 */
export default function DevicesPanel(props: { project: Project | null }) {
  const [samplerId, setSamplerId] = createSignal<string | null>(null);
  const [arpId, setArpId] = createSignal<string | null>(null);
  const [chordId, setChordId] = createSignal<string | null>(null);
  const [humanizeId, setHumanizeId] = createSignal<string | null>(null);

  const devices = (): Node[] => props.project?.devices ?? [];

  function ofClass(className: DeviceClassName): Node[] {
    return devices().filter((d) => deviceClassOf(d) === className);
  }

  function pick(list: Node[], sel: string | null): Node | null {
    if (list.length === 0) return null;
    return list.find((d) => d.id === sel) ?? list[0];
  }

  function picker(
    testid: string,
    label: string,
    list: Node[],
    sel: string | null,
    setSel: (id: string) => void,
  ) {
    return (
      <div class="form-row">
        <label>
          {label}{" "}
          <select
            data-testid={testid}
            value={pick(list, sel)?.id ?? ""}
            onInput={(e) => setSel(e.currentTarget.value)}
            style={{ "margin-left": "4px", "max-width": "200px" }}
          >
            <For each={list}>{(d) => <option value={d.id}>{d.name}</option>}</For>
          </select>
        </label>
      </div>
    );
  }

  return (
    <div data-testid="devices-panel" class="session-view">
      <Show when={props.project} fallback={<div class="session-dim">No project open.</div>}>
        <div class="view-label">Sampler</div>
        {picker("devices-sampler", "Device", ofClass("sampler"), samplerId(), (id) => setSamplerId(id))}
        <SamplerPanel node={pick(ofClass("sampler"), samplerId())} />
        <div class="view-label">Drum rack</div>
        <DrumRackPanel
          trackId={props.project?.tracks[0]?.id ?? "trk_drums"}
          devices={devices()}
        />
        <div class="view-label">Arpeggiator</div>
        {picker("devices-arp", "Device", ofClass("arpeggiator"), arpId(), (id) => setArpId(id))}
        <ArpPanel node={pick(ofClass("arpeggiator"), arpId())} />
        <div class="view-label">Chord</div>
        {picker("devices-chord", "Device", ofClass("chord"), chordId(), (id) => setChordId(id))}
        <ChordPanel node={pick(ofClass("chord"), chordId())} />
        <div class="view-label">Humanize</div>
        {picker("devices-humanize", "Device", ofClass("humanize"), humanizeId(), (id) => setHumanizeId(id))}
        <HumanizePanel node={pick(ofClass("humanize"), humanizeId())} />
      </Show>
    </div>
  );
}
