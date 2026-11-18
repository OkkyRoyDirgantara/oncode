import { afterEach, describe, expect, it, vi } from "vitest";
import { useApp } from "@/app/store";
import type { Settings } from "@/ipc";
import { createXtermAdapter } from "./xterm";
import { terminalTheme } from "./theme";

describe("tema terminal", () => {
  it("diturunkan dari token warna aplikasi, dengan cadangan bila token kosong", () => {
    const tokens: Record<string, string> = { "--bg": " #ffffff ", "--fg": "#1f2330", "--danger": "#c9303f" };
    const theme = terminalTheme((name) => tokens[name] ?? "");
    expect(theme.background).toBe("#ffffff");
    expect(theme.foreground).toBe("#1f2330");
    expect(theme.brightWhite).toBe("#1f2330");
    expect(theme.red).toBe("#c9303f");
    expect(theme.green).toBe("#7cc98a");
  });
});

// jsdom tidak punya kanvas, WebGL, maupun tata letak: yang diuji di sini hanya bahwa adapter
// bisa dibuat, dipasang, menerima tulisan, dan dibuang tanpa galat (jalur cadangan renderer).
describe("adapter xterm di jsdom", () => {
  afterEach(() => {
    document.body.innerHTML = "";
    useApp.setState({ settings: null });
  });

  it("terpasang dengan renderer cadangan, memakai ukuran bawaan tanpa tata letak, dan memproses tulisan", async () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    const error = vi.spyOn(console, "error").mockImplementation(() => {});
    useApp.setState({ settings: { editor: { fontFamily: "Fira Code", fontSize: 14 }, terminal: { shell: null, fontSize: 11 } } as unknown as Settings });
    const stage = document.createElement("div");
    document.body.appendChild(stage);
    const input: string[] = [];
    const adapter = createXtermAdapter({ onInput: (data) => input.push(data), onResize: () => {}, onFind: () => {} });

    const size = await adapter.mount(stage);
    expect(size).toEqual({ cols: 80, rows: 24 });
    const host = stage.querySelector(".tm-host") as HTMLElement;
    expect(host).not.toBeNull();
    expect(host.querySelector(".xterm")).not.toBeNull();

    await new Promise<void>((resolve) => adapter.write("hello\r\n", resolve));
    adapter.setVisible(false);
    expect(host.hidden).toBe(true);
    adapter.setVisible(true);
    expect(host.hidden).toBe(false);

    // Pengaturan berubah: tidak boleh melempar.
    useApp.setState({ settings: { editor: { fontFamily: "Fira Code", fontSize: 16 }, terminal: { shell: null, fontSize: null } } as unknown as Settings });
    window.dispatchEvent(new CustomEvent("oncode:theme", { detail: "light" }));

    // WebGL tidak ada di jsdom: adapter harus jatuh ke renderer bawaan, bukan gagal.
    expect(warn.mock.calls.some((call) => String(call[0]).includes("WebGL"))).toBe(true);

    adapter.clear();
    adapter.dispose();
    expect(stage.querySelector(".tm-host")).toBeNull();
    // Aman dipanggil setelah dibuang.
    adapter.write("late");
    adapter.dispose();
    warn.mockRestore();
    error.mockRestore();
  });

  it("Ctrl+C sampai ke shell sebagai ETX; Ctrl+Shift+C/V ditangani terminal lewat papan klip", async () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    const writeText = vi.fn(() => Promise.resolve());
    const readText = vi.fn(() => Promise.resolve("pasted text"));
    Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText, readText } });
    const stage = document.createElement("div");
    document.body.appendChild(stage);
    const input: string[] = [];
    let findRequests = 0;
    const adapter = createXtermAdapter({ onInput: (data) => input.push(data), onResize: () => {}, onFind: () => findRequests++ });
    await adapter.mount(stage);
    const textarea = stage.querySelector("textarea")!;
    const press = (init: KeyboardEventInit) => {
      const event = new KeyboardEvent("keydown", { bubbles: true, cancelable: true, ...init });
      textarea.dispatchEvent(event);
      return event;
    };

    press({ key: "c", code: "KeyC", keyCode: 67, ctrlKey: true });
    expect(input).toEqual(["\x03"]);

    // Tanpa seleksi tidak ada yang disalin, dan tombolnya tidak diteruskan ke shell.
    const copy = press({ key: "C", code: "KeyC", keyCode: 67, ctrlKey: true, shiftKey: true });
    expect(copy.defaultPrevented).toBe(true);
    expect(writeText).not.toHaveBeenCalled();
    expect(input).toEqual(["\x03"]);

    const paste = press({ key: "V", code: "KeyV", keyCode: 86, ctrlKey: true, shiftKey: true });
    expect(paste.defaultPrevented).toBe(true);
    await vi.waitFor(() => expect(input).toEqual(["\x03", "pasted text"]));
    expect(readText).toHaveBeenCalledTimes(1);

    press({ key: "f", code: "KeyF", keyCode: 70, ctrlKey: true });
    expect(findRequests).toBe(1);
    expect(input).toHaveLength(2);

    // Tombol readline lain tetap diteruskan.
    press({ key: "w", code: "KeyW", keyCode: 87, ctrlKey: true });
    expect(input.at(-1)).toBe("\x17");

    adapter.dispose();
    warn.mockRestore();
  });
});
