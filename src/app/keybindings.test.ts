import { describe, expect, it } from "vitest";
import { parseKeybindings } from "./keybindings";
import { parseSession } from "./session";

describe("keybindings.json", () => {
  it("menerima string, daftar, dan komentar baris", () => {
    const text = '// komentar\n{\n  "file.save": "Mod+S",\n  // lagi\n  "workbench.togglePanel": [],\n  "x": ["Mod+K", "F5"]\n}\n';
    expect(parseKeybindings(text)).toEqual({ overrides: { "file.save": "Mod+S", "workbench.togglePanel": [], x: ["Mod+K", "F5"] }, error: null });
    expect(parseKeybindings("")).toEqual({ overrides: {}, error: null });
  });

  it("menolak bentuk yang salah tanpa menerapkan sebagian", () => {
    expect(parseKeybindings("[]").error).toMatch(/JSON object/);
    expect(parseKeybindings('{ "a": 1 }').error).toMatch(/"a"/);
    expect(parseKeybindings("{ rusak").error).not.toBeNull();
    expect(parseKeybindings('{ "a": "Mod+S", "b": [1] }').overrides).toEqual({});
  });
});

describe("sesi", () => {
  it("mengabaikan data rusak dan menjaga tab aktif tetap bagian dari daftar", () => {
    expect(parseSession(null)).toEqual({ files: [], active: null });
    expect(parseSession("bukan json")).toEqual({ files: [], active: null });
    expect(parseSession(JSON.stringify({ files: ["/a", 3, "/b"], active: "/b" }))).toEqual({ files: ["/a", "/b"], active: "/b" });
    expect(parseSession(JSON.stringify({ files: ["/a"], active: "/tidak-ada" }))).toEqual({ files: ["/a"], active: null });
  });
});
