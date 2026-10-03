import { For, Show, createEffect, createSignal, onCleanup, onMount } from "solid-js";
import { LOCAL_GUIDANCE, findAction, registerLocalActionHandler } from "./actions/registry";
import type { EngineState, Project } from "./generated/project";
import { listen } from "@tauri-apps/api/event";
import { eventToToken, loadCustomKeymap } from "./input/keybindings";
import { createVimStore, handleKey, setVimContext } from "./input/vim";
import {
  contextForTab,
  elementIdForFocusAction,
  findSectionByName,
  sectionNameForGotoAction,
  tabForFocusAction,
} from "./input/view-nav";
import { sampleTimelineDoc } from "./timeline/model";
import Palette from "./palette/Palette";
import ShortcutEditor from "./input/Editor";
import AutomationView from "./automation/AutomationView";
import GroovePanel from "./groove/GroovePanel";
import BrowserPalette from "./browser/Browser";
import { deviceClassOf, findNativePreset, presetParamOps } from "./browser/presets";
import type { LibraryItem } from "./generated/project";
import {
  engine_play,
  engine_position,
  engine_set_tempo,
  engine_stop,
  link_toggle,
  op_apply,
  op_redo,
  op_undo,
  project_get,
  project_new,
  project_open,
  project_save,
  track_add,
} from "./tauri/commands";
import { Mixer } from "./mixer";
import { TimelineView } from "./timeline";
import { PianoTab } from "./pianoroll";
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
  const [workspaceTab, setWorkspaceTab] = createSignal("Timeline");
  const [shortcutsOpen, setShortcutsOpen] = createSignal(false);

  // Track K: one vim store + remappable keymap for the whole shell. The
  // keymap is a signal so the shortcuts editor applies remaps live, and it
  // starts from persisted customs so remaps survive reloads.
  const vim = createVimStore();
  const [keymap, setKeymap] = createSignal(loadCustomKeymap());

  // Vim key context follows the workspace: piano-roll keys only make sense
  // on the Piano roll tab, mixer keys when the mixer has focus. Runs only
  // when the tab changes, so an explicit mixer focus (`gm`, mixer click)
  // is not clobbered.
  createEffect(() => {
    setVimContext(vim, contextForTab(workspaceTab()));
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

  async function togglePlay() {
    // Space stops a recording pass too (standard DAW behavior); starting a
    // take is the Record tab's Punch in / the `r` action's job.
    if (engine() === "Playing" || engine() === "Recording") setEngine(await engine_stop({}));
    else setEngine(await engine_play({}));
  }

  // Global `record.punch` action (`r`, palette, native menu): show the
  // Record tab and toggle the pass there (the panel owns armed tracks).
  const [punchNonce, setPunchNonce] = createSignal(0);
  // Same nonce pattern for the session/comp actions: the panels own jam,
  // selection, and picks; keys just bump and show the tab.
  const [launchNonce, setLaunchNonce] = createSignal(0);
  const [jamNonce, setJamNonce] = createSignal(0);
  const [commitNonce, setCommitNonce] = createSignal(0);
  const [grooveNonce, setGrooveNonce] = createSignal(0);
  const [mergeNonce, setMergeNonce] = createSignal(0);
  const [linked, setLinked] = createSignal(false);

  async function stop() {
    setEngine(await engine_stop({}));
  }

  async function undo() {
    try {
      await op_undo({});
    } catch (e) {
      setError(`undo failed: ${String(e)}`);
      return;
    }
    await refresh();
  }

  async function redo() {
    try {
      await op_redo({});
    } catch (e) {
      setError(`redo failed: ${String(e)}`);
      return;
    }
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
    setEngine("Stopped");
    setError(null);
  }

  async function newProject() {
    const name = projectName().trim() || "Untitled";
    setProject(await project_new({ name }));
    setEngine("Stopped");
    setError(null);
  }

  async function patchTrack(
    trackId: string,
    field: "volume" | "pan" | "muted" | "solo",
    value: number | boolean,
  ) {
    try {
      await op_apply({
        op: {
          seq: 0,
          actor: "ui",
          kind: "ParamSet",
          target: `${trackId}:${field}`,
          value_json: JSON.stringify(value),
        },
      });
      await refresh();
    } catch (e) {
      setError(`mixer edit refused: ${String(e)}`);
    }
  }

  async function handlePick(item: LibraryItem): Promise<string | void> {
    if (item.kind === "Preset") {
      const preset = findNativePreset(item.id);
      if (!preset) return "Demo entry — not a loadable preset.";
      const device = project()?.devices.find((d) => deviceClassOf(d) === preset.deviceClass);
      if (!device) return `No ${preset.deviceClass} device in this project.`;
      try {
        for (const op of presetParamOps("ui", device.id, preset.id)) {
          await op_apply({ op });
        }
      } catch (e) {
        return `preset refused: ${e instanceof Error ? e.message : String(e)}`;
      }
      await refresh();
      return `Applied ${preset.name} to ${device.name}.`;
    }
    if (item.kind === "Sample")
      return "Demo samples carry no audio — import a WAV on the Timeline to hear samples.";
    if (item.kind === "Plugin") return "Plugin hosting isn't wired yet.";
    if (item.kind === "Project") return "Project templates aren't loadable yet.";
  }

  async function runAction(id: string) {
    const a = findAction(id);
    if (!a) return;
    if (id === "transport.play") {
      // Space / palette Play toggles like every other DAW; the engine
      // signal updates inside togglePlay.
      await togglePlay();
    } else if (id === "project.save") {
      // Ctrl+S / menu Save behave exactly like the Save button: the typed
      // path, or an honest error when there is none. Invoking project_save
      // with no path would only reject inside the bridge. Return before
      // the shared refresh: save() already refreshes on success, and a
      // refresh would wipe the no-path error.
      await save();
      setPaletteOpen(false);
      return;
    } else if (id === "project.open") {
      // Menu Open behaves like the Open button (same typed-path rule).
      await open();
      setPaletteOpen(false);
      return;
    } else if (id === "track.add") {
      // Menu / palette Add Track with the standard auto-name (the bridge
      // needs a name and a key cannot supply one).
      try {
        await track_add({ name: `Track ${(project()?.tracks.length ?? 0) + 1}` });
      } catch (e) {
        setError(`add track failed: ${e instanceof Error ? e.message : String(e)}`);
        setPaletteOpen(false);
        return;
      }
    } else {
      try {
        const result = await a.run({});
        // Other transport actions resolve to the new engine state: mirror
        // it into the badge so the header never lies.
        if (result === "Playing" || result === "Stopped" || result === "Recording") {
          setEngine(result);
        }
      } catch (e) {
        // Keys, palette, and menu share this path: an IPC action invoked
        // without its arguments consults the guidance map first (same copy
        // the palette shows inline), so bridge rejections never reach the
        // user raw and nothing rejects into the console.
        const hint = LOCAL_GUIDANCE[id];
        setError(
          hint ?? `action failed: ${id}: ${e instanceof Error ? e.message : String(e)}`,
        );
        setPaletteOpen(false);
        return;
      }
    }
    setPaletteOpen(false);
    if (!LOCAL_GUIDANCE[id]) await refresh();
  }

  onMount(() => {
    // Engine badge follows the backend even when transport changes come
    // from elsewhere (Space key, palette, MIDI). No bridge (plain
    // browser) means no events: the rejection is swallowed like refresh().
    const unlistens: Array<() => void> = [];
    listen<EngineState>("engine_state_changed", (e) => {
      const s = e.payload;
      if (s === "Playing" || s === "Stopped" || s === "Recording") setEngine(s);
    })
      .then((u) => {
        unlistens.push(u);
      })
      .catch(() => {});
    // Native menu items dispatch registry ids through `menu-action`
    // (see src-tauri/src/menu.rs): run them like palette actions.
    listen<{ action: string }>("menu-action", (e) => {
      const id = e.payload?.action;
      if (typeof id === "string") void runAction(id);
    })
      .then((u) => {
        unlistens.push(u);
      })
      .catch(() => {});
    onCleanup(() => {
      for (const u of unlistens) u();
    });
    registerLocalActionHandler("palette.open", () => setPaletteOpen(true));
    // `g`-chord view focus (Track K): switch to the tab (the mixer is an
    // always-visible side column, so it has no tab), move DOM focus there,
    // and sync the vim key context so view-scoped keys work on arrival.
    const focusView = (id: string) => {
      const tab = tabForFocusAction(id);
      if (tab) setWorkspaceTab(tab);
      setVimContext(vim, id === "view.focusMixer" ? "mixer" : contextForTab(tab ?? "Timeline"));
      queueMicrotask(() => {
        const el = document.getElementById(elementIdForFocusAction(id) ?? "");
        el?.focus({ preventScroll: false });
        el?.scrollIntoView({ block: "nearest" });
      });
    };
    for (const id of [
      "view.focusArrangement",
      "view.focusPianoRoll",
      "view.focusMixer",
      "view.focusSession",
    ]) {
      registerLocalActionHandler(id, () => focusView(id));
    }
    // `gc` / `gv`: jump to the named timeline section (same sidecar doc the
    // Timeline renders) by moving the vim cursor to its first beat.
    const gotoSection = (id: string) => {
      const name = sectionNameForGotoAction(id);
      const section = name ? findSectionByName(sampleTimelineDoc().sections, name) : undefined;
      if (!section) return;
      setWorkspaceTab("Timeline");
      const step = Math.max(0, Math.min(vim.grid.steps - 1, Math.round(section.start_beats)));
      vim.cursor = { track: vim.cursor.track, step };
      setCursor(`${vim.cursor.track}:${step}`);
      queueMicrotask(() => {
        document.getElementById("timeline-view")?.focus({ preventScroll: false });
      });
    };
    registerLocalActionHandler("section.goto.chorus", () => gotoSection("section.goto.chorus"));
    registerLocalActionHandler("section.goto.verse", () => gotoSection("section.goto.verse"));
    // Mixer `m` / `M`: toggle mute/solo on the strip under the vim cursor
    // through the same ParamSet op path as the mixer buttons (undoable).
    const toggleStrip = (field: "muted" | "solo") => {
      const tracks = project()?.tracks ?? [];
      if (tracks.length === 0) return;
      const t = tracks[Math.max(0, Math.min(tracks.length - 1, vim.cursor.track))];
      void patchTrack(t.id, field, field === "muted" ? !t.muted : !t.solo);
    };
    registerLocalActionHandler("mixer.muteSelected", () => toggleStrip("muted"));
    registerLocalActionHandler("mixer.soloSelected", () => toggleStrip("solo"));
    registerLocalActionHandler("record.punch", () => {
      setWorkspaceTab("Record");
      setPunchNonce((n) => n + 1);
    });
    registerLocalActionHandler("session.launch", () => {
      setWorkspaceTab("Session");
      setLaunchNonce((n) => n + 1);
    });
    registerLocalActionHandler("session.jam_record", () => {
      setWorkspaceTab("Session");
      setJamNonce((n) => n + 1);
    });
    registerLocalActionHandler("comp.commit", () => {
      setWorkspaceTab("Comp");
      setCommitNonce((n) => n + 1);
    });
    registerLocalActionHandler("groove.apply", () => {
      setWorkspaceTab("Groove");
      setGrooveNonce((n) => n + 1);
    });
    // Branches live in an always-visible side panel: no tab switch needed.
    registerLocalActionHandler("branch.merge", () => {
      setMergeNonce((n) => n + 1);
    });
    registerLocalActionHandler("view.fullscreen", async () => {
      try {
        const { getCurrentWindow } = await import("@tauri-apps/api/window");
        const win = getCurrentWindow();
        await win.setFullscreen(!(await win.isFullscreen()));
      } catch (e) {
        setError(`fullscreen needs the app window: ${e instanceof Error ? e.message : String(e)}`);
      }
    });
    registerLocalActionHandler("app.quit", async () => {
      try {
        const { getCurrentWindow } = await import("@tauri-apps/api/window");
        await getCurrentWindow().close();
      } catch (e) {
        setError(`quit needs the app window: ${e instanceof Error ? e.message : String(e)}`);
      }
    });
    registerLocalActionHandler("link.join", async () => {
      try {
        setLinked(await link_toggle({}));
      } catch (e) {
        setError(`link toggle failed: ${String(e)}`);
      }
    });
    registerLocalActionHandler("help.show_shortcuts", () => setShortcutsOpen(true));
    registerLocalActionHandler("vim.mode.normal", () => setVimMode("normal"));
    registerLocalActionHandler("vim.mode.insert", () => setVimMode("insert"));
    registerLocalActionHandler("vim.mode.visual", () => setVimMode("visual"));
    // Guidance actions (`LOCAL_GUIDANCE`): surface the text on every path.
    // The trailing refresh is skipped for these — it would wipe the error.
    for (const [gid, text] of Object.entries(LOCAL_GUIDANCE)) {
      registerLocalActionHandler(gid, () => setError(text));
    }

    const onKey = (e: KeyboardEvent) => {
      if (paletteOpen()) return; // palette handles its own keys
      const t = e.target as HTMLElement | null;
      if (t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA")) {
        if (e.key !== "Escape") return;
      }
      const out = handleKey(vim, eventToToken(e), keymap(), {
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

  // Live transport clock: poll the engine position while playing. The
  // interval dies with the component and with every stop.
  const [position, setPosition] = createSignal(0);
  let poll: number | undefined;
  onCleanup(() => window.clearInterval(poll));
  createEffect(() => {
    window.clearInterval(poll);
    poll = undefined;
    if (engine() === "Playing" || engine() === "Recording") {
      poll = window.setInterval(() => {
        engine_position({})
          .then((b) => {
            if (Number.isFinite(b)) setPosition(b);
          })
          .catch(() => {});
      }, 250);
    }
  });

  return (
    <main class="daw-shell">
      <header class="transport-bar">
        <div class="tb-group" role="group" aria-label="Transport">
          <button
            class="transport-play"
            classList={{ playing: playing() }}
            onClick={togglePlay}
          >
            {playing() ? "❚❚" : "▶"}
          </button>
          <button onClick={stop} title="Stop">■</button>
          <span class={`engine-badge${playing() ? " playing" : ""}`}>
            {engine()}
          </span>
          <Show when={linked()}>
            <span class="engine-badge playing" data-testid="link-badge" title="Joined to the Link session clock">
              LINK
            </span>
          </Show>
          <span class="transport-readout daw-numeric" title="Transport position (beats)">
            {position().toFixed(2)}
          </span>
        </div>
        <div class="tb-group" role="group" aria-label="Edit">
          <button data-testid="undo-btn" onClick={() => void undo()} title="Undo (op_undo)">↩</button>
          <button data-testid="redo-btn" onClick={() => void redo()} title="Redo (op_redo)">↪</button>
          <span class={`vim-badge ${vimMode()}`}>
            {vimMode().toUpperCase()}
          </span>
          <span class="transport-readout daw-numeric">{cursor()}</span>
        </div>
        <div class="tb-group" role="group" aria-label="Tempo">
          <input
            data-testid="tempo-input"
            class="tb-number daw-numeric"
            type="number"
            min={1}
            aria-label="Tempo BPM"
            value={tempoDraft()}
            onInput={(e) => setTempoDraft(e.currentTarget.value)}
          />
          <span class="transport-readout">BPM</span>
          <button data-testid="tempo-set" onClick={() => void applyTempo()}>
            Set
          </button>
        </div>
        <button class="tb-palette" onClick={() => setPaletteOpen((v) => !v)} title="Command palette (Ctrl+K)">
          ⌘K
        </button>
        <span class="transport-spacer" />
        <Show when={project()}>
          <span class="transport-readout daw-numeric tb-project">
            {project()?.name} @ {project()?.tempo} BPM
          </span>
        </Show>
        <div class="tb-group" role="group" aria-label="Project">
          <input
            data-testid="project-name"
            class="tb-text"
            placeholder="name"
            aria-label="New project name"
            value={projectName()}
            onInput={(e) => setProjectName(e.currentTarget.value)}
          />
          <button data-testid="project-new" onClick={() => void newProject()}>
            New
          </button>
          <input
            data-testid="project-path"
            class="tb-path"
            placeholder="/path/to/project"
            aria-label="Project file path"
            value={savePath()}
            onInput={(e) => setSavePath(e.currentTarget.value)}
          />
          <button data-testid="project-open" onClick={() => void open()}>
            Open
          </button>
          <button data-testid="project-save" onClick={() => void save()}>
            Save
          </button>
        </div>
        <Show when={error()}>
          <span data-testid="transport-error" class="transport-error">{error()}</span>
        </Show>
      </header>
      <Palette open={paletteOpen()} onClose={() => setPaletteOpen(false)} onRan={() => void refresh()} />
      <ShortcutEditor
        open={shortcutsOpen()}
        onClose={() => setShortcutsOpen(false)}
        onKeymapChange={setKeymap}
      />
      <div class="daw-grid">
        <div class="daw-rail">
          <Panel title="Browser">
            <BrowserPalette onPick={handlePick} />
          </Panel>
        </div>
        <div class="daw-workspace">
          <nav class="workspace-tabs" aria-label="Workspace">
            <For each={["Timeline", "Session", "Piano roll", "Score", "Automation", "Groove", "Record", "Comp"]}>
              {(tab) => (
                <button
                  data-testid={`tab-${tab}`}
                  classList={{ active: workspaceTab() === tab }}
                  aria-current={workspaceTab() === tab ? "page" : undefined}
                  onClick={() => setWorkspaceTab(tab)}
                >
                  {tab}
                </button>
              )}
            </For>
          </nav>
          <div class="workspace-view" onFocusIn={() => setVimContext(vim, contextForTab(workspaceTab()))}>
            <Show when={workspaceTab() === "Timeline"}>
              <Panel title="Timeline">
                <TimelineView project={project()} onChanged={() => void refresh()} />
              </Panel>
            </Show>
            <Show when={workspaceTab() === "Session"}>
              <Panel title="Session">
                <SessionView
                  project={project()}
                  launchNonce={launchNonce()}
                  jamNonce={jamNonce()}
                />
              </Panel>
            </Show>
            <Show when={workspaceTab() === "Piano roll"}>
              <Panel title="Piano roll">
                <PianoTab project={project()} />
              </Panel>
            </Show>
            <Show when={workspaceTab() === "Score"}>
              <Panel title="Score">
                <ScorePanel project={project()} />
              </Panel>
            </Show>
            <Show when={workspaceTab() === "Automation"}>
              <Panel title="Automation">
                <AutomationView project={project()} />
              </Panel>
            </Show>
            <Show when={workspaceTab() === "Groove"}>
              <Panel title="Groove">
                <GroovePanel source={null} target={null} applyNonce={grooveNonce()} />
              </Panel>
            </Show>
            <Show when={workspaceTab() === "Record"}>
              <Panel title="Record">
                <RecordPanel
                  project={project()}
                  engineState={engine()}
                  onTransport={setEngine}
                  punchNonce={punchNonce()}
                />
              </Panel>
            </Show>
            <Show when={workspaceTab() === "Comp"}>
              <Panel title="Comp">
                <CompPanel project={project()} commitNonce={commitNonce()} />
              </Panel>
            </Show>
          </div>
        </div>
        <div class="daw-column">
          <Panel title="Mixer">
            <div onFocusIn={() => setVimContext(vim, "mixer")}>
            <Show when={project()} fallback={<div class="session-dim">No project open.</div>}>
              {(p) => <Mixer project={p()} onPatch={(id, field, value) => void patchTrack(id, field, value)} />}
            </Show>
            </div>
          </Panel>
          <Panel title="Branches">
            <BranchPanel project={project()} mergeNonce={mergeNonce()} />
          </Panel>
          <Panel title="Devices">
            <DevicesPanel project={project()} />
          </Panel>
          <Panel title="Export">
            <ExportPanel project={project()} />
          </Panel>
        </div>
      </div>
    </main>
  );
}
