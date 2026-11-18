// Find References. Panel referensi bawaan @codemirror/lsp-client hanya bisa menampilkan berkas yang
// sedang terbuka di editor (referensi di berkas lain dibuang), jadi hasilnya ditampilkan lewat
// quick pick aplikasi dan dibuka lewat sistem tab.
import type { EditorView } from "@codemirror/view";
import { LSPPlugin } from "@codemirror/lsp-client";
import { openFile } from "@/app/coreCommands";
import { quickPick, toast, type PickItem } from "@/app/overlay";
import { relativePath } from "@/app/store";
import { getBuffer } from "@/features/editor/buffers";
import { commands } from "@/ipc";
import type { LspRange } from "./diagnostics";
import { uriToPath } from "./uri";

export interface LspLocation {
  uri: string;
  range: LspRange;
}

export interface Reference {
  path: string;
  /** Baris berbasis 1. */
  line: number;
  /** Offset dalam baris, berbasis 0. */
  column: number;
}

/** Berkas yang belum terbuka dibaca dari disk untuk pratinjau baris, paling banyak sejumlah ini. */
const PREVIEW_FILE_LIMIT = 30;

/** Lokasi dari server → daftar unik, urut menurut berkas lalu posisi. */
export function toReferences(locations: readonly LspLocation[]): Reference[] {
  const seen = new Set<string>();
  const references: Reference[] = [];
  for (const location of locations) {
    if (!location || typeof location.uri !== "string" || !location.range) continue;
    const reference = { path: uriToPath(location.uri), line: location.range.start.line + 1, column: location.range.start.character };
    const key = `${reference.path}\n${reference.line}\n${reference.column}`;
    if (seen.has(key)) continue;
    seen.add(key);
    references.push(reference);
  }
  return references.sort((a, b) => a.path.localeCompare(b.path) || a.line - b.line || a.column - b.column);
}

async function loadLines(paths: string[]): Promise<Map<string, string[]>> {
  const lines = new Map<string, string[]>();
  let fromDisk = 0;
  await Promise.all(
    paths.map(async (path) => {
      const buffer = getBuffer(path);
      if (buffer && buffer.status === "ready") {
        lines.set(path, buffer.doc.toJSON());
        return;
      }
      if (fromDisk >= PREVIEW_FILE_LIMIT) return;
      fromDisk += 1;
      try {
        const opened = await commands.fsOpen(path);
        if (opened.kind === "text") lines.set(path, opened.text.split("\n"));
      } catch {
        // Tanpa pratinjau; referensinya tetap bisa dibuka.
      }
    }),
  );
  return lines;
}

export async function showReferences(locations: readonly LspLocation[]): Promise<void> {
  const references = toReferences(locations);
  if (references.length === 0) {
    toast.info("No references found.");
    return;
  }
  const lines = await loadLines([...new Set(references.map((r) => r.path))]);
  const items: PickItem<Reference>[] = references.map((reference) => ({
    label: `${relativePath(reference.path)}:${reference.line}:${reference.column + 1}`,
    description: lines.get(reference.path)?.[reference.line - 1]?.trim().slice(0, 160),
    value: reference,
  }));
  const picked = await quickPick({ placeholder: `${references.length} reference${references.length === 1 ? "" : "s"}`, items });
  if (picked) openFile(picked.path, { line: picked.line, column: picked.column });
}

/** Command CodeMirror: cari semua referensi simbol di kursor. */
export function findReferences(view: EditorView): boolean {
  const plugin = LSPPlugin.get(view);
  if (!plugin) return false;
  const capabilities = plugin.client.serverCapabilities;
  if (capabilities && !capabilities.referencesProvider) return false;
  plugin.client.sync();
  plugin.client
    .request<unknown, LspLocation[] | null>("textDocument/references", {
      textDocument: { uri: plugin.uri },
      position: plugin.toPosition(view.state.selection.main.head),
      context: { includeDeclaration: true },
    })
    .then(
      (locations) => showReferences(locations ?? []),
      (error) => plugin.reportError("Finding references failed", error),
    );
  return true;
}
