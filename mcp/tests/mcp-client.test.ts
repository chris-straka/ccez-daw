import { describe, expect, test } from "bun:test";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { InMemoryTransport } from "@modelcontextprotocol/sdk/inMemory.js";
import { InMemoryBackend } from "../src/backend";
import { createServer } from "../src/index";

function textOf(result: unknown): any {
  const content = (result as any).content;
  expect(Array.isArray(content)).toBe(true);
  expect(content[0].type).toBe("text");
  return JSON.parse(content[0].text);
}

async function linkedClient(backend: InMemoryBackend): Promise<Client> {
  const [clientT, serverT] = InMemoryTransport.createLinkedPair();
  const server = createServer(backend);
  await server.connect(serverT);
  const client = new Client({ name: "track-l-test", version: "0.0.0" });
  await client.connect(clientT);
  return client;
}

describe("Track L validation: MCP client lists tracks and adds a clip", () => {
  test("six frozen tools are listed", async () => {
    const client = await linkedClient(new InMemoryBackend());
    const { tools } = await client.listTools();
    expect(tools.map((t) => t.name)).toEqual([
      "project_get",
      "project_list_tracks",
      "project_add_clip",
      "param_set",
      "transport_play",
      "transport_stop",
    ]);
    await client.close();
  });

  test("list tracks, add a clip, undo it; op log actor is mcp", async () => {
    const backend = new InMemoryBackend();
    const client = await linkedClient(backend);

    const tracks = textOf(
      await client.callTool({
        name: "project_list_tracks",
        arguments: { projectId: "proj_1" },
      }),
    );
    expect(tracks).toHaveLength(1);
    expect(tracks[0].id).toBe("trk_1");

    const added = textOf(
      await client.callTool({
        name: "project_add_clip",
        arguments: {
          trackId: "trk_1",
          name: "Take 1",
          startBeats: 0,
          lengthBeats: 4,
          kind: "Midi",
        },
      }),
    );
    expect(typeof added.clipId).toBe("string");
    expect(typeof added.seq).toBe("number");

    const project = textOf(
      await client.callTool({
        name: "project_get",
        arguments: { projectId: "proj_1" },
      }),
    );
    expect(project.clips.map((c: any) => c.id)).toContain(added.clipId);

    // The op appears in the log with actor `mcp` (contract validation).
    const log = backend.getOpLog();
    const clipOp = log.find((o) => o.target === added.clipId);
    expect(clipOp?.kind).toBe("ClipAdded");
    expect(clipOp?.actor).toBe("mcp");

    // Undo removes the clip and records an UndoMarker, still actor mcp.
    const { undoneSeq } = backend.undo();
    expect(undoneSeq).toBe(added.seq);
    expect(backend.getProject().clips).toHaveLength(0);
    const marker = backend.getOpLog().at(-1);
    expect(marker?.kind).toBe("UndoMarker");
    expect(marker?.actor).toBe("mcp");

    await client.close();
  });

  test("param_set and transport round-trip through the client", async () => {
    const backend = new InMemoryBackend();
    const client = await linkedClient(backend);

    const set = textOf(
      await client.callTool({
        name: "param_set",
        arguments: { node: "trk_1", param: "volume", value: 0.5 },
      }),
    );
    expect(typeof set.seq).toBe("number");
    const paramOp = backend.getOpLog().at(-1);
    expect(paramOp?.kind).toBe("ParamSet");
    expect(paramOp?.target).toBe("trk_1:volume");
    expect(paramOp?.actor).toBe("mcp");

    expect(
      textOf(
        await client.callTool({
          name: "transport_play",
          arguments: { projectId: "proj_1" },
        }),
      ),
    ).toEqual({ state: "Playing" });
    expect(
      textOf(
        await client.callTool({
          name: "transport_stop",
          arguments: { projectId: "proj_1" },
        }),
      ),
    ).toEqual({ state: "Stopped" });

    await client.close();
  });

  test("bad clip input is rejected", async () => {
    const client = await linkedClient(new InMemoryBackend());
    // Zero-length clip: SDK schema or handler must reject. Either a
    // thrown MCP error or an error result counts as rejection.
    let rejected = false;
    try {
      const res = await client.callTool({
        name: "project_add_clip",
        arguments: {
          trackId: "trk_1",
          name: "bad",
          startBeats: 0,
          lengthBeats: 0,
          kind: "Midi",
        },
      });
      rejected = (res as any).isError === true;
    } catch {
      rejected = true;
    }
    expect(rejected).toBe(true);

    let unknownTrack = false;
    try {
      const res = await client.callTool({
        name: "project_add_clip",
        arguments: {
          trackId: "nope",
          name: "bad",
          startBeats: 0,
          lengthBeats: 4,
          kind: "Midi",
        },
      });
      unknownTrack = (res as any).isError === true;
    } catch {
      unknownTrack = true;
    }
    expect(unknownTrack).toBe(true);
    await client.close();
  });
});
