import { describe, expect, test } from "bun:test";
import { InMemoryBackend } from "../src/backend";

const clip = (name: string, startBeats = 0) => ({
  trackId: "trk_1",
  name,
  startBeats,
  lengthBeats: 4,
  kind: "Midi" as const,
});

describe("InMemoryBackend undo mirrors core Engine::undo", () => {
  test("undo twice walks back two ops, skipping ones already undone", () => {
    const b = new InMemoryBackend();
    const one = b.addClip(clip("One"));
    const two = b.addClip(clip("Two", 4));
    expect(b.undo()).toEqual({ undoneSeq: two.seq });
    expect(b.undo()).toEqual({ undoneSeq: one.seq });
    expect(b.getProject().clips).toEqual([]);
    expect(b.getProject().tracks[0]!.clip_ids).toEqual([]);
    expect(() => b.undo()).toThrow("nothing to undo");
  });

  test("undoing AutomationPointSet restores the lane's earlier points", () => {
    const b = new InMemoryBackend();
    b.setAutomationPoint("vol", 0, 0.2, "trk_1", "volume");
    b.setAutomationPoint("vol", 4, 0.9);
    b.setAutomationPoint("vol", 4, 0.5);
    b.undo();
    expect(b.listAutomationLanes()[0]!.points).toEqual([
      { beat: 0, value: 0.2 },
      { beat: 4, value: 0.9 },
    ]);
    b.undo();
    expect(b.listAutomationLanes()[0]!.points).toEqual([{ beat: 0, value: 0.2 }]);
    // Undoing the op that created the lane removes the lane, as core rebuilds it.
    b.undo();
    expect(b.listAutomationLanes()).toEqual([]);
  });
});

describe("automation upsert mirrors core apply_automation_point", () => {
  test("an exact-beat point is replaced; points stay ascending", () => {
    const b = new InMemoryBackend();
    b.setAutomationPoint("vol", 4, 0.5, "trk_1", "volume");
    b.setAutomationPoint("vol", 0, 0.1);
    b.setAutomationPoint("vol", 4, 0.8);
    b.setAutomationPoint("vol", 2, 0.3);
    expect(b.listAutomationLanes()[0]!.points).toEqual([
      { beat: 0, value: 0.1 },
      { beat: 2, value: 0.3 },
      { beat: 4, value: 0.8 },
    ]);
  });
});

describe("batches are validated whole, then appended atomically", () => {
  test("jamRecord: a bad clip mid-batch writes nothing", () => {
    const b = new InMemoryBackend();
    expect(() => b.jamRecord([clip("Good"), { ...clip("Bad"), trackId: "nope" }])).toThrow("unknown track");
    expect(b.getOpLog()).toEqual([]);
    expect(b.getProject().clips).toEqual([]);
  });

  test("mergeBranch: a bad op mid-batch (bad kind or bad JSON) writes nothing", () => {
    const b = new InMemoryBackend();
    const good = { kind: "ParamSet" as const, target: "trk_1:pan", value_json: "0.5" };
    expect(() =>
      b.mergeBranch("feature", "main", [good, { kind: "ParamSet", target: "trk_1:volume", value_json: "{not json" }]),
    ).toThrow("value_json");
    expect(() => b.mergeBranch("feature", "main", [good, { kind: "Bogus" as any, target: "x", value_json: "1" }])).toThrow(
      "bad op kind",
    );
    expect(b.getOpLog()).toEqual([]);
    // Same-target ops inside one batch still resolve in order.
    const out = b.mergeBranch("feature", "main", [good, good, { ...good, value_json: "0.1" }]);
    expect(out.merged).toHaveLength(1);
    expect(out.conflicts).toEqual(["trk_1:pan"]);
  });
});

describe("groove_apply ids stay unique", () => {
  test("re-applying the same groove to the same clip is rejected like core's DuplicateTarget", () => {
    const b = new InMemoryBackend();
    const src = b.addClip(clip("Beat"));
    const first = b.applyGroove(src.clipId, "swing", 0.5);
    const before = b.getOpLog().length;
    expect(() => b.applyGroove(src.clipId, "swing", 0.5)).toThrow("duplicate clip id");
    expect(b.getOpLog()).toHaveLength(before);
    expect(b.getProject().clips.filter((c) => c.id === first.clipId)).toHaveLength(1);
    // Once the grooved copy is undone the id is free again.
    b.undo();
    expect(b.applyGroove(src.clipId, "swing", 0.5).clipId).toBe(first.clipId);
  });
});

describe("link_join validates before creating the session", () => {
  test("a bad tempo leaves no session behind", () => {
    const b = new InMemoryBackend();
    expect(() => b.joinLink("jam", "laptop", 0)).toThrow("tempo");
    // Session state is private (no tool lists sessions); peek to prove no empty ghost.
    expect((b as any).linkSessions.has("jam")).toBe(false);
    expect(b.joinLink("jam", "phone")).toEqual({ session: "jam", tempo: 120, peers: ["phone"] });
    expect(() => b.joinLink("jam", "tablet", 9999)).toThrow("tempo");
    expect(b.joinLink("jam", "phone").peers).toEqual(["phone"]);
  });
});
