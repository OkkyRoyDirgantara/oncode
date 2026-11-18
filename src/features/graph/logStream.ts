// Aliran `git log` bertahap untuk tampilan Graph: membuka stream, mengambil halaman, memuat ulang, dan menutupnya.
import { useEffect, useMemo, useState, useSyncExternalStore } from "react";
import { commands, errorMessage, type GraphRow, type LogQuery } from "@/ipc";

export const PAGE_SIZE = 300;
/** Saat memuat ulang, paling banyak sejumlah ini baris dimuat kembali sekaligus. */
export const RELOAD_CAP = 3000;
/** Batas jumlah baris saat mencari sebuah commit dengan memuat halaman berikutnya. */
export const FIND_CAP = 10000;

export interface LogState {
  rows: GraphRow[];
  /** Tidak ada commit lagi di stream. */
  done: boolean;
  loading: boolean;
  error: string | null;
}

const EMPTY: LogState = { rows: [], done: false, loading: true, error: null };

export class LogController {
  private state: LogState = EMPTY;
  private listeners = new Set<() => void>();
  /** Bertambah setiap kali stream dibuka ulang; hasil dari sesi lama diabaikan. */
  private session = 0;
  private streamId: number | null = null;
  private inflight: Promise<void> | null = null;
  disposed = false;

  constructor(
    readonly root: string,
    readonly query: LogQuery,
  ) {}

  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  };

  getState = (): LogState => this.state;

  private set(patch: Partial<LogState>) {
    this.state = { ...this.state, ...patch };
    this.listeners.forEach((l) => l());
  }

  private closeStream() {
    const id = this.streamId;
    this.streamId = null;
    if (id !== null) void commands.gitLogClose(id).catch(() => undefined);
  }

  private track(work: Promise<void>): Promise<void> {
    const tracked = work.finally(() => {
      if (this.inflight === tracked) this.inflight = null;
    });
    this.inflight = tracked;
    return tracked;
  }

  /**
   * Membuka stream dari awal. Dengan `keep`, baris lama tetap tampil sampai baris baru siap,
   * dan jumlah yang dimuat disamakan dengan yang sudah ada agar posisi gulir bertahan.
   */
  open(keep = false): Promise<void> {
    if (this.disposed) return Promise.resolve();
    const session = ++this.session;
    this.closeStream();
    const target = keep ? Math.min(Math.max(this.state.rows.length, PAGE_SIZE), RELOAD_CAP) : PAGE_SIZE;
    this.set(keep ? { loading: true, error: null } : { rows: [], done: false, loading: true, error: null });
    return this.track(
      (async () => {
        try {
          const id = await commands.gitLogOpen(this.root, this.query);
          if (session !== this.session) {
            // Sudah digantikan (atau tab ditutup) selagi menunggu: stream ini tidak dipakai.
            void commands.gitLogClose(id).catch(() => undefined);
            return;
          }
          this.streamId = id;
          const rows: GraphRow[] = [];
          let done = false;
          while (!done && rows.length < target) {
            const page = await commands.gitLogNext(id, PAGE_SIZE);
            if (session !== this.session) return;
            for (const row of page.rows) rows.push(row);
            // Halaman kosong tanpa `done` tidak boleh membuat putaran tanpa akhir.
            done = page.done || page.rows.length === 0;
          }
          this.set({ rows, done, loading: false, error: null });
        } catch (e) {
          if (session !== this.session) return;
          this.closeStream();
          this.set({ loading: false, done: true, error: errorMessage(e) });
        }
      })(),
    );
  }

  /** Mengambil halaman berikutnya; tidak melakukan apa pun bila sedang memuat atau sudah habis. */
  loadMore(): Promise<void> {
    const id = this.streamId;
    if (this.disposed || this.state.loading || this.state.done || id === null) return Promise.resolve();
    const session = this.session;
    this.set({ loading: true });
    return this.track(
      (async () => {
        try {
          const page = await commands.gitLogNext(id, PAGE_SIZE);
          if (session !== this.session) return;
          this.set({ rows: [...this.state.rows, ...page.rows], done: page.done || page.rows.length === 0, loading: false });
        } catch (e) {
          if (session !== this.session) return;
          this.closeStream();
          this.set({ loading: false, done: true, error: errorMessage(e) });
        }
      })(),
    );
  }

  /**
   * Indeks baris commit `oid`, memuat halaman berikutnya sampai ketemu, stream habis, atau `cap` tercapai.
   * Mengembalikan -1 bila tidak ditemukan.
   */
  async find(oid: string, cap: number = FIND_CAP): Promise<number> {
    for (;;) {
      const index = this.state.rows.findIndex((r) => r.commit.oid === oid);
      if (index >= 0) return index;
      if (this.disposed || this.state.done || this.state.rows.length >= cap) return -1;
      if (this.inflight) await this.inflight;
      else if (this.streamId === null) return -1;
      else await this.loadMore();
    }
  }

  dispose(): void {
    this.disposed = true;
    this.session++;
    this.closeStream();
    this.listeners.clear();
  }
}

const noSubscribe = () => () => undefined;
const emptyState = () => EMPTY;

/** Stream log untuk `root` dan `query`; stream ditutup saat kueri berubah atau komponen dilepas. */
export function useLogStream(root: string, query: LogQuery): { controller: LogController | null; state: LogState } {
  const key = JSON.stringify(query);
  const [controller, setController] = useState<LogController | null>(null);
  useEffect(() => {
    const created = new LogController(root, JSON.parse(key) as LogQuery);
    setController(created);
    void created.open();
    return () => created.dispose();
  }, [root, key]);
  const state = useSyncExternalStore(controller?.subscribe ?? noSubscribe, controller?.getState ?? emptyState);
  return useMemo(() => ({ controller, state }), [controller, state]);
}
