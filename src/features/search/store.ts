// State pencarian teks: isian, hasil yang dialirkan backend, dan jalannya pencarian (debounce + pembatalan).
import { Channel } from "@tauri-apps/api/core";
import { create } from "zustand";
import { useApp } from "@/app/store";
import { commands, errorMessage, newOperationId, type FileMatches, type LineMatch, type SearchEvent, type SearchQuery, type SearchSummary } from "@/ipc";

export const SEARCH_DEBOUNCE_MS = 250;
export const MAX_RESULTS = 5000;
/** Hasil yang datang beruntun dikumpulkan sebentar agar daftar tidak digambar ulang per berkas. */
const FLUSH_MS = 40;

export type SearchStatus = "idle" | "running" | "done" | "failed";
export type MatchKey = Pick<LineMatch, "line" | "column" | "length">;
export type SearchOption = "isRegex" | "caseSensitive" | "wholeWord";

interface SearchState {
  pattern: string;
  replacement: string;
  isRegex: boolean;
  caseSensitive: boolean;
  wholeWord: boolean;
  /** Glob dipisah koma, apa adanya seperti diketik. */
  include: string;
  exclude: string;
  showReplace: boolean;
  showFilters: boolean;

  status: SearchStatus;
  error: string | null;
  summary: SearchSummary | null;
  /** Urut menurut path. Berkas dan kecocokan yang disingkirkan pengguna sudah tidak ada di sini. */
  files: FileMatches[];
  /** Kecocokan yang disingkirkan dari berkas yang masih tampil; "Replace All" tidak boleh menyentuhnya. */
  dismissed: Record<string, MatchKey[]>;
  collapsed: Record<string, boolean>;
  selected: string | null;
  /** Kueri yang menghasilkan `files`; dipakai "Replace All" agar sama persis dengan yang tampil. */
  resultQuery: SearchQuery | null;
  /** Isian berubah dan pencarian baru belum dimulai; hasil yang tampil sudah tidak mewakili isian. */
  pending: boolean;
  replacing: boolean;
  /** Bertambah setiap kali kolom kueri diminta fokus. */
  focusSeq: number;
}

const initial: SearchState = {
  pattern: "",
  replacement: "",
  isRegex: false,
  caseSensitive: false,
  wholeWord: false,
  include: "",
  exclude: "",
  showReplace: false,
  showFilters: false,
  status: "idle",
  error: null,
  summary: null,
  files: [],
  dismissed: {},
  collapsed: {},
  selected: null,
  resultQuery: null,
  pending: false,
  replacing: false,
  focusSeq: 0,
};

export const useSearch = create<SearchState>(() => initial);

// ----- Fungsi murni -----

/** "src, *.ts ,," → ["src", "*.ts"] */
export function parseGlobs(text: string): string[] {
  return text
    .split(",")
    .map((glob) => glob.trim())
    .filter(Boolean);
}

export function buildQuery(state: Pick<SearchState, "pattern" | "isRegex" | "caseSensitive" | "wholeWord" | "include" | "exclude">): SearchQuery {
  return {
    pattern: state.pattern,
    isRegex: state.isRegex,
    caseSensitive: state.caseSensitive,
    wholeWord: state.wholeWord,
    include: parseGlobs(state.include),
    exclude: parseGlobs(state.exclude),
    maxResults: MAX_RESULTS,
  };
}

export function resultCounts(files: readonly FileMatches[]): { files: number; matches: number } {
  return { files: files.length, matches: files.reduce((n, f) => n + f.matches.length, 0) };
}

const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? "" : "s"}`;

/** "3 results in 2 files", dengan catatan bila backend berhenti sebelum selesai. */
export function summaryText(files: readonly FileMatches[], truncated: boolean): string {
  const counts = resultCounts(files);
  return `${plural(counts.matches, "result")} in ${plural(counts.files, "file")}${truncated ? " — results truncated" : ""}`;
}

export type ResultRow = { kind: "file"; key: string; file: FileMatches; collapsed: boolean } | { kind: "match"; key: string; file: FileMatches; match: LineMatch };

export const matchKey = (path: string, match: MatchKey) => `${path}:${match.line}:${match.column}`;

/** Pohon hasil yang diratakan: baris berkas diikuti kecocokannya, kecuali berkas yang dilipat. */
export function flattenResults(files: readonly FileMatches[], collapsed: Record<string, boolean>): ResultRow[] {
  const rows: ResultRow[] = [];
  for (const file of files) {
    const closed = collapsed[file.path] === true;
    rows.push({ kind: "file", key: file.path, file, collapsed: closed });
    if (!closed) for (const match of file.matches) rows.push({ kind: "match", key: matchKey(file.path, match), file, match });
  }
  return rows;
}

function mergeSorted(files: FileMatches[], incoming: FileMatches[]): FileMatches[] {
  return [...files, ...incoming].sort((a, b) => (a.path < b.path ? -1 : a.path > b.path ? 1 : 0));
}

// ----- Jalannya pencarian -----

interface Run {
  id: string;
  finished: boolean;
}

/** Pencarian terbaru. Event dari pencarian lain (yang sudah dibatalkan) diabaikan. */
let current: Run | null = null;
let debounceTimer: ReturnType<typeof setTimeout> | undefined;
let flushTimer: ReturnType<typeof setTimeout> | undefined;
let buffered: FileMatches[] = [];

function flush() {
  clearTimeout(flushTimer);
  flushTimer = undefined;
  if (buffered.length === 0) return;
  const incoming = buffered;
  buffered = [];
  useSearch.setState((s) => ({ files: mergeSorted(s.files, incoming) }));
}

function cancelCurrent() {
  const previous = current;
  current = null;
  clearTimeout(flushTimer);
  flushTimer = undefined;
  buffered = [];
  if (previous && !previous.finished) void commands.appCancel(previous.id).catch(() => undefined);
}

/** Menerapkan satu event dari backend. Event milik pencarian yang sudah digantikan dibuang. */
function handleEvent(run: Run, event: SearchEvent) {
  if (current !== run || run.finished) return;
  switch (event.kind) {
    case "matches":
      buffered.push(event.file);
      flushTimer ??= setTimeout(flush, FLUSH_MS);
      break;
    case "done":
      run.finished = true;
      flush();
      useSearch.setState({ status: "done", summary: event.summary });
      break;
    case "failed":
      run.finished = true;
      flush();
      useSearch.setState({ status: "failed", error: event.message });
      break;
  }
}

const cleared = { status: "idle" as const, error: null, summary: null, files: [], dismissed: {}, collapsed: {}, selected: null, resultQuery: null, pending: false };

/** Memulai pencarian sekarang juga dengan isian saat ini; pencarian sebelumnya dibatalkan. Kueri kosong mengosongkan hasil. */
export function searchNow(): void {
  clearTimeout(debounceTimer);
  debounceTimer = undefined;
  cancelCurrent();
  const state = useSearch.getState();
  if (state.pattern === "" || !useApp.getState().workspace) {
    useSearch.setState(cleared);
    return;
  }
  const query = buildQuery(state);
  const run: Run = { id: newOperationId(), finished: false };
  current = run;
  useSearch.setState({ ...cleared, status: "running", resultQuery: query });

  const channel = new Channel<SearchEvent>();
  channel.onmessage = (event) => handleEvent(run, event);
  commands.searchStart(query, run.id, channel).catch((e: unknown) => handleEvent(run, { kind: "failed", message: errorMessage(e) }));
}

/** Menjadwalkan pencarian setelah jeda mengetik. */
export function scheduleSearch(delay = SEARCH_DEBOUNCE_MS): void {
  clearTimeout(debounceTimer);
  if (!useSearch.getState().pending) useSearch.setState({ pending: true });
  debounceTimer = setTimeout(searchNow, delay);
}

// ----- Isian -----

export function setPattern(pattern: string): void {
  useSearch.setState({ pattern });
  scheduleSearch();
}

export function setReplacement(replacement: string): void {
  useSearch.setState({ replacement });
}

export function toggleOption(option: SearchOption): void {
  useSearch.setState((s) => ({ [option]: !s[option] }));
  scheduleSearch();
}

export function setFilter(which: "include" | "exclude", value: string): void {
  useSearch.setState({ [which]: value });
  scheduleSearch();
}

export function toggleReplace(): void {
  useSearch.setState((s) => ({ showReplace: !s.showReplace }));
}

export function toggleFilters(): void {
  useSearch.setState((s) => ({ showFilters: !s.showFilters }));
}

/** Mengosongkan kueri dan hasil; opsi pencarian dipertahankan. */
export function clearSearch(): void {
  useSearch.setState({ pattern: "" });
  searchNow();
}

/** Meminta kolom kueri difokuskan; `prefill` (mis. teks terpilih di editor) menggantikan kueri dan langsung dicari. */
export function requestFocus(prefill?: string): void {
  if (prefill !== undefined && prefill !== useSearch.getState().pattern) {
    useSearch.setState({ pattern: prefill });
    searchNow();
  }
  useSearch.setState((s) => ({ focusSeq: s.focusSeq + 1 }));
}

// ----- Hasil -----

export function selectRow(key: string | null): void {
  if (useSearch.getState().selected !== key) useSearch.setState({ selected: key });
}

export function setCollapsed(path: string, collapsed: boolean): void {
  useSearch.setState((s) => ({ collapsed: { ...s.collapsed, [path]: collapsed } }));
}

export function setAllCollapsed(collapsed: boolean): void {
  useSearch.setState((s) => ({ collapsed: collapsed ? Object.fromEntries(s.files.map((f) => [f.path, true])) : {} }));
}

/** Menyingkirkan satu berkas dari hasil (dan dari "Replace All"). */
export function dismissFile(path: string): void {
  useSearch.setState((s) => {
    const { [path]: _dropped, ...dismissed } = s.dismissed;
    return { files: s.files.filter((f) => f.path !== path), dismissed };
  });
}

/** Menyingkirkan satu kecocokan; berkas yang kehabisan kecocokan ikut hilang. */
export function dismissMatch(path: string, match: MatchKey): void {
  const file = useSearch.getState().files.find((f) => f.path === path);
  if (!file) return;
  const matches = file.matches.filter((m) => !(m.line === match.line && m.column === match.column));
  if (matches.length === 0) return dismissFile(path);
  useSearch.setState((s) => ({
    files: s.files.map((f) => (f.path === path ? { ...f, matches } : f)),
    dismissed: { ...s.dismissed, [path]: [...(s.dismissed[path] ?? []), { line: match.line, column: match.column, length: match.length }] },
  }));
}

/** Folder proyek berganti: pencarian dibatalkan dan semuanya kembali ke keadaan awal. */
export function resetSearch(): void {
  clearTimeout(debounceTimer);
  debounceTimer = undefined;
  cancelCurrent();
  useSearch.setState((s) => ({ ...initial, focusSeq: s.focusSeq }));
}
