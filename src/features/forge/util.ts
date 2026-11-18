// Utilitas kecil fitur forge: penamaan, waktu relatif, URL aman, dan pesan galat.
import { openUrl } from "@tauri-apps/plugin-opener";
import { errorCode, errorMessage, type Account, type ForgeKind, type RepoRef } from "@/ipc";
import { showError } from "@/app/overlay";

/** Nama penyedia untuk ditampilkan. */
export function forgeName(kind: ForgeKind | null | undefined): string {
  return kind === "github" ? "GitHub" : kind === "gitlab" ? "GitLab" : "the Git host";
}

/** GitLab menyebutnya merge request; GitHub (dan host tak dikenal) pull request. */
export function prNoun(kind: ForgeKind | null | undefined, options: { plural?: boolean; lower?: boolean } = {}): string {
  const base = kind === "gitlab" ? "Merge Request" : "Pull Request";
  const text = options.plural ? `${base}s` : base;
  return options.lower ? text.toLowerCase() : text;
}

/** Singkatan: PR atau MR. */
export function prShort(kind: ForgeKind | null | undefined): string {
  return kind === "gitlab" ? "MR" : "PR";
}

/** Awalan nomor: `#12` di GitHub, `!12` di GitLab. */
export function prNumber(kind: ForgeKind | null | undefined, number: number): string {
  return `${kind === "gitlab" ? "!" : "#"}${number}`;
}

/** Nama host dari origin akun (mis. `https://github.com` → `github.com`). */
export function hostName(origin: string): string {
  const withoutScheme = origin.includes("://") ? origin.slice(origin.indexOf("://") + 3) : origin;
  return withoutScheme.split("/")[0].toLowerCase();
}

/** Argumen `host` untuk command login: null untuk github.com/gitlab.com, selain itu origin lengkap. */
export function loginHostFor(kind: ForgeKind, host: string | null | undefined): string | null {
  if (!host) return null;
  const name = hostName(host);
  if ((kind === "github" && name === "github.com") || (kind === "gitlab" && name === "gitlab.com")) return null;
  return host.includes("://") ? host.replace(/\/+$/, "") : `https://${name}`;
}

/**
 * Menormalkan masukan host self-hosted menjadi origin (dengan path bila instance berada di subpath).
 * Mengembalikan null bila bukan URL http(s) yang sah.
 */
export function normalizeHost(input: string): string | null {
  const raw = input.trim();
  if (!raw || /\s/.test(raw)) return null;
  const candidate = /^[a-z][a-z0-9+.-]*:\/\//i.test(raw) ? raw : `https://${raw}`;
  let url: URL;
  try {
    url = new URL(candidate);
  } catch {
    return null;
  }
  if (url.protocol !== "https:" && url.protocol !== "http:") return null;
  // Kredensial di URL tidak pernah diterima; token diminta lewat kolom tersendiri.
  if (!url.hostname || url.username || url.password || url.search || url.hash) return null;
  return `${url.protocol}//${url.host}${url.pathname.replace(/\/+$/, "")}`;
}

/** `owner/name` sebuah repositori. */
export function repoSlug(repo: RepoRef): string {
  return `${repo.owner}/${repo.name}`;
}

/** URL http(s) yang sudah dinormalkan, atau null untuk skema lain (javascript:, file:, relatif, …). */
export function safeExternalUrl(value: string | null | undefined): string | null {
  if (!value) return null;
  const trimmed = value.trim();
  if (!/^https?:\/\//i.test(trimmed)) return null;
  try {
    const url = new URL(trimmed);
    return url.protocol === "https:" || url.protocol === "http:" ? url.href : null;
  } catch {
    return null;
  }
}

/** URL gambar yang boleh dimuat: hanya `https:`. */
export function safeImageUrl(value: string | null | undefined): string | null {
  const url = safeExternalUrl(value);
  return url && url.startsWith("https://") ? url : null;
}

/** Membuka tautan di browser sistem. Skema selain http(s) diabaikan; WebView tidak pernah bernavigasi. */
export async function openExternal(value: string | null | undefined): Promise<boolean> {
  const url = safeExternalUrl(value);
  if (!url) return false;
  try {
    await openUrl(url);
    return true;
  } catch (e) {
    showError(e, "Could not open the browser");
    return false;
  }
}

const UNITS: [limit: number, seconds: number, name: string][] = [
  [60, 1, "second"],
  [3600, 60, "minute"],
  [86400, 3600, "hour"],
  [86400 * 30, 86400, "day"],
  [86400 * 365, 86400 * 30, "month"],
  [Infinity, 86400 * 365, "year"],
];

/** Waktu relatif singkat dari cap waktu Unix (detik), mis. "3 hours ago". */
export function relativeTime(unixSeconds: number, nowMs: number = Date.now()): string {
  if (!Number.isFinite(unixSeconds) || unixSeconds <= 0) return "";
  const diff = Math.round(nowMs / 1000 - unixSeconds);
  if (diff < 45) return diff < -45 ? "in the future" : "just now";
  for (const [limit, seconds, name] of UNITS) {
    if (diff < limit) {
      const n = Math.max(1, Math.round(diff / seconds));
      return `${n} ${name}${n === 1 ? "" : "s"} ago`;
    }
  }
  return "";
}

export interface ForgeErrorText {
  /** Kalimat utama (bahasa Inggris, ditulis di sini). */
  title: string;
  /** Pesan asli dari backend/penyedia, bila menambah informasi. */
  detail?: string;
}

/** Menerjemahkan kode galat forge menjadi pesan yang bisa ditindaklanjuti. Tidak pernah memuat token. */
export function describeForgeError(error: unknown, account?: Pick<Account, "host"> | null): ForgeErrorText {
  const code = errorCode(error);
  const message = errorMessage(error);
  const host = account ? hostName(account.host) : "the server";
  switch (code) {
    case "unauthorized":
      return { title: `Sign-in for ${host} is missing or has expired. Sign in again.`, detail: message };
    case "forbidden":
      return { title: `This account is not allowed to do that on ${host}. The token may be missing a required scope.`, detail: message };
    case "notFound":
      return { title: `Not found on ${host}, or this account has no access to it.`, detail: message };
    case "rateLimited":
      return { title: `The API rate limit of ${host} was reached. Try again in a few minutes.`, detail: message };
    case "network":
      return { title: `Could not reach ${host}. Check your connection and try again.`, detail: message };
    case "keychain":
      return { title: "The system keychain is unavailable or refused access, so the sign-in could not be stored.", detail: message };
    case "unsupported":
      return { title: "This is not supported by the host or not configured.", detail: message };
    case "invalidInput":
      return { title: `The request was rejected by ${host}.`, detail: message };
    case "invalidResponse":
      return { title: `${host} sent an unexpected answer.`, detail: message };
    default:
      return { title: message };
  }
}

/** Satu baris teks untuk toast. */
export function forgeErrorLine(error: unknown, account?: Pick<Account, "host"> | null): string {
  const text = describeForgeError(error, account);
  return text.detail && text.detail !== text.title ? `${text.title}\n${text.detail}` : text.title;
}
