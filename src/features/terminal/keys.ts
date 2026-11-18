// Penyaring tombol terminal: menentukan tombol mana yang diteruskan ke shell, mana yang
// ditangani terminal sendiri (salin, tempel, cari), dan mana yang dibiarkan untuk aplikasi.
// Murni (tanpa DOM dan tanpa registry) agar mudah diuji untuk kedua platform.

export interface KeyLike {
  key: string;
  code?: string;
  ctrlKey: boolean;
  metaKey: boolean;
  altKey: boolean;
  shiftKey: boolean;
}

export type KeyAction = "shell" | "app" | "copy" | "paste" | "find";

/**
 * Command aplikasi yang tetap berlaku saat terminal fokus. Keybinding-nya diambil dari registry
 * (termasuk penimpaan pengguna); tombol lain yang terikat ke command di luar daftar ini
 * (mis. Ctrl+W, Ctrl+S, Ctrl+O) diteruskan ke shell.
 */
export const COMMANDS_TO_SKIP_SHELL: readonly string[] = [
  "workbench.commandPalette",
  "workbench.quickOpen",
  "workbench.toggleSidebar",
  "workbench.togglePanel",
  "workbench.nextTab",
  "workbench.previousTab",
  "window.new",
  "search.focus",
  "terminal.toggle",
  "terminal.new",
  "terminal.kill",
  "terminal.clear",
  "terminal.focusNext",
];

const KEY_ALIASES: Record<string, string> = {
  arrowup: "Up",
  arrowdown: "Down",
  arrowleft: "Left",
  arrowright: "Right",
  esc: "Escape",
  " ": "Space",
  spacebar: "Space",
  return: "Enter",
  del: "Delete",
  backquote: "`",
};

function canonicalKey(key: string): string {
  const alias = KEY_ALIASES[key.toLowerCase()];
  if (alias) return alias;
  if (key.length === 1) return key.toUpperCase();
  return key.charAt(0).toUpperCase() + key.slice(1);
}

/**
 * Keybinding kanonis sebuah event, dengan format yang sama seperti `keybindingFromEvent`
 * di `app/commands.ts`, tetapi platformnya diberikan dari luar. Null bila hanya modifier.
 */
export function bindingOf(e: KeyLike, isMac: boolean): string | null {
  if (["Control", "Shift", "Alt", "Meta", "OS", "Dead", "Unidentified"].includes(e.key)) return null;
  const mods: string[] = [];
  const mod = isMac ? e.metaKey : e.ctrlKey;
  if (mod) mods.push("Mod");
  if (isMac && e.ctrlKey) mods.push("Ctrl");
  if (e.altKey) mods.push("Alt");
  if (e.shiftKey) mods.push("Shift");
  let key = e.key;
  const letter = /^Key([A-Z])$/.exec(e.code ?? "");
  const digit = /^Digit([0-9])$/.exec(e.code ?? "");
  if (letter) key = letter[1];
  else if (digit && (e.altKey || mod)) key = digit[1];
  else if (e.code === "Backquote") key = "`";
  else if (e.code === "Backslash") key = "\\";
  return [...mods, canonicalKey(key)].join("+");
}

export interface ClassifyOptions {
  isMac: boolean;
  /** Keybinding kanonis (mis. "Mod+P") milik command di `COMMANDS_TO_SKIP_SHELL`. */
  appBindings: ReadonlySet<string>;
}

/**
 * - `copy` / `paste`: Mod+Shift+C / Mod+Shift+V; di macOS juga Cmd+C / Cmd+V.
 *   Ctrl+C polos selalu sampai ke shell sebagai SIGINT.
 * - `find`: Mod+F membuka bilah cari terminal.
 * - `app`: keybinding aplikasi yang diizinkan; di macOS semua kombinasi Cmd lain juga milik aplikasi
 *   karena Cmd tidak punya arti bagi shell.
 * - `shell`: sisanya.
 */
export function classifyKey(e: KeyLike, options: ClassifyOptions): KeyAction {
  const binding = bindingOf(e, options.isMac);
  if (!binding) return "shell";
  if (binding === "Mod+Shift+C" || (options.isMac && binding === "Mod+C")) return "copy";
  if (binding === "Mod+Shift+V" || (options.isMac && binding === "Mod+V")) return "paste";
  if (binding === "Mod+F") return "find";
  if (options.appBindings.has(binding)) return "app";
  if (options.isMac && e.metaKey) return "app";
  return "shell";
}
