// Tab berbentuk daftar: reflog, riwayat berkas, dan isi stash.
import { useEffect, useRef, useState } from "react";
import type { MouseEvent as ReactMouseEvent, ReactNode } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { GitBranch, RefreshCw, Undo2 } from "lucide-react";
import { showMenu } from "@/app/overlay";
import type { TabProps } from "@/app/registry";
import { commands, type Commit, type ReflogEntry } from "@/ipc";
import * as git from "@/features/scm/actions";
import { openDiff } from "@/features/scm/open";
import { onRefsChanged, useRepo } from "@/features/scm/store";
import { ErrorBox } from "./CommitDetails";
import { FileList } from "./FileList";
import { fullDate, plural, relativeDate, shortOid } from "./format";
import { ROW_HEIGHT } from "./geometry";
import { copyText, reflogMenu } from "./menus";
import { openGraph } from "./open";
import { useAsync } from "./useAsync";
import "./graph.css";

/** Daftar tervirtualisasi dengan tinggi baris tetap. */
function VirtualRows<T>({ items, label, render }: { items: readonly T[]; label: string; render: (item: T, index: number) => ReactNode }) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const virtualizer = useVirtualizer({ count: items.length, getScrollElement: () => scrollRef.current, estimateSize: () => ROW_HEIGHT, overscan: 12 });
  return (
    <div className="gr-list" ref={scrollRef} role="list" aria-label={label}>
      <div className="gr-list-inner" style={{ height: virtualizer.getTotalSize() }}>
        {virtualizer.getVirtualItems().map((v) => (
          <div key={v.key} className="gr-vrow" role="listitem" style={{ transform: `translateY(${v.start}px)` }}>
            {render(items[v.index], v.index)}
          </div>
        ))}
      </div>
    </div>
  );
}

function useReloadOnRefs(root: string, reload: () => void) {
  useEffect(() => onRefsChanged((changed) => changed === root && reload()), [root, reload]);
}

// ----- Reflog -----

export const REFLOG_LIMIT = 500;

export function ReflogTab({ tab }: TabProps) {
  const root = tab.params.root as string;
  const entries = useAsync(() => commands.gitReflog(root, REFLOG_LIMIT), [root]);
  useReloadOnRefs(root, entries.reload);
  const data = entries.data ?? [];

  const row = (entry: ReflogEntry) => {
    const onContextMenu = (e: ReactMouseEvent) => {
      e.preventDefault();
      showMenu(e, [{ label: "Show in Graph", run: () => openGraph(root, { oid: entry.oid, label: entry.selector, hunt: true }) }, { separator: true }, ...reflogMenu(root, entry.oid)]);
    };
    return (
      <div className="gr-row" title={`${entry.selector}: ${entry.message}`} onContextMenu={onContextMenu}>
        <span className="gr-reflog-selector oc-mono">{entry.selector}</span>
        <span className="gr-hash oc-mono">{shortOid(entry.oid)}</span>
        <span className="gr-subject oc-ellipsis oc-grow">{entry.message}</span>
        <span className="gr-row-actions">
          <button className="oc-iconbtn" title="Create Branch Here…" aria-label={`Create branch at ${entry.selector}`} onClick={() => void git.createBranch(root, entry.oid)}>
            <GitBranch size={14} />
          </button>
          <button className="oc-iconbtn" title="Reset Current Branch to Here…" aria-label={`Reset current branch to ${entry.selector}`} onClick={() => void git.reset(root, entry.oid, "hard")}>
            <Undo2 size={14} />
          </button>
        </span>
        <span className="gr-date" title={fullDate(entry.time)}>
          {relativeDate(entry.time)}
        </span>
      </div>
    );
  };

  return (
    <div className="gr-tab gr-column">
      <div className="gr-toolbar">
        <span className="gr-intro oc-ellipsis" title="The reflog records every position HEAD has been at, including commits no branch points to any more.">
          Every move of HEAD is recorded here. After a bad reset or rebase, find the entry from before it and create a branch or reset to it.
        </span>
        <span className="oc-grow" />
        {entries.data && <span className="gr-status">{data.length} {data.length === 1 ? "entry" : "entries"}</span>}
        <button className="oc-iconbtn" title="Refresh" aria-label="Refresh" onClick={entries.reload}>
          <RefreshCw size={14} className={entries.status === "loading" ? "oc-spin" : undefined} />
        </button>
      </div>
      {entries.status === "error" && <ErrorBox message={`Could not load the reflog: ${entries.error}`} retry={entries.reload} />}
      {entries.status === "loading" && !entries.data && <div className="gr-hint">Loading reflog…</div>}
      {entries.data && data.length === 0 && <div className="gr-hint">The reflog is empty.</div>}
      {data.length > 0 && <VirtualRows items={data} label="Reflog" render={row} />}
    </div>
  );
}

// ----- Riwayat berkas -----

export const HISTORY_LIMIT = 500;

export function FileHistoryTab({ tab }: TabProps) {
  const root = tab.params.root as string;
  const path = tab.params.path as string;
  const history = useAsync(() => commands.gitLog(root, { path }, HISTORY_LIMIT), [root, path]);
  useReloadOnRefs(root, history.reload);
  const [selected, setSelected] = useState<string | null>(null);
  const data = history.data ?? [];

  const open = (commit: Commit, preview: boolean) => {
    setSelected(commit.oid);
    openDiff({ root, path, origPath: null, source: { kind: "commit", oid: commit.oid, parent: null } }, { preview });
  };

  const row = (commit: Commit) => {
    const onContextMenu = (e: ReactMouseEvent) => {
      e.preventDefault();
      showMenu(e, [
        { label: "Open Changes", run: () => open(commit, false) },
        { label: "Show in Graph", run: () => openGraph(root, { oid: commit.oid, label: shortOid(commit.oid), hunt: true }) },
        { separator: true },
        { label: "Copy Hash", run: () => copyText(commit.oid, "Hash") },
      ]);
    };
    return (
      <div
        className="gr-row"
        role="button"
        tabIndex={0}
        aria-pressed={selected === commit.oid}
        data-selected={selected === commit.oid || undefined}
        title={commit.subject}
        onClick={() => open(commit, true)}
        onDoubleClick={() => open(commit, false)}
        onKeyDown={(e) => e.key === "Enter" && open(commit, false)}
        onContextMenu={onContextMenu}
      >
        <span className="gr-subject oc-ellipsis oc-grow">{commit.subject}</span>
        <span className="gr-author oc-ellipsis" title={`${commit.author.name} <${commit.author.email}>`}>
          {commit.author.name}
        </span>
        <span className="gr-date" title={fullDate(commit.author.time)}>
          {relativeDate(commit.author.time)}
        </span>
        <span className="gr-hash oc-mono">{shortOid(commit.oid)}</span>
      </div>
    );
  };

  return (
    <div className="gr-tab gr-column">
      <div className="gr-toolbar">
        <span className="gr-intro oc-ellipsis oc-selectable" title={path}>
          History of <span className="oc-mono">{path}</span>
        </span>
        <span className="oc-grow" />
        {history.data && (
          <span className="gr-status">
            {plural(data.length, "commit")}
            {data.length >= HISTORY_LIMIT ? "+" : ""}
          </span>
        )}
        <button className="oc-iconbtn" title="Refresh" aria-label="Refresh" onClick={history.reload}>
          <RefreshCw size={14} className={history.status === "loading" ? "oc-spin" : undefined} />
        </button>
      </div>
      {history.status === "error" && <ErrorBox message={`Could not load the history of ${path}: ${history.error}`} retry={history.reload} />}
      {history.status === "loading" && !history.data && <div className="gr-hint">Loading history…</div>}
      {history.data && data.length === 0 && <div className="gr-hint">No commits touch this file yet.</div>}
      {data.length > 0 && <VirtualRows items={data} label={`History of ${path}`} render={row} />}
    </div>
  );
}

// ----- Stash -----

export function StashTab({ tab }: TabProps) {
  const root = tab.params.root as string;
  const initialName = tab.params.name as string;
  const repo = useRepo(root);
  // Nama stash (`stash@{n}`) bergeser saat stash lain dibuat atau dihapus; hash-nya tetap.
  const oid = useRef<string | null>(null);
  const stashes = repo?.stashes ?? [];
  if (oid.current === null) oid.current = stashes.find((s) => s.name === initialName)?.oid ?? null;
  const stash = oid.current ? stashes.find((s) => s.oid === oid.current) : undefined;
  const name = stash?.name ?? initialName;
  const files = useAsync(() => commands.gitStashFiles(root, name), [root, name, oid.current]);
  const gone = oid.current !== null ? !stash : files.status === "error";

  return (
    <div className="gr-tab gr-column">
      <div className="gr-toolbar">
        <span className="gr-intro oc-ellipsis" title={stash?.message}>
          <span className="oc-mono">{name}</span>
          {stash ? ` — ${stash.message}` : ""}
        </span>
        <span className="oc-grow" />
        {stash && (
          <span className="gr-status" title={fullDate(stash.time)}>
            {relativeDate(stash.time)}
          </span>
        )}
        <button className="oc-btn oc-btn-sm" disabled={gone} title="Apply the changes and keep the stash" onClick={() => void git.stashApply(root, name, false)}>
          Apply
        </button>
        <button className="oc-btn oc-btn-sm" disabled={gone} title="Apply the changes and drop the stash" onClick={() => void git.stashApply(root, name, true)}>
          Pop
        </button>
        <button className="oc-btn oc-btn-sm" disabled={gone} onClick={() => void git.stashDrop(root, name, stash?.message ?? name)}>
          Drop…
        </button>
      </div>
      {gone && oid.current !== null ? (
        <div className="gr-hint">This stash no longer exists (it was popped or dropped).</div>
      ) : (
        <>
          {files.status === "error" && <ErrorBox message={`Could not load ${name}: ${files.error}`} retry={files.reload} />}
          {files.status === "loading" && !files.data && <div className="gr-hint">Loading stash…</div>}
          {files.data && (
            <div className="gr-scroll">
              <div className="gr-files-header">{plural(files.data.length, "changed file")}</div>
              <FileList root={root} files={files.data} source={{ kind: "stash", name }} stats={false} />
            </div>
          )}
        </>
      )}
    </div>
  );
}
