// State repositori di sisi UI. Backend menyiarkan snapshot; UI hanya menyimpannya dan menggambar ulang.
import { create } from "zustand";
import { commands, events, type Refs, type RepoSnapshot, type Stash, type Submodule, type Worktree } from "@/ipc";
import { onWorkspaceChanged } from "@/app/store";

export interface Busy {
  label: string;
  /** 0–100, atau null bila tidak diketahui. */
  percent: number | null;
  /** Untuk membatalkan lewat `commands.appCancel`. */
  operationId?: string;
}

export interface RepoState {
  root: string;
  snapshot: RepoSnapshot | null;
  refs: Refs | null;
  stashes: Stash[];
  worktrees: Worktree[];
  submodules: Submodule[];
  busy: Busy | null;
}

interface ScmState {
  repos: Record<string, RepoState>;
  /** Repositori yang sedang ditampilkan di panel Source Control dan status bar. */
  activeRoot: string | null;
}

export const useScm = create<ScmState>(() => ({ repos: {}, activeRoot: null }));

const emptyRepo = (root: string): RepoState => ({ root, snapshot: null, refs: null, stashes: [], worktrees: [], submodules: [], busy: null });

function patchRepo(root: string, patch: Partial<RepoState>) {
  useScm.setState((s) => ({ repos: { ...s.repos, [root]: { ...(s.repos[root] ?? emptyRepo(root)), ...patch } } }));
}

export function setBusy(root: string, busy: Busy | null): void {
  if (useScm.getState().repos[root]) patchRepo(root, { busy });
}

export function activeRepo(): RepoState | null {
  const s = useScm.getState();
  return s.activeRoot ? (s.repos[s.activeRoot] ?? null) : null;
}

export function useActiveRepo(): RepoState | null {
  return useScm((s) => (s.activeRoot ? (s.repos[s.activeRoot] ?? null) : null));
}

export function useRepo(root: string | null | undefined): RepoState | null {
  return useScm((s) => (root ? (s.repos[root] ?? null) : null));
}

export function setActiveRepo(root: string): void {
  if (useScm.getState().repos[root]) useScm.setState({ activeRoot: root });
}

const refsListeners = new Set<(root: string) => void>();

/** Dipanggil saat branch, tag, atau HEAD sebuah repositori berubah (mis. untuk memuat ulang graph). */
export function onRefsChanged(listener: (root: string) => void): () => void {
  refsListeners.add(listener);
  return () => refsListeners.delete(listener);
}

const pendingRefs = new Map<string, ReturnType<typeof setTimeout>>();

/** Memuat ulang ref, stash, worktree, dan submodule sebuah repositori. */
export async function reloadRefs(root: string): Promise<void> {
  if (!useScm.getState().repos[root]) return;
  const [refs, stashes, worktrees, submodules] = await Promise.all([
    commands.gitRefs(root).catch(() => null),
    commands.gitStashList(root).catch(() => [] as Stash[]),
    commands.gitWorktrees(root).catch(() => [] as Worktree[]),
    commands.gitSubmodules(root).catch(() => [] as Submodule[]),
  ]);
  if (!useScm.getState().repos[root]) return;
  patchRepo(root, { refs: refs ?? useScm.getState().repos[root].refs, stashes, worktrees, submodules });
}

function scheduleRefs(root: string) {
  clearTimeout(pendingRefs.get(root));
  pendingRefs.set(
    root,
    setTimeout(() => {
      pendingRefs.delete(root);
      void reloadRefs(root);
      refsListeners.forEach((l) => l(root));
    }, 120),
  );
}

/** Mendaftarkan repositori (akar workspace atau submodule) dan memuat keadaannya. */
export async function addRepo(root: string): Promise<void> {
  if (!useScm.getState().repos[root]) patchRepo(root, {});
  if (!useScm.getState().activeRoot) useScm.setState({ activeRoot: root });
  const snapshot = await commands.gitSnapshot(root).catch(() => null);
  if (snapshot && useScm.getState().repos[root]) patchRepo(root, { snapshot });
  await reloadRefs(root);
}

/** Membuka repositori di `path` (mis. submodule) dan menjadikannya aktif. */
export async function openRepo(path: string): Promise<string> {
  const root = await commands.gitOpen(path);
  await addRepo(root);
  useScm.setState({ activeRoot: root });
  return root;
}

export function installScmStore(): void {
  void events.repoChanged.listen((e) => {
    const { root, snapshot } = e.payload;
    if (!useScm.getState().repos[root]) return;
    const previous = useScm.getState().repos[root].snapshot;
    patchRepo(root, { snapshot });
    // Jumlah stash dan HEAD ikut menentukan daftar yang ditampilkan.
    if (!previous || previous.stashCount !== snapshot.stashCount || previous.headOid !== snapshot.headOid || JSON.stringify(previous.head) !== JSON.stringify(snapshot.head)) {
      scheduleRefs(root);
    }
  });
  void events.refsChanged.listen((e) => {
    if (useScm.getState().repos[e.payload.root]) scheduleRefs(e.payload.root);
  });
  onWorkspaceChanged((ws) => {
    useScm.setState({ repos: {}, activeRoot: null });
    if (ws?.repoRoot) void addRepo(ws.repoRoot);
  });
}

/** Jumlah berkas yang berubah, untuk lencana ikon Source Control. */
export function changeCount(snapshot: RepoSnapshot | null): number {
  if (!snapshot) return 0;
  const paths = new Set<string>();
  for (const c of snapshot.staged) paths.add(c.path);
  for (const c of snapshot.unstaged) paths.add(c.path);
  for (const p of snapshot.untracked) paths.add(p);
  for (const c of snapshot.conflicted) paths.add(c.path);
  return paths.size;
}

/** Nama pendek untuk HEAD saat ini. */
export function headLabel(snapshot: RepoSnapshot | null): string {
  if (!snapshot) return "";
  switch (snapshot.head.kind) {
    case "branch":
    case "unborn":
      return snapshot.head.name;
    case "detached":
      return snapshot.head.oid.slice(0, 8);
  }
}

/** Hanya untuk pengujian. */
export function resetScmForTest(repos: Record<string, RepoState> = {}, activeRoot: string | null = null): void {
  useScm.setState({ repos, activeRoot });
}
