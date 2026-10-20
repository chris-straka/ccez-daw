import { describe, expect, test } from "bun:test";
import { existsSync, readdirSync, readFileSync, statSync } from "node:fs";
import { dirname, join, relative, resolve } from "node:path";

// `bun test` has no Solid JSX transform: Bun compiles .tsx against
// `solid-js/jsx-dev-runtime`, which Solid ships as a stub, so a unit test
// that reaches a component (often through a folder's index.ts barrel) dies
// with "Export named 'jsxDEV' not found" on Bun >= 1.4.3. Unit tests import
// leaf modules; components are covered by the e2e specs.

const TESTS = import.meta.dir;
const IMPORT = /(?:import|export)\s[^;]*?from\s+"(\.[^"]+)"|import\(\s*"(\.[^"]+)"\s*\)/g;

function resolveLocal(from: string, spec: string): string | null {
  const base = resolve(dirname(from), spec);
  for (const c of [base, `${base}.ts`, `${base}.tsx`, join(base, "index.ts"), join(base, "index.tsx")]) {
    if (existsSync(c) && statSync(c).isFile()) return c;
  }
  return null;
}

/** First import chain from `entry` that reaches a .tsx file, or null. */
function tsxChain(entry: string): string[] | null {
  const seen = new Set<string>();
  const walk = (file: string, chain: string[]): string[] | null => {
    if (file.endsWith(".tsx")) return chain;
    if (seen.has(file)) return null;
    seen.add(file);
    for (const m of readFileSync(file, "utf8").matchAll(IMPORT)) {
      const next = resolveLocal(file, m[1] ?? m[2]!);
      if (!next) continue;
      const found = walk(next, [...chain, relative(TESTS, next)]);
      if (found) return found;
    }
    return null;
  };
  return walk(entry, [relative(TESTS, entry)]);
}

describe("unit tests stay JSX-free", () => {
  test("no test file transitively imports a .tsx module", () => {
    const offenders = readdirSync(TESTS)
      .filter((f) => f.endsWith(".test.ts"))
      .map((f) => tsxChain(join(TESTS, f)))
      .filter((c): c is string[] => c !== null)
      .map((c) => c.join(" → "));
    expect(offenders).toEqual([]);
  });
});
