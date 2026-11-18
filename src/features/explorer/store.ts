// State pohon explorer: isi folder yang sudah dimuat, folder yang terbuka, dan baris terpilih.
import { create } from "zustand";
import { dirName } from "@/app/store";
import { commands, errorMessage, type DirEntry, type FsChanged } from "@/ipc";
import { ancestorChain, isSameOrUnder, movedPath, type DirState } from "./tree";

interface ExplorerState {
  /** Folder proyek yang sedang ditampilkan. */
  root: string | null;
  dirs: Record<string, DirState>;
  expanded: Record<string, boolean>;
  selected: string | null;
  /** Bertambah setiap kali sebuah path diminta tampil (agar tampilan menggulir ke sana). */
  revealSeq: number;
}

const initial: ExplorerState = { root: null, dirs: {}, expanded: {}, selected: null, revealSeq: 0 };

export const useExplorer = create<ExplorerState>(() => initial);

/** Bertambah saat folder proyek berganti; jawaban dari folder lama diabaikan. */
let generation = 0;
let requestCounter = 0;
/** Permintaan terbaru per folder; jawaban yang lebih lama tidak boleh menimpa yang baru. */
const latestRequest = new Map<string, number>();
const inflight = new Map<string, Promise<boolean>>();

function omit<T>(record: Record<string, T>, drop: (key: string) => boolean): Record<string, T> {
  let changed = false;
  const next: Record<string, T> = {};
  for (const [key, value] of Object.entries(record)) {
    if (drop(key)) changed = true;
    else next[key] = value;
  }
  return changed ? next : record;
}

/** Mengganti folder proyek dan mengosongkan pohon. Tidak berbuat apa-apa bila folder sama. */
export function setRoot(root: string | null): void {
  if (useExplorer.getState().root === root) return;
  generation += 1;
  latestRequest.clear();
  inflight.clear();
  useExplorer.setState({ ...initial, root });
  if (root) void loadDir(root);
}

/** Entri yang hilang dari sebuah folder ikut membawa pergi keadaan (cache, terbuka, terpilih) di bawahnya. */
function forgetRemoved(previous: DirEntry[] | null, entries: DirEntry[]) {
  if (!previous) return;
  const present = new Set(entries.map((e) => e.path));
  const removed = previous.filter((e) => !present.has(e.path)).map((e) => e.path);
  if (removed.length === 0) return;
  const gone = (key: string) => removed.some((path) => isSameOrUnder(key, path));
  useExplorer.setState((s) => ({
    dirs: omit(s.dirs, gone),
    expanded: omit(s.expanded, gone),
    selected: s.selected && gone(s.selected) ? null : s.selected,
  }));
}

/**
 * Memuat (ulang) isi satu folder. Mengembalikan true bila isinya kini tersedia.
 * Tanpa `force`, pemuatan yang sedang berjalan untuk folder yang sama dipakai bersama.
 */
export function loadDir(path: string, force = false): Promise<boolean> {
  const pending = inflight.get(path);
  if (pending && !force) return pending;

  const startedIn = generation;
  const request = ++requestCounter;
  latestRequest.set(path, request);
  const superseded = () => startedIn !== generation || latestRequest.get(path) !== request;
  // Permintaan yang lebih baru untuk folder yang sama: hasilnya yang berlaku.
  const newerResult = (): boolean | Promise<boolean> =>
    startedIn === generation ? (inflight.get(path) ?? useExplorer.getState().dirs[path]?.entries != null) : false;

  useExplorer.setState((s) => ({ dirs: { ...s.dirs, [path]: { entries: s.dirs[path]?.entries ?? null, error: s.dirs[path]?.error ?? null, loading: true } } }));
  // Folder yang sementara itu sudah hilang dari isi induknya tidak dihidupkan lagi oleh jawaban yang terlambat.
  const orphaned = () => {
    const s = useExplorer.getState();
    const siblings = path === s.root ? null : s.dirs[dirName(path)]?.entries;
    if (!siblings || siblings.some((e) => e.path === path)) return false;
    useExplorer.setState({ dirs: omit(s.dirs, (key) => key === path) });
    return true;
  };
  const promise: Promise<boolean> = commands.fsReadDir(path).then(
    (entries) => {
      if (superseded()) return newerResult();
      inflight.delete(path);
      if (orphaned()) return false;
      const previous = useExplorer.getState().dirs[path]?.entries ?? null;
      useExplorer.setState((s) => ({ dirs: { ...s.dirs, [path]: { entries, loading: false, error: null } } }));
      forgetRemoved(previous, entries);
      return true;
    },
    (error: unknown) => {
      if (superseded()) return newerResult();
      inflight.delete(path);
      if (orphaned()) return false;
      useExplorer.setState((s) => ({ dirs: { ...s.dirs, [path]: { entries: null, loading: false, error: errorMessage(error) } } }));
      return false;
    },
  );
  inflight.set(path, promise);
  return promise;
}

export async function expandDir(path: string): Promise<void> {
  const state = useExplorer.getState();
  if (!state.expanded[path]) useExplorer.setState((s) => ({ expanded: { ...s.expanded, [path]: true } }));
  if (!state.dirs[path]?.entries) await loadDir(path);
}

export function collapseDir(path: string): void {
  useExplorer.setState((s) => ({ expanded: omit(s.expanded, (key) => key === path) }));
}

export function toggleDir(path: string): void {
  if (useExplorer.getState().expanded[path]) collapseDir(path);
  else void expandDir(path);
}

export function collapseAll(): void {
  useExplorer.setState({ expanded: {} });
}

export function selectPath(path: string | null): void {
  if (useExplorer.getState().selected !== path) useExplorer.setState({ selected: path });
}

/**
 * Menyegarkan `path` dan semua folder di dalamnya: yang terbuka dimuat ulang, yang tertutup
 * dibuang cache-nya dan dimuat lagi saat dibuka.
 */
export async function reloadUnder(path: string): Promise<void> {
  const { root, expanded } = useExplorer.getState();
  if (!root) return;
  const open = (key: string) => key === root || expanded[key] === true;
  useExplorer.setState((s) => ({ dirs: omit(s.dirs, (key) => !open(key) && isSameOrUnder(key, path)) }));
  const targets = [root, ...Object.keys(expanded)].filter((key) => open(key) && isSameOrUnder(key, path));
  await Promise.all(targets.map((key) => loadDir(key, true)));
}

/** Memuat ulang akar dan setiap folder yang terbuka; cache folder lain dibuang. Bentangan dan pilihan dipertahankan. */
export async function refreshAll(): Promise<void> {
  const root = useExplorer.getState().root;
  if (root) await reloadUnder(root);
}

/**
 * Menanggapi perubahan dari luar: folder terbuka yang memuat path yang berubah dimuat ulang;
 * folder tertutup yang sudah pernah dimuat cukup dibuang cache-nya.
 */
export async function applyFsChanges(paths: Iterable<string>, overflow: boolean): Promise<void> {
  const { root, dirs, expanded } = useExplorer.getState();
  if (!root) return;
  if (overflow) return refreshAll();
  const reload = new Set<string>();
  const drop = new Set<string>();
  for (const path of paths) {
    // Induknya berubah isi; path itu sendiri bisa berupa folder yang isinya berubah.
    for (const dir of [dirName(path), path]) {
      if (!dirs[dir]) continue;
      if (dir === root || expanded[dir]) reload.add(dir);
      else drop.add(dir);
    }
  }
  if (drop.size > 0) useExplorer.setState((s) => ({ dirs: omit(s.dirs, (key) => drop.has(key)) }));
  await Promise.all([...reload].map((dir) => loadDir(dir, true)));
}

const FS_DEBOUNCE_MS = 150;
let pendingPaths = new Set<string>();
let pendingOverflow = false;
let pendingTimer: ReturnType<typeof setTimeout> | undefined;

/** Mengumpulkan event `fs-changed` yang beruntun menjadi satu pemuatan ulang. */
export function queueFsChange(change: FsChanged): void {
  for (const path of change.paths) pendingPaths.add(path);
  pendingOverflow ||= change.overflow;
  clearTimeout(pendingTimer);
  pendingTimer = setTimeout(() => {
    const paths = pendingPaths;
    const overflow = pendingOverflow;
    pendingPaths = new Set();
    pendingOverflow = false;
    void applyFsChanges(paths, overflow);
  }, FS_DEBOUNCE_MS);
}

/**
 * Membuka semua folder leluhur `path`, memilihnya, dan meminta tampilan menggulir ke sana.
 * Mengembalikan false bila `path` tidak ada di pohon (di luar proyek, sudah dihapus, atau di dalam `.git`).
 */
export async function revealPath(path: string): Promise<boolean> {
  const root = useExplorer.getState().root;
  if (!root) return false;
  const chain = ancestorChain(root, path);
  if (chain.length === 0) return false;
  const startedIn = generation;
  for (let i = 0; i < chain.length; i++) {
    const dir = chain[i];
    if (!useExplorer.getState().dirs[dir]?.entries && !(await loadDir(dir))) return false;
    if (startedIn !== generation) return false;
    const next = chain[i + 1] ?? path;
    if (!useExplorer.getState().dirs[dir]?.entries?.some((e) => e.path === next)) return false;
  }
  useExplorer.setState((s) => {
    const expanded = { ...s.expanded };
    for (const dir of chain.slice(1)) expanded[dir] = true;
    return { expanded, selected: path, revealSeq: s.revealSeq + 1 };
  });
  return true;
}

/** Setelah ganti nama: keadaan (cache, terbuka, terpilih) di bawah `from` ikut pindah ke `to`. */
export function moveTreeState(from: string, to: string): void {
  const rekey = <T,>(record: Record<string, T>): Record<string, T> => {
    const next: Record<string, T> = {};
    for (const [key, value] of Object.entries(record)) next[movedPath(key, from, to) ?? key] = value;
    return next;
  };
  useExplorer.setState((s) => ({
    // Entri di cache masih memuat path lama; buang, lalu pemanggil memuat ulang lewat `reloadUnder`.
    dirs: omit(s.dirs, (key) => isSameOrUnder(key, from)),
    expanded: rekey(s.expanded),
    selected: s.selected ? (movedPath(s.selected, from, to) ?? s.selected) : null,
  }));
}

/** Hanya untuk pengujian. */
export function resetExplorerForTest(): void {
  generation += 1;
  latestRequest.clear();
  inflight.clear();
  clearTimeout(pendingTimer);
  pendingPaths = new Set();
  pendingOverflow = false;
  useExplorer.setState(initial);
}
