import { describe, expect, test } from "bun:test";
import { runEdit, type EditClipboard, type EditField } from "../src/input/edit";

function field(value: string, start = 0, end = start): EditField & { inputs: number } {
  const f = {
    value,
    selectionStart: start,
    selectionEnd: end,
    inputs: 0,
    label: "project-name",
    setRangeText(text: string, s: number, e: number) {
      f.value = f.value.slice(0, s) + text + f.value.slice(e);
      f.selectionStart = f.selectionEnd = s + text.length;
    },
    select() {
      f.selectionStart = 0;
      f.selectionEnd = f.value.length;
    },
    focus() {},
    notifyInput() {
      f.inputs += 1;
    },
  };
  return f;
}

function clipboard(initial = ""): EditClipboard & { text: string } {
  const c = {
    text: initial,
    writeText: async (t: string) => {
      c.text = t;
    },
    readText: async () => c.text,
  };
  return c;
}

describe("palette Edit actions on the last text field", () => {
  test("select all, copy, then paste into another field", async () => {
    const name = field("hello world");
    const path = field("/tmp/", 5);
    const cb = clipboard();
    expect(await runEdit("edit.select_all", name, cb)).toBe("edit: selected 11 chars in project-name");
    expect(await runEdit("edit.copy", name, cb)).toBe("edit: copied 11 chars");
    expect(cb.text).toBe("hello world");
    expect(await runEdit("edit.paste", path, cb)).toBe("edit: pasted 11 chars into project-name");
    expect(path.value).toBe("/tmp/hello world");
    expect(path.inputs).toBe(1);
  });

  test("cut removes the selection and tells the field it changed", async () => {
    const f = field("abcdef", 1, 3);
    const cb = clipboard();
    expect(await runEdit("edit.cut", f, cb)).toBe("edit: cut 2 chars");
    expect(cb.text).toBe("bc");
    expect(f.value).toBe("adef");
    expect(f.inputs).toBe(1);
  });

  test("nothing selected, no field, or no clipboard says so", async () => {
    const cb = clipboard("keep");
    expect(await runEdit("edit.copy", field("abc", 2), cb)).toBe("edit: nothing selected to copy");
    expect(cb.text).toBe("keep");
    expect(await runEdit("edit.paste", null, cb)).toBe("edit: paste needs a text field — click one first");
    const denied: EditClipboard = {
      writeText: () => Promise.reject(new Error("denied")),
      readText: () => Promise.reject(new Error("denied")),
    };
    expect(await runEdit("edit.copy", field("abc", 0, 3), denied)).toBe("edit: clipboard unavailable: denied");
  });
});
