// Transport @codemirror/lsp-client di atas command dan channel Tauri.
// Backend membingkai pesan (Content-Length); di sini hanya JSON mentah yang lewat.
import { Channel } from "@tauri-apps/api/core";
import type { Transport } from "@codemirror/lsp-client";
import { commands, type LspEvent } from "@/ipc";

type Handler = (message: string) => void;

export interface ServerExit {
  code: number | null;
  stderrTail: string;
}

/** Bentuk permintaan yang dibuat @codemirror/lsp-client: `{"jsonrpc":"2.0","id":N,"method":...`. */
const REQUEST_PREFIX = /^\{"jsonrpc":"2\.0","id":[^,]+,"method":/;

export class ServerTransport implements Transport {
  /** Id server di backend; terisi setelah `lspStart` selesai. */
  id: number | null = null;
  private handlers: Handler[] = [];
  /** Pesan yang tiba sebelum ada pendengar. */
  private backlog: string[] = [];
  private outbox: string[] = [];
  private sending = false;
  private closed = false;

  send(message: string): void {
    if (this.closed) {
      // Permintaan: lempar agar klien langsung menolaknya, tidak menunggu batas waktu. Notifikasi
      // dibuang diam-diam, karena klien mengirimnya dari dalam `.then` tanpa penangan galat (mis.
      // didClose yang dijadwalkan tepat sebelum server berhenti) dan lemparan akan menjadi
      // "unhandled rejection".
      if (REQUEST_PREFIX.test(message)) throw new Error("The language server is not running");
      return;
    }
    this.outbox.push(message);
    void this.pump();
  }

  subscribe(handler: Handler): void {
    this.handlers.push(handler);
    if (this.backlog.length > 0) {
      const pending = this.backlog;
      this.backlog = [];
      for (const message of pending) this.receive(message);
    }
  }

  unsubscribe(handler: Handler): void {
    this.handlers = this.handlers.filter((h) => h !== handler);
  }

  /** Pesan dari server. */
  receive(message: string): void {
    if (this.closed) return;
    if (this.handlers.length === 0) {
      this.backlog.push(message);
      return;
    }
    for (const handler of [...this.handlers]) handler(message);
  }

  /** Koneksi berakhir: pesan masuk dibuang; `send` menolak permintaan dan membuang notifikasi. */
  close(): void {
    this.closed = true;
    this.outbox = [];
    this.backlog = [];
    this.handlers = [];
  }

  get isClosed(): boolean {
    return this.closed;
  }

  /** Mulai mengirim antrean (dipanggil setelah id diketahui). */
  start(id: number): void {
    this.id = id;
    void this.pump();
  }

  /**
   * Pesan dikirim satu per satu: command Tauri berjalan asinkron di backend, jadi dua pemanggilan
   * yang berdekatan bisa tertukar, padahal urutan (mis. didChange sebelum completion) penting bagi LSP.
   */
  private async pump(): Promise<void> {
    if (this.sending || this.id === null) return;
    this.sending = true;
    try {
      while (this.outbox.length > 0 && !this.closed) {
        const message = this.outbox.shift()!;
        try {
          await commands.lspSend(this.id, message);
        } catch (e) {
          // Proses server kemungkinan sudah mati; event `exit` dari backend yang menentukan langkah berikutnya.
          if (!this.closed) console.warn("Gagal mengirim pesan ke language server", e);
        }
      }
    } finally {
      this.sending = false;
    }
  }
}

/**
 * Memulai server untuk `language` dan mengembalikan transport-nya. `onExit` dipanggil saat proses
 * berakhir, yang bisa terjadi sebelum promise ini selesai (mis. program langsung keluar).
 */
export async function openTransport(language: string, onExit: (exit: ServerExit) => void): Promise<ServerTransport> {
  const transport = new ServerTransport();
  const channel = new Channel<LspEvent>();
  channel.onmessage = (event) => {
    if (event.kind === "message") transport.receive(event.json);
    else onExit({ code: event.code, stderrTail: event.stderrTail });
  };
  transport.start(await commands.lspStart(language, channel));
  return transport;
}

/**
 * Membungkus transport untuk mengintip hasil permintaan dengan `method` tertentu tanpa mengubah
 * alurnya. Pesan hanya di-parse ulang selagi ada permintaan semacam itu yang belum dijawab.
 */
export function watchResponses(inner: Transport, method: string, onResult: (result: unknown) => void): Transport {
  const pending = new Set<number | string>();
  const wrappers = new Map<Handler, Handler>();
  const marker = `"method":${JSON.stringify(method)}`;
  return {
    send(message) {
      if (message.includes(marker)) {
        try {
          const parsed = JSON.parse(message) as { id?: number | string; method?: string };
          if (parsed.method === method && parsed.id !== undefined) pending.add(parsed.id);
        } catch {
          // Bukan JSON: biarkan transport di bawahnya yang menangani.
        }
      }
      inner.send(message);
    },
    subscribe(handler) {
      const wrapper: Handler = (message) => {
        let result: unknown;
        let matched = false;
        if (pending.size > 0) {
          try {
            const parsed = JSON.parse(message) as { id?: number | string; method?: string; result?: unknown };
            if (parsed.id !== undefined && parsed.method === undefined && pending.delete(parsed.id)) {
              matched = parsed.result !== undefined && parsed.result !== null;
              result = parsed.result;
            }
          } catch {
            // Diabaikan; klien akan melaporkan pesan yang rusak.
          }
        }
        handler(message);
        if (matched) onResult(result);
      };
      wrappers.set(handler, wrapper);
      inner.subscribe(wrapper);
    },
    unsubscribe(handler) {
      const wrapper = wrappers.get(handler);
      if (!wrapper) return;
      wrappers.delete(handler);
      inner.unsubscribe(wrapper);
    },
  };
}
