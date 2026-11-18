// Fitur Source Control: panel, tab diff dan konflik, status bar, Git Output, command, dan ekstensi editor.
import { Channel } from "@tauri-apps/api/core";
import { ArrowDown, ArrowUp, CloudUpload, GitBranch, GitCompare, GitMerge, LoaderCircle, RefreshCw } from "lucide-react";
import { registerCommands } from "@/app/commands";
import { quickPick, toast } from "@/app/overlay";
import { registerPanelView, registerSidebarView, registerStatusItem, registerTabType } from "@/app/registry";
import { revealSidebar, useApp } from "@/app/store";
import { commands, type Progress } from "@/ipc";
import { activeEditor } from "../editor/CodeEditor";
import { registerEditorExtension } from "../editor/extensions";
import * as git from "./actions";
import { ConflictTab, confirmCloseConflict, conflictTabDirty } from "./ConflictTab";
import { DiffTab } from "./DiffTab";
import { GitOutputActions, GitOutputPanel, installGitOutput } from "./GitOutput";
import { gitEditorExtension, repoForPath, toggleBlame } from "./gutter";
import { openDiff } from "./open";
import { installPromptHandler } from "./prompts";
import { ScmView } from "./ScmView";
import { activeRepo, changeCount, headLabel, installScmStore, useActiveRepo, useScm } from "./store";

function BranchStatus() {
  const repo = useActiveRepo();
  if (!repo?.snapshot) return null;
  const { snapshot } = repo;
  const operation = snapshot.operation ? ` (${snapshot.operation.kind === "cherryPick" ? "cherry-pick" : snapshot.operation.kind})` : "";
  return (
    <button className="oc-status-item" title="Switch branch" onClick={() => void git.pickBranch(repo.root)}>
      {snapshot.operation ? <GitMerge size={13} /> : <GitBranch size={13} />}
      {headLabel(snapshot)}
      {snapshot.head.kind === "detached" ? " (detached)" : ""}
      {operation}
    </button>
  );
}

function SyncStatus() {
  const repo = useActiveRepo();
  if (!repo?.snapshot) return null;
  const { snapshot } = repo;
  if (repo.busy) {
    return (
      <span className="oc-status-item" title={repo.busy.label}>
        <LoaderCircle size={13} className="oc-spin" />
        <span className="oc-ellipsis" style={{ maxWidth: 260 }}>
          {repo.busy.label}
          {repo.busy.percent != null ? ` ${repo.busy.percent}%` : ""}
        </span>
      </span>
    );
  }
  if (snapshot.head.kind !== "branch") return null;
  const upstream = snapshot.upstream;
  if (!upstream?.exists) {
    if ((repo.refs?.remotes.length ?? 0) === 0) return null;
    return (
      <button className="oc-status-item" title="Publish branch" onClick={() => void git.push(repo.root, { setUpstream: true })}>
        <CloudUpload size={13} />
      </button>
    );
  }
  return (
    <button className="oc-status-item" title={`Sync with ${upstream.name}: pull ${upstream.behind}, push ${upstream.ahead}`} onClick={() => void git.sync(repo.root)}>
      <RefreshCw size={12} />
      {upstream.behind}
      <ArrowDown size={12} />
      {upstream.ahead}
      <ArrowUp size={12} />
    </button>
  );
}

/** Fetch berkala untuk repositori aktif; galat diabaikan agar tidak mengganggu. */
function installAutoFetch() {
  let last = Date.now();
  setInterval(() => {
    const minutes = useApp.getState().settings?.git.autoFetchMinutes ?? 0;
    const repo = activeRepo();
    if (!minutes || !repo || repo.busy || !document.hasFocus()) return;
    if ((repo.refs?.remotes.length ?? 0) === 0 || repo.snapshot?.operation) return;
    if (Date.now() - last < minutes * 60_000) return;
    last = Date.now();
    void commands.gitFetch(repo.root, null, `auto-fetch-${last}`, new Channel<Progress>()).catch(() => {});
  }, 30_000);
}

async function pickRef(root: string, placeholder: string): Promise<string | null> {
  const refs = useScm.getState().repos[root]?.refs;
  if (!refs) return null;
  const current = refs.branches.find((b) => b.isHead)?.name;
  return quickPick({
    placeholder,
    items: [
      ...refs.branches.filter((b) => b.name !== current).map((b) => ({ label: b.name, description: "branch", value: b.name })),
      ...refs.remoteBranches.map((b) => ({ label: b.name, description: "remote branch", value: b.name })),
      ...refs.tags.map((t) => ({ label: t.name, description: "tag", value: t.name })),
    ],
  });
}

/** Menjalankan aksi pada repositori aktif, atau memberi tahu bila tidak ada. */
function withRepo(action: (root: string) => unknown): () => unknown {
  return () => {
    const root = git.requireRepo();
    return root ? action(root) : undefined;
  };
}

export function registerScmFeature(): void {
  installScmStore();
  installPromptHandler();
  installGitOutput();
  installAutoFetch();

  registerSidebarView({
    id: "scm",
    title: "Source Control",
    icon: <GitBranch size={20} />,
    order: 30,
    component: ScmView,
    useBadge: () => {
      const count = changeCount(useActiveRepo()?.snapshot ?? null);
      return count > 0 ? count : undefined;
    },
  });
  registerTabType({ kind: "diff", component: DiffTab, icon: () => <GitCompare size={14} /> });
  registerTabType({
    kind: "conflict",
    component: ConflictTab,
    icon: () => <GitMerge size={14} />,
    useDirty: conflictTabDirty,
    beforeClose: confirmCloseConflict,
  });
  registerPanelView({ id: "gitOutput", title: "Git Output", order: 30, component: GitOutputPanel, actions: GitOutputActions });
  registerStatusItem({ id: "scm.branch", align: "left", order: 10, component: BranchStatus });
  registerStatusItem({ id: "scm.sync", align: "left", order: 11, component: SyncStatus });
  registerEditorExtension(gitEditorExtension);

  registerCommands([
    { id: "scm.focus", title: "Show Source Control", category: "View", keybinding: "Mod+Shift+G", run: () => revealSidebar("scm") },
    {
      id: "git.commit",
      title: "Commit…",
      category: "Git",
      run: withRepo(() => {
        revealSidebar("scm");
        setTimeout(() => window.dispatchEvent(new Event("oncode:focus-commit")), 50);
      }),
    },
    { id: "git.stageAll", title: "Stage All Changes", category: "Git", run: withRepo(git.stageAll) },
    { id: "git.unstageAll", title: "Unstage All Changes", category: "Git", run: withRepo(git.unstageAll) },
    { id: "git.pull", title: "Pull", category: "Git", run: withRepo(git.pull) },
    { id: "git.push", title: "Push", category: "Git", run: withRepo((root) => git.push(root)) },
    { id: "git.forcePush", title: "Force Push (with lease)…", category: "Git", run: withRepo((root) => git.push(root, { forceWithLease: true })) },
    { id: "git.fetch", title: "Fetch", category: "Git", run: withRepo((root) => git.fetch(root)) },
    { id: "git.sync", title: "Sync (Pull then Push)", category: "Git", run: withRepo(git.sync) },
    { id: "git.switchBranch", title: "Switch Branch…", category: "Git", run: withRepo(git.pickBranch) },
    { id: "git.createBranch", title: "Create Branch…", category: "Git", run: withRepo((root) => git.createBranch(root)) },
    { id: "git.createTag", title: "Create Tag…", category: "Git", run: withRepo((root) => git.createTag(root)) },
    { id: "git.stash", title: "Stash Changes…", category: "Git", run: withRepo((root) => git.stashPush(root)) },
    {
      id: "git.stashPop",
      title: "Pop Latest Stash",
      category: "Git",
      run: withRepo((root) => {
        const latest = useScm.getState().repos[root]?.stashes[0];
        return latest ? git.stashApply(root, latest.name, true) : toast.info("There are no stashes.");
      }),
    },
    { id: "git.undoLastCommit", title: "Undo Last Commit…", category: "Git", run: withRepo(git.undoLastCommit) },
    {
      id: "git.merge",
      title: "Merge Branch into Current…",
      category: "Git",
      run: withRepo(async (root) => {
        const rev = await pickRef(root, "Merge which branch into the current branch?");
        if (rev) await git.merge(root, rev);
      }),
    },
    {
      id: "git.rebase",
      title: "Rebase Current Branch onto…",
      category: "Git",
      run: withRepo(async (root) => {
        const onto = await pickRef(root, "Rebase the current branch onto…");
        if (onto) await git.rebaseOnto(root, onto);
      }),
    },
    { id: "git.continue", title: "Continue Merge/Rebase", category: "Git", run: withRepo((root) => git.operation(root, "continue")) },
    { id: "git.abort", title: "Abort Merge/Rebase…", category: "Git", run: withRepo((root) => git.operation(root, "abort")) },
    { id: "git.addRemote", title: "Add Remote…", category: "Git", run: withRepo(git.addRemote) },
    {
      id: "git.toggleBlame",
      title: "Toggle Blame",
      category: "Git",
      when: () => activeEditor() !== null,
      run: async () => {
        const editor = activeEditor();
        if (editor && !(await toggleBlame(editor.view).catch((e) => (git.showError(e, "Blame"), true)))) {
          toast.info("This file is not part of an open Git repository.");
        }
      },
    },
    {
      id: "git.openChanges",
      title: "Open Changes of Active File",
      category: "Git",
      when: () => activeEditor() !== null,
      run: () => {
        const editor = activeEditor();
        const context = editor && repoForPath(editor.path, Object.keys(useScm.getState().repos));
        if (!context) return toast.info("This file is not part of an open Git repository.");
        openDiff({ root: context.root, path: context.relative, origPath: null, source: { kind: "worktree" } }, {});
      },
    },
  ]);
}
