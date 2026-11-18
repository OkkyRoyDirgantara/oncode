import { describe, expect, it } from "vitest";
import { bindingOf, classifyKey, type KeyLike } from "./keys";

/** Event tiruan dari notasi seperti "Ctrl+Shift+KeyC" (bagian terakhir adalah `code`). */
function key(spec: string, keyOverride?: string): KeyLike {
  const parts = spec.split("+");
  const code = parts.pop()!;
  const letter = /^Key([A-Z])$/.exec(code);
  const shift = parts.includes("Shift");
  const base = letter ? (shift ? letter[1] : letter[1].toLowerCase()) : code === "Backquote" ? "`" : code;
  return {
    key: keyOverride ?? base,
    code,
    ctrlKey: parts.includes("Ctrl"),
    metaKey: parts.includes("Meta"),
    altKey: parts.includes("Alt"),
    shiftKey: shift,
  };
}

// Keybinding bawaan command yang tetap berlaku di terminal.
const appBindings = new Set(["Mod+`", "Mod+Shift+`", "Mod+J", "Mod+Shift+P", "F1", "Mod+P", "Mod+B"]);
const linux = (spec: string, k?: string) => classifyKey(key(spec, k), { isMac: false, appBindings });
const mac = (spec: string, k?: string) => classifyKey(key(spec, k), { isMac: true, appBindings });

describe("bindingOf", () => {
  it("memetakan Ctrl ke Mod di Linux/Windows dan Cmd ke Mod di macOS", () => {
    expect(bindingOf(key("Ctrl+KeyP"), false)).toBe("Mod+P");
    expect(bindingOf(key("Meta+KeyP"), true)).toBe("Mod+P");
    expect(bindingOf(key("Ctrl+KeyP"), true)).toBe("Ctrl+P");
    expect(bindingOf(key("Meta+KeyP"), false)).toBe("P");
  });

  it("memakai `code` untuk huruf dan backquote sehingga Shift tidak mengubah hasil", () => {
    expect(bindingOf(key("Ctrl+Shift+KeyC"), false)).toBe("Mod+Shift+C");
    expect(bindingOf(key("Ctrl+Shift+Backquote", "~"), false)).toBe("Mod+Shift+`");
    expect(bindingOf(key("Ctrl+Alt+KeyS"), false)).toBe("Mod+Alt+S");
  });

  it("mengembalikan null untuk tombol modifier saja", () => {
    expect(bindingOf({ key: "Control", code: "ControlLeft", ctrlKey: true, metaKey: false, altKey: false, shiftKey: false }, false)).toBeNull();
    expect(bindingOf({ key: "Shift", code: "ShiftLeft", ctrlKey: false, metaKey: false, altKey: false, shiftKey: true }, true)).toBeNull();
  });
});

describe("classifyKey di Linux/Windows", () => {
  it("Ctrl+C polos sampai ke shell (SIGINT); Ctrl+Shift+C/V menyalin dan menempel", () => {
    expect(linux("Ctrl+KeyC")).toBe("shell");
    expect(linux("Ctrl+KeyV")).toBe("shell");
    expect(linux("Ctrl+Shift+KeyC")).toBe("copy");
    expect(linux("Ctrl+Shift+KeyV")).toBe("paste");
  });

  it("keybinding aplikasi yang diizinkan dibiarkan untuk aplikasi", () => {
    expect(linux("Ctrl+Backquote")).toBe("app");
    expect(linux("Ctrl+Shift+Backquote", "~")).toBe("app");
    expect(linux("Ctrl+KeyJ")).toBe("app");
    expect(linux("Ctrl+Shift+KeyP")).toBe("app");
    expect(linux("Ctrl+KeyP")).toBe("app");
    expect(linux("Ctrl+KeyB")).toBe("app");
    expect(linux("F1")).toBe("app");
  });

  it("tombol readline dan tombol biasa diteruskan ke shell", () => {
    for (const spec of ["Ctrl+KeyW", "Ctrl+KeyS", "Ctrl+KeyO", "Ctrl+KeyG", "Ctrl+KeyR", "Ctrl+KeyD", "Ctrl+KeyL", "Ctrl+KeyA", "Alt+KeyB", "KeyA", "Shift+KeyA", "Enter", "Tab", "Escape", "F12", "F2", "ArrowUp"]) {
      expect(linux(spec), spec).toBe("shell");
    }
  });

  it("Ctrl+F membuka bilah cari; tombol Meta (Super) bukan Mod", () => {
    expect(linux("Ctrl+KeyF")).toBe("find");
    expect(linux("Meta+KeyC")).toBe("shell");
    expect(linux("Meta+KeyP")).toBe("shell");
  });

  it("tombol modifier saja selalu milik shell", () => {
    expect(classifyKey({ key: "Control", code: "ControlLeft", ctrlKey: true, metaKey: false, altKey: false, shiftKey: false }, { isMac: false, appBindings })).toBe("shell");
  });
});

describe("classifyKey di macOS", () => {
  it("Cmd+C/V menyalin dan menempel; Ctrl+C tetap SIGINT", () => {
    expect(mac("Meta+KeyC")).toBe("copy");
    expect(mac("Meta+KeyV")).toBe("paste");
    expect(mac("Meta+Shift+KeyC")).toBe("copy");
    expect(mac("Meta+Shift+KeyV")).toBe("paste");
    expect(mac("Ctrl+KeyC")).toBe("shell");
    expect(mac("Ctrl+KeyV")).toBe("shell");
  });

  it("keybinding aplikasi memakai Cmd; Ctrl+huruf diteruskan ke shell", () => {
    expect(mac("Meta+Backquote")).toBe("app");
    expect(mac("Meta+KeyJ")).toBe("app");
    expect(mac("Meta+Shift+KeyP")).toBe("app");
    expect(mac("Meta+KeyP")).toBe("app");
    expect(mac("Meta+KeyB")).toBe("app");
    for (const spec of ["Ctrl+KeyP", "Ctrl+KeyB", "Ctrl+KeyJ", "Ctrl+KeyW", "Ctrl+KeyR", "Ctrl+Backquote"]) {
      expect(mac(spec), spec).toBe("shell");
    }
  });

  it("kombinasi Cmd lain milik aplikasi karena tidak berarti bagi shell", () => {
    expect(mac("Meta+KeyW")).toBe("app");
    expect(mac("Meta+KeyS")).toBe("app");
    expect(mac("Meta+KeyQ")).toBe("app");
    expect(mac("Meta+KeyF")).toBe("find");
  });

  it("tombol biasa dan Option diteruskan ke shell", () => {
    for (const spec of ["KeyA", "Enter", "Alt+KeyB", "F12"]) expect(mac(spec), spec).toBe("shell");
  });
});
