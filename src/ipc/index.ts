// Satu-satunya pintu ke backend. Semua pemanggilan IPC melewati modul ini,
// sehingga bila tauri-specta harus diganti cukup modul ini yang berubah.
export * from "./bindings";
import type { AppError } from "./bindings";

export function isAppError(e: unknown): e is AppError {
  return typeof e === "object" && e !== null && "code" in e && "message" in e;
}

/** Pesan galat yang layak ditampilkan ke pengguna. */
export function errorMessage(e: unknown): string {
  if (isAppError(e)) return e.message;
  if (e instanceof Error) return e.message;
  return typeof e === "string" ? e : JSON.stringify(e);
}

/** Rincian tambahan (mis. stderr git), bila ada dan berbeda dari pesan utama. */
export function errorDetail(e: unknown): string | undefined {
  if (isAppError(e) && e.detail && e.detail.trim() !== e.message.trim()) return e.detail;
  return undefined;
}

export function errorCode(e: unknown): string | null {
  return isAppError(e) ? e.code : null;
}

let counter = 0;
/** Id unik untuk operasi yang bisa dibatalkan lewat `commands.appCancel`. */
export function newOperationId(): string {
  counter += 1;
  return `op-${Date.now().toString(36)}-${counter}`;
}
