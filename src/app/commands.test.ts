import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  canonicalKeybinding,
  commandForKeybinding,
  executeCommand,
  formatKeybinding,
  installKeybindings,
  keybindingFromEvent,
  registerCommand,
  resetCommandsForTest,
  setKeybindingOverrides,
} from "./commands";

function key(init: Partial<KeyboardEvent> & { key: string; code?: string }) {
  return { code: "", ctrlKey: false, metaKey: false, altKey: false, shiftKey: false, ...init } as KeyboardEvent;
}

describe("keybinding", () => {
  beforeEach(() => resetCommandsForTest());

  it("menormalkan penulisan", () => {
    expect(canonicalKeybinding("shift+ctrl+p")).toBe("Mod+Shift+P");
    expect(canonicalKeybinding("Mod+`")).toBe("Mod+`");
    expect(canonicalKeybinding("alt+ArrowDown")).toBe("Alt+Down");
    expect(canonicalKeybinding("F12")).toBe("F12");
    expect(canonicalKeybinding("Ctrl+\\")).toBe("Mod+\\");
  });

  it("membaca event keyboard", () => {
    expect(keybindingFromEvent(key({ key: "P", code: "KeyP", ctrlKey: true, shiftKey: true }))).toBe("Mod+Shift+P");
    expect(keybindingFromEvent(key({ key: "p", code: "KeyP", ctrlKey: true }))).toBe("Mod+P");
    expect(keybindingFromEvent(key({ key: "ArrowDown", code: "ArrowDown", altKey: true }))).toBe("Alt+Down");
    expect(keybindingFromEvent(key({ key: "F2", code: "F2" }))).toBe("F2");
    expect(keybindingFromEvent(key({ key: "~", code: "Backquote", ctrlKey: true, shiftKey: true }))).toBe("Mod+Shift+`");
    expect(keybindingFromEvent(key({ key: "Control", code: "ControlLeft", ctrlKey: true }))).toBeNull();
  });

  it("menampilkan keybinding untuk pengguna", () => {
    expect(formatKeybinding("Mod+Shift+P")).toBe("Ctrl+Shift+P");
    expect(formatKeybinding("Alt+Down")).toBe("Alt+↓");
  });

  it("memilih command aktif dan menghormati `when`", () => {
    let enabled = false;
    registerCommand({ id: "a", title: "A", keybinding: "Mod+K", run: () => "a" });
    registerCommand({ id: "b", title: "B", keybinding: "Mod+K", when: () => enabled, run: () => "b" });
    expect(commandForKeybinding("Mod+K")?.id).toBe("a");
    enabled = true;
    expect(commandForKeybinding("Mod+K")?.id).toBe("b");
  });

  it("menerapkan penimpaan pengguna", () => {
    registerCommand({ id: "a", title: "A", keybinding: "Mod+K", run: () => 1 });
    setKeybindingOverrides({ a: "ctrl+shift+k" });
    expect(commandForKeybinding("Mod+K")).toBeUndefined();
    expect(commandForKeybinding("Mod+Shift+K")?.id).toBe("a");
    setKeybindingOverrides({ a: [] });
    expect(commandForKeybinding("Mod+Shift+K")).toBeUndefined();
  });

  it("menjalankan command dari keyboard dan mencegah aksi bawaan", () => {
    const run = vi.fn();
    registerCommand({ id: "save", title: "Save", keybinding: "Mod+S", run });
    const dispose = installKeybindings(window);
    const event = new KeyboardEvent("keydown", { key: "s", code: "KeyS", ctrlKey: true, cancelable: true, bubbles: true });
    window.dispatchEvent(event);
    expect(run).toHaveBeenCalledTimes(1);
    expect(event.defaultPrevented).toBe(true);
    dispose();
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "s", code: "KeyS", ctrlKey: true }));
    expect(run).toHaveBeenCalledTimes(1);
  });

  it("executeCommand menolak id yang tidak dikenal dan melewati command nonaktif", async () => {
    await expect(executeCommand("tidak.ada")).rejects.toThrow();
    const run = vi.fn(() => 7);
    registerCommand({ id: "x", title: "X", when: () => false, run });
    await expect(executeCommand("x")).resolves.toBeUndefined();
    expect(run).not.toHaveBeenCalled();
  });
});
