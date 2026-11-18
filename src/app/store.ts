// State aplikasi: info, workspace, pengaturan, dan keadaan tata letak.
import { create } from "zustand";
import { open as openDialogNative } from "@tauri-apps/plugin-dialog";
import { commands, events, type AppInfo, type Settings, type WorkspaceInfo } from "@/ipc";
import { showError } from "./overlay";
import { closeAllTabs } from "./workbench";

interface AppState {
  ready: boolean;
  info: AppInfo | null;
  workspace: WorkspaceInfo | null;
  settings: Settings | null;
  sidebarView: string;
  sidebarVisible: boolean;
  panelView: string;
  panelVisible: boolean;
}

export const useApp = create<AppState>(() => ({
  ready: false,
  info: null,
  workspace: null,
  settings: null,
  sidebarView: "explorer",
  sidebarVisible: true,
  panelView: "terminal",
  panelVisible: false,
}));

const workspaceListeners = new Set<(ws: WorkspaceInfo | null) => void>();
const beforeSwitchListeners = new Set<() => void>();

/** Dipanggil tepat sebelum tab folder saat ini ditutup untuk berganti atau menutup folder. */
export function onBeforeWorkspaceSwitch(listener: () => void): () => void {
  beforeSwitchListeners.add(listener);
  return () => beforeSwitchListeners.delete(listener);
}

/** Dipanggil setiap kali folder dibuka, ditutup, atau status kepercayaannya berubah. */
export function onWorkspaceChanged(listener: (ws: WorkspaceInfo | null) => void): () => void {
  workspaceListeners.add(listener);
  return () => workspaceListeners.delete(listener);
}

function setWorkspace(workspace: WorkspaceInfo | null) {
  const previous = useApp.getState().workspace;
  useApp.setState((s) => ({
    workspace,
    settings: workspace?.settings ?? s.settings,
    // Folder repositori membuka Source Control lebih dulu (bisa diubah di pengaturan); juga saat
    // repositori baru tersedia setelah folder dipercaya.
    sidebarView:
      workspace && (previous?.root !== workspace.root || (!previous.repoRoot && workspace.repoRoot))
        ? workspace.repoRoot && workspace.settings.workbench.startupView === "scm"
          ? "scm"
          : previous?.root === workspace.root
            ? s.sidebarView
            : "explorer"
        : s.sidebarView,
  }));
  workspaceListeners.forEach((l) => l(workspace));
}

export async function initApp(): Promise<void> {
  const info = await commands.appInfo();
  const settings = await commands.settingsGet();
  useApp.setState({ info, settings });
  void events.settingsChanged.listen((e) => useApp.setState({ settings: e.payload.settings }));
  const folder = new URLSearchParams(window.location.search).get("folder");
  if (folder) await openFolder(folder);
  useApp.setState({ ready: true });
}

export async function openFolder(path: string): Promise<boolean> {
  try {
    if (useApp.getState().workspace) {
      beforeSwitchListeners.forEach((l) => l());
      if (!(await closeAllTabs())) return false;
    }
    setWorkspace(await commands.workspaceOpen(path));
    void commands.appInfo().then((info) => useApp.setState({ info }));
    return true;
  } catch (e) {
    showError(e, "Could not open folder");
    return false;
  }
}

export async function pickFolder(title = "Open Folder"): Promise<string | null> {
  const picked = await openDialogNative({ directory: true, multiple: false, title });
  return typeof picked === "string" ? picked : null;
}

export async function pickAndOpenFolder(): Promise<void> {
  const path = await pickFolder();
  if (path) await openFolder(path);
}

export async function closeFolder(): Promise<void> {
  beforeSwitchListeners.forEach((l) => l());
  if (!(await closeAllTabs())) return;
  await commands.workspaceClose();
  setWorkspace(null);
}

export async function setTrusted(trusted: boolean): Promise<void> {
  try {
    setWorkspace(await commands.workspaceSetTrusted(trusted));
  } catch (e) {
    showError(e);
  }
}

export function showSidebar(view: string): void {
  useApp.setState((s) => (s.sidebarVisible && s.sidebarView === view ? { sidebarVisible: false } : { sidebarView: view, sidebarVisible: true }));
}

export function revealSidebar(view: string): void {
  useApp.setState({ sidebarView: view, sidebarVisible: true });
}

export function toggleSidebar(): void {
  useApp.setState((s) => ({ sidebarVisible: !s.sidebarVisible }));
}

export function showPanel(view: string): void {
  useApp.setState({ panelView: view, panelVisible: true });
}

export function togglePanel(view?: string): void {
  useApp.setState((s) => {
    if (view && (!s.panelVisible || s.panelView !== view)) return { panelView: view, panelVisible: true };
    return { panelVisible: !s.panelVisible };
  });
}

/** Path relatif terhadap folder proyek, dengan garis miring biasa. */
export function relativePath(absolute: string): string {
  const root = useApp.getState().workspace?.root;
  if (!root) return absolute;
  const normalizedRoot = root.replace(/[\\/]+$/, "");
  if (absolute === normalizedRoot) return "";
  if (absolute.startsWith(normalizedRoot + "/") || absolute.startsWith(normalizedRoot + "\\")) {
    return absolute.slice(normalizedRoot.length + 1).replace(/\\/g, "/");
  }
  return absolute;
}

/** Menggabungkan path relatif ke folder proyek memakai pemisah milik OS. */
export function absolutePath(relative: string): string {
  const root = useApp.getState().workspace?.root ?? "";
  if (!relative) return root;
  if (/^([a-zA-Z]:[\\/]|[\\/])/.test(relative)) return relative;
  const sep = root.includes("\\") && !root.includes("/") ? "\\" : "/";
  return root.replace(/[\\/]+$/, "") + sep + relative.replace(/[\\/]/g, sep);
}

export function baseName(path: string): string {
  const trimmed = path.replace(/[\\/]+$/, "");
  const index = Math.max(trimmed.lastIndexOf("/"), trimmed.lastIndexOf("\\"));
  return index >= 0 ? trimmed.slice(index + 1) : trimmed;
}

export function dirName(path: string): string {
  const trimmed = path.replace(/[\\/]+$/, "");
  const index = Math.max(trimmed.lastIndexOf("/"), trimmed.lastIndexOf("\\"));
  return index > 0 ? trimmed.slice(0, index) : index === 0 ? trimmed.slice(0, 1) : "";
}
