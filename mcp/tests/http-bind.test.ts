import { describe, expect, test } from "bun:test";
import { resolve } from "node:path";
import { DEFAULT_HTTP_HOST, resolveHttpHost } from "../src/index";

function freePort(): number {
  const s = Bun.serve({ port: 0, fetch: () => new Response("ok") });
  const p = s.port;
  s.stop(true);
  if (!p) throw new Error("failed to allocate ephemeral port");
  return p;
}

describe("MCP HTTP bind security", () => {
  test("resolveHttpHost defaults to 127.0.0.1 and honors explicit --host", () => {
    expect(DEFAULT_HTTP_HOST).toBe("127.0.0.1");
    expect(resolveHttpHost(["bun", "index.ts", "--http"])).toBe("127.0.0.1");
    expect(resolveHttpHost(["bun", "index.ts", "--http", "--host=0.0.0.0"])).toBe("0.0.0.0");
    expect(resolveHttpHost(["bun", "index.ts", "--http", "--host", "::1"])).toBe("::1");
    expect(() => resolveHttpHost(["bun", "index.ts", "--http", "--host="])).toThrow();
    expect(() => resolveHttpHost(["bun", "index.ts", "--http", "--host"])).toThrow();
  });

  test("default --http bind uses 127.0.0.1 rather than 0.0.0.0", async () => {
    const port = freePort();
    const indexPath = resolve(import.meta.dir, "../src/index.ts");
    const proc = Bun.spawn(["bun", indexPath, "--http", `--port=${port}`], {
      stdout: "pipe",
      stderr: "pipe",
    });
    const reader = proc.stderr.getReader();
    let stderr = "";
    const deadline = Date.now() + 5000;
    try {
      while (Date.now() < deadline) {
        const { value, done } = await reader.read();
        if (done) break;
        stderr += new TextDecoder().decode(value);
        if (stderr.includes("listening for Streamable HTTP")) break;
      }
      expect(stderr).toContain(`127.0.0.1:${port}`);
    } finally {
      proc.kill();
      await proc.exited;
    }
  });
});
