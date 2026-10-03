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
export type KeyContext = "global" | "arrangement" | "piano-roll" | "mixer";

/**
 * Named boolean flags a `when` clause can test (e.g. `{ hasSelection: true }`).
 * The live UI builds these from shell state; headless tests pass literals.
 */
export type WhenFlags = Record<string, boolean | undefined>;

export interface Binding {
  /** Key sequence, e.g. `"j"`, `"gg"`, `"gc"`, `"ctrl+k"`, `"escape"`, `" "`. */
  keys: string;
  /** `"all"` = active in every vim mode (e.g. mode switches, palette). */
  mode: VimMode | "all";
  /** `"all"` = active in every UI context. */
  context: KeyContext | "all";
  /** Action id from the action registry. */
  action: string;
  /**
   * Optional when-clause over {@link WhenFlags} names, e.g.
   * `"hasSelection"`, `"!playing"`, `"hasSelection && focused"`,
   * `"takeEmpty || !playing"`. Undefined/empty = always applies.
   * Unparseable clauses fail closed (never apply).
   */
  when?: string;
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
  { keys: "?", mode: "normal", context: "all", action: "help.show_shortcuts" },

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

  // --- `g`-prefixed chords (first `g` waits via prefix `pending`) ---
  { keys: "gc", mode: "normal", context: "all", action: "section.goto.chorus" },
  { keys: "gv", mode: "normal", context: "all", action: "section.goto.verse" },
  { keys: "ga", mode: "normal", context: "all", action: "view.focusArrangement" },
  { keys: "gp", mode: "normal", context: "all", action: "view.focusPianoRoll" },
  { keys: "gm", mode: "normal", context: "all", action: "view.focusMixer" },
  { keys: "gs", mode: "normal", context: "all", action: "view.focusSession" },

  // --- mixer view (context-scoped; inert in other views) ---
  { keys: "m", mode: "normal", context: "mixer", action: "mixer.muteSelected" },
  { keys: "M", mode: "normal", context: "mixer", action: "mixer.soloSelected" },

  // --- post-v0 feature coverage (single-key, all-mode-distinct) ---
  // Programmatic actions (`automation.point_set`, `groove.apply`,
  // `branch.merge`) stay palette/MCP-only: they need lane/clip/branch args
  // no key can supply.
  { keys: "S", mode: "normal", context: "all", action: "session.launch" },
  { keys: "J", mode: "normal", context: "all", action: "session.jam_record" },
  { keys: "C", mode: "normal", context: "all", action: "comp.commit" },
  { keys: "r", mode: "normal", context: "all", action: "record.punch" },
  { keys: "L", mode: "normal", context: "all", action: "link.join" },
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

function bindingApplies(
  b: Binding,
  mode: VimMode,
  context: KeyContext,
  flags: WhenFlags = {},
): boolean {
  return (
    (b.mode === "all" || b.mode === mode) &&
    (b.context === "all" || b.context === context) &&
    evaluateWhenClause(b.when, flags)
  );
}

/**
 * Match accumulated `pending` keystrokes against the table. Returns `match`
 * on an exact sequence, `pending` when the keystrokes are a strict prefix of
 * some binding (wait for more keys), else `none` (reset the buffer).
 * `flags` gates `when`-clause bindings; chords (`gc`, `gg`, …) accumulate
 * across calls in the vim layer's pending buffer.
 */
export function matchKeys(
  bindings: readonly Binding[],
  mode: VimMode,
  context: KeyContext,
  pending: string,
  flags: WhenFlags = {},
): KeyMatch {
  let prefix = false;
  for (const b of bindings) {
    if (!bindingApplies(b, mode, context, flags)) continue;
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
  flags: WhenFlags = {},
): string | undefined {
  const m = matchKeys(bindings, mode, context, keys, flags);
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

// ---------------------------------------------------------------------------
// When-clauses
// ---------------------------------------------------------------------------

/**
 * Evaluate a when-clause over `flags`. Grammar: `||` binds loosest, then
 * `&&`, then unary `!`; atoms are flag names or `true`/`false` literals.
 * Missing flags read as false; anything unparseable fails closed (false).
 */
export function evaluateWhenClause(when: string | undefined, flags: WhenFlags = {}): boolean {
  if (when === undefined || when.trim() === "") return true;
  try {
    const tokens = tokenizeWhen(when);
    if (tokens.length === 0) return false;
    const parser = { tokens, pos: 0 };
    const value = parseWhenOr(parser, flags);
    if (parser.pos !== tokens.length) return false;
    return value;
  } catch {
    return false;
  }
}

function tokenizeWhen(src: string): string[] {
  const out: string[] = [];
  const re = /\s*(!|\|\||&&|\(|\)|[A-Za-z0-9_.-]+|.)\s*/g;
  let m: RegExpExecArray | null;
  while ((m = re.exec(src)) !== null) out.push(m[1]);
  return out;
}

interface WhenParser {
  tokens: string[];
  pos: number;
}

function parseWhenOr(p: WhenParser, flags: WhenFlags): boolean {
  let value = parseWhenAnd(p, flags);
  while (p.tokens[p.pos] === "||") {
    p.pos++;
    value = parseWhenAnd(p, flags) || value;
  }
  return value;
}

function parseWhenAnd(p: WhenParser, flags: WhenFlags): boolean {
  let value = parseWhenUnary(p, flags);
  while (p.tokens[p.pos] === "&&") {
    p.pos++;
    value = parseWhenUnary(p, flags) && value;
  }
  return value;
}

function parseWhenUnary(p: WhenParser, flags: WhenFlags): boolean {
  if (p.tokens[p.pos] === "!") {
    p.pos++;
    return !parseWhenUnary(p, flags);
  }
  return parseWhenPrimary(p, flags);
}

function parseWhenPrimary(p: WhenParser, flags: WhenFlags): boolean {
  const tok = p.tokens[p.pos++];
  if (tok === undefined) throw new Error("unexpected end of when-clause");
  if (tok === "(") {
    const value = parseWhenOr(p, flags);
    if (p.tokens[p.pos++] !== ")") throw new Error("missing closing paren");
    return value;
  }
  if (tok === "true") return true;
  if (tok === "false") return false;
  if (/^[A-Za-z0-9_.-]+$/.test(tok)) return flags[tok] === true;
  throw new Error(`bad token ${tok}`);
}

// ---------------------------------------------------------------------------
// Validation + conflict detection
// ---------------------------------------------------------------------------

const VALID_MODES = new Set(["normal", "insert", "visual", "all"]);
const VALID_CONTEXTS = new Set(["global", "arrangement", "piano-roll", "mixer", "all"]);

/** Null = valid; otherwise a human-readable reason (shown in the editor). */
export function validateBinding(b: Binding): string | null {
  // Note: `" "` (Space) is a legitimate binding, so only zero-length is rejected.
  if (typeof b.keys !== "string" || b.keys.length === 0) return "keys must be non-empty";
  if (typeof b.action !== "string" || b.action.length === 0) return "action must be non-empty";
  if (!VALID_MODES.has(b.mode)) return `unknown mode ${b.mode}`;
  if (!VALID_CONTEXTS.has(b.context)) return `unknown context ${b.context}`;
  if (b.when !== undefined && b.when.trim() !== "" && !isWhenSyntaxOk(b.when)) {
    return `unparseable when-clause ${JSON.stringify(b.when)}`;
  }
  return null;
}

/** True when the clause parses (checked against vacuous flag sets). */
function isWhenSyntaxOk(when: string): boolean {
  try {
    const tokens = tokenizeWhen(when);
    if (tokens.length === 0) return false;
    const parser = { tokens, pos: 0 };
    parseWhenOr(parser, {});
    return parser.pos === tokens.length;
  } catch {
    return false;
  }
}

function modesOverlap(a: Binding["mode"], b: Binding["mode"]): boolean {
  return a === "all" || b === "all" || a === b;
}

function contextsOverlap(a: Binding["context"], b: Binding["context"]): boolean {
  return a === "all" || b === "all" || a === b;
}

export interface BindingConflict {
  /** The contested key sequence. */
  keys: string;
  /** Distinct action ids reachable from these keys in overlapping mode/context. */
  actions: string[];
  /** The overlapping bindings (same keys, different actions). */
  bindings: Binding[];
}

/**
 * Find every key sequence claimed by 2+ different actions in overlapping
 * mode/context. Same-action duplicates (e.g. `h` in normal + visual) are not
 * conflicts. `when`-clauses are reported, not resolved: two gated bindings
 * still conflict when both clauses could hold.
 */
export function findConflicts(bindings: readonly Binding[]): BindingConflict[] {
  const byKeys = new Map<string, Binding[]>();
  for (const b of bindings) {
    const group = byKeys.get(b.keys);
    if (group) group.push(b);
    else byKeys.set(b.keys, [b]);
  }
  const out: BindingConflict[] = [];
  for (const [keys, group] of byKeys) {
    if (group.length < 2) continue;
    const clashing: Binding[] = [];
    for (let i = 0; i < group.length; i++) {
      for (let j = i + 1; j < group.length; j++) {
        const a = group[i];
        const c = group[j];
        if (a.action === c.action) continue;
        if (!modesOverlap(a.mode, c.mode) || !contextsOverlap(a.context, c.context)) continue;
        if (!clashing.includes(a)) clashing.push(a);
        if (!clashing.includes(c)) clashing.push(c);
      }
    }
    if (clashing.length > 0) {
      out.push({ keys, actions: [...new Set(clashing.map((b) => b.action))], bindings: clashing });
    }
  }
  return out;
}

/**
 * Preview what remapping `action` to `newKeys` would clash with, without
 * mutating anything. Self-bindings of `action` are excluded from the report.
 */
export function previewRemapConflicts(
  bindings: readonly Binding[],
  action: string,
  newKeys: string,
): BindingConflict[] {
  const next = bindings.map((b) => (b.action === action ? { ...b, keys: newKeys } : b));
  return findConflicts(next).filter((c) => c.actions.includes(action));
}

// ---------------------------------------------------------------------------
// Persistence (custom bindings survive reloads)
// ---------------------------------------------------------------------------

export const CUSTOM_BINDINGS_STORAGE_KEY = "ccez-daw.keybindings.v1";

export interface BindingStore {
  getItem(key: string): string | null;
  setItem(key: string, value: string): void;
  removeItem(key: string): void;
}

/** Headless-safe in-memory store (also used by tests). */
export function memoryBindingStore(seed: Record<string, string> = {}): BindingStore {
  const map = new Map(Object.entries(seed));
  return {
    getItem: (k) => (map.has(k) ? (map.get(k) as string) : null),
    setItem: (k, v) => {
      map.set(k, v);
    },
    removeItem: (k) => {
      map.delete(k);
    },
  };
}

let sharedMemoryStore: BindingStore | null = null;

/** `localStorage` in the app; a shared in-memory store headlessly. */
export function defaultBindingStore(): BindingStore {
  try {
    const ls = (globalThis as unknown as { localStorage?: BindingStore }).localStorage;
    if (ls && typeof ls.getItem === "function") return ls;
  } catch {
    // Non-DOM runtimes without localStorage fall through below.
  }
  if (!sharedMemoryStore) sharedMemoryStore = memoryBindingStore();
  return sharedMemoryStore;
}

export interface PersistedBindings {
  overrides: RemapOverrides;
  extra: Binding[];
}

function sanitizePersisted(raw: unknown): PersistedBindings {
  const empty: PersistedBindings = { overrides: {}, extra: [] };
  if (typeof raw !== "object" || raw === null) return empty;
  const r = raw as Record<string, unknown>;
  const out: PersistedBindings = { overrides: {}, extra: [] };
  if (typeof r.overrides === "object" && r.overrides !== null) {
    for (const [k, v] of Object.entries(r.overrides as Record<string, unknown>)) {
      if (k.length > 0 && typeof v === "string" && v.length > 0) out.overrides[k] = v;
    }
  }
  if (Array.isArray(r.extra)) {
    for (const b of r.extra) {
      if (typeof b !== "object" || b === null) continue;
      const cand = b as Binding;
      const normalized: Binding = {
        keys: cand.keys,
        mode: cand.mode,
        context: cand.context,
        action: cand.action,
        ...(typeof cand.when === "string" ? { when: cand.when } : {}),
      };
      if (validateBinding(normalized) === null) out.extra.push(normalized);
    }
  }
  return out;
}

/** Load persisted custom bindings; corrupt/missing data yields empty customs. */
export function loadPersistedBindings(store: BindingStore = defaultBindingStore()): PersistedBindings {
  try {
    const raw = store.getItem(CUSTOM_BINDINGS_STORAGE_KEY);
    if (raw === null) return { overrides: {}, extra: [] };
    return sanitizePersisted(JSON.parse(raw) as unknown);
  } catch {
    return { overrides: {}, extra: [] };
  }
}

export function savePersistedBindings(
  data: PersistedBindings,
  store: BindingStore = defaultBindingStore(),
): void {
  store.setItem(CUSTOM_BINDINGS_STORAGE_KEY, JSON.stringify(data));
}

export function clearPersistedBindings(store: BindingStore = defaultBindingStore()): void {
  store.removeItem(CUSTOM_BINDINGS_STORAGE_KEY);
}

/** Defaults plus persisted customs (the table the live shell should use). */
export function loadCustomKeymap(store: BindingStore = defaultBindingStore()): Binding[] {
  const persisted = loadPersistedBindings(store);
  return createKeymap(persisted.overrides, persisted.extra);
}
