// Tab berkas: satu EditorView per tab, semuanya berbagi dokumen lewat buffer.
import { useEffect, useRef, useState } from "react";
import { Compartment, EditorSelection, EditorState, type Extension } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { FileWarning } from "lucide-react";
import type { TabProps } from "@/app/registry";
import { useApp } from "@/app/store";
import { LARGE_FILE_BYTES, ensureBuffer, getBuffer, reloadBuffer, releaseBuffer, retainBuffer, saveBuffer, useBuffers, type Buffer } from "./buffers";
import { editorExtensionProviders } from "./extensions";
import { languageId, loadLanguage } from "./languages";
import { baseExtensions, createCompartments, settingsExtensions, type EditorCompartments } from "./setup";
import { bufferSync } from "./sync";
import { takePendingReveal, type RevealTarget } from "@/app/coreCommands";
import { showError } from "@/app/overlay";

/** Editor yang sedang fokus (atau terakhir fokus), untuk status bar dan command. */
let activeView: EditorView | null = null;
const viewPaths = new WeakMap<EditorView, string>();
const cursorListeners = new Set<() => void>();

export function activeEditor(): { view: EditorView; path: string } | null {
  if (!activeView || !activeView.dom.isConnected) return null;
  const path = viewPaths.get(activeView);
  return path ? { view: activeView, path } : null;
}

export function onActiveEditorChanged(listener: () => void): () => void {
  cursorListeners.add(listener);
  return () => cursorListeners.delete(listener);
}

function notifyCursor() {
  cursorListeners.forEach((l) => l());
}

/** Mencatat editor yang fokus dan memberi tahu status bar saat kursor bergerak. */
export function trackActiveEditor(path: string): Extension {
  return EditorView.updateListener.of((update) => {
    viewPaths.set(update.view, path);
    if (update.focusChanged && update.view.hasFocus) activeView = update.view;
    if (update.selectionSet || update.focusChanged || update.docChanged) notifyCursor();
  });
}

function createView(parent: HTMLElement, buffer: Buffer, compartments: EditorCompartments, extra: Compartment[]): EditorView {
  const large = buffer.size > LARGE_FILE_BYTES;
  const settings = useApp.getState().settings?.editor;
  const view = new EditorView({
    parent,
    state: EditorState.create({
      doc: buffer.doc,
      extensions: [
        baseExtensions(compartments, settings, { large, readOnly: buffer.readOnly }),
        extra.map((c) => c.of([])),
        bufferSync(buffer),
        trackActiveEditor(buffer.path),
      ],
    }),
  });
  viewPaths.set(view, buffer.path);
  return view;
}

export function FileTab({ tab, visible, focused }: TabProps) {
  const path = tab.params.path as string;
  const host = useRef<HTMLDivElement>(null);
  const viewRef = useRef<EditorView | null>(null);
  const compartmentsRef = useRef<EditorCompartments | null>(null);
  const meta = useBuffers((s) => s.meta[path]);
  const editorSettings = useApp((s) => s.settings?.editor);
  const [ready, setReady] = useState(false);

  // Buffer hidup selama ada tab yang menampilkannya.
  useEffect(() => {
    retainBuffer(path);
    void ensureBuffer(path);
    return () => releaseBuffer(path);
  }, [path]);

  const status = meta?.status ?? "loading";
  useEffect(() => {
    if (status !== "ready" || !host.current) return;
    const buffer = getBuffer(path);
    if (!buffer) return;
    const compartments = createCompartments();
    const providers = editorExtensionProviders();
    const extra = providers.map(() => new Compartment());
    const view = createView(host.current, buffer, compartments, extra);
    viewRef.current = view;
    compartmentsRef.current = compartments;
    setReady(true);

    let disposed = false;
    const large = buffer.size > LARGE_FILE_BYTES;
    if (!large) {
      void loadLanguage(path).then((support) => {
        if (!disposed && support) view.dispatch({ effects: compartments.language.reconfigure(support) });
      });
    }
    const context = { path, languageId: languageId(path), large };
    providers.forEach((provider, i) => {
      void Promise.resolve()
        .then(() => provider(context))
        .then((extension: Extension | null) => {
          if (!disposed && extension) view.dispatch({ effects: extra[i].reconfigure(extension) });
        })
        .catch((e) => console.warn("Ekstensi editor gagal dimuat", e));
    });

    return () => {
      disposed = true;
      if (activeView === view) activeView = null;
      view.destroy();
      viewRef.current = null;
      setReady(false);
      notifyCursor();
    };
  }, [path, status]);

  // Pengaturan editor diterapkan tanpa membuat ulang editor.
  useEffect(() => {
    const view = viewRef.current;
    const compartments = compartmentsRef.current;
    if (view && compartments) view.dispatch({ effects: compartments.settings.reconfigure(settingsExtensions(editorSettings)) });
  }, [editorSettings, ready]);

  useEffect(() => {
    const view = viewRef.current;
    if (!view || !focused) return;
    activeView = view;
    // Fokus hanya bila tidak ada kolom isian lain yang sedang dipakai.
    const current = document.activeElement;
    if (!current || current === document.body || current.closest(".oc-tab, .oc-tabs")) view.focus();
    notifyCursor();
  }, [focused, ready]);

  useEffect(() => {
    if (visible) viewRef.current?.requestMeasure();
  }, [visible]);

  // Lompat ke baris tertentu (hasil pencarian, diagnostik, go to definition).
  useEffect(() => {
    const reveal = (detail: RevealTarget) => {
      const view = viewRef.current;
      if (!view) return false;
      const line = view.state.doc.line(Math.min(Math.max(detail.line, 1), view.state.doc.lines));
      const anchor = Math.min(line.from + Math.max(detail.column, 0), line.to);
      const head = Math.min(anchor + (detail.length ?? 0), line.to);
      view.dispatch({ selection: EditorSelection.single(anchor, head), effects: EditorView.scrollIntoView(anchor, { y: "center" }) });
      view.focus();
      return true;
    };
    // Posisi yang diminta sebelum editor ini siap.
    if (ready && visible) {
      const pending = takePendingReveal(path);
      if (pending) reveal(pending);
    }
    const onReveal = (e: Event) => {
      const detail = (e as CustomEvent<RevealTarget>).detail;
      if (detail.path !== path || !visible) return;
      if (reveal(detail)) takePendingReveal(path);
    };
    window.addEventListener("oncode:reveal", onReveal);
    return () => window.removeEventListener("oncode:reveal", onReveal);
  }, [path, visible, ready]);

  if (status === "loading") return <div className="oc-empty">Loading…</div>;
  if (status === "error") {
    return (
      <div className="oc-empty">
        <div>
          <FileWarning size={28} />
          <div className="oc-selectable" style={{ marginTop: 8 }}>{meta?.error ?? "The file could not be opened."}</div>
          <button className="oc-btn" style={{ marginTop: 12 }} onClick={() => void reloadBuffer(path)}>Retry</button>
        </div>
      </div>
    );
  }
  if (status === "binary" || status === "tooLarge") {
    return (
      <div className="oc-empty">
        <div>
          <FileWarning size={28} />
          <div style={{ marginTop: 8 }}>{status === "binary" ? "This is a binary file and cannot be shown as text." : "This file is too large to open (limit: 50 MB)."}</div>
        </div>
      </div>
    );
  }
  return (
    <>
      {meta?.conflict && (
        <div className="oc-banner" role="alert">
          <span>This file changed on disk while you have unsaved changes.</span>
          <div className="oc-banner-actions">
            <button className="oc-btn oc-btn-sm" onClick={() => void reloadBuffer(path)}>Use disk version</button>
            <button className="oc-btn oc-btn-sm" onClick={() => void saveBuffer(path).catch(showError)}>Overwrite with mine</button>
          </div>
        </div>
      )}
      <div ref={host} className="oc-grow" style={{ overflow: "hidden" }} />
    </>
  );
}
