// Fitur LSP: ekstensi editor per berkas, item status bar, dan command navigasi/penyuntingan.
import { useSyncExternalStore } from "react";
import type { MouseEvent as ReactMouseEvent } from "react";
import type { EditorView } from "@codemirror/view";
import { LSPPlugin, formatDocument, jumpToDefinition, renameSymbol } from "@codemirror/lsp-client";
import { registerCommands } from "@/app/commands";
import { choose, showMenu, toast, type MenuItem } from "@/app/overlay";
import { registerStatusItem } from "@/app/registry";
import { onWorkspaceChanged, useApp } from "@/app/store";
import { activeEditor, onActiveEditorChanged } from "@/features/editor/CodeEditor";
import { registerEditorExtension } from "@/features/editor/extensions";
import { languageId } from "@/features/editor/languages";
import { findReferences } from "./references";
import { lspEditorExtension, reconcileServers, restartServer, statusForLanguage, stopAllServers, stopServer, useLsp, type ServerState, type ServerStatus } from "./servers";
import "./lsp.css";

const STATE_LABEL: Record<ServerState, string> = {
  starting: "starting…",
  running: "running",
  failed: "crashed",
  missing: "not installed",
  stopped: "stopped",
};

function showInstallHint(status: ServerStatus): void {
  void choose({
    title: `${status.name} is not installed`,
    message: status.installHint || `Install ${status.name} and make sure it is on your PATH, then restart the server.`,
    detail: status.stderrTail || undefined,
    buttons: [
      { id: "close", label: "Close" },
      { id: "retry", label: "Try Again", kind: "primary" },
    ],
  }).then((answer) => {
    if (answer === "retry") restartServer(status.key);
  });
}

function showServerOutput(status: ServerStatus): void {
  void choose({
    title: `${status.name} stopped working`,
    message: status.message ?? "The language server exited unexpectedly.",
    detail: status.stderrTail || undefined,
    buttons: [{ id: "close", label: "Close", kind: "primary" }],
  });
}

export function serverMenu(status: ServerStatus): MenuItem[] {
  const items: MenuItem[] = [];
  if (status.state === "missing") items.push({ label: "How to Install…", run: () => showInstallHint(status) }, { separator: true });
  if (status.state === "failed") items.push({ label: "Show Error Output…", run: () => showServerOutput(status) }, { separator: true });
  items.push(
    { label: status.state === "stopped" || status.state === "missing" || status.state === "failed" ? "Start Server" : "Restart Server", run: () => restartServer(status.key) },
    { label: "Stop Server", disabled: status.state !== "running" && status.state !== "starting", run: () => stopServer(status.key) },
  );
  return items;
}

function useActiveEditorPath(): string {
  return useSyncExternalStore(onActiveEditorChanged, () => activeEditor()?.path ?? "");
}

/** Status server untuk bahasa editor aktif; tersembunyi bila bahasanya tidak punya server. */
export function LspStatus() {
  const path = useActiveEditorPath();
  const language = path ? languageId(path) : null;
  // Selector mengembalikan objek status yang tersimpan (referensi stabil), bukan objek baru.
  const status = useLsp((s) => statusForLanguage(s, language));
  if (!status) return null;
  const title =
    status.state === "missing"
      ? `${status.name} is not installed. ${status.installHint}`
      : `${status.name}: ${STATE_LABEL[status.state]}${status.message && status.state === "failed" ? ` — ${status.message}` : ""}`;
  const open = (e: ReactMouseEvent<HTMLButtonElement>) => {
    const rect = e.currentTarget.getBoundingClientRect();
    showMenu({ x: rect.left, y: rect.top - 4 }, serverMenu(status));
  };
  return (
    <button className="oc-status-item" title={title} aria-label={`${status.name} language server: ${STATE_LABEL[status.state]}`} onClick={open}>
      <span className="lsp-dot" data-state={status.state} />
      {status.name}
    </button>
  );
}

/** Editor aktif yang sedang terhubung ke server yang berjalan. */
function lspView(): EditorView | null {
  const active = activeEditor();
  return active && LSPPlugin.get(active.view) ? active.view : null;
}

function activeServer(): ServerStatus | null {
  const active = activeEditor();
  return active ? statusForLanguage(useLsp.getState(), languageId(active.path)) : null;
}

export function registerLspFeature(): void {
  registerEditorExtension(lspEditorExtension);
  registerStatusItem({ id: "lsp.server", align: "right", order: 30, component: LspStatus });

  // Keybinding (F12, F2, ...) hanya berlaku selagi fokus ada di editor yang terhubung, supaya tidak
  // merebut tombol yang sama dari tampilan lain (mis. F2 untuk mengganti nama berkas di explorer).
  // Dari command palette command tetap tersedia: daftar palet disusun selagi fokus masih di editor.
  const connected = () => {
    const view = lspView();
    return view !== null && view.dom.contains(document.activeElement);
  };
  const run = (command: (view: EditorView) => boolean, unsupported: string) => () => {
    const view = lspView();
    if (view && !command(view)) toast.info(unsupported);
  };
  registerCommands([
    { id: "editor.goToDefinition", title: "Go to Definition", category: "Edit", keybinding: "F12", when: connected, run: run(jumpToDefinition, "The language server does not support Go to Definition.") },
    { id: "editor.findReferences", title: "Find All References", category: "Edit", keybinding: "Shift+F12", when: connected, run: run(findReferences, "The language server does not support Find References.") },
    { id: "editor.rename", title: "Rename Symbol", category: "Edit", keybinding: "F2", when: connected, run: run(renameSymbol, "Place the cursor on a symbol the language server can rename.") },
    { id: "editor.format", title: "Format Document", category: "Edit", keybinding: "Shift+Alt+F", when: connected, run: run(formatDocument, "The language server does not support formatting.") },
    {
      id: "lsp.restart",
      title: "Restart Language Server",
      category: "LSP",
      when: () => activeServer() !== null,
      run: () => {
        const status = activeServer();
        if (status) restartServer(status.key);
      },
    },
  ]);

  // Backend menghentikan semua server setiap kali workspace dibuka ulang. Folder lain atau ditutup:
  // buang semuanya. Folder yang sama (status kepercayaan berubah): mulai lagi untuk editor yang masih terbuka.
  let root = useApp.getState().workspace?.root ?? null;
  onWorkspaceChanged((workspace) => {
    const next = workspace?.root ?? null;
    if (next === null || next !== root) stopAllServers();
    else void reconcileServers();
    root = next;
  });

  // Pengaturan LSP berubah (dinyalakan/dimatikan, daftar server diubah): terapkan ke editor yang terbuka.
  let lspSettings = JSON.stringify(useApp.getState().settings?.lsp ?? null);
  useApp.subscribe((state, previous) => {
    if (state.settings === previous.settings) return;
    const next = JSON.stringify(state.settings?.lsp ?? null);
    if (next === lspSettings) return;
    lspSettings = next;
    // Pergantian workspace ikut mengganti pengaturan; itu sudah ditangani di atas.
    if (state.workspace === previous.workspace) void reconcileServers();
  });
}
