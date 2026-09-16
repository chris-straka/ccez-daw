import type { MidiClip } from "../pianoroll/model";
import { accidentalGlyph, ledgerLines, type Clef } from "./pitch";
import {
  beatForX,
  displayDuration,
  hasStem,
  layoutClip,
  pageHeight,
  pageSystems,
  stemDown,
  stepForY,
  systemTop,
  type NotationLayoutOpts,
} from "./layout";
import type { NoteSelection } from "./selection";

const CLEF_GLYPH: Record<Clef, string> = { treble: "𝄞", bass: "𝄢" };

/**
 * Print-friendly score view: black-on-white SVG, one staff per system,
 * clef per note pitch (treble >= middle C, bass below).
 *
 * - Click a notehead: `onSelect` with the toggled selection (shift adds).
 * - Click empty staff space: `onCreate` with `{ step, clef, beat }` so the
 *   shell can commit via `createNoteAtStep` (same ids as the piano roll).
 * - `@media print` hides the screen hint and fits the page width; the SVG
 *   is pure vector so it prints sharp at any size.
 */
export default function ScoreView(props: {
  clip: MidiClip;
  selected?: NoteSelection;
  layout?: NotationLayoutOpts;
  onSelect?: (next: NoteSelection) => void;
  onCreate?: (at: { step: number; clef: Clef; beat: number }) => void;
}) {
  const { layout, notes } = layoutClip(props.clip, props.layout);
  const systems = pageSystems(props.clip.length_beats, props.layout);
  const height = pageHeight(props.clip.length_beats, props.layout);
  const sel = () => props.selected ?? new Set<number>();

  function staffLines(system: number): Array<{ x1: number; x2: number; y: number }> {
    const top = systemTop(system, layout);
    const rows: Array<{ x1: number; x2: number; y: number }> = [];
    for (let i = 0; i < 5; i++) {
      const y = top + i * layout.lineGap;
      rows.push({ x1: layout.marginLeft, x2: layout.pageWidth - layout.marginRight, y });
    }
    return rows;
  }

  function barLines(): number[] {
    const xs: number[] = [];
    for (let m = 0; m <= layout.measuresPerSystem; m++) {
      xs.push(layout.marginLeft + m * layout.measureWidth);
    }
    return xs;
  }

  function toggle(id: number, additive: boolean): void {
    const cur = new Set(sel());
    if (additive) {
      if (cur.has(id)) cur.delete(id);
      else cur.add(id);
    } else {
      cur.clear();
      cur.add(id);
    }
    props.onSelect?.(cur);
  }

  function createAt(e: MouseEvent, svg: SVGSVGElement): void {
    const rect = svg.getBoundingClientRect();
    if (rect.width <= 0 || rect.height <= 0) return;
    const x = ((e as MouseEvent).clientX - rect.left) * (layout.pageWidth / rect.width);
    const y = ((e as MouseEvent).clientY - rect.top) * (height / rect.height);
    const rowH = layout.staffHeight + layout.systemGap;
    const system = Math.min(systems - 1, Math.max(0, Math.round((y - layout.topMargin) / rowH)));
    const top = systemTop(system, layout);
    const measureInSystem = Math.min(
      layout.measuresPerSystem - 1,
      Math.max(0, Math.floor((x - layout.marginLeft) / layout.measureWidth)),
    );
    const step = stepForY(y, top, layout);
    const beat =
      system * layout.measuresPerSystem * layout.beatsPerMeasure +
      beatForX(x, measureInSystem, layout);
    // The printed page shows one treble staff per system; the shell commits
    // through `createNoteAtStep` (same ids as the piano roll).
    props.onCreate?.({ step, clef: "treble", beat: Math.max(0, beat) });
  }

  return (
    <div class="ccez-score">
      <style>{`@media print { .ccez-score .screen-hint { display: none; } .ccez-score svg { max-width: 100%; height: auto; } }`}</style>
      <svg
        role="img"
        aria-label="Score"
        width={layout.pageWidth}
        height={height}
        viewBox={`0 0 ${layout.pageWidth} ${height}`}
        style={{ background: "#fff", display: "block", width: "100%", height: "auto" }}
        onClick={(e: MouseEvent) => createAt(e, e.currentTarget as SVGSVGElement)}
      >
        {Array.from({ length: systems }, (_, s) => (
          <g>
            {staffLines(s).map((l) => (
              <line x1={l.x1} x2={l.x2} y1={l.y} y2={l.y} stroke="#000" stroke-width={1} />
            ))}
            {barLines().map((x, i) => (
              <line
                x1={x}
                x2={x}
                y1={systemTop(s, layout)}
                y2={systemTop(s, layout) + layout.staffHeight}
                stroke="#000"
                stroke-width={i === 0 || i === layout.measuresPerSystem ? 2 : 1}
              />
            ))}
            <text
              x={layout.marginLeft - 34}
              y={systemTop(s, layout) + layout.staffHeight - 2}
              font-size="30"
            >
              {CLEF_GLYPH.treble}
            </text>
          </g>
        ))}
        {notes.map((p) => {
          const dur = displayDuration(p.note.len_beats);
          const down = stemDown(p.step);
          const isSel = sel().has(p.note.note_id);
          const rx = 6;
          const ry = 4.4;
          const stemX = down ? p.x - rx : p.x + rx;
          const stemEnd = down ? p.y + 3.2 * layout.lineGap : p.y - 3.2 * layout.lineGap;
          const ledgers = ledgerLines(p.step);
          const ledgerYs: number[] = [];
          for (let k = 1; k <= ledgers; k++) {
            const ls = p.step > 8 ? 8 + k * 2 : 0 - k * 2;
            const top = systemTop(p.system, layout);
            ledgerYs.push(top + ((8 - ls) * layout.lineGap) / 2);
          }
          return (
            <g
              onClick={(e: MouseEvent) => {
                e.stopPropagation();
                toggle(p.note.note_id, (e as MouseEvent & { shiftKey?: boolean }).shiftKey ?? false);
              }}
              style={{ cursor: "pointer" }}
            >
              <title>{`pitch ${p.note.pitch} beat ${p.note.start_beats}`}</title>
              {ledgerYs.map((ly) => (
                <line x1={p.x - 10} x2={p.x + 10} y1={ly} y2={ly} stroke="#000" stroke-width={1} />
              ))}
              {accidentalGlyph(p.note.pitch) && (
                <text x={p.x - 18} y={p.y + 5} font-size="14">
                  {accidentalGlyph(p.note.pitch)}
                </text>
              )}
              <ellipse
                cx={p.x}
                cy={p.y}
                rx={rx}
                ry={ry}
                transform={`rotate(${-18} ${p.x} ${p.y})`}
                fill={isSel ? "#0b5cad" : dur === "whole" || dur === "half" ? "#fff" : "#000"}
                stroke={isSel ? "#0b5cad" : "#000"}
                stroke-width={1.6}
              />
              {hasStem(dur) && (
                <line x1={stemX} x2={stemX} y1={p.y} y2={stemEnd} stroke={isSel ? "#0b5cad" : "#000"} stroke-width={1.4} />
              )}
            </g>
          );
        })}
      </svg>
      <p class="screen-hint session-dim">
        Click empty staff to add a note; click a note to select (shift-click adds); print via
        browser print — vector SVG stays sharp.
      </p>
    </div>
  );
}
