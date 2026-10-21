import { describe, expect, test } from "bun:test";
import { GAMEAUDIO_EXPORT_ACTOR, InMemoryBackend } from "../src/backend";
import { MergeOpSchema } from "../src/tools";
import { OpKindSchema, OpSchema } from "../../ui/src/generated/project";

/**
 * Ops the MCP writes must be ops core can read: every kind lands in the
 * generated `OpKindSchema` (typegen from core `OpKind`) and every actor
 * fits `contracts/op-log-format.md` (`ui`, `mcp`, `ai:<sidecar>`,
 * `script:<name>`, `gameaudio:<tool>`).
 */
const ACTOR = /^(ui|mcp|(ai|script|gameaudio):.+)$/;

describe("MCP op log matches the frozen op-log contract", () => {
  test("game-audio export record is a core OpKind with a contract actor", () => {
    const backend = new InMemoryBackend();
    const { seq } = backend.logGameAudioExport("demo-v1", 2);
    const op = backend.getOpLog().find((o) => o.seq === seq);
    expect(OpSchema.safeParse(op).success).toBe(true);
    expect(op?.kind).toBe("GameAudioExported");
    expect(op?.actor).toBe(GAMEAUDIO_EXPORT_ACTOR);
    expect(ACTOR.test(op!.actor)).toBe(true);
    // Payload shape core validates (`check_game_audio_export`).
    expect(op?.target).toBe("demo-v1");
    expect(JSON.parse(op!.value_json)).toEqual({ packageName: "demo-v1", stemCount: 2 });
  });

  test("merge input accepts exactly the core OpKind set", () => {
    const merge = MergeOpSchema.shape.kind.options;
    expect([...merge].sort()).toEqual([...OpKindSchema.options].sort());
  });
});
