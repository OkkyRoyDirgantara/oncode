// Definisi kolom halaman pengaturan: label, penjelasan, cara membaca/menulis nilainya, dan validasi.
// Formulir dirender dari daftar ini, sehingga pencarian, penanda "ditimpa proyek", dan pengujian memakai satu sumber.
import type { ServerSpec, Settings } from "@/ipc";

export type SectionId = "editor" | "files" | "appearance" | "git" | "terminal" | "lsp" | "accounts" | "keyboard";

export const SECTIONS: { id: SectionId; title: string }[] = [
  { id: "editor", title: "Editor" },
  { id: "files", title: "Files" },
  { id: "appearance", title: "Appearance" },
  { id: "git", title: "Git" },
  { id: "terminal", title: "Terminal" },
  { id: "lsp", title: "Language Servers" },
  { id: "accounts", title: "Accounts" },
  // Tanpa kolom: hanya tombol untuk membuka keybindings.json.
  { id: "keyboard", title: "Keyboard Shortcuts" },
];

interface FieldBase {
  /** Kunci bertitik seperti di settings.json, mis. `editor.fontSize`. */
  id: string;
  section: SectionId;
  label: string;
  description: string;
  /** Kata tambahan untuk pencarian. */
  keywords?: string;
}

export interface TextField extends FieldBase {
  kind: "text";
  placeholder?: string;
  monospace?: boolean;
  get: (s: Settings) => string;
  /** Mengembalikan pesan galat, atau null bila sah. */
  validate?: (text: string) => string | null;
  set: (s: Settings, text: string) => Settings;
}

export interface NumberField extends FieldBase {
  kind: "number";
  min: number;
  max: number;
  /** Boleh dikosongkan (nilai null). */
  optional?: boolean;
  placeholder?: string;
  unit?: string;
  get: (s: Settings) => number | null;
  set: (s: Settings, value: number | null) => Settings;
}

export interface CheckboxField extends FieldBase {
  kind: "checkbox";
  get: (s: Settings) => boolean;
  set: (s: Settings, value: boolean) => Settings;
}

export interface SelectField extends FieldBase {
  kind: "select";
  options: { value: string; label: string }[];
  get: (s: Settings) => string;
  set: (s: Settings, value: string) => Settings;
}

export type Field = TextField | NumberField | CheckboxField | SelectField;

/** Teks kosong atau hanya spasi disimpan sebagai null. */
const nullable = (text: string): string | null => (text.trim() === "" ? null : text.trim());

// eslint-disable-next-line no-control-regex
const CONTROL = /[\x00-\x1f\x7f]/;

function validatePath(text: string): string | null {
  if (CONTROL.test(text)) return "The path cannot contain control characters";
  if (text.length > 4096) return "The path is too long";
  return null;
}

function validateClientId(text: string): string | null {
  const value = text.trim();
  if (value === "") return null;
  if (/\s/.test(value) || CONTROL.test(value)) return "A Client ID cannot contain spaces";
  if (value.length > 200) return "This is too long for a Client ID";
  return null;
}

export const FIELDS: Field[] = [
  {
    kind: "text",
    id: "editor.fontFamily",
    section: "editor",
    label: "Font family",
    description: "CSS font list for the editor and terminal; the first installed font is used.",
    keywords: "typeface monospace",
    monospace: true,
    get: (s) => s.editor.fontFamily,
    validate: (text) => (text.trim() === "" ? "Enter at least one font name" : CONTROL.test(text) || /[;{}]/.test(text) ? "A font list cannot contain ; { }" : text.length > 500 ? "The font list is too long" : null),
    set: (s, text) => ({ ...s, editor: { ...s.editor, fontFamily: text.trim() } }),
  },
  {
    kind: "number",
    id: "editor.fontSize",
    section: "editor",
    label: "Font size",
    description: "Editor font size in pixels.",
    min: 8,
    max: 40,
    unit: "px",
    get: (s) => s.editor.fontSize,
    set: (s, value) => ({ ...s, editor: { ...s.editor, fontSize: value ?? s.editor.fontSize } }),
  },
  {
    kind: "number",
    id: "editor.tabSize",
    section: "editor",
    label: "Tab size",
    description: "Number of columns a tab occupies, and the indent width when inserting spaces.",
    keywords: "indent indentation",
    min: 1,
    max: 16,
    get: (s) => s.editor.tabSize,
    set: (s, value) => ({ ...s, editor: { ...s.editor, tabSize: value ?? s.editor.tabSize } }),
  },
  {
    kind: "checkbox",
    id: "editor.insertSpaces",
    section: "editor",
    label: "Insert spaces",
    description: "Indent with spaces when pressing Tab instead of inserting a tab character.",
    keywords: "indent tabs",
    get: (s) => s.editor.insertSpaces,
    set: (s, value) => ({ ...s, editor: { ...s.editor, insertSpaces: value } }),
  },
  {
    kind: "checkbox",
    id: "editor.wordWrap",
    section: "editor",
    label: "Word wrap",
    description: "Wrap long lines at the edge of the editor instead of scrolling horizontally.",
    get: (s) => s.editor.wordWrap,
    set: (s, value) => ({ ...s, editor: { ...s.editor, wordWrap: value } }),
  },
  {
    kind: "checkbox",
    id: "editor.lineNumbers",
    section: "editor",
    label: "Line numbers",
    description: "Show line numbers in the editor gutter.",
    keywords: "gutter",
    get: (s) => s.editor.lineNumbers,
    set: (s, value) => ({ ...s, editor: { ...s.editor, lineNumbers: value } }),
  },
  {
    kind: "checkbox",
    id: "editor.inlineBlame",
    section: "editor",
    label: "Inline blame",
    description: "Show who last changed the current line, and when, at the end of that line.",
    keywords: "git annotation author",
    get: (s) => s.editor.inlineBlame,
    set: (s, value) => ({ ...s, editor: { ...s.editor, inlineBlame: value } }),
  },
  {
    kind: "select",
    id: "files.autoSave",
    section: "files",
    label: "Auto save",
    description: "Save changed files automatically: never, when the window loses focus, or one second after you stop typing.",
    options: [
      { value: "off", label: "Off" },
      { value: "onFocusChange", label: "On focus change" },
      { value: "afterDelay", label: "After delay" },
    ],
    get: (s) => s.files.autoSave,
    set: (s, value) => ({ ...s, files: { ...s.files, autoSave: value as Settings["files"]["autoSave"] } }),
  },
  {
    kind: "select",
    id: "workbench.theme",
    section: "appearance",
    label: "Theme",
    description: "Colour theme. System follows the light or dark setting of your operating system.",
    keywords: "color colour dark light",
    options: [
      { value: "system", label: "System" },
      { value: "dark", label: "Dark" },
      { value: "light", label: "Light" },
    ],
    get: (s) => s.workbench.theme,
    set: (s, value) => ({ ...s, workbench: { ...s.workbench, theme: value as Settings["workbench"]["theme"] } }),
  },
  {
    kind: "select",
    id: "workbench.startupView",
    section: "appearance",
    label: "Startup view",
    description: "Sidebar view shown when a folder is opened. Source Control applies only when the folder is a Git repository.",
    keywords: "sidebar scm explorer",
    options: [
      { value: "scm", label: "Source Control" },
      { value: "explorer", label: "Explorer" },
    ],
    get: (s) => s.workbench.startupView,
    set: (s, value) => ({ ...s, workbench: { ...s.workbench, startupView: value as Settings["workbench"]["startupView"] } }),
  },
  {
    kind: "text",
    id: "git.path",
    section: "git",
    label: "Path to git",
    description: "Full path of the git program. Leave empty to find git on PATH. Takes effect after restarting OnCode.",
    keywords: "executable binary",
    placeholder: "Found on PATH",
    monospace: true,
    get: (s) => s.git.path ?? "",
    validate: validatePath,
    set: (s, text) => ({ ...s, git: { ...s.git, path: nullable(text) } }),
  },
  {
    kind: "number",
    id: "git.autoFetchMinutes",
    section: "git",
    label: "Auto-fetch interval",
    description: "Fetch from the remotes in the background every this many minutes. 0 never fetches automatically.",
    min: 0,
    max: 1440,
    unit: "minutes",
    get: (s) => s.git.autoFetchMinutes,
    set: (s, value) => ({ ...s, git: { ...s.git, autoFetchMinutes: value ?? s.git.autoFetchMinutes } }),
  },
  {
    kind: "select",
    id: "git.pullStrategy",
    section: "git",
    label: "Pull strategy",
    description: "How Pull combines remote commits with yours. Fast-forward only never creates a merge commit and stops if the branches have diverged.",
    keywords: "merge rebase ff-only",
    options: [
      { value: "ffOnly", label: "Fast-forward only" },
      { value: "merge", label: "Merge" },
      { value: "rebase", label: "Rebase" },
    ],
    get: (s) => s.git.pullStrategy,
    set: (s, value) => ({ ...s, git: { ...s.git, pullStrategy: value as Settings["git"]["pullStrategy"] } }),
  },
  {
    kind: "checkbox",
    id: "git.confirmForcePush",
    section: "git",
    label: "Confirm force push",
    description: "Ask before a force push (with lease) overwrites a branch on the remote.",
    get: (s) => s.git.confirmForcePush,
    set: (s, value) => ({ ...s, git: { ...s.git, confirmForcePush: value } }),
  },
  {
    kind: "text",
    id: "terminal.shell",
    section: "terminal",
    label: "Shell",
    description: "Program started in new terminals. Leave empty to use your default shell.",
    placeholder: "Default shell",
    monospace: true,
    get: (s) => s.terminal.shell ?? "",
    validate: validatePath,
    set: (s, text) => ({ ...s, terminal: { ...s.terminal, shell: nullable(text) } }),
  },
  {
    kind: "number",
    id: "terminal.fontSize",
    section: "terminal",
    label: "Font size",
    description: "Terminal font size in pixels. Leave empty to use the editor font size.",
    min: 8,
    max: 40,
    optional: true,
    placeholder: "Editor size",
    unit: "px",
    get: (s) => s.terminal.fontSize,
    set: (s, value) => ({ ...s, terminal: { ...s.terminal, fontSize: value } }),
  },
  {
    kind: "checkbox",
    id: "lsp.enabled",
    section: "lsp",
    label: "Enable language servers",
    description: "Start a language server for completion, diagnostics and navigation when a file of its language is opened (trusted folders only).",
    keywords: "lsp intellisense completion diagnostics",
    get: (s) => s.lsp.enabled,
    set: (s, value) => ({ ...s, lsp: { ...s.lsp, enabled: value } }),
  },
  {
    kind: "text",
    id: "forge.githubClientId",
    section: "accounts",
    label: "GitHub OAuth Client ID",
    description: "Client ID of a GitHub OAuth app with device flow enabled. Needed only for browser sign-in; signing in with a personal access token works without it.",
    keywords: "login sign in device flow",
    monospace: true,
    placeholder: "Not set",
    get: (s) => s.forge.githubClientId ?? "",
    validate: validateClientId,
    set: (s, text) => ({ ...s, forge: { ...s.forge, githubClientId: nullable(text) } }),
  },
  {
    kind: "text",
    id: "forge.gitlabClientId",
    section: "accounts",
    label: "GitLab OAuth Client ID",
    description: "Application ID of a GitLab OAuth application with device authorization enabled. Needed only for browser sign-in; token sign-in works without it.",
    keywords: "login sign in device flow",
    monospace: true,
    placeholder: "Not set",
    get: (s) => s.forge.gitlabClientId ?? "",
    validate: validateClientId,
    set: (s, text) => ({ ...s, forge: { ...s.forge, gitlabClientId: nullable(text) } }),
  },
];

export function fieldById(id: string): Field {
  const field = FIELDS.find((f) => f.id === id);
  if (!field) throw new Error(`Kolom pengaturan tidak dikenal: ${id}`);
  return field;
}

/** Hasil mengurai isian angka: nilai yang sah, atau pesan galat. */
export type Parsed<T> = { ok: true; value: T } | { ok: false; message: string };

export function parseNumber(field: Pick<NumberField, "min" | "max" | "optional">, text: string): Parsed<number | null> {
  const raw = text.trim();
  if (raw === "") return field.optional ? { ok: true, value: null } : { ok: false, message: "Enter a number" };
  if (!/^\d+$/.test(raw)) return { ok: false, message: "Enter a whole number" };
  const value = Number(raw);
  if (!Number.isSafeInteger(value) || value < field.min || value > field.max) return { ok: false, message: `Enter a value from ${field.min} to ${field.max}` };
  return { ok: true, value };
}

/** Nilai kolom sebagai teks, untuk kotak isian. */
export function fieldText(field: TextField | NumberField, settings: Settings): string {
  if (field.kind === "text") return field.get(settings);
  const value = field.get(settings);
  return value === null ? "" : String(value);
}

/** Perbandingan nilai JSON (urutan kunci objek tidak berpengaruh). */
export function sameValue(a: unknown, b: unknown): boolean {
  if (a === b) return true;
  if (typeof a !== "object" || typeof b !== "object" || a === null || b === null) return false;
  if (Array.isArray(a) !== Array.isArray(b)) return false;
  if (Array.isArray(a) && Array.isArray(b)) return a.length === b.length && a.every((v, i) => sameValue(v, b[i]));
  const ka = Object.keys(a as object);
  const kb = Object.keys(b as object);
  if (ka.length !== kb.length) return false;
  return ka.every((k) => k in (b as object) && sameValue((a as Record<string, unknown>)[k], (b as Record<string, unknown>)[k]));
}

/** Mengembalikan satu kolom ke nilai bawaannya. */
export function applyDefault(field: Field, settings: Settings, defaults: Settings): Settings {
  switch (field.kind) {
    case "text":
      return field.set(settings, field.get(defaults));
    case "number":
      return field.set(settings, field.get(defaults));
    case "checkbox":
      return field.set(settings, field.get(defaults));
    case "select":
      return field.set(settings, field.get(defaults));
  }
}

/** Nilai kolom ini berbeda dari nilai bawaan. */
export function differsFromDefault(field: Field, settings: Settings, defaults: Settings): boolean {
  return !sameValue(field.get(settings), field.get(defaults));
}

/** Nilai bawaan sebuah kolom sebagai teks untuk tooltip. */
export function defaultLabel(field: Field, defaults: Settings): string {
  switch (field.kind) {
    case "checkbox":
      return field.get(defaults) ? "on" : "off";
    case "select": {
      const value = field.get(defaults);
      return field.options.find((o) => o.value === value)?.label ?? value;
    }
    case "number": {
      const value = field.get(defaults);
      return value === null ? "empty" : String(value);
    }
    case "text":
      return field.get(defaults) || "empty";
  }
}

/** Jumlah hal yang berbeda dari bawaan: kolom formulir ditambah penimpaan language server. */
export function countNonDefault(settings: Settings, defaults: Settings): number {
  const fields = FIELDS.filter((f) => differsFromDefault(f, settings, defaults)).length;
  const languages = new Set([...Object.keys(settings.lsp.servers), ...Object.keys(defaults.lsp.servers)]);
  let servers = 0;
  for (const language of languages) if (!sameValue(settings.lsp.servers[language] ?? null, defaults.lsp.servers[language] ?? null)) servers += 1;
  return fields + servers;
}

export interface OverrideContext {
  /** Folder proyek dipercaya (pengaturan proyek hanya berlaku di folder yang dipercaya). */
  trusted: boolean;
  /** Pengaturan pengguna yang tersimpan. */
  user: Settings;
  /** Pengaturan efektif jendela ini (pengguna + proyek). */
  effective: Settings | null;
}

/** Kolom ini sedang ditimpa oleh `.oncode/settings.json` proyek. */
export function isOverridden(field: Field, context: OverrideContext): boolean {
  if (!context.trusted || !context.effective) return false;
  return !sameValue(field.get(context.user), field.get(context.effective));
}

/** Server untuk bahasa ini ditimpa oleh pengaturan proyek. */
export function isServerOverridden(language: string, context: OverrideContext): boolean {
  if (!context.trusted || !context.effective) return false;
  return !sameValue(normalizeSpec(context.user.lsp.servers[language]), normalizeSpec(context.effective.lsp.servers[language]));
}

function normalizeSpec(spec: ServerSpec | undefined): unknown {
  if (!spec) return null;
  return { command: spec.command, args: spec.args ?? [], env: spec.env ?? [] };
}

/** Kueri pencarian cocok bila setiap katanya muncul di salah satu teks. */
export function matchesQuery(query: string, ...texts: (string | undefined)[]): boolean {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  if (words.length === 0) return true;
  const haystack = texts.filter(Boolean).join("\n").toLowerCase();
  return words.every((w) => haystack.includes(w));
}

export function fieldMatches(field: Field, query: string): boolean {
  const section = SECTIONS.find((s) => s.id === field.section)?.title;
  const options = field.kind === "select" ? field.options.map((o) => o.label).join(" ") : "";
  return matchesQuery(query, field.label, field.description, field.id, section, field.keywords, options);
}

/** Memecah baris argumen seperti shell sederhana: spasi memisah, kutip tunggal/ganda mengelompokkan. */
export function splitArgs(text: string): Parsed<string[]> {
  const args: string[] = [];
  let current = "";
  let quote: '"' | "'" | null = null;
  let started = false;
  for (let i = 0; i < text.length; i++) {
    const ch = text[i];
    if (quote) {
      if (ch === quote) quote = null;
      else if (ch === "\\" && quote === '"' && i + 1 < text.length && (text[i + 1] === '"' || text[i + 1] === "\\")) current += text[++i];
      else current += ch;
    } else if (ch === '"' || ch === "'") {
      quote = ch;
      started = true;
    } else if (/\s/.test(ch)) {
      if (started) args.push(current);
      current = "";
      started = false;
    } else {
      current += ch;
      started = true;
    }
  }
  if (quote) return { ok: false, message: "A quote is not closed" };
  if (started) args.push(current);
  return { ok: true, value: args };
}

/** Kebalikan `splitArgs`: argumen yang memuat spasi atau kutip dibungkus kutip ganda. */
export function joinArgs(args: string[]): string {
  return args.map((a) => (a === "" || /[\s"']/.test(a) ? `"${a.replace(/(["\\])/g, "\\$1")}"` : a)).join(" ");
}

/** Menetapkan atau menghapus (spec = null) penimpaan server untuk satu bahasa. */
export function withServerOverride(settings: Settings, language: string, spec: ServerSpec | null): Settings {
  const servers = { ...settings.lsp.servers };
  if (spec === null) delete servers[language];
  else servers[language] = spec;
  return { ...settings, lsp: { ...settings.lsp, servers } };
}

export function validateServerCommand(text: string): string | null {
  if (text.trim() === "") return "Enter a command";
  return validatePath(text);
}
