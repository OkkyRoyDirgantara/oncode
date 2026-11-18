import { describe, expect, it } from "vitest";
import { EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import type { MergeSegment } from "@/ipc";
import { buildInitialResult, chooseConflict, choiceText, conflictExtension, conflictField, unresolvedCount } from "./conflictState";

const segments: MergeSegment[] = [
  { kind: "resolved", text: "timeout = 60\n" },
  { kind: "conflict", current: "retries = 4\n", base: "retries = 3\n", incoming: "retries = 5\n" },
  { kind: "resolved", text: 'log = "debug"\n' },
  { kind: "conflict", current: "a\nb\n", base: "", incoming: "" },
];

function editor() {
  const initial = buildInitialResult(segments);
  const view = new EditorView({ state: EditorState.create({ doc: initial.text, extensions: conflictExtension(initial.conflicts) }) });
  return { view, initial };
}

describe("editor konflik", () => {
  it("menyusun dokumen awal dengan sisi current dan mencatat posisi tiap konflik", () => {
    const initial = buildInitialResult(segments);
    expect(initial.text).toBe('timeout = 60\nretries = 4\nlog = "debug"\na\nb\n');
    expect(initial.conflicts.map((c) => initial.text.slice(c.from, c.to))).toEqual(["retries = 4\n", "a\nb\n"]);
    // Sisi incoming lebih pendek di konflik kedua; rentangnya dihitung pada berkas incoming.
    const incomingFile = 'timeout = 60\nretries = 5\nlog = "debug"\n';
    expect(initial.incomingRanges.map(([f, t]) => incomingFile.slice(f, t))).toEqual(["retries = 5\n", ""]);
    const currentFile = 'timeout = 60\nretries = 4\nlog = "debug"\na\nb\n';
    expect(initial.currentRanges.map(([f, t]) => currentFile.slice(f, t))).toEqual(["retries = 4\n", "a\nb\n"]);
  });

  it("menormalkan CRLF agar posisi cocok dengan dokumen editor", () => {
    const initial = buildInitialResult([
      { kind: "resolved", text: "x\r\n" },
      { kind: "conflict", current: "ours\r\n", base: "", incoming: "theirs\r\n" },
    ]);
    expect(initial.text).toBe("x\nours\n");
    expect(initial.conflicts[0]).toMatchObject({ from: 2, to: 7 });
  });

  it("memilih sisi mengganti isi, menandai selesai, dan menggeser konflik berikutnya", () => {
    const { view } = editor();
    expect(unresolvedCount(view.state)).toBe(2);
    chooseConflict(view, 0, "incoming");
    expect(view.state.doc.toString()).toBe('timeout = 60\nretries = 5\nlog = "debug"\na\nb\n');
    chooseConflict(view, 0, "both");
    expect(view.state.doc.toString()).toBe('timeout = 60\nretries = 4\nretries = 5\nlog = "debug"\na\nb\n');
    const [first, second] = view.state.field(conflictField);
    expect(first.resolved).toBe(true);
    expect(view.state.sliceDoc(second.from, second.to)).toBe("a\nb\n");
    expect(unresolvedCount(view.state)).toBe(1);

    // Sisi incoming kosong (baris dihapus): rentang menjadi kosong tetapi tetap bisa dipilih ulang.
    chooseConflict(view, 1, "incoming");
    expect(view.state.doc.toString()).toBe('timeout = 60\nretries = 4\nretries = 5\nlog = "debug"\n');
    chooseConflict(view, 1, "current");
    expect(view.state.doc.toString().endsWith('log = "debug"\na\nb\n')).toBe(true);
    expect(unresolvedCount(view.state)).toBe(0);
  });

  it("suntingan manual di dalam konflik menandainya selesai; suntingan di luar tidak", () => {
    const { view, initial } = editor();
    view.dispatch({ changes: { from: 0, insert: "# komentar\n" } });
    expect(unresolvedCount(view.state)).toBe(2);
    const shifted = view.state.field(conflictField)[0];
    expect(shifted.from).toBe(initial.conflicts[0].from + "# komentar\n".length);
    expect(view.state.sliceDoc(shifted.from, shifted.to)).toBe("retries = 4\n");

    view.dispatch({ changes: { from: shifted.from + 10, to: shifted.from + 11, insert: "9" } });
    const edited = view.state.field(conflictField)[0];
    expect(edited.resolved).toBe(true);
    expect(view.state.sliceDoc(edited.from, edited.to)).toBe("retries = 9\n");
    expect(unresolvedCount(view.state)).toBe(1);
  });

  it("'both' tidak menyatukan dua sisi dalam satu baris", () => {
    const conflict = { index: 0, from: 0, to: 0, resolved: false, current: "tanpa newline", incoming: "kedua\n", base: "" };
    expect(choiceText(conflict, "both")).toBe("tanpa newline\nkedua\n");
    expect(choiceText({ ...conflict, current: "a\n" }, "both")).toBe("a\nkedua\n");
    expect(choiceText({ ...conflict, base: "asal\n" }, "base")).toBe("asal\n");
  });
});
