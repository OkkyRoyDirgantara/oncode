// Sesi per proyek: tab berkas yang terbuka dipulihkan saat folder dibuka lagi.
import { commands } from "@/ipc";
import { openFile } from "./coreCommands";
import { onBeforeWorkspaceSwitch, onWorkspaceChanged, useApp } from "./store";
import { activateTab, useWorkbench } from "./workbench";

export interface SessionData {
  /** Path absolut berkas yang terbuka, sesuai urutan tab. */
  files: string[];
  active: string | null;
}

/** Tab berkas (tetap, bukan pratinjau) dari semua grup. */
export function collectSession(): SessionData {
  const state = useWorkbench.getState();
  const files: string[] = [];
  for (const group of state.groups) {
    for (const tab of group.tabs) {
      const path = tab.kind === "file" && !tab.preview ? (tab.params.path as string) : null;
      if (path && !files.includes(path)) files.push(path);
    }
  }
  const activeGroup = state.groups.find((g) => g.id === state.activeGroupId);
  const activeTab = activeGroup?.tabs.find((t) => t.id === activeGroup.activeId);
  return { files, active: activeTab?.kind === "file" ? (activeTab.params.path as string) : null };
}

export function parseSession(json: string | null): SessionData {
  if (!json) return { files: [], active: null };
  try {
    const data = JSON.parse(json) as Partial<SessionData>;
    const files = Array.isArray(data.files) ? data.files.filter((f): f is string => typeof f === "string").slice(0, 50) : [];
    return { files, active: typeof data.active === "string" && files.includes(data.active) ? data.active : null };
  } catch {
    return { files: [], active: null };
  }
}

/** Sesi tidak disimpan selama folder sedang berganti (tab ditutup) atau sedang dipulihkan. */
let frozen = true;
let timer: ReturnType<typeof setTimeout> | undefined;
let lastSaved = "";

function save() {
  const root = useApp.getState().workspace?.root;
  if (!root || frozen) return;
  const json = JSON.stringify(collectSession());
  if (json === lastSaved) return;
  lastSaved = json;
  void commands.sessionSet(root, json).catch(() => {});
}

async function restore(root: string) {
  const session = parseSession(await commands.sessionGet(root).catch(() => null));
  lastSaved = JSON.stringify(session);
  for (const path of session.files) {
    // Berkas yang sudah tidak ada dilewati.
    const stat = await commands.fsStat(path).catch(() => null);
    if (stat && useApp.getState().workspace?.root === root) openFile(path);
  }
  if (session.active && useApp.getState().workspace?.root === root) {
    for (const group of useWorkbench.getState().groups) {
      const tab = group.tabs.find((t) => t.kind === "file" && t.params.path === session.active);
      if (tab) activateTab(group.id, tab.id);
    }
  }
}

export function installSession(): void {
  onBeforeWorkspaceSwitch(() => {
    clearTimeout(timer);
    save();
    frozen = true;
  });
  onWorkspaceChanged((workspace) => {
    if (!workspace) return;
    frozen = true;
    void restore(workspace.root).finally(() => {
      frozen = false;
    });
  });
  useWorkbench.subscribe(() => {
    if (frozen) return;
    clearTimeout(timer);
    timer = setTimeout(save, 500);
  });
  window.addEventListener("beforeunload", save);
}

/** Hanya untuk pengujian. */
export function setSessionFrozenForTest(value: boolean): void {
  frozen = value;
  lastSaved = "";
}
