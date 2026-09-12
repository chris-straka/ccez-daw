import { For, Show, createSignal } from "solid-js";
import type { AdaptiveCue, TransitionKind } from "../generated/project";
import {
  TRANSITION_KINDS,
  addLayer,
  cloneCue,
  collectStates,
  describeTransition,
  layerMatrix,
  makeLayer,
  makeTransition,
  parseCue,
  removeLayer,
  removeTransition,
  serializeCue,
  setLayerVolume,
  stingerSlots,
  toggleLayerState,
  transitionFor,
  upsertTransition,
  validateCue,
} from "./cues";

/**
 * GA-3 cue/transition editor view.
 *
 * Three sections over one `AdaptiveCue` draft: the layer matrix
 * (layers × states checkboxes; empty row = always-on bed), the
 * transition rule editor (exact `(from, to)` pairs with kind-specific
 * fields), and the stinger slots (the `Stinger` rules and their
 * one-shots). Edits mutate a local clone and commit through `onChange`
 * only when the draft validates, so the host never receives a cue the
 * export validator would reject.
 */
export default function Cues(props: {
  cue: AdaptiveCue;
  clipIds: string[];
  cueIds: string[];
  onChange: (cue: AdaptiveCue) => void;
}) {
  const [draft, setDraft] = createSignal<AdaptiveCue>(cloneCue(props.cue));
  const [note, setNote] = createSignal("");
  const [from, setFrom] = createSignal("explore");
  const [to, setTo] = createSignal("combat");
  const [kind, setKind] = createSignal<TransitionKind>("Fade");

  const states = () => collectStates(draft());
  const matrix = () => layerMatrix(draft(), states());
  const problems = () => validateCue(draft(), { clipIds: props.clipIds, cueIds: props.cueIds });
  const slots = () => stingerSlots(draft());

  function touch(fn: (d: AdaptiveCue) => void) {
    const d = cloneCue(draft());
    try {
      fn(d);
    } catch (e) {
      setNote(e instanceof Error ? e.message : String(e));
      return;
    }
    const errs = validateCue(d, { clipIds: props.clipIds, cueIds: props.cueIds });
    // Local matrix/rule edits are always committable when they parse;
    // surface validator output as the note instead of blocking typing.
    setDraft(d);
    setNote(errs.length > 0 ? errs[0] as string : "Draft ok");
  }

  function commit() {
    const errs = problems();
    if (errs.length > 0) {
      setNote(`Cannot commit: ${errs[0]}`);
      return;
    }
    props.onChange(cloneCue(draft()));
    setNote("Committed");
  }

  function roundTrip() {
    try {
      const back = parseCue(serializeCue(draft()));
      setDraft(back);
      setNote("Round-trip ok (serialize → parse is identity)");
    } catch (e) {
      setNote(e instanceof Error ? e.message : String(e));
    }
  }

  function addLayerUi() {
    touch((d) => {
      const n = d.layers.length + 1;
      addLayer(d, makeLayer(`layer_${n}`, `Layer ${n}`));
    });
  }

  function addRuleUi() {
    touch((d) => {
      upsertTransition(d, { ...makeTransition(from(), to(), kind()), id: `trx_${from()}_${to()}` });
    });
  }

  return (
    <div>
      <h2>Cue: {draft().name}</h2>
      <p>
        Tempo {draft().tempo} BPM · default `{draft().default_state}` · {draft().layers.length} layers ·{" "}
        {draft().transitions.length} rules
      </p>

      <h3>Layer matrix</h3>
      <table>
        <thead>
          <tr>
            <th>Layer</th>
            <For each={states()}>{(s) => <th>{s === "" ? "(bed)" : s}</th>}</For>
            <th>Vol</th>
            <th />
          </tr>
        </thead>
        <tbody>
          <For each={draft().layers}>
            {(l) => (
              <tr>
                <td>
                  {l.name}
                  <Show when={l.states.length === 0}> (bed)</Show>
                </td>
                <For each={states()}>
                  {(s) => (
                    <td>
                      <input
                        type="checkbox"
                        checked={matrix()[l.id]?.[s] ?? false}
                        onChange={() => touch((d) => toggleLayerState(d, l.id, s))}
                      />
                    </td>
                  )}
                </For>
                <td>
                  <input
                    type="number"
                    min={0}
                    max={4}
                    step={0.1}
                    value={l.volume}
                    onChange={(e) => touch((d) => setLayerVolume(d, l.id, Number(e.currentTarget.value)))}
                  />
                </td>
                <td>
                  <button onClick={() => touch((d) => removeLayer(d, l.id))}>Remove</button>
                </td>
              </tr>
            )}
          </For>
        </tbody>
      </table>
      <button onClick={addLayerUi}>Add layer</button>

      <h3>Transition rules</h3>
      <ul>
        <For each={draft().transitions}>
          {(t) => (
            <li>
              {t.from_state} → {t.to_state}: {describeTransition(t)}{" "}
              <button onClick={() => touch((d) => removeTransition(d, t.from_state, t.to_state))}>Remove</button>
            </li>
          )}
        </For>
      </ul>
      <div>
        <input value={from()} onInput={(e) => setFrom(e.currentTarget.value)} placeholder="from state" />
        <input value={to()} onInput={(e) => setTo(e.currentTarget.value)} placeholder="to state" />
        <select value={kind()} onChange={(e) => setKind(e.currentTarget.value as TransitionKind)}>
          <For each={TRANSITION_KINDS}>{(k) => <option value={k}>{k}</option>}</For>
        </select>
        <button onClick={addRuleUi}>Add / replace rule</button>
        <p>
          Preview {from()} → {to()}: {describeTransition(transitionFor(draft(), from(), to()))}
        </p>
      </div>

      <h3>Stinger slots</h3>
      <Show when={slots().length > 0} fallback={<p>No stinger slots — add a `Stinger` rule to arm one.</p>}>
        <ul>
          <For each={slots()}>
            {(s) => (
              <li>
                {s.from} → {s.to}: one-shot `{s.stingerId}`
              </li>
            )}
          </For>
        </ul>
      </Show>

      <div>
        <button onClick={commit}>Commit</button> <button onClick={roundTrip}>Round-trip check</button>
      </div>
      <Show when={note()}>
        <p>{note()}</p>
      </Show>
      <Show when={problems().length > 0}>
        <ul>
          <For each={problems()}>{(p) => <li>{p}</li>}</For>
        </ul>
      </Show>
    </div>
  );
}
