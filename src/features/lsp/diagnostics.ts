// Diagnostics dari semua language server, per berkas. Dipakai panel Problems dan status bar.
import { create } from "zustand";
import { uriToPath } from "./uri";

export type Severity = "error" | "warning" | "info" | "hint";

export interface Diagnostic {
  /** Baris mulai, berbasis 1 (cocok dengan `openFile(path, { line })`). */
  line: number;
  /** Kolom mulai sebagai offset dalam baris, berbasis 0 (cocok dengan `openFile(path, { column })`). */
  column: number;
  endLine: number;
  endColumn: number;
  severity: Severity;
  message: string;
  source?: string;
  code?: string;
}

/** Bentuk minimal `textDocument/publishDiagnostics` yang dipakai di sini. */
export interface LspPosition {
  line: number;
  character: number;
}
export interface LspRange {
  start: LspPosition;
  end: LspPosition;
}
export interface LspDiagnostic {
  range: LspRange;
  severity?: number;
  message: string;
  source?: string;
  code?: string | number | { value: string | number };
}
export interface PublishDiagnosticsParams {
  uri: string;
  version?: number | null;
  diagnostics: LspDiagnostic[];
}

interface DiagnosticsState {
  /** Path absolut → diagnostics. Berkas tanpa diagnostics tidak punya entri. */
  files: Record<string, Diagnostic[]>;
}

export const useDiagnostics = create<DiagnosticsState>(() => ({ files: {} }));

/** Server mana yang menerbitkan diagnostics untuk path mana, agar bisa dihapus saat server itu berhenti. */
const owners = new Map<string, Set<string>>();

export function toSeverity(severity: number | undefined): Severity {
  // Menurut spesifikasi, tanpa severity klien bebas menafsirkan; anggap galat seperti @codemirror/lsp-client.
  return severity === 2 ? "warning" : severity === 3 ? "info" : severity === 4 ? "hint" : "error";
}

export function convertDiagnostic(item: LspDiagnostic): Diagnostic {
  const code = typeof item.code === "object" && item.code !== null ? item.code.value : item.code;
  return {
    line: item.range.start.line + 1,
    column: item.range.start.character,
    endLine: item.range.end.line + 1,
    endColumn: item.range.end.character,
    severity: toSeverity(item.severity),
    message: item.message,
    ...(item.source ? { source: item.source } : {}),
    ...(code !== undefined && code !== null && code !== "" ? { code: String(code) } : {}),
  };
}

/** Mencatat satu notifikasi `publishDiagnostics` dari server `owner`; daftar kosong menghapus entri berkasnya. */
export function publishDiagnostics(owner: string, params: PublishDiagnosticsParams): void {
  if (!params || typeof params.uri !== "string" || !Array.isArray(params.diagnostics)) return;
  const path = uriToPath(params.uri);
  const list = params.diagnostics.filter((d) => d && d.range && typeof d.message === "string").map(convertDiagnostic);
  let paths = owners.get(owner);
  if (list.length === 0) {
    paths?.delete(path);
    if (!(path in useDiagnostics.getState().files)) return;
    useDiagnostics.setState((s) => {
      const { [path]: _removed, ...rest } = s.files;
      return { files: rest };
    });
    return;
  }
  if (!paths) owners.set(owner, (paths = new Set()));
  paths.add(path);
  useDiagnostics.setState((s) => ({ files: { ...s.files, [path]: list } }));
}

/** Menghapus diagnostics milik satu server (server berhenti), atau semuanya bila `owner` tidak diberikan. */
export function clearDiagnostics(owner?: string): void {
  if (owner === undefined) {
    owners.clear();
    if (Object.keys(useDiagnostics.getState().files).length > 0) useDiagnostics.setState({ files: {} });
    return;
  }
  const paths = owners.get(owner);
  owners.delete(owner);
  if (!paths || paths.size === 0) return;
  useDiagnostics.setState((s) => {
    const files = { ...s.files };
    for (const path of paths) delete files[path];
    return { files };
  });
}
