// Sesi terminal: satu PTY di backend dan satu adapter (xterm) per sesi.
// Store tidak mengenal xterm; lihat `adapter.ts`.
import { Channel } from "@tauri-apps/api/core";
import { create } from "zustand";
import { showError } from "@/app/overlay";
import { commands, type TermEvent } from "@/ipc";
import { DEFAULT_SIZE, type AdapterFactory, type TerminalAdapter, type TerminalSize } from "./adapter";
import { FlowControl } from "./flow";

export interface TerminalSession {
  /** Id terminal di backend. */
  id: number;
  title: string;
  /** Terisi setelah proses shell berakhir; `code` null bila dihentikan sinyal. */
  exited?: { code: number | null };
}

interface TerminalState {
  sessions: TerminalSession[];
  activeId: number | null;
  /** Sedang ada sesi yang dibuat (menunggu tata letak atau backend). */
  creating: boolean;
  findOpen: boolean;
}

export const useTerminals = create<TerminalState>(() => ({ sessions: [], activeId: null, creating: false, findOpen: false }));

interface Runtime {
  id: number | null;
  adapter: TerminalAdapter;
  flow: FlowControl;
  cols: number;
  rows: number;
  /** Masukan yang belum terkirim; dikirim berurutan, satu pemanggilan IPC pada satu waktu. */
  outbox: string;
  sending: boolean;
  exited: { code: number | null } | null;
  disposed: boolean;
}

const runtimes = new Map<number, Runtime>();
let factory: AdapterFactory | null = null;
let stage: HTMLElement | null = null;
let panelVisible = false;
let wantFocus = false;
let pendingCreations = 0;
let titleCounter = 0;
/** Bertambah saat semua sesi dibuang (workspace berganti); pembuatan yang masih berjalan dibatalkan. */
let epoch = 0;

const ignore = () => {};

export function setAdapterFactory(next: AdapterFactory): void {
  factory = next;
}

/** Elemen tempat semua terminal dipasang. Dipanggil komponen panel saat terpasang/dilepas. */
export function setStage(element: HTMLElement | null): void {
  stage = element;
  if (!element) return;
  for (const runtime of runtimes.values()) void runtime.adapter.mount(element).catch(ignore);
}

function syncVisibility(): void {
  const activeId = useTerminals.getState().activeId;
  for (const [id, runtime] of runtimes) runtime.adapter.setVisible(panelVisible && id === activeId);
  if (wantFocus && panelVisible && activeId !== null) {
    const runtime = runtimes.get(activeId);
    if (runtime) {
      wantFocus = false;
      runtime.adapter.focus();
    }
  }
}

/** Panel terminal terlihat atau tidak (panel disembunyikan, atau tampilan panel lain dipilih). */
export function setPanelVisible(visible: boolean): void {
  panelVisible = visible;
  syncVisibility();
}

/** Fokuskan terminal aktif sekarang, atau begitu panel terlihat dan sesi tersedia. */
export function focusActiveTerminal(): void {
  wantFocus = true;
  syncVisibility();
}

function exitMessage(code: number | null): string {
  const text = code === null ? "[Process terminated]" : `[Process exited with code ${code}]`;
  // Redup (SGR 2), lalu kembali normal.
  return `\r\n\x1b[2m${text}\x1b[0m\r\n`;
}

async function pump(runtime: Runtime): Promise<void> {
  if (runtime.sending) return;
  runtime.sending = true;
  try {
    while (runtime.outbox && runtime.id !== null && !runtime.exited && !runtime.disposed) {
      const chunk = runtime.outbox;
      runtime.outbox = "";
      await commands.termWrite(runtime.id, chunk);
    }
  } catch (e) {
    console.warn("Gagal menulis ke terminal", e);
    runtime.outbox = "";
  } finally {
    runtime.sending = false;
  }
}

function handleInput(runtime: Runtime, data: string): void {
  if (runtime.disposed) return;
  if (runtime.exited) {
    // Tombol apa pun menutup terminal yang prosesnya sudah berakhir. Ditunda agar xterm
    // tidak dibuang di tengah penangan event-nya sendiri.
    const id = runtime.id;
    if (id !== null) queueMicrotask(() => closeSession(id));
    return;
  }
  runtime.outbox += data;
  void pump(runtime);
}

function handleResize(runtime: Runtime, size: TerminalSize): void {
  if (size.cols === runtime.cols && size.rows === runtime.rows) return;
  runtime.cols = size.cols;
  runtime.rows = size.rows;
  if (runtime.id !== null && !runtime.exited && !runtime.disposed) {
    void commands.termResize(runtime.id, size.cols, size.rows).catch(ignore);
  }
}

function handleEvent(runtime: Runtime, event: TermEvent): void {
  if (runtime.disposed) return;
  if (event.kind === "data") {
    const length = event.data.length;
    runtime.flow.written(length);
    runtime.adapter.write(event.data, () => runtime.flow.processed(length));
    return;
  }
  runtime.exited = { code: event.code };
  runtime.outbox = "";
  runtime.adapter.write(exitMessage(event.code));
  const id = runtime.id;
  if (id !== null) {
    useTerminals.setState((s) => ({ sessions: s.sessions.map((x) => (x.id === id ? { ...x, exited: { code: event.code } } : x)) }));
  }
}

/** Membuat terminal baru dan menjadikannya aktif. Mengembalikan id-nya, atau null bila gagal/dibatalkan. */
export async function createSession(): Promise<number | null> {
  if (!factory) throw new Error("Adapter terminal belum didaftarkan");
  const startedIn = epoch;
  pendingCreations += 1;
  useTerminals.setState({ creating: true });
  let runtime: Runtime | null = null;
  try {
    const created: Runtime = {
      id: null,
      adapter: undefined as unknown as TerminalAdapter,
      flow: new FlowControl((paused) => {
        if (created.id !== null && !created.exited && !created.disposed) void commands.termSetPaused(created.id, paused).catch(ignore);
      }),
      cols: DEFAULT_SIZE.cols,
      rows: DEFAULT_SIZE.rows,
      outbox: "",
      sending: false,
      exited: null,
      disposed: false,
    };
    created.adapter = await factory({
      onInput: (data) => handleInput(created, data),
      onResize: (size) => handleResize(created, size),
      onFind: () => useTerminals.setState({ findOpen: true }),
    });
    runtime = created;

    // Ukuran sebenarnya diketahui setelah terminal terpasang; shell langsung dimulai dengan ukuran itu.
    const size = stage ? await created.adapter.mount(stage) : DEFAULT_SIZE;
    if (startedIn !== epoch) throw new Cancelled();
    created.cols = size.cols;
    created.rows = size.rows;

    const channel = new Channel<TermEvent>();
    channel.onmessage = (event) => handleEvent(created, event);
    const id = await commands.termSpawn(size.cols, size.rows, channel);
    created.id = id;
    if (startedIn !== epoch) {
      void commands.termKill(id).catch(ignore);
      throw new Cancelled();
    }

    runtimes.set(id, created);
    titleCounter += 1;
    const session: TerminalSession = { id, title: `Terminal ${titleCounter}`, ...(created.exited ? { exited: created.exited } : {}) };
    useTerminals.setState((s) => ({ sessions: [...s.sessions, session], activeId: id }));
    // Ukuran bisa berubah selagi menunggu backend.
    if (created.cols !== size.cols || created.rows !== size.rows) void commands.termResize(id, created.cols, created.rows).catch(ignore);
    void pump(created);
    wantFocus = true;
    syncVisibility();
    return id;
  } catch (e) {
    if (runtime) {
      runtime.disposed = true;
      runtime.adapter.dispose();
    }
    if (!(e instanceof Cancelled)) showError(e, "Could not start the terminal");
    return null;
  } finally {
    pendingCreations -= 1;
    useTerminals.setState({ creating: pendingCreations > 0 });
  }
}

class Cancelled extends Error {}

/** Membuat terminal pertama bila belum ada satu pun (dipanggil saat panel menjadi terlihat). */
export function ensureSession(): void {
  if (useTerminals.getState().sessions.length === 0 && pendingCreations === 0) void createSession();
}

/** Menutup terminal: proses shell dihentikan (bila masih hidup) dan xterm dibuang. */
export function closeSession(id: number): void {
  const runtime = runtimes.get(id);
  if (!runtime) return;
  runtimes.delete(id);
  runtime.disposed = true;
  if (!runtime.exited) void commands.termKill(id).catch(ignore);
  runtime.adapter.dispose();
  useTerminals.setState((s) => {
    const index = s.sessions.findIndex((x) => x.id === id);
    const sessions = s.sessions.filter((x) => x.id !== id);
    const activeId = s.activeId === id ? (sessions[Math.min(index, sessions.length - 1)]?.id ?? null) : s.activeId;
    return { sessions, activeId, findOpen: sessions.length > 0 && s.findOpen };
  });
  if (useTerminals.getState().activeId !== null) wantFocus = true;
  syncVisibility();
}

/** Menutup semua terminal (workspace ditutup atau diganti). */
export function closeAllSessions(): void {
  epoch += 1;
  for (const id of [...runtimes.keys()]) closeSession(id);
  titleCounter = 0;
  wantFocus = false;
  useTerminals.setState({ sessions: [], activeId: null, findOpen: false });
}

export function setActiveSession(id: number): void {
  if (!runtimes.has(id)) return;
  useTerminals.setState({ activeId: id });
  wantFocus = true;
  syncVisibility();
}

export function focusNextSession(): void {
  const { sessions, activeId } = useTerminals.getState();
  if (sessions.length === 0) return;
  const index = sessions.findIndex((s) => s.id === activeId);
  setActiveSession(sessions[(index + 1) % sessions.length].id);
}

export function renameSession(id: number, title: string): void {
  const trimmed = title.trim();
  if (!trimmed) return;
  useTerminals.setState((s) => ({ sessions: s.sessions.map((x) => (x.id === id ? { ...x, title: trimmed } : x)) }));
}

export function clearActiveSession(): void {
  const id = useTerminals.getState().activeId;
  if (id !== null) runtimes.get(id)?.adapter.clear();
}

export function findInActiveSession(query: string, forward: boolean): boolean {
  const id = useTerminals.getState().activeId;
  if (id === null || !query) return false;
  return runtimes.get(id)?.adapter.find(query, forward) ?? false;
}

export function closeFind(): void {
  const id = useTerminals.getState().activeId;
  if (id !== null) runtimes.get(id)?.adapter.clearFind();
  useTerminals.setState({ findOpen: false });
  focusActiveTerminal();
}

/** Hanya untuk pengujian. */
export function resetTerminalsForTest(): void {
  closeAllSessions();
  factory = null;
  stage = null;
  panelVisible = false;
  pendingCreations = 0;
}
