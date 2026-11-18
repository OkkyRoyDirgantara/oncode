// Language server tiruan untuk pengujian, dipasang di atas `mockCommands`: mencatat pesan yang
// dikirim klien dan menjawab `initialize` (serta permintaan lain lewat `respond`).
import type { Channel } from "@tauri-apps/api/core";
import { EditorState, type Extension } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { useApp } from "@/app/store";
import { mockCommands, type LspEvent, type LspServerInfo, type Settings, type WorkspaceInfo } from "@/ipc";
import { lspEditorExtension } from "./servers";

export interface RpcMessage {
  jsonrpc?: string;
  id?: number | string;
  method?: string;
  params?: any; // eslint-disable-line @typescript-eslint/no-explicit-any
  result?: unknown;
  error?: unknown;
}

export interface FakeProcess {
  id: number;
  language: string;
  channel: Channel<LspEvent>;
  received: RpcMessage[];
  stopped: boolean;
  /** Kirim pesan dari server ke klien. */
  emit(message: RpcMessage): void;
  /** Proses berakhir. */
  exit(code: number | null, stderrTail?: string): void;
  methods(): string[];
  notifications(method: string): RpcMessage[];
}

export interface FakeBackendOptions {
  infos?: LspServerInfo[];
  /** Kembalikan galat `AppError` dari `lspStart` untuk bahasa ini. */
  startError?: (language: string) => { code: string; message: string } | null;
  /** Server menjawab `initialize`? (default: ya) */
  answerInitialize?: boolean;
  capabilities?: Record<string, unknown>;
  /** Jawaban untuk permintaan selain `initialize`; `undefined` berarti tidak dijawab. */
  respond?: (message: RpcMessage, process: FakeProcess) => unknown;
}

export const RUST: LspServerInfo = { language: "rust", command: "rust-analyzer", args: [], installed: true, installHint: "Pasang rust-analyzer dengan `rustup component add rust-analyzer`." };
export const TS_FAMILY: LspServerInfo[] = ["typescript", "typescriptreact", "javascript"].map((language) => ({
  language,
  command: "typescript-language-server",
  args: ["--stdio"],
  installed: true,
  installHint: "npm install -g typescript-language-server typescript",
}));

export function fakeLspBackend(options: FakeBackendOptions = {}) {
  const processes: FakeProcess[] = [];
  const stopCalls: number[] = [];
  let nextId = 1;
  let serversCalls = 0;
  mockCommands({
    lspServers: () => {
      serversCalls += 1;
      return options.infos ?? [RUST, ...TS_FAMILY];
    },
    lspStart: ({ language, onEvent }) => {
      const error = options.startError?.(language);
      if (error) throw { ...error, detail: null };
      const process: FakeProcess = {
        id: nextId++,
        language,
        channel: onEvent,
        received: [],
        stopped: false,
        emit: (message) => onEvent.onmessage({ kind: "message", json: JSON.stringify({ jsonrpc: "2.0", ...message }) }),
        exit: (code, stderrTail = "") => onEvent.onmessage({ kind: "exit", code, stderrTail }),
        methods: () => process.received.map((m) => m.method ?? "(response)"),
        notifications: (method) => process.received.filter((m) => m.method === method),
      };
      processes.push(process);
      return process.id;
    },
    lspSend: ({ id, json }) => {
      const process = processes.find((p) => p.id === id);
      if (!process) throw { code: "notFound", message: "no such server", detail: null };
      const message = JSON.parse(json) as RpcMessage;
      process.received.push(message);
      if (message.id === undefined || !message.method) return null;
      if (message.method === "initialize") {
        if (options.answerInitialize !== false) {
          process.emit({ id: message.id, result: { capabilities: options.capabilities ?? { textDocumentSync: 2, definitionProvider: true, referencesProvider: true, renameProvider: true } } });
        }
        return null;
      }
      const result = options.respond?.(message, process);
      if (result !== undefined) process.emit({ id: message.id, result });
      return null;
    },
    lspStop: ({ id }) => {
      stopCalls.push(id);
      const process = processes.find((p) => p.id === id);
      if (process) process.stopped = true;
      return null;
    },
    fsOpen: () => {
      throw { code: "notFound", message: "not on disk", detail: null };
    },
  });
  return { processes, stopCalls, serversCalls: () => serversCalls, options };
}

export function setWorkspaceForTest(overrides: { root?: string; trusted?: boolean; lspEnabled?: boolean } | null): void {
  if (!overrides) {
    useApp.setState({ workspace: null });
    return;
  }
  const settings = { lsp: { enabled: overrides.lspEnabled ?? true, servers: {} } } as unknown as Settings;
  const root = overrides.root ?? "/proj";
  useApp.setState({ settings, workspace: { root, name: "proj", trusted: overrides.trusted ?? true, repoRoot: null, settings } as WorkspaceInfo });
}

/** Membuat editor untuk sebuah berkas dengan ekstensi LSP, seperti yang dilakukan tab berkas. */
export async function openEditor(path: string, languageId: string | null, doc = "", extra: Extension = []): Promise<EditorView> {
  const extension = await lspEditorExtension({ path, languageId, large: false });
  const parent = document.createElement("div");
  document.body.appendChild(parent);
  return new EditorView({ parent, state: EditorState.create({ doc, extensions: [extra, extension ?? []] }) });
}
