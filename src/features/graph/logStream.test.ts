import { afterEach, describe, expect, it, vi } from "vitest";
import { clearMocks } from "@tauri-apps/api/mocks";
import { mockCommands, type GraphRow, type LogPage, type LogQuery } from "@/ipc";
import { LogController, PAGE_SIZE } from "./logStream";
import { linearRows, oid, ROOT } from "./testing";

afterEach(() => clearMocks());

/** Backend log palsu: tiap stream membaca `total` commit berurutan, seperti proses `git log` yang dibaca per halaman. */
function fakeLog(total: number) {
  let nextId = 1;
  const cursors = new Map<number, number>();
  const calls = {
    open: vi.fn((_: { root: string; query: LogQuery }) => {
      const id = nextId++;
      cursors.set(id, 0);
      return id;
    }),
    next: vi.fn(({ streamId, count }: { streamId: number; count: number }): LogPage => {
      const at = cursors.get(streamId);
      if (at === undefined) throw { code: "invalid", message: `stream ${streamId} is closed`, detail: null };
      const rows: GraphRow[] = linearRows(at, Math.min(count, total - at));
      cursors.set(streamId, at + rows.length);
      return { rows, done: at + rows.length >= total };
    }),
    close: vi.fn(({ streamId }: { streamId: number }) => {
      cursors.delete(streamId);
      return null;
    }),
  };
  mockCommands({ gitLogOpen: calls.open, gitLogNext: calls.next, gitLogClose: calls.close });
  return { ...calls, openStreams: () => [...cursors.keys()] };
}

describe("LogController", () => {
  it("opens the stream once with the query and loads the first page of 300", async () => {
    const log = fakeLog(1000);
    const controller = new LogController(ROOT, { all: true, grep: "fix" });
    expect(controller.getState()).toMatchObject({ rows: [], loading: true, done: false });

    await controller.open();

    expect(log.open).toHaveBeenCalledTimes(1);
    expect(log.open).toHaveBeenCalledWith({ root: ROOT, query: { all: true, grep: "fix" } });
    expect(log.next).toHaveBeenCalledTimes(1);
    expect(log.next).toHaveBeenCalledWith({ streamId: 1, count: PAGE_SIZE });
    const state = controller.getState();
    expect(state.rows).toHaveLength(300);
    expect(state.rows[0].commit.oid).toBe(oid(0));
    expect(state).toMatchObject({ loading: false, done: false, error: null });
    controller.dispose();
  });

  it("appends pages from the same stream until the backend reports done", async () => {
    const log = fakeLog(700);
    const controller = new LogController(ROOT, {});
    await controller.open();
    await controller.loadMore();
    expect(controller.getState().rows).toHaveLength(600);
    expect(controller.getState().done).toBe(false);

    await controller.loadMore();
    expect(controller.getState().rows).toHaveLength(700);
    expect(controller.getState().rows[699].commit.oid).toBe(oid(699));
    expect(controller.getState().done).toBe(true);

    // Setelah habis, tidak ada permintaan lagi.
    await controller.loadMore();
    expect(log.next).toHaveBeenCalledTimes(3);
    expect(log.open).toHaveBeenCalledTimes(1);
    controller.dispose();
  });

  it("does not request two pages at once", async () => {
    const log = fakeLog(2000);
    const controller = new LogController(ROOT, {});
    await controller.open();
    await Promise.all([controller.loadMore(), controller.loadMore(), controller.loadMore()]);
    expect(log.next).toHaveBeenCalledTimes(2);
    expect(controller.getState().rows).toHaveLength(600);
    controller.dispose();
  });

  it("notifies subscribers and stops after unsubscribe", async () => {
    fakeLog(10);
    const controller = new LogController(ROOT, {});
    const listener = vi.fn();
    const unsubscribe = controller.subscribe(listener);
    await controller.open();
    expect(listener).toHaveBeenCalled();
    const seen = listener.mock.calls.length;
    unsubscribe();
    await controller.open(true);
    expect(listener).toHaveBeenCalledTimes(seen);
    controller.dispose();
  });

  it("closes the stream on dispose", async () => {
    const log = fakeLog(1000);
    const controller = new LogController(ROOT, {});
    await controller.open();
    expect(log.openStreams()).toEqual([1]);

    controller.dispose();
    await vi.waitFor(() => expect(log.close).toHaveBeenCalledWith({ streamId: 1 }));
    expect(log.openStreams()).toEqual([]);

    // Controller yang sudah dilepas tidak membuka stream baru.
    await controller.open();
    await controller.loadMore();
    expect(log.open).toHaveBeenCalledTimes(1);
  });

  it("closes a stream that finishes opening after dispose (tab closed while loading)", async () => {
    const log = fakeLog(1000);
    const controller = new LogController(ROOT, {});
    const opening = controller.open();
    controller.dispose();
    await opening;
    await vi.waitFor(() => expect(log.close).toHaveBeenCalledWith({ streamId: 1 }));
    expect(log.openStreams()).toEqual([]);
    expect(log.next).not.toHaveBeenCalled();
    expect(controller.getState().rows).toEqual([]);
  });

  it("reload closes the old stream first and reloads as many rows as were shown", async () => {
    const log = fakeLog(2000);
    const controller = new LogController(ROOT, {});
    await controller.open();
    await controller.loadMore();
    await controller.loadMore();
    expect(controller.getState().rows).toHaveLength(900);
    const before = controller.getState().rows;

    const reloading = controller.open(true);
    // Baris lama tetap tampil selagi memuat ulang.
    expect(controller.getState().rows).toBe(before);
    expect(controller.getState().loading).toBe(true);
    await reloading;

    expect(log.close).toHaveBeenCalledWith({ streamId: 1 });
    expect(log.open).toHaveBeenCalledTimes(2);
    expect(log.openStreams()).toEqual([2]);
    expect(controller.getState().rows).toHaveLength(900);
    expect(controller.getState().rows).not.toBe(before);
    expect(controller.getState()).toMatchObject({ loading: false, done: false });

    // Halaman berikutnya datang dari stream yang baru.
    await controller.loadMore();
    expect(log.next).toHaveBeenLastCalledWith({ streamId: 2, count: PAGE_SIZE });
    expect(controller.getState().rows).toHaveLength(1200);
    controller.dispose();
  });

  it("ignores the result of a reload that was superseded by a newer one", async () => {
    const log = fakeLog(500);
    const controller = new LogController(ROOT, {});
    await controller.open();
    const first = controller.open(true);
    const second = controller.open(true);
    await Promise.all([first, second]);
    await vi.waitFor(() => expect(log.openStreams()).toHaveLength(1));
    expect(log.open).toHaveBeenCalledTimes(3);
    expect(controller.getState().rows).toHaveLength(300);
    expect(controller.getState().loading).toBe(false);
    controller.dispose();
  });

  it("reports an error and stops paging when the log cannot be opened", async () => {
    mockCommands({
      gitLogOpen: () => {
        throw { code: "unknown", message: "bad revision 'nope'", detail: null };
      },
    });
    const controller = new LogController(ROOT, { revs: ["nope"] });
    await controller.open();
    expect(controller.getState()).toMatchObject({ rows: [], loading: false, done: true, error: "bad revision 'nope'" });
  });

  it("keeps the rows already shown when a later page fails", async () => {
    const log = fakeLog(1000);
    const controller = new LogController(ROOT, {});
    await controller.open();
    log.next.mockImplementationOnce(() => {
      throw { code: "unknown", message: "git log died", detail: null };
    });
    await controller.loadMore();
    expect(controller.getState()).toMatchObject({ loading: false, done: true, error: "git log died" });
    expect(controller.getState().rows).toHaveLength(300);
    await vi.waitFor(() => expect(log.close).toHaveBeenCalledWith({ streamId: 1 }));
    controller.dispose();
  });

  describe("find", () => {
    it("returns the index right away when the commit is loaded", async () => {
      const log = fakeLog(1000);
      const controller = new LogController(ROOT, {});
      await controller.open();
      await expect(controller.find(oid(42))).resolves.toBe(42);
      expect(log.next).toHaveBeenCalledTimes(1);
      controller.dispose();
    });

    it("loads more pages until the commit shows up", async () => {
      const log = fakeLog(2000);
      const controller = new LogController(ROOT, {});
      await controller.open();
      await expect(controller.find(oid(950))).resolves.toBe(950);
      expect(log.next).toHaveBeenCalledTimes(4);
      expect(controller.getState().rows).toHaveLength(1200);
      controller.dispose();
    });

    it("waits for the first page when called while the stream is still opening", async () => {
      fakeLog(100);
      const controller = new LogController(ROOT, {});
      void controller.open();
      await expect(controller.find(oid(7))).resolves.toBe(7);
      controller.dispose();
    });

    it("returns -1 when the history ends without the commit", async () => {
      const log = fakeLog(450);
      const controller = new LogController(ROOT, {});
      await controller.open();
      await expect(controller.find("f".repeat(40))).resolves.toBe(-1);
      expect(controller.getState().done).toBe(true);
      expect(log.next).toHaveBeenCalledTimes(2);
      controller.dispose();
    });

    it("gives up at the cap instead of loading the whole history", async () => {
      const log = fakeLog(100000);
      const controller = new LogController(ROOT, {});
      await controller.open();
      await expect(controller.find("f".repeat(40), 900)).resolves.toBe(-1);
      expect(controller.getState().rows).toHaveLength(900);
      expect(log.next).toHaveBeenCalledTimes(3);
      controller.dispose();
    });

    it("returns -1 for a controller that was never opened or was disposed", async () => {
      fakeLog(100);
      const fresh = new LogController(ROOT, {});
      await expect(fresh.find(oid(1))).resolves.toBe(-1);
      const disposed = new LogController(ROOT, {});
      await disposed.open();
      disposed.dispose();
      await expect(disposed.find(oid(5000))).resolves.toBe(-1);
    });
  });
});
