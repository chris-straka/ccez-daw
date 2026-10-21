import { describe, expect, test } from "bun:test";
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, relative } from "node:path";

// AGENTS.md: every interactive element gets a data-testid. Scans the JSX
// source (no rendering) for <button>, <input>, <select>, <textarea> opening
// tags and reports any without one.

const SRC = join(import.meta.dir, "..", "src");
const INTERACTIVE = /<(button|input|select|textarea)\b/g;

function tsxFiles(dir: string): string[] {
  return readdirSync(dir).flatMap((f) => {
    const p = join(dir, f);
    if (statSync(p).isDirectory()) return tsxFiles(p);
    return p.endsWith(".tsx") ? [p] : [];
  });
}

/** Source of the opening tag starting at `start` (skips `{…}` and quotes). */
function openingTag(src: string, start: number): string {
  let depth = 0;
  let quote: string | null = null;
  for (let i = start + 1; i < src.length; i++) {
    const c = src[i];
    if (quote) {
      if (c === quote && src[i - 1] !== "\\") quote = null;
    } else if (c === '"' || c === "'" || c === "`") {
      quote = c;
    } else if (c === "{") {
      depth++;
    } else if (c === "}") {
      depth--;
    } else if (c === ">" && depth === 0) {
      return src.slice(start, i + 1);
    }
  }
  return src.slice(start);
}

describe("interactive elements carry a data-testid", () => {
  test("every <button>/<input>/<select>/<textarea> in ui/src has one", () => {
    const missing: string[] = [];
    for (const file of tsxFiles(SRC)) {
      const src = readFileSync(file, "utf8");
      for (const m of src.matchAll(INTERACTIVE)) {
        if (/\bdata-testid\s*=/.test(openingTag(src, m.index!))) continue;
        const line = src.slice(0, m.index!).split("\n").length;
        missing.push(`${relative(SRC, file)}:${line} <${m[1]}>`);
      }
    }
    expect(missing).toEqual([]);
  });
});
