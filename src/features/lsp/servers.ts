// Siklus hidup language server: satu proses per perintah server per workspace, dimulai saat
// berkas pertama bahasanya dibuka, dimulai ulang bila mati (dengan batas), dan dihentikan saat
// workspace berganti. Setiap proses punya `LSPClient` sendiri; editor dipasangi plugin klien
// itu selama server berjalan dan dilepas begitu server berhenti.
import { Compartment, type Extension } from "@codemirror/state";
import { EditorView, ViewPlugin, keymap } from "@codemirror/view";
import { setDiagnostics } from "@codemirror/lint";
import {
  LSPClient,
  formatKeymap,
  hoverTooltips,
  jumpToDefinitionKeymap,
  renameKeymap,
  serverCompletion,
  serverDiagnostics,
  signatureHelp,
  type LSPClientConfig,
} from "@codemirror/lsp-client";
import DOMPurify from "dompurify";
import { create } from "zustand";
import { toast } from "@/app/overlay";
import { useApp } from "@/app/store";
import type { EditorContext } from "@/features/editor/extensions";
import { commands, errorCode, errorMessage, type LspServerInfo } from "@/ipc";
import { clearDiagnostics, publishDiagnostics, type PublishDiagnosticsParams } from "./diagnostics";
import { findReferences } from "./references";
import { warnAboutSkippedRenameEdits, type WorkspaceEdit } from "./rename";
import { openTransport, watchResponses, type ServerExit, type ServerTransport } from "./transport";
import { pathToUri } from "./uri";
import { AppWorkspace, showDiagnosticsInOtherViews } from "./workspace";

export type ServerState = "starting" | "running" | "failed" | "missing" | "stopped";

export interface ServerStatus {
  key: string;
  /** Nama perintah server, mis. `rust-analyzer`. */
  name: string;
  /** Id bahasa LSP yang dilayani server ini. */
  languages: string[];
  state: ServerState;
  /** Cara memasang server (dari backend), ditampilkan saat `missing`. */
  installHint: string;
  /** Penjelasan singkat untuk `failed` dan `missing`. */
  message?: string;
  /** Ekor stderr proses terakhir yang mati. */
  stderrTail?: string;
}

interface LspState {
  /** Daftar server yang dikonfigurasi; null sebelum dimuat. */
  infos: LspServerInfo[] | null;
  /** Status per server (kunci: perintah + argumen). */
  servers: Record<string, ServerStatus>;
}

export const useLsp = create<LspState>(() => ({ infos: null, servers: {} }));

/** Proses yang keluar secepat ini tanpa pernah menjawab `initialize` dianggap tidak terpasang. */
export const EARLY_EXIT_MS = 3000;
export const MAX_RESTARTS = 5;
export const RESTART_WINDOW_MS = 3 * 60 * 1000;
const REQUEST_TIMEOUT_MS = 10_000;

/** Jeda sebelum percobaan mulai ulang ke-`attempt` (1, 2, ...): 0,5 d, 1 d, 2 d, 4 d, 8 d. */
export function restartDelay(attempt: number): number {
  return 500 * 2 ** Math.max(0, attempt - 1);
}

/** Bahasa yang memakai perintah yang sama (mis. ts/tsx/js/jsx) berbagi satu proses. */
export function serverKey(info: Pick<LspServerInfo, "command" | "args">): string {
  return [info.command, ...info.args].join("\u0000");
}

export function statusForLanguage(state: LspState, language: string | null): ServerStatus | null {
  if (!language) return null;
  const info = state.infos?.find((i) => i.language === language);
  return info ? (state.servers[serverKey(info)] ?? null) : null;
}

type Phase = "idle" | "starting" | "running" | "backoff" | "missing" | "failed" | "stopped";

interface Attachment {
  view: EditorView | null;
  slot: Compartment;
  uri: string;
  languageId: string;
  entry: Entry | null;
  /** Klien yang plugin-nya sedang terpasang di editor ini. */
  client: LSPClient | null;
}

interface Entry {
  key: string;
  info: LspServerInfo;
  phase: Phase;
  /** Bertambah setiap kali proses diganti atau dihentikan; event dari proses lama diabaikan. */
  generation: number;
  client: LSPClient | null;
  transport: ServerTransport | null;
  startedAt: number;
  initialized: boolean;
  /** Waktu mulai ulang otomatis dalam jendela pembatas. */
  restarts: number[];
  timer: ReturnType<typeof setTimeout> | null;
  attachments: Set<Attachment>;
}

const entries = new Map<string, Entry>();
let infosPromise: Promise<LspServerInfo[]> | null = null;
/** Bertambah saat workspace atau pengaturan LSP berubah; pekerjaan asinkron yang lama dibatalkan. */
let epoch = 0;

function lspAllowed(): boolean {
  const { workspace, settings } = useApp.getState();
  return !!workspace && workspace.trusted && !!settings?.lsp.enabled;
}

/** Daftar server yang dikonfigurasi (bawaan + pengaturan), dimuat sekali per workspace. */
export function loadServerInfos(): Promise<LspServerInfo[]> {
  if (!infosPromise) {
    const startedIn = epoch;
    infosPromise = commands.lspServers().then(
      (infos) => {
        if (startedIn === epoch) useLsp.setState({ infos });
        return infos;
      },
      (e) => {
        console.warn("Daftar language server tidak bisa dimuat", e);
        if (startedIn === epoch) infosPromise = null;
        return [];
      },
    );
  }
  return infosPromise;
}

function publish(entry: Entry, extra: Pick<ServerStatus, "message" | "stderrTail"> = {}): void {
  const state: ServerState | null =
    entry.phase === "idle" ? null : entry.phase === "backoff" ? "starting" : entry.phase;
  useLsp.setState((s) => {
    if (!state) {
      if (!(entry.key in s.servers)) return s;
      const { [entry.key]: _removed, ...rest } = s.servers;
      return { servers: rest };
    }
    const languages = (s.infos ?? [entry.info]).filter((i) => serverKey(i) === entry.key).map((i) => i.language);
    const status: ServerStatus = {
      key: entry.key,
      name: entry.info.command,
      languages: languages.length > 0 ? languages : [entry.info.language],
      state,
      installHint: entry.info.installHint,
      ...(extra.message ? { message: extra.message } : {}),
      ...(extra.stderrTail ? { stderrTail: extra.stderrTail } : {}),
    };
    return { servers: { ...s.servers, [entry.key]: status } };
  });
}

function entryFor(info: LspServerInfo): Entry {
  const key = serverKey(info);
  let entry = entries.get(key);
  if (!entry) {
    entry = {
      key,
      info,
      phase: "idle",
      generation: 0,
      client: null,
      transport: null,
      startedAt: 0,
      initialized: false,
      restarts: [],
      timer: null,
      attachments: new Set(),
    };
    entries.set(key, entry);
  }
  return entry;
}

/** Memasang atau melepas plugin klien pada satu editor sesuai keadaan servernya. */
function syncAttachment(attachment: Attachment): void {
  const view = attachment.view;
  if (!view) return;
  const entry = attachment.entry;
  const desired = entry && entry.phase === "running" ? entry.client : null;
  if (attachment.client === desired) return;
  const wasAttached = attachment.client !== null;
  attachment.client = desired;
  try {
    view.dispatch({ effects: attachment.slot.reconfigure(desired ? desired.plugin(attachment.uri, attachment.languageId) : []) });
    // Garis bawah dari server yang sudah berhenti tidak boleh tertinggal di editor.
    if (!desired && wasAttached) view.dispatch(setDiagnostics(view.state, []));
  } catch (e) {
    console.warn("Plugin LSP tidak bisa dipasang ke editor", e);
  }
}

function syncAttachments(entry: Entry): void {
  for (const attachment of [...entry.attachments]) syncAttachment(attachment);
}

/** Memutus klien dan transport proses saat ini. `entry.phase` harus sudah bukan `running`. */
function teardown(entry: Entry): void {
  const { client, transport } = entry;
  entry.client = null;
  entry.transport = null;
  entry.initialized = false;
  transport?.close();
  // Setelah `disconnect`, notifikasi dari plugin yang dilepas (didClose) tidak lagi dikirim ke transport yang sudah tutup.
  client?.disconnect();
  syncAttachments(entry);
  clearDiagnostics(entry.key);
}

function clientConfig(entry: Entry): LSPClientConfig {
  const root = useApp.getState().workspace?.root;
  return {
    rootUri: root ? pathToUri(root) : undefined,
    timeout: REQUEST_TIMEOUT_MS,
    workspace: (client) => new AppWorkspace(client),
    // Dokumentasi dari server dirender sebagai HTML; server yang nakal tidak boleh bisa menyisipkan skrip.
    sanitizeHTML: (html) => DOMPurify.sanitize(html),
    notificationHandlers: {
      // Dicoba sebelum penangan pustaka; mengembalikan false agar `serverDiagnostics()` tetap menandai editor.
      "textDocument/publishDiagnostics": (client, params: PublishDiagnosticsParams) => {
        publishDiagnostics(entry.key, params);
        showDiagnosticsInOtherViews(client, params);
        return false;
      },
    },
    extensions: [
      serverCompletion(),
      hoverTooltips(),
      signatureHelp(),
      serverDiagnostics(),
      keymap.of([...formatKeymap, ...renameKeymap, ...jumpToDefinitionKeymap, { key: "Shift-F12", run: findReferences, preventDefault: true }]),
    ],
  };
}

async function start(entry: Entry): Promise<void> {
  if (entry.timer) {
    clearTimeout(entry.timer);
    entry.timer = null;
  }
  const generation = ++entry.generation;
  entry.phase = "starting";
  entry.startedAt = Date.now();
  entry.initialized = false;
  publish(entry);

  let transport: ServerTransport;
  try {
    transport = await openTransport(entry.info.language, (exit) => handleExit(entry, generation, exit));
  } catch (e) {
    if (generation !== entry.generation) return;
    const code = errorCode(e);
    if (code === "serverNotInstalled") {
      entry.phase = "missing";
      publish(entry, { message: errorMessage(e) });
    } else if (code === "untrusted" || code === "disabled") {
      entry.phase = "idle";
      publish(entry);
    } else {
      entry.phase = "failed";
      publish(entry, { message: errorMessage(e) });
    }
    return;
  }
  if (generation !== entry.generation) {
    // Dihentikan, atau prosesnya sudah keluar, selagi menunggu backend.
    if (transport.id !== null) void commands.lspStop(transport.id).catch(() => {});
    transport.close();
    return;
  }

  const client = new LSPClient(clientConfig(entry));
  entry.transport = transport;
  entry.client = client;
  const watched = watchResponses(transport, "textDocument/rename", (result) => warnAboutSkippedRenameEdits(client, result as WorkspaceEdit));
  client.connect(watched);
  client.initializing.then(
    () => {
      if (generation !== entry.generation) return;
      entry.initialized = true;
      entry.phase = "running";
      publish(entry);
      syncAttachments(entry);
    },
    (error: unknown) => {
      if (generation !== entry.generation) return;
      // Server hidup tetapi tidak menjawab `initialize` dengan benar: hentikan dan perlakukan seperti mati.
      entry.generation += 1;
      if (transport.id !== null) void commands.lspStop(transport.id).catch(() => {});
      scheduleRestart(entry, { message: `Initialization failed: ${errorMessage(error)}` });
    },
  );
}

function handleExit(entry: Entry, generation: number, exit: ServerExit): void {
  if (generation !== entry.generation) return;
  entry.generation += 1;
  const early = !entry.initialized && Date.now() - entry.startedAt < EARLY_EXIT_MS;
  const description = exit.code === null ? "The server process was terminated." : `The server process exited with code ${exit.code}.`;
  if (early) {
    // Mis. proxy rustup untuk rust-analyzer yang komponennya belum dipasang: tidak ada gunanya mencoba lagi.
    entry.phase = "missing";
    teardown(entry);
    publish(entry, { message: description, stderrTail: exit.stderrTail });
    return;
  }
  scheduleRestart(entry, { message: description, stderrTail: exit.stderrTail });
}

function scheduleRestart(entry: Entry, extra: Pick<ServerStatus, "message" | "stderrTail">): void {
  const now = Date.now();
  entry.restarts = entry.restarts.filter((time) => now - time < RESTART_WINDOW_MS);
  if (entry.restarts.length >= MAX_RESTARTS) {
    entry.phase = "failed";
    teardown(entry);
    publish(entry, extra);
    const { key, info } = entry;
    toast.error(`${info.command} crashed ${MAX_RESTARTS} times in the last 3 minutes and will not be restarted.`, [
      { label: "Restart", run: () => restartServer(key) },
    ]);
    return;
  }
  entry.restarts.push(now);
  entry.phase = "backoff";
  teardown(entry);
  publish(entry, extra);
  entry.timer = setTimeout(() => {
    entry.timer = null;
    if (entry.phase === "backoff") void start(entry);
  }, restartDelay(entry.restarts.length));
}

/** Memulai server bila diizinkan dan belum berjalan. Keadaan `missing`, `failed`, dan `stopped` tidak dicoba lagi. */
function ensureStarted(entry: Entry): void {
  if (entry.phase !== "idle" || !lspAllowed()) return;
  void start(entry);
}

function halt(entry: Entry, phase: Phase): void {
  entry.generation += 1;
  if (entry.timer) {
    clearTimeout(entry.timer);
    entry.timer = null;
  }
  const id = entry.transport?.id ?? null;
  entry.phase = phase;
  teardown(entry);
  if (id !== null) void commands.lspStop(id).catch(() => {});
  publish(entry);
}

/** Menghentikan server atas permintaan pengguna; tidak dimulai lagi sampai diminta. */
export function stopServer(key: string): void {
  const entry = entries.get(key);
  if (entry) halt(entry, "stopped");
}

/** Memulai ulang server (juga dari keadaan `missing`, `failed`, atau `stopped`). */
export function restartServer(key: string): void {
  const entry = entries.get(key);
  if (!entry) return;
  halt(entry, "idle");
  entry.restarts = [];
  if (!lspAllowed()) {
    toast.warning("Language servers only run in trusted folders with LSP enabled in settings.");
    return;
  }
  void start(entry);
}

function allAttachments(): Attachment[] {
  return [...entries.values()].flatMap((entry) => [...entry.attachments]);
}

function haltAll(): void {
  epoch += 1;
  for (const entry of entries.values()) halt(entry, "idle");
  entries.clear();
  infosPromise = null;
  clearDiagnostics();
  useLsp.setState({ infos: null, servers: {} });
}

/** Workspace ditutup atau diganti: semua server berhenti dan editor lama dilepas. */
export function stopAllServers(): void {
  const attachments = allAttachments();
  for (const attachment of attachments) attachment.entry = null;
  haltAll();
  for (const attachment of attachments) syncAttachment(attachment);
}

/**
 * Status kepercayaan atau pengaturan LSP berubah sementara editor tetap terbuka: semua server
 * dihentikan, lalu server untuk editor yang masih ada dimulai lagi bila sekarang diizinkan.
 */
export async function reconcileServers(): Promise<void> {
  const attachments = allAttachments();
  for (const attachment of attachments) attachment.entry = null;
  haltAll();
  for (const attachment of attachments) syncAttachment(attachment);
  if (!useApp.getState().workspace) return;
  const startedIn = epoch;
  const infos = await loadServerInfos();
  if (startedIn !== epoch) return;
  for (const attachment of attachments) {
    if (!attachment.view) continue;
    const info = infos.find((i) => i.language === attachment.languageId);
    if (!info) continue;
    const entry = entryFor(info);
    attachment.entry = entry;
    entry.attachments.add(attachment);
  }
  for (const entry of entries.values()) ensureStarted(entry);
}

/**
 * Penyedia ekstensi editor. Editor didaftarkan pada server bahasanya (bila ada yang dikonfigurasi);
 * plugin klien dipasang begitu server berjalan dan dilepas saat server berhenti, sehingga memulai
 * ulang server atau memercayai folder berlaku juga untuk editor yang sudah terbuka.
 */
export async function lspEditorExtension(context: EditorContext): Promise<Extension | null> {
  if (context.large || !context.languageId || !useApp.getState().workspace) return null;
  const languageId = context.languageId;
  const startedIn = epoch;
  const infos = await loadServerInfos();
  if (startedIn !== epoch) return null;
  const info = infos.find((i) => i.language === languageId);
  if (!info) return null;

  const attachment: Attachment = { view: null, slot: new Compartment(), uri: pathToUri(context.path), languageId, entry: null, client: null };
  return [
    attachment.slot.of([]),
    ViewPlugin.define((view) => {
      attachment.view = view;
      // Tidak boleh men-dispatch di tengah pembaruan editor; pasang setelahnya.
      queueMicrotask(() => {
        if (attachment.view !== view || startedIn !== epoch) return;
        const entry = entryFor(info);
        attachment.entry = entry;
        entry.attachments.add(attachment);
        syncAttachment(attachment);
        ensureStarted(entry);
      });
      return {
        destroy() {
          attachment.view = null;
          attachment.entry?.attachments.delete(attachment);
          attachment.entry = null;
        },
      };
    }),
  ];
}

/** Hanya untuk pengujian. */
export function resetLspForTest(): void {
  stopAllServers();
}
