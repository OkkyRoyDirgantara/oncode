// Ekstensi editor milik Git: penanda perubahan terhadap HEAD di gutter, dan blame (gutter serta anotasi di baris kursor).
import { Chunk } from "@codemirror/merge";
import { Compartment, RangeSetBuilder, StateEffect, StateField, Text, type EditorState, type Extension } from "@codemirror/state";
import { Decoration, EditorView, GutterMarker, ViewPlugin, WidgetType, gutter, type DecorationSet, type ViewUpdate } from "@codemirror/view";
import { showMenu } from "@/app/overlay";
import { executeCommand } from "@/app/commands";
import { useApp } from "@/app/store";
import { commands, type BlameRange } from "@/ipc";
import type { EditorContext } from "../editor/extensions";
import { useScm } from "./store";

/** Repositori yang memuat `path` (yang akarnya paling dalam) dan path relatifnya. */
export function repoForPath(path: string, roots: string[]): { root: string; relative: string } | null {
  const normalized = path.replace(/\\/g, "/");
  let best: string | null = null;
  for (const root of roots) {
    const r = root.replace(/\\/g, "/").replace(/\/+$/, "");
    if ((normalized === r || normalized.startsWith(r + "/")) && (!best || r.length > best.replace(/\\/g, "/").length)) best = root;
  }
  if (!best) return null;
  const base = best.replace(/\\/g, "/").replace(/\/+$/, "");
  return { root: best, relative: normalized.slice(base.length + 1) };
}

// ----- Penanda perubahan -----

const setOriginal = StateEffect.define<Text | null>();

interface ChangeState {
  original: Text | null;
  chunks: readonly Chunk[];
}

const changeField = StateField.define<ChangeState>({
  create: () => ({ original: null, chunks: [] }),
  update(value, tr) {
    for (const effect of tr.effects) {
      if (effect.is(setOriginal)) return { original: effect.value, chunks: effect.value ? Chunk.build(effect.value, tr.state.doc) : [] };
    }
    if (tr.docChanged && value.original) return { original: value.original, chunks: Chunk.updateB(value.chunks, value.original, tr.state.doc, tr.changes) };
    return value;
  },
});

class ChangeMarker extends GutterMarker {
  constructor(readonly kind: "added" | "modified" | "deleted") {
    super();
  }
  eq(other: ChangeMarker) {
    return other.kind === this.kind;
  }
  toDOM() {
    const el = document.createElement("span");
    el.className = `git-mark git-mark-${this.kind}`;
    return el;
  }
}

const MARKERS = { added: new ChangeMarker("added"), modified: new ChangeMarker("modified"), deleted: new ChangeMarker("deleted") };

/** Jenis penanda per baris (nomor baris mulai dari 1) untuk sekumpulan chunk. */
export function changeMarks(chunks: readonly Chunk[], doc: Text): Map<number, "added" | "modified" | "deleted"> {
  const marks = new Map<number, "added" | "modified" | "deleted">();
  for (const chunk of chunks) {
    if (chunk.fromB >= chunk.toB) {
      // Penghapusan murni: ditandai pada baris tempat isi yang hilang berada.
      const line = doc.lineAt(Math.min(chunk.fromB, doc.length)).number;
      if (!marks.has(line)) marks.set(line, "deleted");
      continue;
    }
    const kind = chunk.fromA >= chunk.toA ? "added" : "modified";
    const first = doc.lineAt(chunk.fromB).number;
    const last = doc.lineAt(Math.min(chunk.toB - 1, doc.length)).number;
    for (let line = first; line <= last; line++) marks.set(line, kind);
  }
  return marks;
}

const changeGutter = gutter({
  class: "cm-gitGutter",
  markers(view) {
    const { chunks } = view.state.field(changeField);
    const builder = new RangeSetBuilder<GutterMarker>();
    const marks = changeMarks(chunks, view.state.doc);
    for (const line of [...marks.keys()].sort((a, b) => a - b)) builder.add(view.state.doc.line(line).from, view.state.doc.line(line).from, MARKERS[marks.get(line)!]);
    return builder.finish();
  },
  lineMarkerChange: (update) => update.startState.field(changeField) !== update.state.field(changeField),
});

// ----- Blame -----

interface BlameState {
  ranges: BlameRange[] | null;
  gutter: boolean;
}

const setBlame = StateEffect.define<BlameState>();

const blameField = StateField.define<BlameState>({
  create: () => ({ ranges: null, gutter: false }),
  update(value, tr) {
    for (const effect of tr.effects) if (effect.is(setBlame)) return effect.value;
    // Nomor baris bergeser setelah suntingan; data blame tidak lagi berlaku.
    if (tr.docChanged && value.ranges) return { ranges: null, gutter: value.gutter };
    return value;
  },
});

function blameAt(ranges: BlameRange[], line: number): BlameRange | undefined {
  return ranges.find((r) => line >= r.startLine && line < r.startLine + r.lineCount);
}

export function formatBlameDate(seconds: number, now = Date.now()): string {
  const days = Math.floor((now / 1000 - seconds) / 86400);
  if (days < 1) return "today";
  if (days < 2) return "yesterday";
  if (days < 30) return `${days} days ago`;
  if (days < 365) return `${Math.floor(days / 30)} mo ago`;
  return `${Math.floor(days / 365)} yr ago`;
}

class BlameMarker extends GutterMarker {
  constructor(
    readonly blame: BlameRange,
    readonly first: boolean,
  ) {
    super();
  }
  eq(other: BlameMarker) {
    return other.blame === this.blame && other.first === this.first;
  }
  toDOM() {
    const el = document.createElement("span");
    const r = this.blame;
    el.textContent = r.uncommitted ? "Not committed yet" : `${r.author} · ${formatBlameDate(r.time)}`;
    el.title = r.uncommitted ? "Not committed yet" : `${r.oid.slice(0, 8)} ${r.summary}\n${r.author} <${r.authorEmail}>\n${new Date(r.time * 1000).toLocaleString()}`;
    if (!this.first) el.className = "blame-continued";
    return el;
  }
}

function blameGutter(context: { root: string; relative: string }): Extension {
  return gutter({
    class: "cm-blameGutter",
    markers(view) {
      const state = view.state.field(blameField);
      const builder = new RangeSetBuilder<GutterMarker>();
      if (!state.gutter || !state.ranges) return builder.finish();
      for (const range of state.ranges) {
        for (let i = 0; i < range.lineCount; i++) {
          const number = range.startLine + i;
          if (number > view.state.doc.lines) break;
          const from = view.state.doc.line(number).from;
          builder.add(from, from, new BlameMarker(range, i === 0));
        }
      }
      return builder.finish();
    },
    lineMarkerChange: (update) => update.startState.field(blameField) !== update.state.field(blameField),
    domEventHandlers: {
      click(view, line, event) {
        const ranges = view.state.field(blameField).ranges;
        const range = ranges && blameAt(ranges, view.state.doc.lineAt(line.from).number);
        if (!range || range.uncommitted) return false;
        showMenu(event as MouseEvent, [
          { label: `${range.oid.slice(0, 8)}  ${range.summary}`, disabled: true },
          { separator: true },
          { label: "Copy Commit Hash", run: () => navigator.clipboard.writeText(range.oid) },
          { label: "Show File History", run: () => executeCommand("git.fileHistory", context.root, context.relative) },
        ]);
        return true;
      },
    },
  });
}

class InlineBlame extends WidgetType {
  constructor(readonly text: string) {
    super();
  }
  eq(other: InlineBlame) {
    return other.text === this.text;
  }
  toDOM() {
    const el = document.createElement("span");
    el.className = "cm-blame-inline";
    el.textContent = this.text;
    return el;
  }
  ignoreEvent() {
    return true;
  }
}

function inlineBlame(state: EditorState): DecorationSet {
  const blame = state.field(blameField);
  if (!blame.ranges || !(useApp.getState().settings?.editor.inlineBlame ?? false)) return Decoration.none;
  const line = state.doc.lineAt(state.selection.main.head);
  const range = blameAt(blame.ranges, line.number);
  if (!range) return Decoration.none;
  const text = range.uncommitted ? "Not committed yet" : `${range.author}, ${formatBlameDate(range.time)} • ${range.summary}`;
  return Decoration.set([Decoration.widget({ widget: new InlineBlame(text), side: 1 }).range(line.to)]);
}

async function fetchBlame(view: EditorView, context: { root: string; relative: string }, showGutter: boolean): Promise<void> {
  const before = view.state.doc;
  const ranges = await commands.gitBlame(context.root, context.relative, before.toString());
  // Abaikan hasil bila dokumen sudah berubah selagi menunggu.
  if (view.state.doc !== before || !view.dom.isConnected) return;
  view.dispatch({ effects: setBlame.of({ ranges, gutter: showGutter }) });
}

/** Konteks repositori dan kompartemen gutter blame milik tiap editor (gutter hanya dipasang saat blame menyala). */
const blameContexts = new WeakMap<EditorView, { root: string; relative: string; gutter: Compartment }>();

/** Menyalakan atau mematikan gutter blame pada sebuah editor. Mengembalikan false bila berkas bukan bagian repositori. */
export async function toggleBlame(view: EditorView): Promise<boolean> {
  const context = blameContexts.get(view);
  if (!context) return false;
  const current = view.state.field(blameField, false);
  if (current?.gutter) {
    view.dispatch({ effects: [setBlame.of({ ranges: current.ranges, gutter: false }), context.gutter.reconfigure([])] });
    return true;
  }
  view.dispatch({ effects: context.gutter.reconfigure(blameGutter(context)) });
  await fetchBlame(view, context, true);
  return true;
}

/** Ekstensi Git untuk sebuah editor berkas, atau null bila berkas di luar repositori. */
export function gitEditorExtension(editor: EditorContext): Extension | null {
  const found = repoForPath(editor.path, Object.keys(useScm.getState().repos));
  if (!found || editor.large) return null;
  const context = found;
  const gutterCompartment = new Compartment();

  const sync = ViewPlugin.fromClass(
    class {
      private head: string | null | undefined;
      private indexKey = "";
      private unsubscribe: () => void;
      private timer: ReturnType<typeof setTimeout> | undefined;
      private onSaved = (e: Event) => {
        if ((e as CustomEvent<{ path: string }>).detail.path === editor.path) this.scheduleBlame();
      };

      constructor(readonly view: EditorView) {
        blameContexts.set(view, { ...context, gutter: gutterCompartment });
        void this.loadOriginal();
        this.unsubscribe = useScm.subscribe((state) => {
          const snapshot = state.repos[context.root]?.snapshot;
          if (!snapshot) return;
          // Isi HEAD berubah setelah commit, checkout, reset, dan sejenisnya.
          if (snapshot.headOid !== this.head) void this.loadOriginal();
        });
        window.addEventListener("oncode:saved", this.onSaved);
        this.scheduleBlame();
      }

      private async loadOriginal() {
        this.head = useScm.getState().repos[context.root]?.snapshot?.headOid;
        const text = await commands.gitFileAt(context.root, "HEAD", context.relative).catch(() => null);
        if (!this.view.dom.isConnected) return;
        // Berkas yang belum ada di HEAD tidak diberi penanda (semuanya baris baru).
        this.view.dispatch({ effects: setOriginal.of(text === null ? null : Text.of(text.replace(/\r\n?/g, "\n").split("\n"))) });
      }

      /** Blame diambil ulang saat dokumen tenang, bila gutter menyala atau anotasi baris kursor aktif. */
      private scheduleBlame() {
        clearTimeout(this.timer);
        const gutterOn = this.view.state.field(blameField).gutter;
        if (!gutterOn && !(useApp.getState().settings?.editor.inlineBlame ?? false)) return;
        this.timer = setTimeout(() => {
          const blame = this.view.state.field(blameField);
          if (!blame.ranges) void fetchBlame(this.view, context, blame.gutter).catch(() => {});
        }, 700);
      }

      update(update: ViewUpdate) {
        if (update.docChanged) this.scheduleBlame();
        const key = String(useApp.getState().settings?.editor.inlineBlame ?? false);
        if (key !== this.indexKey) {
          this.indexKey = key;
          this.scheduleBlame();
        }
      }

      destroy() {
        clearTimeout(this.timer);
        this.unsubscribe();
        window.removeEventListener("oncode:saved", this.onSaved);
      }
    },
  );

  const inline = EditorView.decorations.compute([blameField, "selection"], inlineBlame);
  return [changeField, changeGutter, blameField, gutterCompartment.of([]), inline, sync];
}
