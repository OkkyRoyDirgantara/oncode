// Tab editor konflik: Current | Hasil | Incoming. Hanya panel Hasil yang bisa disunting.
import { useEffect, useMemo, useRef, useState } from "react";
import { Compartment, EditorState, type Extension } from "@codemirror/state";
import { EditorView, lineNumbers } from "@codemirror/view";
import { ArrowDown, ArrowUp, Check } from "lucide-react";
import { confirm, showError } from "@/app/overlay";
import type { Tab, TabProps } from "@/app/registry";
import { useApp } from "@/app/store";
import { closeTab } from "@/app/workbench";
import { commands, errorMessage, type ConflictDetail, type ConflictKind } from "@/ipc";
import { loadLanguage } from "../editor/languages";
import { baseExtensions, createCompartments } from "../editor/setup";
import { oncodeTheme } from "../editor/theme";
import * as git from "./actions";
import { buildInitialResult, conflictExtension, conflictField, setActiveConflict, sideHighlight, unresolvedCount } from "./conflictState";
import { useScm } from "./store";

/** Tab konflik yang isinya sudah diubah tetapi belum ditandai selesai. */
const touched = new Set<string>();

export function conflictTabDirty(tab: Tab): boolean {
  return touched.has(tab.id);
}

export async function confirmCloseConflict(tab: Tab): Promise<boolean> {
  if (!touched.has(tab.id)) return true;
  const ok = await confirm({
    title: "Discard conflict resolution?",
    message: "Your choices in this conflict editor have not been applied yet. Closing the tab discards them; the file stays in conflict.",
    confirmLabel: "Discard",
    danger: true,
  });
  if (ok) touched.delete(tab.id);
  return ok;
}

const SIMPLE_TEXT: Record<ConflictKind, string> = {
  bothModified: "Both sides changed this file and it cannot be merged as text.",
  bothAdded: "Both sides added a file with this name.",
  bothDeleted: "Both sides deleted (or renamed away) this file.",
  addedByUs: "This file was added only on the current side.",
  addedByThem: "This file was added only on the incoming side.",
  deletedByUs: "The current side deleted this file, the incoming side changed it.",
  deletedByThem: "The incoming side deleted this file, the current side changed it.",
};

function sidePane(parent: HTMLElement, text: string, ranges: [number, number][], className: string, language: Compartment): EditorView {
  const extensions: Extension = [lineNumbers(), language.of([]), oncodeTheme, EditorState.readOnly.of(true), EditorView.editable.of(false), sideHighlight(ranges, className)];
  return new EditorView({ parent, state: EditorState.create({ doc: text.replace(/\r\n?/g, "\n"), extensions }) });
}

function scrollToOffset(view: EditorView, offset: number) {
  view.dispatch({ effects: EditorView.scrollIntoView(Math.min(offset, view.state.doc.length), { y: "center" }) });
}

export function ConflictTab({ tab, groupId, visible }: TabProps) {
  const root = tab.params.root as string;
  const path = tab.params.path as string;
  const [detail, setDetail] = useState<ConflictDetail | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [unresolved, setUnresolved] = useState(0);
  const [active, setActive] = useState(0);
  const [saving, setSaving] = useState(false);
  const hosts = { current: useRef<HTMLDivElement>(null), result: useRef<HTMLDivElement>(null), incoming: useRef<HTMLDivElement>(null) };
  const views = useRef<{ current: EditorView; result: EditorView; incoming: EditorView } | null>(null);
  const stillConflicted = useScm((s) => s.repos[root]?.snapshot?.conflicted.some((c) => c.path === path) ?? false);

  useEffect(() => {
    let cancelled = false;
    commands
      .gitConflict(root, path)
      .then((d) => !cancelled && setDetail(d))
      .catch((e) => !cancelled && setError(errorMessage(e)));
    return () => {
      cancelled = true;
    };
  }, [root, path]);

  const initial = useMemo(() => (detail && detail.segments.length > 0 ? buildInitialResult(detail.segments) : null), [detail]);
  const close = () => {
    touched.delete(tab.id);
    void closeTab(groupId, tab.id, true);
  };

  useEffect(() => {
    if (!detail || !initial || !hosts.current.current || !hosts.result.current || !hosts.incoming.current) return;
    const settings = useApp.getState().settings?.editor;
    const langCurrent = new Compartment();
    const langIncoming = new Compartment();
    const compartments = createCompartments();
    const current = sidePane(hosts.current.current, detail.current ?? "", initial.currentRanges, "cm-conflict-current", langCurrent);
    const incoming = sidePane(hosts.incoming.current, detail.incoming ?? "", initial.incomingRanges, "cm-conflict-incoming", langIncoming);
    const result = new EditorView({
      parent: hosts.result.current,
      state: EditorState.create({
        doc: initial.text,
        extensions: [
          baseExtensions(compartments, settings, { large: false, readOnly: false }),
          conflictExtension(initial.conflicts),
          EditorView.updateListener.of((update) => {
            if (update.docChanged) touched.add(tab.id);
            if (update.docChanged || update.startState.field(conflictField) !== update.state.field(conflictField)) setUnresolved(unresolvedCount(update.state));
          }),
        ],
      }),
    });
    views.current = { current, result, incoming };
    setUnresolved(unresolvedCount(result.state));
    let disposed = false;
    void loadLanguage(path).then((support) => {
      if (disposed || !support) return;
      current.dispatch({ effects: langCurrent.reconfigure(support) });
      incoming.dispatch({ effects: langIncoming.reconfigure(support) });
      result.dispatch({ effects: compartments.language.reconfigure(support) });
    });
    return () => {
      disposed = true;
      current.destroy();
      incoming.destroy();
      result.destroy();
      views.current = null;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [detail, initial]);

  useEffect(() => {
    if (visible) views.current?.result.requestMeasure();
  }, [visible]);

  const goTo = (index: number) => {
    const v = views.current;
    if (!v || !initial || initial.conflicts.length === 0) return;
    const next = (index + initial.conflicts.length) % initial.conflicts.length;
    setActive(next);
    setActiveConflict(v.result, next);
    scrollToOffset(v.current, initial.currentRanges[next][0]);
    scrollToOffset(v.incoming, initial.incomingRanges[next][0]);
  };

  const markResolved = async () => {
    const v = views.current;
    if (!v) return;
    const left = unresolvedCount(v.result.state);
    if (left > 0) {
      const ok = await confirm({
        title: `${left} conflict${left === 1 ? " is" : "s are"} still unresolved`,
        message: "Unresolved conflicts keep the Current side. Mark the file as resolved anyway?",
        confirmLabel: "Mark as Resolved",
      });
      if (!ok) return;
    }
    // Akhir baris mengikuti berkas aslinya.
    const crlf = /\r\n/.test(detail?.current ?? detail?.incoming ?? "");
    const content = v.result.state.doc.sliceString(0, v.result.state.doc.length, crlf ? "\r\n" : "\n");
    setSaving(true);
    try {
      await commands.gitResolveWrite(root, path, content);
      close();
    } catch (e) {
      git.handleGitError(root, e, "Mark as resolved");
    } finally {
      setSaving(false);
    }
  };

  const resolveSimple = async (action: () => Promise<boolean>) => {
    if (await action().catch((e) => (showError(e), false))) close();
  };

  if (error) return <div className="oc-empty oc-selectable">{error}</div>;
  if (!detail) return <div className="oc-empty">Loading…</div>;

  if (!stillConflicted && !saving) {
    return (
      <div className="conflict-simple">
        <div>
          <div>{path} is no longer in conflict.</div>
          <div className="conflict-simple-actions">
            <button className="oc-btn oc-btn-primary" onClick={close}>Close</button>
          </div>
        </div>
      </div>
    );
  }

  if (!initial) {
    // Konflik non-teks: biner, atau salah satu sisi menghapus berkas.
    const keepCurrent = detail.current !== null || detail.kind === "deletedByThem" || detail.kind === "addedByUs" || (detail.binary && detail.kind !== "deletedByUs");
    const keepIncoming = detail.incoming !== null || detail.kind === "deletedByUs" || detail.kind === "addedByThem" || (detail.binary && detail.kind !== "deletedByThem");
    const currentExists = detail.kind !== "deletedByUs" && detail.kind !== "addedByThem" && detail.kind !== "bothDeleted";
    const incomingExists = detail.kind !== "deletedByThem" && detail.kind !== "addedByUs" && detail.kind !== "bothDeleted";
    return (
      <div className="conflict-simple">
        <div>
          <div style={{ fontWeight: 600 }}>{path}</div>
          <div className="oc-muted" style={{ marginTop: 6 }}>
            {detail.binary ? "This is a binary file. " : ""}
            {SIMPLE_TEXT[detail.kind]}
          </div>
          <div className="conflict-simple-actions">
            {keepCurrent && currentExists && (
              <button className="oc-btn" onClick={() => void resolveSimple(() => git.resolveWithSide(root, path, "current"))}>
                Keep Current ({detail.currentLabel})
              </button>
            )}
            {keepIncoming && incomingExists && (
              <button className="oc-btn" onClick={() => void resolveSimple(() => git.resolveWithSide(root, path, "incoming"))}>
                Keep Incoming ({detail.incomingLabel})
              </button>
            )}
            <button className="oc-btn oc-btn-danger" onClick={() => void resolveSimple(() => git.resolveByDeleting(root, path))}>
              Delete File
            </button>
          </div>
        </div>
      </div>
    );
  }

  const total = initial.conflicts.length;
  return (
    <div className="conflict-root">
      <div className="diff-toolbar">
        <span className="oc-ellipsis oc-selectable" title={path}>{path}</span>
        <span className={unresolved > 0 ? "oc-git-conflict" : "oc-git-added"}>
          {total === 0 ? "No conflicting regions" : unresolved > 0 ? `${unresolved} of ${total} unresolved` : `All ${total} resolved`}
        </span>
        <div className="oc-grow" />
        <button className="oc-iconbtn" title="Previous conflict" aria-label="Previous conflict" disabled={total === 0} onClick={() => goTo(active - 1)}>
          <ArrowUp size={15} />
        </button>
        <button className="oc-iconbtn" title="Next conflict" aria-label="Next conflict" disabled={total === 0} onClick={() => goTo(active + 1)}>
          <ArrowDown size={15} />
        </button>
        <button className="oc-btn oc-btn-sm oc-btn-primary" disabled={saving} onClick={() => void markResolved()}>
          <Check size={14} /> Mark as Resolved
        </button>
      </div>
      <div className="conflict-panes">
        <div className="conflict-pane">
          <div className="conflict-pane-title" data-side="current" title={detail.currentLabel}>Current · {detail.currentLabel}</div>
          <div className="conflict-pane-body" ref={hosts.current} />
        </div>
        <div className="conflict-pane">
          <div className="conflict-pane-title" data-side="result">Result (editable)</div>
          <div className="conflict-pane-body" ref={hosts.result} />
        </div>
        <div className="conflict-pane">
          <div className="conflict-pane-title" data-side="incoming" title={detail.incomingLabel}>Incoming · {detail.incomingLabel}</div>
          <div className="conflict-pane-body" ref={hosts.incoming} />
        </div>
      </div>
    </div>
  );
}
