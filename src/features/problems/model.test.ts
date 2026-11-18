import { describe, expect, it } from "vitest";
import type { Diagnostic, Severity } from "@/features/lsp/diagnostics";
import { DEFAULT_FILTER, buildGroups, countProblems, flattenRows } from "./model";

const d = (severity: Severity, line: number, column: number, message: string, extra: Partial<Diagnostic> = {}): Diagnostic => ({
  line,
  column,
  endLine: line,
  endColumn: column + 1,
  severity,
  message,
  ...extra,
});
const relative = (path: string) => path.replace(/^\/proj\//, "");

const files: Record<string, Diagnostic[]> = {
  "/proj/src/warn_only.rs": [d("warning", 3, 0, "unused import", { source: "rustc", code: "unused_imports" })],
  "/proj/src/main.rs": [
    d("warning", 1, 0, "unused variable"),
    d("error", 20, 4, "mismatched types", { source: "rustc", code: "E0308" }),
    d("hint", 2, 0, "consider borrowing"),
    d("error", 5, 9, "cannot find value"),
    d("error", 5, 2, "expected `;`"),
    d("info", 1, 0, "note"),
  ],
  "/proj/a/info_only.ts": [d("info", 1, 0, "todo comment", { source: "eslint" })],
  "/proj/lib/errors.go": [d("error", 7, 1, "undefined: foo", { source: "compiler" })],
};

describe("countProblems", () => {
  it("menghitung galat, peringatan, dan info (hint dihitung sebagai info)", () => {
    expect(countProblems(files)).toEqual({ errors: 4, warnings: 2, infos: 3 });
    expect(countProblems({})).toEqual({ errors: 0, warnings: 0, infos: 0 });
  });
});

describe("buildGroups", () => {
  it("berkas dengan galat dulu, lalu peringatan, lalu sisanya; di dalamnya menurut path", () => {
    const groups = buildGroups(files, DEFAULT_FILTER, relative);
    expect(groups.map((g) => g.relative)).toEqual(["lib/errors.go", "src/main.rs", "src/warn_only.rs", "a/info_only.ts"]);
    expect(groups[1]).toMatchObject({ path: "/proj/src/main.rs", name: "main.rs", dir: "src", counts: { errors: 3, warnings: 1, infos: 2 } });
  });

  it("di dalam berkas: severity lalu baris lalu kolom", () => {
    const main = buildGroups(files, DEFAULT_FILTER, relative).find((g) => g.name === "main.rs")!;
    expect(main.items.map((i) => `${i.severity}:${i.line}:${i.column}`)).toEqual(["error:5:2", "error:5:9", "error:20:4", "warning:1:0", "info:1:0", "hint:2:0"]);
    // Daftar di store tidak ikut diurutkan.
    expect(files["/proj/src/main.rs"][0].severity).toBe("warning");
  });

  it("tombol severity menyembunyikan jenisnya dan berkas yang jadi kosong", () => {
    const errorsOnly = buildGroups(files, { ...DEFAULT_FILTER, warnings: false, infos: false }, relative);
    expect(errorsOnly.map((g) => g.relative)).toEqual(["lib/errors.go", "src/main.rs"]);
    expect(errorsOnly[1].items).toHaveLength(3);
    expect(errorsOnly[1].counts).toEqual({ errors: 3, warnings: 0, infos: 0 });

    const noErrors = buildGroups(files, { ...DEFAULT_FILTER, errors: false }, relative);
    // Tanpa galat, main.rs diurutkan sebagai berkas peringatan.
    expect(noErrors.map((g) => g.relative)).toEqual(["src/main.rs", "src/warn_only.rs", "a/info_only.ts"]);
    expect(buildGroups(files, { text: "", errors: false, warnings: false, infos: false }, relative)).toEqual([]);
  });

  it("teks penyaring mencocokkan pesan, sumber, kode, atau path, tanpa peka huruf besar/kecil", () => {
    const by = (text: string) => buildGroups(files, { ...DEFAULT_FILTER, text }, relative).map((g) => `${g.relative}=${g.items.length}`);
    expect(by("UNUSED")).toEqual(["src/main.rs=1", "src/warn_only.rs=1"]);
    expect(by("e0308")).toEqual(["src/main.rs=1"]);
    expect(by("eslint")).toEqual(["a/info_only.ts=1"]);
    // Path cocok: semua masalah berkas itu lolos.
    expect(by("main.rs")).toEqual(["src/main.rs=6"]);
    expect(by("  compiler  ")).toEqual(["lib/errors.go=1"]);
    expect(by("nothing matches this")).toEqual([]);
  });

  it("path di luar folder proyek dan path Windows tetap dipecah menjadi nama dan folder", () => {
    const groups = buildGroups(
      { "/elsewhere/x.rs": [d("error", 1, 0, "e")], "C:\\proj\\src\\w.rs": [d("error", 1, 0, "e")], "/proj/top.rs": [d("error", 1, 0, "e")] },
      DEFAULT_FILTER,
      relative,
    );
    expect(groups.map((g) => [g.name, g.dir])).toEqual([
      ["x.rs", "/elsewhere"],
      ["w.rs", "C:/proj/src"],
      ["top.rs", ""],
    ]);
  });
});

describe("flattenRows", () => {
  it("judul berkas diikuti masalahnya; berkas yang dilipat hanya judulnya", () => {
    const groups = buildGroups(files, { ...DEFAULT_FILTER, infos: false }, relative);
    const rows = flattenRows(groups, new Set(["/proj/src/main.rs"]));
    expect(rows.map((r) => (r.kind === "file" ? `# ${r.group.name}` : r.diagnostic.message))).toEqual([
      "# errors.go",
      "undefined: foo",
      "# main.rs",
      "# warn_only.rs",
      "unused import",
    ]);
  });
});
