import { For, Show, createMemo, createSignal } from "solid-js";
import { ACTIONS, findAction } from "./actions/registry";
import type { EngineState, Project } from "./generated/project";
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
  const [query, setQuery] = createSignal("");
  const [error, setError] = createSignal<string | null>(null);

  const matches = createMemo(() => {
    const q = query().toLowerCase();
    return ACTIONS.filter(
      (a) => a.title.toLowerCase().includes(q) || a.id.includes(q),
    );
  });

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

  void refresh();

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
      <Show when={paletteOpen()}>
        <div style={{ padding: "0 8px" }}>
          <input
            placeholder="Type a command…"
            value={query()}
            onInput={(e) => setQuery(e.currentTarget.value)}
            style={{ width: "100%", padding: "6px" }}
          />
          <ul>
            <For each={matches()}>
              {(a) => (
                <li>
                  <button onClick={() => void runAction(a.id)}>{a.title}</button>
                </li>
              )}
            </For>
          </ul>
        </div>
      </Show>
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
            <div>v0 stub: samples / presets / plugins</div>
          </Panel>
        </div>
        <div style={{ display: "flex", "flex-direction": "column" }}>
          <Panel title="Piano roll">
            <div>v0 stub: FL-style mouse editing lands in Track F</div>
          </Panel>
          <Panel title="Command palette">
            <div>{matches().length} actions registered</div>
          </Panel>
        </div>
      </div>
    </main>
  );
}
