/**
 * Palette / scripting `edit.*` actions: cut, copy, paste and select-all on
 * the last focused text field, through the async clipboard. The native
 * Edit menu needs none of this (its OS roles already acted); a palette run
 * moved focus into the palette, so the shell hands over the field the user
 * was in. Every outcome is a status line, including "nothing to do".
 */

/** The slice of an `<input>` / `<textarea>` an edit needs. */
export interface EditField {
  value: string;
  selectionStart: number | null;
  selectionEnd: number | null;
  /** Shown in the status line (the field's data-testid, else "text field"). */
  label: string;
  setRangeText(text: string, start: number, end: number, mode?: "end"): void;
  select(): void;
  focus(): void;
  /** Tell the field's owner its value changed (an `input` event). */
  notifyInput(): void;
}

export interface EditClipboard {
  writeText(text: string): Promise<void>;
  readText(): Promise<string>;
}

export const EDIT_ACTIONS = ["edit.cut", "edit.copy", "edit.paste", "edit.select_all"] as const;

const VERB: Record<string, string> = {
  "edit.cut": "cut",
  "edit.copy": "copy",
  "edit.paste": "paste",
  "edit.select_all": "select all",
};

const chars = (n: number): string => `${n} char${n === 1 ? "" : "s"}`;

export async function runEdit(
  id: string,
  field: EditField | null,
  clipboard: EditClipboard | null,
): Promise<string> {
  if (!field) return `edit: ${VERB[id] ?? id} needs a text field — click one first`;
  field.focus();
  const start = field.selectionStart ?? field.value.length;
  const end = field.selectionEnd ?? start;
  const selected = field.value.slice(start, end);
  if (id === "edit.select_all") {
    field.select();
    return `edit: selected ${chars(field.value.length)} in ${field.label}`;
  }
  if ((id === "edit.copy" || id === "edit.cut") && selected === "") {
    return `edit: nothing selected to ${VERB[id]}`;
  }
  if (!clipboard) return "edit: clipboard unavailable in this window";
  try {
    if (id === "edit.copy") {
      await clipboard.writeText(selected);
      return `edit: copied ${chars(selected.length)}`;
    }
    if (id === "edit.cut") {
      await clipboard.writeText(selected);
      field.setRangeText("", start, end, "end");
      field.notifyInput();
      return `edit: cut ${chars(selected.length)}`;
    }
    if (id === "edit.paste") {
      const text = await clipboard.readText();
      field.setRangeText(text, start, end, "end");
      field.notifyInput();
      return `edit: pasted ${chars(text.length)} into ${field.label}`;
    }
  } catch (e) {
    return `edit: clipboard unavailable: ${e instanceof Error ? e.message : String(e)}`;
  }
  return `edit: unknown action ${id}`;
}

/** Wrap a DOM text field (or nothing) as an `EditField`. */
export function domEditField(el: Element | null): EditField | null {
  if (!(el instanceof HTMLInputElement || el instanceof HTMLTextAreaElement)) return null;
  if (el instanceof HTMLInputElement && !["text", "search", "url", "email", ""].includes(el.type)) {
    return null;
  }
  return {
    get value() {
      return el.value;
    },
    set value(v: string) {
      el.value = v;
    },
    get selectionStart() {
      return el.selectionStart;
    },
    set selectionStart(v: number | null) {
      el.selectionStart = v;
    },
    get selectionEnd() {
      return el.selectionEnd;
    },
    set selectionEnd(v: number | null) {
      el.selectionEnd = v;
    },
    label: el.getAttribute("data-testid") ?? "text field",
    setRangeText: (text, start, end, mode) => el.setRangeText(text, start, end, mode),
    select: () => el.select(),
    focus: () => el.focus(),
    notifyInput: () => el.dispatchEvent(new Event("input", { bubbles: true })),
  };
}
