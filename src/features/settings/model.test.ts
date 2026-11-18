import { describe, expect, it } from "vitest";
import type { Settings } from "@/ipc";
import { cloneSettings, defaults } from "./fixtures";
import { FIELDS, SECTIONS, applyDefault, countNonDefault, defaultLabel, differsFromDefault, fieldById, fieldMatches, isOverridden, isServerOverridden, joinArgs, matchesQuery, parseNumber, sameValue, splitArgs, withServerOverride, type NumberField, type TextField } from "./model";

const clone = cloneSettings;

describe("daftar kolom", () => {
  it("mencakup semua kolom yang diminta, masing-masing dengan penjelasan satu kalimat", () => {
    expect(FIELDS.map((f) => f.id)).toEqual([
      "editor.fontFamily",
      "editor.fontSize",
      "editor.tabSize",
      "editor.insertSpaces",
      "editor.wordWrap",
      "editor.lineNumbers",
      "editor.inlineBlame",
      "files.autoSave",
      "workbench.theme",
      "workbench.startupView",
      "git.path",
      "git.autoFetchMinutes",
      "git.pullStrategy",
      "git.confirmForcePush",
      "terminal.shell",
      "terminal.fontSize",
      "lsp.enabled",
      "forge.githubClientId",
      "forge.gitlabClientId",
    ]);
    for (const field of FIELDS) {
      expect(field.description.length, field.id).toBeGreaterThan(10);
      expect(SECTIONS.some((s) => s.id === field.section), field.id).toBe(true);
    }
    expect(new Set(FIELDS.map((f) => f.id)).size).toBe(FIELDS.length);
  });

  it("id kolom menunjuk ke jalur yang sama di objek Settings, dan set tidak mengubah objek asal", () => {
    for (const field of FIELDS) {
      const [group, key] = field.id.split(".") as [keyof Settings, string];
      const before = clone(defaults);
      const frozen = clone(defaults);
      const current = (before[group] as Record<string, unknown>)[key];
      // `get` membaca jalur yang sama (null ditampilkan sebagai teks kosong pada kolom teks).
      expect(field.get(before) ?? "", field.id).toEqual(current ?? "");
      let after: Settings;
      if (field.kind === "checkbox") after = field.set(before, !field.get(before));
      else if (field.kind === "select") after = field.set(before, field.options.find((o) => o.value !== field.get(before))!.value);
      else if (field.kind === "number") after = field.set(before, field.min + 1);
      else after = field.set(before, "changed-value");
      expect(before, `${field.id} memutasi masukan`).toEqual(frozen);
      // Hanya jalur itu yang berubah.
      const expected = clone(defaults);
      (expected[group] as Record<string, unknown>)[key] = (after[group] as Record<string, unknown>)[key];
      expect(after, field.id).toEqual(expected);
      expect((after[group] as Record<string, unknown>)[key], field.id).not.toEqual(current);
    }
  });

  it("opsi select sesuai nilai enum backend", () => {
    const values = (id: string) => {
      const field = fieldById(id);
      return field.kind === "select" ? field.options.map((o) => `${o.value}=${o.label}`) : [];
    };
    expect(values("files.autoSave")).toEqual(["off=Off", "onFocusChange=On focus change", "afterDelay=After delay"]);
    expect(values("workbench.theme")).toEqual(["system=System", "dark=Dark", "light=Light"]);
    expect(values("workbench.startupView")).toEqual(["scm=Source Control", "explorer=Explorer"]);
    expect(values("git.pullStrategy")).toEqual(["ffOnly=Fast-forward only", "merge=Merge", "rebase=Rebase"]);
  });

  it("kolom teks yang boleh kosong menyimpan null, dan memangkas spasi", () => {
    for (const id of ["git.path", "terminal.shell", "forge.githubClientId", "forge.gitlabClientId"]) {
      const field = fieldById(id) as TextField;
      const [group, key] = id.split(".") as [keyof Settings, string];
      const read = (s: Settings) => (s[group] as Record<string, unknown>)[key];
      expect(read(field.set(defaults, "   ")), id).toBeNull();
      expect(read(field.set(defaults, "")), id).toBeNull();
      expect(read(field.set(defaults, "  value  ")), id).toBe("value");
    }
    expect(fieldById("git.path").description).toMatch(/after restarting/);
  });

  it("validasi teks", () => {
    const check = (id: string, text: string) => (fieldById(id) as TextField).validate?.(text) ?? null;
    expect(check("editor.fontFamily", "Fira Code, monospace")).toBeNull();
    expect(check("editor.fontFamily", "  ")).toMatch(/at least one font/);
    expect(check("editor.fontFamily", "x; } body { display:none")).toMatch(/cannot contain/);
    expect(check("git.path", "/usr/bin/git")).toBeNull();
    expect(check("git.path", "")).toBeNull();
    expect(check("git.path", "/usr/bin/git\n--exec")).toMatch(/control characters/);
    expect(check("forge.githubClientId", "Iv1.0123456789abcdef")).toBeNull();
    expect(check("forge.githubClientId", "")).toBeNull();
    expect(check("forge.githubClientId", "two words")).toMatch(/cannot contain spaces/);
  });
});

describe("parseNumber", () => {
  const fontSize = fieldById("editor.fontSize") as NumberField;
  const tabSize = fieldById("editor.tabSize") as NumberField;
  const autoFetch = fieldById("git.autoFetchMinutes") as NumberField;
  const terminalSize = fieldById("terminal.fontSize") as NumberField;

  it("menerima bilangan bulat dalam rentang", () => {
    expect(parseNumber(fontSize, "8")).toEqual({ ok: true, value: 8 });
    expect(parseNumber(fontSize, " 40 ")).toEqual({ ok: true, value: 40 });
    expect(parseNumber(tabSize, "1")).toEqual({ ok: true, value: 1 });
    expect(parseNumber(tabSize, "16")).toEqual({ ok: true, value: 16 });
    expect(parseNumber(autoFetch, "0")).toEqual({ ok: true, value: 0 });
  });

  it.each([
    [fontSize, "7"],
    [fontSize, "41"],
    [tabSize, "0"],
    [tabSize, "17"],
    [fontSize, "14.5"],
    [fontSize, "-14"],
    [fontSize, "1e1"],
    [fontSize, "abc"],
    [fontSize, ""],
    [autoFetch, "1441"],
    [terminalSize, "7"],
  ])("menolak %#", (field, text) => {
    expect(parseNumber(field, text).ok).toBe(false);
  });

  it("kolom opsional boleh kosong (null)", () => {
    expect(parseNumber(terminalSize, "")).toEqual({ ok: true, value: null });
    expect(parseNumber(terminalSize, "  ")).toEqual({ ok: true, value: null });
    expect(parseNumber(terminalSize, "12")).toEqual({ ok: true, value: 12 });
  });
});

describe("penanda ditimpa proyek", () => {
  const tabSize = fieldById("editor.tabSize");
  const fontSize = fieldById("editor.fontSize");
  const effective: Settings = { ...clone(defaults), editor: { ...defaults.editor, tabSize: 2 }, lsp: { enabled: true, servers: { rust: { command: "ra-custom" } } } };

  it("hanya kolom yang nilainya berbeda dari pengaturan efektif, dan hanya di folder yang dipercaya", () => {
    expect(isOverridden(tabSize, { trusted: true, user: defaults, effective })).toBe(true);
    expect(isOverridden(fontSize, { trusted: true, user: defaults, effective })).toBe(false);
    expect(isOverridden(tabSize, { trusted: false, user: defaults, effective })).toBe(false);
    expect(isOverridden(tabSize, { trusted: true, user: defaults, effective: null })).toBe(false);
    expect(isOverridden(tabSize, { trusted: true, user: { ...defaults, editor: { ...defaults.editor, tabSize: 2 } }, effective })).toBe(false);
    expect(FIELDS.filter((f) => isOverridden(f, { trusted: true, user: defaults, effective })).map((f) => f.id)).toEqual(["editor.tabSize"]);
  });

  it("server per bahasa", () => {
    expect(isServerOverridden("rust", { trusted: true, user: defaults, effective })).toBe(true);
    expect(isServerOverridden("go", { trusted: true, user: defaults, effective })).toBe(false);
    expect(isServerOverridden("rust", { trusted: false, user: defaults, effective })).toBe(false);
    // `args: []` dan args yang tidak ditulis dianggap sama.
    const user = withServerOverride(defaults, "rust", { command: "ra-custom", args: [] });
    expect(isServerOverridden("rust", { trusted: true, user, effective })).toBe(false);
  });
});

describe("pencarian", () => {
  it("setiap kata harus cocok; label, penjelasan, id, bagian, dan opsi ikut dicari", () => {
    expect(matchesQuery("", "x")).toBe(true);
    expect(matchesQuery("font size", "Font size")).toBe(true);
    expect(matchesQuery("font zzz", "Font size")).toBe(false);
    const ids = (q: string) => FIELDS.filter((f) => fieldMatches(f, q)).map((f) => f.id);
    expect(ids("font size")).toEqual(["editor.fontSize", "terminal.fontSize"]);
    expect(ids("TERMINAL")).toEqual(["editor.fontFamily", "terminal.shell", "terminal.fontSize"]);
    expect(ids("rebase")).toEqual(["git.pullStrategy"]);
    expect(ids("git.path")).toEqual(["git.path"]);
    expect(ids("accounts")).toEqual(["forge.githubClientId", "forge.gitlabClientId"]);
    expect(ids("no such thing")).toEqual([]);
  });
});

describe("argumen server", () => {
  it("splitArgs", () => {
    expect(splitArgs("")).toEqual({ ok: true, value: [] });
    expect(splitArgs("  --stdio  ")).toEqual({ ok: true, value: ["--stdio"] });
    expect(splitArgs('--log "C:\\\\Program Files\\\\x" --flag')).toEqual({ ok: true, value: ["--log", "C:\\Program Files\\x", "--flag"] });
    expect(splitArgs("--name 'two words' \"\"")).toEqual({ ok: true, value: ["--name", "two words", ""] });
    expect(splitArgs("C:\\tools\\ls.exe")).toEqual({ ok: true, value: ["C:\\tools\\ls.exe"] });
    expect(splitArgs('--bad "unclosed').ok).toBe(false);
  });

  it("joinArgs adalah kebalikan splitArgs", () => {
    for (const args of [[], ["--stdio"], ["--log", "C:\\Program Files\\x"], ["a b", "", 'say "hi"', "it's"], ["--x=1", "-y"]]) {
      expect(splitArgs(joinArgs(args))).toEqual({ ok: true, value: args });
    }
    expect(joinArgs(["--stdio", "two words"])).toBe('--stdio "two words"');
  });

  it("withServerOverride menetapkan dan menghapus tanpa mengubah masukan", () => {
    const withRust = withServerOverride(defaults, "rust", { command: "ra", args: ["--x"] });
    expect(withRust.lsp.servers).toEqual({ rust: { command: "ra", args: ["--x"] } });
    expect(defaults.lsp.servers).toEqual({});
    expect(withServerOverride(withRust, "rust", null).lsp.servers).toEqual({});
    expect(withRust.lsp.servers.rust).toBeDefined();
  });
});

describe("sameValue", () => {
  it("membandingkan nilai JSON tanpa memedulikan urutan kunci", () => {
    expect(sameValue({ a: 1, b: [1, { c: null }] }, { b: [1, { c: null }], a: 1 })).toBe(true);
    expect(sameValue({ a: 1 }, { a: 1, b: undefined })).toBe(false);
    expect(sameValue([1, 2], [2, 1])).toBe(false);
    expect(sameValue(null, {})).toBe(false);
    expect(sameValue(defaults, clone(defaults))).toBe(true);
  });
});

describe("nilai bawaan", () => {
  const changed: Settings = {
    ...cloneSettings(defaults),
    editor: { ...defaults.editor, fontSize: 20, wordWrap: true },
    git: { ...defaults.git, path: "/opt/git", pullStrategy: "rebase" },
    terminal: { shell: null, fontSize: 11 },
    lsp: { enabled: true, servers: { rust: { command: "ra" }, go: { command: "gopls" } } },
  };

  it("differsFromDefault dan applyDefault bekerja per kolom untuk semua jenis kontrol", () => {
    expect(FIELDS.filter((f) => differsFromDefault(f, changed, defaults)).map((f) => f.id)).toEqual(["editor.fontSize", "editor.wordWrap", "git.path", "git.pullStrategy", "terminal.fontSize"]);
    expect(FIELDS.filter((f) => differsFromDefault(f, defaults, defaults))).toEqual([]);
    let current = changed;
    for (const field of FIELDS) {
      const next = applyDefault(field, current, defaults);
      expect(differsFromDefault(field, next, defaults), field.id).toBe(false);
      current = next;
    }
    // Yang tersisa hanya penimpaan server, yang bukan kolom formulir.
    expect(current).toEqual({ ...cloneSettings(defaults), lsp: changed.lsp });
    expect(changed.editor.fontSize).toBe(20);
  });

  it("applyDefault mengikuti nilai bawaan dari backend, bukan konstanta di frontend", () => {
    const other: Settings = { ...cloneSettings(defaults), editor: { ...defaults.editor, tabSize: 2 }, git: { ...defaults.git, path: "/usr/local/bin/git" }, terminal: { shell: null, fontSize: 13 } };
    expect(applyDefault(fieldById("editor.tabSize"), defaults, other).editor.tabSize).toBe(2);
    expect(applyDefault(fieldById("git.path"), defaults, other).git.path).toBe("/usr/local/bin/git");
    expect(applyDefault(fieldById("terminal.fontSize"), defaults, other).terminal.fontSize).toBe(13);
  });

  it("defaultLabel", () => {
    expect(defaultLabel(fieldById("editor.fontSize"), defaults)).toBe("14");
    expect(defaultLabel(fieldById("terminal.fontSize"), defaults)).toBe("empty");
    expect(defaultLabel(fieldById("git.path"), defaults)).toBe("empty");
    expect(defaultLabel(fieldById("editor.insertSpaces"), defaults)).toBe("on");
    expect(defaultLabel(fieldById("editor.wordWrap"), defaults)).toBe("off");
    expect(defaultLabel(fieldById("git.pullStrategy"), defaults)).toBe("Fast-forward only");
  });

  it("countNonDefault menghitung kolom dan penimpaan language server", () => {
    expect(countNonDefault(defaults, defaults)).toBe(0);
    expect(countNonDefault(changed, defaults)).toBe(7);
  });
});
