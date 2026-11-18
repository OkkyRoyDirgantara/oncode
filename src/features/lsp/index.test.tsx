import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen } from "@testing-library/react";
import { clearMocks } from "@tauri-apps/api/mocks";
import { LSPPlugin } from "@codemirror/lsp-client";
import type { EditorView } from "@codemirror/view";
import { executeCommand, getCommand, keybindingsFor } from "@/app/commands";
import { OverlayHost, resetOverlayForTest } from "@/app/overlay";
import { getStatusItems } from "@/app/registry";
import { editorExtensionProviders } from "@/features/editor/extensions";
import { LspStatus, registerLspFeature } from "./index";
import { lspEditorExtension, resetLspForTest, serverKey, useLsp } from "./servers";
import { RUST, fakeLspBackend, openEditor, setWorkspaceForTest } from "./testing";

const mocks = vi.hoisted(() => ({ activeEditor: vi.fn(), listeners: new Set<() => void>() }));
vi.mock("@/features/editor/CodeEditor", () => ({
  activeEditor: mocks.activeEditor,
  onActiveEditorChanged: (listener: () => void) => {
    mocks.listeners.add(listener);
    return () => mocks.listeners.delete(listener);
  },
}));

const wait = (ms = 0) => new Promise((resolve) => setTimeout(resolve, ms));
const setActive = (value: { view: EditorView; path: string } | null) => {
  mocks.activeEditor.mockReturnValue(value);
  mocks.listeners.forEach((l) => l());
};

describe("fitur LSP", () => {
  let views: EditorView[] = [];

  beforeAll(() => registerLspFeature());

  beforeEach(() => {
    resetLspForTest();
    setWorkspaceForTest({});
    mocks.activeEditor.mockReturnValue(null);
  });

  afterEach(async () => {
    views.forEach((v) => v.destroy());
    views = [];
    resetLspForTest();
    await wait();
    resetOverlayForTest();
    setWorkspaceForTest(null);
    clearMocks();
    document.body.innerHTML = "";
  });

  it("mendaftarkan ekstensi editor, item status, dan command dengan keybinding yang diminta", () => {
    expect(editorExtensionProviders()).toContain(lspEditorExtension);
    const item = getStatusItems("right").find((i) => i.id === "lsp.server")!;
    expect(item.order).toBe(30);
    expect(keybindingsFor(getCommand("editor.goToDefinition")!)).toEqual(["F12"]);
    expect(keybindingsFor(getCommand("editor.findReferences")!)).toEqual(["Shift+F12"]);
    expect(keybindingsFor(getCommand("editor.rename")!)).toEqual(["F2"]);
    expect(keybindingsFor(getCommand("editor.format")!)).toEqual(["Alt+Shift+F"]);
    expect(getCommand("lsp.restart")?.category).toBe("LSP");
    // Tanpa editor yang terhubung ke server, command tidak aktif.
    for (const id of ["editor.goToDefinition", "editor.findReferences", "editor.rename", "editor.format", "lsp.restart"]) {
      expect(getCommand(id)!.when!(), id).toBe(false);
    }
  });

  it("command aktif saat editor aktif terhubung ke server yang berjalan dan memanggil klien", async () => {
    const backend = fakeLspBackend({ respond: (m) => (m.method === "textDocument/definition" ? [{ uri: "file:///proj/a.rs", range: { start: { line: 0, character: 3 }, end: { line: 0, character: 7 } } }] : undefined) });
    const view = await openEditor("/proj/a.rs", "rust", "fn main() {}\nmain();\n");
    views.push(view);
    await vi.waitFor(() => expect(LSPPlugin.get(view)).not.toBeNull());
    setActive({ view, path: "/proj/a.rs" });
    // Fokus di tempat lain (mis. explorer): keybinding F12/F2 tidak direbut.
    expect(getCommand("editor.goToDefinition")!.when!()).toBe(false);
    expect(getCommand("lsp.restart")!.when!()).toBe(true);
    view.focus();
    expect(getCommand("editor.goToDefinition")!.when!()).toBe(true);
    expect(getCommand("editor.rename")!.when!()).toBe(true);

    view.dispatch({ selection: { anchor: 14 } });
    await executeCommand("editor.goToDefinition");
    await vi.waitFor(() => expect(view.state.selection.main.head).toBe(3));

    await executeCommand("lsp.restart");
    await vi.waitFor(() => expect(backend.processes).toHaveLength(2));
    expect(backend.stopCalls).toEqual([1]);
    await vi.waitFor(() => expect(LSPPlugin.get(view)).not.toBeNull());
  });

  it("item status: tersembunyi tanpa server, menampilkan nama dan keadaan, menu Restart/Stop", async () => {
    const backend = fakeLspBackend();
    render(
      <>
        <LspStatus />
        <OverlayHost />
      </>,
    );
    expect(screen.queryByRole("button")).toBeNull();

    const view = await openEditor("/proj/a.rs", "rust", "fn main() {}\n");
    views.push(view);
    const markdown = await openEditor("/proj/readme.md", "markdown", "# hi\n");
    views.push(markdown);
    await vi.waitFor(() => expect(useLsp.getState().servers[serverKey(RUST)]?.state).toBe("running"));

    // Bahasa tanpa server yang dikonfigurasi: tetap tersembunyi.
    act(() => setActive({ view: markdown, path: "/proj/readme.md" }));
    expect(screen.queryByRole("button")).toBeNull();

    act(() => setActive({ view, path: "/proj/a.rs" }));
    const button = screen.getByRole("button", { name: "rust-analyzer language server: running" });
    expect(button).toHaveTextContent("rust-analyzer");
    expect(button.querySelector(".lsp-dot")).toHaveAttribute("data-state", "running");

    fireEvent.click(button);
    expect(await screen.findByText("Restart Server")).toBeInTheDocument();
    await act(async () => {
      fireEvent.click(screen.getByText("Stop Server"));
    });
    expect(backend.stopCalls).toEqual([1]);
    expect(screen.getByRole("button", { name: "rust-analyzer language server: stopped" })).toBeInTheDocument();
  });

  it("server tidak terpasang: tooltip dan dialog menampilkan petunjuk pemasangan", async () => {
    fakeLspBackend({ startError: () => ({ code: "serverNotInstalled", message: "rust-analyzer tidak ditemukan" }) });
    render(
      <>
        <LspStatus />
        <OverlayHost />
      </>,
    );
    const view = await openEditor("/proj/a.rs", "rust", "fn main() {}\n");
    views.push(view);
    await vi.waitFor(() => expect(useLsp.getState().servers[serverKey(RUST)]?.state).toBe("missing"));
    act(() => setActive({ view, path: "/proj/a.rs" }));

    const button = screen.getByRole("button", { name: "rust-analyzer language server: not installed" });
    expect(button.title).toContain("rustup component add rust-analyzer");
    fireEvent.click(button);
    fireEvent.click(await screen.findByText("How to Install…"));
    expect(await screen.findByText("rust-analyzer is not installed")).toBeInTheDocument();
    expect(screen.getByText(/rustup component add rust-analyzer/)).toBeInTheDocument();
  });
});
