import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import { WebStandardStreamableHTTPServerTransport } from "@modelcontextprotocol/sdk/server/webStandardStreamableHttp.js";
import { InMemoryBackend } from "./backend.js";
import {
  ClipInput,
  ParamInput,
  ProjectRef,
  TOOLS,
  createToolHandlers,
  type ToolHandler,
} from "./tools.js";

/** Wrap a plain handler value into an MCP text content block. */
function toContent(value: unknown) {
  return { content: [{ type: "text" as const, text: JSON.stringify(value) }] };
}

/** Build a wired server over one backend (one backend per instance, so
 *  the op log actor is always `"mcp"` for that server's mutations). */
export function createServer(backend: InMemoryBackend): McpServer {
  const server = new McpServer({ name: "ccez-daw", version: "0.1.0" });
  const handlers = createToolHandlers(backend);
  const inputSchemas = [
    ProjectRef.shape,
    ProjectRef.shape,
    ClipInput.shape,
    ParamInput.shape,
    ProjectRef.shape,
    ProjectRef.shape,
  ] as const;

  TOOLS.forEach((tool, i) => {
    const run: ToolHandler = handlers[tool.name as keyof typeof handlers];
    server.registerTool(
      tool.name,
      { description: tool.description, inputSchema: inputSchemas[i] },
      async (args: unknown) => toContent(await run(args)),
    );
  });
  return server;
}

function getPort(): number {
  const fromArg = process.argv
    .find((a) => a.startsWith("--port="))
    ?.slice("--port=".length);
  const raw = fromArg ?? process.env["PORT"] ?? "3001";
  const port = Number.parseInt(raw, 10);
  if (!Number.isFinite(port) || port <= 0) throw new Error(`bad port: ${raw}`);
  return port;
}

async function main() {
  const backend = new InMemoryBackend();
  const server = createServer(backend);

  if (process.argv.includes("--http")) {
    // Streamable HTTP (stateless) via the official TS SDK web-standard
    // transport; Bun.serve speaks web-standard Request/Response natively.
    // Stateless transports are single-use, so each request gets a fresh
    // transport + server over the one shared backend (the op log persists).
    const port = getPort();
    Bun.serve({
      port,
      fetch: async (req: Request) => {
        const transport = new WebStandardStreamableHTTPServerTransport({
          sessionIdGenerator: undefined,
        });
        const perRequest = createServer(backend);
        await perRequest.connect(transport);
        try {
          return await transport.handleRequest(req);
        } finally {
          await perRequest.close();
          await transport.close();
        }
      },
    });
    console.error(`ccez-daw MCP listening for Streamable HTTP on :${port}/mcp`);
  } else {
    await server.connect(new StdioServerTransport());
  }
}

if (import.meta.main) {
  void main();
}
