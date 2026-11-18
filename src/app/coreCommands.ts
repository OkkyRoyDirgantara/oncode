// Command inti workbench: palet, quick open, tata letak, tab, jendela, folder.
import { commands } from "@/ipc";
import { allCommands, formatKeybinding, keybindingsFor, registerCommands } from "./commands";
import { quickPick, showError, type PickItem } from "./overlay";
import { getPanelViews, getSidebarViews } from "./registry";
import { absolutePath, baseName, closeFolder, openFolder, pickAndOpenFolder, revealSidebar, togglePanel, toggleSidebar, useApp } from "./store";
import { activeTab, closeAllTabs, closeTab, cycleTab, openTab, splitActiveTab, toggleOrientation } from "./workbench";

async function showCommandPalette() {
  const items: PickItem<string>[] = allCommands()
    .filter((c) => !c.hidden && (!c.when || c.when()))
    .map((c) => ({
      label: c.category ? `${c.category}: ${c.title}` : c.title,
      value: c.id,
      hint: keybindingsFor(c)[0] ? formatKeybinding(keybindingsFor(c)[0]) : undefined,
    }))
    .sort((a, b) => a.label.localeCompare(b.label));
  const id = await quickPick({ placeholder: "Type a command", items });
  if (!id) return;
  const command = allCommands().find((c) => c.id === id);
  try {
    await command?.run();
  } catch (e) {
    showError(e);
  }
}

export interface RevealTarget {
  path: string;
  /** Nomor baris mulai dari 1. */
  line: number;
  /** Kolom mulai dari 0. */
  column: number;
  /** Panjang pilihan (mis. kecocokan pencarian). */
  length?: number;
}

const pendingReveals = new Map<string, RevealTarget>();

/** Posisi yang menunggu editor berkas ini siap; diambil sekali oleh editor. */
export function takePendingReveal(path: string): RevealTarget | undefined {
  const target = pendingReveals.get(path);
  pendingReveals.delete(path);
  return target;
}

/** Membuka berkas di editor, opsional langsung melompat ke sebuah posisi. */
export function openFile(path: string, options: { preview?: boolean; line?: number; column?: number; length?: number; toSide?: boolean } = {}): void {
  const absolute = absolutePath(path);
  if (options.line !== undefined) {
    // Editor yang belum terpasang mengambil posisi ini begitu siap; yang sudah ada mendengar event di bawah.
    pendingReveals.set(absolute, { path: absolute, line: options.line, column: options.column ?? 0, length: options.length });
  }
  openTab(
    { id: `file:${absolute}`, kind: "file", title: baseName(absolute), description: absolute, params: { path: absolute } },
    { preview: options.preview, toSide: options.toSide },
  );
  if (options.line !== undefined) {
    setTimeout(() => window.dispatchEvent(new CustomEvent("oncode:reveal", { detail: pendingReveals.get(absolute) ?? { path: absolute, line: options.line, column: options.column ?? 0, length: options.length } })), 0);
  }
}

async function showQuickOpen() {
  if (!useApp.getState().workspace) return;
  const path = await quickPick<string>({
    placeholder: "Go to file",
    emptyText: "No matching files",
    items: async (query) => {
      const results = await commands.quickOpen(query, 60);
      return results.map((r) => {
        const slash = r.path.lastIndexOf("/");
        const name = r.path.slice(slash + 1);
        return {
          label: name,
          description: slash >= 0 ? r.path.slice(0, slash) : "",
          value: r.path,
          matches: r.indices.filter((i) => i > slash).map((i) => i - slash - 1),
        };
      });
    },
  });
  if (path) openFile(path);
}

export function registerCoreCommands(): void {
  const hasWorkspace = () => useApp.getState().workspace !== null;
  registerCommands([
    { id: "workbench.commandPalette", title: "Show All Commands", category: "View", keybinding: ["Mod+Shift+P", "F1"], hidden: true, run: showCommandPalette },
    { id: "workbench.quickOpen", title: "Go to File…", category: "File", keybinding: "Mod+P", when: hasWorkspace, run: showQuickOpen },
    { id: "workbench.toggleSidebar", title: "Toggle Sidebar", category: "View", keybinding: "Mod+B", run: toggleSidebar },
    { id: "workbench.togglePanel", title: "Toggle Panel", category: "View", keybinding: "Mod+J", run: () => togglePanel() },
    { id: "workbench.closeTab", title: "Close Editor", category: "View", keybinding: "Mod+W", run: () => {
        const active = activeTab();
        if (active) return closeTab(active.group.id, active.tab.id);
      } },
    { id: "workbench.closeAllTabs", title: "Close All Editors", category: "View", run: () => closeAllTabs() },
    { id: "workbench.nextTab", title: "Next Editor", category: "View", keybinding: "Mod+PageDown", run: () => cycleTab(1) },
    { id: "workbench.previousTab", title: "Previous Editor", category: "View", keybinding: "Mod+PageUp", run: () => cycleTab(-1) },
    { id: "workbench.split", title: "Split Editor", category: "View", keybinding: "Mod+\\", run: splitActiveTab },
    { id: "workbench.toggleSplitDirection", title: "Toggle Split Direction", category: "View", run: toggleOrientation },
    { id: "file.openFolder", title: "Open Folder…", category: "File", keybinding: "Mod+O", run: pickAndOpenFolder },
    { id: "file.closeFolder", title: "Close Folder", category: "File", when: hasWorkspace, run: closeFolder },
    { id: "window.new", title: "New Window", category: "Window", keybinding: "Mod+Shift+N", run: () => commands.windowOpen(null) },
    {
      id: "file.openRecent",
      title: "Open Recent…",
      category: "File",
      run: async () => {
        const recent = useApp.getState().info?.recent ?? [];
        const path = await quickPick({ placeholder: "Open recent folder", items: recent.map((p) => ({ label: baseName(p), description: p, value: p })) });
        if (path) await openFolder(path);
      },
    },
  ]);
  for (const view of getSidebarViews()) {
    registerCommands([{ id: `view.${view.id}`, title: `Show ${view.title}`, category: "View", run: () => revealSidebar(view.id) }]);
  }
  for (const view of getPanelViews()) {
    registerCommands([{ id: `panel.${view.id}`, title: `Show ${view.title}`, category: "View", run: () => togglePanel(view.id) }]);
  }
}
