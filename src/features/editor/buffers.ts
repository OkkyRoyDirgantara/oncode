// Buffer berkas: satu dokumen per path, dipakai bersama oleh semua tab/split yang menampilkannya.
import { Text } from "@codemirror/state";
import type { EditorView } from "@codemirror/view";
import { create } from "zustand";
import { commands, errorMessage, type Eol } from "@/ipc";

export type BufferStatus = "loading" | "ready" | "binary" | "tooLarge" | "error";

export interface Buffer {
  path: string;
  status: BufferStatus;
  error?: string;
  /** Isi terkini. */
  doc: Text;
  /** Isi saat terakhir dimuat atau disimpan; pembanding untuk tanda "belum disimpan". */
  savedDoc: Text;
  encoding: string;
  bom: boolean;
  eol: Eol;
  mtimeMs: number;
  size: number;
  readOnly: boolean;
  views: Set<EditorView>;
  /** Tab yang menampilkan buffer ini; buffer dilepas saat mencapai nol. */
  refs: number;
}

export interface BufferMeta {
  status: BufferStatus;
  dirty: boolean;
  error?: string;
  encoding: string;
  eol: Eol;
  readOnly: boolean;
  /** Berkas berubah di disk sementara buffer berisi perubahan yang belum disimpan. */
  conflict: boolean;
  /** Bertambah setiap kali isi dimuat ulang dari disk. */
  revision: number;
}

const buffers = new Map<string, Buffer>();
/** Pemuatan yang sedang berjalan, terikat pada objek buffer-nya (bukan hanya path). */
const loading = new Map<string, { buffer: Buffer; promise: Promise<Buffer> }>();

export const useBuffers = create<{ meta: Record<string, BufferMeta> }>(() => ({ meta: {} }));

/** Batas ukuran untuk fitur berat (highlight, lipat, LSP). */
export const LARGE_FILE_BYTES = 5 * 1024 * 1024;

function publish(buffer: Buffer, patch: Partial<BufferMeta> = {}) {
  useBuffers.setState((s) => {
    const previous = s.meta[buffer.path];
    const next: BufferMeta = {
      status: buffer.status,
      dirty: buffer.status === "ready" && !buffer.doc.eq(buffer.savedDoc),
      error: buffer.error,
      encoding: buffer.encoding,
      eol: buffer.eol,
      readOnly: buffer.readOnly,
      conflict: previous?.conflict ?? false,
      revision: previous?.revision ?? 0,
      ...patch,
    };
    if (previous && (Object.keys(next) as (keyof BufferMeta)[]).every((k) => previous[k] === next[k])) return s;
    return { meta: { ...s.meta, [buffer.path]: next } };
  });
}

function emptyBuffer(path: string): Buffer {
  return {
    path,
    status: "loading",
    doc: Text.empty,
    savedDoc: Text.empty,
    encoding: "UTF-8",
    bom: false,
    eol: "lf",
    mtimeMs: 0,
    size: 0,
    readOnly: false,
    views: new Set(),
    refs: 0,
  };
}

async function load(buffer: Buffer): Promise<void> {
  try {
    const opened = await commands.fsOpen(buffer.path);
    if (opened.kind === "text") {
      const doc = Text.of(opened.text.split("\n"));
      buffer.doc = doc;
      buffer.savedDoc = doc;
      buffer.encoding = opened.encoding;
      buffer.bom = opened.bom;
      buffer.eol = opened.eol;
      buffer.mtimeMs = opened.stat.mtimeMs;
      buffer.size = opened.stat.size;
      buffer.readOnly = opened.stat.readOnly;
      buffer.status = "ready";
      buffer.error = undefined;
    } else {
      buffer.status = opened.kind === "binary" ? "binary" : "tooLarge";
      buffer.size = opened.size;
    }
  } catch (e) {
    buffer.status = "error";
    buffer.error = errorMessage(e);
  }
}

/** Memuat buffer (sekali) dan mengembalikannya. */
export function ensureBuffer(path: string): Promise<Buffer> {
  const existing = buffers.get(path);
  if (existing && existing.status !== "loading") return Promise.resolve(existing);
  const pending = loading.get(path);
  // Pemuatan lama untuk buffer yang sudah dilepas tidak boleh dipakai untuk buffer baru di path yang sama.
  if (pending && pending.buffer === existing) return pending.promise;
  const buffer = existing ?? emptyBuffer(path);
  buffers.set(path, buffer);
  publish(buffer);
  const promise = load(buffer).then(() => {
    if (loading.get(path)?.buffer === buffer) loading.delete(path);
    // Buffer bisa sudah dilepas (tab ditutup) atau diganti selagi dimuat.
    if (buffers.get(path) === buffer) publish(buffer, { revision: (useBuffers.getState().meta[path]?.revision ?? 0) + 1 });
    return buffer;
  });
  loading.set(path, { buffer, promise });
  return promise;
}

export function getBuffer(path: string): Buffer | undefined {
  return buffers.get(path);
}

export function openBuffers(): Buffer[] {
  return [...buffers.values()];
}

export function retainBuffer(path: string): void {
  const buffer = buffers.get(path) ?? emptyBuffer(path);
  buffers.set(path, buffer);
  buffer.refs += 1;
}

export function releaseBuffer(path: string): void {
  const buffer = buffers.get(path);
  if (!buffer) return;
  buffer.refs -= 1;
  if (buffer.refs <= 0) {
    buffers.delete(path);
    loading.delete(path);
    useBuffers.setState((s) => {
      const { [path]: _removed, ...rest } = s.meta;
      return { meta: rest };
    });
  }
}

/** Dipanggil editor setiap kali isi dokumen berubah. */
export function setBufferDoc(path: string, doc: Text): void {
  const buffer = buffers.get(path);
  if (!buffer) return;
  buffer.doc = doc;
  publish(buffer);
}

export function isDirty(path: string): boolean {
  return useBuffers.getState().meta[path]?.dirty ?? false;
}

export function dirtyPaths(): string[] {
  return Object.entries(useBuffers.getState().meta)
    .filter(([, m]) => m.dirty)
    .map(([path]) => path);
}

/** Menyimpan buffer ke disk dengan encoding dan akhir baris aslinya. */
export async function saveBuffer(path: string): Promise<boolean> {
  const buffer = buffers.get(path);
  if (!buffer || buffer.status !== "ready") return false;
  const doc = buffer.doc;
  const stat = await commands.fsSave(path, doc.sliceString(0, doc.length, "\n"), buffer.encoding, buffer.bom, buffer.eol);
  buffer.savedDoc = doc;
  buffer.mtimeMs = stat.mtimeMs;
  buffer.size = stat.size;
  publish(buffer, { conflict: false });
  window.dispatchEvent(new CustomEvent("oncode:saved", { detail: { path } }));
  return true;
}

/** Mengganti isi semua editor buffer ini dengan `text` (mis. setelah dimuat ulang dari disk). */
function replaceInViews(buffer: Buffer, doc: Text) {
  for (const view of buffer.views) {
    view.dispatch({ changes: { from: 0, to: view.state.doc.length, insert: doc }, userEvent: "oncode.reload" });
  }
}

/** Memuat ulang dari disk, membuang perubahan yang belum disimpan. */
export async function reloadBuffer(path: string): Promise<void> {
  const buffer = buffers.get(path);
  if (!buffer) return;
  await load(buffer);
  if (buffer.status === "ready") replaceInViews(buffer, buffer.doc);
  publish(buffer, { conflict: false, revision: (useBuffers.getState().meta[path]?.revision ?? 0) + 1 });
}

/** Menanggapi perubahan berkas dari luar: buffer bersih dimuat ulang, buffer kotor ditandai konflik. */
export async function handleExternalChange(path: string): Promise<"reloaded" | "conflict" | "unchanged" | "deleted"> {
  const buffer = buffers.get(path);
  if (!buffer || buffer.status === "loading") return "unchanged";
  let stat;
  try {
    stat = await commands.fsStat(path);
  } catch {
    return "unchanged";
  }
  if (!stat) return "deleted";
  if (stat.mtimeMs === buffer.mtimeMs && stat.size === buffer.size) return "unchanged";
  if (buffer.status !== "ready" || buffer.doc.eq(buffer.savedDoc)) {
    await reloadBuffer(path);
    return "reloaded";
  }
  buffer.mtimeMs = stat.mtimeMs;
  buffer.size = stat.size;
  publish(buffer, { conflict: true });
  return "conflict";
}

/** Memindahkan buffer ke path baru setelah berkas diganti nama. */
export function renameBuffer(from: string, to: string): void {
  const buffer = buffers.get(from);
  if (!buffer) return;
  buffers.delete(from);
  buffer.path = to;
  // Tab berganti id sehingga dipasang ulang: yang lama melepas path lama (tidak berpengaruh),
  // yang baru menahan path baru. Hitungan dimulai dari nol agar tidak berlipat.
  buffer.refs = 0;
  buffers.set(to, buffer);
  useBuffers.setState((s) => {
    const { [from]: meta, ...rest } = s.meta;
    return { meta: meta ? { ...rest, [to]: meta } : rest };
  });
}

export function bufferText(path: string): string | null {
  const buffer = buffers.get(path);
  return buffer && buffer.status === "ready" ? buffer.doc.toString() : null;
}

/** Hanya untuk pengujian. */
export function resetBuffersForTest(): void {
  buffers.clear();
  loading.clear();
  useBuffers.setState({ meta: {} });
}
