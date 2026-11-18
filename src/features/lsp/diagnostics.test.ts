import { beforeEach, describe, expect, it } from "vitest";
import { clearDiagnostics, convertDiagnostic, publishDiagnostics, toSeverity, useDiagnostics } from "./diagnostics";

const range = (line: number, character: number, endLine = line, endCharacter = character + 1) => ({
  start: { line, character },
  end: { line: endLine, character: endCharacter },
});
const files = () => useDiagnostics.getState().files;

describe("store diagnostics", () => {
  beforeEach(() => clearDiagnostics());

  it("mengubah diagnostics LSP: baris berbasis 1, kolom berbasis 0, severity, sumber, dan kode", () => {
    expect(toSeverity(1)).toBe("error");
    expect(toSeverity(2)).toBe("warning");
    expect(toSeverity(3)).toBe("info");
    expect(toSeverity(4)).toBe("hint");
    expect(toSeverity(undefined)).toBe("error");
    expect(convertDiagnostic({ range: range(4, 8, 5, 2), severity: 2, message: "unused variable", source: "rustc", code: "unused_variables" })).toEqual({
      line: 5,
      column: 8,
      endLine: 6,
      endColumn: 2,
      severity: "warning",
      message: "unused variable",
      source: "rustc",
      code: "unused_variables",
    });
    expect(convertDiagnostic({ range: range(0, 0), message: "x", code: 2304 }).code).toBe("2304");
    expect(convertDiagnostic({ range: range(0, 0), message: "x", code: { value: "E0308" } }).code).toBe("E0308");
    expect(convertDiagnostic({ range: range(0, 0), message: "x" })).not.toHaveProperty("code");
  });

  it("mencatat publishDiagnostics per path dan mengganti daftar lama berkas itu", () => {
    publishDiagnostics("rust-analyzer", { uri: "file:///proj/src/a%20b.rs", diagnostics: [{ range: range(0, 0), severity: 1, message: "first" }] });
    expect(Object.keys(files())).toEqual(["/proj/src/a b.rs"]);
    expect(files()["/proj/src/a b.rs"].map((d) => d.message)).toEqual(["first"]);

    publishDiagnostics("rust-analyzer", {
      uri: "file:///proj/src/a%20b.rs",
      version: 3,
      diagnostics: [
        { range: range(1, 0), severity: 1, message: "second" },
        { range: range(2, 0), severity: 2, message: "third" },
      ],
    });
    expect(files()["/proj/src/a b.rs"].map((d) => d.message)).toEqual(["second", "third"]);
  });

  it("daftar kosong menghapus entri berkas", () => {
    publishDiagnostics("s", { uri: "file:///proj/a.rs", diagnostics: [{ range: range(0, 0), message: "x" }] });
    const before = files();
    publishDiagnostics("s", { uri: "file:///proj/a.rs", diagnostics: [] });
    expect(files()).toEqual({});
    // Berkas tanpa entri: store tidak berubah (tidak memicu render ulang).
    const empty = files();
    publishDiagnostics("s", { uri: "file:///proj/other.rs", diagnostics: [] });
    expect(files()).toBe(empty);
    expect(before).not.toBe(empty);
  });

  it("server berhenti: hanya diagnostics miliknya yang dihapus", () => {
    publishDiagnostics("rust-analyzer", { uri: "file:///proj/a.rs", diagnostics: [{ range: range(0, 0), message: "rust" }] });
    publishDiagnostics("rust-analyzer", { uri: "file:///proj/b.rs", diagnostics: [{ range: range(0, 0), message: "rust" }] });
    publishDiagnostics("gopls", { uri: "file:///proj/main.go", diagnostics: [{ range: range(0, 0), message: "go" }] });
    clearDiagnostics("rust-analyzer");
    expect(Object.keys(files())).toEqual(["/proj/main.go"]);
    clearDiagnostics("rust-analyzer");
    expect(Object.keys(files())).toEqual(["/proj/main.go"]);
    clearDiagnostics();
    expect(files()).toEqual({});
  });

  it("mengabaikan notifikasi yang bentuknya rusak", () => {
    publishDiagnostics("s", { uri: 42, diagnostics: [] } as never);
    publishDiagnostics("s", { uri: "file:///a", diagnostics: null } as never);
    publishDiagnostics("s", { uri: "file:///a", diagnostics: [{ message: "no range" }, null] } as never);
    expect(files()).toEqual({});
  });
});
