// Fungsi murni untuk dialog clone: nama folder dari URL dan validasi masukan.

/**
 * Nama folder bawaan untuk sebuah URL clone, seperti yang dipilih `git clone`:
 * segmen terakhir tanpa garis miring penutup dan tanpa akhiran `.git`.
 * Mendukung bentuk https/ssh/git/file maupun bentuk scp (`git@host:owner/repo.git`).
 */
export function repoNameFromUrl(url: string): string {
  let value = url.trim();
  // Query dan fragmen bukan bagian dari nama.
  value = value.replace(/[?#].*$/, "");
  value = value.replace(/[\\/]+$/, "");
  value = value.replace(/\.git$/i, "");
  value = value.replace(/[\\/]+$/, "");
  const cut = Math.max(value.lastIndexOf("/"), value.lastIndexOf("\\"), value.lastIndexOf(":"));
  const name = cut >= 0 ? value.slice(cut + 1) : value;
  // Sisa yang hanya berupa host (mis. "https://example.com") bukan nama repositori.
  if (/^[a-z][a-z0-9+.-]*:\/\/[^/]*$/i.test(value)) return "";
  return validateFolderName(name) === null ? name : "";
}

/** Pesan galat untuk nama folder tujuan, atau null bila sah. */
export function validateFolderName(name: string): string | null {
  if (name.trim() === "") return "Enter a folder name";
  if (name !== name.trim()) return "The name cannot start or end with a space";
  if (/[\\/]/.test(name)) return "The name cannot contain a path separator";
  if (name === "." || name === "..") return "This name is reserved";
  // eslint-disable-next-line no-control-regex
  if (/[\x00-\x1f\x7f]/.test(name)) return "The name cannot contain control characters";
  if (/[<>:"|?*]/.test(name)) return 'The name cannot contain any of < > : " | ? *';
  if (name.startsWith("-")) return "The name cannot start with a dash";
  if (name.endsWith(".")) return "The name cannot end with a dot";
  if (name.length > 255) return "The name is too long";
  return null;
}

/** Pesan galat untuk URL clone, atau null bila bentuknya masuk akal. Git sendiri yang memeriksa sisanya. */
export function validateCloneUrl(url: string): string | null {
  const value = url.trim();
  if (value === "") return "Enter a repository URL";
  // eslint-disable-next-line no-control-regex
  if (/[\s\x00-\x1f\x7f]/.test(value)) return "The URL cannot contain spaces";
  if (value.startsWith("-")) return "The URL cannot start with a dash";
  // Bentuk `transport::alamat` (mis. `ext::`) menjalankan program pembantu; tidak pernah diterima dari sini.
  if (/^[a-z0-9+.-]+::/i.test(value)) return "This kind of URL is not supported";
  const scheme = /^([a-z][a-z0-9+.-]*):\/\//i.exec(value);
  if (scheme) {
    if (!["https", "http", "ssh", "git", "file"].includes(scheme[1].toLowerCase())) return "Use an https://, ssh:// or git:// URL";
    return null;
  }
  // Bentuk scp: [user@]host:path
  if (/^[^/\\:@]+@[^/\\:]+:.+/.test(value) || /^[^/\\:@]+\.[^/\\:@]+:.+/.test(value)) return null;
  // Path lokal absolut.
  if (/^(\/|[a-zA-Z]:[\\/]|~\/)/.test(value)) return null;
  return "Enter a full URL, for example https://github.com/owner/repo.git";
}

/** Menggabungkan folder induk dan nama untuk ditampilkan. */
export function joinPath(parent: string, name: string): string {
  if (!parent) return name;
  const sep = parent.includes("\\") && !parent.includes("/") ? "\\" : "/";
  return parent.replace(/[\\/]+$/, "") + sep + name;
}
