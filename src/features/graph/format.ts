// Format tanggal dan hash untuk daftar commit.

/** Waktu relatif ringkas dari detik Unix, mis. "5m ago", "3d ago", "2y ago". */
export function relativeDate(seconds: number, nowMs: number = Date.now()): string {
  const diff = Math.floor(nowMs / 1000 - seconds);
  if (!Number.isFinite(diff)) return "";
  if (diff < 60) return "just now";
  if (diff < 3600) return `${Math.floor(diff / 60)}m ago`;
  if (diff < 86400) return `${Math.floor(diff / 3600)}h ago`;
  const days = Math.floor(diff / 86400);
  if (days < 30) return `${days}d ago`;
  if (days < 365) return `${Math.floor(days / 30)}mo ago`;
  return `${Math.floor(days / 365)}y ago`;
}

/** Tanggal dan jam lengkap menurut locale pengguna. */
export function fullDate(seconds: number): string {
  return new Date(seconds * 1000).toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" });
}

export function shortOid(oid: string): string {
  return oid.slice(0, 8);
}

export function plural(n: number, word: string): string {
  return `${n} ${word}${n === 1 ? "" : "s"}`;
}
