import { createMemo, createSignal, For, Show } from "solid-js";
import { getDefaultProvider } from "./embeddings";
import { type KindFilter, searchLibrary } from "./library";
import { seedNativePresetLibrary } from "./presets";
import { seedLibrary } from "./seed";
import type { LibraryItem } from "../generated/project";

const KINDS: KindFilter[] = ["All", "Sample", "Preset", "Plugin", "Project"];

/**
 * Track J: the one browser palette.
 *
 * A single search box indexes samples, presets, plugins, and projects
 * together; kind tabs filter, semantic ranking orders. Results show the
 * cosine score so users learn what "similar" means here. `onPick` lets
 * host views (timeline, mixer) insert the chosen entry; it may return a
 * one-line outcome message, shown under the list so picks never fail
 * silently. By default picking just highlights the row.
 */
export default function BrowserPalette(props: {
  onPick?: (item: LibraryItem) => Promise<string | void> | string | void;
}) {
  const [query, setQuery] = createSignal("");
  const [kind, setKind] = createSignal<KindFilter>("All");
  const [picked, setPicked] = createSignal<string | null>(null);
  const [note, setNote] = createSignal<string | null>(null);

  // Demo seeds plus the curated native preset library: every preset is
  // a searchable Preset row, stamped through ParamSet ops by the host view.
  const items = createMemo(() => [...seedLibrary(), ...seedNativePresetLibrary()]);
  const results = createMemo(() =>
    searchLibrary(query(), items(), { kind: kind(), topK: 8 }),
  );

  async function pick(item: LibraryItem) {
    setPicked(item.id);
    setNote(null);
    try {
      const msg = await props.onPick?.(item);
      if (msg) setNote(msg);
    } catch (e) {
      setNote(`pick refused: ${e instanceof Error ? e.message : String(e)}`);
    }
  }

  return (
    <div>
      <input
        class="browser-search"
        placeholder="Search samples, presets, plugins, projects…"
        aria-label="Browser search"
        value={query()}
        onInput={(e) => setQuery(e.currentTarget.value)}
      />
      <div class="browser-tabs">
        <For each={KINDS}>
          {(k) => (
            <button
              aria-pressed={kind() === k}
              classList={{ active: kind() === k }}
              onClick={() => setKind(k)}
            >
              {k}
            </button>
          )}
        </For>
      </div>
      <ul class="browser-list">
        <For each={results()}>
          {({ item, score }) => (
            <li>
              <button
                data-testid={`browser-row-${item.id}`}
                class="browser-row"
                classList={{ selected: picked() === item.id }}
                onClick={() => void pick(item)}
                style={{ display: "flex", gap: "6px", width: "100%", "text-align": "left", border: "none", background: "transparent" }}
              >
                <span class={`kind-chip kind-${item.kind.toLowerCase()}`}>
                  {item.kind}
                </span>
                <span style={{ flex: 1 }}>{item.name}</span>
                <span class="mixer-values session-dim daw-numeric">
                  {score.toFixed(2)}
                </span>
              </button>
              <Show when={picked() === item.id}>
                <div class="session-dim" style={{ padding: "0 0 4px 60px" }}>
                  {item.text}
                </div>
              </Show>
            </li>
          )}
        </For>
      </ul>
      <div class="mixer-foot" style={{ margin: "6px 0" }}>
        {results().length} result(s) · {getDefaultProvider().name} · semantic
        similarity search
      </div>
      <Show when={note()}>
        <div class="mixer-foot" data-testid="browser-note">{note()}</div>
      </Show>
    </div>
  );
}
