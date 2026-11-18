// Klien @codemirror/lsp-client sungguhan dan EditorView sungguhan (jsdom), berbicara dengan
// language server tiruan lewat transport Tauri yang di-mock di batas IPC.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen } from "@testing-library/react";
import { clearMocks } from "@tauri-apps/api/mocks";
import { forEachDiagnostic } from "@codemirror/lint";
import { LSPPlugin, formatDocument, jumpToDefinition, renameSymbol } from "@codemirror/lsp-client";
import type { EditorView } from "@codemirror/view";
import { OverlayHost, resetOverlayForTest } from "@/app/overlay";
import { useDiagnostics } from "./diagnostics";
import { findReferences } from "./references";
import { resetLspForTest, serverKey, stopServer, useLsp } from "./servers";
import { RUST, fakeLspBackend, openEditor, setWorkspaceForTest, type FakeBackendOptions, type FakeProcess } from "./testing";

const mocks = vi.hoisted(() => ({ openFile: vi.fn(), activeEditor: vi.fn() }));
vi.mock("@/app/coreCommands", () => ({ openFile: mocks.openFile }));
vi.mock("@/features/editor/CodeEditor", () => ({ activeEditor: mocks.activeEditor }));

const wait = (ms = 0) => new Promise((resolve) => setTimeout(resolve, ms));
const range = (line: number, from: number, to: number) => ({ start: { line, character: from }, end: { line, character: to } });

function lintMessages(view: EditorView): string[] {
  const messages: string[] = [];
  forEachDiagnostic(view.state, (d, from, to) => messages.push(`${d.severity}:${d.message}@${from}-${to}`));
  return messages;
}

describe("LSPClient di atas transport Tauri", () => {
  let views: EditorView[] = [];
  let server: FakeProcess;

  async function start(options: FakeBackendOptions, path = "/proj/src/a.rs", doc = "fn main() {\n    helper();\n}\n") {
    const backend = fakeLspBackend(options);
    const view = await openEditor(path, "rust", doc);
    views.push(view);
    await vi.waitFor(() => expect(LSPPlugin.get(view)).not.toBeNull());
    server = backend.processes[0];
    return { backend, view };
  }
  async function another(path: string, doc: string) {
    const view = await openEditor(path, "rust", doc);
    views.push(view);
    await vi.waitFor(() => expect(LSPPlugin.get(view)).not.toBeNull());
    return view;
  }

  beforeEach(() => {
    resetLspForTest();
    setWorkspaceForTest({});
    mocks.openFile.mockReset();
    mocks.activeEditor.mockReset();
    mocks.activeEditor.mockReturnValue(null);
  });

  afterEach(async () => {
    // Menutup editor lalu menghentikan server pada giliran yang sama: didClose yang sudah
    // dijadwalkan klien tidak boleh menjadi "unhandled rejection" (Vitest menggagalkan run bila terjadi).
    views.forEach((v) => v.destroy());
    views = [];
    resetLspForTest();
    await wait();
    resetOverlayForTest();
    setWorkspaceForTest(null);
    clearMocks();
    document.body.innerHTML = "";
  });

  it("jabat tangan initialize → initialized → didOpen dengan URI, bahasa, versi, dan isi berkas", async () => {
    await start({}, "/proj/src/my file.rs", "fn main() {}\n");
    await vi.waitFor(() => expect(server.notifications("textDocument/didOpen")).toHaveLength(1));
    expect(server.methods()).toEqual(["initialize", "initialized", "textDocument/didOpen"]);
    const init = server.received[0];
    expect(init.params.rootUri).toBe("file:///proj");
    expect(init.params.capabilities.textDocument.publishDiagnostics).toEqual({ versionSupport: true });
    expect(server.notifications("textDocument/didOpen")[0].params.textDocument).toEqual({
      uri: "file:///proj/src/my%20file.rs",
      languageId: "rust",
      version: 0,
      text: "fn main() {}\n",
    });
    expect(useLsp.getState().servers[serverKey(RUST)].state).toBe("running");
  });

  it("suntingan dikirim sebagai didChange dengan versi yang naik", async () => {
    const { view } = await start({});
    view.dispatch({ changes: { from: 0, insert: "// hi\n" } });
    LSPPlugin.get(view)!.client.sync();
    await vi.waitFor(() => expect(server.notifications("textDocument/didChange")).toHaveLength(1));
    const change = server.notifications("textDocument/didChange")[0].params;
    expect(change.textDocument).toEqual({ uri: "file:///proj/src/a.rs", version: 1 });
    expect(change.contentChanges).toEqual([{ text: "// hi\nfn main() {\n    helper();\n}\n" }]);
  });

  it("publishDiagnostics mengisi store Problems dan menandai editor; server berhenti menghapus keduanya", async () => {
    const { view } = await start({});
    server.emit({
      method: "textDocument/publishDiagnostics",
      params: {
        uri: "file:///proj/src/a.rs",
        version: 0,
        diagnostics: [
          { range: range(1, 4, 10), severity: 1, message: "cannot find function `helper`", source: "rustc", code: "E0425" },
          { range: range(0, 3, 7), severity: 2, message: "unused", source: "clippy" },
        ],
      },
    });
    expect(useDiagnostics.getState().files).toEqual({
      "/proj/src/a.rs": [
        { line: 2, column: 4, endLine: 2, endColumn: 10, severity: "error", message: "cannot find function `helper`", source: "rustc", code: "E0425" },
        { line: 1, column: 3, endLine: 1, endColumn: 7, severity: "warning", message: "unused", source: "clippy" },
      ],
    });
    expect(lintMessages(view).sort()).toEqual(["error:cannot find function `helper`@16-22", "warning:unused@3-7"]);

    // Diagnostics untuk berkas yang tidak terbuka tetap masuk ke store.
    server.emit({ method: "textDocument/publishDiagnostics", params: { uri: "file:///proj/src/other.rs", diagnostics: [{ range: range(0, 0, 1), severity: 3, message: "note" }] } });
    expect(Object.keys(useDiagnostics.getState().files).sort()).toEqual(["/proj/src/a.rs", "/proj/src/other.rs"]);

    stopServer(serverKey(RUST));
    expect(useDiagnostics.getState().files).toEqual({});
    expect(lintMessages(view)).toEqual([]);
    expect(LSPPlugin.get(view)).toBeNull();
  });

  it("dua editor untuk berkas yang sama: satu didOpen, keduanya ditandai, didClose setelah yang terakhir ditutup", async () => {
    const { view } = await start({});
    const second = await another("/proj/src/a.rs", view.state.doc.toString());
    await wait();
    expect(server.notifications("textDocument/didOpen")).toHaveLength(1);

    // Server menulis URI dengan escape berbeda; berkasnya tetap dikenali.
    server.emit({ method: "textDocument/publishDiagnostics", params: { uri: "file:///proj/src/a%2Ers", diagnostics: [{ range: range(0, 0, 2), severity: 1, message: "both" }] } });
    expect(lintMessages(view)).toEqual(["error:both@0-2"]);
    expect(lintMessages(second)).toEqual(["error:both@0-2"]);

    // Perubahan di kedua editor (yang kedua mengikuti lewat sinkronisasi buffer) dikirim sekali.
    const change = { from: 0, insert: "x" };
    view.dispatch({ changes: change });
    second.dispatch({ changes: change });
    LSPPlugin.get(second)!.client.sync();
    await vi.waitFor(() => expect(server.notifications("textDocument/didChange")).toHaveLength(1));
    expect(LSPPlugin.get(view)!.unsyncedChanges.empty).toBe(true);
    expect(LSPPlugin.get(second)!.unsyncedChanges.empty).toBe(true);

    views = views.filter((v) => v !== view);
    view.destroy();
    await wait();
    expect(server.notifications("textDocument/didClose")).toHaveLength(0);
    views = [];
    second.destroy();
    await vi.waitFor(() => expect(server.notifications("textDocument/didClose")).toHaveLength(1));
  });

  it("Go to Definition ke berkas lain membukanya lewat sistem tab dan memindahkan kursor di editornya", async () => {
    const { view } = await start({
      respond: (message) => (message.method === "textDocument/definition" ? { uri: "file:///proj/src/b.rs", range: range(2, 3, 9) } : undefined),
    });
    const target = await openEditor("/proj/src/b.rs", null, "// b\n\nfn helper() {}\n");
    views.push(target);
    // Tab berkas menjadi editor aktif beberapa saat setelah dibuka.
    mocks.openFile.mockImplementation((path: string) => setTimeout(() => mocks.activeEditor.mockReturnValue({ view: target, path }), 40));

    view.dispatch({ selection: { anchor: 18 } });
    expect(jumpToDefinition(view)).toBe(true);
    await vi.waitFor(() => expect(target.state.selection.main.head).toBe(9));
    expect(mocks.openFile).toHaveBeenCalledWith("/proj/src/b.rs");
    const request = server.received.find((m) => m.method === "textDocument/definition")!;
    expect(request.params).toEqual({ textDocument: { uri: "file:///proj/src/a.rs" }, position: { line: 1, character: 6 } });
  });

  it("Go to Definition di berkas yang sama tidak membuka tab", async () => {
    const { view } = await start({
      respond: (message) => (message.method === "textDocument/definition" ? [{ uri: "file:///proj/src/a.rs", range: range(0, 3, 7) }] : undefined),
    });
    view.dispatch({ selection: { anchor: 18 } });
    jumpToDefinition(view);
    await vi.waitFor(() => expect(view.state.selection.main.head).toBe(3));
    expect(mocks.openFile).not.toHaveBeenCalled();
  });

  it("Find References menampilkan semua lokasi (juga di berkas yang tidak terbuka) dan membuka pilihan lewat openFile", async () => {
    const { view } = await start({
      respond: (message) =>
        message.method === "textDocument/references"
          ? [
              { uri: "file:///proj/src/a.rs", range: range(1, 4, 10) },
              { uri: "file:///proj/src/closed.rs", range: range(41, 8, 14) },
            ]
          : undefined,
    });
    render(<OverlayHost />);
    view.dispatch({ selection: { anchor: 18 } });
    expect(findReferences(view)).toBe(true);

    const closed = await screen.findByText("src/closed.rs:42:9");
    expect(screen.getByText("src/a.rs:2:5")).toBeInTheDocument();
    // Pratinjau baris diambil dari editor yang terbuka bila ada (di sini buffer tidak ada, jadi tanpa pratinjau).
    const request = server.received.find((m) => m.method === "textDocument/references")!;
    expect(request.params.context).toEqual({ includeDeclaration: true });
    await act(async () => {
      fireEvent.click(closed);
    });
    expect(mocks.openFile).toHaveBeenCalledWith("/proj/src/closed.rs", { line: 42, column: 8 });
  });

  it("Rename menerapkan suntingan ke berkas terbuka dan memperingatkan tentang berkas yang terlewat", async () => {
    const edit = (line: number, from: number, to: number) => ({ range: range(line, from, to), newText: "assist" });
    const { view } = await start({
      respond: (message) =>
        message.method === "textDocument/rename"
          ? { changes: { "file:///proj/src/a.rs": [edit(1, 4, 10)], "file:///proj/src/closed.rs": [edit(0, 3, 9)] } }
          : undefined,
    });
    render(<OverlayHost />);
    view.dispatch({ selection: { anchor: 18 } });
    expect(renameSymbol(view)).toBe(true);
    const input = view.dom.querySelector("input[name=name]") as HTMLInputElement;
    expect(input.value).toBe("helper");
    input.value = "assist";
    fireEvent.submit(input.form!);

    await vi.waitFor(() => expect(view.state.doc.toString()).toBe("fn main() {\n    assist();\n}\n"));
    const request = server.received.find((m) => m.method === "textDocument/rename")!;
    expect(request.params.newName).toBe("assist");
    expect(await screen.findByText(/1 other file still uses the old name: closed\.rs\./)).toBeInTheDocument();
  });

  it("Format Document menerapkan suntingan dari server", async () => {
    const { view } = await start({
      capabilities: { textDocumentSync: 1, documentFormattingProvider: true },
      respond: (message) => (message.method === "textDocument/formatting" ? [{ range: range(1, 0, 4), newText: "\t" }] : undefined),
    });
    expect(formatDocument(view)).toBe(true);
    await vi.waitFor(() => expect(view.state.doc.toString()).toBe("fn main() {\n\thelper();\n}\n"));
  });
});
