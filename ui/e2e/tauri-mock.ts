import type { Page } from "@playwright/test";

/**
 * Web-mode Tauri backend mock.
 *
 * The Solid UI talks to Rust exclusively through
 * `window.__TAURI_INTERNALS__.invoke` (see `@tauri-apps/api@2 core.js`:
 * `invoke()` -> `window.__TAURI_INTERNALS__.invoke(cmd, args)`), with events
 * multiplexed over the `plugin:event|*` commands. There is no real backend in
 * web mode, so every spec installs this mock with `page.addInitScript`
 * BEFORE navigation — the mock must exist before the first `invoke` call.
 *
 * The init payload is a single self-contained function plus JSON-safe canned
 * data (Playwright serializes both across the Node/browser boundary), so this
 * file must not close over Node state inside `installInBrowser`.
 */

/** Canned backend state served to `invoke` by command name. */
export interface CannedBackend {
  projectName: string;
  tracks: Array<{ id: string; name: string }>;
}

export function defaultCanned(): CannedBackend {
  return {
    projectName: "e2e-fixture",
    tracks: [{ id: "t1", name: "Drums" }],
  };
}

export interface IpcCall {
  cmd: string;
  args: unknown;
}

/**
 * Runs INSIDE the browser (serialized by `addInitScript`). Installs
 * `window.__TAURI_INTERNALS__.invoke` with canned responses, an event-plugin
 * shim (`plugin:event|listen/emit/unlisten`), and two inspection hooks:
 * - `window.__e2eIpcCalls`: every invoke call in order.
 * - `window.__e2eEmit(event, payload)`: push a backend event to listeners.
 */
export function installInBrowser(canned: CannedBackend): void {
  const w = window as unknown as Record<string, unknown>;
  const calls: IpcCall[] = [];
  w["__e2eIpcCalls"] = calls;

  type Cb = (data: unknown) => void;
  const callbacks = new Map<number, Cb>();
  const listeners = new Map<string, number[]>();
  let nextId = 1;

  function cannedProject(): Record<string, unknown> {
    return {
      schema_version: 0,
      id: "proj_e2e",
      name: canned.projectName,
      tempo: 120,
      time_sig_num: 4,
      time_sig_den: 4,
      tracks: canned.tracks.map((t) => ({
        id: t.id,
        name: t.name,
        volume: 0.8,
        pan: 0,
        muted: false,
        solo: false,
        clip_ids: [],
        device_ids: [],
      })),
      clips: [],
      devices: [],
      routing: [],
      automation: [],
    };
  }

  const internals = ((w["__TAURI_INTERNALS__"] as Record<string, unknown> | undefined) ?? {}) as Record<
    string,
    unknown
  >;
  w["__TAURI_INTERNALS__"] = internals;
  // Legacy alias some layers probe for; the real transport is INTERNALS.
  w["__TAURI__"] = w["__TAURI__"] ?? {};

  internals["transformCallback"] = (cb: Cb | undefined, once: boolean): number => {
    const id = nextId++;
    callbacks.set(id, (data: unknown) => {
      if (once) callbacks.delete(id);
      if (cb) cb(data);
    });
    return id;
  };
  internals["unregisterCallback"] = (id: number): void => {
    callbacks.delete(id);
  };

  internals["invoke"] = async (cmd: string, args: Record<string, unknown>): Promise<unknown> => {
    calls.push({ cmd, args });
    // Event-plugin shim: listen/emit/unlisten round-trip through callbacks.
    if (cmd === "plugin:event|listen") {
      const event = args["event"] as string;
      const handler = args["handler"] as number;
      const list = listeners.get(event) ?? [];
      list.push(handler);
      listeners.set(event, list);
      return handler;
    }
    if (cmd === "plugin:event|emit") {
      const event = args["event"] as string;
      for (const id of listeners.get(event) ?? []) {
        callbacks.get(id)?.({ event, payload: args["payload"] });
      }
      return null;
    }
    if (cmd === "plugin:event|unlisten") {
      const event = args["event"] as string;
      const id = args["id"] as number;
      listeners.set(event, (listeners.get(event) ?? []).filter((h) => h !== id));
      return null;
    }
    switch (cmd) {
      case "project_get":
      case "project_new":
      case "project_open":
      case "track_list":
        return cannedProject();
      case "engine_play":
        return "Playing";
      case "engine_stop":
        return "Stopped";
      case "engine_set_tempo":
        return null;
      case "project_save":
      case "param_set":
        return null;
      case "op_apply":
      case "op_undo":
      case "op_redo":
        return 1;
      case "track_add":
        return "t_e2e";
      case "clip_add":
        return "clip_e2e";
      default:
        return null;
    }
  };

  w["__e2eEmit"] = (event: string, payload: unknown): void => {
    for (const id of listeners.get(event) ?? []) {
      callbacks.get(id)?.({ event, payload });
    }
  };
}

/** Install the mock on `page` (call before `page.goto`). */
export async function mockTauri(page: Page, canned: CannedBackend = defaultCanned()): Promise<void> {
  await page.addInitScript(installInBrowser, canned);
}

/** Read back every invoke call the page made, in order. */
export async function ipcCalls(page: Page): Promise<IpcCall[]> {
  return page.evaluate(() => (window as unknown as { __e2eIpcCalls: IpcCall[] }).__e2eIpcCalls ?? []);
}

/** Push a backend event into the page's `listen` handlers. */
export async function emitToPage(page: Page, event: string, payload: unknown): Promise<void> {
  await page.evaluate(
    ([e, p]) =>
      (window as unknown as { __e2eEmit: (event: string, payload: unknown) => void }).__e2eEmit(e, p as unknown),
    [event, payload] as [string, unknown],
  );
}
