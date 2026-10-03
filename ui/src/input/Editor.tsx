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

  // Focus the search box on open like the command palette, so Escape lands
  // inside the dialog (where it closes) instead of in the vim layer.
  let searchEl: HTMLInputElement | undefined;
  createEffect(() => {
    if (props.open) {
      setTable(loadEditorTable(activeStore()));
      setQuery("");
      setDrafts({});
      setNotice(null);
      queueMicrotask(() => searchEl?.focus());
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

  // Escape closes like the command palette (which also stops the key from
  // reaching the vim layer while the editor is open).
  function onKey(e: KeyboardEvent) {
    if (e.key === "Escape") {
      e.stopPropagation();
      props.onClose();
    }
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
      <>
        <div class="palette-backdrop" onClick={props.onClose} />
        <div
          role="dialog"
          aria-label="Shortcuts editor"
          class="palette-dialog editor-dialog"
          onKeyDown={onKey}
        >
          <div class="session-controls">
            <strong>Shortcuts</strong>
            <input
              ref={searchEl}
              placeholder="Search keys or actions…"
              value={query()}
              onInput={(e) => setQuery(e.currentTarget.value)}
              style={{ flex: 1 }}
            />
            <button onClick={reset}>Reset to defaults</button>
            <button onClick={props.onClose}>Close</button>
          </div>
          <Show when={notice()}>
            <p class="editor-notice">{notice()}</p>
          </Show>
          <Show when={conflicts().length > 0}>
            <p class="editor-conflicts">
              Conflicts:{" "}
              {conflicts()
                .map((c) => `${c.keys} (${c.actions.join(", ")})`)
                .join("; ")}
            </p>
          </Show>
          <ul class="editor-list">
            <For each={rows()}>
              {(b) => (
                <li class="editor-row">
                  <code class={`editor-keys${badKeys().has(b.keys) ? " contested" : ""}`}>
                    {b.keys === " " ? "Space" : b.keys}
                  </code>
                  <span class="editor-action" title={formatBinding(b)}>
                    {b.action} <small>({b.mode}/{b.context})</small>
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
      </>
    </Show>
  );
}
