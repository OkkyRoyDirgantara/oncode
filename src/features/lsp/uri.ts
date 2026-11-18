// Konversi path berkas <-> URI `file://` untuk LSP, benar untuk path POSIX maupun Windows.

const WINDOWS_DRIVE = /^[a-zA-Z]:[\\/]/;
const UNC = /^\\\\[^\\/]+[\\/]/;

function isWindowsPath(path: string): boolean {
  return WINDOWS_DRIVE.test(path) || UNC.test(path);
}

function encodeSegment(segment: string): string {
  // encodeURIComponent membiarkan !'()* apa adanya; sisanya (spasi, #, ?, %, non-ASCII, ...) di-escape.
  return encodeURIComponent(segment);
}

/**
 * `/home/a b/x.rs` → `file:///home/a%20b/x.rs`; `C:\x\y` → `file:///C:/x/y`;
 * `\\server\share\x` → `file://server/share/x`.
 */
export function pathToUri(path: string): string {
  if (UNC.test(path)) {
    const [host, ...rest] = path.slice(2).split(/[\\/]+/);
    return `file://${host}/${rest.filter(Boolean).map(encodeSegment).join("/")}`;
  }
  if (WINDOWS_DRIVE.test(path)) {
    const segments = path.slice(2).split(/[\\/]+/).filter(Boolean);
    return `file:///${path.slice(0, 2)}/${segments.map(encodeSegment).join("/")}`;
  }
  // Di POSIX garis miring terbalik adalah karakter nama berkas biasa.
  const segments = path.split("/").map(encodeSegment);
  const joined = segments.join("/");
  return `file://${joined.startsWith("/") ? "" : "/"}${joined}`;
}

function decode(text: string): string {
  try {
    return decodeURIComponent(text);
  } catch {
    // Escape yang rusak: pakai apa adanya daripada gagal.
    return text;
  }
}

/**
 * Kebalikan `pathToUri`. Huruf drive Windows dinormalkan menjadi huruf besar (server sering
 * mengirim `file:///c%3A/...`). URI yang bukan `file:` dikembalikan apa adanya.
 */
export function uriToPath(uri: string): string {
  const match = /^file:\/\/([^/]*)(\/.*)?$/i.exec(uri);
  if (!match) return uri;
  const host = match[1];
  // Buang query dan fragment; `#` dan `?` di dalam nama berkas selalu ter-escape.
  const raw = (match[2] ?? "/").replace(/[?#].*$/, "");
  const decoded = raw.split("/").map(decode).join("/");
  if (host && host.toLowerCase() !== "localhost") return `\\\\${decode(host)}${decoded.replace(/\//g, "\\")}`;
  const drive = /^\/([a-zA-Z]):(\/.*)?$/.exec(decoded);
  if (drive) return `${drive[1].toUpperCase()}:${(drive[2] ?? "/").replace(/\//g, "\\")}`;
  return decoded;
}

/** Kunci pembanding path: path Windows tidak peka huruf besar/kecil dan pemisahnya disamakan. */
export function pathKey(path: string): string {
  return isWindowsPath(path) ? path.replace(/\//g, "\\").toLowerCase() : path;
}

/** Dua URI menunjuk berkas yang sama walau cara escape-nya berbeda. */
export function uriKey(uri: string): string {
  return pathKey(uriToPath(uri));
}
