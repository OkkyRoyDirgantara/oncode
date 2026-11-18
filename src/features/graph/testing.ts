// Pembantu pengujian fitur Graph: pembuat data palsu dan keadaan repositori.
import type { Branch, Commit, GraphRow, Refs, RepoSnapshot, Stash } from "@/ipc";
import type { RepoState } from "@/features/scm/store";

export const ROOT = "/work/repo";

/** Hash 40 karakter yang mudah dikenali: `oid(3)` diawali `0003`. */
export function oid(n: number): string {
  return n.toString(16).padStart(4, "0").padEnd(40, "a");
}

export function commit(n: number, overrides: Partial<Commit> = {}): Commit {
  const who = { name: "Ana", email: "ana@example.com", time: 1_700_000_000 - n * 60 };
  return { oid: oid(n), parents: [oid(n + 1)], author: who, committer: who, subject: `Commit ${n}`, refs: [], ...overrides };
}

/** Baris lurus di jalur 0: commit `from` … `from + count - 1` (yang terbaru lebih dulu). */
export function linearRows(from: number, count: number): GraphRow[] {
  return Array.from({ length: count }, (_, i) => ({
    commit: commit(from + i),
    column: 0,
    color: 0,
    edges: [
      ...(from + i > 0 ? [{ from: 0, to: 0, color: 0, kind: "top" as const }] : []),
      { from: 0, to: 0, color: 0, kind: "bottom" as const },
    ],
  }));
}

export function branch(name: string, overrides: Partial<Branch> = {}): Branch {
  return { name, oid: oid(0), isHead: false, upstream: null, ahead: 0, behind: 0, upstreamGone: false, worktree: null, subject: "", time: 0, ...overrides };
}

export function snapshot(overrides: Partial<RepoSnapshot> = {}): RepoSnapshot {
  return {
    root: ROOT,
    head: { kind: "branch", name: "main" },
    headOid: oid(0),
    upstream: null,
    operation: null,
    staged: [],
    unstaged: [],
    untracked: [],
    conflicted: [],
    stashCount: 0,
    ...overrides,
  };
}

export function repoState(overrides: Partial<RepoState> = {}): RepoState {
  const refs: Refs = { branches: [branch("main", { isHead: true })], remoteBranches: [], tags: [], remotes: [] };
  return { root: ROOT, snapshot: snapshot(), refs, stashes: [] as Stash[], worktrees: [], submodules: [], busy: null, ...overrides };
}

/** Membuat elemen di jsdom tampak punya ukuran, agar virtualizer merender baris. */
export function stubViewport(height: number, width = 900): () => void {
  const proto = HTMLElement.prototype;
  const previous = { h: Object.getOwnPropertyDescriptor(proto, "clientHeight"), w: Object.getOwnPropertyDescriptor(proto, "clientWidth") };
  Object.defineProperty(proto, "clientHeight", { configurable: true, get: () => height });
  Object.defineProperty(proto, "clientWidth", { configurable: true, get: () => width });
  return () => {
    if (previous.h) Object.defineProperty(proto, "clientHeight", previous.h);
    else delete (proto as unknown as Record<string, unknown>).clientHeight;
    if (previous.w) Object.defineProperty(proto, "clientWidth", previous.w);
    else delete (proto as unknown as Record<string, unknown>).clientWidth;
  };
}
