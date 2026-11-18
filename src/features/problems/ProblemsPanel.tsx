// Panel Problems: diagnostics dari semua language server, dikelompokkan per berkas.
import { useCallback, useMemo, useRef, useState } from "react";
import type { ReactNode } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { ChevronDown, ChevronRight, CircleX, Info, TriangleAlert } from "lucide-react";
import { openFile } from "@/app/coreCommands";
import { relativePath, showPanel, useApp } from "@/app/store";
import { useDiagnostics, type Severity } from "@/features/lsp/diagnostics";
import { DEFAULT_FILTER, buildGroups, countProblems, flattenRows, type ProblemCounts, type ProblemFilter, type ProblemRow } from "./model";
import "./problems.css";

const ROW_HEIGHT = 24;
/** Di atas jumlah baris ini daftar divirtualisasi. */
export const VIRTUALIZE_THRESHOLD = 300;

export function SeverityIcon({ severity, size = 14 }: { severity: Severity; size?: number }) {
  if (severity === "error") return <CircleX size={size} className="pb-error" aria-label="Error" />;
  if (severity === "warning") return <TriangleAlert size={size} className="pb-warning" aria-label="Warning" />;
  return <Info size={size} className="pb-info" aria-label="Info" />;
}

/** Jumlah masalah di seluruh workspace. Selector mengambil nilai tersimpan; turunannya dihitung dengan useMemo. */
export function useProblemCounts(): ProblemCounts {
  const files = useDiagnostics((s) => s.files);
  return useMemo(() => countProblems(files), [files]);
}

/** Lencana tab panel: galat + peringatan. */
export function useProblemsBadge(): number | undefined {
  const counts = useProblemCounts();
  return counts.errors + counts.warnings || undefined;
}

function Row({ row, collapsed, onToggle }: { row: ProblemRow; collapsed: boolean; onToggle: (path: string) => void }) {
  if (row.kind === "file") {
    const { group } = row;
    return (
      <div
        className="oc-list-row pb-row pb-file"
        role="button"
        tabIndex={0}
        aria-expanded={!collapsed}
        title={group.path}
        onClick={() => onToggle(group.path)}
        onKeyDown={(e) => e.key === "Enter" && onToggle(group.path)}
      >
        {collapsed ? <ChevronRight size={14} /> : <ChevronDown size={14} />}
        <span>{group.name}</span>
        {group.dir && <span className="pb-dir">{group.dir}</span>}
        <span className="oc-count">{group.items.length}</span>
      </div>
    );
  }
  const { diagnostic, group } = row;
  const origin = [diagnostic.source, diagnostic.code && `(${diagnostic.code})`].filter(Boolean).join(" ");
  return (
    <div
      className="oc-list-row pb-row pb-problem"
      role="button"
      tabIndex={0}
      title={diagnostic.message}
      onClick={() => openFile(group.path, { line: diagnostic.line, column: diagnostic.column })}
      onKeyDown={(e) => e.key === "Enter" && openFile(group.path, { line: diagnostic.line, column: diagnostic.column })}
    >
      <SeverityIcon severity={diagnostic.severity} />
      <span className="pb-message">{diagnostic.message.split("\n")[0]}</span>
      {origin && <span className="pb-meta">{origin}</span>}
      <span className="pb-meta">
        [Ln {diagnostic.line}, Col {diagnostic.column + 1}]
      </span>
    </div>
  );
}

function VirtualRows({ rows, render }: { rows: ProblemRow[]; render: (row: ProblemRow) => ReactNode }) {
  const scroller = useRef<HTMLDivElement>(null);
  const virtualizer = useVirtualizer({ count: rows.length, getScrollElement: () => scroller.current, estimateSize: () => ROW_HEIGHT, overscan: 12 });
  return (
    <div className="pb-list" ref={scroller} data-virtual="true">
      <div style={{ height: virtualizer.getTotalSize(), position: "relative" }}>
        {virtualizer.getVirtualItems().map((item) => (
          <div key={item.key} style={{ position: "absolute", top: 0, left: 0, right: 0, height: ROW_HEIGHT, transform: `translateY(${item.start}px)` }}>
            {render(rows[item.index])}
          </div>
        ))}
      </div>
    </div>
  );
}

function Toggle({ label, count, pressed, onClick, children }: { label: string; count: number; pressed: boolean; onClick: () => void; children: ReactNode }) {
  return (
    <button className="pb-toggle" aria-pressed={pressed} title={`${pressed ? "Hide" : "Show"} ${label}`} aria-label={`${label}: ${count}`} onClick={onClick}>
      {children}
      {count}
    </button>
  );
}

export function ProblemsPanel(_props: { visible: boolean }) {
  const files = useDiagnostics((s) => s.files);
  // Path relatif bergantung pada folder proyek; ikutkan sebagai dependensi.
  const root = useApp((s) => s.workspace?.root ?? null);
  const [filter, setFilter] = useState<ProblemFilter>(DEFAULT_FILTER);
  const [collapsed, setCollapsed] = useState<ReadonlySet<string>>(() => new Set());

  const totals = useMemo(() => countProblems(files), [files]);
  // eslint-disable-next-line react-hooks/exhaustive-deps
  const groups = useMemo(() => buildGroups(files, filter, relativePath), [files, filter, root]);
  const rows = useMemo(() => flattenRows(groups, collapsed), [groups, collapsed]);
  const toggle = useCallback((path: string) => {
    setCollapsed((previous) => {
      const next = new Set(previous);
      if (!next.delete(path)) next.add(path);
      return next;
    });
  }, []);
  const render = (row: ProblemRow) => <Row row={row} collapsed={row.kind === "file" && collapsed.has(row.group.path)} onToggle={toggle} />;

  const total = totals.errors + totals.warnings + totals.infos;
  return (
    <div className="pb-root">
      <div className="pb-toolbar">
        <input
          className="oc-input"
          placeholder="Filter by message, source, code or file"
          aria-label="Filter problems"
          value={filter.text}
          onChange={(e) => setFilter({ ...filter, text: e.target.value })}
        />
        <Toggle label="Errors" count={totals.errors} pressed={filter.errors} onClick={() => setFilter({ ...filter, errors: !filter.errors })}>
          <CircleX size={13} className="pb-error" />
        </Toggle>
        <Toggle label="Warnings" count={totals.warnings} pressed={filter.warnings} onClick={() => setFilter({ ...filter, warnings: !filter.warnings })}>
          <TriangleAlert size={13} className="pb-warning" />
        </Toggle>
        <Toggle label="Infos" count={totals.infos} pressed={filter.infos} onClick={() => setFilter({ ...filter, infos: !filter.infos })}>
          <Info size={13} className="pb-info" />
        </Toggle>
      </div>
      {total === 0 ? (
        <div className="oc-empty">No problems have been detected in the workspace.</div>
      ) : rows.length === 0 ? (
        <div className="oc-empty">No problems match the current filter.</div>
      ) : rows.length > VIRTUALIZE_THRESHOLD ? (
        <VirtualRows rows={rows} render={render} />
      ) : (
        <div className="pb-list">
          {rows.map((row, index) => (
            <div key={`${row.group.path}:${index}`}>{render(row)}</div>
          ))}
        </div>
      )}
    </div>
  );
}

/** Item status bar: jumlah galat dan peringatan; klik membuka panel. */
export function ProblemsStatus() {
  const counts = useProblemCounts();
  return (
    <button
      className="oc-status-item"
      title={`${counts.errors} error${counts.errors === 1 ? "" : "s"}, ${counts.warnings} warning${counts.warnings === 1 ? "" : "s"}`}
      aria-label={`Problems: ${counts.errors} errors, ${counts.warnings} warnings`}
      onClick={() => showPanel("problems")}
    >
      <CircleX size={13} />
      {counts.errors}
      <TriangleAlert size={13} />
      {counts.warnings}
    </button>
  );
}
