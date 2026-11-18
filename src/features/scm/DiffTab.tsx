// Tab diff: dua versi berkas berdampingan (atau menyatu), dengan stage/unstage/discard per hunk dan per baris.
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Chunk, MergeView, getChunks, goToNextChunk, goToPreviousChunk, unifiedMergeView } from "@codemirror/merge";
import { Compartment, EditorState, Text, type Extension } from "@codemirror/state";
import { Decoration, EditorView, ViewPlugin, WidgetType, lineNumbers, type DecorationSet, type ViewUpdate } from "@codemirror/view";
import { ArrowDown, ArrowUp, Columns2, FileText, Minus, Plus, Rows2, Undo2 } from "lucide-react";
import { confirm, showError, toast } from "@/app/overlay";
import type { TabProps } from "@/app/registry";
import { useApp } from "@/app/store";
import { commands, errorCode, type DiffContents, type DiffSource, type SpliceOp, type SpliceTarget } from "@/ipc";
import { ensureBuffer, getBuffer, isDirty, releaseBuffer, retainBuffer, saveBuffer, type Buffer } from "../editor/buffers";
import { trackActiveEditor } from "../editor/CodeEditor";
import { loadLanguage } from "../editor/languages";
import { baseExtensions, createCompartments } from "../editor/setup";
import { bufferSync } from "../editor/sync";
import { oncodeTheme } from "../editor/theme";
import * as git from "./actions";
import { chunkToOp, selectionOps } from "./lines";
import { openRepoFile, repoFile, sourceLabel, type DiffParams } from "./open";
import { useScm } from "./store";

type Mode = "split" | "unified";
const MODE_KEY = "oncode.diff.mode";

interface ChunkAction {
  label: string;
  title: string;
  target: SpliceTarget;
  danger?: boolean;
}

function actionsFor(source: DiffSource): ChunkAction[] {
  if (source.kind === "worktree") {
    return [
      { label: "Stage", title: "Stage this change", target: "stage" },
      { label: "Discard", title: "Discard this change from the working tree", target: "discard", danger: true },
    ];
  }
  if (source.kind === "staged") return [{ label: "Unstage", title: "Unstage this change", target: "unstage" }];
  return [];
}

class ChunkWidget extends WidgetType {
  constructor(
    readonly index: number,
    readonly actions: ChunkAction[],
    readonly run: (index: number, target: SpliceTarget) => void,
  ) {
    super();
  }
  eq(other: ChunkWidget) {
    return other.index === this.index && other.actions === this.actions;
  }
  toDOM() {
    const box = document.createElement("span");
    box.className = "diff-actions";
    for (const action of this.actions) {
      const button = document.createElement("button");
      button.textContent = action.label;
      button.title = action.title;
      if (action.danger) button.dataset.danger = "true";
      button.onmousedown = (e) => e.preventDefault();
      button.onclick = (e) => {
        e.preventDefault();
        this.run(this.index, action.target);
      };
      box.appendChild(button);
    }
    return box;
  }
  ignoreEvent() {
    return true;
  }
}

/** Tombol aksi di awal tiap chunk pada sisi B. */
function chunkActions(actions: ChunkAction[], run: (index: number, target: SpliceTarget) => void): Extension {
  if (actions.length === 0) return [];
  const build = (state: EditorState): DecorationSet => {
    const chunks = getChunks(state)?.chunks ?? [];
    const widgets = chunks.map((chunk, i) => {
      const pos = state.doc.lineAt(Math.min(chunk.fromB, state.doc.length)).from;
      return Decoration.widget({ widget: new ChunkWidget(i, actions, run), side: -1 }).range(pos);
    });
    return Decoration.set(widgets, true);
  };
  return ViewPlugin.fromClass(
    class {
      decorations: DecorationSet;
      constructor(view: EditorView) {
        this.decorations = build(view.state);
      }
      update(update: ViewUpdate) {
        if (update.docChanged || getChunks(update.state) !== getChunks(update.startState)) this.decorations = build(update.state);
      }
    },
    { decorations: (plugin) => plugin.decorations },
  );
}

const readOnly: Extension = [EditorState.readOnly.of(true), EditorView.editable.of(false)];

function plainExtensions(language: Compartment): Extension {
  return [lineNumbers(), language.of([]), oncodeTheme, readOnly, EditorView.contentAttributes.of({ tabindex: "0" })];
}

function toText(text: string | null | undefined): Text {
  return Text.of((text ?? "").split("\n"));
}

function unavailable(contents: DiffContents): string | null {
  for (const side of [contents.a, contents.b]) {
    if (side.binary) return "This is a binary file; its content cannot be compared as text.";
    if (side.tooLarge) return "This file is too large to compare here.";
  }
  return null;
}

export function DiffTab({ tab, visible }: TabProps) {
  const { root, path, origPath, source } = tab.params as unknown as DiffParams;
  const [contents, setContents] = useState<DiffContents | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [mode, setMode] = useState<Mode>(() => (localStorage.getItem(MODE_KEY) === "unified" ? "unified" : "split"));
  const [chunkCount, setChunkCount] = useState(0);
  const host = useRef<HTMLDivElement>(null);
  const editors = useRef<{ a: () => Text; b: EditorView; dispose: () => void } | null>(null);
  const contentsRef = useRef<DiffContents | null>(null);
  contentsRef.current = contents;

  const snapshot = useScm((s) => s.repos[root]?.snapshot);
  const absolute = useMemo(() => repoFile(root, path), [root, path]);
  const actions = useMemo(() => actionsFor(source), [source]);
  // Sisi kanan diff worktree adalah buffer berkas itu sendiri, sehingga bisa disunting langsung.
  const usesBuffer = source.kind === "worktree";

  const load = useCallback(async () => {
    try {
      const next = await commands.gitDiff(root, path, origPath, source);
      setError(null);
      setContents((previous) => (previous && previous.a.fingerprint === next.a.fingerprint && previous.b.fingerprint === next.b.fingerprint ? previous : next));
    } catch (e) {
      setError(errorCode(e) === "notARepo" ? "The repository is no longer open." : String((e as { message?: string }).message ?? e));
    }
  }, [root, path, origPath, source]);

  // Muat saat dibuka, dan muat ulang setiap kali keadaan repositori berubah.
  useEffect(() => {
    void load();
  }, [load, snapshot]);

  useEffect(() => {
    if (!usesBuffer) return;
    retainBuffer(absolute);
    return () => releaseBuffer(absolute);
  }, [usesBuffer, absolute]);

  const applyOps = useCallback(
    async (target: SpliceTarget, pick: (chunks: readonly Chunk[], a: Text, b: Text) => SpliceOp[]) => {
      const current = editors.current;
      if (!current) return;
      try {
        if (usesBuffer && isDirty(absolute)) await saveBuffer(absolute);
        // Sidik isi terbaru; backend menolak bila berkas sudah berubah sejak diff ini.
        const fresh = await commands.gitDiff(root, path, origPath, source);
        const a = current.a();
        const b = current.b.state.doc;
        if ((fresh.a.text ?? "") !== a.toString() || (fresh.b.text ?? "") !== b.toString()) {
          setContents(fresh);
          toast.warning("The file changed since this diff was shown. The diff was refreshed; try again.");
          return;
        }
        const ops = pick(Chunk.build(a, b), a, b);
        if (ops.length === 0) {
          toast.info("The selection does not contain any changes.");
          return;
        }
        if (target === "discard") {
          const ok = await confirm({
            title: "Discard this change?",
            message: `The selected change in ${path} is removed from the working tree. This cannot be undone.`,
            confirmLabel: "Discard",
            danger: true,
          });
          if (!ok) return;
        }
        await commands.gitSplice(root, target, path, ops, fresh.a.fingerprint, fresh.b.fingerprint);
      } catch (e) {
        git.handleGitError(root, e, target === "stage" ? "Stage" : target === "unstage" ? "Unstage" : "Discard");
        void load();
      }
    },
    [root, path, origPath, source, usesBuffer, absolute, load],
  );
  const applyRef = useRef(applyOps);
  applyRef.current = applyOps;

  const problem = contents ? unavailable(contents) : null;
  const viewKey = contents && !problem ? `${mode}:${contents.a.fingerprint}:${usesBuffer && contents.b.exists ? "buffer" : contents.b.fingerprint}` : null;

  useEffect(() => {
    const parent = host.current;
    const data = contentsRef.current;
    if (!viewKey || !parent || !data) return;
    let disposed = false;
    let cleanup: (() => void) | null = null;

    const build = (buffer: Buffer | null) => {
      if (disposed) return;
      const settings = useApp.getState().settings?.editor;
      const langA = new Compartment();
      const langB = new Compartment();
      const aDoc = toText(data.a.text);
      const runChunk = (index: number, target: SpliceTarget) =>
        void applyRef.current(target, (chunks, a, b) => (chunks[index] ? [chunkToOp(chunks[index], a, b)] : []));
      const compartments = createCompartments();
      const bExtensions: Extension = buffer
        ? [baseExtensions(compartments, settings, { large: false, readOnly: buffer.readOnly }), bufferSync(buffer), trackActiveEditor(buffer.path)]
        : plainExtensions(langB);
      const bLanguage = buffer ? compartments.language : langB;
      const bDoc = buffer ? buffer.doc : toText(data.b.text);
      const countChunks = EditorView.updateListener.of((u) => setChunkCount(getChunks(u.state)?.chunks.length ?? 0));
      const collapse = { margin: 3, minSize: 8 };

      let a: () => Text;
      let b: EditorView;
      let destroy: () => void;
      let scroller: HTMLElement;
      if (mode === "split") {
        const merge = new MergeView({
          parent,
          a: { doc: aDoc, extensions: plainExtensions(langA) },
          b: { doc: bDoc, extensions: [bExtensions, chunkActions(actions, runChunk), countChunks] },
          collapseUnchanged: collapse,
          gutter: true,
          highlightChanges: true,
        });
        a = () => merge.a.state.doc;
        b = merge.b;
        destroy = () => merge.destroy();
        scroller = merge.dom;
        void loadLanguage(path).then((support) => {
          if (disposed || !support) return;
          merge.a.dispatch({ effects: langA.reconfigure(support) });
          merge.b.dispatch({ effects: bLanguage.reconfigure(support) });
        });
      } else {
        const view = new EditorView({
          parent,
          state: EditorState.create({
            doc: bDoc,
            extensions: [
              bExtensions,
              unifiedMergeView({ original: aDoc, mergeControls: false, collapseUnchanged: collapse, gutter: true, highlightChanges: true }),
              chunkActions(actions, runChunk),
              countChunks,
            ],
          }),
        });
        a = () => aDoc;
        b = view;
        destroy = () => view.destroy();
        scroller = view.scrollDOM;
        void loadLanguage(path).then((support) => {
          if (!disposed && support) view.dispatch({ effects: bLanguage.reconfigure(support) });
        });
      }
      setChunkCount(getChunks(b.state)?.chunks.length ?? 0);
      // Posisi gulir dipertahankan saat tampilan dibangun ulang (mis. setelah satu hunk di-stage).
      const saved = scrollPositions.get(tab.id);
      if (saved !== undefined) requestAnimationFrame(() => (scroller.scrollTop = saved));
      const dispose = () => {
        scrollPositions.set(tab.id, scroller.scrollTop);
        destroy();
      };
      editors.current = { a, b, dispose };
      cleanup = dispose;
    };

    if (usesBuffer && data.b.exists) {
      void ensureBuffer(absolute).then((buffer) => build(buffer.status === "ready" ? buffer : null));
    } else {
      build(null);
    }
    return () => {
      disposed = true;
      cleanup?.();
      editors.current = null;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [viewKey]);

  useEffect(() => {
    if (visible) editors.current?.b.requestMeasure();
  }, [visible]);

  const changeMode = (next: Mode) => {
    localStorage.setItem(MODE_KEY, next);
    setMode(next);
  };

  const navigate = (direction: 1 | -1) => {
    const view = editors.current?.b;
    if (!view) return;
    (direction === 1 ? goToNextChunk : goToPreviousChunk)({ state: view.state, dispatch: (tr) => view.dispatch(tr) });
    view.focus();
  };

  const applySelection = (target: SpliceTarget) => {
    const view = editors.current?.b;
    if (!view) return;
    const range = view.state.selection.main;
    const from = view.state.doc.lineAt(range.from).number - 1;
    const toLine = view.state.doc.lineAt(range.to);
    // Pilihan yang berakhir di awal baris tidak mencakup baris itu.
    const to = range.empty || range.to > toLine.from ? toLine.number : toLine.number - 1;
    void applyOps(target, (chunks, a, b) => selectionOps(chunks, a, b, from, Math.max(to, from)));
  };

  const fileAction = async (target: SpliceTarget) => {
    const paths = origPath ? [path, origPath] : [path];
    if (target === "stage") await git.stage(root, [path]);
    else if (target === "unstage") await git.unstage(root, paths);
    else {
      const snap = useScm.getState().repos[root]?.snapshot;
      const untracked = snap?.untracked.includes(path) ?? false;
      await git.discard(root, untracked ? [] : [path], untracked ? [path] : []);
    }
  };

  if (error) return <div className="oc-empty oc-selectable">{error}</div>;
  if (!contents) return <div className="oc-empty">Loading…</div>;

  const noChanges = !problem && chunkCount === 0 && contents.a.exists === contents.b.exists;
  return (
    <div className="oc-col" style={{ height: "100%" }}>
      <div className="diff-toolbar">
        <span className="oc-ellipsis oc-selectable" title={path}>
          {origPath && origPath !== path ? `${origPath} → ${path}` : path}
        </span>
        <span className="oc-faint">{sourceLabel(source)}</span>
        {!problem && <span className="oc-faint">{chunkCount === 0 ? "no changes" : `${chunkCount} change${chunkCount === 1 ? "" : "s"}`}</span>}
        <div className="oc-grow" />
        {actions.length > 0 && !problem && chunkCount > 0 && (
          <>
            {actions.map((action) => (
              <button key={action.target} className="oc-btn oc-btn-sm" title={`${action.label} only the selected lines (or the change at the cursor)`} onClick={() => applySelection(action.target)}>
                {action.target === "stage" ? <Plus size={13} /> : action.target === "unstage" ? <Minus size={13} /> : <Undo2 size={13} />}
                {action.label} Selected Lines
              </button>
            ))}
          </>
        )}
        {actions.map((action) => (
          <button key={`file-${action.target}`} className="oc-btn oc-btn-sm" onClick={() => void fileAction(action.target).catch(showError)}>
            {action.label} File
          </button>
        ))}
        <button className="oc-iconbtn" title="Previous change (Alt+↑)" aria-label="Previous change" disabled={!!problem || chunkCount === 0} onClick={() => navigate(-1)}>
          <ArrowUp size={15} />
        </button>
        <button className="oc-iconbtn" title="Next change (Alt+↓)" aria-label="Next change" disabled={!!problem || chunkCount === 0} onClick={() => navigate(1)}>
          <ArrowDown size={15} />
        </button>
        <button className="oc-iconbtn" title={mode === "split" ? "Switch to unified view" : "Switch to side-by-side view"} aria-label="Toggle diff layout" onClick={() => changeMode(mode === "split" ? "unified" : "split")}>
          {mode === "split" ? <Rows2 size={15} /> : <Columns2 size={15} />}
        </button>
        {contents.b.exists && source.kind !== "stash" && source.kind !== "range" && (
          <button className="oc-iconbtn" title="Open file" aria-label="Open file" onClick={() => openRepoFile(root, path, { preview: false })}>
            <FileText size={15} />
          </button>
        )}
      </div>
      {!problem && mode === "split" && (
        <div className="diff-labels">
          <div className="oc-ellipsis">{contents.a.exists ? contents.a.label : `${contents.a.label} — file does not exist`}</div>
          <div className="oc-ellipsis">{contents.b.exists ? contents.b.label : `${contents.b.label} — file deleted`}</div>
        </div>
      )}
      {problem ? (
        <div className="diff-binary">
          <div>
            <div>{problem}</div>
            <div className="oc-faint" style={{ marginTop: 6 }}>
              {contents.a.exists ? `${contents.a.label}: ${contents.a.size.toLocaleString()} bytes` : `${contents.a.label}: absent`} ·{" "}
              {contents.b.exists ? `${contents.b.label}: ${contents.b.size.toLocaleString()} bytes` : `${contents.b.label}: absent`}
            </div>
          </div>
        </div>
      ) : (
        <div className="diff-host" ref={host} onKeyDown={(e) => {
          if (e.altKey && (e.key === "ArrowDown" || e.key === "ArrowUp")) {
            e.preventDefault();
            navigate(e.key === "ArrowDown" ? 1 : -1);
          }
        }}>
          {noChanges && <div className="oc-banner" style={{ position: "absolute", right: 12, bottom: 12, zIndex: 3, borderRadius: 8, border: "1px solid var(--border)" }}>Both versions are identical.</div>}
        </div>
      )}
    </div>
  );
}

const scrollPositions = new Map<string, number>();

/** Buffer sisi kanan diff worktree yang berisi perubahan belum disimpan. */
export function diffBufferPath(params: DiffParams): string | null {
  if (params.source.kind !== "worktree") return null;
  const absolute = repoFile(params.root, params.path);
  return getBuffer(absolute) ? absolute : null;
}
