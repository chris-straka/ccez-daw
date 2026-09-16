import { describe, expect, test } from "bun:test";
import type { Clip } from "../src/generated/project";
import { decodeClip, encodeClip, type MidiClip } from "../src/pianoroll/model";
import { createNote } from "../src/pianoroll/store";
import {
  createNoteAtStep,
  pruneSelection,
  selectOnly,
  toggleSelect,
} from "../src/notation";
import {
  emptyStaffFor,
  midiClips,
  readScoreClip,
  resolveScoreClip,
  scoreCommitOp,
  writeScoreClip,
} from "../src/notation";
import { sampleProject } from "../src/project/sample";

function midiClip(id: string, name: string): Clip {
  return {
    id,
    track_id: "trk_music",
    name,
    start_beats: 0,
    length_beats: 8,
    kind: "Midi",
    source: `take:${id}`,
  };
}

describe("score panel model", () => {
  test("only MIDI clips are listed; selection defaults to the first", () => {
    const project = { ...sampleProject(), clips: [midiClip("a", "A"), midiClip("b", "B")] };
    // Prepend an audio clip: the score must ignore it.
    project.clips.unshift({
      id: "audio",
      track_id: "trk_click",
      name: "Count-in",
      start_beats: 0,
      length_beats: 4,
      kind: "Audio",
      source: "builtin:click",
    });
    expect(midiClips(project).map((c) => c.id)).toEqual(["a", "b"]);
    expect(resolveScoreClip(project)?.id).toBe("a");
    expect(resolveScoreClip(project, "b")?.id).toBe("b");
    expect(resolveScoreClip(project, "missing")).toBeNull();
    expect(midiClips(null)).toEqual([]);
    expect(resolveScoreClip(null)).toBeNull();
    expect(resolveScoreClip({ ...project, clips: [] })).toBeNull();
  });

  test("asset read/write path round-trips byte-stable; missing key is empty, corrupt throws", () => {
    const frozen = midiClip("a", "A");
    expect(readScoreClip(frozen.source, {})).toBeNull();
    expect(emptyStaffFor(frozen)).toEqual({ length_beats: 8, notes: [] });

    let clip: MidiClip = emptyStaffFor(frozen);
    clip = createNoteAtStep(clip, 0, "treble", 1, { snap: 0.25, lenBeats: 1 });
    const bytes = writeScoreClip(clip);
    const assets = { [frozen.source]: bytes };
    const back = readScoreClip(frozen.source, assets);
    expect(back).not.toBeNull();
    expect(encodeClip(back!)).toBe(bytes);
    expect(encodeClip(decodeClip(bytes))).toBe(bytes);
    expect(() => readScoreClip(frozen.source, { [frozen.source]: "not json" })).toThrow();
  });

  test("score creates use the same note_id sequence as the piano roll", () => {
    const frozen = midiClip("a", "A");
    let clip: MidiClip = emptyStaffFor(frozen);
    // Piano-roll gesture first…
    clip = createNote(clip, 60, 0, { snap: 0.25, lenBeats: 1 });
    // …then a staff click: ids continue the same sequence, no clash.
    clip = createNoteAtStep(clip, 0, "treble", 2, { snap: 0.25, lenBeats: 1 });
    expect(clip.notes.map((n) => n.note_id)).toEqual([1, 2]);
    const bytes = writeScoreClip(clip);
    expect(encodeClip(decodeClip(bytes))).toBe(bytes);
  });

  test("select/additive-select/delete flow prunes against the clip", () => {
    const frozen = midiClip("a", "A");
    let clip: MidiClip = emptyStaffFor(frozen);
    clip = createNoteAtStep(clip, 0, "treble", 0, { snap: 0.25, lenBeats: 1 });
    clip = createNoteAtStep(clip, 2, "treble", 1, { snap: 0.25, lenBeats: 1 });
    const [first, second] = clip.notes.map((n) => n.note_id);

    // Click selects only; shift-click adds; pruning drops stale ids.
    let sel = selectOnly([first]);
    sel = toggleSelect(sel, second);
    expect([...sel].sort()).toEqual([first, second]);
    expect(pruneSelection(clip, new Set([first, 9999]))).toEqual(new Set([first]));
  });

  test("commit is one frozen ClipAdded op carrying the full clip", () => {
    const frozen = midiClip("a", "Sketch");
    const op = scoreCommitOp("ui", frozen);
    expect(op.seq).toBe(0);
    expect(op.kind).toBe("ClipAdded");
    expect(op.target).toBe("a");
    expect(JSON.parse(op.value_json)).toEqual(frozen);
  });
});
