import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { installKeybindings, registerCommands, resetCommandsForTest, setKeybindingOverrides } from "@/app/commands";
import { appBindings, installTerminalKeyShield } from "./shield";

// jsdom melaporkan platform non-Mac, jadi Mod = Ctrl di sini.
function press(target: Element, init: KeyboardEventInit): KeyboardEvent {
  const event = new KeyboardEvent("keydown", { bubbles: true, cancelable: true, ...init });
  target.dispatchEvent(event);
  return event;
}

describe("perisai tombol terminal", () => {
  let disposers: (() => void)[] = [];
  let terminalInput: HTMLTextAreaElement;
  let otherInput: HTMLInputElement;
  let reachedTerminal: KeyboardEvent[];
  const closeTab = vi.fn();
  const quickOpen = vi.fn();
  const toggle = vi.fn();

  beforeEach(() => {
    resetCommandsForTest();
    closeTab.mockClear();
    quickOpen.mockClear();
    toggle.mockClear();
    registerCommands([
      { id: "workbench.closeTab", title: "Close", keybinding: "Mod+W", run: closeTab },
      { id: "workbench.quickOpen", title: "Go to File", keybinding: "Mod+P", run: quickOpen },
      { id: "terminal.toggle", title: "Toggle Terminal", keybinding: "Mod+`", run: toggle },
    ]);
    document.body.innerHTML = '<div class="tm-host"><textarea></textarea></div><input id="other" />';
    terminalInput = document.querySelector("textarea")!;
    otherInput = document.querySelector("#other")!;
    reachedTerminal = [];
    terminalInput.addEventListener("keydown", (e) => reachedTerminal.push(e));
    // Urutan yang sama dengan main.tsx: fitur didaftarkan sebelum penangan keybinding global.
    disposers = [installTerminalKeyShield(), installKeybindings()];
  });

  afterEach(() => {
    disposers.forEach((d) => d());
    document.body.innerHTML = "";
    resetCommandsForTest();
  });

  it("mengumpulkan keybinding command yang diizinkan, termasuk penimpaan pengguna", () => {
    expect([...appBindings()].sort()).toEqual(["Mod+P", "Mod+`"]);
    setKeybindingOverrides({ "workbench.quickOpen": "Mod+E" });
    expect([...appBindings()].sort()).toEqual(["Mod+E", "Mod+`"]);
  });

  it("Ctrl+W di terminal sampai ke xterm dan tidak menutup tab", () => {
    press(terminalInput, { key: "w", code: "KeyW", ctrlKey: true });
    expect(closeTab).not.toHaveBeenCalled();
    expect(reachedTerminal).toHaveLength(1);
  });

  it("Ctrl+W di luar terminal tetap menjalankan command aplikasi", () => {
    press(otherInput, { key: "w", code: "KeyW", ctrlKey: true });
    expect(closeTab).toHaveBeenCalledTimes(1);
  });

  it("keybinding aplikasi yang diizinkan tetap berjalan saat terminal fokus dan tidak sampai ke xterm", () => {
    press(terminalInput, { key: "p", code: "KeyP", ctrlKey: true });
    press(terminalInput, { key: "`", code: "Backquote", ctrlKey: true });
    expect(quickOpen).toHaveBeenCalledTimes(1);
    expect(toggle).toHaveBeenCalledTimes(1);
    expect(reachedTerminal).toHaveLength(0);
  });

  it("tombol biasa tidak disentuh sehingga masukan teks tetap berjalan", () => {
    const event = press(terminalInput, { key: "a", code: "KeyA" });
    expect(event.defaultPrevented).toBe(false);
    expect(reachedTerminal).toHaveLength(1);
  });
});
