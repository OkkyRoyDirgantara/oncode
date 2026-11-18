// Tema xterm diturunkan dari token warna aplikasi (styles/theme.css).
import type { ITheme } from "@xterm/xterm";

/** Membaca nilai sebuah variabel CSS, mis. `--bg`; string kosong bila tidak ada. */
export type TokenReader = (name: string) => string;

export function terminalTheme(read: TokenReader): ITheme {
  const token = (name: string, fallback: string) => read(name).trim() || fallback;
  return {
    background: token("--bg", "#1d1f27"),
    foreground: token("--fg", "#d9dce3"),
    cursor: token("--fg", "#d9dce3"),
    cursorAccent: token("--bg", "#1d1f27"),
    selectionBackground: token("--selection", "rgba(79, 156, 255, 0.28)"),
    black: token("--bg-sunken", "#17191f"),
    red: token("--danger", "#f0616d"),
    green: token("--ok", "#7cc98a"),
    yellow: token("--warn", "#e3b556"),
    blue: token("--accent", "#4f9cff"),
    magenta: token("--syn-keyword", "#c792ea"),
    cyan: token("--syn-type", "#5fd0c5"),
    white: token("--fg-muted", "#9299ab"),
    brightBlack: token("--fg-faint", "#636a7d"),
    brightRed: token("--syn-tag", "#f08a8a"),
    brightGreen: token("--syn-string", "#a5d6a7"),
    brightYellow: token("--syn-property", "#e3c48a"),
    brightBlue: token("--syn-function", "#82b1ff"),
    brightMagenta: token("--lane-3", "#d58cf0"),
    brightCyan: token("--lane-5", "#4fd0d0"),
    brightWhite: token("--fg", "#d9dce3"),
  };
}

/** Tema untuk keadaan dokumen saat ini (dipanggil ulang pada event `oncode:theme`). */
export function currentTerminalTheme(): ITheme {
  const style = getComputedStyle(document.documentElement);
  return terminalTheme((name) => style.getPropertyValue(name));
}
