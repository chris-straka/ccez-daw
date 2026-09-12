import { describe, expect, test } from "bun:test";
import {
  aiActor,
  LocalTranscriptionProvider,
  SIDECAR_IDS,
  submitTranscription,
  type TranscriptionInput,
} from "../src/ai";
import { NlOpStore } from "../src/nl";

const provider = new LocalTranscriptionProvider();

async function runJob(input: TranscriptionInput) {
  const seen: number[] = [];
  const job = submitTranscription(provider, input, (p) => seen.push(p));
  // Background: polling synchronously never reports done.
  expect(["pending", "running"]).toContain(job.poll().status);
  const plan = await job.wait();
  expect(job.poll().status).toBe("done");
  expect(job.tryTake()).toBe(plan);
  return { plan, seen };
}

describe("Track M: transcription sidecars yield editable undoable ops", () => {
  test("drums job: hits become an editable ClipAdded op, then undo", async () => {
    const frames = [
      0.9, 0.1, 0.1, 0.1, 0.9, 0.1, 0.1, 0.1, 0.9, 0.1, 0.1, 0.1, 0.9, 0.1, 0.1, 0.1,
    ];
    const { plan } = await runJob({
      kind: "drums",
      frames,
      framesPerBeat: 4,
      threshold: 0.5,
      lengthBeats: 4,
      clipId: "clip_drums_ai",
      trackId: "trk_music",
    });
    expect(plan.kind).toBe("drums");
    expect(plan.ops).toHaveLength(1);
    expect(plan.ops[0].kind).toBe("ClipAdded");
    expect(plan.notes).toHaveLength(4);
    expect(plan.notes.every((n) => n.pitch === 36)).toBe(true);

    // Editable: reshape notes before committing.
    const edited = plan.notes.map((n) => ({ ...n, velocity: 100 }));
    expect(edited[0].velocity).toBe(100);

    const store = new NlOpStore([{ id: "trk_music", name: "Music" }]);
    const seqs = store.applyPlan(aiActor(SIDECAR_IDS.drums), plan);
    expect(seqs).toEqual([1]);
    expect(store.clips()).toHaveLength(1);
    expect(store.undo()).toBe(1);
    expect(store.clips()).toHaveLength(0);
    expect(store.opKinds()).toContain("UndoMarker");
  });

  test("melody job: pitch runs become notes, then undo", async () => {
    const v = (beat: number, midi: number) => ({ beat, midi });
    const { plan } = await runJob({
      kind: "melody",
      frames: [
        v(0, 60), v(0.5, 60), v(1, 60), v(1.5, 60),
        { beat: 2, midi: null },
        v(2.5, 64), v(3, 64), v(3.5, 64),
      ],
      minLenBeats: 0.25,
      lengthBeats: 4,
      clipId: "clip_mel_ai",
      trackId: "trk_music",
    });
    expect(plan.kind).toBe("melody");
    expect(plan.notes.map((n) => n.pitch)).toEqual([60, 64]);

    const store = new NlOpStore([{ id: "trk_music", name: "Music" }]);
    store.applyPlan(aiActor("transcribe-melody"), plan);
    expect(store.clips()[0].id).toBe("clip_mel_ai");
    store.undo();
    expect(store.clips()).toHaveLength(0);
    store.redo();
    expect(store.clips()).toHaveLength(1);
  });

  test("chords job: chroma bars become labels + pad, then undo", async () => {
    const chroma = (tones: number[]) => {
      const c = new Array(12).fill(0);
      for (const t of tones) c[t % 12] = 1;
      return c;
    };
    const { plan } = await runJob({
      kind: "chords",
      chromas: [chroma([0, 4, 7]), chroma([9, 0, 4])],
      beatsPerBar: 4,
      clipId: "clip_chords_ai",
      trackId: "trk_music",
    });
    expect(plan.kind).toBe("chords");
    expect(plan.labels).toEqual(["C", "Am"]);
    expect(plan.notes).toHaveLength(6);

    const store = new NlOpStore([{ id: "trk_music", name: "Music" }]);
    store.applyPlan(aiActor(SIDECAR_IDS.chords), plan);
    expect(store.clips()[0].name).toContain("C");
    store.undo();
    expect(store.clips()).toHaveLength(0);
  });

  test("sidecar pattern: models unpinned, actors ai:-scoped, DAW runs without them", async () => {
    // Model id is a plain string on the HTTP sidecar — nothing pins a version.
    const { HttpTranscriptionSidecar, UnreachableProvider } = await import("../src/ai");
    const http = new HttpTranscriptionSidecar({
      endpoint: "http://127.0.0.1:9",
      model: "whisper-large-v3",
    });
    expect(http.model).toBe("whisper-large-v3");
    expect(http.id).toBe("transcribe-sidecar");

    // Actor rule: transcription output must be ai:-scoped.
    expect(aiActor("transcribe-drums")).toBe("ai:transcribe-drums");
    expect(() => aiActor("  ")).toThrow();

    // The DAW path never touches a provider: a hand-built plan applies cleanly.
    const store = new NlOpStore([{ id: "trk_music", name: "Music" }]);
    await expect(new UnreachableProvider().transcribe({} as never)).rejects.toThrow();
    expect(store.clips()).toHaveLength(0);
  });
});
