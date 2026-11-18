// Panel Terminal: panggung tempat xterm dipasang, daftar sesi, bilah cari, dan aksi bilah tab.
import { useEffect, useRef, useState } from "react";
import type { KeyboardEvent as ReactKeyboardEvent, MouseEvent as ReactMouseEvent } from "react";
import { ChevronDown, ChevronUp, Plus, SquareTerminal, Trash2, X } from "lucide-react";
import "@xterm/xterm/css/xterm.css";
import { promptInput, showMenu } from "@/app/overlay";
import { useApp } from "@/app/store";
import {
  closeFind,
  closeSession,
  createSession,
  ensureSession,
  findInActiveSession,
  renameSession,
  setActiveSession,
  setPanelVisible,
  setStage,
  useTerminals,
  type TerminalSession,
} from "./store";
import "./terminal.css";

export async function renameTerminal(session: TerminalSession): Promise<void> {
  const title = await promptInput({ title: "Rename Terminal", value: session.title, confirmLabel: "Rename" });
  if (title) renameSession(session.id, title);
}

function SessionList({ sessions, activeId }: { sessions: TerminalSession[]; activeId: number | null }) {
  const menu = (e: ReactMouseEvent, session: TerminalSession) => {
    e.preventDefault();
    showMenu(e, [
      { label: "Rename…", run: () => renameTerminal(session) },
      { separator: true },
      { label: "Kill Terminal", danger: true, run: () => closeSession(session.id) },
    ]);
  };
  return (
    <div className="tm-list" role="listbox" aria-label="Terminals">
      {sessions.map((session) => (
        <div
          key={session.id}
          role="option"
          aria-selected={session.id === activeId}
          className="oc-list-row tm-list-row"
          data-exited={session.exited ? "true" : undefined}
          title={session.exited ? `${session.title} (exited)` : session.title}
          onClick={() => setActiveSession(session.id)}
          onDoubleClick={() => void renameTerminal(session)}
          onContextMenu={(e) => menu(e, session)}
        >
          <SquareTerminal size={14} />
          <span className="tm-list-title">{session.title}</span>
          <div className="oc-row-actions">
            <button
              className="oc-iconbtn"
              title="Kill Terminal"
              aria-label={`Kill ${session.title}`}
              onClick={(e) => {
                e.stopPropagation();
                closeSession(session.id);
              }}
            >
              <Trash2 size={13} />
            </button>
          </div>
        </div>
      ))}
    </div>
  );
}

function FindBar() {
  const [query, setQuery] = useState("");
  const [missing, setMissing] = useState(false);
  const input = useRef<HTMLInputElement>(null);
  useEffect(() => input.current?.focus(), []);
  const search = (forward: boolean, text = query) => setMissing(text !== "" && !findInActiveSession(text, forward));
  const onKeyDown = (e: ReactKeyboardEvent) => {
    if (e.key === "Enter") {
      e.preventDefault();
      search(!e.shiftKey);
    } else if (e.key === "Escape") {
      e.preventDefault();
      closeFind();
    }
  };
  return (
    <div className="tm-find" data-missing={missing ? "true" : undefined} role="search">
      <input
        ref={input}
        className="oc-input"
        placeholder="Find in terminal"
        aria-label="Find in terminal"
        value={query}
        onChange={(e) => {
          setQuery(e.target.value);
          setMissing(false);
        }}
        onKeyDown={onKeyDown}
      />
      <button className="oc-iconbtn" title="Previous match (Shift+Enter)" aria-label="Previous match" onClick={() => search(false)}>
        <ChevronUp size={14} />
      </button>
      <button className="oc-iconbtn" title="Next match (Enter)" aria-label="Next match" onClick={() => search(true)}>
        <ChevronDown size={14} />
      </button>
      <button className="oc-iconbtn" title="Close (Escape)" aria-label="Close find" onClick={closeFind}>
        <X size={14} />
      </button>
    </div>
  );
}

export function TerminalPanel({ visible }: { visible: boolean }) {
  const hasWorkspace = useApp((s) => s.workspace !== null);
  const sessions = useTerminals((s) => s.sessions);
  const activeId = useTerminals((s) => s.activeId);
  const creating = useTerminals((s) => s.creating);
  const findOpen = useTerminals((s) => s.findOpen);
  const shown = visible && hasWorkspace;

  // Terminal pertama dibuat saat panel pertama kali terlihat; terminal tetap hidup saat panel disembunyikan.
  useEffect(() => {
    setPanelVisible(shown);
    if (shown) ensureSession();
  }, [shown]);
  useEffect(() => () => setPanelVisible(false), []);

  if (!hasWorkspace) return <div className="oc-empty">Open a folder to use the terminal</div>;
  return (
    <div className="tm-root">
      <div className="tm-main">
        {/* xterm dipasang langsung ke elemen ini oleh adapter; React tidak merender anak di dalamnya. */}
        <div className="tm-stage" ref={setStage} />
        {sessions.length === 0 && (
          <div className="tm-overlay">
            {creating ? (
              <span>Starting terminal…</span>
            ) : (
              <button className="oc-btn" onClick={() => void createSession()}>
                <Plus size={14} /> New Terminal
              </button>
            )}
          </div>
        )}
        {findOpen && sessions.length > 0 && <FindBar key={activeId} />}
      </div>
      {sessions.length > 1 && <SessionList sessions={sessions} activeId={activeId} />}
    </div>
  );
}

/** Aksi di bilah tab panel: terminal baru dan matikan terminal aktif. */
export function TerminalActions() {
  const activeId = useTerminals((s) => s.activeId);
  const hasWorkspace = useApp((s) => s.workspace !== null);
  return (
    <>
      <button className="oc-iconbtn" title="New Terminal" aria-label="New Terminal" disabled={!hasWorkspace} onClick={() => void createSession()}>
        <Plus size={15} />
      </button>
      <button
        className="oc-iconbtn"
        title="Kill Terminal"
        aria-label="Kill Terminal"
        disabled={activeId === null}
        onClick={() => activeId !== null && closeSession(activeId)}
      >
        <Trash2 size={14} />
      </button>
    </>
  );
}
