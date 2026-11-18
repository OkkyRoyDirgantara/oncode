// Titik sambung antarfitur: jenis tab, tampilan sidebar, tampilan panel bawah, dan item status bar.
// Tiap fitur mendaftarkan diri lewat modul `features/<nama>/index.ts`.
import type { ComponentType, ReactNode } from "react";

export interface Tab {
  /** Unik di seluruh aplikasi; tab dengan id sama tidak dibuka dua kali dalam satu grup. */
  id: string;
  kind: string;
  title: string;
  /** Teks lengkap untuk tooltip (mis. path). */
  description?: string;
  params: Record<string, unknown>;
  /** Tab pratinjau diganti saat tab pratinjau lain dibuka. */
  preview?: boolean;
}

export interface TabProps {
  tab: Tab;
  groupId: string;
  /** Tab ini sedang terlihat di grupnya. */
  visible: boolean;
  /** Tab ini terlihat dan grupnya sedang aktif. */
  focused: boolean;
}

export interface TabType {
  kind: string;
  component: ComponentType<TabProps>;
  icon?: (tab: Tab) => ReactNode;
  /** Hook: tab punya perubahan yang belum disimpan. */
  useDirty?: (tab: Tab) => boolean;
  /** Dipanggil sebelum tab ditutup; kembalikan false untuk membatalkan. */
  beforeClose?: (tab: Tab) => Promise<boolean> | boolean;
  /** Tetap dirender saat tidak terlihat (default: true agar keadaan editor bertahan). */
  keepAlive?: boolean;
}

export interface SidebarView {
  id: string;
  title: string;
  icon: ReactNode;
  order: number;
  component: ComponentType;
  /** Hook: angka kecil pada ikon (mis. jumlah perubahan). */
  useBadge?: () => number | undefined;
  /** Hook: tampilkan ikon hanya bila true. */
  useVisible?: () => boolean;
}

export interface PanelView {
  id: string;
  title: string;
  order: number;
  component: ComponentType<{ visible: boolean }>;
  /** Hook: angka di samping judul tab panel. */
  useBadge?: () => number | undefined;
  /** Aksi di sisi kanan bilah tab panel. */
  actions?: ComponentType;
}

export interface StatusItem {
  id: string;
  align: "left" | "right";
  order: number;
  component: ComponentType;
}

const tabTypes = new Map<string, TabType>();
const sidebarViews = new Map<string, SidebarView>();
const panelViews = new Map<string, PanelView>();
const statusItems = new Map<string, StatusItem>();

export function registerTabType(type: TabType): void {
  tabTypes.set(type.kind, type);
}
export function getTabType(kind: string): TabType | undefined {
  return tabTypes.get(kind);
}
export function registerSidebarView(view: SidebarView): void {
  sidebarViews.set(view.id, view);
}
export function getSidebarViews(): SidebarView[] {
  return [...sidebarViews.values()].sort((a, b) => a.order - b.order);
}
export function registerPanelView(view: PanelView): void {
  panelViews.set(view.id, view);
}
export function getPanelViews(): PanelView[] {
  return [...panelViews.values()].sort((a, b) => a.order - b.order);
}
export function registerStatusItem(item: StatusItem): void {
  statusItems.set(item.id, item);
}
export function getStatusItems(align: "left" | "right"): StatusItem[] {
  return [...statusItems.values()].filter((i) => i.align === align).sort((a, b) => a.order - b.order);
}
