#!/usr/bin/env bun
/**
 * Call one tool on a running ccez-daw MCP server over Streamable HTTP, for
 * agents whose harness has no MCP client configured for it:
 *
 *   bun run start:http &                       # serves :3001/mcp (PORT=…)
 *   bun scripts/call.ts compose_new '{"name":"theme","tempo":84}'
 *   bun scripts/call.ts --list                 # tool names + descriptions
 *
 * State (the composition) lives in the server process, so successive calls
 * build on each other. Prints the tool's JSON result; exits 1 on a tool error.
 */
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { StreamableHTTPClientTransport } from "@modelcontextprotocol/sdk/client/streamableHttp.js";

const url = new URL(process.env["CCEZ_MCP_URL"] ?? `http://localhost:${process.env["PORT"] ?? 3001}/mcp`);
const [name, raw] = process.argv.slice(2);
if (!name) {
  console.error("usage: call.ts <tool> [json-args] | --list");
  process.exit(2);
}
const client = new Client({ name: "ccez-daw-call", version: "0.1.0" });
await client.connect(new StreamableHTTPClientTransport(url));
try {
  if (name === "--list") {
    for (const t of (await client.listTools()).tools) console.log(`${t.name}: ${t.description}`);
  } else {
    const args = raw ? JSON.parse(raw.startsWith("@") ? await Bun.file(raw.slice(1)).text() : raw) : {};
    const r = (await client.callTool({ name, arguments: args })) as any;
    const text = r.content?.[0]?.text ?? "";
    if (r.isError) {
      console.error(text);
      process.exitCode = 1;
    } else {
      try {
        console.log(JSON.stringify(JSON.parse(text), null, 2));
      } catch {
        console.log(text);
      }
    }
  }
} finally {
  await client.close();
}
