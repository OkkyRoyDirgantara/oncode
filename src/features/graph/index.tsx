// Fitur Graph: graph commit, detail commit, interactive rebase, reflog, riwayat berkas, stash, worktree.
import { Archive, GitGraph, History } from "lucide-react";
import { registerCommands } from "@/app/commands";
import { toast } from "@/app/overlay";
import { registerSidebarView, registerTabType } from "@/app/registry";
import { activeEditor } from "@/features/editor/CodeEditor";
import * as git from "@/features/scm/actions";
import { useActiveRepo, useScm } from "@/features/scm/store";
import { GraphTab } from "./GraphTab";
import { FileHistoryTab, ReflogTab, StashTab } from "./ListTabs";
import { openFileHistory, openGraph, openReflog, openStash } from "./open";
import { repoRelativePath } from "./paths";
import { interactiveRebase } from "./RebaseDialog";
import { RefsPane } from "./RefsPane";
import { addWorktree, removeWorktree, updateSubmodules } from "./worktree";
import "./graph.css";

/** Tampilan sidebar: daftar ref ringkas; memilih ref membuka tab Graph pada commit itu. */
function GraphSidebar() {
  const repo = useActiveRepo();
  return (
    <>
      <div className="oc-sidebar-title">Graph</div>
      {repo ? (
        <div className="gr-sidebar">
          <div className="gr-sidebar-open">
            <button className="oc-btn oc-btn-primary oc-btn-block" onClick={() => openGraph(repo.root)}>
              <GitGraph size={15} /> Open Graph
            </button>
          </div>
          <RefsPane root={repo.root} onReveal={(target) => openGraph(repo.root, target)} />
        </div>
      ) : (
        <div className="oc-sidebar-body">
          <div className="gr-hint">Open a Git repository to see its commit graph.</div>
        </div>
      )}
    </>
  );
}

/** Repositori yang memuat `absolute` (yang akarnya paling dalam), beserta path relatifnya. */
function locateInRepo(absolute: string): { root: string; path: string } | null {
  let best: { root: string; path: string } | null = null;
  for (const root of Object.keys(useScm.getState().repos)) {
    const path = repoRelativePath(root, absolute);
    if (path !== null && (!best || root.length > best.root.length)) best = { root, path };
  }
  return best;
}

export function registerGraphFeature(): void {
  registerTabType({ kind: "graph", component: GraphTab, icon: () => <GitGraph size={14} /> });
  registerTabType({ kind: "reflog", component: ReflogTab, icon: () => <History size={14} /> });
  registerTabType({ kind: "fileHistory", component: FileHistoryTab, icon: () => <History size={14} /> });
  registerTabType({ kind: "stash", component: StashTab, icon: () => <Archive size={14} /> });

  registerSidebarView({ id: "graph", title: "Graph", order: 40, icon: <GitGraph size={20} />, component: GraphSidebar });

  registerCommands([
    {
      id: "git.showGraph",
      title: "Show Graph",
      category: "Git",
      keybinding: "Mod+Shift+H",
      run: () => {
        const root = git.requireRepo();
        if (root) openGraph(root);
      },
    },
    {
      id: "git.showReflog",
      title: "Show Reflog",
      category: "Git",
      run: (root?: unknown) => {
        const target = typeof root === "string" ? root : git.requireRepo();
        if (target) openReflog(target);
      },
    },
    {
      id: "git.interactiveRebase",
      title: "Interactive Rebase…",
      category: "Git",
      // Argumen: (root, baseOid | null). Tanpa argumen (dari command palette): commit yang belum ada di upstream.
      run: async (root?: unknown, base?: unknown) => {
        const target = typeof root === "string" ? root : git.requireRepo();
        if (!target) return;
        if (typeof base === "string" || base === null) return interactiveRebase(target, base);
        const upstream = useScm.getState().repos[target]?.snapshot?.upstream;
        if (upstream?.exists) return interactiveRebase(target, upstream.name);
        openGraph(target);
        toast.info("This branch has no upstream. Right-click a commit in the Graph and choose “Interactive Rebase from Here…”.");
      },
    },
    {
      id: "git.fileHistory",
      title: "Show File History (path)",
      category: "Git",
      hidden: true,
      run: (root: string, path: string) => openFileHistory(root, path),
    },
    {
      id: "git.fileHistoryActive",
      title: "Show File History",
      category: "Git",
      run: () => {
        const editor = activeEditor();
        if (!editor) {
          toast.info("Open a file first to see its history.");
          return;
        }
        const located = locateInRepo(editor.path);
        if (!located) {
          toast.info("This file is not inside an open Git repository.");
          return;
        }
        openFileHistory(located.root, located.path);
      },
    },
    {
      id: "git.showStash",
      title: "Show Stash",
      category: "Git",
      hidden: true,
      run: (root: string, name: string) => openStash(root, name),
    },
    { id: "git.addWorktree", title: "Add Worktree…", category: "Git", run: () => addWorktree() },
    {
      id: "git.removeWorktree",
      title: "Remove Worktree",
      category: "Git",
      hidden: true,
      run: (root: string, path: string) => removeWorktree(root, path),
    },
    {
      id: "git.updateSubmodules",
      title: "Update Submodules",
      category: "Git",
      // Argumen: (root, paths). Tanpa argumen: semua submodule repositori aktif.
      run: (root?: unknown, paths?: unknown) => {
        const target = typeof root === "string" ? root : git.requireRepo();
        if (target) return updateSubmodules(target, Array.isArray(paths) ? (paths as string[]) : []);
      },
    },
  ]);
}
