// Logika murni tampilan Graph: kueri log dari pilihan toolbar dan aturan pemilihan commit.
import type { GraphRow, LogQuery, Refs } from "@/ipc";

/** Nilai pilihan cakupan: semua branch, branch aktif, atau `ref:<nama>`. */
export type Scope = "all" | "current" | `ref:${string}`;

export function buildQuery(scope: string, grep: string, author: string): LogQuery {
  const query: LogQuery = {};
  if (scope === "all") query.all = true;
  else if (scope.startsWith("ref:")) query.revs = [scope.slice(4)];
  if (grep.trim()) query.grep = grep.trim();
  if (author.trim()) query.author = author.trim();
  return query;
}

/** Cakupan `ref:<nama>` masih menunjuk ref yang ada. */
export function scopeExists(scope: string, refs: Refs | null): boolean {
  if (!scope.startsWith("ref:") || !refs) return true;
  const name = scope.slice(4);
  return refs.branches.some((b) => b.name === name) || refs.remoteBranches.some((b) => b.name === name) || refs.tags.some((t) => t.name === name);
}

/**
 * Pilihan berikutnya setelah sebuah commit diklik. Klik biasa memilih satu commit;
 * Ctrl/Cmd-klik menambah atau melepas commit kedua (paling banyak dua, untuk perbandingan).
 */
export function nextSelection(previous: readonly string[], oid: string, additive: boolean): string[] {
  if (!additive) return [oid];
  if (previous.includes(oid)) return previous.filter((o) => o !== oid);
  if (previous.length === 0) return [oid];
  return [previous[0], oid];
}

/**
 * Mengurutkan dua commit terpilih menjadi [lama, baru] menurut posisinya di daftar
 * (baris lebih bawah = lebih lama). Bila salah satunya belum dimuat, urutan pemilihan dipakai.
 */
export function orderOldestFirst(rows: readonly GraphRow[], oids: readonly string[]): string[] {
  const index = new Map<string, number>();
  for (let i = 0; i < rows.length && index.size < oids.length; i++) {
    if (oids.includes(rows[i].commit.oid)) index.set(rows[i].commit.oid, i);
  }
  if (oids.some((o) => !index.has(o))) return [...oids];
  return [...oids].sort((a, b) => index.get(b)! - index.get(a)!);
}
