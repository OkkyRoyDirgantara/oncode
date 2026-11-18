import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Account, LoginPoll } from "@/ipc";
import { pollDeviceLogin, SLOW_DOWN_SECONDS, type LoginOutcome, type Timer } from "./login";

const account: Account = { id: "github:github.com:octocat", kind: "github", host: "https://github.com", username: "octocat", displayName: "The Octocat", avatarUrl: null };

/** Menjawab polling dari antrean; setelah habis, selalu `pending`. */
function scripted(answers: (LoginPoll | Error | { code: string; message: string; detail: null })[]) {
  const queue = [...answers];
  return vi.fn(async (_code: string): Promise<LoginPoll> => {
    const next = queue.shift() ?? { status: "pending" as const };
    if (next instanceof Error || "code" in next) throw next;
    return next;
  });
}

function settled(promise: Promise<LoginOutcome>): { value: LoginOutcome | null } {
  const box: { value: LoginOutcome | null } = { value: null };
  void promise.then((v) => (box.value = v));
  return box;
}

describe("pollDeviceLogin", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it("menunggu satu jeda sebelum polling pertama, lalu selesai saat server menjawab done", async () => {
    const poll = scripted([{ status: "pending" }, { status: "done", account }]);
    const cancel = vi.fn(async () => null);
    const handle = pollDeviceLogin({ userCode: "ABCD-1234", interval: 5, expiresIn: 900, poll, cancel });
    const outcome = settled(handle.result);

    await vi.advanceTimersByTimeAsync(4999);
    expect(poll).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(1);
    expect(poll).toHaveBeenCalledTimes(1);
    expect(poll).toHaveBeenCalledWith("ABCD-1234");
    expect(outcome.value).toBeNull();

    await vi.advanceTimersByTimeAsync(5000);
    expect(poll).toHaveBeenCalledTimes(2);
    expect(outcome.value).toEqual({ status: "done", account });
    // Login yang berhasil tidak dibatalkan, dan tidak ada polling lagi setelahnya.
    expect(cancel).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(60_000);
    expect(poll).toHaveBeenCalledTimes(2);
    expect(vi.getTimerCount()).toBe(0);
  });

  it("slowDown menambah jeda lima detik untuk semua polling berikutnya", async () => {
    const poll = scripted([{ status: "slowDown" }, { status: "pending" }, { status: "slowDown" }]);
    const progress = vi.fn();
    const handle = pollDeviceLogin({ userCode: "X", interval: 5, expiresIn: 900, poll, cancel: async () => null, onProgress: progress });

    await vi.advanceTimersByTimeAsync(5000);
    expect(poll).toHaveBeenCalledTimes(1);
    expect(progress).toHaveBeenLastCalledWith({ intervalSeconds: 5 + SLOW_DOWN_SECONDS, attempts: 1, retrying: false });

    // Jeda lama (5 s) belum cukup; yang baru 10 s.
    await vi.advanceTimersByTimeAsync(5000);
    expect(poll).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(5000);
    expect(poll).toHaveBeenCalledTimes(2);

    // Tetap 10 s setelah jawaban pending, lalu 15 s setelah slowDown kedua.
    await vi.advanceTimersByTimeAsync(10_000);
    expect(poll).toHaveBeenCalledTimes(3);
    await vi.advanceTimersByTimeAsync(14_999);
    expect(poll).toHaveBeenCalledTimes(3);
    await vi.advanceTimersByTimeAsync(1);
    expect(poll).toHaveBeenCalledTimes(4);
    handle.cancel();
  });

  it("berhenti saat ditolak", async () => {
    const poll = scripted([{ status: "denied" }]);
    const handle = pollDeviceLogin({ userCode: "X", interval: 5, expiresIn: 900, poll, cancel: async () => null });
    const outcome = settled(handle.result);
    await vi.advanceTimersByTimeAsync(5000);
    expect(outcome.value).toEqual({ status: "denied" });
    await vi.advanceTimersByTimeAsync(60_000);
    expect(poll).toHaveBeenCalledTimes(1);
  });

  it("berhenti saat server menyatakan kode kedaluwarsa", async () => {
    const poll = scripted([{ status: "pending" }, { status: "expired" }]);
    const handle = pollDeviceLogin({ userCode: "X", interval: 5, expiresIn: 900, poll, cancel: async () => null });
    const outcome = settled(handle.result);
    await vi.advanceTimersByTimeAsync(10_000);
    expect(outcome.value).toEqual({ status: "expired" });
    expect(vi.getTimerCount()).toBe(0);
  });

  it("berhenti sendiri saat expiresIn terlewati dan memberi tahu backend", async () => {
    const poll = scripted([]);
    const cancel = vi.fn(async () => null);
    const handle = pollDeviceLogin({ userCode: "CODE", interval: 5, expiresIn: 12, poll, cancel });
    const outcome = settled(handle.result);

    await vi.advanceTimersByTimeAsync(10_000);
    expect(poll).toHaveBeenCalledTimes(2);
    expect(outcome.value).toBeNull();

    // Batas 12 s: tidak ada polling ketiga, hasilnya kedaluwarsa.
    await vi.advanceTimersByTimeAsync(2000);
    expect(outcome.value).toEqual({ status: "expired" });
    expect(poll).toHaveBeenCalledTimes(2);
    expect(cancel).toHaveBeenCalledWith("CODE");
    await vi.advanceTimersByTimeAsync(60_000);
    expect(poll).toHaveBeenCalledTimes(2);
  });

  it("pembatalan menghentikan polling dan memanggil cancel sekali", async () => {
    const poll = scripted([]);
    const cancel = vi.fn(async () => null);
    const handle = pollDeviceLogin({ userCode: "CODE", interval: 5, expiresIn: 900, poll, cancel });
    const outcome = settled(handle.result);

    await vi.advanceTimersByTimeAsync(5000);
    expect(poll).toHaveBeenCalledTimes(1);
    handle.cancel();
    handle.cancel();
    await vi.advanceTimersByTimeAsync(0);
    expect(outcome.value).toEqual({ status: "cancelled" });
    expect(cancel).toHaveBeenCalledTimes(1);
    expect(cancel).toHaveBeenCalledWith("CODE");

    await vi.advanceTimersByTimeAsync(120_000);
    expect(poll).toHaveBeenCalledTimes(1);
    expect(vi.getTimerCount()).toBe(0);
  });

  it("jawaban polling yang datang setelah pembatalan diabaikan", async () => {
    let release!: (value: LoginPoll) => void;
    const poll = vi.fn(() => new Promise<LoginPoll>((r) => (release = r)));
    const handle = pollDeviceLogin({ userCode: "X", interval: 1, expiresIn: 900, poll, cancel: async () => null });
    const outcome = settled(handle.result);
    await vi.advanceTimersByTimeAsync(1000);
    expect(poll).toHaveBeenCalledTimes(1);
    handle.cancel();
    release({ status: "done", account });
    await vi.advanceTimersByTimeAsync(10_000);
    expect(outcome.value).toEqual({ status: "cancelled" });
    expect(poll).toHaveBeenCalledTimes(1);
  });

  it("galat jaringan dicoba lagi; galat lain menghentikan polling", async () => {
    const poll = scripted([{ code: "network", message: "offline", detail: null }, { code: "unauthorized", message: "bad client", detail: null }]);
    const progress = vi.fn();
    const handle = pollDeviceLogin({ userCode: "X", interval: 5, expiresIn: 900, poll, cancel: async () => null, onProgress: progress });
    const outcome = settled(handle.result);

    await vi.advanceTimersByTimeAsync(5000);
    expect(progress).toHaveBeenLastCalledWith({ intervalSeconds: 5, attempts: 1, retrying: true });
    expect(outcome.value).toBeNull();

    await vi.advanceTimersByTimeAsync(5000);
    expect(outcome.value).toEqual({ status: "error", error: { code: "unauthorized", message: "bad client", detail: null } });
    await vi.advanceTimersByTimeAsync(60_000);
    expect(poll).toHaveBeenCalledTimes(2);
  });

  it("memakai timer yang disuntikkan", async () => {
    vi.useRealTimers();
    let now = 0;
    const pending: { callback: () => void; at: number }[] = [];
    const timer: Timer = {
      set: (callback, ms) => {
        const entry = { callback, at: now + ms };
        pending.push(entry);
        return entry;
      },
      clear: (handle) => {
        const index = pending.indexOf(handle as (typeof pending)[number]);
        if (index >= 0) pending.splice(index, 1);
      },
      now: () => now,
    };
    const poll = scripted([{ status: "done", account }]);
    const handle = pollDeviceLogin({ userCode: "X", interval: 7, expiresIn: 60, poll, cancel: async () => null, timer });
    expect(pending.map((p) => p.at)).toEqual([7000]);
    now = 7000;
    pending.shift()!.callback();
    await expect(handle.result).resolves.toEqual({ status: "done", account });
    expect(pending).toEqual([]);
  });
});
