import { describe, expect, test } from "bun:test";
import { compileNlCommand, LocalNlProvider, NlOpStore } from "../src/nl";
import { NlParseError, parseNlCommand } from "../src/nl";

describe("Track L: NL command adds clip + undoes it", () => {
  test("NL add-clip compiles to an ordinary ClipAdded op, applies, undoes, redoes", async () => {
    const store = new NlOpStore([{ id: "trk_music", name: "Music" }]);
    const plan = await compileNlCommand(
      "add a midi clip called Solo on track trk_music at beat 8 length 4",
      new LocalNlProvider(),
    );

    // NL output is ordinary ops: one ClipAdded with a Clip JSON payload.
    expect(plan.ops).toHaveLength(1);
    expect(plan.ops[0].kind).toBe("ClipAdded");
    const clip = JSON.parse(plan.ops[0].valueJson);
    expect(clip).toMatchObject({
      track_id: "trk_music",
      name: "Solo",
      start_beats: 8,
      length_beats: 4,
      kind: "Midi",
    });

    // Apply as an AI sidecar actor (sidecar pattern: undoable by design).
    const seqs = store.applyPlan("ai:jam", plan);
    expect(seqs).toEqual([1]);
    expect(store.clips()).toHaveLength(1);
    expect(store.clips()[0].name).toBe("Solo");

    // Undo removes it via an UndoMarker (history survives, redo works).
    const undone = store.undo();
    expect(undone).toBe(1);
    expect(store.clips()).toHaveLength(0);
    expect(store.opKinds()).toContain("UndoMarker");

    store.redo();
    expect(store.clips()).toHaveLength(1);
    expect(store.clips()[0].name).toBe("Solo");
  });

  test("parser covers tempo / param / move, rejects gibberish", () => {
    expect(parseNlCommand("set tempo to 128").ops[0].kind).toBe("TempoSet");
    expect(
      parseNlCommand("set trk_music:volume to 0.5").ops[0],
    ).toMatchObject({ kind: "ParamSet", target: "trk_music:volume" });
    expect(
      parseNlCommand("move clip clip_x to beat 16").ops[0].kind,
    ).toBe("ClipMoved");
    expect(() =>
      parseNlCommand("florp the wobble zeebly"),
    ).toThrow(NlParseError);
  });

  test("unknown track warns, bad actor rejected", async () => {
    const plan = await compileNlCommand("add clip called X at beat 0", undefined, {
      knownTrackIds: ["trk_music"],
      defaultTrackId: "trk_nope",
    });
    expect(plan.warnings.length).toBeGreaterThan(0);
    const store = new NlOpStore();
    expect(() => store.applyPlan("ai:jam", plan)).toThrow(/unknown op target/);
    expect(() =>
      store.applyPlan("human", { ops: plan.ops }),
    ).toThrow(/bad actor/);
  });
});
