// Panel Source Control: kotak commit, banner operasi, daftar perubahan, stash, worktree, submodule.
import { useEffect, useMemo, useRef, useState } from "react";
import type { MouseEvent as ReactMouseEvent, ReactNode } from "react";
import { Archive, ArrowDown, ArrowUp, Check, ChevronDown, ChevronRight, CloudUpload, Ellipsis, FileText, Minus, Plus, RefreshCw, Undo2, X } from "lucide-react";
import { executeCommand } from "@/app/commands";
import { showMenu, type MenuItem } from "@/app/overlay";
import { setTrusted, useApp } from "@/app/store";
import { commands, type ChangeKind, type ConflictKind, type FileChange, type Operation, type RepoSnapshot } from "@/ipc";
import * as git from "./actions";
import { openConflict, openDiff, openRepoFile } from "./open";
import { headLabel, openRepo, setActiveRepo, useActiveRepo, useScm, type RepoState } from "./store";
import "./scm.css";

const KIND_LETTER: Record<ChangeKind, string> = { added: "A", modified: "M", deleted: "D", renamed: "R", copied: "C", typeChanged: "T" };
const KIND_CLASS: Record<ChangeKind, string> = {
  added: "oc-git-added",
  modified: "oc-git-modified",
  deleted: "oc-git-deleted",
  renamed: "oc-git-renamed",
  copied: "oc-git-renamed",
  typeChanged: "oc-git-modified",
};
const KIND_TITLE: Record<ChangeKind, string> = { added: "Added", modified: "Modified", deleted: "Deleted", renamed: "Renamed", copied: "Copied", typeChanged: "Type changed" };
const CONFLICT_TITLE: Record<ConflictKind, string> = {
  bothModified: "Both modified",
  bothAdded: "Both added",
  bothDeleted: "Both deleted",
  addedByUs: "Added by us",
  addedByThem: "Added by them",
  deletedByUs: "Deleted by us",
  deletedByThem: "Deleted by them",
};

function splitPath(path: string): { name: string; dir: string } {
  const i = path.lastIndexOf("/");
  return i < 0 ? { name: path, dir: "" } : { name: path.slice(i + 1), dir: path.slice(0, i) };
}

function Section({ title, count, actions, children, defaultOpen = true }: { title: string; count: number; actions?: ReactNode; children: ReactNode; defaultOpen?: boolean }) {
  const [open, setOpen] = useState(defaultOpen);
  return (
    <div>
      <div className="oc-section-header" role="button" tabIndex={0} aria-expanded={open} onClick={() => setOpen(!open)} onKeyDown={(e) => e.key === "Enter" && setOpen(!open)}>
        {open ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
        <span>{title}</span>
        <span className="oc-count">{count}</span>
        <div className="oc-row-actions" onClick={(e) => e.stopPropagation()}>
          {actions}
        </div>
      </div>
      {open && children}
    </div>
  );
}

function IconButton({ title, onClick, children }: { title: string; onClick: () => unknown; children: ReactNode }) {
  return (
    <button
      className="oc-iconbtn"
      title={title}
      aria-label={title}
      onClick={(e) => {
        e.stopPropagation();
        void onClick();
      }}
    >
      {children}
    </button>
  );
}

interface FileRowProps {
  root: string;
  path: string;
  origPath?: string | null;
  letter: string;
  letterClass: string;
  title: string;
  deleted?: boolean;
  onOpen: () => void;
  actions: ReactNode;
  menu: MenuItem[];
}

function FileRow({ root, path, origPath, letter, letterClass, title, deleted, onOpen, actions, menu }: FileRowProps) {
  const { name, dir } = splitPath(path);
  const onContextMenu = (e: ReactMouseEvent) => {
    e.preventDefault();
    showMenu(e, [
      ...menu,
      { separator: true },
      { label: "Open File", disabled: deleted, run: () => openRepoFile(root, path, { preview: false }) },
      { label: "File History", run: () => executeCommand("git.fileHistory", root, path) },
      { label: "Copy Path", run: () => navigator.clipboard.writeText(path) },
    ]);
  };
  return (
    <div className="oc-list-row scm-file" style={{ paddingLeft: 22 }} data-deleted={deleted || undefined} title={`${origPath ? `${origPath} → ` : ""}${path} • ${title}`} onClick={onOpen} onContextMenu={onContextMenu}>
      <span className="scm-file-name oc-ellipsis">
        {name}
        {dir && <span className="scm-file-dir">{dir}</span>}
      </span>
      <div className="oc-row-actions">{actions}</div>
      <span className={`oc-status-letter ${letterClass}`} style={{ marginLeft: "auto" }}>
        {letter}
      </span>
    </div>
  );
}

function operationText(op: Operation): { title: string; detail: string; canSkip: boolean } {
  switch (op.kind) {
    case "merge":
      return { title: "Merge in progress", detail: `Merging ${op.incoming}`, canSkip: false };
    case "rebase":
      return {
        title: op.interactive ? "Interactive rebase in progress" : "Rebase in progress",
        detail: `Step ${op.step} of ${op.total}${op.headName ? ` · ${op.headName}` : ""} onto ${op.onto}`,
        canSkip: true,
      };
    case "cherryPick":
      return { title: "Cherry-pick in progress", detail: `Applying ${op.commit}`, canSkip: true };
    case "revert":
      return { title: "Revert in progress", detail: `Reverting ${op.commit}`, canSkip: true };
  }
}

function OperationBanner({ root, snapshot }: { root: string; snapshot: RepoSnapshot }) {
  if (!snapshot.operation) return null;
  const text = operationText(snapshot.operation);
  const conflicts = snapshot.conflicted.length;
  return (
    <div className="scm-operation" role="status">
      <div className="scm-operation-title">{text.title}</div>
      <div>{text.detail}</div>
      <div className="oc-muted">{conflicts > 0 ? `${conflicts} conflicted file${conflicts === 1 ? "" : "s"} left to resolve.` : "No conflicts left. Continue when you are ready."}</div>
      <div className="scm-operation-actions">
        <button className="oc-btn oc-btn-sm oc-btn-primary" disabled={conflicts > 0} onClick={() => void git.operation(root, "continue")}>
          Continue
        </button>
        {text.canSkip && (
          <button className="oc-btn oc-btn-sm" onClick={() => void git.operation(root, "skip")}>
            Skip
          </button>
        )}
        <button className="oc-btn oc-btn-sm" onClick={() => void git.operation(root, "abort")}>
          Abort
        </button>
      </div>
    </div>
  );
}

function CommitBox({ repo }: { repo: RepoState }) {
  const root = repo.root;
  const snapshot = repo.snapshot!;
  const [message, setMessage] = useState("");
  const [amend, setAmend] = useState(false);
  const [signoff, setSignoff] = useState(false);
  const textarea = useRef<HTMLTextAreaElement>(null);
  const subjectLength = message.split("\n")[0].length;

  // Kotak commit dikosongkan saat berpindah repositori.
  useEffect(() => {
    setMessage("");
    setAmend(false);
  }, [root]);

  useEffect(() => {
    const focus = () => textarea.current?.focus();
    window.addEventListener("oncode:focus-commit", focus);
    return () => window.removeEventListener("oncode:focus-commit", focus);
  }, []);

  const toggleAmend = async (next: boolean) => {
    setAmend(next);
    if (next && !message.trim()) setMessage(await commands.gitLastMessage(root).catch(() => ""));
  };

  const submit = async (then?: "push" | "sync") => {
    if (await git.commit(root, { message, amend, signoff, then })) {
      setMessage("");
      setAmend(false);
    }
  };

  const busy = repo.busy !== null;
  const stagedCount = snapshot.staged.length;
  const label = amend ? "Amend Commit" : stagedCount > 0 ? `Commit ${stagedCount} file${stagedCount === 1 ? "" : "s"}` : "Commit";
  return (
    <div className="scm-commit">
      <textarea
        ref={textarea}
        className="oc-textarea"
        placeholder={`Message (Ctrl+Enter to commit on ${headLabel(snapshot)})`}
        aria-label="Commit message"
        value={message}
        spellCheck={false}
        rows={3}
        onChange={(e) => setMessage(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) {
            e.preventDefault();
            void submit();
          }
        }}
      />
      {subjectLength > 50 && (
        <div className="scm-hint" data-long={subjectLength > 72}>
          Subject is {subjectLength} characters (aim for 50, keep under 72)
        </div>
      )}
      <div className="scm-commit-row">
        <div className="scm-split-btn">
          <button className="oc-btn oc-btn-primary" disabled={busy || !!snapshot.operation} onClick={() => void submit()}>
            <Check size={15} />
            <span className="oc-ellipsis">{label}</span>
          </button>
          <button
            className="oc-btn oc-btn-primary"
            aria-label="More commit actions"
            disabled={busy || !!snapshot.operation}
            onClick={(e) => {
              const rect = e.currentTarget.getBoundingClientRect();
              showMenu({ x: rect.left, y: rect.bottom + 2 }, [
                { label: "Commit", run: () => submit() },
                { label: "Commit & Push", run: () => submit("push") },
                { label: "Commit & Sync", run: () => submit("sync") },
                { separator: true },
                { label: `${signoff ? "✓ " : ""}Add Signed-off-by`, run: () => setSignoff(!signoff) },
              ]);
            }}
          >
            <ChevronDown size={14} />
          </button>
        </div>
        <label className="oc-check" title="Replace the last commit with the staged changes and this message">
          <input type="checkbox" checked={amend} disabled={snapshot.head.kind === "unborn"} onChange={(e) => void toggleAmend(e.target.checked)} />
          Amend
        </label>
      </div>
    </div>
  );
}

function SyncRow({ repo }: { repo: RepoState }) {
  const snapshot = repo.snapshot!;
  const root = repo.root;
  if (snapshot.head.kind === "detached" || snapshot.head.kind === "unborn") return null;
  const upstream = snapshot.upstream;
  const hasRemote = (repo.refs?.remotes.length ?? 0) > 0;
  if (!upstream || !upstream.exists) {
    if (!hasRemote) return null;
    return (
      <div style={{ padding: "0 10px 8px" }}>
        <button className="oc-btn oc-btn-block" disabled={repo.busy !== null} onClick={() => void git.push(root, { setUpstream: true })}>
          <CloudUpload size={15} /> Publish Branch
        </button>
      </div>
    );
  }
  if (upstream.ahead === 0 && upstream.behind === 0) return null;
  return (
    <div style={{ padding: "0 10px 8px" }}>
      <button className="oc-btn oc-btn-block" disabled={repo.busy !== null} onClick={() => void git.sync(root)} title={`Pull ${upstream.behind} and push ${upstream.ahead} commit(s) with ${upstream.name}`}>
        <RefreshCw size={14} /> Sync Changes
        {upstream.behind > 0 && (
          <span className="oc-row" style={{ gap: 1 }}>
            {upstream.behind}
            <ArrowDown size={13} />
          </span>
        )}
        {upstream.ahead > 0 && (
          <span className="oc-row" style={{ gap: 1 }}>
            {upstream.ahead}
            <ArrowUp size={13} />
          </span>
        )}
      </button>
    </div>
  );
}

function Changes({ repo }: { repo: RepoState }) {
  const root = repo.root;
  const snapshot = repo.snapshot!;

  // Berkas untracked ditampilkan bersama perubahan lain, seperti di panel Source Control pada umumnya.
  const changes = useMemo(() => {
    const rows: { change: FileChange; untracked: boolean }[] = snapshot.unstaged.map((change) => ({ change, untracked: false }));
    for (const path of snapshot.untracked) rows.push({ change: { path, origPath: null, kind: "added", submodule: null }, untracked: true });
    return rows.sort((a, b) => a.change.path.localeCompare(b.change.path));
  }, [snapshot.unstaged, snapshot.untracked]);

  const discardAll = () =>
    git.discard(
      root,
      snapshot.unstaged.map((c) => c.path),
      snapshot.untracked,
    );

  return (
    <>
      {snapshot.conflicted.length > 0 && (
        <Section title="Conflicts" count={snapshot.conflicted.length}>
          {snapshot.conflicted.map((c) => (
            <FileRow
              key={c.path}
              root={root}
              path={c.path}
              letter="!"
              letterClass="oc-git-conflict"
              title={CONFLICT_TITLE[c.kind]}
              onOpen={() => openConflict(root, c.path)}
              actions={
                <IconButton title="Mark as resolved (stage current file content)" onClick={() => git.markResolved(root, c.path)}>
                  <Plus size={15} />
                </IconButton>
              }
              menu={[
                { label: "Open Conflict Editor", run: () => openConflict(root, c.path) },
                { label: "Accept Current (ours)", run: () => git.resolveWithSide(root, c.path, "current") },
                { label: "Accept Incoming (theirs)", run: () => git.resolveWithSide(root, c.path, "incoming") },
                { label: "Mark as Resolved", run: () => git.markResolved(root, c.path) },
              ]}
            />
          ))}
        </Section>
      )}
      {snapshot.staged.length > 0 && (
        <Section
          title="Staged Changes"
          count={snapshot.staged.length}
          actions={
            <IconButton title="Unstage all" onClick={() => git.unstageAll(root)}>
              <Minus size={15} />
            </IconButton>
          }
        >
          {snapshot.staged.map((c) => (
            <FileRow
              key={c.path}
              root={root}
              path={c.path}
              origPath={c.origPath}
              letter={KIND_LETTER[c.kind]}
              letterClass={KIND_CLASS[c.kind]}
              title={`${KIND_TITLE[c.kind]} (staged)`}
              deleted={c.kind === "deleted"}
              onOpen={() => openDiff({ root, path: c.path, origPath: c.origPath, source: { kind: "staged" } })}
              actions={
                <IconButton title="Unstage" onClick={() => git.unstage(root, c.origPath ? [c.path, c.origPath] : [c.path])}>
                  <Minus size={15} />
                </IconButton>
              }
              menu={[
                { label: "Open Changes", run: () => openDiff({ root, path: c.path, origPath: c.origPath, source: { kind: "staged" } }, {}) },
                { label: "Unstage", run: () => git.unstage(root, c.origPath ? [c.path, c.origPath] : [c.path]) },
              ]}
            />
          ))}
        </Section>
      )}
      <Section
        title="Changes"
        count={changes.length}
        actions={
          changes.length > 0 && (
            <>
              <IconButton title="Stash all changes…" onClick={() => git.stashPush(root)}>
                <Archive size={14} />
              </IconButton>
              <IconButton title="Discard all changes…" onClick={discardAll}>
                <Undo2 size={14} />
              </IconButton>
              <IconButton title="Stage all" onClick={() => git.stageAll(root)}>
                <Plus size={15} />
              </IconButton>
            </>
          )
        }
      >
        {changes.length === 0 && snapshot.staged.length === 0 && snapshot.conflicted.length === 0 && <div className="scm-empty">No changes. The working tree is clean.</div>}
        {changes.map(({ change: c, untracked }) => (
          <FileRow
            key={c.path}
            root={root}
            path={c.path}
            origPath={c.origPath}
            letter={untracked ? "U" : KIND_LETTER[c.kind]}
            letterClass={untracked ? "oc-git-untracked" : KIND_CLASS[c.kind]}
            title={untracked ? "Untracked" : KIND_TITLE[c.kind]}
            deleted={c.kind === "deleted"}
            onOpen={() => openDiff({ root, path: c.path, origPath: c.origPath, source: { kind: "worktree" } })}
            actions={
              <>
                {c.kind !== "deleted" && (
                  <IconButton title="Open file" onClick={() => openRepoFile(root, c.path, { preview: false })}>
                    <FileText size={14} />
                  </IconButton>
                )}
                <IconButton title={untracked ? "Delete (move to trash)…" : "Discard changes…"} onClick={() => git.discard(root, untracked ? [] : [c.path], untracked ? [c.path] : [])}>
                  <Undo2 size={14} />
                </IconButton>
                <IconButton title="Stage" onClick={() => git.stage(root, [c.path])}>
                  <Plus size={15} />
                </IconButton>
              </>
            }
            menu={[
              { label: "Open Changes", run: () => openDiff({ root, path: c.path, origPath: c.origPath, source: { kind: "worktree" } }, {}) },
              { label: "Stage", run: () => git.stage(root, [c.path]) },
              { label: untracked ? "Delete (Move to Trash)…" : "Discard Changes…", danger: true, run: () => git.discard(root, untracked ? [] : [c.path], untracked ? [c.path] : []) },
              { label: "Stash This File…", disabled: untracked, run: () => git.stashPush(root, { paths: [c.path], includeUntracked: false }) },
            ]}
          />
        ))}
      </Section>
    </>
  );
}

function relativeTime(seconds: number): string {
  const diff = Date.now() / 1000 - seconds;
  if (diff < 60) return "just now";
  if (diff < 3600) return `${Math.floor(diff / 60)}m ago`;
  if (diff < 86400) return `${Math.floor(diff / 3600)}h ago`;
  if (diff < 86400 * 30) return `${Math.floor(diff / 86400)}d ago`;
  return new Date(seconds * 1000).toLocaleDateString();
}

function Extras({ repo }: { repo: RepoState }) {
  const root = repo.root;
  const otherWorktrees = repo.worktrees.filter((w) => !w.isCurrent);
  return (
    <>
      {repo.stashes.length > 0 && (
        <Section title="Stashes" count={repo.stashes.length} defaultOpen={false}>
          {repo.stashes.map((s) => (
            <div
              key={s.oid}
              className="oc-list-row"
              style={{ paddingLeft: 22 }}
              title={`${s.name}: ${s.message}`}
              onClick={() => void executeCommand("git.showStash", root, s.name)}
              onContextMenu={(e) => {
                e.preventDefault();
                showMenu(e, [
                  { label: "Show Changes", run: () => executeCommand("git.showStash", root, s.name) },
                  { label: "Apply", run: () => git.stashApply(root, s.name, false) },
                  { label: "Pop (apply and drop)", run: () => git.stashApply(root, s.name, true) },
                  { label: "Create Branch from Stash…", run: () => git.stashBranch(root, s.name) },
                  { separator: true },
                  { label: "Drop…", danger: true, run: () => git.stashDrop(root, s.name, s.message) },
                ]);
              }}
            >
              <Archive size={13} className="oc-faint" />
              <span className="oc-ellipsis">{s.message}</span>
              <span className="oc-faint" style={{ marginLeft: "auto", flex: "none", fontSize: 11 }}>
                {relativeTime(s.time)}
              </span>
              <div className="oc-row-actions" style={{ marginLeft: 0 }}>
                <IconButton title="Pop (apply and drop)" onClick={() => git.stashApply(root, s.name, true)}>
                  <ArrowUp size={14} />
                </IconButton>
                <IconButton title="Drop…" onClick={() => git.stashDrop(root, s.name, s.message)}>
                  <X size={14} />
                </IconButton>
              </div>
            </div>
          ))}
        </Section>
      )}
      {otherWorktrees.length > 0 && (
        <Section title="Worktrees" count={otherWorktrees.length} defaultOpen={false}>
          {otherWorktrees.map((w) => (
            <div
              key={w.path}
              className="oc-list-row"
              style={{ paddingLeft: 22 }}
              title={w.path}
              onClick={() => void commands.windowOpen(w.path)}
              onContextMenu={(e) => {
                e.preventDefault();
                showMenu(e, [
                  { label: "Open in New Window", run: () => commands.windowOpen(w.path) },
                  { label: "Copy Path", run: () => navigator.clipboard.writeText(w.path) },
                  { separator: true },
                  { label: "Remove Worktree…", danger: true, disabled: w.isMain, run: () => executeCommand("git.removeWorktree", root, w.path) },
                ]);
              }}
            >
              <span className="oc-ellipsis">{w.branch ?? (w.head ? w.head.slice(0, 8) : "(bare)")}</span>
              <span className="scm-file-dir oc-ellipsis">{w.path}</span>
              {w.prunable && <span className="oc-git-deleted" style={{ marginLeft: "auto", fontSize: 11 }}>missing</span>}
            </div>
          ))}
        </Section>
      )}
      {repo.submodules.length > 0 && (
        <Section title="Submodules" count={repo.submodules.length} defaultOpen={false}>
          {repo.submodules.map((s) => (
            <div
              key={s.path}
              className="oc-list-row"
              style={{ paddingLeft: 22 }}
              title={`${s.path} @ ${s.oid.slice(0, 8)} ${s.describe}`}
              onClick={() => s.status !== "uninitialized" && void openRepo(`${root}/${s.path}`).catch(git.showError)}
              onContextMenu={(e) => {
                e.preventDefault();
                showMenu(e, [
                  { label: "Open as Repository", disabled: s.status === "uninitialized", run: () => openRepo(`${root}/${s.path}`) },
                  { label: s.status === "uninitialized" ? "Initialize and Update" : "Update to Recorded Commit", run: () => executeCommand("git.updateSubmodules", root, [s.path]) },
                ]);
              }}
            >
              <span className="oc-ellipsis">{s.path}</span>
              <span className={`oc-faint ${s.status === "modified" ? "oc-git-modified" : s.status === "conflict" ? "oc-git-conflict" : ""}`} style={{ marginLeft: "auto", flex: "none", fontSize: 11 }}>
                {s.status === "uninitialized" ? "not initialized" : s.status === "modified" ? "different commit" : s.status === "conflict" ? "conflict" : s.oid.slice(0, 8)}
              </span>
            </div>
          ))}
        </Section>
      )}
    </>
  );
}

function headerMenu(root: string): MenuItem[] {
  return [
    { label: "Pull", run: () => git.pull(root) },
    { label: "Push", run: () => git.push(root) },
    { label: "Fetch", run: () => git.fetch(root) },
    { label: "Sync (Pull then Push)", run: () => git.sync(root) },
    { separator: true },
    { label: "Switch Branch…", run: () => git.pickBranch(root) },
    { label: "Create Branch…", run: () => git.createBranch(root) },
    { label: "Create Tag…", run: () => git.createTag(root) },
    { separator: true },
    { label: "Stash Changes…", run: () => git.stashPush(root) },
    { label: "Undo Last Commit…", run: () => git.undoLastCommit(root) },
    { separator: true },
    { label: "Show Graph", run: () => executeCommand("git.showGraph") },
    { label: "Show Reflog", run: () => executeCommand("git.showReflog") },
    { label: "Add Worktree…", run: () => executeCommand("git.addWorktree") },
    { label: "Add Remote…", run: () => git.addRemote(root) },
    { separator: true },
    { label: "Force Push…", danger: true, run: () => git.push(root, { forceWithLease: true }) },
  ];
}

export function ScmView() {
  const workspace = useApp((s) => s.workspace);
  const info = useApp((s) => s.info);
  const repo = useActiveRepo();
  // Selector harus mengembalikan referensi yang stabil; daftar akar diturunkan lewat useMemo.
  const repos = useScm((s) => s.repos);
  const repoRoots = useMemo(() => Object.keys(repos), [repos]);

  if (!workspace) return null;
  if (!repo || !repo.snapshot) {
    return (
      <>
        <div className="oc-sidebar-title">Source Control</div>
        <div className="oc-sidebar-body">
          <div className="scm-empty">
            {!workspace.trusted ? (
              <>
                Git is disabled in restricted mode.
                <br />
                <button className="oc-btn oc-btn-primary" onClick={() => void setTrusted(true)}>Trust Folder</button>
              </>
            ) : !info?.git ? (
              <span className="oc-selectable">Git is unavailable: {info?.gitError ?? "git was not found"}.</span>
            ) : repo ? (
              "Loading repository…"
            ) : (
              <>
                This folder is not a Git repository.
                <br />
                <button className="oc-btn oc-btn-primary" onClick={() => void executeCommand("git.init")}>Initialize Repository</button>
              </>
            )}
          </div>
        </div>
      </>
    );
  }

  const root = repo.root;
  return (
    <>
      <div className="oc-sidebar-title">
        <span className="oc-ellipsis">Source Control</span>
        <div className="oc-grow" />
        <IconButton title="Fetch" onClick={() => git.fetch(root)}>
          <RefreshCw size={14} className={repo.busy ? "oc-spin" : undefined} />
        </IconButton>
        <button className="oc-iconbtn" title="More actions" aria-label="More actions" onClick={(e) => {
          const rect = e.currentTarget.getBoundingClientRect();
          showMenu({ x: rect.left, y: rect.bottom + 2 }, headerMenu(root));
        }}>
          <Ellipsis size={15} />
        </button>
      </div>
      <div className="oc-progress" data-indeterminate={repo.busy && repo.busy.percent === null ? true : undefined}>
        <div style={{ width: repo.busy?.percent != null ? `${repo.busy.percent}%` : repo.busy ? undefined : 0 }} />
      </div>
      <div className="oc-sidebar-body">
        {repoRoots.length > 1 && (
          <select className="oc-select scm-repo-picker" aria-label="Repository" value={root} onChange={(e) => setActiveRepo(e.target.value)}>
            {repoRoots.map((r) => (
              <option key={r} value={r}>
                {r === workspace.repoRoot ? splitPath(r.replace(/\\/g, "/")).name : r.replace(workspace.root, "").replace(/^[\\/]/, "")}
              </option>
            ))}
          </select>
        )}
        <CommitBox repo={repo} />
        <div className="scm-busy" aria-live="polite">
          {repo.busy && (
            <>
              <span className="oc-ellipsis">{repo.busy.label}{repo.busy.percent != null ? ` ${repo.busy.percent}%` : "…"}</span>
              {repo.busy.operationId && (
                <button className="oc-btn oc-btn-sm" style={{ marginLeft: "auto", height: 18 }} onClick={() => git.cancelBusy(root)}>
                  Cancel
                </button>
              )}
            </>
          )}
        </div>
        <OperationBanner root={root} snapshot={repo.snapshot} />
        <SyncRow repo={repo} />
        <Changes repo={repo} />
        <Extras repo={repo} />
      </div>
    </>
  );
}
