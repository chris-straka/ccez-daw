import { createMemo, createSignal, For, Show } from "solid-js";
import { getDefaultProvider } from "./embeddings";
import { type KindFilter, searchLibrary } from "./library";
import { seedLibrary } from "./seed";
import type { LibraryItem } from "../generated/project";

const KINDS: KindFilter[] = ["All", "Sample", "Preset", "Plugin", "Project"];

/**
 * Track J: the one browser palette.
 *
 * A single search box indexes samples, presets, plugins, and projects
 * together; kind tabs filter, semantic ranking orders. Results show the
 * cosine score so users learn what "similar" means here. `onPick` lets
 * host views (timeline, mixer) insert the chosen entry; by default picking
 * just highlights the row.
 */
export default function BrowserPalette(props: {
  onPick?: (item: LibraryItem) => void;
}) {
  const [query, setQuery] = createSignal("");
  const [kind, setKind] = createSignal<KindFilter>("All");
  const [picked, setPicked] = createSignal<string | null>(null);

  const items = createMemo(() => seedLibrary());
  const results = createMemo(() =>
    searchLibrary(query(), items(), { kind: kind(), topK: 8 }),
  );

  function pick(item: LibraryItem) {
    setPicked(item.id);
    props.onPick?.(item);
  }

  return (
    <div>
      <input
        placeholder="Search samples, presets, plugins, projects…"
        aria-label="Browser search"
        value={query()}
        onInput={(e) => setQuery(e.currentTarget.value)}
        style={{ width: "100%", padding: "6px", "box-sizing": "border-box" }}
      />
      <div style={{ display: "flex", gap: "4px", margin: "6px 0" }}>
        <For each={KINDS}>
          {(k) => (
            <button
              aria-pressed={kind() === k}
              onClick={() => setKind(k)}
              style={{
                "font-weight": kind() === k ? "bold" : "normal",
              }}
            >
              {k}
            </button>
          )}
        </For>
      </div>
      <ul style={{ margin: 0, padding: 0, "list-style": "none" }}>
        <For each={results()}>
          {({ item, score }) => (
            <li>
              <button
                onClick={() => pick(item)}
                style={{
                  display: "flex",
                  gap: "6px",
                  width: "100%",
                  "text-align": "left",
                  background: picked() === item.id ? "#333" : "transparent",
                  color: "inherit",
                  border: "none",
                  padding: "4px 2px",
                  cursor: "pointer",
                }}
              >
                <span
                  style={{
                    "font-size": "11px",
                    color: "#999",
                    "min-width": "52px",
                  }}
                >
                  [{item.kind}]
                </span>
                <span style={{ flex: 1 }}>{item.name}</span>
                <span style={{ "font-size": "11px", color: "#888" }}>
                  {score.toFixed(2)}
                </span>
              </button>
              <Show when={picked() === item.id}>
                <div style={{ "font-size": "12px", color: "#aaa", padding: "0 0 4px 60px" }}>
                  {item.text}
                </div>
              </Show>
            </li>
          )}
        </For>
      </ul>
      <div style={{ "font-size": "11px", color: "#777", margin: "6px 0" }}>
        {results().length} result(s) · {getDefaultProvider().name} · semantic
        similarity search
      </div>
    </div>
  );
}
