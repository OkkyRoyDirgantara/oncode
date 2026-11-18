import { describe, expect, it } from "vitest";
import { Chunk } from "@codemirror/merge";
import { Text } from "@codemirror/state";
import { changeMarks, formatBlameDate, repoForPath } from "./gutter";

const doc = (s: string) => Text.of(s.split("\n"));
const marks = (a: string, b: string) => Object.fromEntries(changeMarks(Chunk.build(doc(a), doc(b)), doc(b)));

describe("penanda perubahan di gutter", () => {
  it("membedakan baris baru, baris berubah, dan penghapusan", () => {
    expect(marks("1\n2\n3\n", "1\n2\n3\n")).toEqual({});
    expect(marks("1\n2\n3\n", "1\nDUA\n3\n")).toEqual({ 2: "modified" });
    expect(marks("1\n3\n", "1\n2a\n2b\n3\n")).toEqual({ 2: "added", 3: "added" });
    // Baris "2" dihapus: penanda di baris yang kini menempati posisinya.
    expect(marks("1\n2\n3\n", "1\n3\n")).toEqual({ 2: "deleted" });
  });

  it("menandai penambahan di akhir berkas", () => {
    expect(marks("a\n", "a\nb\n")).toEqual({ 2: "added" });
    // Tanpa newline di akhir, baris terakhir lama ikut berubah (kini diakhiri newline).
    expect(marks("a", "a\nb")).toEqual({ 1: "modified", 2: "modified" });
  });
});

describe("repoForPath", () => {
  it("memilih repositori terdalam dan mengembalikan path relatif", () => {
    const roots = ["/p/app", "/p/app/vendor/lib"];
    expect(repoForPath("/p/app/src/main.rs", roots)).toEqual({ root: "/p/app", relative: "src/main.rs" });
    expect(repoForPath("/p/app/vendor/lib/x.c", roots)).toEqual({ root: "/p/app/vendor/lib", relative: "x.c" });
    expect(repoForPath("/p/app-lain/x", roots)).toBeNull();
    expect(repoForPath("C:\\kode\\app\\src\\a.ts", ["C:\\kode\\app"])).toEqual({ root: "C:\\kode\\app", relative: "src/a.ts" });
  });
});

describe("formatBlameDate", () => {
  it("memakai satuan yang wajar", () => {
    const now = Date.UTC(2026, 10, 17);
    const ago = (days: number) => now / 1000 - days * 86400;
    expect(formatBlameDate(ago(0.2), now)).toBe("today");
    expect(formatBlameDate(ago(1.5), now)).toBe("yesterday");
    expect(formatBlameDate(ago(12), now)).toBe("12 days ago");
    expect(formatBlameDate(ago(95), now)).toBe("3 mo ago");
    expect(formatBlameDate(ago(800), now)).toBe("2 yr ago");
  });
});
