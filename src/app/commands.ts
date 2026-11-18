// Registry command dan keybinding. Setiap aksi di aplikasi terdaftar di sini agar
// bisa dijalankan dari command palette, menu, dan keyboard.

export interface Command {
  id: string;
  title: string;
  category?: string;
  /** Mis. "Mod+Shift+P"; `Mod` = Ctrl di Linux/Windows, Cmd di macOS. */
  keybinding?: string | string[];
  /** Command hanya aktif (dan hanya menangkap keybinding) bila ini mengembalikan true. */
  when?: () => boolean;
  /** Tidak ditampilkan di command palette. */
  hidden?: boolean;
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  run: (...args: any[]) => unknown;
}

const registry = new Map<string, Command>();
const overrides = new Map<string, string[]>();
const listeners = new Set<() => void>();

export const isMac = typeof navigator !== "undefined" && /Mac|iPhone|iPad/.test(navigator.platform ?? "");

export function registerCommand(command: Command): () => void {
  registry.set(command.id, command);
  listeners.forEach((l) => l());
  return () => {
    if (registry.get(command.id) === command) registry.delete(command.id);
    listeners.forEach((l) => l());
  };
}

export function registerCommands(commands: Command[]): () => void {
  const disposers = commands.map(registerCommand);
  return () => disposers.forEach((d) => d());
}

export function getCommand(id: string): Command | undefined {
  return registry.get(id);
}

export function allCommands(): Command[] {
  return [...registry.values()];
}

export function onCommandsChanged(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

export async function executeCommand(id: string, ...args: unknown[]): Promise<unknown> {
  const command = registry.get(id);
  if (!command) throw new Error(`Command tidak dikenal: ${id}`);
  if (command.when && !command.when()) return undefined;
  return await command.run(...args);
}

/** Menimpa keybinding bawaan (dari keybindings pengguna). Daftar kosong menghapus keybinding. */
export function setKeybindingOverrides(map: Record<string, string | string[]>): void {
  overrides.clear();
  for (const [id, value] of Object.entries(map)) {
    overrides.set(id, (Array.isArray(value) ? value : [value]).map(canonicalKeybinding).filter(Boolean));
  }
  listeners.forEach((l) => l());
}

export function keybindingsFor(command: Command): string[] {
  const custom = overrides.get(command.id);
  if (custom) return custom;
  if (!command.keybinding) return [];
  return (Array.isArray(command.keybinding) ? command.keybinding : [command.keybinding]).map(canonicalKeybinding);
}

const MODIFIER_ORDER = ["Mod", "Ctrl", "Alt", "Shift"];
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

/** Menormalkan penulisan keybinding: urutan modifier tetap, tombol huruf besar. */
export function canonicalKeybinding(binding: string): string {
  const parts = binding.split("+").map((p) => p.trim()).filter(Boolean);
  // "Ctrl++" berarti tombol plus.
  if (binding.trim().endsWith("++") || binding.trim() === "+") parts.push("+");
  if (parts.length === 0) return "";
  const key = canonicalKey(parts[parts.length - 1]);
  const mods = new Set(
    parts.slice(0, -1).map((m) => {
      const lower = m.toLowerCase();
      if (lower === "cmd" || lower === "meta" || lower === "command") return isMac ? "Mod" : "Meta";
      if (lower === "ctrl" || lower === "control") return isMac ? "Ctrl" : "Mod";
      if (lower === "mod") return "Mod";
      if (lower === "alt" || lower === "option" || lower === "opt") return "Alt";
      if (lower === "shift") return "Shift";
      return m;
    }),
  );
  return [...MODIFIER_ORDER.filter((m) => mods.has(m)), key].join("+");
}

/** Keybinding kanonis untuk sebuah event keyboard, atau null bila hanya modifier yang ditekan. */
export function keybindingFromEvent(e: Pick<KeyboardEvent, "key" | "code" | "ctrlKey" | "metaKey" | "altKey" | "shiftKey">): string | null {
  if (["Control", "Shift", "Alt", "Meta", "OS", "Dead", "Unidentified"].includes(e.key)) return null;
  const mods: string[] = [];
  const mod = isMac ? e.metaKey : e.ctrlKey;
  if (mod) mods.push("Mod");
  if (isMac && e.ctrlKey) mods.push("Ctrl");
  if (e.altKey) mods.push("Alt");
  if (e.shiftKey) mods.push("Shift");
  // Dengan Shift/Alt, `key` bisa berupa simbol hasil tata letak; pakai `code` untuk huruf dan angka.
  let key = e.key;
  const letter = /^Key([A-Z])$/.exec(e.code ?? "");
  const digit = /^Digit([0-9])$/.exec(e.code ?? "");
  if (letter) key = letter[1];
  else if (digit && (e.altKey || mod)) key = digit[1];
  else if (e.code === "Backquote") key = "`";
  else if (e.code === "Backslash") key = "\\";
  return [...mods, canonicalKey(key)].join("+");
}

/** Teks keybinding untuk ditampilkan, mis. "Ctrl+Shift+P" atau "⌘⇧P". */
export function formatKeybinding(binding: string): string {
  const parts = canonicalKeybinding(binding).split("+");
  if (binding.endsWith("++")) parts.splice(-2, 2, "+");
  if (isMac) {
    const symbols: Record<string, string> = { Mod: "⌘", Ctrl: "⌃", Alt: "⌥", Shift: "⇧", Enter: "↩", Up: "↑", Down: "↓", Left: "←", Right: "→" };
    return parts.map((p) => symbols[p] ?? p).join("");
  }
  const names: Record<string, string> = { Mod: "Ctrl", Up: "↑", Down: "↓", Left: "←", Right: "→" };
  return parts.map((p) => names[p] ?? p).join("+");
}

/** Command aktif yang terikat pada keybinding ini. */
export function commandForKeybinding(binding: string): Command | undefined {
  let found: Command | undefined;
  for (const command of registry.values()) {
    if (!keybindingsFor(command).includes(binding)) continue;
    if (command.when && !command.when()) continue;
    found = command;
  }
  return found;
}

/** Memasang penangan keyboard global. Dipanggil sekali saat aplikasi mulai. */
export function installKeybindings(target: Window = window): () => void {
  const handler = (e: KeyboardEvent) => {
    if (e.defaultPrevented || e.isComposing) return;
    const binding = keybindingFromEvent(e);
    if (!binding) return;
    const command = commandForKeybinding(binding);
    if (!command) return;
    e.preventDefault();
    e.stopPropagation();
    void Promise.resolve(command.run()).catch((err) => console.error(`Command ${command.id} gagal`, err));
  };
  target.addEventListener("keydown", handler, true);
  return () => target.removeEventListener("keydown", handler, true);
}

/** Hanya untuk pengujian. */
export function resetCommandsForTest(): void {
  registry.clear();
  overrides.clear();
}
