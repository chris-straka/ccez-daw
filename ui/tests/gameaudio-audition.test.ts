import { describe, expect, test } from "bun:test";
import {
  addTimelineEntry,
  advanceTransport,
  beatsToSamples,
  beatsToSeconds,
  effectiveKind,
  fireEvent,
  isDegraded,
  layersForState,
  makeFireRuntime,
  makeTransport,
  normalizeParam,
  previewGains,
  resolveRtpc,
  secondsToBeats,
  snapshotAt,
  transitionFor,
  valueFor,
} from "../src/gameaudio/model";
import { defaultSnapshot, sampleBank, sampleCue, sampleParams } from "../src/gameaudio/sample";
import type { GameStateSnapshot } from "../src/generated/project";

function snap(state: string, values: Array<[string, number]> = []): GameStateSnapshot {
  return { state, values: values.map(([param, value]) => ({ param, value })) };
}

describe("game-state audition simulator", () => {
  test("scripted state change: explore -> combat fades, back BarWaits, unknown cuts", () => {
    const cue = sampleCue();
    // Audible sets mirror the contract: empty `states` = always-on bed.
    expect(layersForState(cue, "explore").map((l) => l.id)).toEqual(["bed"]);
    expect(layersForState(cue, "combat").map((l) => l.id)).toEqual(["bed", "drums", "brass"]);

    const fade = transitionFor(cue, "explore", "combat");
    expect(fade?.kind).toBe("Fade");
    expect(fade?.fade_beats).toBe(4);
    expect(effectiveKind(cue, "explore", "combat")).toBe("Fade");
    expect(isDegraded("Fade")).toBe(false);

    // combat -> explore is BarWait: the simulator honestly plays a cut.
    expect(effectiveKind(cue, "combat", "explore")).toBe("BarWait");
    expect(isDegraded("BarWait")).toBe(true);

    // No rule for menu traffic: the contract Cut fallback.
    expect(transitionFor(cue, "explore", "menu")).toBeUndefined();
    expect(effectiveKind(cue, "explore", "menu")).toBe("Cut");
  });

  test("preview ramp: outgoing falls, incoming rises, bed holds", () => {
    const cue = sampleCue();
    const at = previewGains(cue, "explore", "combat", 10, 4, 10);
    expect(at["bed"]).toBe(1);
    expect(at["drums"]).toBe(0);
    const mid = previewGains(cue, "explore", "combat", 10, 4, 12);
    expect(mid["drums"]).toBeCloseTo(0.5, 9);
    expect(mid["bed"]).toBe(1);
    const settled = previewGains(cue, "explore", "combat", 10, 4, 14);
    expect(settled["drums"]).toBe(1);
    expect(settled["brass"]).toBe(1);
  });

  test("live param sliders: clamp, default, RTPC map, unknown ignored", () => {
    const [threat, , health] = sampleParams();
    expect(normalizeParam(threat, 1.5)).toBe(1);
    expect(normalizeParam(threat, -0.5)).toBe(0);
    expect(normalizeParam(threat, 0.25)).toBeCloseTo(0.25, 12);
    // Declared-but-missing reads as the default (health 100).
    expect(valueFor(health, snap("explore"))).toBe(100);
    expect(valueFor(threat, snap("combat", [["threat", 0.8]]))).toBe(0.8);

    const bank = sampleBank();
    const footstep = bank.events.find((e) => e.id === "player.footstep")!;
    const lo = resolveRtpc(footstep.rtpc, snap("combat", [["threat", 0]]), sampleParams());
    const hi = resolveRtpc(footstep.rtpc, snap("combat", [["threat", 1]]), sampleParams());
    expect(lo[0].value).toBeCloseTo(0.5, 12);
    expect(hi[0].value).toBeCloseTo(1.0, 12);
    expect(lo[0].normalized).toBeCloseTo(0, 12);
    expect(hi[0].normalized).toBeCloseTo(1, 12);
    // Unknown game params are skipped quietly, never an error.
    const mixed = resolveRtpc(
      [...footstep.rtpc, { param: "future_param", target_node: "bus", target_param: "vol", min: 0, max: 1 }],
      snap("combat", [["threat", 1]]),
      sampleParams(),
    );
    expect(mixed).toHaveLength(1);
  });

  test("transport sync: play advances in beats, pause holds, units round-trip", () => {
    const t = { ...makeTransport(120), playing: true };
    expect(advanceTransport(t, 1).beat).toBeCloseTo(2, 9); // 120 BPM: 1 s = 2 beats
    expect(advanceTransport({ ...t, playing: false }, 10).beat).toBe(0);
    expect(beatsToSeconds(4, 120)).toBeCloseTo(2, 12);
    expect(secondsToBeats(2, 120)).toBeCloseTo(4, 12);
    expect(beatsToSamples(4, 120, 48000)).toBe(96000);
    expect(beatsToSamples(0, 120, 48000)).toBe(0);
  });

  test("event fire buttons: accept, cooldown, oldest-steal, rejects", () => {
    const bank = sampleBank();
    const declared = sampleParams();
    const s = snap("combat", [["threat", 0.5]]);
    // Same seed -> identical voice sequence (reproducible audition).
    const a = makeFireRuntime(7);
    const b = makeFireRuntime(7);
    for (let t = 0; t < 5; t++) {
      const va = fireEvent(bank, a, "ui.click", s, declared, t * 100, t);
      const vb = fireEvent(bank, b, "ui.click", s, declared, t * 100, t);
      expect(va).toEqual(vb);
      expect(va.ok).toBe(true);
    }
    // Cooldown (footstep 90 ms): a burst inside the window drops.
    const rt = makeFireRuntime(3);
    expect(fireEvent(bank, rt, "player.footstep", s, declared, 0, 0).ok).toBe(true);
    const dropped = fireEvent(bank, rt, "player.footstep", s, declared, 50, 0.5);
    expect(dropped.ok).toBe(false);
    if (!dropped.ok) expect(dropped.kind).toBe("Cooldown");
    expect(fireEvent(bank, rt, "player.footstep", s, declared, 90, 1).ok).toBe(true);
    // Polyphony 2: the third voice steals the oldest, never stacks.
    fireEvent(bank, rt, "player.footstep", s, declared, 180, 2);
    const voices = rt.active.filter((v) => v.event_id === "player.footstep");
    expect(voices).toHaveLength(2);
    expect(voices.every((v) => v.started_ms >= 90)).toBe(true);
    // Humanization stays inside the declared spreads.
    for (const v of rt.active) {
      expect(Math.abs(v.gain - 0.8)).toBeLessThanOrEqual(0.1 + 1e-9);
      expect(Math.abs(v.pitch_semitones)).toBeLessThanOrEqual(2 + 1e-9);
    }
    // Unknown events and empty pools reject loudly.
    const unknown = fireEvent(bank, makeFireRuntime(1), "nope", s, declared, 0, 0);
    expect(unknown.ok).toBe(false);
    const empty = fireEvent(
      { ...bank, events: [{ ...bank.events[0], id: "ev_empty", clip_ids: [] }] },
      makeFireRuntime(1),
      "ev_empty",
      s,
      declared,
      0,
      0,
    );
    expect(empty.ok).toBe(false);
    if (!empty.ok) expect(empty.kind).toBe("EmptyPool");
    // RTPC rides along on the voice.
    const first = fireEvent(bank, makeFireRuntime(9), "player.footstep", s, declared, 1000, 8);
    expect(first.ok).toBe(true);
    if (first.ok) expect(first.voice.rtpc).toHaveLength(1);
  });

  test("state timeline: pins stay sorted, clock reads the latest pin", () => {
    const d = defaultSnapshot();
    let entries = addTimelineEntry([], { id: "b", beat: 8, snapshot: snap("combat") });
    entries = addTimelineEntry(entries, { id: "a", beat: 4, snapshot: snap("explore") });
    expect(entries.map((e) => e.id)).toEqual(["a", "b"]);
    expect(snapshotAt(entries, d, 0).state).toBe("explore"); // default before first pin
    expect(snapshotAt(entries, d, 4).state).toBe("explore");
    expect(snapshotAt(entries, d, 9).state).toBe("combat");
  });
});
