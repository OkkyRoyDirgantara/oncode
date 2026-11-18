// Logika murni panel Problems: hitung, saring, kelompokkan per berkas, dan urutkan.
import type { Diagnostic, Severity } from "@/features/lsp/diagnostics";

export interface ProblemFilter {
  text: string;
  errors: boolean;
  warnings: boolean;
  /** Mencakup `info` dan `hint`. */
  infos: boolean;
}

export const DEFAULT_FILTER: ProblemFilter = { text: "", errors: true, warnings: true, infos: true };

export interface ProblemCounts {
  errors: number;
  warnings: number;
  infos: number;
}

export interface ProblemGroup {
  /** Path absolut. */
  path: string;
  /** Path relatif terhadap folder proyek (dengan garis miring), untuk ditampilkan. */
  relative: string;
  name: string;
  dir: string;
  counts: ProblemCounts;
  items: Diagnostic[];
}

export type ProblemRow = { kind: "file"; group: ProblemGroup } | { kind: "problem"; group: ProblemGroup; diagnostic: Diagnostic };

const SEVERITY_RANK: Record<Severity, number> = { error: 0, warning: 1, info: 2, hint: 3 };

function count(items: readonly Diagnostic[]): ProblemCounts {
  const counts = { errors: 0, warnings: 0, infos: 0 };
  for (const item of items) {
    if (item.severity === "error") counts.errors += 1;
    else if (item.severity === "warning") counts.warnings += 1;
    else counts.infos += 1;
  }
  return counts;
}

/** Jumlah seluruh masalah, tanpa penyaring. */
export function countProblems(files: Record<string, Diagnostic[]>): ProblemCounts {
  const total = { errors: 0, warnings: 0, infos: 0 };
  for (const items of Object.values(files)) {
    const counts = count(items);
    total.errors += counts.errors;
    total.warnings += counts.warnings;
    total.infos += counts.infos;
  }
  return total;
}

function severityShown(severity: Severity, filter: ProblemFilter): boolean {
  if (severity === "error") return filter.errors;
  if (severity === "warning") return filter.warnings;
  return filter.infos;
}

function matchesText(item: Diagnostic, needle: string): boolean {
  return (
    item.message.toLowerCase().includes(needle) ||
    (item.source?.toLowerCase().includes(needle) ?? false) ||
    (item.code?.toLowerCase().includes(needle) ?? false)
  );
}

/**
 * Kelompok per berkas. Di dalam berkas: severity (galat dulu) lalu posisi. Antarberkas: yang punya
 * galat dulu, lalu peringatan, lalu menurut path. Teks penyaring dicocokkan dengan pesan, sumber,
 * kode, atau path berkas (bila path cocok, semua masalah berkas itu lolos).
 */
export function buildGroups(files: Record<string, Diagnostic[]>, filter: ProblemFilter, relative: (path: string) => string): ProblemGroup[] {
  const needle = filter.text.trim().toLowerCase();
  const groups: ProblemGroup[] = [];
  for (const [path, all] of Object.entries(files)) {
    const shown = relative(path).replace(/\\/g, "/");
    const pathMatches = needle !== "" && shown.toLowerCase().includes(needle);
    const items = all
      .filter((item) => severityShown(item.severity, filter) && (needle === "" || pathMatches || matchesText(item, needle)))
      .sort((a, b) => SEVERITY_RANK[a.severity] - SEVERITY_RANK[b.severity] || a.line - b.line || a.column - b.column);
    if (items.length === 0) continue;
    const slash = shown.lastIndexOf("/");
    groups.push({ path, relative: shown, name: shown.slice(slash + 1), dir: slash > 0 ? shown.slice(0, slash) : "", counts: count(items), items });
  }
  const rank = (group: ProblemGroup) => (group.counts.errors > 0 ? 0 : group.counts.warnings > 0 ? 1 : 2);
  return groups.sort((a, b) => rank(a) - rank(b) || a.relative.localeCompare(b.relative));
}

/** Baris datar untuk daftar (yang bisa divirtualisasi); berkas yang dilipat hanya menyisakan judulnya. */
export function flattenRows(groups: readonly ProblemGroup[], collapsed: ReadonlySet<string>): ProblemRow[] {
  const rows: ProblemRow[] = [];
  for (const group of groups) {
    rows.push({ kind: "file", group });
    if (collapsed.has(group.path)) continue;
    for (const diagnostic of group.items) rows.push({ kind: "problem", group, diagnostic });
  }
  return rows;
}
