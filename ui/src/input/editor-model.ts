/**
 * Shortcuts editor model (Track K shortcut hardening).
 *
 * Pure, DOM-free helpers behind `Editor.tsx` (see also `docs/notes/n-shortcuts.md`): list/search the effective
 * binding table, remap one action to new keys (with conflict preview),
 * reset to defaults, and persist customs as an `{overrides, extra}` diff so
 * future defaults still shine through. Headless tests drive these directly.
 */
import {
  type Binding,
  type BindingConflict,
  type BindingStore,
  type PersistedBindings,
  type RemapOverrides,
  DEFAULT_BINDINGS,
  clearPersistedBindings,
  createKeymap,
  defaultBindingStore,
  findConflicts,
  loadPersistedBindings,
  previewRemapConflicts,
  savePersistedBindings,
  validateBinding,
} from "./keybindings";

/** Case-insensitive substring search over keys / action / mode / context. */
export function searchBindings(bindings: readonly Binding[], query: string): Binding[] {
  const q = query.trim().toLowerCase();
  if (q === "") return [...bindings];
  return bindings.filter((b) =>
    `${b.keys} ${b.action} ${b.mode} ${b.context}`.toLowerCase().includes(q),
  );
}

/** The effective table the editor shows (defaults + persisted customs). */
export function loadEditorTable(store: BindingStore = defaultBindingStore()): Binding[] {
  const persisted = loadPersistedBindings(store);
  return createKeymap(persisted.overrides, persisted.extra);
}

export interface RemapResult {
  /** New table (unchanged when `error` is set). */
  bindings: Binding[];
  /** Conflicts the new keys would create (empty = clean remap). */
  conflicts: BindingConflict[];
  /** Validation failure; null when the remap applied. */
  error: string | null;
}

/**
 * Re-key EVERY binding of `actionId` to `newKeys` (same uniform semantics as
 * `createKeymap` overrides). Applies even when conflicts result — the caller
 * surfaces `conflicts` as warnings — but rejects unknown actions and
 * invalid keys without touching the table.
 */
export function remapAction(
  bindings: readonly Binding[],
  actionId: string,
  newKeys: string,
): RemapResult {
  if (!bindings.some((b) => b.action === actionId)) {
    return { bindings: [...bindings], conflicts: [], error: `unknown action ${actionId}` };
  }
  if (newKeys.length === 0) {
    return { bindings: [...bindings], conflicts: [], error: "keys must be non-empty" };
  }
  const next = bindings.map((b) => (b.action === actionId ? { ...b, keys: newKeys } : b));
  for (const b of next) {
    if (b.action !== actionId) continue;
    const problem = validateBinding(b);
    if (problem !== null) {
      return { bindings: [...bindings], conflicts: [], error: problem };
    }
  }
  return { bindings: next, conflicts: previewRemapConflicts(bindings, actionId, newKeys), error: null };
}

/** Fresh copy of the default table (does not touch storage; see `resetEditor`). */
export function resetBindings(): Binding[] {
  return [...DEFAULT_BINDINGS];
}

function bindingKey(b: Binding): string {
  return JSON.stringify([b.keys, b.mode, b.context, b.action, b.when ?? null]);
}

function bindingShape(b: Binding): string {
  return JSON.stringify([b.mode, b.context, b.action, b.when ?? null]);
}

/**
 * Express an edited table as persisted customs: actions uniformly re-keyed
 * from their default shape become `overrides`; rows with no default
 * counterpart become `extra`. Round-trips: `createKeymap(diff(t))` deep-equals
 * `t` for tables produced by `remapAction` over the defaults.
 */
export function diffBindingsAgainstDefaults(current: readonly Binding[]): PersistedBindings {
  const overrides: RemapOverrides = {};
  const defaultShapes = new Map<string, string[]>();
  for (const d of DEFAULT_BINDINGS) {
    const list = defaultShapes.get(d.action);
    if (list) list.push(bindingShape(d));
    else defaultShapes.set(d.action, [bindingShape(d)]);
  }
  const byAction = new Map<string, Binding[]>();
  for (const b of current) {
    const list = byAction.get(b.action);
    if (list) list.push(b);
    else byAction.set(b.action, [b]);
  }
  const overridden = new Set<string>();
  for (const [action, rows] of byAction) {
    const def = defaultShapes.get(action) ?? [];
    const shapes = rows.map(bindingShape).sort();
    if (rows.length > 0 && rows.every((r) => r.keys === rows[0].keys) && shapes.join() === [...def].sort().join()) {
      const before = (DEFAULT_BINDINGS.filter((d) => d.action === action) as Binding[]).every(
        (d) => d.keys === rows[0].keys,
      );
      if (!before) {
        overrides[action] = rows[0].keys;
        overridden.add(action);
      }
    }
  }
  const defaultKeys = new Set(DEFAULT_BINDINGS.map(bindingKey));
  const extra = current.filter((b) => !defaultKeys.has(bindingKey(b)) && !overridden.has(b.action));
  return { overrides, extra };
}

/** Diff an edited table and persist it. */
export function persistEditorTable(
  current: readonly Binding[],
  store: BindingStore = defaultBindingStore(),
): PersistedBindings {
  const data = diffBindingsAgainstDefaults(current);
  savePersistedBindings(data, store);
  return data;
}

/** Clear customs from storage and return a fresh default table. */
export function resetEditor(store: BindingStore = defaultBindingStore()): Binding[] {
  clearPersistedBindings(store);
  return resetBindings();
}

/** One-line display for editor rows and conflict reports. */
export function formatBinding(b: Binding): string {
  const when = b.when !== undefined && b.when !== "" ? ` when=${b.when}` : "";
  return `${b.keys}  ${b.mode}  ${b.context}  ${b.action}${when}`;
}

/** Key sequences currently contested (for conflict badges in the editor). */
export function contestedKeys(bindings: readonly Binding[]): Set<string> {
  return new Set(findConflicts(bindings).map((c) => c.keys));
}
