import { For, Show, createSignal, onCleanup, onMount } from "solid-js";
import { findAction, registerLocalActionHandler } from "./actions/registry";
import type { EngineState, Project } from "./generated/project";
import { createKeymap, eventToToken } from "./input/keybindings";
import { createVimStore, handleKey } from "./input/vim";
import Palette from "./palette/Palette";
import BrowserPalette from "./browser/Browser";
import { engine_play, engine_stop, project_get } from "./tauri/commands";

function Panel(props: { title: string; children?: unknown }) {
  return (
    <section
      style={{
        border: "1px solid #444",
        "border-radius": "6px",
        padding: "8px",
        margin: "4px",
        "min-width": "0",
      }}
    >
      <h2 style={{ margin: "0 0 8px 0", "font-size": "13px", color: "#aaa" }}>
        {props.title}
      </h2>
      {props.children as never}
    </section>
  );
}

export default function App() {
  const [project, setProject] = createSignal<Project | null>(null);
  const [engine, setEngine] = createSignal<EngineState>("Stopped");
  const [paletteOpen, setPaletteOpen] = createSignal(false);
  const [vimMode, setVimMode] = createSignal("normal");
  const [cursor, setCursor] = createSignal("0:0");
  const [error, setError] = createSignal<string | null>(null);

  // Track K: one vim store + remappable keymap for the whole shell.
  const vim = createVimStore();
  const keymap = createKeymap();

  async function refresh() {
    try {
      setProject(await project_get({}));
      setError(null);
    } catch (e) {
      // Running in a plain browser (no Tauri bridge) shows the shell with
      // placeholder state instead of crashing: the dev/build cycle proof.
      setError(`Tauri bridge unavailable: ${String(e)}`);
    }
  }

  async function play() {
    setEngine(await engine_play({}));
  }

  async function stop() {
    setEngine(await engine_stop({}));
  }

  async function runAction(id: string) {
    const a = findAction(id);
    if (!a) return;
    await a.run({});
    setPaletteOpen(false);
    await refresh();
  }

  onMount(() => {
    registerLocalActionHandler("palette.open", () => setPaletteOpen(true));
    registerLocalActionHandler("vim.mode.normal", () => setVimMode("normal"));
    registerLocalActionHandler("vim.mode.insert", () => setVimMode("insert"));
    registerLocalActionHandler("vim.mode.visual", () => setVimMode("visual"));

    const onKey = (e: KeyboardEvent) => {
      if (paletteOpen()) return; // palette handles its own keys
      const t = e.target as HTMLElement | null;
      if (t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA")) {
        if (e.key !== "Escape") return;
      }
      const out = handleKey(vim, eventToToken(e), keymap, {
        runAction: (id) => void runAction(id),
        openPalette: () => setPaletteOpen(true),
      });
      if (out.kind === "mode-changed") setVimMode(out.mode);
      else if (out.kind === "moved") setCursor(`${out.cursor.track}:${out.cursor.step}`);
      else if (out.kind === "dispatched" || out.kind === "pending") setVimMode(vim.mode);
      if (out.kind !== "ignored") e.preventDefault();
    };
    window.addEventListener("keydown", onKey);
    onCleanup(() => window.removeEventListener("keydown", onKey));
    void refresh();
  });

  return (
    <main
      style={{
        background: "#1a1a1a",
        color: "#eee",
        height: "100vh",
        display: "flex",
        "flex-direction": "column",
        margin: 0,
        "font-family": "system-ui, sans-serif",
      }}
    >
      <header
        style={{ display: "flex", gap: "8px", padding: "8px", "align-items": "center" }}
      >
        <strong>ccez-daw v0</strong>
        <button onClick={play}>Play</button>
        <button onClick={stop}>Stop</button>
        <span>Engine: {engine()}</span>
        <span style={{ color: "#8cf" }}>-- {vimMode().toUpperCase()} --</span>
        <span style={{ color: "#888" }}>cursor {cursor()}</span>
        <button onClick={() => setPaletteOpen((v) => !v)}>
          Palette (Ctrl+K)
        </button>
        <Show when={project()}>
          <span>
            {project()?.name} @ {project()?.tempo} BPM
          </span>
        </Show>
        <Show when={error()}>
          <span style={{ color: "#f88" }}>{error()}</span>
        </Show>
      </header>
      <Palette open={paletteOpen()} onClose={() => setPaletteOpen(false)} onRan={() => void refresh()} />
      <div style={{ display: "grid", "grid-template-columns": "2fr 1fr 1fr", flex: 1 }}>
        <Panel title="Timeline">
          <For each={project()?.tracks ?? []}>
            {(t) => (
              <div>
                {t.name} — vol {t.volume} pan {t.pan}
              </div>
            )}
          </For>
        </Panel>
        <div style={{ display: "flex", "flex-direction": "column" }}>
          <Panel title="Mixer">
            <For each={project()?.tracks ?? []}>
              {(t) => <div>{t.name}</div>}
            </For>
          </Panel>
          <Panel title="Browser">
            <BrowserPalette />
          </Panel>
        </div>
        <div style={{ display: "flex", "flex-direction": "column" }}>
          <Panel title="Piano roll">
            <div>v0 stub: FL-style mouse editing lands in Track F</div>
          </Panel>
          <Panel title="Command palette">
            <div>Ctrl+K — every registry action, fuzzy-matched</div>
          </Panel>
        </div>
      </div>
    </main>
  );
}
