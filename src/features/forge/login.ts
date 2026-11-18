// Polling login device flow. Dipisah dari komponen agar bisa diuji dengan timer palsu.
import { errorCode, type Account, type LoginPoll } from "@/ipc";

/** Timer yang bisa diganti saat pengujian. */
export interface Timer {
  set(callback: () => void, ms: number): unknown;
  clear(handle: unknown): void;
  now(): number;
}

export const systemTimer: Timer = {
  set: (callback, ms) => setTimeout(callback, ms),
  clear: (handle) => clearTimeout(handle as ReturnType<typeof setTimeout>),
  now: () => Date.now(),
};

export type LoginOutcome =
  | { status: "done"; account: Account }
  | { status: "denied" }
  | { status: "expired" }
  | { status: "cancelled" }
  | { status: "error"; error: unknown };

export interface PollProgress {
  /** Jeda polling saat ini, dalam detik. */
  intervalSeconds: number;
  /** Jumlah polling yang sudah dijawab. */
  attempts: number;
  /** Polling terakhir gagal karena jaringan dan akan dicoba lagi. */
  retrying: boolean;
}

export interface DevicePollOptions {
  userCode: string;
  /** Jeda polling awal dari server, dalam detik. */
  interval: number;
  /** Masa berlaku kode, dalam detik. */
  expiresIn: number;
  poll: (userCode: string) => Promise<LoginPoll>;
  /** Memberi tahu backend bahwa login ini ditinggalkan. */
  cancel: (userCode: string) => Promise<unknown>;
  timer?: Timer;
  onProgress?: (progress: PollProgress) => void;
}

export interface DevicePollHandle {
  /** Selalu selesai (tidak pernah reject). */
  result: Promise<LoginOutcome>;
  /** Menghentikan polling dan membatalkan login di backend. Aman dipanggil berkali-kali. */
  cancel: () => void;
}

/** Server meminta jeda ditambah lima detik setiap kali menjawab `slow_down` (RFC 8628 §3.5). */
export const SLOW_DOWN_SECONDS = 5;
const MIN_INTERVAL_SECONDS = 1;

/**
 * Menjalankan polling sampai login selesai, ditolak, kedaluwarsa, atau dibatalkan.
 * Polling pertama terjadi setelah satu jeda, bukan seketika (sesuai aturan device flow).
 */
export function pollDeviceLogin(options: DevicePollOptions): DevicePollHandle {
  const timer = options.timer ?? systemTimer;
  const deadline = timer.now() + Math.max(0, options.expiresIn) * 1000;
  let intervalSeconds = Math.max(MIN_INTERVAL_SECONDS, options.interval);
  let attempts = 0;
  let settled = false;
  let handle: unknown = null;
  let resolve!: (outcome: LoginOutcome) => void;
  const result = new Promise<LoginOutcome>((r) => (resolve = r));

  const finish = (outcome: LoginOutcome) => {
    if (settled) return;
    settled = true;
    if (handle !== null) timer.clear(handle);
    handle = null;
    resolve(outcome);
  };

  const abandon = () => {
    // Backend membuang device code yang tersimpan; kegagalan di sini tidak penting bagi pengguna.
    void Promise.resolve()
      .then(() => options.cancel(options.userCode))
      .catch(() => undefined);
  };

  const schedule = () => {
    if (settled) return;
    const remaining = deadline - timer.now();
    if (remaining <= 0) {
      abandon();
      finish({ status: "expired" });
      return;
    }
    handle = timer.set(tick, Math.min(intervalSeconds * 1000, remaining));
  };

  const tick = () => {
    handle = null;
    if (settled) return;
    if (timer.now() >= deadline) {
      abandon();
      finish({ status: "expired" });
      return;
    }
    options.poll(options.userCode).then(
      (answer) => {
        if (settled) return;
        attempts += 1;
        switch (answer.status) {
          case "pending":
            options.onProgress?.({ intervalSeconds, attempts, retrying: false });
            schedule();
            return;
          case "slowDown":
            intervalSeconds += SLOW_DOWN_SECONDS;
            options.onProgress?.({ intervalSeconds, attempts, retrying: false });
            schedule();
            return;
          case "denied":
            finish({ status: "denied" });
            return;
          case "expired":
            finish({ status: "expired" });
            return;
          case "done":
            finish({ status: "done", account: answer.account });
            return;
        }
      },
      (error: unknown) => {
        if (settled) return;
        attempts += 1;
        // Galat jaringan tidak menggugurkan login; coba lagi pada jeda berikutnya.
        if (errorCode(error) === "network") {
          options.onProgress?.({ intervalSeconds, attempts, retrying: true });
          schedule();
          return;
        }
        finish({ status: "error", error });
      },
    );
  };

  schedule();

  return {
    result,
    cancel: () => {
      if (settled) return;
      abandon();
      finish({ status: "cancelled" });
    },
  };
}

/** Scope token yang dibutuhkan, untuk ditampilkan di dialog login. */
export const TOKEN_SCOPES = {
  github: ["repo", "read:user"],
  gitlab: ["api", "read_user"],
} as const;
