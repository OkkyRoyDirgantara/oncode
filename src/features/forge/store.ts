// State fitur forge: akun yang login, hubungan repositori aktif dengan forge, dan daftar PR/MR.
import { create } from "zustand";
import { commands, events, type Account, type ForgeContext, type PrFilter, type PullRequest } from "@/ipc";
import { describeForgeError } from "./util";

export type LoadStatus = "idle" | "loading" | "ready" | "error";

interface ForgeState {
  accounts: Account[];
  accountsLoaded: boolean;
  /** Akar repositori yang konteksnya sedang ditampilkan. */
  root: string | null;
  context: ForgeContext | null;
  contextStatus: LoadStatus;
  contextError: string | null;
  filter: PrFilter;
  prs: PullRequest[];
  nextPage: number | null;
  prStatus: LoadStatus;
  /** Sedang memuat halaman berikutnya. */
  loadingMore: boolean;
  prError: string | null;
}

const initial: ForgeState = {
  accounts: [],
  accountsLoaded: false,
  root: null,
  context: null,
  contextStatus: "idle",
  contextError: null,
  filter: "open",
  prs: [],
  nextPage: null,
  prStatus: "idle",
  loadingMore: false,
  prError: null,
};

export const useForge = create<ForgeState>(() => ({ ...initial }));

// Nomor urut permintaan: jawaban yang datang terlambat (repo sudah berganti) dibuang.
let contextSeq = 0;
let prSeq = 0;
let installed = false;

export async function loadAccounts(): Promise<Account[]> {
  try {
    const accounts = await commands.forgeAccounts();
    useForge.setState({ accounts, accountsLoaded: true });
    return accounts;
  } catch {
    useForge.setState({ accountsLoaded: true });
    return useForge.getState().accounts;
  }
}

/** Mendengarkan perubahan daftar akun. Aman dipanggil berkali-kali. */
export function installForgeStore(): void {
  if (installed) return;
  installed = true;
  void loadAccounts();
  // Tampilan sidebar me-resolve ulang konteks saat daftar akun berubah.
  void events.accountsChanged.listen(() => void loadAccounts());
}

function clearPrs() {
  prSeq += 1;
  useForge.setState({ prs: [], nextPage: null, prStatus: "idle", loadingMore: false, prError: null });
}

/** Mencari tahu forge mana yang menjadi tujuan remote repositori ini, lalu memuat daftar PR bila ada akunnya. */
export async function resolveContext(root: string | null, options: { reload?: boolean } = {}): Promise<ForgeContext | null> {
  const seq = ++contextSeq;
  if (!root) {
    clearPrs();
    useForge.setState({ root: null, context: null, contextStatus: "idle", contextError: null });
    return null;
  }
  const previous = useForge.getState();
  const sameRoot = previous.root === root;
  if (!sameRoot) clearPrs();
  useForge.setState({ root, contextStatus: sameRoot && previous.contextStatus === "ready" ? "ready" : "loading", contextError: null, ...(sameRoot ? {} : { context: null }) });
  try {
    const context = await commands.forgeContext(root);
    if (seq !== contextSeq) return context;
    const before = useForge.getState().context;
    useForge.setState({ context, contextStatus: "ready", contextError: null });
    // Daftar PR hanya dimuat ulang bila repositori atau akunnya memang berganti.
    const changed = contextKey(before) !== contextKey(context);
    if (!context?.account) clearPrs();
    else if (options.reload || changed || useForge.getState().prStatus === "idle") void loadPrs();
    return context;
  } catch (e) {
    if (seq !== contextSeq) return null;
    clearPrs();
    useForge.setState({ context: null, contextStatus: "error", contextError: describeForgeError(e).title });
    return null;
  }
}

function contextKey(context: ForgeContext | null): string {
  if (!context) return "";
  return `${context.account?.id ?? ""}|${context.repo.host}/${context.repo.owner}/${context.repo.name}`;
}

/** Memuat halaman pertama daftar PR/MR untuk konteks saat ini. */
export async function loadPrs(): Promise<void> {
  const { context, filter } = useForge.getState();
  if (!context?.account) return;
  const seq = ++prSeq;
  useForge.setState({ prStatus: "loading", prError: null, loadingMore: false });
  try {
    const page = await commands.forgePrs(context.account.id, context.repo, filter, 1);
    if (seq !== prSeq) return;
    useForge.setState({ prs: page.items, nextPage: page.nextPage, prStatus: "ready" });
  } catch (e) {
    if (seq !== prSeq) return;
    const text = describeForgeError(e, context.account);
    useForge.setState({ prs: [], nextPage: null, prStatus: "error", prError: text.detail && text.detail !== text.title ? `${text.title} (${text.detail})` : text.title });
  }
}

/** Menambahkan halaman berikutnya ke daftar. */
export async function loadMorePrs(): Promise<void> {
  const { context, filter, nextPage, loadingMore, prStatus } = useForge.getState();
  if (!context?.account || nextPage === null || loadingMore || prStatus !== "ready") return;
  const seq = prSeq;
  useForge.setState({ loadingMore: true, prError: null });
  try {
    const page = await commands.forgePrs(context.account.id, context.repo, filter, nextPage);
    if (seq !== prSeq) return;
    // Daftar di server bisa bergeser di antara dua permintaan; nomor yang sudah ada tidak digandakan.
    const known = new Set(useForge.getState().prs.map((p) => p.number));
    useForge.setState((s) => ({ prs: [...s.prs, ...page.items.filter((p) => !known.has(p.number))], nextPage: page.nextPage, loadingMore: false }));
  } catch (e) {
    if (seq !== prSeq) return;
    useForge.setState({ loadingMore: false, prError: describeForgeError(e, context.account).title });
  }
}

export function setFilter(filter: PrFilter): void {
  if (useForge.getState().filter === filter) return;
  useForge.setState({ filter });
  void loadPrs();
}

/** Memuat ulang akun, konteks, dan daftar PR. */
export async function refreshForge(): Promise<void> {
  await loadAccounts();
  const root = useForge.getState().root;
  if (!root) return;
  await resolveContext(root, { reload: true });
}

/** Hanya untuk pengujian. */
export function resetForgeForTest(): void {
  contextSeq += 1;
  prSeq += 1;
  installed = false;
  useForge.setState({ ...initial });
}
