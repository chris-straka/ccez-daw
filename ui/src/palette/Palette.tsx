import { For, Show, createEffect, createMemo, createSignal, onCleanup } from "solid-js";
import { filterActions, runPaletteAction } from "./model";

/** Thin Solid view over `palette.ts`: fuzzy input + keyboard navigation. */
export default function Palette(props: {
  open: boolean;
  onClose: () => void;
  onRan?: (id: string) => void;
}) {
  const [query, setQuery] = createSignal("");
  const [selected, setSelected] = createSignal(0);
  const [error, setError] = createSignal<string | null>(null);

  const matches = createMemo(() => filterActions(query()));

  createEffect(() => {
    if (props.open) {
      setQuery("");
      setSelected(0);
      setError(null);
    }
  });

  async function run(id: string) {
    try {
      await runPaletteAction(id, {});
      setError(null);
      props.onRan?.(id);
      props.onClose();
    } catch (e) {
      setError(String(e));
    }
  }

  function onKey(e: KeyboardEvent) {
    if (e.key === "Escape") {
      e.stopPropagation();
      props.onClose();
    } else if (e.key === "ArrowDown") {
      e.preventDefault();
      setSelected((s) => Math.min(s + 1, Math.max(matches().length - 1, 0)));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setSelected((s) => Math.max(s - 1, 0));
    } else if (e.key === "Enter") {
      const m = matches()[selected()];
      if (m) void run(m.id);
    }
  }

  let inputEl: HTMLInputElement | undefined;
  createEffect(() => {
    if (props.open) inputEl?.focus();
  });
  onCleanup(() => setQuery(""));

  return (
    <Show when={props.open}>
      <div
        role="dialog"
        aria-label="Command palette"
        style={{
          position: "fixed",
          top: "12%",
          left: "50%",
          transform: "translateX(-50%)",
          width: "min(560px, 90vw)",
          background: "#222",
          border: "1px solid #555",
          "border-radius": "8px",
          padding: "8px",
          "z-index": 50,
        }}
        onKeyDown={onKey}
      >
        <input
          ref={inputEl}
          placeholder="Type a command…"
          value={query()}
          onInput={(e) => {
            setQuery(e.currentTarget.value);
            setSelected(0);
          }}
          style={{ width: "100%", padding: "6px", "box-sizing": "border-box" }}
        />
        <ul style={{ "list-style": "none", margin: "8px 0 0 0", padding: 0, "max-height": "40vh", overflow: "auto" }}>
          <For each={matches()}>
            {(a, i) => (
              <li>
                <button
                  onClick={() => void run(a.id)}
                  style={{
                    width: "100%",
                    "text-align": "left",
                    padding: "4px 8px",
                    background: i() === selected() ? "#3a3a3a" : "transparent",
                    color: "#eee",
                    border: "none",
                    cursor: "pointer",
                  }}
                >
                  {a.title} <span style={{ color: "#888" }}>({a.id})</span>
                </button>
              </li>
            )}
          </For>
        </ul>
        <Show when={error()}>
          <div style={{ color: "#f88" }}>{error()}</div>
        </Show>
      </div>
    </Show>
  );
}
