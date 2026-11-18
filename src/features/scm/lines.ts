// Konversi rentang karakter chunk diff (CodeMirror) ke rentang baris untuk staging parsial.
import type { Text } from "@codemirror/state";
import type { Chunk } from "@codemirror/merge";
import type { SpliceOp } from "@/ipc";

/**
 * Rentang baris `[awal, akhir)` (mulai dari 0) untuk rentang karakter sebuah chunk.
 * Chunk kosong di ujung dokumen berada setelah baris terakhir.
 */
export function lineRange(doc: Text, from: number, to: number): [number, number] {
  if (from > doc.length) return [doc.lines, doc.lines];
  const start = doc.lineAt(from).number - 1;
  if (to <= from) return [start, start];
  const end = doc.lineAt(Math.min(to - 1, doc.length)).number;
  return [start, end];
}

/** Operasi sambung-potong untuk seluruh chunk. */
export function chunkToOp(chunk: Chunk, a: Text, b: Text): SpliceOp {
  const [aFrom, aTo] = lineRange(a, chunk.fromA, chunk.toA);
  const [bFrom, bTo] = lineRange(b, chunk.fromB, chunk.toB);
  return { aFrom, aTo, bFrom, bTo };
}

/**
 * Operasi untuk baris terpilih saja di sisi B (`[selFrom, selTo)`, nomor baris mulai dari 0).
 * Mengikuti urutan patch Git: baris lama dipertahankan, baris baru yang dipilih disisipkan
 * setelahnya. Bila pilihan mencakup seluruh sisi B chunk, seluruh chunk yang dipakai.
 */
export function selectionOps(chunks: readonly Chunk[], a: Text, b: Text, selFrom: number, selTo: number): SpliceOp[] {
  const ops: SpliceOp[] = [];
  for (const chunk of chunks) {
    const full = chunkToOp(chunk, a, b);
    if (full.bFrom === full.bTo) {
      // Penghapusan murni: terpilih bila kursor/pilihan menyentuh posisinya.
      if (selFrom <= full.bFrom && full.bFrom <= selTo) ops.push(full);
      continue;
    }
    const from = Math.max(full.bFrom, selFrom);
    const to = Math.min(full.bTo, selTo);
    if (from >= to) continue;
    if (from === full.bFrom && to === full.bTo) ops.push(full);
    else ops.push({ aFrom: full.aTo, aTo: full.aTo, bFrom: from, bTo: to });
  }
  return ops;
}
