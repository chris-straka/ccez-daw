import { For, Show } from "solid-js";
import type { EdgeKind, Project } from "../generated/project";
import { cablePoints, layoutPatch } from "./layout";

const KIND_COLORS: Record<EdgeKind, string> = {
  Audio: "#6af",
  Midi: "#4d4",
  Modulation: "#da6",
  Sidechain: "#a6d",
};

/**
 * Track G: visual patch graph over the one routing model.
 *
 * Nodes are tracks, devices, clips, and implicit buses laid out in
 * signal-flow layers (sources left, mix points right); cables are the
 * frozen `Edge` rows, colored by kind. Control-rate cables (modulation,
 * sidechain) never affect layout — same rule as `core/src/audio/graph.rs`.
 * Read-only for now: edits go through `addEdge` / `removeEdge` /
 * `reconnectEdge` in `./model`, and the parent re-renders with the new
 * `Project`. Cyclic audio cables paint red and are listed by name.
 */
export default function PatchView(props: {
  project: Project | null;
  onSelectEdge?: (edgeId: string | null) => void;
}) {
  return (
    <div style={{ display: "flex", "flex-direction": "column", gap: "8px" }}>
      <Show
        when={props.project}
        fallback={<div style={{ color: "#888" }}>No project open.</div>}
      >
        {(project) => {
          const layout = () => layoutPatch(project());
          return (
            <div>
              <Show when={layout().cycle.length > 0}>
                <div style={{ color: "#f88", "font-size": "12px" }}>
                  Audio cycle: {layout().cycle.join(", ")} — remove a cable to render.
                </div>
              </Show>
              <svg
                width={(maxLayer(layout().nodes) + 1) * 180}
                height={Math.max(1, layout().nodes.length) * 56}
                style={{ display: "block" }}
                role="img"
                aria-label="Patch graph"
              >
                <For each={layout().edges}>
                  {(edge) => {
                    const pts = cablePoints(layout(), edge);
                    return (
                      <Show when={pts}>
                        {(p) => (
                          <line
                            x1={p().x1}
                            y1={p().y1}
                            x2={p().x2}
                            y2={p().y2}
                            stroke={KIND_COLORS[edge.kind]}
                            stroke-width="2"
                            opacity={edge.kind === "Audio" || edge.kind === "Midi" ? 1 : 0.7}
                            stroke-dasharray={
                              edge.kind === "Modulation" || edge.kind === "Sidechain"
                                ? "6 3"
                                : undefined
                            }
                            onClick={() => props.onSelectEdge?.(edge.id)}
                          >
                            <title>{`${edge.from_node}:${edge.from_port} -> ${edge.to_node}:${edge.to_port} (${edge.kind})`}</title>
                          </line>
                        )}
                      </Show>
                    );
                  }}
                </For>
                <For each={layout().nodes}>
                  {(node) => (
                    <g>
                      <rect
                        x={node.x}
                        y={node.y}
                        width={node.w}
                        height={node.h}
                        rx="4"
                        fill={node.inCycle ? "#422" : "#222"}
                        stroke={node.inCycle ? "#f88" : "#666"}
                      >
                        <title>{`${node.id} (${node.role}, layer ${node.layer})`}</title>
                      </rect>
                      <text x={node.x + 8} y={node.y + 21} fill="#eee" font-size="12">
                        {node.id}
                      </text>
                    </g>
                  )}
                </For>
              </svg>
              <div style={{ color: "#888", "font-size": "11px" }}>
                <For each={Object.entries(KIND_COLORS)}>
                  {([kind, color]) => <span style={{ color, "margin-right": "8px" }}>— {kind}</span>}
                </For>
                <span>· dashed = control-rate (no order, no cycle)</span>
              </div>
            </div>
          );
        }}
      </Show>
    </div>
  );
}

function maxLayer(nodes: Array<{ layer: number }>): number {
  let max = 0;
  for (const n of nodes) max = Math.max(max, n.layer);
  return max;
}
