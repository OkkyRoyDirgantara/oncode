import { afterEach, describe, expect, it, vi } from "vitest";
import { clearMocks } from "@tauri-apps/api/mocks";
import type { Channel } from "@tauri-apps/api/core";
import type { Transport } from "@codemirror/lsp-client";
import { mockCommands, type LspEvent } from "@/ipc";
import { ServerTransport, openTransport, watchResponses, type ServerExit } from "./transport";

const flush = () => new Promise((resolve) => setTimeout(resolve, 0));

function backend(options: { startGate?: Promise<void> } = {}) {
  const sent: [number, string][] = [];
  let channel: Channel<LspEvent> | null = null;
  let gate: Promise<void> | null = null;
  const started: string[] = [];
  mockCommands({
    lspStart: async ({ language, onEvent }) => {
      started.push(language);
      channel = onEvent;
      if (options.startGate) await options.startGate;
      return 7;
    },
    lspSend: async ({ id, json }) => {
      sent.push([id, json]);
      if (gate) await gate;
      return null;
    },
  });
  return {
    sent,
    started,
    emit: (event: LspEvent) => channel!.onmessage(event),
    hold() {
      let release!: () => void;
      gate = new Promise((resolve) => (release = resolve));
      return () => {
        gate = null;
        release();
      };
    },
  };
}

describe("transport LSP", () => {
  afterEach(() => clearMocks());

  it("memulai server untuk bahasanya dan mengirim pesan lewat lspSend dengan id dari backend", async () => {
    const be = backend();
    const transport = await openTransport("rust", () => {});
    expect(be.started).toEqual(["rust"]);
    expect(transport.id).toBe(7);
    transport.send('{"a":1}');
    await flush();
    expect(be.sent).toEqual([[7, '{"a":1}']]);
  });

  it("mengirim satu per satu sesuai urutan pemanggilan", async () => {
    const be = backend();
    const transport = await openTransport("rust", () => {});
    const release = be.hold();
    transport.send("1");
    transport.send("2");
    transport.send("3");
    await flush();
    // Pesan kedua tidak dikirim sebelum yang pertama selesai.
    expect(be.sent.map((s) => s[1])).toEqual(["1"]);
    release();
    await flush();
    await flush();
    expect(be.sent.map((s) => s[1])).toEqual(["1", "2", "3"]);
  });

  it("meneruskan pesan dari channel ke semua pendengar dan berhenti setelah unsubscribe", async () => {
    const be = backend();
    const transport = await openTransport("rust", () => {});
    const a: string[] = [];
    const b: string[] = [];
    const onA = (m: string) => a.push(m);
    const onB = (m: string) => b.push(m);
    transport.subscribe(onA);
    transport.subscribe(onB);
    be.emit({ kind: "message", json: "one" });
    transport.unsubscribe(onA);
    be.emit({ kind: "message", json: "two" });
    expect(a).toEqual(["one"]);
    expect(b).toEqual(["one", "two"]);
  });

  it("pesan yang tiba sebelum ada pendengar disimpan lalu diserahkan berurutan", async () => {
    const be = backend();
    const transport = await openTransport("rust", () => {});
    be.emit({ kind: "message", json: "early-1" });
    be.emit({ kind: "message", json: "early-2" });
    const got: string[] = [];
    transport.subscribe((m) => got.push(m));
    be.emit({ kind: "message", json: "later" });
    expect(got).toEqual(["early-1", "early-2", "later"]);
  });

  it("melaporkan proses yang berakhir, juga bila terjadi sebelum lspStart selesai", async () => {
    let release!: () => void;
    const be = backend({ startGate: new Promise((resolve) => (release = resolve)) });
    const exits: ServerExit[] = [];
    const pending = openTransport("rust", (exit) => exits.push(exit));
    await flush();
    be.emit({ kind: "exit", code: 1, stderrTail: "error: not installed" });
    expect(exits).toEqual([{ code: 1, stderrTail: "error: not installed" }]);
    release();
    expect((await pending).id).toBe(7);
  });

  it("setelah ditutup: permintaan ditolak, notifikasi dan pesan masuk dibuang, antrean tidak dikirim", async () => {
    const be = backend();
    const transport = await openTransport("rust", () => {});
    const got: string[] = [];
    transport.subscribe((m) => got.push(m));
    const release = be.hold();
    transport.send("1");
    transport.send("2");
    await flush();
    transport.close();
    release();
    await flush();
    expect(be.sent.map((s) => s[1])).toEqual(["1"]);
    // Permintaan ditolak seketika; notifikasi (yang dikirim klien tanpa penangan galat) dibuang diam-diam.
    expect(() => transport.send('{"jsonrpc":"2.0","id":9,"method":"textDocument/hover","params":{}}')).toThrow(/not running/);
    expect(() => transport.send('{"jsonrpc":"2.0","method":"textDocument/didClose","params":{}}')).not.toThrow();
    await flush();
    expect(be.sent.map((s) => s[1])).toEqual(["1"]);
    be.emit({ kind: "message", json: "late" });
    expect(got).toEqual([]);
    expect(transport.isClosed).toBe(true);
  });

  it("kegagalan lspSend tidak menghentikan antrean", async () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    const sent: string[] = [];
    mockCommands({
      lspSend: ({ json }) => {
        sent.push(json);
        if (json === "bad") throw { code: "io", message: "broken pipe", detail: null };
        return null;
      },
    });
    const transport = new ServerTransport();
    transport.send("queued-before-start");
    expect(sent).toEqual([]);
    transport.start(3);
    transport.send("bad");
    transport.send("after");
    await flush();
    await flush();
    expect(sent).toEqual(["queued-before-start", "bad", "after"]);
    expect(warn).toHaveBeenCalled();
    warn.mockRestore();
  });
});

describe("watchResponses", () => {
  function memoryTransport() {
    const sent: string[] = [];
    let handlers: ((m: string) => void)[] = [];
    const transport: Transport = {
      send: (m) => void sent.push(m),
      subscribe: (h) => void handlers.push(h),
      unsubscribe: (h) => void (handlers = handlers.filter((x) => x !== h)),
    };
    return { transport, sent, deliver: (m: unknown) => handlers.forEach((h) => h(JSON.stringify(m))), count: () => handlers.length };
  }

  it("melaporkan hasil permintaan yang diawasi setelah klien menerimanya, tanpa mengubah pesan", () => {
    const inner = memoryTransport();
    const results: unknown[] = [];
    const order: string[] = [];
    const watched = watchResponses(inner.transport, "textDocument/rename", (result) => {
      results.push(result);
      order.push("watcher");
    });
    const received: string[] = [];
    watched.subscribe((m) => {
      received.push(m);
      order.push("client");
    });

    watched.send(JSON.stringify({ jsonrpc: "2.0", id: 4, method: "textDocument/hover", params: {} }));
    watched.send(JSON.stringify({ jsonrpc: "2.0", id: 5, method: "textDocument/rename", params: { newName: "x" } }));
    expect(inner.sent).toHaveLength(2);

    inner.deliver({ jsonrpc: "2.0", id: 4, result: { contents: "doc" } });
    expect(results).toEqual([]);
    inner.deliver({ jsonrpc: "2.0", id: 5, result: { changes: { "file:///a": [] } } });
    expect(results).toEqual([{ changes: { "file:///a": [] } }]);
    expect(order).toEqual(["client", "client", "watcher"]);
    expect(received).toHaveLength(2);
    // Jawaban yang sama tidak dilaporkan dua kali.
    inner.deliver({ jsonrpc: "2.0", id: 5, result: { changes: {} } });
    expect(results).toHaveLength(1);
  });

  it("mengabaikan jawaban galat/null dan notifikasi, dan unsubscribe melepas pembungkusnya", () => {
    const inner = memoryTransport();
    const results: unknown[] = [];
    const watched = watchResponses(inner.transport, "textDocument/rename", (r) => results.push(r));
    const handler = () => {};
    watched.subscribe(handler);
    watched.send(JSON.stringify({ jsonrpc: "2.0", id: 1, method: "textDocument/rename", params: {} }));
    watched.send(JSON.stringify({ jsonrpc: "2.0", id: 2, method: "textDocument/rename", params: {} }));
    inner.deliver({ jsonrpc: "2.0", method: "textDocument/rename", params: {} });
    inner.deliver({ jsonrpc: "2.0", id: 1, error: { code: -1, message: "no" } });
    inner.deliver({ jsonrpc: "2.0", id: 2, result: null });
    expect(results).toEqual([]);
    watched.unsubscribe(handler);
    expect(inner.count()).toBe(0);
  });
});
