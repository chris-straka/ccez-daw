import { describe, expect, test } from "bun:test";
import { OpSchema, type Op } from "../src/generated/project";
import { parseSample } from "../src/project/sample";
import {
  BranchError,
  BranchStore,
  MAIN_BRANCH,
  classifyOp,
  describeOp,
  mergeCommitOps,
  type OpCategory,
} from "../src/branch/branch";

function op(seq: number, kind: Op["kind"], target: string, value: string): Op {
  return OpSchema.parse({ seq, actor: "ui", kind, target, value_json: value });
}

/** Mirror of the Rust `two_track_setup` branch test fixture. */
function twoTrackSetup(): BranchStore {
  const store = new BranchStore();
  store.stage("main", "ui", "TrackAdded", "trk_a", '"A"');
  store.stage("main", "ui", "TrackAdded", "trk_b", '"B"');
  store.createBranch("mix", "main");
  store.stage("mix", "ui", "ParamSet", "trk_a:volume", "0.5");
  store.stage("main", "ui", "TempoSet", "transport", "128");
  return store;
}

describe("branch workflow: list, compare, preview, commit", () => {
  test("store is born with main; branches fork and delete", () => {
    const store = new BranchStore();
    expect(store.branchNames()).toEqual([MAIN_BRANCH]);
    store.createBranch("mix", "main");
    expect(store.branchNames()).toEqual(["main", "mix"]);
    expect(() => store.createBranch("mix", "main")).toThrow(BranchError);
    expect(() => store.deleteBranch("main")).toThrow(BranchError);
    store.deleteBranch("mix");
    expect(store.branchNames()).toEqual(["main"]);
    expect(() => store.deleteBranch("mix")).toThrow(BranchError);
    expect(() => store.createBranch("x", "ghost")).toThrow(BranchError);
  });

  test("compare lists each side's unique ops", () => {
    const store = twoTrackSetup();
    const diff = store.compare("mix", "main");
    expect(diff.onlyInA).toHaveLength(1);
    expect(diff.onlyInA[0].target).toBe("trk_a:volume");
    expect(diff.onlyInB).toHaveLength(1);
    expect(diff.onlyInB[0].kind).toBe("TempoSet");
  });

  test("classifyOp covers every kind; describeOp names seq/kind/target", () => {
    const cases: Array<[Op["kind"], OpCategory]> = [
      ["TrackAdded", "added"],
      ["ClipAdded", "added"],
      ["ClipMoved", "moved"],
      ["ParamSet", "retargeted"],
      ["TempoSet", "tempo"],
      ["AutomationPointSet", "automation"],
      ["UndoMarker", "marker"],
    ];
    for (const [kind, want] of cases) {
      expect(classifyOp(op(1, kind, "t", "0"))).toBe(want);
    }
    const text = describeOp(op(7, "ParamSet", "trk_a:volume", "0.5"));
    expect(text).toContain("#7");
    expect(text).toContain("ParamSet");
    expect(text).toContain("trk_a:volume");
  });

  test("merge applies clean ops with fresh seqs", () => {
    const store = twoTrackSetup();
    const before = store.tipSeq("main");
    const outcome = store.mergeApply("mix", "main");
    expect(outcome.conflicts).toHaveLength(0);
    expect(outcome.merged).toHaveLength(1);
    expect(outcome.merged[0].seq).toBeGreaterThan(before);
    expect(store.history("main").some((o) => o.target === "trk_a:volume")).toBe(true);
  });

  test("merge surfaces same-target conflicts and leaves them out", () => {
    const store = new BranchStore();
    store.stage("main", "ui", "TrackAdded", "trk_a", '"A"');
    store.createBranch("alt", "main");
    store.stage("main", "ui", "ParamSet", "trk_a:volume", "0.9");
    store.stage("alt", "ui", "ParamSet", "trk_a:volume", "0.2");
    const preview = store.mergePreview("alt", "main");
    expect(preview.merged).toHaveLength(0);
    expect(preview.conflicts).toHaveLength(1);
    expect(preview.conflicts[0].target).toBe("trk_a:volume");
    const applied = store.mergeApply("alt", "main");
    expect(applied.conflicts).toHaveLength(1);
    expect(
      store.history("main").filter((o) => o.target === "trk_a:volume"),
    ).toHaveLength(1);
  });

  test("identical edits on both sides are not conflicts", () => {
    const store = new BranchStore();
    store.stage("main", "ui", "TrackAdded", "trk_a", '"A"');
    store.createBranch("alt", "main");
    store.stage("main", "ui", "ParamSet", "trk_a:volume", "0.5");
    store.stage("alt", "ui", "ParamSet", "trk_a:volume", "0.5");
    const preview = store.mergePreview("alt", "main");
    expect(preview.conflicts).toHaveLength(0);
    expect(preview.merged).toHaveLength(0);
  });

  test("clip moves conflict across beats but agree on one destination", () => {
    const store = new BranchStore();
    store.stage("main", "ui", "ClipAdded", "clip_b", '{"id":"clip_b"}');
    store.createBranch("arr", "main");
    store.stage("main", "ui", "ClipMoved", "clip_b", '{"startBeats": 8}');
    store.stage("arr", "ui", "ClipMoved", "clip_b", '{"startBeats": 2}');
    const preview = store.mergePreview("arr", "main");
    expect(preview.merged).toHaveLength(0);
    expect(preview.conflicts).toHaveLength(1);
    expect(classifyOp(preview.conflicts[0].sourceOp)).toBe("moved");

    const calm = new BranchStore();
    calm.stage("main", "ui", "ClipAdded", "clip_b", '{"id":"clip_b"}');
    calm.createBranch("arr", "main");
    calm.stage("main", "ui", "ClipMoved", "clip_b", '{"startBeats": 4}');
    calm.stage("arr", "ui", "ClipMoved", "clip_b", '{"startBeats": 4}');
    const ok = calm.mergePreview("arr", "main");
    expect(ok.conflicts).toHaveLength(0);
    expect(ok.merged).toHaveLength(0);
  });

  test("automation merges clean across lanes, conflicts on one point", () => {
    const store = new BranchStore();
    store.createBranch("auto", "main");
    store.stage(
      "auto",
      "ui",
      "AutomationPointSet",
      "lane_vol",
      '{"beat": 4, "value": 0.5}',
    );
    store.stage(
      "main",
      "ui",
      "AutomationPointSet",
      "lane_pan",
      '{"beat": 4, "value": 0.0}',
    );
    const clean = store.mergePreview("auto", "main");
    expect(clean.conflicts).toHaveLength(0);
    expect(clean.merged).toHaveLength(1);
    expect(classifyOp(clean.merged[0])).toBe("automation");

    store.stage(
      "main",
      "ui",
      "AutomationPointSet",
      "lane_vol",
      '{"beat": 4, "value": 0.9}',
    );
    const clash = store.mergePreview("auto", "main");
    expect(clash.merged).toHaveLength(0);
    expect(clash.conflicts).toHaveLength(1);
    expect(clash.conflicts[0].target).toBe("lane_vol");
  });

  test("merge commit ships clean ops with seq 0; conflicts never ship", () => {
    const store = twoTrackSetup();
    const preview = store.mergePreview("mix", "main");
    const ops = mergeCommitOps(preview);
    expect(ops).toHaveLength(1);
    expect(ops[0].seq).toBe(0);
    expect(ops[0].kind).toBe("ParamSet");
    expect(() => OpSchema.parse(ops[0])).not.toThrow();
  });

  test("snapshots capture the project and list with summaries", () => {
    const store = new BranchStore();
    const project = parseSample();
    const mark = store.captureSnapshot("v1", project);
    expect(mark.seq).toBe(0);
    expect(mark.summary).toContain("2 tracks");
    expect(store.snapshotNames()).toEqual(["v1"]);
    expect(() => store.captureSnapshot("v1", project)).toThrow(BranchError);
    store.deleteSnapshot("v1");
    expect(store.snapshotNames()).toEqual([]);
    expect(() => store.deleteSnapshot("v1")).toThrow(BranchError);
  });
});
