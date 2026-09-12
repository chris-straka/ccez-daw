/**
 * Vim modal state machine (Track K).
 *
 * Pure and DOM-free: feed normalized key tokens (see `eventToToken`) into
 * `handleKey` and observe the returned outcome. The live UI attaches it via
 * `attachVimLayer`; headless tests drive `handleKey` directly with the same
 * keymap the UI uses, so replay tests prove real behavior.
 */
import {
  type Binding,
  type KeyContext,
  type VimMode,
  createKeymap,
  matchKeys,
} from "./keybindings";
import {
  type Cursor,
  type GridSize,
  firstPosition,
  lastPosition,
  lineEnd,
  lineStart,
  moveCursor,
  nextClipStart,
  prevClipStart,
} from "./motions";

export type { VimMode };

export interface VimStore {
  mode: VimMode;
  context: KeyContext;
  cursor: Cursor;
  grid: GridSize;
  /** Sorted clip/note start steps for `w`/`b` word motions. */
  clipStarts: readonly number[];
  pending: string;
  /** Ids of non-motion actions dispatched, in order (replay log). */
  dispatched: string[];
}

export interface VimEffects {
  /** Run a registry action (palette/IPC-backed or local). */
  runAction: (id: string) => Promise<unknown> | unknown;
  /** Open the command palette (for the `palette.open` action). */
  openPalette: () => void;
}

export function createVimStore(grid: GridSize = { tracks: 8, steps: 64 }): VimStore {
  return {
    mode: "normal",
    context: "arrangement",
    cursor: { track: 0, step: 0 },
    grid,
    clipStarts: [],
    pending: "",
    dispatched: [],
  };
}

export function setVimMode(store: VimStore, mode: VimMode): void {
  store.mode = mode;
  store.pending = "";
}

export type KeyOutcome =
  | { kind: "dispatched"; action: string }
  | { kind: "pending" }
  | { kind: "ignored" }
  | { kind: "mode-changed"; mode: VimMode }
  | { kind: "moved"; cursor: Cursor };

const MODE_ACTIONS: Record<string, VimMode> = {
  "vim.mode.normal": "normal",
  "vim.mode.insert": "insert",
  "vim.mode.visual": "visual",
};

function applyMotion(store: VimStore, action: string): boolean {
  switch (action) {
    case "vim.motion.left":
      store.cursor = moveCursor(store.cursor, store.grid, "left");
      return true;
    case "vim.motion.down":
      store.cursor = moveCursor(store.cursor, store.grid, "down");
      return true;
    case "vim.motion.up":
      store.cursor = moveCursor(store.cursor, store.grid, "up");
      return true;
    case "vim.motion.right":
      store.cursor = moveCursor(store.cursor, store.grid, "right");
      return true;
    case "vim.motion.wordForward":
      store.cursor = {
        track: store.cursor.track,
        step: nextClipStart(store.clipStarts, store.cursor.step),
      };
      return true;
    case "vim.motion.wordBack":
      store.cursor = {
        track: store.cursor.track,
        step: prevClipStart(store.clipStarts, store.cursor.step),
      };
      return true;
    case "vim.motion.lineStart":
      store.cursor = lineStart(store.cursor);
      return true;
    case "vim.motion.lineEnd":
      store.cursor = lineEnd(store.cursor, store.grid);
      return true;
    case "vim.motion.first":
      store.cursor = firstPosition();
      return true;
    case "vim.motion.last":
      store.cursor = lastPosition(store.grid);
      return true;
    default:
      return false;
  }
}

/**
 * Feed one normalized key token. Multi-key sequences accumulate in
 * `store.pending` until they match, fail (`ignored`, buffer reset), or are a
 * strict prefix (`pending`, wait for more keys).
 */
export function handleKey(
  store: VimStore,
  token: string,
  bindings: readonly Binding[],
  effects: VimEffects,
): KeyOutcome {
  const pending = store.pending + token;
  const m = matchKeys(bindings, store.mode, store.context, pending);
  if (m.kind === "pending") {
    store.pending = pending;
    return { kind: "pending" };
  }
  store.pending = "";
  if (m.kind === "none") return { kind: "ignored" };

  const mode = MODE_ACTIONS[m.action];
  if (mode !== undefined) {
    setVimMode(store, mode);
    return { kind: "mode-changed", mode };
  }
  if (applyMotion(store, m.action)) return { kind: "moved", cursor: store.cursor };
  if (m.action === "palette.open") {
    effects.openPalette();
    store.dispatched.push(m.action);
    return { kind: "dispatched", action: m.action };
  }
  store.dispatched.push(m.action);
  void effects.runAction(m.action);
  return { kind: "dispatched", action: m.action };
}

/**
 * Replay a whole key sequence headlessly (the validation the track contract
 * requires). Returns the outcomes in order.
 */
export function replayKeys(
  store: VimStore,
  tokens: readonly string[],
  bindings: readonly Binding[],
  effects: VimEffects,
): KeyOutcome[] {
  return tokens.map((t) => handleKey(store, t, bindings, effects));
}

/** Default no-op effects for pure replay tests (records nothing further). */
export function stubEffects(log: string[] = []): VimEffects & { log: string[] } {
  return {
    log,
    runAction: (id: string) => {
      log.push(id);
      return Promise.resolve({ stubbed: id });
    },
    openPalette: () => {
      log.push("palette.open");
    },
  };
}

export function defaultKeymap(): Binding[] {
  return createKeymap();
}

/**
 * Attach the vim layer to a DOM root (called once by the App shell).
 * Returns a detach function.
 */
export function attachVimLayer(
  root: { addEventListener: (t: string, fn: (e: never) => void) => void; removeEventListener: (t: string, fn: (e: never) => void) => void },
  store: VimStore,
  bindings: readonly Binding[],
  effects: VimEffects,
  tokenize: (e: never) => string | null,
): () => void {
  const onKey = (e: never) => {
    const token = tokenize(e);
    if (token === null) return;
    handleKey(store, token, bindings, effects);
  };
  root.addEventListener("keydown", onKey);
  return () => root.removeEventListener("keydown", onKey);
}
