/**
 * Remappable keybinding table (Track K).
 *
 * A `Binding` maps a key sequence (e.g. `"g"`, `"gg"`, `"ctrl+k"`) pressed in
 * a vim mode + UI context to an action id from the action registry. EVERY
 * binding is remappable: pass overrides to `createKeymap`; the defaults below
 * are just the starting point. Key matching is pure (no DOM), so headless
 * key-sequence replay tests drive the same table the live UI uses.
 */

export type VimMode = "normal" | "insert" | "visual";
export type KeyContext = "global" | "arrangement" | "piano-roll";

export interface Binding {
  /** Key sequence, e.g. `"j"`, `"gg"`, `"ctrl+k"`, `"escape"`, `" "`. */
  keys: string;
  /** `"all"` = active in every vim mode (e.g. mode switches, palette). */
  mode: VimMode | "all";
  /** `"all"` = active in every UI context. */
  context: KeyContext | "all";
  /** Action id from the action registry. */
  action: string;
}

export const DEFAULT_BINDINGS: readonly Binding[] = [
  // --- global chrome (work in every mode) ---
  { keys: "ctrl+k", mode: "all", context: "all", action: "palette.open" },
  { keys: "escape", mode: "all", context: "all", action: "vim.mode.normal" },
  { keys: "i", mode: "normal", context: "all", action: "vim.mode.insert" },
  { keys: "v", mode: "normal", context: "all", action: "vim.mode.visual" },
  { keys: " ", mode: "normal", context: "all", action: "transport.play" },
  { keys: "s", mode: "normal", context: "all", action: "transport.stop" },
  { keys: "u", mode: "normal", context: "all", action: "project.undo" },
  { keys: "ctrl+r", mode: "normal", context: "all", action: "project.redo" },
  { keys: "ctrl+s", mode: "all", context: "all", action: "project.save" },

  // --- arrangement + piano-roll motions (normal + visual) ---
  { keys: "h", mode: "normal", context: "all", action: "vim.motion.left" },
  { keys: "j", mode: "normal", context: "all", action: "vim.motion.down" },
  { keys: "k", mode: "normal", context: "all", action: "vim.motion.up" },
  { keys: "l", mode: "normal", context: "all", action: "vim.motion.right" },
  { keys: "h", mode: "visual", context: "all", action: "vim.motion.left" },
  { keys: "j", mode: "visual", context: "all", action: "vim.motion.down" },
  { keys: "k", mode: "visual", context: "all", action: "vim.motion.up" },
  { keys: "l", mode: "visual", context: "all", action: "vim.motion.right" },
  { keys: "arrowleft", mode: "normal", context: "all", action: "vim.motion.left" },
  { keys: "arrowdown", mode: "normal", context: "all", action: "vim.motion.down" },
  { keys: "arrowup", mode: "normal", context: "all", action: "vim.motion.up" },
  { keys: "arrowright", mode: "normal", context: "all", action: "vim.motion.right" },
  { keys: "w", mode: "normal", context: "all", action: "vim.motion.wordForward" },
  { keys: "b", mode: "normal", context: "all", action: "vim.motion.wordBack" },
  { keys: "w", mode: "visual", context: "all", action: "vim.motion.wordForward" },
  { keys: "b", mode: "visual", context: "all", action: "vim.motion.wordBack" },
  { keys: "0", mode: "normal", context: "all", action: "vim.motion.lineStart" },
  { keys: "$", mode: "normal", context: "all", action: "vim.motion.lineEnd" },
  { keys: "gg", mode: "normal", context: "all", action: "vim.motion.first" },
  { keys: "G", mode: "normal", context: "all", action: "vim.motion.last" },
  { keys: "0", mode: "visual", context: "all", action: "vim.motion.lineStart" },
  { keys: "$", mode: "visual", context: "all", action: "vim.motion.lineEnd" },
];

/** Remap overrides: action id -> replacement key sequence. */
export type RemapOverrides = Partial<Record<string, string>>;

/**
 * Build the live binding table. An override replaces the key sequence of
 * EVERY binding for that action id; `extra` bindings are appended (for
 * context- or mode-specific additions).
 */
export function createKeymap(
  overrides: RemapOverrides = {},
  extra: readonly Binding[] = [],
): Binding[] {
  const base: Binding[] = DEFAULT_BINDINGS.map((b) => {
    const remap = overrides[b.action];
    return remap !== undefined ? { ...b, keys: remap } : b;
  });
  return [...base, ...extra];
}

export type KeyMatch =
  | { kind: "match"; action: string }
  | { kind: "pending" }
  | { kind: "none" };

function bindingApplies(b: Binding, mode: VimMode, context: KeyContext): boolean {
  return (b.mode === "all" || b.mode === mode) && (b.context === "all" || b.context === context);
}

/**
 * Match accumulated `pending` keystrokes against the table. Returns `match`
 * on an exact sequence, `pending` when the keystrokes are a strict prefix of
 * some binding (wait for more keys), else `none` (reset the buffer).
 */
export function matchKeys(
  bindings: readonly Binding[],
  mode: VimMode,
  context: KeyContext,
  pending: string,
): KeyMatch {
  let prefix = false;
  for (const b of bindings) {
    if (!bindingApplies(b, mode, context)) continue;
    if (b.keys === pending) return { kind: "match", action: b.action };
    if (b.keys.startsWith(pending)) prefix = true;
  }
  return prefix ? { kind: "pending" } : { kind: "none" };
}

/** Resolve a single complete key sequence to its action id, if any. */
export function resolveAction(
  bindings: readonly Binding[],
  mode: VimMode,
  context: KeyContext,
  keys: string,
): string | undefined {
  const m = matchKeys(bindings, mode, context, keys);
  return m.kind === "match" ? m.action : undefined;
}

/**
 * Normalize a browser `KeyboardEvent` to the key-sequence token this table
 * speaks. Multi-key sequences (e.g. `gg`) are built by concatenating tokens
 * in the vim layer.
 */
export function eventToToken(e: { key: string; ctrlKey: boolean; metaKey: boolean }): string {
  const k = e.key.length === 1 ? e.key : e.key.toLowerCase();
  if (e.ctrlKey || e.metaKey) return `ctrl+${k.toLowerCase()}`;
  if (k === " ") return " ";
  return k;
}
