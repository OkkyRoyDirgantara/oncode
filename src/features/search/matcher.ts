// Pencocok di sisi JS untuk berkas yang sedang terbuka di editor. Harus berperilaku sama dengan
// backend (crate `grep-regex` + `regex`): baris demi baris, kecocokan kosong dilewati, kata utuh
// memakai batas kata Unicode, dan `$1`/`${nama}`/`$$` pada teks pengganti mengikuti aturan `regex`.
//
// Sintaks `regex` Rust tidak sama dengan `RegExp` JS. Pola diterjemahkan konstruksi demi
// konstruksi; apa pun yang tidak dikenali ditolak (bukan ditebak), agar hasil di editor tidak
// pernah diam-diam berbeda dari hasil backend.
import type { SearchQuery } from "@/ipc";

export type MatchOptions = Pick<SearchQuery, "pattern" | "isRegex" | "caseSensitive" | "wholeWord">;

export interface LineHit {
  /** Awal dan akhir kecocokan di dalam baris, dalam unit UTF-16 (indeks string JS). */
  start: number;
  end: number;
  /** Isi grup tangkapan; indeks 0 adalah seluruh kecocokan. */
  groups: (string | undefined)[];
  named: Record<string, string | undefined>;
}

export interface Matcher {
  /** Semua kecocokan tak kosong pada satu baris (tanpa akhir baris), dari kiri ke kanan dan tidak tumpang-tindih. */
  find(line: string): LineHit[];
}

export type CompileResult = { ok: true; matcher: Matcher } | { ok: false; reason: string };

/** Karakter kata menurut `\w` Unicode milik crate `regex` (UTS #18). */
const WORD = "\\p{Alphabetic}\\p{M}\\p{Nd}\\p{Pc}\\p{Join_Control}";
const BOUNDARY = `(?:(?<=[${WORD}])(?![${WORD}])|(?<![${WORD}])(?=[${WORD}]))`;
const NOT_BOUNDARY = `(?:(?<=[${WORD}])(?=[${WORD}])|(?<![${WORD}])(?![${WORD}]))`;
/** Karakter yang boleh (dan perlu) di-escape pada `RegExp` mode `u`. */
const SYNTAX = "^$\\.*+?()[]{}|/";
const WORD_CHAR = new RegExp(`[${WORD}]`, "u");

class Untranslatable extends Error {}

function unsupported(what: string): never {
  throw new Untranslatable(`${what} cannot be applied inside the editor`);
}

function isValidProperty(body: string): boolean {
  try {
    new RegExp(`\\p{${body}}`, "u");
    return true;
  } catch {
    return false;
  }
}

/** `\pL`, `\p{Greek}`, `\P{L}`, `\p{sc:Greek}` → bentuk yang dikenal JS. */
function translateProperty(pattern: string, at: number): [string, number] {
  const m = /^\\([pP])(?:([A-Za-z])|\{([^{}^]+)\})/.exec(pattern.slice(at));
  if (!m) unsupported("This Unicode class");
  const negated = m[1] === "P";
  const body = (m[2] ?? m[3]).trim().replace(/\s*[:=]\s*/, "=");
  // `\p{Greek}` di Rust berarti aksara Greek; JS mengharuskan `Script=Greek`.
  const known = [body, `Script=${body}`].find((candidate) => !candidate.includes("!") && isValidProperty(candidate));
  if (!known) unsupported(`The Unicode class ${m[0]}`);
  return [`\\${negated ? "P" : "p"}{${known}}`, at + m[0].length];
}

/** Menerjemahkan satu escape yang dimulai di `at` (posisi garis miring terbalik). */
function translateEscape(pattern: string, at: number, inClass: boolean): [string, number] {
  const c = pattern[at + 1];
  if (c === undefined) unsupported("A trailing backslash");
  const rest = pattern.slice(at);
  switch (c) {
    case "d":
      return ["\\p{Nd}", at + 2];
    case "D":
      return ["\\P{Nd}", at + 2];
    case "s":
      return ["\\p{White_Space}", at + 2];
    case "S":
      return ["\\P{White_Space}", at + 2];
    case "w":
      return [inClass ? WORD : `[${WORD}]`, at + 2];
    case "W":
      if (inClass) unsupported("\\W inside a character class");
      return [`[^${WORD}]`, at + 2];
    case "b":
      if (inClass || pattern[at + 2] === "{") unsupported(inClass ? "\\b inside a character class" : "The \\b{…} word boundary");
      return [BOUNDARY, at + 2];
    case "B":
      if (inClass) unsupported("\\B inside a character class");
      return [NOT_BOUNDARY, at + 2];
    // `\A` dan `\z` sengaja ditolak: pencarian backend memperlakukannya sebagai awal/akhir berkas,
    // sedangkan penggantiannya (baris demi baris) sebagai awal/akhir baris.
    case "A":
    case "z":
      unsupported(`The anchor \\${c}`);
    case "n":
    case "r":
    case "t":
    case "f":
    case "v":
      return [`\\${c}`, at + 2];
    case "a":
      return ["\\x07", at + 2];
    case "x": {
      const m = /^\\x(?:([0-9A-Fa-f]{2})|\{([0-9A-Fa-f]{1,6})\})/.exec(rest);
      if (!m) unsupported("This \\x escape");
      return [m[1] ? `\\x${m[1]}` : `\\u{${m[2]}}`, at + m[0].length];
    }
    case "u": {
      const m = /^\\u(?:([0-9A-Fa-f]{4})|\{([0-9A-Fa-f]{1,6})\})/.exec(rest);
      if (!m) unsupported("This \\u escape");
      return [m[1] ? `\\u${m[1]}` : `\\u{${m[2]}}`, at + m[0].length];
    }
    case "U": {
      const m = /^\\U(?:([0-9A-Fa-f]{8})|\{([0-9A-Fa-f]{1,6})\})/.exec(rest);
      if (!m) unsupported("This \\U escape");
      return [`\\u{${(m[1] ?? m[2]).replace(/^0+(?=.)/, "")}}`, at + m[0].length];
    }
    case "p":
    case "P":
      return translateProperty(pattern, at);
    default:
      // Rust hanya mengizinkan escape tanda baca ASCII; huruf dan angka lain punya arti khusus atau tidak sah.
      if (/[A-Za-z0-9]/.test(c) || c.charCodeAt(0) > 0x7e || c === "<" || c === ">") unsupported(`The escape \\${c}`);
      if (SYNTAX.includes(c) || (inClass && c === "-")) return [`\\${c}`, at + 2];
      return [c, at + 2];
  }
}

/** Menerjemahkan kelas karakter `[...]` yang dimulai di `at`. */
function translateClass(pattern: string, at: number): [string, number] {
  let i = at + 1;
  let out = "[";
  if (pattern[i] === "^") {
    out += "^";
    i += 1;
  }
  // `]` tepat setelah pembuka adalah literal di Rust; di JS itu menutup kelas kosong.
  if (pattern[i] === "]") {
    out += "\\]";
    i += 1;
  }
  while (i < pattern.length) {
    const c = pattern[i];
    if (c === "]") return [`${out}]`, i + 1];
    if (c === "[") unsupported("Nested and POSIX character classes ([[:alpha:]])");
    if (c === "\\") {
      const [text, next] = translateEscape(pattern, i, true);
      out += text;
      i = next;
      continue;
    }
    if ((c === "&" || c === "-" || c === "~") && pattern[i + 1] === c) unsupported("Character class set operations (&&, --, ~~)");
    out += c;
    i += 1;
  }
  unsupported("An unclosed character class");
}

/**
 * Menerjemahkan pola crate `regex` menjadi sumber `RegExp` JS (flag `u`). Melempar bila pola
 * memakai konstruksi yang tidak punya padanan pasti: flag sebaris `(?i)`, kelas bersarang, dsb.
 */
export function translateRegex(pattern: string): string {
  let out = "";
  let i = 0;
  while (i < pattern.length) {
    const c = pattern[i];
    if (c === "\\") {
      const [text, next] = translateEscape(pattern, i, false);
      out += text;
      i = next;
    } else if (c === "[") {
      const [text, next] = translateClass(pattern, i);
      out += text;
      i = next;
    } else if (c === "(" && pattern[i + 1] === "?") {
      const rest = pattern.slice(i + 2);
      const named = /^P?<([A-Za-z_][A-Za-z0-9_]*)>/.exec(rest);
      if (rest.startsWith(":")) {
        out += "(?:";
        i += 3;
      } else if (named) {
        out += `(?<${named[1]}>`;
        i += 2 + named[0].length;
      } else {
        unsupported("Inline flags such as (?i)");
      }
    } else if (c === ".") {
      // JS juga mengecualikan U+2028/2029 dari `.`; Rust tidak.
      out += "[^\\n\\r]";
      i += 1;
    } else if (c === "{") {
      const m = /^\{\d+(?:,\d*)?\}/.exec(pattern.slice(i));
      if (!m) unsupported("This use of {");
      out += m[0];
      i += m[0].length;
    } else if (c === "}" || c === "]") {
      // Literal di Rust, galat sintaks pada mode `u` JS.
      out += `\\${c}`;
      i += 1;
    } else {
      out += c;
      i += 1;
    }
  }
  return out;
}

function escapeLiteral(text: string): string {
  return text.replace(/[\\^$.*+?()[\]{}|/]/g, "\\$&");
}

/** Indeks karakter berikutnya, tanpa membelah pasangan surrogate. */
function nextIndex(line: string, index: number): number {
  const code = line.charCodeAt(index);
  const pair = code >= 0xd800 && code <= 0xdbff && index + 1 < line.length && (line.charCodeAt(index + 1) & 0xfc00) === 0xdc00;
  return index + (pair ? 2 : 1);
}

/**
 * Benar bila karakter tepat sebelum `index` adalah karakter kata. Karakternya diambil sendiri
 * (termasuk pasangan surrogate) alih-alih memakai `[...]$`: V8 salah menangani kelas yang diikuti
 * `$` pada regex tanpa flag `g`/`y` bila karakter terakhirnya astral.
 */
function followsWordChar(line: string, index: number): boolean {
  if (index <= 0) return false;
  const low = line.charCodeAt(index - 1);
  const pair = index >= 2 && (low & 0xfc00) === 0xdc00 && (line.charCodeAt(index - 2) & 0xfc00) === 0xd800;
  return WORD_CHAR.test(line.slice(index - (pair ? 2 : 1), index));
}

/** Menyusun pencocok untuk sebuah kueri, atau alasan mengapa kueri itu tidak bisa dijalankan di sisi JS. */
export function compileMatcher(options: MatchOptions): CompileResult {
  if (options.pattern === "") return { ok: false, reason: "The search pattern is empty" };
  let source: string;
  if (options.isRegex) {
    try {
      source = translateRegex(options.pattern);
    } catch (e) {
      if (e instanceof Untranslatable) return { ok: false, reason: e.message };
      throw e;
    }
  } else {
    source = escapeLiteral(options.pattern);
  }
  // Kata utuh: sama dengan `\b{start-half}(?:pola)\b{end-half}` milik grep-regex. Separuh akhir
  // menjadi lookahead; separuh awal diperiksa per kandidat di `find` agar tidak bergantung pada lookbehind.
  if (options.wholeWord) source = `(?:${source})(?![${WORD}])`;

  let regex: RegExp;
  try {
    regex = new RegExp(source, options.caseSensitive ? "gu" : "giu");
  } catch {
    return { ok: false, reason: "This pattern uses syntax that cannot be applied inside the editor" };
  }

  const find = (line: string): LineHit[] => {
    const hits: LineHit[] = [];
    regex.lastIndex = 0;
    let m: RegExpExecArray | null;
    while (regex.lastIndex <= line.length && (m = regex.exec(line)) !== null) {
      const start = m.index;
      const end = start + m[0].length;
      const afterWordChar = options.wholeWord && followsWordChar(line, start);
      // Kecocokan kosong tidak dilaporkan backend; kandidat yang menempel pada karakter kata dicoba lagi dari karakter berikutnya.
      if (end === start || afterWordChar) {
        regex.lastIndex = nextIndex(line, start);
        continue;
      }
      hits.push({ start, end, groups: [...m], named: m.groups ?? {} });
    }
    return hits;
  };
  return { ok: true, matcher: { find } };
}

/**
 * Mengisi `$1`, `$nama`, `${nama}`, dan `$$` pada teks pengganti, mengikuti `Captures::interpolate`
 * crate `grep-matcher`: nama tanpa kurung kurawal memakan huruf/angka/garis bawah sebanyak mungkin
 * (`$1a` adalah grup bernama "1a"), dan grup yang tidak ada atau tidak ikut cocok menjadi teks kosong.
 */
export function expandReplacement(template: string, hit: Pick<LineHit, "groups" | "named">): string {
  let out = "";
  let i = 0;
  for (;;) {
    const dollar = template.indexOf("$", i);
    if (dollar < 0) break;
    out += template.slice(i, dollar);
    i = dollar;
    if (template[i + 1] === "$") {
      out += "$";
      i += 2;
      continue;
    }
    const braced = template[i + 1] === "{";
    const nameStart = i + (braced ? 2 : 1);
    let nameEnd = nameStart;
    while (nameEnd < template.length && /[0-9A-Za-z_]/.test(template[nameEnd])) nameEnd += 1;
    if (nameEnd === nameStart || (braced && template[nameEnd] !== "}")) {
      out += "$";
      i += 1;
      continue;
    }
    const name = template.slice(nameStart, nameEnd);
    i = nameEnd + (braced ? 1 : 0);
    const value = /^[0-9]+$/.test(name) && Number(name) <= 0xffffffff ? hit.groups[Number(name)] : Object.hasOwn(hit.named, name) ? hit.named[name] : undefined;
    out += value ?? "";
  }
  return out + template.slice(i);
}

export interface LineEdit {
  /** Nomor baris, mulai dari 1. */
  line: number;
  /** Rentang yang diganti di dalam baris, dalam unit UTF-16. */
  from: number;
  to: number;
  insert: string;
}

/**
 * Suntingan untuk mengganti setiap kecocokan pada `lines`. `interpolate` (mode regex) menafsirkan
 * `$1` dkk.; pada mode literal teks pengganti dipakai apa adanya. `skip` menyingkirkan kecocokan tertentu.
 */
export function computeEdits(
  matcher: Matcher,
  lines: Iterable<string>,
  replacement: string,
  interpolate: boolean,
  skip?: (line: number, column: number, length: number) => boolean,
): LineEdit[] {
  const edits: LineEdit[] = [];
  let number = 0;
  for (const text of lines) {
    number += 1;
    for (const hit of matcher.find(text)) {
      if (skip?.(number, hit.start, hit.end - hit.start)) continue;
      edits.push({ line: number, from: hit.start, to: hit.end, insert: interpolate ? expandReplacement(replacement, hit) : replacement });
    }
  }
  return edits;
}

/** Menerapkan suntingan (urut seperti keluaran `computeEdits`) pada salinan `lines`. */
export function applyEdits(lines: readonly string[], edits: readonly LineEdit[]): string[] {
  const out = [...lines];
  // Dari belakang, agar suntingan di baris yang sama tidak menggeser rentang sebelumnya.
  for (let i = edits.length - 1; i >= 0; i--) {
    const edit = edits[i];
    const text = out[edit.line - 1];
    out[edit.line - 1] = text.slice(0, edit.from) + edit.insert + text.slice(edit.to);
  }
  return out;
}
