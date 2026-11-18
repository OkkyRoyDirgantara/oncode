// Data contoh untuk pengujian fitur forge. Tidak dipakai oleh kode aplikasi.
import type { Account, AppError, Branch, ForgeContext, PullRequest, Refs, RemoteRepo, RepoSnapshot, WorkspaceInfo } from "@/ipc";
import type { RepoState } from "@/features/scm/store";
import { cloneSettings, defaults } from "@/features/settings/fixtures";

export const ROOT = "/work/project";

export const githubAccount: Account = {
  id: "github:github.com:octocat",
  kind: "github",
  host: "https://github.com",
  username: "octocat",
  displayName: "The Octocat",
  avatarUrl: "https://avatars.example/u/1",
};

export const gitlabAccount: Account = {
  id: "gitlab:gitlab.com:tanuki",
  kind: "gitlab",
  host: "https://gitlab.com",
  username: "tanuki",
  displayName: "Tanuki",
  avatarUrl: null,
};

export function makeContext(overrides: Partial<ForgeContext> = {}): ForgeContext {
  return {
    remote: "origin",
    repo: { host: "github.com", owner: "acme", name: "widgets" },
    kind: "github",
    account: githubAccount,
    webUrl: "https://github.com/acme/widgets",
    ...overrides,
  };
}

export function makePr(number: number, overrides: Partial<PullRequest> = {}): PullRequest {
  return {
    number,
    title: `Change number ${number}`,
    state: "open",
    draft: false,
    author: "alice",
    authorAvatar: null,
    sourceBranch: `feature/${number}`,
    targetBranch: "main",
    headSha: `${number}`.padStart(40, "a"),
    updatedAt: Math.floor(Date.now() / 1000) - 3600,
    webUrl: `https://github.com/acme/widgets/pull/${number}`,
    isCrossRepo: false,
    ...overrides,
  };
}

export function makeRemoteRepo(fullName: string, overrides: Partial<RemoteRepo> = {}): RemoteRepo {
  return {
    fullName,
    description: "",
    private: false,
    fork: false,
    cloneUrlHttps: `https://github.com/${fullName}.git`,
    cloneUrlSsh: `git@github.com:${fullName}.git`,
    defaultBranch: "main",
    updatedAt: Math.floor(Date.now() / 1000) - 86400,
    webUrl: `https://github.com/${fullName}`,
    ...overrides,
  };
}

export function makeBranch(name: string, overrides: Partial<Branch> = {}): Branch {
  return { name, oid: "b".repeat(40), isHead: false, upstream: null, ahead: 0, behind: 0, upstreamGone: false, worktree: null, subject: `Subject of ${name}`, time: 0, ...overrides };
}

export function makeSnapshot(overrides: Partial<RepoSnapshot> = {}): RepoSnapshot {
  return {
    root: ROOT,
    head: { kind: "branch", name: "feature/login" },
    headOid: "b".repeat(40),
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

export function makeRefs(overrides: Partial<Refs> = {}): Refs {
  return {
    branches: [makeBranch("feature/login", { isHead: true, subject: "Add login form" }), makeBranch("main")],
    remoteBranches: [
      { name: "origin/HEAD", remote: "origin", oid: "c".repeat(40) },
      { name: "origin/main", remote: "origin", oid: "c".repeat(40) },
      { name: "origin/develop", remote: "origin", oid: "d".repeat(40) },
    ],
    tags: [],
    remotes: [{ name: "origin", fetchUrl: "https://github.com/acme/widgets.git", pushUrl: "https://github.com/acme/widgets.git" }],
    ...overrides,
  };
}

export function makeRepoState(overrides: Partial<RepoState> = {}): RepoState {
  return { root: ROOT, snapshot: makeSnapshot(), refs: makeRefs(), stashes: [], worktrees: [], submodules: [], busy: null, ...overrides };
}

export function makeWorkspace(overrides: Partial<WorkspaceInfo> = {}): WorkspaceInfo {
  return { root: ROOT, name: "project", trusted: true, repoRoot: ROOT, settings: cloneSettings(defaults), ...overrides };
}

export function appError(code: string, message = code): AppError {
  return { code, message, detail: null };
}
