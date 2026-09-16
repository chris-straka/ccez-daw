import { For, Show, createSignal, onCleanup, onMount } from "solid-js";
import { findAction, registerLocalActionHandler } from "./actions/registry";
import type { EngineState, Project } from "./generated/project";
import { createKeymap, eventToToken } from "./input/keybindings";
import { createVimStore, handleKey } from "./input/vim";
import Palette from "./palette/Palette";
import AutomationView from "./automation/AutomationView";
import GroovePanel from "./groove/GroovePanel";
import BrowserPalette from "./browser/Browser";
import {
  engine_play,
  engine_set_tempo,
  engine_stop,
  op_redo,
  op_undo,
  project_get,
  project_new,
  project_open,
  project_save,
} from "./tauri/commands";
import SessionView from "./timeline/SessionView";
import RecordPanel from "./record/RecordPanel";
import CompPanel from "./comp/CompPanel";
import BranchPanel from "./branch/BranchPanel";
import ScorePanel from "./notation/ScorePanel";
import DevicesPanel from "./devices/DevicesPanel";
import ExportPanel from "./export/ExportPanel";

function Panel(props: { title: string; children?: unknown }) {
  return (
    <section class="daw-panel">
      <h2>{props.title}</h2>
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
  const [tempoDraft, setTempoDraft] = createSignal("120");
  const [savePath, setSavePath] = createSignal("");
  const [projectName, setProjectName] = createSignal("");

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

  async function undo() {
    await op_undo({});
    await refresh();
  }

  async function redo() {
    await op_redo({});
    await refresh();
  }

  async function applyTempo() {
    const bpm = Number(tempoDraft());
    if (!Number.isFinite(bpm) || bpm <= 0) {
      setError(`tempo must be a number > 0 (got ${tempoDraft()})`);
      return;
    }
    await engine_set_tempo({ tempo: bpm });
    await refresh();
  }

  async function save() {
    const path = savePath().trim();
    if (!path) {
      setError("save needs a file path");
      return;
    }
    await project_save({ path });
    await refresh();
  }

  async function open() {
    const path = savePath().trim();
    if (!path) {
      setError("open needs a file path");
      return;
    }
    setProject(await project_open({ path }));
    setError(null);
  }

  async function newProject() {
    const name = projectName().trim() || "Untitled";
    setProject(await project_new({ name }));
    setError(null);
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
    registerLocalActionHandler("view.focusSession", () => {
      document.getElementById("session-view")?.focus({ preventScroll: false });
      document.getElementById("session-view")?.scrollIntoView({ block: "nearest" });
    });
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

  const playing = () => engine() === "Playing";
  return (
    <main class="daw-shell">
      <header class="transport-bar">
        <strong class="transport-brand">ccez-daw v0</strong>
        <button
          class="transport-play"
          classList={{ playing: playing() }}
          onClick={play}
        >
          {playing() ? "■ Stop" : "▶ Play"}
        </button>
        <button onClick={stop}>Stop</button>
        <span class={`engine-badge${playing() ? " playing" : ""}`}>
          {engine()}
        </span>
        <span class={`vim-badge ${vimMode()}`}>
          -- {vimMode().toUpperCase()} --
        </span>
        <span class="transport-readout daw-numeric">cursor {cursor()}</span>
        <button data-testid="undo-btn" onClick={() => void undo()} title="Undo (op_undo)">Undo</button>
        <button data-testid="redo-btn" onClick={() => void redo()} title="Redo (op_redo)">Redo</button>
        <label class="transport-readout">
          Tempo{" "}
          <input
            data-testid="tempo-input"
            type="number"
            min={1}
            value={tempoDraft()}
            onInput={(e) => setTempoDraft(e.currentTarget.value)}
            style={{ width: "64px" }}
          />
          <button data-testid="tempo-set" onClick={() => void applyTempo()}>
            Set
          </button>
        </label>
        <button onClick={() => setPaletteOpen((v) => !v)}>
          Palette (Ctrl+K)
        </button>
        <span class="transport-spacer" />
        <input
          data-testid="project-name"
          placeholder="name"
          aria-label="New project name"
          value={projectName()}
          onInput={(e) => setProjectName(e.currentTarget.value)}
          style={{ width: "110px" }}
        />
        <button data-testid="project-new" onClick={() => void newProject()}>
          New
        </button>
        <input
          data-testid="project-path"
          placeholder="/path/to/project"
          aria-label="Project file path"
          value={savePath()}
          onInput={(e) => setSavePath(e.currentTarget.value)}
          style={{ width: "150px" }}
        />
        <button data-testid="project-open" onClick={() => void open()}>
          Open
        </button>
        <button data-testid="project-save" onClick={() => void save()}>
          Save
        </button>
        <Show when={project()}>
          <span class="transport-readout daw-numeric">
            {project()?.name} @ {project()?.tempo} BPM
          </span>
        </Show>
        <Show when={error()}>
          <span class="transport-error">{error()}</span>
        </Show>
      </header>
      <Palette open={paletteOpen()} onClose={() => setPaletteOpen(false)} onRan={() => void refresh()} />
      <div class="daw-grid">
        <Panel title="Timeline">
          <For each={project()?.tracks ?? []}>
            {(t) => (
              <div class="track-row daw-numeric">
                {t.name} — vol {t.volume} pan {t.pan}
              </div>
            )}
          </For>
        </Panel>
        <div class="daw-column">
          <Panel title="Mixer">
            <For each={project()?.tracks ?? []}>
              {(t) => <div class="track-row">{t.name}</div>}
            </For>
          </Panel>
          <Panel title="Browser">
            <BrowserPalette />
          </Panel>
          <Panel title="Automation">
            <AutomationView project={project()} />
          </Panel>
          <Panel title="Groove">
            <GroovePanel source={null} target={null} />
          </Panel>
        </div>
        <div class="daw-column">
          <Panel title="Piano roll">
            <div class="session-dim">v0 stub: FL-style mouse editing lands in Track F</div>
          </Panel>
          <Panel title="Score">
            <ScorePanel project={project()} />
          </Panel>
          <Panel title="Session">
            <SessionView project={project()} />
          </Panel>
          <Panel title="Record">
            <RecordPanel project={project()} engineState={engine()} />
          </Panel>
          <Panel title="Comp">
            <CompPanel project={project()} />
          </Panel>
          <Panel title="Branches">
            <BranchPanel project={project()} />
          </Panel>
          <Panel title="Devices">
            <DevicesPanel project={project()} />
          </Panel>
          <Panel title="Export">
            <ExportPanel project={project()} />
          </Panel>
          <Panel title="Command palette">
            <div class="session-dim">Ctrl+K — every registry action, fuzzy-matched</div>
          </Panel>
        </div>
      </div>
    </main>
  );
}
