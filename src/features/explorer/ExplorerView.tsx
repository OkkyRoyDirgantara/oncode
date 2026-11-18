// Tampilan sidebar Explorer: pohon berkas yang dimuat per folder dan divirtualisasi.
import { useEffect, useMemo, useRef } from "react";
import type { KeyboardEvent as ReactKeyboardEvent, MouseEvent as ReactMouseEvent } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { ChevronDown, ChevronRight, ChevronsDownUp, File, FilePlus, Folder, FolderOpen, FolderPlus, RefreshCw } from "lucide-react";
import { openFile } from "@/app/coreCommands";
import { showMenu, type MenuItem } from "@/app/overlay";
import { relativePath, useApp } from "@/app/store";
import { useWorkbench } from "@/app/workbench";
import type { DirEntry } from "@/ipc";
import { useActiveRepo } from "../scm/store";
import { copyPath, createEntry, deleteEntry, renameEntry } from "./actions";
import { collapseAll, collapseDir, expandDir, loadDir, refreshAll, revealPath, selectPath, setRoot, toggleDir, useExplorer } from "./store";
import { INDENT, ROW_HEIGHT, buildDecorations, decorationFor, flattenTree, type GitDecorations, type Row } from "./tree";
import "./explorer.css";

/** Event jendela untuk memindahkan fokus keyboard ke pohon (dipakai command `explorer.focus`). */
export const FOCUS_EVENT = "oncode:focus-explorer";

type EntryRow = Extract<Row, { kind: "entry" }>;

function entryMenu(entry: DirEntry): MenuItem[] {
  const dir = entry.isDir ? entry.path : undefined;
  return [
    ...(entry.isDir
      ? []
      : [
          { label: "Open", run: () => openFile(entry.path) },
          { label: "Open to the Side", run: () => openFile(entry.path, { toSide: true }) },
          { separator: true },
        ]),
    { label: "New File…", run: () => createEntry("file", dir) },
    { label: "New Folder…", run: () => createEntry("folder", dir) },
    { separator: true },
    { label: "Rename…", hint: "F2", run: () => renameEntry(entry) },
    { label: "Delete (Move to Trash)…", hint: "Del", danger: true, run: () => deleteEntry(entry) },
    { separator: true },
    { label: "Copy Path", run: () => copyPath(entry, false) },
    { label: "Copy Relative Path", run: () => copyPath(entry, true) },
  ];
}

function TreeRow({ row, top, selected, active, decorations }: { row: EntryRow; top: number; selected: boolean; active: boolean; decorations: GitDecorations }) {
  const { entry, depth, expanded } = row;
  const decoration = decorationFor(decorations, entry);
  const tint = entry.ignored ? "oc-git-ignored" : (decoration?.className ?? "");
  const relative = relativePath(entry.path);
  const title = [relative, entry.isSymlink ? "symbolic link" : "", entry.ignored ? "ignored" : "", decoration?.title ?? ""].filter(Boolean).join(" • ");

  const onClick = (e: ReactMouseEvent) => {
    selectPath(entry.path);
    if (!entry.isDir) openFile(entry.path, { preview: true });
    // Klik kedua dari klik ganda tidak menutup lagi folder yang baru dibuka.
    else if (e.detail <= 1) toggleDir(entry.path);
  };
  const onContextMenu = (e: ReactMouseEvent) => {
    e.preventDefault();
    e.stopPropagation();
    selectPath(entry.path);
    showMenu(e, entryMenu(entry));
  };

  return (
    <div
      role="treeitem"
      className="oc-list-row ex-row"
      aria-level={depth + 1}
      aria-selected={selected}
      aria-expanded={entry.isDir ? expanded : undefined}
      data-active={active || undefined}
      data-path={entry.path}
      title={title}
      style={{ transform: `translateY(${top}px)`, paddingLeft: 6 + depth * INDENT }}
      onClick={onClick}
      onDoubleClick={() => !entry.isDir && openFile(entry.path)}
      onContextMenu={onContextMenu}
    >
      <span className="ex-twist">{entry.isDir ? expanded ? <ChevronDown size={14} /> : <ChevronRight size={14} /> : null}</span>
      <span className={`ex-icon ${tint}`}>{entry.isDir ? expanded ? <FolderOpen size={15} /> : <Folder size={15} /> : <File size={15} />}</span>
      <span className={`ex-name oc-ellipsis ${tint}`}>{entry.name}</span>
      {decoration &&
        (decoration.letter ? (
          <span className={`oc-status-letter ex-status ${decoration.className}`}>{decoration.letter}</span>
        ) : (
          <span className={`ex-dot ${decoration.className}`} aria-hidden="true" />
        ))}
    </div>
  );
}

function NoteRow({ row, top }: { row: Extract<Row, { kind: "note" }>; top: number }) {
  return (
    <div
      className={`ex-note oc-ellipsis ${row.tone === "error" ? "oc-error-text" : ""}`}
      role={row.tone === "error" ? "alert" : undefined}
      title={row.text}
      style={{ transform: `translateY(${top}px)`, paddingLeft: 6 + row.depth * INDENT + 20 }}
    >
      {row.text}
    </div>
  );
}

export function ExplorerView() {
  const workspace = useApp((s) => s.workspace);
  const shown = useApp((s) => s.sidebarVisible && s.sidebarView === "explorer");
  const root = useExplorer((s) => s.root);
  const dirs = useExplorer((s) => s.dirs);
  const expanded = useExplorer((s) => s.expanded);
  const selected = useExplorer((s) => s.selected);
  const revealSeq = useExplorer((s) => s.revealSeq);
  const snapshot = useActiveRepo()?.snapshot ?? null;
  const activePath = useWorkbench((s) => {
    const group = s.groups.find((g) => g.id === s.activeGroupId);
    const tab = group?.tabs.find((t) => t.id === group.activeId);
    return tab?.kind === "file" && typeof tab.params.path === "string" ? tab.params.path : null;
  });

  const scroller = useRef<HTMLDivElement>(null);
  const rows = useMemo(() => (root ? flattenTree(root, dirs, expanded) : []), [root, dirs, expanded]);
  const decorations = useMemo(() => buildDecorations(snapshot), [snapshot]);
  const virtualizer = useVirtualizer({ count: rows.length, getScrollElement: () => scroller.current, estimateSize: () => ROW_HEIGHT, overscan: 12 });

  const workspaceRoot = workspace?.root ?? null;
  useEffect(() => setRoot(workspaceRoot), [workspaceRoot]);

  // Berkas di tab aktif disorot: leluhurnya dibuka dan barisnya digulir ke tampilan.
  useEffect(() => {
    if (activePath && root) void revealPath(activePath);
  }, [activePath, root]);

  useEffect(() => {
    if (!shown || !selected) return;
    const index = rows.findIndex((r) => r.kind === "entry" && r.entry.path === selected);
    if (index >= 0) virtualizer.scrollToIndex(index, { align: "auto" });
    // Hanya saat pilihan berubah atau sebuah path diminta tampil; pemuatan ulang pohon tidak menggeser gulir.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selected, revealSeq, shown]);

  useEffect(() => {
    const focus = () => scroller.current?.focus();
    window.addEventListener(FOCUS_EVENT, focus);
    return () => window.removeEventListener(FOCUS_EVENT, focus);
  }, []);

  if (!workspace || !root) return null;

  const entryRows = rows.filter((r): r is EntryRow => r.kind === "entry");
  const current = entryRows.find((r) => r.entry.path === selected) ?? null;
  const refocus = () => scroller.current?.focus();

  const onKeyDown = (e: ReactKeyboardEvent) => {
    if (e.ctrlKey || e.metaKey || e.altKey || e.target !== e.currentTarget) return;
    const index = current ? entryRows.indexOf(current) : -1;
    const move = (to: number) => {
      const next = entryRows[Math.max(0, Math.min(to, entryRows.length - 1))];
      if (next) selectPath(next.entry.path);
    };
    switch (e.key) {
      case "ArrowDown":
        move(index + 1);
        break;
      case "ArrowUp":
        move(index < 0 ? 0 : index - 1);
        break;
      case "Home":
        move(0);
        break;
      case "End":
        move(entryRows.length - 1);
        break;
      case "ArrowRight":
        if (!current?.entry.isDir) break;
        if (!current.expanded) void expandDir(current.entry.path);
        else if (entryRows[index + 1]?.parent === current.entry.path) move(index + 1);
        break;
      case "ArrowLeft":
        if (!current) break;
        if (current.entry.isDir && current.expanded) collapseDir(current.entry.path);
        else if (current.parent !== root) selectPath(current.parent);
        break;
      case "Enter":
        if (!current) break;
        if (current.entry.isDir) toggleDir(current.entry.path);
        else openFile(current.entry.path, { preview: true });
        break;
      case "F2":
        if (current) void renameEntry(current.entry).finally(refocus);
        break;
      case "Delete":
        if (current) void deleteEntry(current.entry).finally(refocus);
        break;
      default:
        return;
    }
    e.preventDefault();
  };

  const onBackgroundMenu = (e: ReactMouseEvent) => {
    e.preventDefault();
    showMenu(e, [
      { label: "New File…", run: () => createEntry("file", root) },
      { label: "New Folder…", run: () => createEntry("folder", root) },
      { separator: true },
      { label: "Refresh", run: () => refreshAll() },
      { label: "Collapse All", run: collapseAll },
    ]);
  };

  const rootState = dirs[root];
  const refreshing = Object.values(dirs).some((d) => d.loading);
  return (
    <>
      <div className="oc-sidebar-title">
        <span className="oc-ellipsis" title={root}>
          Explorer
        </span>
        <div className="oc-grow" />
        <button className="oc-iconbtn" title="New File…" aria-label="New File" onClick={() => void createEntry("file")}>
          <FilePlus size={15} />
        </button>
        <button className="oc-iconbtn" title="New Folder…" aria-label="New Folder" onClick={() => void createEntry("folder")}>
          <FolderPlus size={15} />
        </button>
        <button className="oc-iconbtn" title="Refresh" aria-label="Refresh" onClick={() => void refreshAll()}>
          <RefreshCw size={14} className={refreshing ? "oc-spin" : undefined} />
        </button>
        <button className="oc-iconbtn" title="Collapse All" aria-label="Collapse All" onClick={collapseAll}>
          <ChevronsDownUp size={15} />
        </button>
      </div>
      <div ref={scroller} className="oc-sidebar-body ex-tree" role="tree" aria-label={`Files in ${workspace.name}`} tabIndex={0} onKeyDown={onKeyDown} onContextMenu={onBackgroundMenu}>
        {rootState?.error && !rootState.entries ? (
          <div className="ex-empty" role="alert">
            <div className="oc-error-text oc-selectable">{rootState.error}</div>
            <button className="oc-btn oc-btn-sm" onClick={() => void loadDir(root, true)}>
              Retry
            </button>
          </div>
        ) : rootState?.entries?.length === 0 ? (
          <div className="ex-empty">This folder is empty.</div>
        ) : !rootState?.entries ? (
          <div className="ex-empty">Loading…</div>
        ) : (
          <div className="ex-rows" style={{ height: virtualizer.getTotalSize() }}>
            {virtualizer.getVirtualItems().map((item) => {
              const row = rows[item.index];
              if (!row) return null;
              return row.kind === "entry" ? (
                <TreeRow key={row.key} row={row} top={item.start} selected={row.entry.path === selected} active={row.entry.path === activePath} decorations={decorations} />
              ) : (
                <NoteRow key={row.key} row={row} top={item.start} />
              );
            })}
          </div>
        )}
      </div>
    </>
  );
}
