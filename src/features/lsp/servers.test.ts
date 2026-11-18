import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { clearMocks } from "@tauri-apps/api/mocks";
import { LSPPlugin } from "@codemirror/lsp-client";
import type { EditorView } from "@codemirror/view";
import { resetOverlayForTest } from "@/app/overlay";
import { publishDiagnostics, useDiagnostics } from "./diagnostics";
import {
  EARLY_EXIT_MS,
  MAX_RESTARTS,
  RESTART_WINDOW_MS,
  lspEditorExtension,
  reconcileServers,
  resetLspForTest,
  restartDelay,
  restartServer,
  serverKey,
  statusForLanguage,
  stopAllServers,
  stopServer,
  useLsp,
} from "./servers";
import { RUST, TS_FAMILY, fakeLspBackend, openEditor, setWorkspaceForTest } from "./testing";

const RUST_KEY = serverKey(RUST);
const status = () => useLsp.getState().servers[RUST_KEY];
/** Menjalankan timer palsu selama `ms` sambil membiarkan promise dan IPC tiruan selesai. */
const tick = (ms = 0) => vi.advanceTimersByTimeAsync(ms);

describe("siklus hidup language server", () => {
  let views: EditorView[] = [];
  const open = async (path: string, language: string | null, doc = "fn main() {}\n") => {
    const view = await openEditor(path, language, doc);
    views.push(view);
    await tick();
    return view;
  };

  beforeEach(() => {
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout", "Date"] });
    resetLspForTest();
    setWorkspaceForTest({});
  });

  afterEach(() => {
    views.forEach((v) => v.destroy());
    views = [];
    resetLspForTest();
    resetOverlayForTest();
    setWorkspaceForTest(null);
    clearMocks();
    vi.useRealTimers();
    document.body.innerHTML = "";
  });

  it("jeda mulai ulang berlipat dua", () => {
    expect([1, 2, 3, 4, 5].map(restartDelay)).toEqual([500, 1000, 2000, 4000, 8000]);
  });

  it("server dimulai saat berkas pertama bahasanya dibuka dan dipakai bersama berkas berikutnya", async () => {
    const backend = fakeLspBackend();
    expect(backend.processes).toHaveLength(0);
    const a = await open("/proj/src/a.rs", "rust");
    expect(backend.processes.map((p) => p.language)).toEqual(["rust"]);
    expect(status().state).toBe("running");
    expect(status().name).toBe("rust-analyzer");
    expect(backend.processes[0].methods().slice(0, 2)).toEqual(["initialize", "initialized"]);
    expect(backend.processes[0].received[0].params.rootUri).toBe("file:///proj");

    const b = await open("/proj/src/b.rs", "rust");
    expect(backend.processes).toHaveLength(1);
    expect(LSPPlugin.get(a)!.client).toBe(LSPPlugin.get(b)!.client);
    expect(backend.serversCalls()).toBe(1);
  });

  it("bahasa yang memakai perintah sama berbagi satu proses; bahasa lain mendapat prosesnya sendiri", async () => {
    const backend = fakeLspBackend();
    await open("/proj/a.ts", "typescript");
    await open("/proj/b.tsx", "typescriptreact");
    expect(backend.processes.map((p) => p.language)).toEqual(["typescript"]);
    await open("/proj/c.rs", "rust");
    expect(backend.processes.map((p) => p.language)).toEqual(["typescript", "rust"]);
    const ts = useLsp.getState().servers[serverKey(TS_FAMILY[0])];
    expect(ts.languages).toEqual(["typescript", "typescriptreact", "javascript"]);
    expect(statusForLanguage(useLsp.getState(), "javascript")).toBe(ts);
    expect(statusForLanguage(useLsp.getState(), "python")).toBeNull();
  });

  it("tidak ada ekstensi untuk berkas besar, tanpa bahasa, atau bahasa tanpa server", async () => {
    const backend = fakeLspBackend();
    expect(await lspEditorExtension({ path: "/proj/big.rs", languageId: "rust", large: true })).toBeNull();
    expect(await lspEditorExtension({ path: "/proj/notes.txt", languageId: null, large: false })).toBeNull();
    expect(await lspEditorExtension({ path: "/proj/a.md", languageId: "markdown", large: false })).toBeNull();
    await tick();
    expect(backend.processes).toHaveLength(0);
    expect(useLsp.getState().servers).toEqual({});
  });

  it("tidak dimulai di folder yang tidak dipercaya atau bila LSP dimatikan", async () => {
    const backend = fakeLspBackend();
    setWorkspaceForTest({ trusted: false });
    await open("/proj/a.rs", "rust");
    expect(backend.processes).toHaveLength(0);
    expect(status()).toBeUndefined();

    resetLspForTest();
    setWorkspaceForTest({ lspEnabled: false });
    await open("/proj/b.rs", "rust");
    expect(backend.processes).toHaveLength(0);
  });

  it("serverNotInstalled → missing dengan petunjuk pemasangan, tanpa percobaan ulang", async () => {
    const backend = fakeLspBackend({ startError: () => ({ code: "serverNotInstalled", message: "rust-analyzer tidak ditemukan di PATH" }) });
    const view = await open("/proj/a.rs", "rust");
    expect(status().state).toBe("missing");
    expect(status().installHint).toContain("rustup component add rust-analyzer");
    expect(status().message).toBe("rust-analyzer tidak ditemukan di PATH");
    expect(LSPPlugin.get(view)).toBeNull();
    await tick(60_000);
    await open("/proj/b.rs", "rust");
    expect(backend.processes).toHaveLength(0);
    expect(status().state).toBe("missing");
  });

  it("proses yang keluar sebelum menjawab initialize (proxy rustup) → missing, tanpa percobaan ulang", async () => {
    const backend = fakeLspBackend({ answerInitialize: false });
    const view = await open("/proj/a.rs", "rust");
    expect(status().state).toBe("starting");
    await tick(200);
    backend.processes[0].exit(1, "error: Unknown binary 'rust-analyzer' in official toolchain");
    expect(status().state).toBe("missing");
    expect(status().stderrTail).toContain("Unknown binary");
    expect(status().message).toBe("The server process exited with code 1.");
    expect(LSPPlugin.get(view)).toBeNull();
    await tick(RESTART_WINDOW_MS);
    expect(backend.processes).toHaveLength(1);
  });

  it("proses yang tidak pernah menjawab initialize tetapi baru keluar setelah batas awal dianggap mati, bukan missing", async () => {
    const backend = fakeLspBackend({ answerInitialize: false });
    await open("/proj/a.rs", "rust");
    await tick(EARLY_EXIT_MS + 1);
    backend.processes[0].exit(101);
    expect(status().state).toBe("starting");
    await tick(restartDelay(1));
    expect(backend.processes).toHaveLength(2);
  });

  it("mati saat berjalan: dimulai ulang dengan jeda yang membesar, editor dipasang lagi ke klien baru", async () => {
    const backend = fakeLspBackend();
    const view = await open("/proj/a.rs", "rust");
    const firstClient = LSPPlugin.get(view)!.client;
    await tick(10_000);

    backend.processes[0].exit(139, "segfault");
    expect(status().state).toBe("starting");
    expect(status().stderrTail).toBe("segfault");
    expect(LSPPlugin.get(view)).toBeNull();
    await tick(restartDelay(1) - 1);
    expect(backend.processes).toHaveLength(1);
    await tick(1);
    expect(backend.processes).toHaveLength(2);
    expect(status().state).toBe("running");
    const secondClient = LSPPlugin.get(view)!.client;
    expect(secondClient).not.toBe(firstClient);
    // Berkas yang masih terbuka dibuka lagi di server baru.
    expect(backend.processes[1].notifications("textDocument/didOpen").map((m) => m.params.textDocument.uri)).toEqual(["file:///proj/a.rs"]);

    // Kematian kedua menunggu lebih lama.
    await tick(10_000);
    backend.processes[1].exit(139);
    await tick(restartDelay(2) - 1);
    expect(backend.processes).toHaveLength(2);
    await tick(1);
    expect(backend.processes).toHaveLength(3);
  });

  it("paling banyak lima mulai ulang dalam tiga menit, lalu failed dengan ekor stderr", async () => {
    const backend = fakeLspBackend();
    const view = await open("/proj/a.rs", "rust");
    for (let attempt = 1; attempt <= MAX_RESTARTS; attempt++) {
      await tick(EARLY_EXIT_MS + 100);
      backend.processes.at(-1)!.exit(1, `crash ${attempt}`);
      expect(status().state).toBe("starting");
      await tick(restartDelay(attempt));
      expect(backend.processes).toHaveLength(attempt + 1);
      expect(status().state).toBe("running");
    }
    await tick(EARLY_EXIT_MS + 100);
    backend.processes.at(-1)!.exit(1, "final crash");
    expect(status().state).toBe("failed");
    expect(status().stderrTail).toBe("final crash");
    expect(LSPPlugin.get(view)).toBeNull();
    await tick(RESTART_WINDOW_MS);
    expect(backend.processes).toHaveLength(MAX_RESTARTS + 1);

    // Pengguna boleh memulainya lagi secara manual.
    restartServer(RUST_KEY);
    await tick();
    expect(backend.processes).toHaveLength(MAX_RESTARTS + 2);
    expect(status().state).toBe("running");
    expect(LSPPlugin.get(view)).not.toBeNull();
  });

  it("mulai ulang yang berjauhan (di luar jendela tiga menit) tidak dihitung", async () => {
    const backend = fakeLspBackend();
    await open("/proj/a.rs", "rust");
    for (let round = 0; round < MAX_RESTARTS + 3; round++) {
      await tick(RESTART_WINDOW_MS);
      backend.processes.at(-1)!.exit(1);
      await tick(restartDelay(1));
      expect(status().state).toBe("running");
    }
    expect(backend.processes).toHaveLength(MAX_RESTARTS + 4);
  });

  it("initialize yang tidak pernah dijawab berakhir sebagai kegagalan: proses dihentikan dan dicoba lagi", async () => {
    const backend = fakeLspBackend({ answerInitialize: false });
    await open("/proj/a.rs", "rust");
    await tick(10_000);
    expect(backend.stopCalls).toEqual([1]);
    expect(status().state).toBe("starting");
    expect(status().message).toMatch(/Initialization failed/);
    // Event exit dari proses yang sengaja dihentikan tidak dihitung sebagai kematian kedua.
    backend.processes[0].exit(null);
    await tick(restartDelay(1));
    expect(backend.processes).toHaveLength(2);
  });

  it("Stop Server: proses dihentikan, editor dilepas, diagnostics dihapus, event exit-nya diabaikan", async () => {
    const backend = fakeLspBackend();
    const view = await open("/proj/a.rs", "rust");
    publishDiagnostics(RUST_KEY, { uri: "file:///proj/a.rs", diagnostics: [{ range: { start: { line: 0, character: 0 }, end: { line: 0, character: 2 } }, message: "x" }] });
    publishDiagnostics("other-server", { uri: "file:///proj/main.go", diagnostics: [{ range: { start: { line: 0, character: 0 }, end: { line: 0, character: 2 } }, message: "y" }] });

    stopServer(RUST_KEY);
    await tick();
    expect(backend.stopCalls).toEqual([1]);
    expect(status().state).toBe("stopped");
    expect(LSPPlugin.get(view)).toBeNull();
    expect(Object.keys(useDiagnostics.getState().files)).toEqual(["/proj/main.go"]);

    backend.processes[0].exit(null);
    await tick(RESTART_WINDOW_MS);
    expect(status().state).toBe("stopped");
    expect(backend.processes).toHaveLength(1);
    // Berkas baru tidak menghidupkan server yang dihentikan pengguna...
    const other = await open("/proj/b.rs", "rust");
    expect(backend.processes).toHaveLength(1);
    // ...tetapi Restart menghubungkan semua editor, termasuk yang dibuka selagi berhenti.
    restartServer(RUST_KEY);
    await tick();
    expect(status().state).toBe("running");
    expect(LSPPlugin.get(view)).not.toBeNull();
    expect(LSPPlugin.get(other)).not.toBeNull();
  });

  it("menghentikan server selagi backend masih memulainya tidak meninggalkan proses", async () => {
    const backend = fakeLspBackend();
    const view = await openEditor("/proj/a.rs", "rust");
    views.push(view);
    // Mikrotask pendaftaran sudah berjalan dan lspStart sudah dipanggil, tetapi belum selesai.
    await Promise.resolve();
    expect(status().state).toBe("starting");
    stopServer(RUST_KEY);
    await tick();
    expect(backend.stopCalls).toEqual([1]);
    expect(status().state).toBe("stopped");
    expect(LSPPlugin.get(view)).toBeNull();
  });

  it("menutup editor terakhir mengirim didClose dan melepas pendaftarannya", async () => {
    const backend = fakeLspBackend();
    const view = await open("/proj/a.rs", "rust");
    view.destroy();
    views = [];
    await tick();
    expect(backend.processes[0].notifications("textDocument/didClose").map((m) => m.params.textDocument.uri)).toEqual(["file:///proj/a.rs"]);
    // Server mati setelah itu: tidak ada editor yang perlu dipasang ulang, tidak ada galat.
    await tick(10_000);
    backend.processes[0].exit(1);
    await tick(restartDelay(1));
    expect(status().state).toBe("running");
  });

  it("workspace berganti: semua server dihentikan, status dan diagnostics dikosongkan, tidak ada mulai ulang", async () => {
    const backend = fakeLspBackend();
    const view = await open("/proj/a.rs", "rust");
    await open("/proj/b.ts", "typescript");
    publishDiagnostics(RUST_KEY, { uri: "file:///proj/a.rs", diagnostics: [{ range: { start: { line: 0, character: 0 }, end: { line: 0, character: 2 } }, message: "x" }] });

    stopAllServers();
    await tick();
    expect(backend.stopCalls.sort()).toEqual([1, 2]);
    expect(useLsp.getState()).toEqual({ infos: null, servers: {} });
    expect(useDiagnostics.getState().files).toEqual({});
    expect(LSPPlugin.get(view)).toBeNull();
    // Event exit yang menyusul dari backend tidak memicu apa pun.
    backend.processes.forEach((p) => p.exit(null));
    await tick(RESTART_WINDOW_MS);
    expect(backend.processes).toHaveLength(2);
    expect(useLsp.getState().servers).toEqual({});
  });

  it("folder dipercaya setelah berkas terbuka: server dimulai untuk editor yang sudah ada", async () => {
    const backend = fakeLspBackend();
    setWorkspaceForTest({ trusted: false });
    const view = await open("/proj/a.rs", "rust");
    expect(backend.processes).toHaveLength(0);

    setWorkspaceForTest({ trusted: true });
    await reconcileServers();
    await tick();
    expect(backend.processes).toHaveLength(1);
    expect(status().state).toBe("running");
    expect(LSPPlugin.get(view)).not.toBeNull();

    // Kepercayaan dicabut: server berhenti dan editor dilepas.
    setWorkspaceForTest({ trusted: false });
    await reconcileServers();
    await tick();
    expect(backend.stopCalls).toEqual([1]);
    expect(status()).toBeUndefined();
    expect(LSPPlugin.get(view)).toBeNull();
  });
});
