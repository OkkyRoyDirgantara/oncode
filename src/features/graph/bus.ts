// Permintaan "tampilkan commit ini di Graph" dari luar tab Graph (sidebar, riwayat berkas, reflog).

export interface RevealTarget {
  oid: string;
  /** Nama yang disebut di pesan, mis. nama branch atau hash pendek. */
  label: string;
  /** Muat halaman berikutnya sampai commit ditemukan. Bila false, commit hanya dipilih. */
  hunt: boolean;
}

const pending = new Map<string, RevealTarget>();
const listeners = new Set<(root: string) => void>();

/** Menitipkan permintaan; tab Graph untuk `root` mengambilnya saat siap. */
export function requestReveal(root: string, target: RevealTarget): void {
  pending.set(root, target);
  listeners.forEach((l) => l(root));
}

export function takeReveal(root: string): RevealTarget | null {
  const target = pending.get(root) ?? null;
  pending.delete(root);
  return target;
}

export function onRevealRequested(listener: (root: string) => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}
