// State editor konflik: rentang tiap konflik di dokumen hasil, pilihan sisi, dan dekorasinya.
import { EditorState, StateEffect, StateField, type Extension, type Transaction } from "@codemirror/state";
import { Decoration, EditorView, WidgetType, type DecorationSet } from "@codemirror/view";
import type { MergeSegment } from "@/ipc";

export interface ConflictRange {
  index: number;
  from: number;
  to: number;
  resolved: boolean;
  current: string;
  incoming: string;
  base: string;
}

export type ConflictChoice = "current" | "incoming" | "both" | "base";

export interface InitialResult {
  /** Isi awal panel Hasil: bagian yang sudah tergabung, dengan sisi current pada tiap konflik. */
  text: string;
  conflicts: ConflictRange[];
  /** Rentang karakter tiap konflik pada berkas sisi current dan incoming, untuk penyorotan. */
  currentRanges: [number, number][];
  incomingRanges: [number, number][];
}

/** CodeMirror memakai `\n` sebagai pemisah baris; panjang dihitung setelah normalisasi. */
function normalize(text: string): string {
  return text.replace(/\r\n?/g, "\n");
}

/** Menyusun dokumen hasil awal dan posisi konflik dari segmen penggabungan tiga arah. */
export function buildInitialResult(segments: MergeSegment[]): InitialResult {
  let text = "";
  let currentOffset = 0;
  let incomingOffset = 0;
  const conflicts: ConflictRange[] = [];
  const currentRanges: [number, number][] = [];
  const incomingRanges: [number, number][] = [];
  for (const segment of segments) {
    if (segment.kind === "resolved") {
      const part = normalize(segment.text);
      text += part;
      currentOffset += part.length;
      incomingOffset += part.length;
    } else {
      const current = normalize(segment.current);
      const incoming = normalize(segment.incoming);
      conflicts.push({ index: conflicts.length, from: text.length, to: text.length + current.length, resolved: false, current, incoming, base: normalize(segment.base) });
      currentRanges.push([currentOffset, currentOffset + current.length]);
      incomingRanges.push([incomingOffset, incomingOffset + incoming.length]);
      text += current;
      currentOffset += current.length;
      incomingOffset += incoming.length;
    }
  }
  return { text, conflicts, currentRanges, incomingRanges };
}

export function choiceText(conflict: ConflictRange, choice: ConflictChoice): string {
  switch (choice) {
    case "current":
      return conflict.current;
    case "incoming":
      return conflict.incoming;
    case "base":
      return conflict.base;
    case "both": {
      const first = conflict.current;
      // Pastikan kedua sisi tidak menyatu dalam satu baris.
      return first && !first.endsWith("\n") && conflict.incoming ? `${first}\n${conflict.incoming}` : first + conflict.incoming;
    }
  }
}

const setRange = StateEffect.define<{ index: number; from: number; to: number; resolved: boolean }>();
const setActive = StateEffect.define<number>();

function mapConflicts(conflicts: readonly ConflictRange[], tr: Transaction): ConflictRange[] {
  let next = conflicts as ConflictRange[];
  if (tr.docChanged) {
    const explicit = new Set(tr.effects.filter((e) => e.is(setRange)).map((e) => e.value.index));
    next = conflicts.map((c) => {
      if (explicit.has(c.index)) return c;
      const from = tr.changes.mapPos(c.from, 1);
      const to = Math.max(from, tr.changes.mapPos(c.to, -1));
      // Suntingan manual di dalam sebuah konflik berarti pengguna sudah memutuskan isinya.
      const touched = !c.resolved && tr.changes.touchesRange(c.from, c.to) !== false && !tr.isUserEvent("oncode.reload");
      return { ...c, from, to, resolved: c.resolved || touched };
    });
  }
  for (const effect of tr.effects) {
    if (effect.is(setRange)) next = next.map((c) => (c.index === effect.value.index ? { ...c, ...effect.value } : c));
  }
  return next;
}

export const conflictField = StateField.define<ConflictRange[]>({
  create: () => [],
  update: (value, tr) => mapConflicts(value, tr),
});

export const activeConflictField = StateField.define<number>({
  create: () => 0,
  update(value, tr) {
    for (const effect of tr.effects) if (effect.is(setActive)) return effect.value;
    return value;
  },
});

class ConflictWidget extends WidgetType {
  constructor(
    readonly index: number,
    readonly resolved: boolean,
    readonly total: number,
    readonly hasBase: boolean,
  ) {
    super();
  }
  eq(other: ConflictWidget) {
    return other.index === this.index && other.resolved === this.resolved && other.total === this.total;
  }
  toDOM(view: EditorView) {
    const box = document.createElement("div");
    box.className = "conflict-widget";
    const label = document.createElement("span");
    label.dataset.state = this.resolved ? "resolved" : "unresolved";
    label.textContent = `Conflict ${this.index + 1} of ${this.total} · ${this.resolved ? "resolved" : "unresolved"}`;
    box.appendChild(label);
    const spacer = document.createElement("span");
    spacer.style.flex = "1";
    box.appendChild(spacer);
    const choices: [ConflictChoice, string][] = [
      ["current", "Use Current"],
      ["incoming", "Use Incoming"],
      ["both", "Use Both"],
    ];
    if (this.hasBase) choices.push(["base", "Use Base"]);
    for (const [choice, text] of choices) {
      const button = document.createElement("button");
      button.textContent = text;
      button.onmousedown = (e) => e.preventDefault();
      button.onclick = () => chooseConflict(view, this.index, choice);
      box.appendChild(button);
    }
    return box;
  }
  ignoreEvent() {
    return true;
  }
}

function decorations(state: EditorState): DecorationSet {
  const conflicts = state.field(conflictField);
  const active = state.field(activeConflictField);
  const ranges = [];
  for (const c of conflicts) {
    const from = Math.min(c.from, state.doc.length);
    ranges.push(Decoration.widget({ widget: new ConflictWidget(c.index, c.resolved, conflicts.length, c.base.length > 0), block: true, side: -1 }).range(from));
    const cls = `${c.resolved ? "cm-conflict-resolved" : "cm-conflict-unresolved"}${c.index === active ? " cm-conflict-active" : ""}`;
    for (let pos = from; pos < c.to && pos <= state.doc.length; ) {
      const line = state.doc.lineAt(pos);
      ranges.push(Decoration.line({ class: cls }).range(line.from));
      pos = line.to + 1;
    }
  }
  return Decoration.set(ranges, true);
}

const decorationField = StateField.define<DecorationSet>({
  create: decorations,
  update(value, tr) {
    if (tr.docChanged || tr.startState.field(conflictField) !== tr.state.field(conflictField) || tr.startState.field(activeConflictField) !== tr.state.field(activeConflictField)) {
      return decorations(tr.state);
    }
    return value;
  },
  provide: (field) => EditorView.decorations.from(field),
});

/** Ekstensi untuk panel Hasil. */
export function conflictExtension(initial: ConflictRange[]): Extension {
  return [conflictField.init(() => initial), activeConflictField, decorationField];
}

/** Mengganti isi sebuah konflik dengan sisi pilihan dan menandainya selesai. */
export function chooseConflict(view: EditorView, index: number, choice: ConflictChoice): void {
  const conflict = view.state.field(conflictField).find((c) => c.index === index);
  if (!conflict) return;
  const insert = choiceText(conflict, choice);
  view.dispatch({
    changes: { from: conflict.from, to: conflict.to, insert },
    effects: [setRange.of({ index, from: conflict.from, to: conflict.from + insert.length, resolved: true }), setActive.of(index)],
    userEvent: "input.conflict",
  });
}

export function setActiveConflict(view: EditorView, index: number): void {
  const conflict = view.state.field(conflictField).find((c) => c.index === index);
  if (!conflict) return;
  view.dispatch({ effects: [setActive.of(index), EditorView.scrollIntoView(Math.min(conflict.from, view.state.doc.length), { y: "center" })] });
}

export function unresolvedCount(state: EditorState): number {
  return state.field(conflictField).filter((c) => !c.resolved).length;
}

/** Dekorasi penyorot rentang konflik pada panel sisi (current/incoming). */
export function sideHighlight(ranges: [number, number][], className: string): Extension {
  return EditorView.decorations.compute([], (state) => {
    const marks = [];
    for (const [from, to] of ranges) {
      for (let pos = Math.min(from, state.doc.length); pos < to && pos <= state.doc.length; ) {
        const line = state.doc.lineAt(pos);
        marks.push(Decoration.line({ class: className }).range(line.from));
        pos = line.to + 1;
      }
    }
    return Decoration.set(marks, true);
  });
}
