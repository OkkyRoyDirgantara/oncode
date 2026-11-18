// Workspace @codemirror/lsp-client untuk aplikasi ini. Berbeda dari bawaan pustaka:
// satu berkas boleh punya beberapa editor (tab di dua grup, split), dan berkas lain
// ditampilkan lewat sistem tab aplikasi.
import { ChangeSet, type Text, type TransactionSpec } from "@codemirror/state";
import type { EditorView } from "@codemirror/view";
import { setDiagnostics, type Diagnostic as LintDiagnostic } from "@codemirror/lint";
import { LSPPlugin, Workspace, type LSPClient, type WorkspaceFile } from "@codemirror/lsp-client";
import { openFile } from "@/app/coreCommands";
import { activeEditor } from "@/features/editor/CodeEditor";
import { getBuffer } from "@/features/editor/buffers";
import { toSeverity, type PublishDiagnosticsParams } from "./diagnostics";
import { pathKey, uriKey, uriToPath } from "./uri";

class AppFile implements WorkspaceFile {
  /** Editor yang menampilkan berkas ini; yang pertama dianggap utama. */
  views: EditorView[] = [];

  constructor(
    readonly uri: string,
    readonly key: string,
    readonly languageId: string,
    public version: number,
    public doc: Text,
  ) {}

  getView(main?: EditorView): EditorView | null {
    if (main && this.views.includes(main)) return main;
    return this.views[0] ?? null;
  }
}

/** Berapa lama menunggu tab berkas siap setelah dibuka (memuat dari disk, memasang editor). */
const DISPLAY_TIMEOUT_MS = 4000;

/** Menunggu editor untuk `path` menjadi editor aktif; jatuh ke editor mana pun yang menampilkan berkas itu. */
async function waitForEditor(path: string): Promise<EditorView | null> {
  const key = pathKey(path);
  const deadline = Date.now() + DISPLAY_TIMEOUT_MS;
  for (;;) {
    const active = activeEditor();
    if (active && pathKey(active.path) === key) return active.view;
    if (Date.now() >= deadline) break;
    await new Promise((resolve) => setTimeout(resolve, 25));
  }
  const views = getBuffer(path)?.views;
  return views && views.size > 0 ? [...views][0] : null;
}

export class AppWorkspace extends Workspace {
  files: AppFile[] = [];
  private versions = new Map<string, number>();

  constructor(client: LSPClient) {
    super(client);
  }

  private nextVersion(uri: string): number {
    const version = (this.versions.get(uri) ?? -1) + 1;
    this.versions.set(uri, version);
    return version;
  }

  /** Server boleh menulis URI dengan escape berbeda (mis. `%40` vs `@`, `c%3A` vs `C:`); bandingkan path-nya. */
  getFile(uri: string): AppFile | null {
    const exact = this.files.find((f) => f.uri === uri);
    if (exact) return exact;
    const key = uriKey(uri);
    return this.files.find((f) => f.key === key) ?? null;
  }

  syncFiles() {
    const result: { file: AppFile; prevDoc: Text; changes: ChangeSet }[] = [];
    for (const file of this.files) {
      const view = file.views[0];
      const plugin = view && LSPPlugin.get(view);
      if (!plugin) continue;
      const doc = view.state.doc;
      if (doc === file.doc) continue;
      // Semua editor sebuah berkas menerima perubahan yang sama (lihat editor/sync.ts), jadi perubahan
      // milik editor utama mewakili semuanya. Bila tidak cocok dengan dokumen terakhir yang dikirim,
      // kirim sebagai penggantian penuh daripada rentang yang salah.
      let changes = plugin.unsyncedChanges;
      if (changes.empty || changes.length !== file.doc.length || changes.newLength !== doc.length) {
        changes = ChangeSet.of({ from: 0, to: file.doc.length, insert: doc }, file.doc.length);
      }
      result.push({ file, prevDoc: file.doc, changes });
      file.doc = doc;
      file.version = this.nextVersion(file.uri);
      for (const other of file.views) LSPPlugin.get(other)?.clear();
    }
    return result;
  }

  openFile(uri: string, languageId: string, view: EditorView): void {
    const existing = this.getFile(uri);
    if (existing) {
      // Kirim dulu perubahan yang tertunda agar editor baru mulai dari keadaan yang sama dengan server.
      this.client.sync();
      if (!existing.views.includes(view)) existing.views.push(view);
      return;
    }
    const file = new AppFile(uri, uriKey(uri), languageId, this.nextVersion(uri), view.state.doc);
    file.views.push(view);
    this.files.push(file);
    this.client.didOpen(file);
  }

  closeFile(uri: string, view: EditorView): void {
    const file = this.getFile(uri);
    if (!file) return;
    file.views = file.views.filter((v) => v !== view);
    if (file.views.length > 0) return;
    this.files = this.files.filter((f) => f !== file);
    this.client.didClose(file.uri);
  }

  updateFile(uri: string, update: TransactionSpec): void {
    // Cukup satu editor: perubahan diteruskan ke editor lain berkas itu oleh sinkronisasi buffer.
    this.getFile(uri)?.getView()?.dispatch(update);
  }

  /**
   * Dipanggil pustaka saat harus menampilkan berkas lain (go to definition). Berkas dibuka lewat
   * sistem tab; pustaka lalu memindahkan kursor di editor yang dikembalikan.
   */
  async displayFile(uri: string): Promise<EditorView | null> {
    if (!/^file:/i.test(uri)) return null;
    const path = uriToPath(uri);
    openFile(path);
    const view = await waitForEditor(path);
    view?.focus();
    return view;
  }
}

/**
 * `serverDiagnostics()` milik pustaka hanya menandai satu editor per berkas. Fungsi ini menandai
 * editor lainnya (split) dengan cara yang sama.
 */
export function showDiagnosticsInOtherViews(client: LSPClient, params: PublishDiagnosticsParams): void {
  const file = client.workspace.getFile(params.uri);
  if (!(file instanceof AppFile) || file.views.length < 2) return;
  if (params.version != null && params.version !== file.version) return;
  const main = file.getView();
  for (const view of file.views) {
    if (view === main) continue;
    const plugin = LSPPlugin.get(view);
    if (!plugin) continue;
    try {
      const diagnostics: LintDiagnostic[] = params.diagnostics.map((item) => ({
        from: plugin.unsyncedChanges.mapPos(plugin.fromPosition(item.range.start, plugin.syncedDoc)),
        to: plugin.unsyncedChanges.mapPos(plugin.fromPosition(item.range.end, plugin.syncedDoc)),
        severity: toSeverity(item.severity),
        message: item.message,
      }));
      view.dispatch(setDiagnostics(view.state, diagnostics));
    } catch (e) {
      // Posisi di luar dokumen (server tertinggal): lewati, notifikasi berikutnya akan memperbaikinya.
      console.warn("Diagnostics tidak bisa dipetakan ke editor", e);
    }
  }
}
