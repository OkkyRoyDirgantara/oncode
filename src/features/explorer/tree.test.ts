import { describe, expect, it } from "vitest";
import type { Tab } from "@/app/registry";
import type { DirEntry, RepoSnapshot } from "@/ipc";
import { ancestorChain, buildDecorations, decorationFor, flattenTree, isSameOrUnder, joinPath, movedPath, rewriteFileTab, validateEntryName, type DirState } from "./tree";

const dir = (path: string, extra: Partial<DirEntry> = {}): DirEntry => ({ name: path.slice(path.lastIndexOf("/") + 1), path, isDir: true, isSymlink: false, ignored: false, ...extra });
const file = (path: string, extra: Partial<DirEntry> = {}): DirEntry => ({ ...dir(path, extra), isDir: false });
const loaded = (entries: DirEntry[]): DirState => ({ entries, loading: false, error: null });

function snapshot(root: string, patch: Partial<RepoSnapshot>): RepoSnapshot {
  return {
    root,
    head: { kind: "branch", name: "main" },
    headOid: null,
    upstream: null,
    operation: null,
    staged: [],
    unstaged: [],
    untracked: [],
    conflicted: [],
    stashCount: 0,
    ...patch,
  } as RepoSnapshot;
}

describe("path helpers", () => {
  it("joins with the separator the directory uses", () => {
    expect(joinPath("/ws", "a/b.txt")).toBe("/ws/a/b.txt");
    expect(joinPath("/ws/", "a")).toBe("/ws/a");
    expect(joinPath("/", "a")).toBe("/a");
    expect(joinPath("C:\\ws", "a/b.txt")).toBe("C:\\ws\\a\\b.txt");
  });

  it("tells descendants from look-alike siblings", () => {
    expect(isSameOrUnder("/ws/src", "/ws/src")).toBe(true);
    expect(isSameOrUnder("/ws/src/a.ts", "/ws/src")).toBe(true);
    expect(isSameOrUnder("/ws/src2/a.ts", "/ws/src")).toBe(false);
    expect(isSameOrUnder("/ws", "/ws/src")).toBe(false);
    expect(isSameOrUnder("/anything", "/")).toBe(true);
  });

  it("lists the folders between the root and a path", () => {
    expect(ancestorChain("/ws", "/ws/a/b/c.txt")).toEqual(["/ws", "/ws/a", "/ws/a/b"]);
    expect(ancestorChain("/ws", "/ws/c.txt")).toEqual(["/ws"]);
    expect(ancestorChain("/ws", "/ws")).toEqual([]);
    expect(ancestorChain("/ws", "/elsewhere/c.txt")).toEqual([]);
  });
});

describe("flattenTree", () => {
  const dirs: Record<string, DirState> = {
    "/ws": loaded([dir("/ws/docs"), dir("/ws/src"), file("/ws/README.md")]),
    "/ws/src": loaded([dir("/ws/src/app"), file("/ws/src/main.ts")]),
    "/ws/src/app": loaded([file("/ws/src/app/store.ts")]),
    "/ws/docs": loaded([file("/ws/docs/hidden-until-expanded.md")]),
  };
  const shape = (rows: ReturnType<typeof flattenTree>) => rows.map((r) => `${"  ".repeat(r.depth)}${r.kind === "entry" ? r.entry.name + (r.entry.isDir ? (r.expanded ? "/-" : "/+") : "") : `(${r.text})`}`);

  it("shows only the root listing when nothing is expanded", () => {
    expect(shape(flattenTree("/ws", dirs, {}))).toEqual(["docs/+", "src/+", "README.md"]);
  });

  it("nests the children of expanded folders in place, with depth", () => {
    const rows = flattenTree("/ws", dirs, { "/ws/src": true, "/ws/src/app": true });
    expect(shape(rows)).toEqual(["docs/+", "src/-", "  app/-", "    store.ts", "  main.ts", "README.md"]);
    const store = rows.find((r) => r.kind === "entry" && r.entry.name === "store.ts");
    expect(store).toMatchObject({ depth: 2, parent: "/ws/src/app" });
  });

  it("ignores the expanded flag of a folder whose ancestor is collapsed", () => {
    expect(shape(flattenTree("/ws", dirs, { "/ws/src/app": true }))).toEqual(["docs/+", "src/+", "README.md"]);
  });

  it("puts a note under folders that are loading, empty, or unreadable", () => {
    const states: Record<string, DirState> = {
      "/ws": loaded([dir("/ws/empty"), dir("/ws/locked"), dir("/ws/pending"), dir("/ws/unknown")]),
      "/ws/empty": loaded([]),
      "/ws/locked": { entries: null, loading: false, error: "Permission denied" },
      "/ws/pending": { entries: null, loading: true, error: null },
    };
    const rows = flattenTree("/ws", states, { "/ws/empty": true, "/ws/locked": true, "/ws/pending": true, "/ws/unknown": true });
    expect(shape(rows)).toEqual(["empty/-", "  (Empty)", "locked/-", "  (Permission denied)", "pending/-", "  (Loading…)", "unknown/-", "  (Loading…)"]);
    expect(rows.filter((r) => r.kind === "note").map((r) => r.tone)).toEqual(["info", "error", "info", "info"]);
  });

  it("keeps showing the previous listing while a folder reloads", () => {
    const states = { "/ws": { entries: [file("/ws/a.txt")], loading: true, error: null } };
    expect(shape(flattenTree("/ws", states, {}))).toEqual(["a.txt"]);
  });
});

describe("validateEntryName", () => {
  const siblings = [dir("/ws/src"), file("/ws/README.md")];

  it("accepts ordinary names", () => {
    expect(validateEntryName("notes.txt", siblings)).toBeNull();
    expect(validateEntryName(".gitignore", siblings)).toBeNull();
    expect(validateEntryName("with space.txt", siblings)).toBeNull();
    expect(validateEntryName("anything", null)).toBeNull();
  });

  it("rejects empty names and stray whitespace", () => {
    expect(validateEntryName("", siblings)).toMatch(/enter a name/i);
    expect(validateEntryName("   ", siblings)).toMatch(/enter a name/i);
    expect(validateEntryName(" a.txt", siblings)).toMatch(/space/);
    expect(validateEntryName("a.txt ", siblings)).toMatch(/space/);
  });

  it("rejects separators unless nested creation is allowed", () => {
    expect(validateEntryName("a/b.txt", siblings)).toMatch(/cannot contain/);
    expect(validateEntryName("a/b.txt", siblings, { nested: true })).toBeNull();
    expect(validateEntryName("a\\b.txt", siblings, { nested: true })).toMatch(/backslash/i);
  });

  it("rejects traversal and empty segments even when nesting is allowed", () => {
    for (const name of ["..", ".", "../x", "a/../b", "a/./b"]) expect(validateEntryName(name, siblings, { nested: true })).toMatch(/cannot be used/);
    expect(validateEntryName("/abs.txt", siblings, { nested: true })).toMatch(/relative/);
    expect(validateEntryName("a//b", siblings, { nested: true })).toMatch(/empty/);
    expect(validateEntryName("a/", siblings, { nested: true })).toMatch(/empty/);
    // Dua titik di dalam nama biasa bukan traversal.
    expect(validateEntryName("a..b", siblings)).toBeNull();
  });

  it("rejects a name that a loaded sibling already has", () => {
    expect(validateEntryName("README.md", siblings)).toMatch(/already exists/);
    expect(validateEntryName("src", siblings)).toMatch(/already exists/);
    // Sistem berkas Linux membedakan huruf besar-kecil; keputusan akhir ada di backend.
    expect(validateEntryName("readme.md", siblings)).toBeNull();
  });

  it("lets a nested path go through an existing folder but not through a file", () => {
    expect(validateEntryName("src/new.ts", siblings, { nested: true })).toBeNull();
    expect(validateEntryName("README.md/new.ts", siblings, { nested: true })).toMatch(/is a file/);
  });

  it("does not treat the current name as a clash when renaming", () => {
    expect(validateEntryName("README.md", siblings, { current: "README.md" })).toBeNull();
    expect(validateEntryName("src", siblings, { current: "README.md" })).toMatch(/already exists/);
  });
});

describe("git decorations", () => {
  it("maps each change list to a letter and colour", () => {
    const d = buildDecorations(
      snapshot("/ws", {
        staged: [
          { path: "new.ts", origPath: null, kind: "added", submodule: null },
          { path: "moved.ts", origPath: "old.ts", kind: "renamed", submodule: null },
        ],
        unstaged: [
          { path: "edited.ts", origPath: null, kind: "modified", submodule: null },
          { path: "gone.ts", origPath: null, kind: "deleted", submodule: null },
        ],
        untracked: ["scratch.txt"],
        conflicted: [{ path: "both.ts", kind: "bothModified" }],
      }),
    );
    const letter = (path: string) => decorationFor(d, { path, isDir: false });
    expect(letter("/ws/new.ts")).toMatchObject({ letter: "A", className: "oc-git-added" });
    expect(letter("/ws/moved.ts")).toMatchObject({ letter: "R", className: "oc-git-renamed" });
    expect(letter("/ws/edited.ts")).toMatchObject({ letter: "M", className: "oc-git-modified" });
    expect(letter("/ws/gone.ts")).toMatchObject({ letter: "D", className: "oc-git-deleted" });
    expect(letter("/ws/scratch.txt")).toMatchObject({ letter: "U", className: "oc-git-untracked" });
    expect(letter("/ws/both.ts")).toMatchObject({ letter: "C", className: "oc-git-conflict" });
    expect(letter("/ws/clean.ts")).toBeNull();
    expect(letter("/ws/old.ts")).toBeNull();
  });

  it("resolves snapshot paths against the repository root, not the workspace folder", () => {
    // Folder proyek /repo/packages/app berada di dalam repositori /repo.
    const d = buildDecorations(
      snapshot("/repo", {
        unstaged: [{ path: "packages/app/src/main.ts", origPath: null, kind: "modified", submodule: null }],
        untracked: ["packages/other/x.ts"],
      }),
    );
    expect(decorationFor(d, { path: "/repo/packages/app/src/main.ts", isDir: false })).toMatchObject({ letter: "M" });
    expect(decorationFor(d, { path: "/repo/packages/app/src", isDir: true })).toMatchObject({ letter: "", className: "oc-git-modified" });
    expect(decorationFor(d, { path: "/repo/packages/app", isDir: true })).toMatchObject({ className: "oc-git-modified" });
    // Path yang sama relatif terhadap folder proyek tidak boleh ikut cocok.
    expect(decorationFor(d, { path: "/repo/packages/app/packages/app/src/main.ts", isDir: false })).toBeNull();
    expect(decorationFor(d, { path: "/repo/src/main.ts", isDir: false })).toBeNull();
  });

  it("colours every ancestor folder, never the repository root or unrelated folders", () => {
    const d = buildDecorations(snapshot("/ws", { untracked: ["a/b/c/new.txt"] }));
    expect([...d.folders.keys()].sort()).toEqual(["/ws/a", "/ws/a/b", "/ws/a/b/c"]);
    expect(decorationFor(d, { path: "/ws/a/b", isDir: true })).toMatchObject({ status: "untracked", letter: "" });
    expect(decorationFor(d, { path: "/ws/a/other", isDir: true })).toBeNull();
    expect(decorationFor(d, { path: "/ws", isDir: true })).toBeNull();
  });

  it("lets the most significant change win on a folder: conflict > modified > added > untracked", () => {
    const d = buildDecorations(
      snapshot("/ws", {
        staged: [{ path: "pkg/added/new.ts", origPath: null, kind: "added", submodule: null }],
        unstaged: [{ path: "pkg/changed/gone.ts", origPath: null, kind: "deleted", submodule: null }],
        untracked: ["pkg/added/scratch.txt", "pkg/fresh/a.txt"],
        conflicted: [{ path: "top/x.ts", kind: "bothModified" }, { path: "top/sub/y.ts", kind: "bothAdded" }],
      }),
    );
    const folder = (path: string) => decorationFor(d, { path, isDir: true })?.status;
    expect(folder("/ws/pkg/fresh")).toBe("untracked");
    expect(folder("/ws/pkg/added")).toBe("added");
    expect(folder("/ws/pkg/changed")).toBe("modified");
    expect(folder("/ws/pkg")).toBe("modified");
    expect(folder("/ws/top")).toBe("conflict");
  });

  it("shows one status for a file that is both staged and changed again", () => {
    const d = buildDecorations(
      snapshot("/ws", {
        staged: [
          { path: "new.ts", origPath: null, kind: "added", submodule: null },
          { path: "kept.ts", origPath: null, kind: "modified", submodule: null },
        ],
        unstaged: [
          { path: "new.ts", origPath: null, kind: "modified", submodule: null },
          { path: "kept.ts", origPath: null, kind: "deleted", submodule: null },
        ],
      }),
    );
    expect(decorationFor(d, { path: "/ws/new.ts", isDir: false })?.letter).toBe("A");
    expect(decorationFor(d, { path: "/ws/kept.ts", isDir: false })?.letter).toBe("D");
  });

  it("has nothing to show without a repository", () => {
    const d = buildDecorations(null);
    expect(d.files.size + d.folders.size).toBe(0);
  });
});

describe("rename rewriting", () => {
  const tab = (path: string, extra: Partial<Tab> = {}): Tab => ({ id: `file:${path}`, kind: "file", title: path.slice(path.lastIndexOf("/") + 1), description: path, params: { path }, ...extra });

  it("moves a path only when it is the renamed entry or inside it", () => {
    expect(movedPath("/ws/a.ts", "/ws/a.ts", "/ws/b.ts")).toBe("/ws/b.ts");
    expect(movedPath("/ws/src/deep/a.ts", "/ws/src", "/ws/lib")).toBe("/ws/lib/deep/a.ts");
    expect(movedPath("/ws/src2/a.ts", "/ws/src", "/ws/lib")).toBeNull();
    expect(movedPath("/ws/a.ts.bak", "/ws/a.ts", "/ws/b.ts")).toBeNull();
  });

  it("rewrites id, title, description and path of a renamed file tab", () => {
    expect(rewriteFileTab(tab("/ws/a.ts", { preview: true }), "/ws/a.ts", "/ws/b.ts")).toEqual({
      id: "file:/ws/b.ts",
      kind: "file",
      title: "b.ts",
      description: "/ws/b.ts",
      params: { path: "/ws/b.ts" },
      preview: true,
    });
  });

  it("rewrites tabs of files under a renamed folder and keeps their file names", () => {
    expect(rewriteFileTab(tab("/ws/src/deep/a.ts"), "/ws/src", "/ws/lib")).toMatchObject({ id: "file:/ws/lib/deep/a.ts", title: "a.ts", description: "/ws/lib/deep/a.ts", params: { path: "/ws/lib/deep/a.ts" } });
  });

  it("leaves unrelated tabs and other tab kinds alone", () => {
    expect(rewriteFileTab(tab("/ws/other.ts"), "/ws/src", "/ws/lib")).toBeNull();
    expect(rewriteFileTab({ id: "diff:x", kind: "diff", title: "a.ts", params: { path: "/ws/src/a.ts" } }, "/ws/src", "/ws/lib")).toBeNull();
  });
});
