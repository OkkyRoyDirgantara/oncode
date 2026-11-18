// Pemetaan path absolut editor ke path relatif repositori.

const isWindowsPath = (path: string) => /^[a-zA-Z]:[\\/]/.test(path) || path.startsWith("\\\\");

/**
 * Path `absolute` relatif terhadap akar repositori, dengan garis miring biasa.
 * Mengembalikan null bila berkas berada di luar repositori (atau adalah akarnya sendiri).
 */
export function repoRelativePath(root: string, absolute: string): string | null {
  const windows = isWindowsPath(root);
  const norm = (p: string) => p.replace(/\\/g, "/");
  const base = norm(root).replace(/\/+$/, "");
  const full = norm(absolute);
  // Sistem berkas Windows tidak membedakan huruf besar dan kecil.
  const prefix = windows ? base.toLowerCase() : base;
  const candidate = windows ? full.toLowerCase() : full;
  if (!candidate.startsWith(`${prefix}/`)) return null;
  const relative = full.slice(base.length + 1).replace(/^\/+/, "");
  return relative === "" ? null : relative;
}

/** Menggabungkan folder dan nama memakai pemisah yang dipakai folder itu. */
export function joinPath(dir: string, name: string): string {
  const sep = dir.includes("\\") && !dir.includes("/") ? "\\" : "/";
  return dir.replace(/[\\/]+$/, "") + sep + name;
}
