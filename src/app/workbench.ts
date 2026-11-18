// Model tab dan grup editor. Murni state; tampilannya ada di Workbench.tsx.
import { create } from "zustand";
import { getTabType, type Tab } from "./registry";

export interface Group {
  id: string;
  tabs: Tab[];
  activeId: string | null;
}

interface WorkbenchState {
  groups: Group[];
  activeGroupId: string;
  orientation: "horizontal" | "vertical";
}

let groupCounter = 1;
const newGroup = (): Group => ({ id: `g${groupCounter++}`, tabs: [], activeId: null });
const first = newGroup();

export const useWorkbench = create<WorkbenchState>(() => ({
  groups: [first],
  activeGroupId: first.id,
  orientation: "horizontal",
}));

export type NewTab = Omit<Tab, "id"> & { id?: string };

export interface OpenOptions {
  /** Buka sebagai pratinjau (menggantikan tab pratinjau sebelumnya). */
  preview?: boolean;
  groupId?: string;
  /** Buka di grup sebelah (dibuat bila belum ada). */
  toSide?: boolean;
  /** Jangan pindahkan fokus ke tab ini. */
  background?: boolean;
}

export function tabId(tab: NewTab): string {
  return tab.id ?? `${tab.kind}:${JSON.stringify(tab.params)}`;
}

function update(change: (s: WorkbenchState) => Partial<WorkbenchState>) {
  useWorkbench.setState((s) => change(s));
}

function mapGroup(groups: Group[], id: string, f: (g: Group) => Group): Group[] {
  return groups.map((g) => (g.id === id ? f(g) : g));
}

/** Membuka tab (atau mengaktifkannya bila sudah ada) dan mengembalikan id-nya. */
export function openTab(input: NewTab, options: OpenOptions = {}): string {
  const id = tabId(input);
  const tab: Tab = { ...input, id, preview: options.preview ?? false };
  update((s) => {
    let groups = s.groups;
    let groupId = options.groupId ?? s.activeGroupId;
    if (options.toSide) {
      const index = groups.findIndex((g) => g.id === s.activeGroupId);
      const next = groups[index + 1];
      if (next) groupId = next.id;
      else {
        const created = newGroup();
        groups = [...groups, created];
        groupId = created.id;
      }
    } else if (!options.groupId) {
      // Bila tab sudah terbuka di grup lain, pakai yang itu.
      const holder = groups.find((g) => g.tabs.some((t) => t.id === id));
      if (holder && !groups.find((g) => g.id === groupId)?.tabs.some((t) => t.id === id)) groupId = holder.id;
    }
    if (!groups.some((g) => g.id === groupId)) groupId = groups[0].id;

    groups = mapGroup(groups, groupId, (g) => {
      const existing = g.tabs.find((t) => t.id === id);
      if (existing) {
        const tabs = g.tabs.map((t) => (t.id === id ? { ...t, ...tab, preview: t.preview && tab.preview } : t));
        return { ...g, tabs, activeId: options.background ? g.activeId : id };
      }
      const previewIndex = tab.preview ? g.tabs.findIndex((t) => t.preview) : -1;
      let tabs: Tab[];
      if (previewIndex >= 0) {
        tabs = g.tabs.map((t, i) => (i === previewIndex ? tab : t));
      } else {
        const activeIndex = g.tabs.findIndex((t) => t.id === g.activeId);
        tabs = [...g.tabs.slice(0, activeIndex + 1), tab, ...g.tabs.slice(activeIndex + 1)];
      }
      return { ...g, tabs, activeId: options.background && g.activeId ? g.activeId : id };
    });
    return { groups, activeGroupId: options.background ? s.activeGroupId : groupId };
  });
  return id;
}

export function activateTab(groupId: string, id: string): void {
  update((s) => ({ groups: mapGroup(s.groups, groupId, (g) => ({ ...g, activeId: id })), activeGroupId: groupId }));
}

export function activateGroup(groupId: string): void {
  if (useWorkbench.getState().activeGroupId !== groupId) update(() => ({ activeGroupId: groupId }));
}

/** Menjadikan tab pratinjau sebagai tab tetap (mis. saat mulai disunting atau diklik ganda). */
export function pinTab(id: string): void {
  update((s) => ({ groups: s.groups.map((g) => ({ ...g, tabs: g.tabs.map((t) => (t.id === id && t.preview ? { ...t, preview: false } : t)) })) }));
}

/** Mengubah tab di semua grup (mis. setelah berkas diganti nama). */
export function updateTabs(change: (tab: Tab) => Tab | null): void {
  update((s) => ({
    groups: s.groups.map((g) => {
      const tabs = g.tabs.map((t) => change(t) ?? t);
      return { ...g, tabs, activeId: tabs.find((_t, i) => g.tabs[i].id === g.activeId)?.id ?? g.activeId };
    }),
  }));
}

function removeTabs(groupId: string, ids: Set<string>) {
  update((s) => {
    let groups = mapGroup(s.groups, groupId, (g) => {
      const tabs = g.tabs.filter((t) => !ids.has(t.id));
      let activeId = g.activeId;
      if (activeId && ids.has(activeId)) {
        const oldIndex = g.tabs.findIndex((t) => t.id === activeId);
        const after = g.tabs.slice(oldIndex + 1).find((t) => !ids.has(t.id));
        const before = [...g.tabs.slice(0, oldIndex)].reverse().find((t) => !ids.has(t.id));
        activeId = (after ?? before)?.id ?? null;
      }
      return { ...g, tabs, activeId };
    });
    // Grup kosong dibuang, kecuali grup terakhir.
    let activeGroupId = s.activeGroupId;
    if (groups.length > 1) {
      const kept = groups.filter((g) => g.tabs.length > 0);
      groups = kept.length > 0 ? kept : [groups[0]];
      if (!groups.some((g) => g.id === activeGroupId)) activeGroupId = groups[Math.max(0, groups.length - 1)].id;
    }
    return { groups, activeGroupId };
  });
}

/** Menutup tab setelah `beforeClose` mengizinkan (kecuali `force`). Mengembalikan false bila dibatalkan. */
export async function closeTabs(groupId: string, ids: string[], force = false): Promise<boolean> {
  const group = useWorkbench.getState().groups.find((g) => g.id === groupId);
  if (!group) return true;
  const closing: string[] = [];
  for (const id of ids) {
    const tab = group.tabs.find((t) => t.id === id);
    if (!tab) continue;
    const elsewhere = useWorkbench.getState().groups.some((g) => g.id !== groupId && g.tabs.some((t) => t.id === id));
    if (!force && !elsewhere) {
      const allowed = await getTabType(tab.kind)?.beforeClose?.(tab);
      if (allowed === false) {
        removeTabs(groupId, new Set(closing));
        return false;
      }
    }
    closing.push(id);
  }
  removeTabs(groupId, new Set(closing));
  return true;
}

export function closeTab(groupId: string, id: string, force = false): Promise<boolean> {
  return closeTabs(groupId, [id], force);
}

/** Menutup tab dengan id tertentu di semua grup tanpa konfirmasi (mis. berkasnya sudah dihapus). */
export function forceCloseEverywhere(match: (tab: Tab) => boolean): void {
  for (const group of useWorkbench.getState().groups) {
    const ids = group.tabs.filter(match).map((t) => t.id);
    if (ids.length) removeTabs(group.id, new Set(ids));
  }
}

export async function closeAllTabs(force = false): Promise<boolean> {
  for (const group of [...useWorkbench.getState().groups]) {
    if (!(await closeTabs(group.id, group.tabs.map((t) => t.id), force))) return false;
  }
  return true;
}

/** Menyalin tab aktif ke grup sebelah. */
export function splitActiveTab(): void {
  const active = activeTab();
  if (!active) return;
  openTab({ ...active.tab }, { toSide: true });
}

export function toggleOrientation(): void {
  update((s) => ({ orientation: s.orientation === "horizontal" ? "vertical" : "horizontal" }));
}

export function activeTab(): { group: Group; tab: Tab } | null {
  const s = useWorkbench.getState();
  const group = s.groups.find((g) => g.id === s.activeGroupId);
  const tab = group?.tabs.find((t) => t.id === group.activeId);
  return group && tab ? { group, tab } : null;
}

export function allTabs(): Tab[] {
  return useWorkbench.getState().groups.flatMap((g) => g.tabs);
}

/** Berpindah ke tab berikut/sebelumnya di grup aktif. */
export function cycleTab(delta: 1 | -1): void {
  const s = useWorkbench.getState();
  const group = s.groups.find((g) => g.id === s.activeGroupId);
  if (!group || group.tabs.length < 2) return;
  const index = group.tabs.findIndex((t) => t.id === group.activeId);
  const next = group.tabs[(index + delta + group.tabs.length) % group.tabs.length];
  activateTab(group.id, next.id);
}

/** Hanya untuk pengujian. */
export function resetWorkbenchForTest(): void {
  const g = newGroup();
  useWorkbench.setState({ groups: [g], activeGroupId: g.id, orientation: "horizontal" });
}
