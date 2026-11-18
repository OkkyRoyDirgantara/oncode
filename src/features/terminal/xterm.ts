// Adapter xterm.js: renderer WebGL (dengan cadangan renderer bawaan), penyesuaian ukuran,
// tautan, pencarian, papan klip, dan tema/font yang mengikuti pengaturan.
import { FitAddon } from "@xterm/addon-fit";
import { SearchAddon } from "@xterm/addon-search";
import { WebLinksAddon } from "@xterm/addon-web-links";
import { WebglAddon } from "@xterm/addon-webgl";
import { Terminal } from "@xterm/xterm";
import { openUrl } from "@tauri-apps/plugin-opener";
import { isMac } from "@/app/commands";
import { toast } from "@/app/overlay";
import { useApp } from "@/app/store";
import type { AdapterEvents, TerminalAdapter, TerminalSize } from "./adapter";
import { classifyKey } from "./keys";
import { appBindings } from "./shield";
import { currentTerminalTheme } from "./theme";

const FALLBACK_FONT = 'ui-monospace, "JetBrains Mono", Menlo, Consolas, monospace';
/** Wadah yang lebih kecil dari ini dianggap belum punya tata letak (panel masih terlipat). */
const MIN_LAYOUT_PX = 20;
const RESIZE_DEBOUNCE_MS = 60;
/** Batas menunggu tata letak sebelum shell dimulai dengan ukuran bawaan. */
const LAYOUT_WAIT_MS = 400;

function fontOptions(): { fontFamily: string; fontSize: number } {
  const settings = useApp.getState().settings;
  return {
    fontFamily: settings?.editor.fontFamily || FALLBACK_FONT,
    fontSize: settings?.terminal.fontSize ?? settings?.editor.fontSize ?? 13,
  };
}

const live = new Set<() => void>();
let syncInstalled = false;

/** Font dan tema semua terminal mengikuti pengaturan dan tema aplikasi tanpa dibuat ulang. */
function installAppearanceSync(): void {
  if (syncInstalled) return;
  syncInstalled = true;
  const refresh = () => live.forEach((apply) => apply());
  useApp.subscribe((state, previous) => {
    if (state.settings !== previous.settings) refresh();
  });
  window.addEventListener("oncode:theme", refresh);
}

export function createXtermAdapter(events: AdapterEvents): TerminalAdapter {
  installAppearanceSync();

  const host = document.createElement("div");
  host.className = "tm-host";

  const term = new Terminal({
    ...fontOptions(),
    theme: currentTerminalTheme(),
    cursorBlink: true,
    scrollback: 10_000,
    allowProposedApi: false,
  });
  const fit = new FitAddon();
  const search = new SearchAddon();
  term.loadAddon(fit);
  term.loadAddon(search);
  term.loadAddon(new WebLinksAddon((_event, uri) => void openUrl(uri).catch((e) => console.warn("Gagal membuka tautan", e))));

  let opened = false;
  let visible = true;
  let disposed = false;
  let observer: ResizeObserver | null = null;
  let fitTimer: ReturnType<typeof setTimeout> | undefined;

  /** Menyesuaikan ukuran terhadap wadah; false bila wadah belum punya tata letak. */
  const tryFit = (): boolean => {
    if (!opened || disposed || host.hidden) return false;
    if (host.clientWidth < MIN_LAYOUT_PX || host.clientHeight < MIN_LAYOUT_PX) return false;
    try {
      const proposed = fit.proposeDimensions();
      if (!proposed || !Number.isFinite(proposed.cols) || !Number.isFinite(proposed.rows)) return false;
      fit.fit();
      return true;
    } catch (e) {
      console.warn("Gagal menyesuaikan ukuran terminal", e);
      return false;
    }
  };

  const scheduleFit = (delay = RESIZE_DEBOUNCE_MS) => {
    clearTimeout(fitTimer);
    fitTimer = setTimeout(() => {
      // Menyesuaikan elemen yang tersembunyi menghasilkan ukuran yang tidak masuk akal.
      if (visible) tryFit();
    }, delay);
  };

  const loadWebgl = () => {
    try {
      const webgl = new WebglAddon();
      // Konteks WebGL bisa hilang (GPU direset, terlalu banyak konteks): kembali ke renderer bawaan.
      webgl.onContextLoss(() => webgl.dispose());
      term.loadAddon(webgl);
    } catch (e) {
      console.warn("WebGL tidak tersedia; terminal memakai renderer bawaan", e);
    }
  };

  const applyAppearance = () => {
    const font = fontOptions();
    term.options.fontFamily = font.fontFamily;
    term.options.fontSize = font.fontSize;
    term.options.theme = currentTerminalTheme();
    scheduleFit(0);
  };
  live.add(applyAppearance);

  const copy = () => {
    const text = term.getSelection();
    if (text) void navigator.clipboard.writeText(text).catch(() => toast.warning("Could not write to the clipboard."));
  };
  const paste = () => {
    navigator.clipboard.readText().then(
      (text) => {
        if (text && !disposed) term.paste(text);
      },
      () => toast.warning("Could not read the clipboard."),
    );
  };

  term.attachCustomKeyEventHandler((event) => {
    const action = classifyKey(event, { isMac, appBindings: appBindings() });
    if (action === "shell") return true;
    // Tombol aplikasi dibiarkan menggelembung ke penangan keybinding global.
    if (action !== "app" && event.type === "keydown") {
      // Tanpa ini WebView bisa ikut menempel lewat event `paste` bawaannya.
      event.preventDefault();
      if (action === "copy") copy();
      else if (action === "paste") paste();
      else events.onFind();
    }
    return false;
  });

  term.onData((data) => events.onInput(data));
  term.onResize(({ cols, rows }) => events.onResize({ cols, rows }));

  return {
    mount(stage: HTMLElement): Promise<TerminalSize> {
      stage.appendChild(host);
      if (!opened) {
        opened = true;
        term.open(host);
        loadWebgl();
        observer = new ResizeObserver(() => scheduleFit());
        observer.observe(host);
      }
      return new Promise((resolve) => {
        const started = Date.now();
        const attempt = () => {
          if (disposed || tryFit() || Date.now() - started > LAYOUT_WAIT_MS) resolve({ cols: term.cols, rows: term.rows });
          else setTimeout(attempt, 30);
        };
        attempt();
      });
    },
    write(data, done) {
      if (!disposed) term.write(data, done);
    },
    setVisible(next) {
      visible = next;
      host.hidden = !next;
      if (next) scheduleFit(0);
    },
    focus() {
      if (!disposed) term.focus();
    },
    clear() {
      if (!disposed) term.clear();
    },
    find(query, forward) {
      if (disposed) return false;
      return forward ? search.findNext(query) : search.findPrevious(query);
    },
    clearFind() {
      if (disposed) return;
      search.clearDecorations();
      term.clearSelection();
    },
    dispose() {
      if (disposed) return;
      disposed = true;
      clearTimeout(fitTimer);
      observer?.disconnect();
      live.delete(applyAppearance);
      term.dispose();
      host.remove();
    },
  };
}
