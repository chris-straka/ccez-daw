import { describe, expect, test } from "bun:test";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { InMemoryTransport } from "@modelcontextprotocol/sdk/inMemory.js";
import { GAMEAUDIO_EXPORT_ACTOR, InMemoryBackend } from "../src/backend";
import { createServer } from "../src/index";
import {
  DEMO_BANK_ID,
  DEMO_CUE_ID,
  DEMO_EVENT_ID,
  GameAudioStore,
} from "../src/gameaudio";

function textOf(result: unknown): any {
  const content = (result as any).content;
  expect(Array.isArray(content)).toBe(true);
  expect(content[0].type).toBe("text");
  return JSON.parse(content[0].text);
}

async function linkedClient(
  backend: InMemoryBackend,
  store: GameAudioStore,
): Promise<Client> {
  const [clientT, serverT] = InMemoryTransport.createLinkedPair();
  const server = createServer(backend, store);
  await server.connect(serverT);
  const client = new Client({ name: "ga-5-test", version: "0.0.0" });
  await client.connect(clientT);
  return client;
}

async function call(client: Client, name: string, args: Record<string, unknown>): Promise<any> {
  return textOf(await client.callTool({ name, arguments: args }));
}

describe("GA-5 validation: MCP client drives game audio", () => {
  test("lists cues, auditions snapshots, triggers an event", async () => {
    const backend = new InMemoryBackend();
    const store = new GameAudioStore();
    const client = await linkedClient(backend, store);

    const { tools } = await client.listTools();
    expect(tools.map((t) => t.name).slice(6)).toEqual([
      "gameaudio_list_cues",
      "gameaudio_audition",
      "gameaudio_trigger_sfx",
      "gameaudio_export",
    ]);

    const cues = await call(client, "gameaudio_list_cues", { projectId: "proj_1" });
    const demo = cues.find((c: any) => c.id === DEMO_CUE_ID);
    expect(demo.name).toBe("Demo Cue");
    expect(demo.layers.map((l: any) => l.id).sort()).toEqual(["bed", "drums"]);

    // explore: only the always-on bed is audible; no transition (same state).
    const explore = await call(client, "gameaudio_audition", { state: "explore", values: [] });
    const exploreCue = explore.cues.find((c: any) => c.cue_id === DEMO_CUE_ID);
    expect(exploreCue.audible_layers).toEqual(["bed"]);
    expect(exploreCue.transition).toBeNull();

    // explore -> combat: drums join with the 4-beat fade rule.
    const combat = await call(client, "gameaudio_audition", {
      state: "combat",
      values: [{ param: "threat", value: 0.8 }],
    });
    const combatCue = combat.cues.find((c: any) => c.cue_id === DEMO_CUE_ID);
    expect(combatCue.audible_layers.sort()).toEqual(["bed", "drums"]);
    expect(combatCue.transition).toMatchObject({ kind: "Fade", fade_beats: 4 });

    // combat -> explore: no rule written, so clean cut (null transition).
    const back = await call(client, "gameaudio_audition", { state: "explore", values: [] });
    expect(back.cues.find((c: any) => c.cue_id === DEMO_CUE_ID).transition).toBeNull();

    // Trigger resolves the seeded pool pick + humanization + RTPC.
    await call(client, "gameaudio_audition", {
      state: "combat",
      values: [{ param: "threat", value: 1 }],
    });
    const hit = await call(client, "gameaudio_trigger_sfx", { eventId: DEMO_EVENT_ID });
    expect(hit.dropped).toBe(false);
    expect(["clip_step_a", "clip_step_b"]).toContain(hit.clip_id);
    expect(hit.volume).toBeGreaterThan(0);
    expect(hit.rtpc).toEqual([{ target: "bus_sfx:volume", value: 1 }]);

    // Audition + trigger are evaluation-only: the project op log is empty.
    expect(backend.getOpLog()).toHaveLength(0);

    await client.close();
  });

  test("cooldown drops bursts; unknown game params ignored quietly", () => {
    let now = 1_000;
    const store = new GameAudioStore({ nowMs: () => now });
    const first = store.trigger(DEMO_EVENT_ID);
    expect(first.dropped).toBe(false);
    const burst = store.trigger(DEMO_EVENT_ID);
    expect(burst.dropped).toBe(true);
    expect(burst.reason).toBe("cooldown");
    now += 200;
    expect(store.trigger(DEMO_EVENT_ID).dropped).toBe(false);

    // Unknown game param: ignored quietly (empty rtpc, no throw).
    now += 200;
    const silent = store.trigger(DEMO_EVENT_ID, [{ param: "no_such_param", value: 5 }]);
    expect(silent.dropped).toBe(false);
    expect(silent.rtpc).toEqual([]);
  });

  test("same seed = same clip/pitch sequence (seeded determinism)", () => {
    let a = 0;
    let b = 0;
    const s1 = new GameAudioStore({ seed: 42, nowMs: () => (a += 500) });
    const s2 = new GameAudioStore({ seed: 42, nowMs: () => (b += 500) });
    const seq1 = [0, 1, 2, 3, 4].map(() => s1.trigger(DEMO_EVENT_ID));
    const seq2 = [0, 1, 2, 3, 4].map(() => s2.trigger(DEMO_EVENT_ID));
    expect(seq1).toEqual(seq2);
  });

  test("exports a green package; op lands with a gameaudio:* actor", async () => {
    const backend = new InMemoryBackend();
    const store = new GameAudioStore();
    const client = await linkedClient(backend, store);

    const c1 = await call(client, "project_add_clip", {
      trackId: "trk_1",
      name: "bed take",
      startBeats: 0,
      lengthBeats: 16,
      kind: "Audio",
    });
    const c2 = await call(client, "project_add_clip", {
      trackId: "trk_1",
      name: "step take",
      startBeats: 0,
      lengthBeats: 1,
      kind: "Audio",
    });
    store.addCue({
      schema_version: 1,
      id: "cue_game",
      name: "Game",
      tempo: 100,
      default_state: "explore",
      layers: [{ id: "bed", name: "Bed", clip_ids: [c1.clipId], states: [], volume: 0.8 }],
      transitions: [],
    });
    store.addBank({
      schema_version: 1,
      id: "bank_game",
      name: "Game",
      events: [
        {
          id: "player.jump",
          name: "Jump",
          clip_ids: [c2.clipId],
          volume: 0.7,
          volume_random: 0.05,
          pitch_random: 1.0,
          cooldown_ms: 50,
          max_polyphony: 2,
          rtpc: [{ param: "threat", target_node: "trk_1", target_param: "volume", min: 0.5, max: 1 }],
        },
      ],
    });

    const report = await call(client, "gameaudio_export", {
      name: "demo-v1",
      cueIds: ["cue_game"],
      bankIds: ["bank_game"],
    });
    expect(report.ok).toBe(true);
    expect(report.rules.filter((r: any) => r.status === "fail")).toEqual([]);
    expect(report.package.event_bank_path).toBe("bank_bank_game.json");
    expect(report.package.validator_version).toBe("1");

    // The green export lands one ordinary op with a gameaudio:* actor.
    const log = backend.getOpLog();
    const exported = log.at(-1);
    expect(exported?.kind).toBe("GameAudioExported");
    expect(exported?.actor).toBe(GAMEAUDIO_EXPORT_ACTOR);
    expect(exported?.actor.startsWith("gameaudio:")).toBe(true);

    await client.close();
  });

  test("broken fixtures fail red with a named rule", async () => {
    const backend = new InMemoryBackend();
    const store = new GameAudioStore();
    const client = await linkedClient(backend, store);

    // Demo clips don't exist in the empty project: rule 2 names them.
    const dangling = await call(client, "gameaudio_export", {
      name: "demo",
      cueIds: [DEMO_CUE_ID],
      bankIds: [DEMO_BANK_ID],
    });
    expect(dangling.ok).toBe(false);
    const r2 = dangling.rules.find((r: any) => r.rule === 2);
    expect(r2.status).toBe("fail");
    expect(r2.detail).toContain("clip_bed");

    // A red export writes no op (errors as data, not silence, not a record).
    expect(backend.getOpLog()).toHaveLength(0);

    // Empty pool + duplicate event ids: rule 2.
    store.addBank({
      schema_version: 1,
      id: "bank_bad",
      name: "Bad",
      events: [
        {
          id: "ev_empty",
          name: "Empty",
          clip_ids: [],
          volume: 1,
          volume_random: 0,
          pitch_random: 0,
          cooldown_ms: 0,
          max_polyphony: 1,
          rtpc: [],
        },
        {
          id: "ev_empty",
          name: "Dup",
          clip_ids: [],
          volume: 1,
          volume_random: 0,
          pitch_random: 0,
          cooldown_ms: 0,
          max_polyphony: 1,
          rtpc: [],
        },
      ],
    });
    const emptyPool = store.validatePackage(store.buildPackage("bad", [], ["bank_bad"]), {
      clipIds: new Set(),
      nodeIds: new Set(["trk_1"]),
    });
    expect(emptyPool.ok).toBe(false);
    expect(emptyPool.rules.find((r) => r.rule === 2)?.detail).toContain("empty pool");
    expect(emptyPool.rules.find((r) => r.rule === 2)?.detail).toContain("duplicate event id");

    // Dangling stinger + unknown mix target: rule 3 (loud), unknown game
    // param stays quiet (rule 3 only checks mix targets).
    store.addCue({
      schema_version: 1,
      id: "cue_sting",
      name: "Sting",
      tempo: 120,
      default_state: "explore",
      layers: [],
      transitions: [
        {
          id: "t1",
          from_state: "explore",
          to_state: "combat",
          kind: "Stinger",
          fade_beats: 0,
          stinger_cue_id: "cue_missing",
        },
      ],
    });
    store.addBank({
      schema_version: 1,
      id: "bank_rtpc",
      name: "Rtpc",
      events: [
        {
          id: "ev_rtpc",
          name: "Rtpc",
          clip_ids: ["clip_x"],
          volume: 1,
          volume_random: 0,
          pitch_random: 0,
          cooldown_ms: 0,
          max_polyphony: 1,
          rtpc: [{ param: "threat", target_node: "bus_missing", target_param: "volume", min: 0, max: 1 }],
        },
      ],
    });
    const badRefs = store.validatePackage(store.buildPackage("bad", ["cue_sting"], ["bank_rtpc"]), {
      clipIds: new Set(["clip_x"]),
      nodeIds: new Set(["trk_1"]),
    });
    expect(badRefs.ok).toBe(false);
    const r3 = badRefs.rules.find((r) => r.rule === 3);
    expect(r3?.status).toBe("fail");
    expect(r3?.detail).toContain("cue_missing");
    expect(r3?.detail).toContain("bus_missing:volume");

    // Bad loop range: rule 4.
    const pkg = store.buildPackage("loops", [], []);
    pkg.stems.push({
      path: "stems/x.wav",
      source_id: "cue_sting",
      source_layer_id: "bed",
      kind: "MusicLayer",
      loop_start_beats: 4,
      loop_end_beats: 2,
    });
    const badLoop = store.validatePackage(pkg, { clipIds: new Set(), nodeIds: new Set() });
    expect(badLoop.rules.find((r) => r.rule === 4)?.status).toBe("fail");

    // Unknown cue/bank ids never build (loud error, not a silent package).
    // Like the v0 bad-input test: a thrown MCP error or an error result
    // both count as rejection.
    let rejected = false;
    try {
      const res = await client.callTool({
        name: "gameaudio_export",
        arguments: { name: "nope", cueIds: ["cue_missing"], bankIds: [] },
      });
      rejected = (res as any).isError === true;
    } catch {
      rejected = true;
    }
    expect(rejected).toBe(true);

    await client.close();
  });
});
