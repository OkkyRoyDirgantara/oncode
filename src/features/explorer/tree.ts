// Logika murni explorer: path, perataan pohon, validasi nama, dekorasi Git, dan penulisan ulang tab.
// Tidak menyentuh state maupun IPC agar mudah diuji.
import type { Tab } from "@/app/registry";
import type { ChangeKind, DirEntry, RepoSnapshot } from "@/ipc";

/** Tinggi satu baris pohon dan lekukan per tingkat, dalam piksel. */
export const ROW_HEIGHT = 22;
export const INDENT = 12;

export interface DirState {
  /** Isi folder; null selama belum pernah berhasil dimuat. */
  entries: DirEntry[] | null;
  loading: boolean;
  error: string | null;
}

// ----- Path -----

function separatorOf(path: string): string {
  return path.includes("\\") && !path.includes("/") ? "\\" : "/";
}

function stripTrailing(path: string): string {
  const trimmed = path.replace(/[\\/]+$/, "");
  // Akar sistem berkas ("/") tetap dipertahankan.
  return trimmed === "" ? path.slice(0, 1) : trimmed;
}

/** Menggabungkan `dir` dengan path relatif bergaris miring, memakai pemisah yang dipakai `dir`. */
export function joinPath(dir: string, relative: string): string {
  const sep = separatorOf(dir);
  const base = stripTrailing(dir);
  const tail = relative.replace(/[\\/]/g, sep);
  return base.endsWith(sep) ? base + tail : base + sep + tail;
}

/** Benar bila `path` sama dengan `ancestor` atau berada di dalamnya. */
export function isSameOrUnder(path: string, ancestor: string): boolean {
  const base = stripTrailing(ancestor);
  if (path === base || path === ancestor) return true;
  if (!path.startsWith(base)) return false;
  const next = path[base.length];
  return next === "/" || next === "\\" || base.endsWith("/") || base.endsWith("\\");
}

/** Folder dari `root` sampai induk langsung `path` (urut dari luar ke dalam). Kosong bila `path` di luar `root`. */
export function ancestorChain(root: string, path: string): string[] {
  const base = stripTrailing(root);
  if (path === base || !isSameOrUnder(path, base)) return [];
  const sep = separatorOf(root);
  const segments = path.slice(base.length).split(/[\\/]/).filter(Boolean);
  const chain = [base];
  let current = base;
  for (const segment of segments.slice(0, -1)) {
    current = current.endsWith(sep) ? current + segment : current + sep + segment;
    chain.push(current);
  }
  return chain;
}

/** Path baru untuk `path` setelah `from` dipindah ke `to`; null bila `path` tidak ikut pindah. */
export function movedPath(path: string, from: string, to: string): string | null {
  if (path === from) return to;
  if (!isSameOrUnder(path, from)) return null;
  return to + path.slice(stripTrailing(from).length);
}

function lastSegment(path: string): string {
  const trimmed = stripTrailing(path);
  return trimmed.slice(Math.max(trimmed.lastIndexOf("/"), trimmed.lastIndexOf("\\")) + 1);
}

/** Tab berkas yang menunjuk `from` (atau berkas di dalamnya) ditulis ulang ke `to`; tab lain menghasilkan null. */
export function rewriteFileTab(tab: Tab, from: string, to: string): Tab | null {
  if (tab.kind !== "file" || typeof tab.params.path !== "string") return null;
  const moved = movedPath(tab.params.path, from, to);
  if (moved === null) return null;
  return { ...tab, id: `file:${moved}`, title: lastSegment(moved), description: moved, params: { ...tab.params, path: moved } };
}

// ----- Perataan pohon -----

export type Row =
  | { kind: "entry"; key: string; entry: DirEntry; depth: number; expanded: boolean; parent: string }
  /** Keterangan di bawah folder yang terbuka: sedang memuat, kosong, atau gagal dibaca. */
  | { kind: "note"; key: string; depth: number; parent: string; text: string; tone: "info" | "error" };

/** Meratakan pohon menjadi daftar baris yang terlihat (hanya isi folder yang terbuka). */
export function flattenTree(root: string, dirs: Record<string, DirState>, expanded: Record<string, boolean>): Row[] {
  const rows: Row[] = [];
  const visit = (dir: string, depth: number) => {
    const state = dirs[dir];
    if (!state || (!state.entries && !state.error)) {
      rows.push({ kind: "note", key: `note:${dir}`, depth, parent: dir, text: "Loading…", tone: "info" });
      return;
    }
    if (!state.entries) {
      rows.push({ kind: "note", key: `note:${dir}`, depth, parent: dir, text: state.error ?? "Could not read this folder.", tone: "error" });
      return;
    }
    if (state.entries.length === 0) {
      rows.push({ kind: "note", key: `note:${dir}`, depth, parent: dir, text: "Empty", tone: "info" });
      return;
    }
    for (const entry of state.entries) {
      const open = entry.isDir && expanded[entry.path] === true;
      rows.push({ kind: "entry", key: entry.path, entry, depth, expanded: open, parent: dir });
      if (open) visit(entry.path, depth + 1);
    }
  };
  visit(root, 0);
  return rows;
}

// ----- Validasi nama -----

export interface NameOptions {
  /** Izinkan `a/b/c.txt`: folder antara dibuat sekaligus. */
  nested?: boolean;
  /** Nama saat ini (ganti nama): tidak dianggap bentrok dengan dirinya sendiri. */
  current?: string;
}

/** Memeriksa nama berkas/folder baru. Mengembalikan pesan galat, atau null bila sah. */
export function validateEntryName(input: string, siblings: readonly DirEntry[] | null, options: NameOptions = {}): string | null {
  if (input.trim() === "") return "Enter a name.";
  if (input.includes("\0")) return "The name contains an invalid character.";
  if (input.includes("\\")) return "Backslashes are not allowed in names.";
  if (!options.nested && input.includes("/")) return 'Names cannot contain "/".';
  const segments = input.split("/");
  for (const segment of segments) {
    if (segment === "") return input.startsWith("/") ? "Enter a path relative to this folder." : "Path segments cannot be empty.";
    if (segment === "." || segment === "..") return '"." and ".." cannot be used as names.';
    if (segment !== segment.trim()) return "Names cannot start or end with a space.";
  }
  const first = segments[0];
  const clash = siblings?.find((s) => s.name === first);
  if (clash && first !== options.current) {
    if (segments.length === 1) return `A file or folder named "${first}" already exists in this location.`;
    if (!clash.isDir) return `"${first}" is a file, so nothing can be created inside it.`;
  }
  return null;
}

// ----- Dekorasi Git -----

export type GitStatus = "modified" | "added" | "untracked" | "deleted" | "renamed" | "conflict";

export const GIT_STATUS: Record<GitStatus, { letter: string; className: string; title: string }> = {
  modified: { letter: "M", className: "oc-git-modified", title: "Modified" },
  added: { letter: "A", className: "oc-git-added", title: "Added" },
  untracked: { letter: "U", className: "oc-git-untracked", title: "Untracked" },
  deleted: { letter: "D", className: "oc-git-deleted", title: "Deleted" },
  renamed: { letter: "R", className: "oc-git-renamed", title: "Renamed" },
  conflict: { letter: "C", className: "oc-git-conflict", title: "Conflict" },
};

const KIND_STATUS: Record<ChangeKind, GitStatus> = {
  added: "added",
  copied: "added",
  modified: "modified",
  typeChanged: "modified",
  deleted: "deleted",
  renamed: "renamed",
};

/** Bila satu berkas punya beberapa status (mis. staged dan unstaged), yang berperingkat tertinggi ditampilkan. */
const FILE_RANK: Record<GitStatus, number> = { modified: 1, renamed: 2, added: 3, untracked: 4, deleted: 5, conflict: 6 };

/** Warna folder: konflik mengalahkan perubahan, perubahan mengalahkan berkas baru. */
const FOLDER_STATUS: Record<GitStatus, GitStatus> = {
  conflict: "conflict",
  modified: "modified",
  renamed: "modified",
  deleted: "modified",
  added: "added",
  untracked: "untracked",
};
const FOLDER_RANK: Record<GitStatus, number> = { untracked: 1, added: 2, modified: 3, renamed: 3, deleted: 3, conflict: 4 };

export interface GitDecorations {
  /** Status per path absolut berkas. */
  files: Map<string, GitStatus>;
  /** Status gabungan per path absolut folder yang memuat perubahan. */
  folders: Map<string, GitStatus>;
}

/**
 * Memetakan snapshot repositori ke path absolut. Path di snapshot relatif terhadap akar
 * repositori (`snapshot.root`), yang belum tentu sama dengan folder proyek.
 */
export function buildDecorations(snapshot: RepoSnapshot | null): GitDecorations {
  const files = new Map<string, GitStatus>();
  const folders = new Map<string, GitStatus>();
  if (!snapshot) return { files, folders };

  const mark = (relative: string, status: GitStatus) => {
    const clean = relative.replace(/\/+$/, "");
    if (!clean) return;
    const absolute = joinPath(snapshot.root, clean);
    const previous = files.get(absolute);
    if (!previous || FILE_RANK[status] > FILE_RANK[previous]) files.set(absolute, status);
  };
  for (const change of snapshot.staged) mark(change.path, KIND_STATUS[change.kind]);
  for (const change of snapshot.unstaged) mark(change.path, KIND_STATUS[change.kind]);
  for (const path of snapshot.untracked) mark(path, "untracked");
  for (const conflict of snapshot.conflicted) mark(conflict.path, "conflict");

  for (const [path, status] of files) {
    const inherited = FOLDER_STATUS[status];
    // Semua folder di antara akar repositori dan berkas ikut diwarnai.
    for (const folder of ancestorChain(snapshot.root, path).slice(1)) {
      const previous = folders.get(folder);
      if (!previous || FOLDER_RANK[inherited] > FOLDER_RANK[previous]) folders.set(folder, inherited);
    }
  }
  return { files, folders };
}

export interface EntryDecoration {
  status: GitStatus;
  /** Huruf status; kosong untuk folder (hanya warna dan titik). */
  letter: string;
  className: string;
  title: string;
}

export function decorationFor(decorations: GitDecorations, entry: Pick<DirEntry, "path" | "isDir">): EntryDecoration | null {
  const own = decorations.files.get(entry.path);
  if (own && !entry.isDir) return { status: own, ...GIT_STATUS[own] };
  // Folder: statusnya sendiri (mis. submodule atau folder untracked) atau gabungan isinya.
  const status = entry.isDir ? (decorations.folders.get(entry.path) ?? own) : undefined;
  if (!status) return null;
  return { status, letter: "", className: GIT_STATUS[status].className, title: `Contains ${GIT_STATUS[status].title.toLowerCase()} files` };
}
