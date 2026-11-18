import { describe, expect, it } from "vitest";
import { Text } from "@codemirror/state";
import { Chunk } from "@codemirror/merge";
import { chunkToOp, lineRange, selectionOps } from "./lines";

const doc = (s: string) => Text.of(s.split("\n"));

/** Model yang sama dengan backend (`splice_forward`): A dengan rentang baris diganti baris dari B. */
function splice(a: string, b: string, ops: { aFrom: number; aTo: number; bFrom: number; bTo: number }[]): string {
  const al = a.split("\n");
  const bl = b.split("\n");
  const out: string[] = [];
  let cursor = 0;
  for (const op of ops) {
    out.push(...al.slice(cursor, op.aFrom), ...bl.slice(op.bFrom, op.bTo));
    cursor = op.aTo;
  }
  out.push(...al.slice(cursor));
  return out.join("\n");
}

function ops(a: string, b: string) {
  const da = doc(a);
  const db = doc(b);
  return Chunk.build(da, db).map((c) => chunkToOp(c, da, db));
}

describe("chunk → rentang baris", () => {
  it("menerapkan semua chunk menghasilkan B", () => {
    const cases: [string, string][] = [
      ["1\n2\n3\n4\n5\n", "1\nDUA\n3\n4\nLIMA\nENAM\n"],
      ["a\nb\n", "a\nb\nc\n"],
      ["a\nb", "a\nb\nc"],
      ["a\nb", "a\nb\nc\n"],
      ["a\nb\n", "a\nb"],
      ["a\nb\nc\n", "a\nc\n"],
      ["a\nb\nc\n", "b\nc\n"],
      ["a\nb\nc\n", "a\nb\n"],
      ["", "x\ny\n"],
      ["x\ny\n", ""],
      ["same\n", "same\n"],
      ["a\n\n\nb\n", "a\nb\n"],
      ["fn a() {\n}\n\nfn b() {\n}\n", "fn a() {\n}\n\nfn x() {\n}\n\nfn b() {\n}\n"],
    ];
    for (const [a, b] of cases) {
      expect(splice(a, b, ops(a, b)), JSON.stringify([a, b])).toBe(b);
    }
  });

  it("menerapkan satu chunk saja hanya mengubah bagian itu", () => {
    const a = "1\n2\n3\n4\n5\n6\n7\n8\n9\n";
    const b = "1\nDUA\n3\n4\n5\n6\n7\nDELAPAN\n9\n";
    const all = ops(a, b);
    expect(all).toHaveLength(2);
    expect(splice(a, b, [all[0]])).toBe("1\nDUA\n3\n4\n5\n6\n7\n8\n9\n");
    expect(splice(a, b, [all[1]])).toBe("1\n2\n3\n4\n5\n6\n7\nDELAPAN\n9\n");
  });

  it("chunk kosong di ujung dokumen berada setelah baris terakhir", () => {
    const d = doc("a\nb");
    expect(lineRange(d, d.length + 1, d.length + 1)).toEqual([2, 2]);
    expect(lineRange(d, 0, 0)).toEqual([0, 0]);
    expect(lineRange(d, 0, 2)).toEqual([0, 1]);
    expect(lineRange(d, 2, d.length + 1)).toEqual([1, 2]);
  });
});

describe("pilihan baris", () => {
  const a = "1\n2\n3\n";
  const b = "1\nA\nB\nC\n3\n";
  const da = doc(a);
  const db = doc(b);
  const chunks = Chunk.build(da, db);

  it("pilihan sebagian menyisipkan baris terpilih setelah baris lama", () => {
    // Hanya baris "B" (indeks 2).
    const picked = selectionOps(chunks, da, db, 2, 3);
    expect(picked).toEqual([{ aFrom: 2, aTo: 2, bFrom: 2, bTo: 3 }]);
    expect(splice(a, b, picked)).toBe("1\n2\nB\n3\n");
  });

  it("pilihan yang mencakup seluruh chunk memakai chunk utuh", () => {
    const picked = selectionOps(chunks, da, db, 0, 5);
    expect(splice(a, b, picked)).toBe(b);
  });

  it("pilihan di luar chunk tidak menghasilkan operasi", () => {
    expect(selectionOps(chunks, da, db, 0, 1)).toEqual([]);
  });

  it("penghapusan murni terpilih bila pilihan menyentuh posisinya", () => {
    const a2 = "1\n2\n3\n";
    const b2 = "1\n3\n";
    const da2 = doc(a2);
    const db2 = doc(b2);
    const c2 = Chunk.build(da2, db2);
    expect(splice(a2, b2, selectionOps(c2, da2, db2, 1, 1))).toBe(b2);
    expect(selectionOps(c2, da2, db2, 2, 2)).toEqual([]);
  });
});
