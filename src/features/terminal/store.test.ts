import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { clearMocks } from "@tauri-apps/api/mocks";
import type { Channel } from "@tauri-apps/api/core";
import { mockCommands, type TermEvent } from "@/ipc";
import { resetOverlayForTest } from "@/app/overlay";
import {
  clearActiveSession,
  closeAllSessions,
  closeSession,
  createSession,
  ensureSession,
  focusActiveTerminal,
  focusNextSession,
  renameSession,
  resetTerminalsForTest,
  setActiveSession,
  setAdapterFactory,
  setPanelVisible,
  setStage,
  useTerminals,
} from "./store";
import { fakeAdapterFactory, type FakeAdapter } from "./testing";

/** Backend tiruan: mencatat panggilan dan menyimpan channel tiap terminal agar pengujian bisa mengirim event. */
function fakeBackend() {
  const calls: unknown[][] = [];
  const channels = new Map<number, Channel<TermEvent>>();
  let nextId = 1;
  let spawnGate: Promise<void> | null = null;
  let writeGate: Promise<void> | null = null;
  mockCommands({
    termSpawn: async ({ cols, rows, onEvent }) => {
      const id = nextId++;
      calls.push(["spawn", id, cols, rows]);
      channels.set(id, onEvent);
      if (spawnGate) await spawnGate;
      return id;
    },
    termWrite: async ({ id, data }) => {
      calls.push(["write", id, data]);
      if (writeGate) await writeGate;
      return null;
    },
    termResize: ({ id, cols, rows }) => {
      calls.push(["resize", id, cols, rows]);
      return null;
    },
    termSetPaused: ({ id, paused }) => {
      calls.push(["paused", id, paused]);
      return null;
    },
    termKill: ({ id }) => {
      calls.push(["kill", id]);
      return null;
    },
  });
  return {
    calls,
    of: (name: string) => calls.filter((c) => c[0] === name),
    emit: (id: number, event: TermEvent) => channels.get(id)!.onmessage(event),
    holdSpawn() {
      let release!: () => void;
      spawnGate = new Promise((resolve) => (release = resolve));
      return () => {
        spawnGate = null;
        release();
      };
    },
    holdWrite() {
      let release!: () => void;
      writeGate = new Promise((resolve) => (release = resolve));
      return () => {
        writeGate = null;
        release();
      };
    },
  };
}

const flush = () => new Promise((resolve) => setTimeout(resolve, 0));
const state = () => useTerminals.getState();

describe("sesi terminal", () => {
  let backend: ReturnType<typeof fakeBackend>;
  let adapters: FakeAdapter[];
  const stage = document.createElement("div");

  beforeEach(() => {
    resetTerminalsForTest();
    backend = fakeBackend();
    const fake = fakeAdapterFactory();
    adapters = fake.adapters;
    setAdapterFactory(fake.factory);
    setStage(stage);
    setPanelVisible(true);
  });

  afterEach(() => {
    resetTerminalsForTest();
    resetOverlayForTest();
    clearMocks();
  });

  it("memulai shell dengan ukuran terminal yang sudah terpasang dan menjadikannya aktif", async () => {
    const id = await createSession();
    expect(id).toBe(1);
    expect(backend.of("spawn")).toEqual([["spawn", 1, 100, 30]]);
    expect(adapters[0].stage).toBe(stage);
    expect(state().sessions).toEqual([{ id: 1, title: "Terminal 1" }]);
    expect(state().activeId).toBe(1);
    expect(state().creating).toBe(false);
    expect(adapters[0].visible).toBe(true);
    expect(adapters[0].focusCount).toBe(1);
  });

  it("memakai 80x24 bila panel belum terpasang, lalu memasang terminal begitu panggung tersedia", async () => {
    setStage(null);
    await createSession();
    expect(backend.of("spawn")).toEqual([["spawn", 1, 80, 24]]);
    expect(adapters[0].stage).toBeNull();
    setStage(stage);
    expect(adapters[0].stage).toBe(stage);
  });

  it("meneruskan keluaran ke xterm dan masukan ke PTY", async () => {
    await createSession();
    backend.emit(1, { kind: "data", data: "hello " });
    backend.emit(1, { kind: "data", data: "world" });
    expect(adapters[0].written).toEqual(["hello ", "world"]);

    adapters[0].events.onInput("ls\r");
    await flush();
    expect(backend.of("write")).toEqual([["write", 1, "ls\r"]]);
  });

  it("mengirim masukan berurutan: ketikan selama pengiriman berjalan digabung ke panggilan berikutnya", async () => {
    await createSession();
    const release = backend.holdWrite();
    adapters[0].events.onInput("a");
    adapters[0].events.onInput("b");
    adapters[0].events.onInput("c");
    await flush();
    expect(backend.of("write")).toEqual([["write", 1, "a"]]);
    release();
    await flush();
    expect(backend.of("write")).toEqual([["write", 1, "a"], ["write", 1, "bc"]]);
  });

  it("masukan yang diketik sebelum backend menjawab dikirim setelah id diketahui", async () => {
    const release = backend.holdSpawn();
    const pending = createSession();
    await flush();
    expect(state().creating).toBe(true);
    adapters[0].events.onInput("x");
    expect(backend.of("write")).toEqual([]);
    release();
    await pending;
    await flush();
    expect(backend.of("write")).toEqual([["write", 1, "x"]]);
  });

  it("hanya mengirim perubahan ukuran yang benar-benar berbeda", async () => {
    await createSession();
    adapters[0].events.onResize({ cols: 100, rows: 30 });
    expect(backend.of("resize")).toEqual([]);
    adapters[0].events.onResize({ cols: 120, rows: 30 });
    adapters[0].events.onResize({ cols: 120, rows: 30 });
    adapters[0].events.onResize({ cols: 120, rows: 40 });
    await flush();
    expect(backend.of("resize")).toEqual([["resize", 1, 120, 30], ["resize", 1, 120, 40]]);
  });

  it("ukuran yang berubah selagi menunggu backend dikirim setelah shell dimulai", async () => {
    const release = backend.holdSpawn();
    const pending = createSession();
    await flush();
    adapters[0].events.onResize({ cols: 90, rows: 20 });
    release();
    await pending;
    await flush();
    expect(backend.of("spawn")).toEqual([["spawn", 1, 100, 30]]);
    expect(backend.of("resize")).toEqual([["resize", 1, 90, 20]]);
  });

  it("menjeda PTY saat xterm tertinggal dan melanjutkan setelah antrean diproses", async () => {
    await createSession();
    adapters[0].autoProcess = false;
    const chunk = "x".repeat(200_000);
    backend.emit(1, { kind: "data", data: chunk });
    backend.emit(1, { kind: "data", data: chunk });
    expect(backend.of("paused")).toEqual([]);
    backend.emit(1, { kind: "data", data: chunk }); // 600 000 > high watermark
    await flush();
    expect(backend.of("paused")).toEqual([["paused", 1, true]]);

    adapters[0].pendingDone.shift()!(); // 400 000
    adapters[0].pendingDone.shift()!(); // 200 000
    expect(backend.of("paused")).toHaveLength(1);
    adapters[0].pendingDone.shift()!(); // 0
    await flush();
    expect(backend.of("paused")).toEqual([["paused", 1, true], ["paused", 1, false]]);
  });

  it("proses berakhir: ditandai, pesan redup dicetak, dan tombol apa pun menutup terminal", async () => {
    await createSession();
    backend.emit(1, { kind: "exit", code: 3 });
    expect(state().sessions[0].exited).toEqual({ code: 3 });
    expect(adapters[0].written.at(-1)).toBe("\r\n\x1b[2m[Process exited with code 3]\x1b[0m\r\n");
    expect(adapters[0].disposed).toBe(false);

    adapters[0].events.onInput("q");
    await flush();
    expect(state().sessions).toEqual([]);
    expect(state().activeId).toBeNull();
    expect(adapters[0].disposed).toBe(true);
    // Tombol itu tidak dikirim ke PTY, dan proses yang sudah mati tidak di-kill lagi.
    expect(backend.of("write")).toEqual([]);
    expect(backend.of("kill")).toEqual([]);
  });

  it("proses yang dihentikan sinyal tidak punya kode keluar", async () => {
    await createSession();
    backend.emit(1, { kind: "exit", code: null });
    expect(state().sessions[0].exited).toEqual({ code: null });
    expect(adapters[0].written.at(-1)).toContain("[Process terminated]");
  });

  it("menutup terminal yang masih hidup menghentikan prosesnya dan mengaktifkan tetangganya", async () => {
    await createSession();
    await createSession();
    await createSession();
    expect(state().sessions.map((s) => s.title)).toEqual(["Terminal 1", "Terminal 2", "Terminal 3"]);
    setActiveSession(2);
    closeSession(2);
    await flush();
    expect(backend.of("kill")).toEqual([["kill", 2]]);
    expect(adapters[1].disposed).toBe(true);
    expect(state().sessions.map((s) => s.id)).toEqual([1, 3]);
    expect(state().activeId).toBe(3);
    // Event yang terlambat dari terminal yang sudah ditutup diabaikan.
    backend.emit(2, { kind: "data", data: "late" });
    backend.emit(2, { kind: "exit", code: 0 });
    expect(adapters[1].written).toEqual([]);
    expect(state().sessions.map((s) => s.id)).toEqual([1, 3]);
  });

  it("hanya terminal aktif yang terlihat, dan tidak ada yang terlihat saat panel tersembunyi", async () => {
    await createSession();
    await createSession();
    expect(adapters.map((a) => a.visible)).toEqual([false, true]);
    setActiveSession(1);
    expect(adapters.map((a) => a.visible)).toEqual([true, false]);
    setPanelVisible(false);
    expect(adapters.map((a) => a.visible)).toEqual([false, false]);
    // Keluaran tetap diproses selagi tersembunyi.
    backend.emit(2, { kind: "data", data: "background" });
    expect(adapters[1].written).toEqual(["background"]);
    setPanelVisible(true);
    expect(adapters.map((a) => a.visible)).toEqual([true, false]);
  });

  it("permintaan fokus ditunda sampai panel terlihat", async () => {
    await createSession();
    setPanelVisible(false);
    const before = adapters[0].focusCount;
    focusActiveTerminal();
    expect(adapters[0].focusCount).toBe(before);
    setPanelVisible(true);
    expect(adapters[0].focusCount).toBe(before + 1);
  });

  it("berpindah ke terminal berikutnya, mengganti nama, dan membersihkan", async () => {
    await createSession();
    await createSession();
    focusNextSession();
    expect(state().activeId).toBe(1);
    focusNextSession();
    expect(state().activeId).toBe(2);
    renameSession(2, "  build  ");
    renameSession(1, "   ");
    expect(state().sessions.map((s) => s.title)).toEqual(["Terminal 1", "build"]);
    clearActiveSession();
    expect(adapters.map((a) => a.clearCount)).toEqual([0, 1]);
  });

  it("ensureSession hanya membuat satu terminal walau dipanggil berkali-kali", async () => {
    ensureSession();
    ensureSession();
    await flush();
    await flush();
    ensureSession();
    await flush();
    expect(backend.of("spawn")).toHaveLength(1);
    expect(state().sessions).toHaveLength(1);
  });

  it("workspace berganti: semua terminal dimatikan, termasuk yang sedang dibuat", async () => {
    await createSession();
    await createSession();
    const release = backend.holdSpawn();
    const pending = createSession();
    await flush();

    closeAllSessions();
    await flush();
    expect(state().sessions).toEqual([]);
    expect(state().activeId).toBeNull();
    expect(backend.of("kill")).toEqual([["kill", 1], ["kill", 2]]);
    expect(adapters[0].disposed && adapters[1].disposed).toBe(true);

    release();
    expect(await pending).toBeNull();
    await flush();
    expect(backend.of("kill")).toEqual([["kill", 1], ["kill", 2], ["kill", 3]]);
    expect(adapters[2].disposed).toBe(true);
    expect(state().sessions).toEqual([]);
    expect(state().creating).toBe(false);

    // Penomoran judul mulai lagi dari awal di workspace baru.
    await createSession();
    expect(state().sessions).toEqual([{ id: 4, title: "Terminal 1" }]);
  });

  it("kegagalan backend dilaporkan dan tidak meninggalkan sesi", async () => {
    clearMocks();
    mockCommands({
      termSpawn: () => {
        throw { code: "spawnFailed", message: "no shell", detail: null };
      },
    });
    const error = vi.spyOn(console, "error").mockImplementation(() => {});
    expect(await createSession()).toBeNull();
    expect(state().sessions).toEqual([]);
    expect(adapters[0].disposed).toBe(true);
    error.mockRestore();
  });
});
