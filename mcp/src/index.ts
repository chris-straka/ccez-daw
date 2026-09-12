import { McpServer } from "@modelcontextprotocol/sdk/server/mcp.js";
import { StdioServerTransport } from "@modelcontextprotocol/sdk/server/stdio.js";
import {
  ClipInput,
  ParamInput,
  ProjectRef,
  TOOLS,
} from "./tools.js";

const server = new McpServer({ name: "ccez-daw", version: "0.1.0" });

// v0 stubs: every tool is registered with its frozen input shape and throws
// "not implemented" until Track L wires handlers to the op log. Registering
// the shapes now freezes the MCP surface in `contracts/mcp-tools.md`.
server.registerTool(
  "project_get",
  { description: TOOLS[0].description, inputSchema: ProjectRef.shape },
  async () => {
    throw new Error("not implemented (Track L)");
  },
);

server.registerTool(
  "project_list_tracks",
  { description: TOOLS[1].description, inputSchema: ProjectRef.shape },
  async () => {
    throw new Error("not implemented (Track L)");
  },
);

server.registerTool(
  "project_add_clip",
  { description: TOOLS[2].description, inputSchema: ClipInput.shape },
  async () => {
    throw new Error("not implemented (Track L)");
  },
);

server.registerTool(
  "param_set",
  { description: TOOLS[3].description, inputSchema: ParamInput.shape },
  async () => {
    throw new Error("not implemented (Track L)");
  },
);

server.registerTool(
  "transport_play",
  { description: TOOLS[4].description, inputSchema: ProjectRef.shape },
  async () => {
    throw new Error("not implemented (Track L)");
  },
);

server.registerTool(
  "transport_stop",
  { description: TOOLS[5].description, inputSchema: ProjectRef.shape },
  async () => {
    throw new Error("not implemented (Track L)");
  },
);

async function main() {
  const transport = new StdioServerTransport();
  await server.connect(transport);
}

void main();
