// Fitur editor: jenis tab `file`, penyimpanan, status bar, dan reaksi terhadap perubahan berkas dari luar.
import { useSyncExternalStore } from "react";
import { gotoLine, openSearchPanel } from "@codemirror/search";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { FileText } from "lucide-react";
import { registerCommands } from "@/app/commands";
import { choose, showError, toast } from "@/app/overlay";
import { registerStatusItem, registerTabType } from "@/app/registry";
import { baseName, onWorkspaceChanged, useApp } from "@/app/store";
import { allTabs, forceCloseEverywhere } from "@/app/workbench";
import { events } from "@/ipc";
import { activeEditor, onActiveEditorChanged, FileTab } from "./CodeEditor";
import { dirtyPaths, handleExternalChange, isDirty, openBuffers, saveBuffer, useBuffers } from "./buffers";
import { languageName } from "./languages";

async function save(path: string): Promise<boolean> {
  try {
    return await saveBuffer(path);
  } catch (e) {
    showError(e, `Could not save ${baseName(path)}`);
    return false;
  }
}

async function confirmClose(path: string): Promise<boolean> {
  if (!isDirty(path)) return true;
  const answer = await choose({
    title: "Unsaved changes",
    message: `Save the changes you made to ${baseName(path)}?\n\nYour changes will be lost if you don't save them.`,
    buttons: [
      { id: "cancel", label: "Cancel" },
      { id: "discard", label: "Don't Save", kind: "danger" },
      { id: "save", label: "Save", kind: "primary" },
    ],
  });
  if (answer === "save") return save(path);
  return answer === "discard";
}

function useActiveEditorInfo() {
  return useSyncExternalStore(onActiveEditorChanged, () => {
    const active = activeEditor();
    if (!active) return "";
    const head = active.view.state.selection.main.head;
    const line = active.view.state.doc.lineAt(head);
    const selected = active.view.state.selection.ranges.reduce((n, r) => n + (r.to - r.from), 0);
    return `${active.path}\n${line.number}\n${head - line.from + 1}\n${selected}`;
  });
}

function CursorStatus() {
  const info = useActiveEditorInfo();
  if (!info) return null;
  const [, line, column, selected] = info.split("\n");
  return (
    <span className="oc-status-item">
      Ln {line}, Col {column}
      {selected !== "0" ? ` (${selected} selected)` : ""}
    </span>
  );
}

function FileStatus() {
  const info = useActiveEditorInfo();
  const path = info.split("\n")[0];
  const meta = useBuffers((s) => (path ? s.meta[path] : undefined));
  if (!path || !meta) return null;
  return (
    <>
      <span className="oc-status-item">{meta.encoding}</span>
      <span className="oc-status-item">{meta.eol === "crlf" ? "CRLF" : "LF"}</span>
      <span className="oc-status-item">{languageName(path)}</span>
    </>
  );
}

let autoSaveTimer: ReturnType<typeof setTimeout> | undefined;

function installAutoSave() {
  useBuffers.subscribe(() => {
    if (useApp.getState().settings?.files.autoSave !== "afterDelay") return;
    clearTimeout(autoSaveTimer);
    autoSaveTimer = setTimeout(() => {
      for (const path of dirtyPaths()) if (!useBuffers.getState().meta[path]?.conflict) void save(path);
    }, 1000);
  });
  window.addEventListener("blur", () => {
    if (useApp.getState().settings?.files.autoSave !== "onFocusChange") return;
    for (const path of dirtyPaths()) if (!useBuffers.getState().meta[path]?.conflict) void save(path);
  });
}

function installExternalChangeHandling() {
  void events.fsChanged.listen(async (e) => {
    const open = openBuffers();
    const changed = e.payload.overflow ? open.map((b) => b.path) : e.payload.paths.filter((p) => open.some((b) => b.path === p));
    for (const path of changed) {
      const result = await handleExternalChange(path);
      if (result === "deleted" && !isDirty(path)) {
        forceCloseEverywhere((t) => t.kind === "file" && t.params.path === path);
      } else if (result === "deleted") {
        toast.warning(`${baseName(path)} was deleted on disk. Your unsaved changes are still open; save to restore the file.`);
      }
    }
  });
}

export function registerEditorFeature(): void {
  registerTabType({
    kind: "file",
    component: FileTab,
    icon: () => <FileText size={14} />,
    useDirty: (tab) => useBuffers((s) => s.meta[tab.params.path as string]?.dirty ?? false),
    beforeClose: (tab) => confirmClose(tab.params.path as string),
  });

  registerStatusItem({ id: "editor.cursor", align: "right", order: 10, component: CursorStatus });
  registerStatusItem({ id: "editor.file", align: "right", order: 20, component: FileStatus });

  const hasEditor = () => activeEditor() !== null;
  registerCommands([
    { id: "file.save", title: "Save", category: "File", keybinding: "Mod+S", when: hasEditor, run: () => save(activeEditor()!.path) },
    {
      id: "file.saveAll",
      title: "Save All",
      category: "File",
      keybinding: "Mod+Alt+S",
      run: async () => {
        for (const path of dirtyPaths()) await save(path);
      },
    },
    { id: "editor.find", title: "Find", category: "Edit", when: hasEditor, run: () => openSearchPanel(activeEditor()!.view) },
    { id: "editor.gotoLine", title: "Go to Line…", category: "Edit", keybinding: "Mod+G", when: hasEditor, run: () => gotoLine(activeEditor()!.view) },
    {
      id: "editor.copyPath",
      title: "Copy Path of Active File",
      category: "File",
      when: hasEditor,
      run: () => navigator.clipboard.writeText(activeEditor()!.path),
    },
  ]);

  installAutoSave();
  installExternalChangeHandling();

  // Saat folder ditutup atau diganti, tab berkas dari folder lama sudah ditutup oleh workbench.
  onWorkspaceChanged((ws) => {
    if (!ws && allTabs().length > 0) forceCloseEverywhere(() => true);
  });

  installCloseGuard();
}

/** Menutup jendela dengan perubahan belum disimpan meminta keputusan lebih dulu. */
function installCloseGuard() {
  let appWindow;
  try {
    appWindow = getCurrentWindow();
  } catch {
    // Di luar Tauri (uji unit) tidak ada jendela.
    return;
  }
  void appWindow
    .onCloseRequested(async (event) => {
      const dirty = dirtyPaths();
      if (dirty.length === 0) return;
      event.preventDefault();
      const names = dirty.slice(0, 5).map(baseName).join(", ") + (dirty.length > 5 ? `, and ${dirty.length - 5} more` : "");
      const answer = await choose({
        title: "Unsaved changes",
        message: `${dirty.length === 1 ? "1 file has" : `${dirty.length} files have`} unsaved changes: ${names}.\n\nYour changes will be lost if you don't save them.`,
        buttons: [
          { id: "cancel", label: "Cancel" },
          { id: "discard", label: "Don't Save", kind: "danger" },
          { id: "save", label: "Save All", kind: "primary" },
        ],
      });
      if (answer === "save") {
        for (const path of dirty) if (!(await save(path))) return;
      } else if (answer !== "discard") {
        return;
      }
      await appWindow.destroy();
    })
    .catch(() => {});
}
