/** Sandbox rules for user-supplied TS/JS script sources (Track K). */
export const MAX_SCRIPT_SOURCE_CHARS = 64 * 1024;
export const MAX_OPS_PER_SCRIPT = 256;

/** Substrings that never appear in a sandboxed script body. */
const BLOCKED_PATTERNS: { re: RegExp; reason: string }[] = [
  { re: /\bimport\b/, reason: "import statements" },
  { re: /\bexport\b/, reason: "export statements" },
  { re: /\brequire\s*\(/, reason: "require()" },
  { re: /\bprocess\b/, reason: "process" },
  { re: /\bglobalThis\b/, reason: "globalThis" },
  { re: /\bglobal\b/, reason: "Node global" },
  { re: /\bwindow\b/, reason: "window" },
  { re: /\bself\b/, reason: "self" },
  { re: /\beval\s*\(/, reason: "eval()" },
  { re: /\bFunction\s*\(/, reason: "Function()" },
  { re: /__proto__/, reason: "__proto__" },
  { re: /\bconstructor\b/, reason: "constructor" },
  { re: /\bprototype\b/, reason: "prototype" },
  { re: /\bfetch\s*\(/, reason: "fetch()" },
  { re: /\bXMLHttpRequest\b/, reason: "XMLHttpRequest" },
  { re: /\bWebSocket\b/, reason: "WebSocket" },
  { re: /\binvoke\s*\(/, reason: "raw Tauri invoke()" },
  { re: /\bop_apply\b/, reason: "raw op_apply (use the script api instead)" },
];

export class ScriptSandboxError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "ScriptSandboxError";
  }
}

/** Reject sources that escape the script sandbox before compiling. */
export function validateScriptSource(source: string): void {
  if (typeof source !== "string" || source.length === 0) {
    throw new ScriptSandboxError("script source must be a non-empty string");
  }
  if (source.length > MAX_SCRIPT_SOURCE_CHARS) {
    throw new ScriptSandboxError(
      `script too large: ${source.length} chars (max ${MAX_SCRIPT_SOURCE_CHARS})`,
    );
  }
  for (const { re, reason } of BLOCKED_PATTERNS) {
    if (re.test(source)) {
      throw new ScriptSandboxError(`blocked in sandbox: ${reason}`);
    }
  }
}

/** Guard the op budget so one script cannot flood the op log. */
export function checkOpBudget(count: number): void {
  if (count > MAX_OPS_PER_SCRIPT) {
    throw new ScriptSandboxError(
      `too many ops: ${count} (max ${MAX_OPS_PER_SCRIPT} per script)`,
    );
  }
}
