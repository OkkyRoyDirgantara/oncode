// Tab Graph: panel referensi, graph commit, dan detail commit.
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { MouseEvent as ReactMouseEvent } from "react";
import { Group as PanelGroup, Panel, Separator } from "react-resizable-panels";
import { Ellipsis, RefreshCw } from "lucide-react";
import { executeCommand } from "@/app/commands";
import { showMenu, toast } from "@/app/overlay";
import type { TabProps } from "@/app/registry";
import { revealSidebar } from "@/app/store";
import type { GraphRow } from "@/ipc";
import * as git from "@/features/scm/actions";
import { changeCount, onRefsChanged, useRepo } from "@/features/scm/store";
import { onRevealRequested, takeReveal, type RevealTarget } from "./bus";
import { CommitDetailView, CompareView, ErrorBox } from "./CommitDetails";
import { plural, shortOid } from "./format";
import { GraphList, type GraphListHandle } from "./GraphList";
import { FIND_CAP, useLogStream } from "./logStream";
import { commitMenu } from "./menus";
import { openReflog } from "./open";
import { buildQuery, nextSelection, orderOldestFirst, scopeExists } from "./query";
import { RefsPane } from "./RefsPane";
import "./graph.css";

function useDebounced<T>(value: T, delayMs: number): T {
  const [debounced, setDebounced] = useState(value);
  useEffect(() => {
    const timer = setTimeout(() => setDebounced(value), delayMs);
    return () => clearTimeout(timer);
  }, [value, delayMs]);
  return debounced;
}

export function GraphTab({ tab, visible }: TabProps) {
  const root = tab.params.root as string;
  const repo = useRepo(root);
  const refs = repo?.refs ?? null;

  const [search, setSearch] = useState("");
  const [author, setAuthor] = useState("");
  const [scope, setScope] = useState("all");
  const grep = useDebounced(search, 300);
  const authorQuery = useDebounced(author, 300);
  const query = useMemo(() => buildQuery(scope, grep, authorQuery), [scope, grep, authorQuery]);
  const filtered = !!(query.grep || query.author);
  const { controller, state } = useLogStream(root, query);

  const [selected, setSelected] = useState<string[]>([]);
  const [seeking, setSeeking] = useState<string | null>(null);
  const list = useRef<GraphListHandle>(null);

  // Cakupan yang menunjuk ref yang sudah dihapus kembali ke "All branches".
  useEffect(() => {
    if (!scopeExists(scope, refs)) setScope("all");
  }, [scope, refs]);

  // Muat ulang saat ref berubah; tab yang tidak terlihat menunda sampai ditampilkan lagi.
  const visibleRef = useRef(visible);
  const stale = useRef(false);
  useEffect(() => {
    visibleRef.current = visible;
    if (visible && stale.current && controller) {
      stale.current = false;
      void controller.open(true);
    }
  }, [visible, controller]);
  useEffect(
    () =>
      onRefsChanged((changed) => {
        if (changed !== root || !controller) return;
        if (visibleRef.current) void controller.open(true);
        else stale.current = true;
      }),
    [root, controller],
  );

  const reveal = useCallback(
    async (target: RevealTarget) => {
      if (!controller) return;
      if (!target.hunt) {
        // Commit yang belum tentu ada di daftar (mis. stash): cukup dipilih; detailnya dimuat lewat hash.
        setSelected([target.oid]);
        const at = controller.getState().rows.findIndex((r) => r.commit.oid === target.oid);
        if (at >= 0) list.current?.scrollToRow(at);
        return;
      }
      setSeeking(target.label);
      const index = await controller.find(target.oid);
      setSeeking(null);
      if (controller.disposed) return;
      if (index < 0) {
        const loaded = controller.getState().rows.length;
        const reason = controller.getState().error
          ? "the history could not be loaded"
          : loaded >= FIND_CAP
            ? `it is not among the first ${loaded} commits`
            : controller.query.all && !controller.query.grep && !controller.query.author
              ? "it is not part of the history shown"
              : "it is hidden by the current branch scope or filters";
        toast.info(`${target.label} was not found in the graph: ${reason}.`);
        return;
      }
      setSelected([target.oid]);
      list.current?.scrollToRow(index);
    },
    [controller],
  );

  // Permintaan dari luar tab (sidebar, reflog, riwayat berkas).
  useEffect(() => {
    if (!controller) return;
    const take = (requested: string) => {
      if (requested !== root) return;
      const target = takeReveal(root);
      if (target) void reveal(target);
    };
    take(root);
    return onRevealRequested(take);
  }, [root, controller, reveal]);

  const onSelect = useCallback((oid: string, additive: boolean) => setSelected((previous) => nextSelection(previous, oid, additive)), []);

  // Menu konteks membaca keadaan terbaru lewat ref agar baris tidak dirender ulang tiap pilihan berubah.
  const latest = useRef({ rows: state.rows, selected, filtered });
  latest.current = { rows: state.rows, selected, filtered };
  const onMenu = useCallback(
    (e: ReactMouseEvent, row: GraphRow) => {
      e.preventDefault();
      const { rows, selected: chosen, filtered: isFiltered } = latest.current;
      const inSelection = chosen.includes(row.commit.oid);
      if (!inSelection) setSelected([row.commit.oid]);
      const selection = inSelection && chosen.length > 1 ? orderOldestFirst(rows, chosen) : undefined;
      showMenu(e, commitMenu({ root, commit: row.commit, rows, filtered: isFiltered, selection }));
    },
    [root],
  );

  const comparison = useMemo(() => (selected.length === 2 ? orderOldestFirst(state.rows, selected) : null), [state.rows, selected]);
  const changes = changeCount(repo?.snapshot ?? null);
  const emptyText = filtered ? "No commits match the search." : repo?.snapshot?.head.kind === "unborn" ? "This repository has no commits yet." : "No commits to show.";

  const openMore = (e: ReactMouseEvent<HTMLButtonElement>) => {
    const rect = e.currentTarget.getBoundingClientRect();
    showMenu({ x: rect.left, y: rect.bottom + 2 }, [
      { label: "Fetch", run: () => git.fetch(root) },
      { label: "Show Reflog", run: () => openReflog(root) },
      { separator: true },
      { label: "Interactive Rebase from Root…", run: () => executeCommand("git.interactiveRebase", root, null) },
    ]);
  };

  if (!repo) return <div className="oc-empty">This repository is no longer open.</div>;

  return (
    <div className="gr-tab">
      <PanelGroup orientation="horizontal" className="gr-panels">
        <Panel id="refs" defaultSize={220} minSize={140} maxSize={520} groupResizeBehavior="preserve-pixel-size">
          <RefsPane root={root} onReveal={(target) => void reveal(target)} />
        </Panel>
        <Separator className="oc-separator oc-separator-h" />
        <Panel id="log" minSize={260}>
          <div className="gr-center">
            <div className="gr-toolbar" role="toolbar" aria-label="Graph filters">
              <input className="oc-input gr-search" type="search" placeholder="Search commit messages" aria-label="Search commit messages" value={search} spellCheck={false} onChange={(e) => setSearch(e.target.value)} />
              <input className="oc-input gr-author-filter" type="search" placeholder="Author" aria-label="Filter by author" value={author} spellCheck={false} onChange={(e) => setAuthor(e.target.value)} />
              <select className="oc-select gr-scope" aria-label="Branch scope" value={scope} onChange={(e) => setScope(e.target.value)}>
                <option value="all">All branches</option>
                <option value="current">Current branch</option>
                {refs && refs.branches.length > 0 && (
                  <optgroup label="Local">
                    {refs.branches.map((b) => (
                      <option key={b.name} value={`ref:${b.name}`}>
                        {b.name}
                      </option>
                    ))}
                  </optgroup>
                )}
                {refs && refs.remoteBranches.length > 0 && (
                  <optgroup label="Remote">
                    {refs.remoteBranches
                      .filter((b) => !b.name.endsWith("/HEAD"))
                      .map((b) => (
                        <option key={b.name} value={`ref:${b.name}`}>
                          {b.name}
                        </option>
                      ))}
                  </optgroup>
                )}
                {refs && refs.tags.length > 0 && (
                  <optgroup label="Tags">
                    {refs.tags.map((t) => (
                      <option key={t.name} value={`ref:${t.name}`}>
                        {t.name}
                      </option>
                    ))}
                  </optgroup>
                )}
              </select>
              <span className="gr-status oc-ellipsis" aria-live="polite">
                {seeking ? `Looking for ${seeking}…` : state.rows.length > 0 ? `${plural(state.rows.length, "commit")}${state.done ? "" : "+"}` : ""}
              </span>
              <button className="oc-iconbtn" title="Refresh" aria-label="Refresh" disabled={!controller} onClick={() => void controller?.open(true)}>
                <RefreshCw size={14} className={state.loading ? "oc-spin" : undefined} />
              </button>
              <button className="oc-iconbtn" title="More actions" aria-label="More actions" onClick={openMore}>
                <Ellipsis size={15} />
              </button>
            </div>
            {state.error && <ErrorBox message={`Could not load the commit history: ${state.error}`} retry={() => void controller?.open(state.rows.length > 0)} />}
            <GraphList
              state={state}
              controller={controller}
              changeCount={changes}
              selected={selected}
              onSelect={onSelect}
              onMenu={onMenu}
              onOpenChanges={() => revealSidebar("scm")}
              emptyText={emptyText}
              visible={visible}
              handleRef={list}
            />
          </div>
        </Panel>
        <Separator className="oc-separator oc-separator-h" />
        <Panel id="details" defaultSize={340} minSize={200} groupResizeBehavior="preserve-pixel-size">
          <div className="gr-details-pane">
            {selected.length === 0 && (
              <div className="gr-hint">
                Select a commit to see its details.
                <br />
                Ctrl-click a second commit to compare the two.
              </div>
            )}
            {selected.length === 1 && <CommitDetailView root={root} oid={selected[0]} onSelectCommit={(oid) => void reveal({ oid, label: shortOid(oid), hunt: true })} />}
            {comparison && <CompareView root={root} from={comparison[0]} to={comparison[1]} />}
          </div>
        </Panel>
      </PanelGroup>
    </div>
  );
}
