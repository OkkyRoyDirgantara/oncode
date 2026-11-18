// `renameSymbol` milik @codemirror/lsp-client hanya menerapkan suntingan pada berkas yang sedang
// terbuka di editor dan diam-diam melewatkan sisanya. Modul ini memeriksa hasil permintaan rename
// dan memberi tahu pengguna bila ada berkas yang terlewat, supaya rename setengah jadi tidak luput.
import type { LSPClient } from "@codemirror/lsp-client";
import { openFile } from "@/app/coreCommands";
import { toast } from "@/app/overlay";
import { baseName } from "@/app/store";
import { uriToPath } from "./uri";

/** Bagian `WorkspaceEdit` yang diperlukan untuk mengetahui berkas mana saja yang disunting. */
export interface WorkspaceEdit {
  changes?: Record<string, unknown[]>;
  documentChanges?: { textDocument?: { uri?: string }; edits?: unknown[] }[];
}

/** URI yang disunting oleh `edit` tetapi tidak akan diubah oleh pustaka. */
export function skippedRenameUris(edit: WorkspaceEdit | null | undefined, isOpen: (uri: string) => boolean): string[] {
  if (!edit || typeof edit !== "object") return [];
  const skipped = new Set<string>();
  for (const [uri, edits] of Object.entries(edit.changes ?? {})) {
    if (Array.isArray(edits) && edits.length > 0 && !isOpen(uri)) skipped.add(uri);
  }
  // Pustaka hanya membaca `changes`; suntingan yang hanya ada di `documentChanges` tidak diterapkan sama sekali.
  for (const change of edit.documentChanges ?? []) {
    const uri = change?.textDocument?.uri;
    if (!uri || !Array.isArray(change.edits) || change.edits.length === 0) continue;
    const applied = isOpen(uri) && (edit.changes?.[uri]?.length ?? 0) > 0;
    if (!applied) skipped.add(uri);
  }
  return [...skipped];
}

export function warnAboutSkippedRenameEdits(client: LSPClient, edit: WorkspaceEdit | null | undefined): void {
  const skipped = skippedRenameUris(edit, (uri) => client.workspace.getFile(uri) !== null);
  if (skipped.length === 0) return;
  const paths = skipped.map(uriToPath);
  const names = paths.slice(0, 3).map(baseName).join(", ");
  const more = paths.length > 3 ? ` and ${paths.length - 3} more` : "";
  toast.warning(
    `Rename only changed files that are open in the editor. ${paths.length} other file${paths.length === 1 ? " still uses" : "s still use"} the old name: ${names}${more}.`,
    [{ label: "Open Files", run: () => paths.slice(0, 20).forEach((path) => openFile(path)) }],
  );
}
