// Fitur Search: pencarian teks di seluruh proyek dan "Replace All".
import { Search } from "lucide-react";
import { registerCommands } from "@/app/commands";
import { registerSidebarView } from "@/app/registry";
import { onWorkspaceChanged, revealSidebar } from "@/app/store";
import { activeEditor } from "../editor/CodeEditor";
import { SearchView } from "./SearchView";
import { requestFocus, resetSearch, useSearch } from "./store";

/** Teks terpilih di editor aktif, bila tidak kosong dan hanya satu baris. */
function editorSelection(): string | undefined {
  const active = activeEditor();
  if (!active) return undefined;
  const { state } = active.view;
  const range = state.selection.main;
  if (range.empty) return undefined;
  const text = state.sliceDoc(range.from, range.to);
  return text.includes("\n") ? undefined : text;
}

/** Memunculkan tampilan Search dan memfokuskan kolom kueri, diisi teks terpilih di editor bila ada. */
export function focusSearch(): void {
  const selection = editorSelection();
  // Pada mode regex, teks terpilih dicari apa adanya.
  const prefill = selection !== undefined && useSearch.getState().isRegex ? selection.replace(/[\\^$.*+?()[\]{}|]/g, "\\$&") : selection;
  revealSidebar("search");
  requestFocus(prefill);
}

export function registerSearchFeature(): void {
  registerSidebarView({ id: "search", title: "Search", icon: <Search size={20} />, order: 20, component: SearchView });

  registerCommands([{ id: "search.focus", title: "Find in Files", category: "Search", keybinding: "Mod+Shift+F", run: focusSearch }]);

  // Hasil pencarian milik folder lama tidak berlaku lagi. Perubahan status kepercayaan pada folder yang sama tidak mengosongkannya.
  let root: string | null = null;
  onWorkspaceChanged((ws) => {
    const next = ws?.root ?? null;
    if (next !== root) resetSearch();
    root = next;
  });
}
