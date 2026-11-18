// Fitur Explorer: tampilan sidebar pohon berkas, command fokus, dan reaksi terhadap perubahan dari luar.
import { Files } from "lucide-react";
import { registerCommands } from "@/app/commands";
import { registerSidebarView } from "@/app/registry";
import { onWorkspaceChanged, revealSidebar, useApp } from "@/app/store";
import { events } from "@/ipc";
import { ExplorerView, FOCUS_EVENT } from "./ExplorerView";
import { queueFsChange, refreshAll, setRoot, useExplorer } from "./store";

/** Mengikuti event `fs-changed` dari watcher. Mengembalikan fungsi untuk berhenti mendengarkan. */
export function watchFsChanges(): () => void {
  const unlisten = events.fsChanged.listen((e) => queueFsChange(e.payload));
  return () => void unlisten.then((stop) => stop());
}

export function registerExplorerFeature(): void {
  registerSidebarView({ id: "explorer", title: "Explorer", icon: <Files size={20} />, order: 10, component: ExplorerView });

  registerCommands([
    {
      id: "explorer.focus",
      title: "Focus on Explorer",
      category: "View",
      keybinding: "Mod+Shift+E",
      run: () => {
        revealSidebar("explorer");
        // Tampilan mungkin baru saja dimunculkan; fokus dipindah setelah ia tergambar.
        setTimeout(() => window.dispatchEvent(new Event(FOCUS_EVENT)), 0);
      },
    },
  ]);

  // Folder berganti: pohon dikosongkan. Folder sama (mis. status kepercayaan berubah): cukup dimuat ulang.
  onWorkspaceChanged((ws) => {
    const root = ws?.root ?? null;
    if (root !== null && useExplorer.getState().root === root) void refreshAll();
    else setRoot(root);
  });
  // Bila folder sudah terbuka sebelum fitur didaftarkan.
  setRoot(useApp.getState().workspace?.root ?? null);

  watchFsChanges();
}
