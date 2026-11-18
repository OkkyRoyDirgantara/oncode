// Operasi berkas dari explorer: buat, ganti nama, hapus ke tempat sampah, salin path.
import { openFile } from "@/app/coreCommands";
import { confirm, promptInput, showError } from "@/app/overlay";
import { baseName, dirName, relativePath, useApp } from "@/app/store";
import { allTabs, forceCloseEverywhere, updateTabs } from "@/app/workbench";
import { commands, type DirEntry } from "@/ipc";
import { dirtyPaths, openBuffers, renameBuffer } from "../editor/buffers";
import { expandDir, loadDir, moveTreeState, reloadUnder, revealPath, selectPath, useExplorer } from "./store";
import { isSameOrUnder, joinPath, movedPath, rewriteFileTab, validateEntryName } from "./tree";

/** Entri yang sedang terpilih, dicari di isi folder induknya. */
export function selectedEntry(): DirEntry | null {
  const { selected, dirs } = useExplorer.getState();
  if (!selected) return null;
  return dirs[dirName(selected)]?.entries?.find((e) => e.path === selected) ?? null;
}

/** Folder tujuan untuk berkas/folder baru: folder terpilih, folder dari berkas terpilih, atau akar proyek. */
export function targetDirectory(): string | null {
  const root = useExplorer.getState().root;
  const entry = selectedEntry();
  if (!entry) return root;
  return entry.isDir ? entry.path : dirName(entry.path);
}

/** Meminta nama lalu membuat berkas atau folder di `dir` (default: `targetDirectory()`). Berkas baru langsung dibuka. */
export async function createEntry(kind: "file" | "folder", dir?: string): Promise<void> {
  const root = useExplorer.getState().root;
  const parent = dir ?? targetDirectory();
  if (!root || !parent) return;
  // Folder tujuan dibuka lebih dulu agar terlihat di mana entri baru akan muncul.
  if (parent !== root) await expandDir(parent);
  else if (!useExplorer.getState().dirs[root]?.entries) await loadDir(root);
  const siblings = useExplorer.getState().dirs[parent]?.entries ?? null;
  const where = relativePath(parent) || useApp.getState().workspace?.name || baseName(root);

  const name = await promptInput({
    title: kind === "file" ? "New File" : "New Folder",
    label: `Name of the new ${kind} in ${where}`,
    placeholder: kind === "file" ? "name.ext or folder/name.ext" : "name or parent/name",
    confirmLabel: "Create",
    validate: (value) => validateEntryName(value, siblings, { nested: true }),
  });
  if (name === null) return;

  const path = joinPath(parent, name);
  try {
    if (kind === "file") await commands.fsCreateFile(path);
    else await commands.fsCreateDir(path);
  } catch (e) {
    showError(e, `Could not create ${name}`);
    return;
  }
  await loadDir(parent, true);
  // Folder antara (`a/b/c.txt`) mungkin sudah pernah dimuat; cache-nya kini usang.
  if (name.includes("/")) await reloadUnder(joinPath(parent, name.split("/")[0]));
  await revealPath(path);
  if (kind === "file") openFile(path);
}

/** Memindahkan buffer dan tab berkas dari `from` (berkas, atau folder beserta isinya) ke `to`. */
export function applyRename(from: string, to: string): void {
  // Buffer dipindah lebih dulu: isi yang belum disimpan ikut ke path baru sebelum tab dipasang ulang.
  for (const buffer of openBuffers()) {
    const moved = movedPath(buffer.path, from, to);
    if (moved !== null) renameBuffer(buffer.path, moved);
  }
  updateTabs((tab) => rewriteFileTab(tab, from, to));
}

export async function renameEntry(entry: DirEntry): Promise<void> {
  const parent = dirName(entry.path);
  const siblings = useExplorer.getState().dirs[parent]?.entries ?? null;
  // Untuk berkas, hanya nama tanpa ekstensi yang terpilih; `.gitignore` dan folder terpilih seluruhnya.
  const dot = entry.name.lastIndexOf(".");
  const stemEnd = !entry.isDir && dot > 0 ? dot : entry.name.length;

  const name = await promptInput({
    title: `Rename ${entry.isDir ? "Folder" : "File"}`,
    label: `New name for "${entry.name}"`,
    value: entry.name,
    selection: [0, stemEnd],
    confirmLabel: "Rename",
    validate: (value) => validateEntryName(value, siblings, { current: entry.name }),
  });
  if (name === null || name === entry.name) return;

  const to = joinPath(parent, name);
  try {
    await commands.fsRename(entry.path, to);
  } catch (e) {
    showError(e, `Could not rename ${entry.name}`);
    return;
  }
  applyRename(entry.path, to);
  moveTreeState(entry.path, to);
  // Induk lebih dulu: folder di bawah nama baru hanya dimuat setelah nama itu ada di isi induknya.
  await loadDir(parent, true);
  if (entry.isDir) await reloadUnder(to);
  selectPath(to);
}

/** Memindahkan entri ke tempat sampah OS setelah konfirmasi, lalu menutup tab berkas yang ikut terhapus. */
export async function deleteEntry(entry: DirEntry): Promise<void> {
  const inside = (path: unknown): path is string => typeof path === "string" && isSameOrUnder(path, entry.path);
  const dirty = dirtyPaths().filter((path) => inside(path) && allTabs().some((t) => t.kind === "file" && t.params.path === path));
  const unsaved =
    dirty.length === 0
      ? ""
      : dirty.length === 1
        ? ` "${baseName(dirty[0])}" has unsaved changes that will be lost.`
        : ` ${dirty.length} open files inside it have unsaved changes that will be lost.`;

  const confirmed = await confirm({
    title: `Delete ${entry.isDir ? "folder" : "file"} "${entry.name}"?`,
    message: `${entry.isDir ? `"${entry.name}" and everything inside it` : `"${entry.name}"`} will be moved to the system trash.${unsaved}`,
    confirmLabel: "Move to Trash",
    danger: true,
  });
  if (!confirmed) return;

  try {
    await commands.fsTrash([entry.path]);
  } catch (e) {
    showError(e, `Could not delete ${entry.name}`);
    return;
  }
  forceCloseEverywhere((tab) => tab.kind === "file" && inside(tab.params.path));
  // Memuat ulang induk sekaligus membuang keadaan pohon di bawah entri yang hilang.
  await loadDir(dirName(entry.path), true);
}

export function copyPath(entry: DirEntry, relative: boolean): void {
  void navigator.clipboard.writeText(relative ? relativePath(entry.path) : entry.path).catch((e) => showError(e, "Could not copy the path"));
}
