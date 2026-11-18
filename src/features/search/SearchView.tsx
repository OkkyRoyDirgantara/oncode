// Tampilan sidebar Search: isian kueri dan pengganti, lalu pohon hasil per berkas yang divirtualisasi.
import { useEffect, useMemo, useRef } from "react";
import type { KeyboardEvent as ReactKeyboardEvent, ReactNode } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { CaseSensitive, ChevronDown, ChevronRight, ChevronsDownUp, Eraser, LoaderCircle, RefreshCw, Regex, ReplaceAll, WholeWord, X } from "lucide-react";
import { openFile } from "@/app/coreCommands";
import { absolutePath, useApp } from "@/app/store";
import type { FileMatches, LineMatch } from "@/ipc";
import { getBuffer } from "../editor/buffers";
import { compileMatcher, expandReplacement, type Matcher } from "./matcher";
import { replaceAll } from "./replace";
import {
  clearSearch,
  dismissFile,
  dismissMatch,
  flattenResults,
  resultCounts,
  searchNow,
  selectRow,
  setAllCollapsed,
  setCollapsed,
  setFilter,
  setPattern,
  setReplacement,
  summaryText,
  toggleFilters,
  toggleOption,
  toggleReplace,
  useSearch,
  type ResultRow,
  type SearchOption,
} from "./store";
import "./search.css";

const ROW_HEIGHT = 22;

let revealToken = 0;

/**
 * Membuka berkas dan memilih kecocokannya. `openFile` tidak membawa panjang seleksi, dan editor
 * baru mendengarkan `oncode:reveal` setelah terpasang, jadi event dikirim sendiri setelah buffer punya editor.
 */
export function openMatch(relativePath: string, match: Pick<LineMatch, "line" | "column" | "length">): void {
  const path = absolutePath(relativePath);
  openFile(path, { preview: true });
  const token = ++revealToken;
  const detail = { path, line: match.line, column: match.column, length: match.length };
  let attempts = 0;
  const attempt = () => {
    // Klik berikutnya menggantikan permintaan ini.
    if (token !== revealToken) return;
    if ((getBuffer(path)?.views.size ?? 0) > 0) {
      setTimeout(() => token === revealToken && window.dispatchEvent(new CustomEvent("oncode:reveal", { detail })), 30);
    } else if (++attempts < 80) {
      setTimeout(attempt, 50);
    }
  };
  setTimeout(attempt, 0);
}

function OptionButton({ option, title, children }: { option: SearchOption; title: string; children: ReactNode }) {
  const pressed = useSearch((s) => s[option]);
  return (
    <button type="button" className="oc-iconbtn sr-option" title={title} aria-label={title} aria-pressed={pressed} onClick={() => toggleOption(option)}>
      {children}
    </button>
  );
}

function Summary() {
  const status = useSearch((s) => s.status);
  const files = useSearch((s) => s.files);
  const error = useSearch((s) => s.error);
  const truncated = useSearch((s) => s.summary?.truncated ?? false);
  const replacing = useSearch((s) => s.replacing);
  const counts = resultCounts(files);

  let content: ReactNode = null;
  if (replacing) content = <span>Replacing…</span>;
  else if (status === "failed") content = <span className="oc-error-text oc-selectable">{error ?? "The search failed."}</span>;
  else if (status === "running") content = <span>{counts.matches > 0 ? `${summaryText(files, false)} so far…` : "Searching…"}</span>;
  else if (status === "done") content = <span>{counts.matches === 0 ? "No results found." : summaryText(files, truncated)}</span>;

  return (
    <div className="sr-summary" role="status" aria-live="polite">
      {(status === "running" || replacing) && <LoaderCircle size={13} className="oc-spin" aria-label="Searching" />}
      {content}
    </div>
  );
}

/** Teks pengganti untuk satu kecocokan (dengan `$1` dkk. terisi bila bisa dihitung dari pratinjau). */
function replacementPreview(matcher: Matcher | null, interpolate: boolean, replacement: string, match: LineMatch): string {
  if (!interpolate) return replacement;
  const hit = matcher?.find(match.preview).find((h) => h.start === match.previewOffset && h.end - h.start === match.length);
  return hit ? expandReplacement(replacement, hit) : replacement;
}

function FileRow({ file, collapsed, top, selected }: { file: FileMatches; collapsed: boolean; top: number; selected: boolean }) {
  const slash = file.path.lastIndexOf("/");
  const name = file.path.slice(slash + 1);
  const dir = slash >= 0 ? file.path.slice(0, slash) : "";
  return (
    <div
      role="treeitem"
      className="oc-list-row sr-row sr-file"
      aria-level={1}
      aria-expanded={!collapsed}
      aria-selected={selected}
      title={file.path}
      style={{ transform: `translateY(${top}px)` }}
      onClick={() => {
        selectRow(file.path);
        setCollapsed(file.path, !collapsed);
      }}
    >
      <span className="sr-twist">{collapsed ? <ChevronRight size={14} /> : <ChevronDown size={14} />}</span>
      <span className="oc-ellipsis sr-file-label">
        <span className="sr-file-name">{name}</span>
        {dir && <span className="sr-file-dir">{dir}</span>}
      </span>
      <div className="oc-row-actions">
        <button
          className="oc-iconbtn"
          title="Dismiss file"
          aria-label={`Dismiss ${file.path}`}
          onClick={(e) => {
            e.stopPropagation();
            dismissFile(file.path);
          }}
        >
          <X size={14} />
        </button>
      </div>
      <span className="oc-count">{file.matches.length}</span>
    </div>
  );
}

function MatchRow({ row, top, selected, replacement }: { row: Extract<ResultRow, { kind: "match" }>; top: number; selected: boolean; replacement: string | null }) {
  const { file, match } = row;
  // Lekukan di awal baris dibuang agar kecocokan lebih cepat terlihat.
  const lead = Math.min(match.preview.length - match.preview.trimStart().length, match.previewOffset);
  const before = match.preview.slice(lead, match.previewOffset);
  const hit = match.preview.slice(match.previewOffset, match.previewOffset + match.length);
  const after = match.preview.slice(match.previewOffset + match.length);
  return (
    <div
      role="treeitem"
      className="oc-list-row sr-row sr-match"
      aria-level={2}
      aria-selected={selected}
      title={`Line ${match.line}`}
      style={{ transform: `translateY(${top}px)` }}
      onClick={() => {
        selectRow(row.key);
        openMatch(file.path, match);
      }}
    >
      <span className="oc-ellipsis sr-preview oc-mono">
        {before}
        <mark className="sr-hit" data-replaced={replacement !== null || undefined}>
          {hit}
        </mark>
        {replacement !== null && replacement !== "" && <ins className="sr-new">{replacement}</ins>}
        {after}
      </span>
      <div className="oc-row-actions">
        <button
          className="oc-iconbtn"
          title="Dismiss match"
          aria-label={`Dismiss match on line ${match.line}`}
          onClick={(e) => {
            e.stopPropagation();
            dismissMatch(file.path, match);
          }}
        >
          <X size={14} />
        </button>
      </div>
    </div>
  );
}

function Results() {
  const shown = useApp((s) => s.sidebarVisible && s.sidebarView === "search");
  const files = useSearch((s) => s.files);
  const collapsed = useSearch((s) => s.collapsed);
  const selected = useSearch((s) => s.selected);
  const showReplace = useSearch((s) => s.showReplace);
  const replacement = useSearch((s) => s.replacement);
  const query = useSearch((s) => s.resultQuery);

  const scroller = useRef<HTMLDivElement>(null);
  const rows = useMemo(() => flattenResults(files, collapsed), [files, collapsed]);
  const virtualizer = useVirtualizer({ count: rows.length, getScrollElement: () => scroller.current, estimateSize: () => ROW_HEIGHT, overscan: 12 });
  // Pratinjau pengganti pada mode regex butuh pencocok di sisi JS; bila pola tak bisa diterjemahkan, teks pengganti tampil apa adanya.
  const matcher = useMemo(() => {
    if (!showReplace || !query?.isRegex) return null;
    const compiled = compileMatcher(query);
    return compiled.ok ? compiled.matcher : null;
  }, [showReplace, query]);

  useEffect(() => {
    if (!shown || !selected) return;
    const index = rows.findIndex((r) => r.key === selected);
    if (index >= 0) virtualizer.scrollToIndex(index, { align: "auto" });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selected, shown]);

  const onKeyDown = (e: ReactKeyboardEvent) => {
    if (e.ctrlKey || e.metaKey || e.altKey || e.target !== e.currentTarget) return;
    const index = rows.findIndex((r) => r.key === selected);
    const row = index >= 0 ? rows[index] : null;
    const move = (to: number) => {
      const next = rows[Math.max(0, Math.min(to, rows.length - 1))];
      if (next) selectRow(next.key);
    };
    switch (e.key) {
      case "ArrowDown":
        move(index + 1);
        break;
      case "ArrowUp":
        move(index < 0 ? 0 : index - 1);
        break;
      case "ArrowRight":
        if (row?.kind === "file" && row.collapsed) setCollapsed(row.file.path, false);
        break;
      case "ArrowLeft":
        if (row?.kind === "file" && !row.collapsed) setCollapsed(row.file.path, true);
        else if (row?.kind === "match") selectRow(row.file.path);
        break;
      case "Enter":
        if (row?.kind === "file") setCollapsed(row.file.path, !row.collapsed);
        else if (row) openMatch(row.file.path, row.match);
        break;
      default:
        return;
    }
    e.preventDefault();
  };

  return (
    <div ref={scroller} className="oc-sidebar-body sr-results" role="tree" aria-label="Search results" tabIndex={0} onKeyDown={onKeyDown}>
      <div className="sr-rows" style={{ height: virtualizer.getTotalSize() }}>
        {virtualizer.getVirtualItems().map((item) => {
          const row = rows[item.index];
          if (!row) return null;
          return row.kind === "file" ? (
            <FileRow key={row.key} file={row.file} collapsed={row.collapsed} top={item.start} selected={row.key === selected} />
          ) : (
            <MatchRow
              key={row.key}
              row={row}
              top={item.start}
              selected={row.key === selected}
              replacement={showReplace ? replacementPreview(matcher, query?.isRegex ?? false, replacement, row.match) : null}
            />
          );
        })}
      </div>
    </div>
  );
}

export function SearchView() {
  const workspace = useApp((s) => s.workspace);
  const pattern = useSearch((s) => s.pattern);
  const replacement = useSearch((s) => s.replacement);
  const include = useSearch((s) => s.include);
  const exclude = useSearch((s) => s.exclude);
  const showReplace = useSearch((s) => s.showReplace);
  const showFilters = useSearch((s) => s.showFilters);
  const focusSeq = useSearch((s) => s.focusSeq);
  const canReplace = useSearch((s) => s.status === "done" && !s.pending && !s.replacing && s.files.length > 0);
  const hasResults = useSearch((s) => s.files.length > 0);
  const allCollapsed = useSearch((s) => s.files.length > 0 && s.files.every((f) => s.collapsed[f.path]));
  const queryInput = useRef<HTMLInputElement>(null);
  const handledFocus = useRef(0);

  // Command `search.focus`: kolom kueri difokuskan dan isinya dipilih. Permintaan yang datang
  // sebelum kolomnya ada (belum ada folder terbuka) dilayani begitu kolom itu muncul.
  useEffect(() => {
    const input = queryInput.current;
    if (focusSeq === handledFocus.current || !input) return;
    handledFocus.current = focusSeq;
    input.focus();
    input.select();
  }, [focusSeq, workspace]);

  if (!workspace) return null;

  const searchOnEnter = (e: ReactKeyboardEvent) => {
    if (e.key !== "Enter") return;
    e.preventDefault();
    searchNow();
  };

  return (
    <>
      <div className="oc-sidebar-title">
        <span className="oc-ellipsis">Search</span>
        <div className="oc-grow" />
        <button className="oc-iconbtn" title="Search Again" aria-label="Search Again" disabled={pattern === ""} onClick={searchNow}>
          <RefreshCw size={14} />
        </button>
        <button className="oc-iconbtn" title="Clear Search Results" aria-label="Clear Search Results" disabled={pattern === ""} onClick={clearSearch}>
          <Eraser size={14} />
        </button>
        <button className="oc-iconbtn" title={allCollapsed ? "Expand All" : "Collapse All"} aria-label={allCollapsed ? "Expand All" : "Collapse All"} disabled={!hasResults} onClick={() => setAllCollapsed(!allCollapsed)}>
          <ChevronsDownUp size={15} />
        </button>
      </div>
      <div className="sr-form">
        <div className="sr-line">
          <button type="button" className="oc-iconbtn sr-expander" title="Toggle Replace" aria-label="Toggle Replace" aria-expanded={showReplace} onClick={toggleReplace}>
            {showReplace ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
          </button>
          <div className="sr-fields">
            <div className="sr-field sr-query">
              <input
                ref={queryInput}
                className="oc-input"
                type="text"
                placeholder="Search"
                aria-label="Search"
                autoComplete="off"
                spellCheck={false}
                value={pattern}
                onChange={(e) => setPattern(e.target.value)}
                onKeyDown={searchOnEnter}
              />
              <div className="sr-options">
                <OptionButton option="caseSensitive" title="Match Case">
                  <CaseSensitive size={15} />
                </OptionButton>
                <OptionButton option="wholeWord" title="Match Whole Word">
                  <WholeWord size={15} />
                </OptionButton>
                <OptionButton option="isRegex" title="Use Regular Expression">
                  <Regex size={15} />
                </OptionButton>
              </div>
            </div>
            {showReplace && (
              <div className="sr-field">
                <input
                  className="oc-input"
                  type="text"
                  placeholder="Replace"
                  aria-label="Replace"
                  autoComplete="off"
                  spellCheck={false}
                  value={replacement}
                  onChange={(e) => setReplacement(e.target.value)}
                />
                <button type="button" className="oc-iconbtn" title="Replace All" aria-label="Replace All" disabled={!canReplace} onClick={() => void replaceAll()}>
                  <ReplaceAll size={15} />
                </button>
              </div>
            )}
          </div>
        </div>
        <button type="button" className="sr-filters-toggle" aria-expanded={showFilters} onClick={toggleFilters}>
          {showFilters ? <ChevronDown size={13} /> : <ChevronRight size={13} />}
          <span>files to include / exclude</span>
          {!showFilters && (include.trim() !== "" || exclude.trim() !== "") && <span className="sr-filters-on">active</span>}
        </button>
        {showFilters && (
          <div className="sr-filters">
            <label className="oc-label" htmlFor="sr-include">
              files to include
            </label>
            <input
              id="sr-include"
              className="oc-input"
              type="text"
              placeholder="e.g. src, *.ts"
              autoComplete="off"
              spellCheck={false}
              value={include}
              onChange={(e) => setFilter("include", e.target.value)}
              onKeyDown={searchOnEnter}
            />
            <label className="oc-label" htmlFor="sr-exclude">
              files to exclude
            </label>
            <input
              id="sr-exclude"
              className="oc-input"
              type="text"
              placeholder="e.g. dist, *.min.js"
              autoComplete="off"
              spellCheck={false}
              value={exclude}
              onChange={(e) => setFilter("exclude", e.target.value)}
              onKeyDown={searchOnEnter}
            />
          </div>
        )}
      </div>
      <Summary />
      <Results />
    </>
  );
}
