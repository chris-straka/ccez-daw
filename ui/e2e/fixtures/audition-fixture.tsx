import { render } from "solid-js/web";
import Audition from "../../src/gameaudio/Audition";

/**
 * Isolated Audition (game-audio simulator) mount for the state-change smoke
 * spec. Uses the built-in demo cue/bank/params from `gameaudio/sample.ts`,
 * so no IPC mock is needed — the simulator is pure UI-local state.
 */
function Fixture() {
  return (
    <div style={{ width: "720px", padding: "16px", background: "#1a1a1a", color: "#eee" }}>
      <Audition />
    </div>
  );
}

const root = document.getElementById("root");
if (!root) throw new Error("missing #root element");
render(() => <Fixture />, root);
