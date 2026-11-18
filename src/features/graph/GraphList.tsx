// Daftar commit tervirtualisasi; tiap baris menggambar potongan graph-nya sendiri dengan SVG kecil.
import { memo, useEffect, useImperativeHandle, useLayoutEffect, useMemo, useRef } from "react";
import type { KeyboardEvent as ReactKeyboardEvent, MouseEvent as ReactMouseEvent, Ref } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import type { GraphRow } from "@/ipc";
import { fullDate, relativeDate, shortOid } from "./format";
import { DOT_RADIUS, edgePath, graphWidth, laneColor, laneX, maxLane, ROW_HEIGHT } from "./geometry";
import type { LogController, LogState } from "./logStream";
import { RefBadges } from "./RefBadge";

/** Halaman berikutnya diminta saat baris yang terlihat tinggal sejauh ini dari akhir. */
export const LOAD_AHEAD = 60;

export interface GraphListHandle {
  /** Menggulir sampai baris commit ke-`index` terlihat. */
  scrollToRow: (index: number) => void;
  focus: () => void;
}

function RowGraph({ row, width }: { row: GraphRow; width: number }) {
  const merge = row.commit.parents.length > 1;
  const color = laneColor(row.color);
  return (
    <svg className="gr-graph" width={width} height={ROW_HEIGHT} viewBox={`0 0 ${width} ${ROW_HEIGHT}`} aria-hidden="true">
      {row.edges.map((edge, i) => (
        <path key={i} d={edgePath(edge)} stroke={laneColor(edge.color)} data-kind={edge.kind} />
      ))}
      {merge ? (
        // Commit merge digambar sebagai titik berongga.
        <circle className="gr-dot-merge" cx={laneX(row.column)} cy={ROW_HEIGHT / 2} r={DOT_RADIUS - 0.5} stroke={color} />
      ) : (
        <circle cx={laneX(row.column)} cy={ROW_HEIGHT / 2} r={DOT_RADIUS} fill={color} />
      )}
    </svg>
  );
}

interface CommitRowProps {
  row: GraphRow;
  start: number;
  width: number;
  selected: boolean;
  onSelect: (oid: string, additive: boolean) => void;
  onMenu: (e: ReactMouseEvent, row: GraphRow) => void;
}

const CommitRow = memo(function CommitRow({ row, start, width, selected, onSelect, onMenu }: CommitRowProps) {
  const { commit } = row;
  return (
    <div
      className="gr-row"
      role="option"
      aria-selected={selected}
      data-oid={commit.oid}
      style={{ transform: `translateY(${start}px)` }}
      onClick={(e) => onSelect(commit.oid, e.ctrlKey || e.metaKey)}
      onContextMenu={(e) => onMenu(e, row)}
    >
      <RowGraph row={row} width={width} />
      <span className="gr-row-main">
        <RefBadges labels={commit.refs} />
        <span className="gr-subject oc-ellipsis" title={commit.subject}>
          {commit.subject}
        </span>
      </span>
      <span className="gr-author oc-ellipsis" title={`${commit.author.name} <${commit.author.email}>`}>
        {commit.author.name}
      </span>
      <span className="gr-date" title={fullDate(commit.author.time)}>
        {relativeDate(commit.author.time)}
      </span>
      <span className="gr-hash oc-mono">{shortOid(commit.oid)}</span>
    </div>
  );
});

export interface GraphListProps {
  state: LogState;
  controller: LogController | null;
  /** Jumlah berkas yang belum di-commit; lebih dari nol menambah baris pertama. */
  changeCount: number;
  selected: readonly string[];
  onSelect: (oid: string, additive: boolean) => void;
  onMenu: (e: ReactMouseEvent, row: GraphRow) => void;
  onOpenChanges: () => void;
  emptyText: string;
  /** Tab sedang terlihat; saat kembali terlihat, posisi gulir dipulihkan. */
  visible?: boolean;
  handleRef?: Ref<GraphListHandle>;
}

export function GraphList({ state, controller, changeCount, selected, onSelect, onMenu, onOpenChanges, emptyText, visible = true, handleRef }: GraphListProps) {
  const { rows, done, loading, error } = state;
  const scrollRef = useRef<HTMLDivElement>(null);
  const offset = changeCount > 0 ? 1 : 0;
  // Selama stream belum habis, baris terakhir adalah penanda "Loading…".
  const count = offset + rows.length + (done ? 0 : 1);
  const virtualizer = useVirtualizer({ count, getScrollElement: () => scrollRef.current, estimateSize: () => ROW_HEIGHT, overscan: 12 });
  const items = virtualizer.getVirtualItems();
  const width = useMemo(() => graphWidth(maxLane(rows)), [rows]);

  // Paging: minta halaman berikutnya saat pengguna mendekati akhir daftar.
  const lastVisible = items.length > 0 ? items[items.length - 1].index : -1;
  useEffect(() => {
    if (!controller || done || loading || lastVisible < 0) return;
    if (lastVisible >= count - 1 - LOAD_AHEAD) void controller.loadMore();
  }, [controller, done, loading, lastVisible, count]);

  // Jangkar gulir: commit teratas yang terlihat tetap di tempatnya saat daftar dimuat ulang.
  const anchor = useRef<{ oid: string; index: number; delta: number } | null>(null);
  const current = useRef({ rows, offset });
  const recordAnchor = () => {
    const el = scrollRef.current;
    if (!el) return;
    const top = el.scrollTop;
    const position = Math.floor(top / ROW_HEIGHT);
    const index = position - current.current.offset;
    const row = top > 0 ? current.current.rows[index] : undefined;
    // Di paling atas tidak ada jangkar, supaya commit baru langsung terlihat.
    anchor.current = row ? { oid: row.commit.oid, index, delta: top - position * ROW_HEIGHT } : null;
  };

  // Kueri baru (controller baru) mulai dari atas.
  useLayoutEffect(() => {
    anchor.current = null;
    if (scrollRef.current) scrollRef.current.scrollTop = 0;
  }, [controller]);

  useLayoutEffect(() => {
    current.current = { rows, offset };
    const el = scrollRef.current;
    const a = anchor.current;
    if (!el || !a) return;
    const index = rows[a.index]?.commit.oid === a.oid ? a.index : rows.findIndex((r) => r.commit.oid === a.oid);
    if (index < 0) return;
    a.index = index;
    const top = (index + offset) * ROW_HEIGHT + a.delta;
    if (Math.abs(el.scrollTop - top) >= 1) el.scrollTop = top;
  }, [rows, offset]);

  // Elemen yang disembunyikan (display: none) bisa kehilangan posisi gulirnya; pulihkan dari jangkar.
  useLayoutEffect(() => {
    const el = scrollRef.current;
    const a = anchor.current;
    if (!visible || !el || !a) return;
    const top = (a.index + current.current.offset) * ROW_HEIGHT + a.delta;
    if (Math.abs(el.scrollTop - top) >= 1) el.scrollTop = top;
  }, [visible]);

  useImperativeHandle(
    handleRef,
    () => ({
      scrollToRow: (index: number) => virtualizer.scrollToIndex(index + current.current.offset, { align: "center" }),
      focus: () => scrollRef.current?.focus(),
    }),
    [virtualizer],
  );

  const onKeyDown = (e: ReactKeyboardEvent) => {
    if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
    if (rows.length === 0) return;
    e.preventDefault();
    const last = selected[selected.length - 1];
    const at = last ? rows.findIndex((r) => r.commit.oid === last) : -1;
    const next = at < 0 ? 0 : Math.min(Math.max(at + (e.key === "ArrowDown" ? 1 : -1), 0), rows.length - 1);
    onSelect(rows[next].commit.oid, false);
    virtualizer.scrollToIndex(next + offset, { align: "auto" });
  };

  const selectedSet = useMemo(() => new Set(selected), [selected]);
  const isEmpty = rows.length === 0 && done && !error;

  return (
    <div className="gr-list" ref={scrollRef} role="listbox" aria-label="Commits" aria-multiselectable="true" aria-busy={loading} tabIndex={0} onScroll={recordAnchor} onKeyDown={onKeyDown}>
      {isEmpty && offset === 0 ? (
        <div className="gr-hint gr-list-empty">{emptyText}</div>
      ) : (
        <div className="gr-list-inner" style={{ height: virtualizer.getTotalSize() }}>
          {items.map((item) => {
            if (offset === 1 && item.index === 0) {
              return (
                <div key="uncommitted" className="gr-row gr-row-uncommitted" role="option" aria-selected={false} style={{ transform: `translateY(${item.start}px)` }} title="Show in Source Control" onClick={onOpenChanges}>
                  <svg className="gr-graph" width={width} height={ROW_HEIGHT} viewBox={`0 0 ${width} ${ROW_HEIGHT}`} aria-hidden="true">
                    <circle className="gr-dot-uncommitted" cx={laneX(0)} cy={ROW_HEIGHT / 2} r={DOT_RADIUS - 0.5} />
                  </svg>
                  <span className="gr-row-main">
                    <span className="gr-subject oc-ellipsis">Uncommitted changes ({changeCount})</span>
                  </span>
                </div>
              );
            }
            const row = rows[item.index - offset];
            if (!row) {
              return (
                <div key="loading" className="gr-row gr-row-loading" style={{ transform: `translateY(${item.start}px)` }}>
                  {rows.length === 0 ? "Loading commits…" : "Loading more commits…"}
                </div>
              );
            }
            return <CommitRow key={row.commit.oid} row={row} start={item.start} width={width} selected={selectedSet.has(row.commit.oid)} onSelect={onSelect} onMenu={onMenu} />;
          })}
          {isEmpty && <div className="gr-hint gr-list-empty" style={{ paddingTop: ROW_HEIGHT + 12 }}>{emptyText}</div>}
        </div>
      )}
    </div>
  );
}
