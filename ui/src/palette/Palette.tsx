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
      <>
        <div class="palette-backdrop" onClick={props.onClose} />
        <div
          role="dialog"
          aria-label="Command palette"
          class="palette-dialog"
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
          />
          <ul>
            <For each={matches()}>
              {(a, i) => (
                <li>
                  <button
                    class="palette-item"
                    classList={{ selected: i() === selected() }}
                    onClick={() => void run(a.id)}
                  >
                    {a.title} <span class="palette-id">({a.id})</span>
                  </button>
                </li>
              )}
            </For>
          </ul>
          <Show when={error()}>
            <div class="palette-error">{error()}</div>
          </Show>
        </div>
      </>
    </Show>
  );
}
