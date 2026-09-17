import { createEffect, onCleanup, onMount } from "solid-js";
import {
  beatToX,
  hitTest,
  notesAt,
  pitchToY,
  pxPerBeat,
  rowH,
  snapBeat,
  xToBeat,
  yToPitch,
  type ViewConfig,
} from "./geometry";
import type { MidiClip } from "./model";
import { createNote, moveNote, resizeNote, sweepDelete } from "./store";

/**
 * FL-style piano roll on a hot canvas.
 *
 * - Left-click empty space: create a snapped note.
 * - Drag a note body: move it (pitch + time, snapped).
 * - Drag a note's right edge: resize its length.
 * - Right-button press/sweep: delete notes under the cursor.
 *
 * Hot-canvas contract: Solid signals commit finished edits (`onChange`) but
 * never participate in per-frame drawing. The rAF loop reads a plain mutable
 * snapshot (`frame`) refreshed by one `createEffect`, so framework code stays
 * out of the draw path.
 */
export default function PianoRoll(props: {
  clip: MidiClip;
  beatsVisible?: number;
  snap?: number;
  onChange?: (next: MidiClip) => void;
}) {
  let canvas: HTMLCanvasElement | undefined;
  let raf = 0;
  const snap = () => props.snap ?? 0.25;

  // Mutable per-frame state: NOT signals — the rAF loop owns these.
  const frame: {
    notes: MidiClip["notes"];
    hover: number | null;
    drag: null | {
      mode: "create" | "move" | "resize" | "delete";
      noteId: number | null;
      grabBeatOffset: number;
      grabPitchOffset: number;
      curBeat: number;
      curPitch: number;
      swept: number[];
    };
    scrollBeats: number;
  } = { notes: [], hover: null, drag: null, scrollBeats: 0 };

  /** Piano-key strip width (CSS px): pitch names live here, the grid starts right of it. */
  const KEY_W = 64;

  const view = (): ViewConfig => ({
    width: Math.max(80, (canvas?.clientWidth || 640) - KEY_W),
    height: canvas?.clientHeight || 320,
    beatsVisible: props.beatsVisible ?? 8,
    scrollBeats: frame.scrollBeats,
  });

  function commit(next: MidiClip): void {
    frame.notes = next.notes;
    props.onChange?.(next);
  }

  function currentClip(): MidiClip {
    return { length_beats: props.clip.length_beats, notes: frame.notes };
  }

  function pos(e: PointerEvent): { x: number; y: number } {
    const r = canvas!.getBoundingClientRect();
    // Grid coordinates start right of the key strip; strip clicks never edit.
    return { x: e.clientX - r.left - KEY_W, y: e.clientY - r.top };
  }

  function draw(): void {
    const ctx = canvas?.getContext("2d");
    if (!ctx || !canvas) {
      raf = requestAnimationFrame(draw);
      return;
    }
    const v = view();
    const dpr = window.devicePixelRatio || 1;
    const w = v.width;
    const h = v.height;
    if (canvas.width !== Math.round(w * dpr) || canvas.height !== Math.round(h * dpr)) {
      canvas.width = Math.round(w * dpr);
      canvas.height = Math.round(h * dpr);
    }
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    // Background + beat grid (bar lines brighter every 4 beats).
    // Canvas palette mirrors theme.css (MIDI notes read steel-blue,
    // selection reads amber, per the studio theme).
    ctx.fillStyle = "#0d1117";
    ctx.fillRect(0, 0, w + KEY_W, h);
    // Everything grid-aligned draws in translated space; the key strip is
    // painted last, on top, in canvas space.
    ctx.save();
    ctx.translate(KEY_W, 0);
    const ppb = pxPerBeat(v);
    const firstLine = Math.floor(v.scrollBeats / snap()) * snap();
    ctx.strokeStyle = "rgba(255,255,255,0.08)";
    ctx.beginPath();
    for (let b = firstLine; b <= v.scrollBeats + v.beatsVisible + snap(); b += snap()) {
      const x = Math.round(beatToX(b, v)) + 0.5;
      ctx.moveTo(x, 0);
      ctx.lineTo(x, h);
    }
    ctx.stroke();
    ctx.strokeStyle = "rgba(255,255,255,0.16)";
    ctx.beginPath();
    for (let b = Math.floor(v.scrollBeats / 4) * 4; b <= v.scrollBeats + v.beatsVisible; b += 4) {
      const x = Math.round(beatToX(b, v)) + 0.5;
      ctx.moveTo(x, 0);
      ctx.lineTo(x, h);
    }
    ctx.stroke();
    // Black-key rows shaded (pure visual, no model read).
    const rh = rowH(v);
    ctx.fillStyle = "#141a22";
    for (let p = 0; p <= 127; p++) {
      if ([1, 3, 6, 8, 10].includes(p % 12)) {
        ctx.fillRect(0, pitchToY(p, v), w, rh);
      }
    }
    // Notes.
    for (const n of frame.notes) {
      const x = beatToX(n.start_beats, v);
      const y = pitchToY(n.pitch, v);
      const nw = Math.max(2, n.len_beats * ppb);
      const selected = frame.hover === n.note_id;
      ctx.fillStyle = n.muted ? "#5f6a75" : selected ? "#f5a623" : "#6f9fd8";
      ctx.fillRect(x, y + 1, nw, rh - 2);
      ctx.fillStyle = "rgba(255,255,255,0.35)";
      ctx.fillRect(x + nw - 4, y + 1, 3, rh - 2);
    }
    // Drag preview (create/move/resize ghost) — drawn, never stored.
    const d = frame.drag;
    if (d && d.mode !== "delete") {
      const sb = snapBeat(d.curBeat, snap());
      ctx.fillStyle = "rgba(111,159,216,0.35)";
      if (d.mode === "create") {
        const y = pitchToY(d.curPitch, v);
        ctx.fillRect(beatToX(sb, v), y + 1, snap() * ppb, rh - 2);
      } else if (d.noteId != null) {
        const n = frame.notes.find((m) => m.note_id === d.noteId);
        if (n) {
          if (d.mode === "move") {
            ctx.fillRect(
              beatToX(sb + d.grabBeatOffset, v),
              pitchToY(d.curPitch + d.grabPitchOffset, v) + 1,
              Math.max(2, n.len_beats * ppb),
              rh - 2,
            );
          } else {
            const x = beatToX(n.start_beats, v);
            const nl = Math.max(1 / 16, sb - n.start_beats);
            ctx.fillRect(x, pitchToY(n.pitch, v) + 1, nl * ppb, rh - 2);
          }
        }
      }
    }
    ctx.restore();
    drawKeyStrip(ctx, v, h);
    raf = requestAnimationFrame(draw);
  }

  /** Piano-key strip: dark keys shaded, every C labeled (C4 = MIDI 60). */
  function drawKeyStrip(ctx: CanvasRenderingContext2D, v: ViewConfig, h: number): void {
    const rh = rowH(v);
    ctx.fillStyle = "#141a22";
    ctx.fillRect(0, 0, KEY_W, h);
    ctx.strokeStyle = "rgba(255,255,255,0.14)";
    ctx.beginPath();
    ctx.moveTo(KEY_W + 0.5, 0);
    ctx.lineTo(KEY_W + 0.5, h);
    ctx.stroke();
    const NAMES = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
    ctx.font = "10px system-ui";
    ctx.textBaseline = "middle";
    for (let p = 0; p < 128; p++) {
      const y = pitchToY(p, v);
      if (y + rh < 0 || y > h) continue;
      const black = [1, 3, 6, 8, 10].includes(p % 12);
      if (black) {
        ctx.fillStyle = "rgba(0,0,0,0.45)";
        ctx.fillRect(0, y, KEY_W, rh + 1);
      }
      if (p % 12 === 0) {
        const octave = Math.floor(p / 12) - 1;
        ctx.fillStyle = "#9aa3ad";
        ctx.fillText(`C${octave}`, 8, y + rh / 2);
      } else if (!black && rh >= 14) {
        ctx.fillStyle = "#5f6a75";
        ctx.fillText(NAMES[p % 12], 8, y + rh / 2);
      }
    }
  }

  function onPointerDown(e: PointerEvent): void {
    if (!canvas) return;
    canvas.setPointerCapture(e.pointerId);
    const { x, y } = pos(e);
    if (x < 0) return; // key strip: look, don't touch.
    const v = view();
    const beat = snapBeat(xToBeat(x, v), snap());
    const pitch = yToPitch(y, v);
    if (e.button === 2) {
      // Right-sweep delete: remove notes under the cursor immediately and
      // keep deleting as the pointer sweeps.
      const victims = notesAt(frame.notes, x, y, v).map((n) => n.note_id);
      frame.drag = {
        mode: "delete",
        noteId: null,
        grabBeatOffset: 0,
        grabPitchOffset: 0,
        curBeat: beat,
        curPitch: pitch,
        swept: victims,
      };
      if (victims.length > 0) commit(sweepDelete(currentClip(), victims));
      e.preventDefault();
      return;
    }
    if (e.button !== 0) return;
    const hit = hitTest(frame.notes, x, y, v);
    if (hit.kind === "empty") {
      frame.drag = {
        mode: "create",
        noteId: null,
        grabBeatOffset: 0,
        grabPitchOffset: 0,
        curBeat: beat,
        curPitch: pitch,
        swept: [],
      };
    } else if (hit.kind === "resize") {
      frame.drag = {
        mode: "resize",
        noteId: hit.note.note_id,
        grabBeatOffset: 0,
        grabPitchOffset: 0,
        curBeat: beat,
        curPitch: pitch,
        swept: [],
      };
    } else {
      frame.drag = {
        mode: "move",
        noteId: hit.note.note_id,
        grabBeatOffset: hit.note.start_beats - beat,
        grabPitchOffset: hit.note.pitch - pitch,
        curBeat: beat,
        curPitch: pitch,
        swept: [],
      };
    }
  }

  function onPointerMove(e: PointerEvent): void {
    if (!canvas) return;
    const { x, y } = pos(e);
    const v = view();
    const beat = xToBeat(x, v);
    const pitch = yToPitch(y, v);
    const d = frame.drag;
    if (!d) {
      const hit = hitTest(frame.notes, x, y, v);
      frame.hover = hit.kind === "empty" ? null : hit.note.note_id;
      return;
    }
    d.curBeat = beat;
    d.curPitch = pitch;
    if (d.mode === "delete") {
      const victims = notesAt(frame.notes, x, y, v)
        .map((n) => n.note_id)
        .filter((id) => !d.swept.includes(id));
      if (victims.length > 0) {
        d.swept.push(...victims);
        commit(sweepDelete(currentClip(), victims));
      }
    }
  }

  function onPointerUp(e: PointerEvent): void {
    const d = frame.drag;
    frame.drag = null;
    if (!d || d.mode === "delete") return;
    const sb = snapBeat(d.curBeat, snap());
    try {
      if (d.mode === "create") {
        // Click without drag still creates: commit at press position.
        commit(createNote(currentClip(), d.curPitch, sb, { snap: snap() }));
      } else if (d.mode === "move" && d.noteId != null) {
        commit(moveNote(currentClip(), d.noteId, d.curPitch + d.grabPitchOffset, sb + d.grabBeatOffset));
      } else if (d.mode === "resize" && d.noteId != null) {
        const n = frame.notes.find((m) => m.note_id === d.noteId);
        if (n) commit(resizeNote(currentClip(), d.noteId, Math.max(1 / 16, sb - n.start_beats)));
      }
    } catch {
      // Invalid edit (e.g. unknown id after a concurrent delete): keep state.
    }
    if (e.pointerId !== undefined && canvas?.hasPointerCapture(e.pointerId)) {
      canvas.releasePointerCapture(e.pointerId);
    }
  }

  onMount(() => {
    frame.notes = props.clip.notes;
    raf = requestAnimationFrame(draw);
    const onCtx = (e: Event) => e.preventDefault();
    canvas?.addEventListener("contextmenu", onCtx);
    onCleanup(() => {
      cancelAnimationFrame(raf);
      canvas?.removeEventListener("contextmenu", onCtx);
    });
  });

  // One-way sync: prop commits refresh the frame snapshot. Per-frame reads
  // never touch the signal — this effect is the only reactive code.
  createEffect(() => {
    frame.notes = props.clip.notes;
  });

  return (
    <canvas
      ref={canvas}
      aria-label="Piano roll"
      style={{ width: "100%", flex: "1 1 auto", "min-height": "320px", display: "block", cursor: "crosshair" }}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
    />
  );
}
