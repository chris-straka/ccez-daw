import { For, Show, createEffect, createMemo, createSignal } from "solid-js";
import {
  type Binding,
  type BindingStore,
  defaultBindingStore,
} from "./keybindings";
import {
  contestedKeys,
  formatBinding,
  loadEditorTable,
  persistEditorTable,
  remapAction,
  resetEditor,
  searchBindings,
} from "./editor-model";
import { findConflicts } from "./keybindings";

/**
 * Shortcuts editor (Track K shortcut hardening).
 *
 * Lists the effective binding table with search, remaps any action to new
 * keys (conflicts shown as warnings, never silent), persists customs across
 * reloads, and resets to defaults. Pure logic lives in `editor.ts`; this is
 * a thin Solid view like `Palette.tsx`. Pure logic lives in `editor-model.ts`.
 */
export default function ShortcutEditor(props: {
  open: boolean;
  onClose: () => void;
  store?: BindingStore;
  onKeymapChange?: (bindings: Binding[]) => void;
}) {
  const activeStore = () => props.store ?? defaultBindingStore();
  const [table, setTable] = createSignal<Binding[]>(loadEditorTable(activeStore()));
  const [query, setQuery] = createSignal("");
  const [drafts, setDrafts] = createSignal<Record<string, string>>({});
  const [notice, setNotice] = createSignal<string | null>(null);

  const rows = createMemo(() => searchBindings(table(), query()));
  const badKeys = createMemo(() => contestedKeys(table()));
  const conflicts = createMemo(() => findConflicts(table()));

  createEffect(() => {
    if (props.open) {
      setTable(loadEditorTable(activeStore()));
      setQuery("");
      setDrafts({});
      setNotice(null);
    }
  });

  function commit(next: Binding[]) {
    persistEditorTable(next, activeStore());
    setTable(next);
    props.onKeymapChange?.(next);
  }

  function applyRemap(action: string) {
    const keys = (drafts()[action] ?? "").trim();
    if (keys.length === 0) {
      setNotice(`Type replacement keys for ${action} first.`);
      return;
    }
    const result = remapAction(table(), action, keys);
    if (result.error !== null) {
      setNotice(result.error);
      return;
    }
    commit(result.bindings);
    setDrafts((d) => ({ ...d, [action]: "" }));
    setNotice(
      result.conflicts.length === 0
        ? `${action} is now ${keys}.`
        : `${action} is now ${keys}, but it clashes: ${result.conflicts
            .map((c) => `${c.keys} (${c.actions.join(", ")})`)
            .join("; ")}.`,
    );
  }

  function reset() {
    const fresh = resetEditor(activeStore());
    setTable(fresh);
    setDrafts({});
    setNotice("Shortcuts reset to defaults.");
    props.onKeymapChange?.(fresh);
  }

  return (
    <Show when={props.open}>
      <div
        role="dialog"
        aria-label="Shortcuts editor"
        style={{
          position: "fixed",
          top: "8%",
          left: "50%",
          transform: "translateX(-50%)",
          width: "min(640px, 92vw)",
          "max-height": "80vh",
          overflow: "auto",
          background: "#222",
          border: "1px solid #555",
          "border-radius": "8px",
          padding: "12px",
          "z-index": 50,
        }}
      >
        <div style={{ display: "flex", gap: "8px", "align-items": "center" }}>
          <strong>Shortcuts</strong>
          <input
            placeholder="Search keys or actions…"
            value={query()}
            onInput={(e) => setQuery(e.currentTarget.value)}
            style={{ flex: 1 }}
          />
          <button onClick={reset}>Reset to defaults</button>
          <button onClick={props.onClose}>Close</button>
        </div>
        <Show when={notice()}>
          <p style={{ color: "#fc8" }}>{notice()}</p>
        </Show>
        <Show when={conflicts().length > 0}>
          <p style={{ color: "#f88" }}>
            Conflicts:{" "}
            {conflicts()
              .map((c) => `${c.keys} (${c.actions.join(", ")})`)
              .join("; ")}
          </p>
        </Show>
        <ul style={{ "list-style": "none", padding: 0 }}>
          <For each={rows()}>
            {(b) => (
              <li
                style={{
                  display: "flex",
                  gap: "8px",
                  "align-items": "center",
                  padding: "4px 0",
                  "border-bottom": "1px solid #333",
                }}
              >
                <code style={{ "min-width": "90px", color: badKeys().has(b.keys) ? "#f88" : "#8cf" }}>
                  {b.keys === " " ? "Space" : b.keys}
                </code>
                <span style={{ flex: 1, color: "#ccc" }} title={formatBinding(b)}>
                  {b.action} <small style={{ color: "#888" }}>({b.mode}/{b.context})</small>
                </span>
                <input
                  placeholder="new keys"
                  aria-label={`Remap ${b.action}`}
                  value={drafts()[b.action] ?? ""}
                  onInput={(e) => setDrafts((d) => ({ ...d, [b.action]: e.currentTarget.value }))}
                  style={{ width: "90px" }}
                />
                <button onClick={() => applyRemap(b.action)}>Remap</button>
              </li>
            )}
          </For>
        </ul>
      </div>
    </Show>
  );
}
