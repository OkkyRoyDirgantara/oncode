import { describe, expect, it } from "vitest";
import { joinPath, repoRelativePath } from "./paths";
import { defaultWorktreeName, isDirtyWorktreeError } from "./worktree";

describe("repoRelativePath", () => {
  it.each([
    ["/work/repo", "/work/repo/src/main.rs", "src/main.rs"],
    ["/work/repo/", "/work/repo/README.md", "README.md"],
    ["/work/repo", "/work/repo/a/b/c.txt", "a/b/c.txt"],
    ["C:\\work\\repo", "C:\\work\\repo\\src\\app.ts", "src/app.ts"],
    // Windows tidak membedakan huruf besar/kecil; huruf pada hasil mengikuti path berkas.
    ["C:\\Work\\Repo", "c:\\work\\repo\\Src\\App.ts", "Src/App.ts"],
    ["C:/work/repo", "C:\\work\\repo\\x.ts", "x.ts"],
  ])("maps %s + %s to %s", (root, absolute, expected) => {
    expect(repoRelativePath(root, absolute)).toBe(expected);
  });

  it.each([
    ["/work/repo", "/work/other/file.ts"],
    // Awalan nama yang sama bukan berarti berada di dalam repositori.
    ["/work/repo", "/work/repo-backup/file.ts"],
    ["/work/repo", "/work/repo"],
    ["/work/repo", "/work/repo/"],
    ["/work/repo", "relative/file.ts"],
    // Di Unix huruf besar/kecil berbeda berarti folder lain.
    ["/work/repo", "/Work/Repo/file.ts"],
  ])("returns null for %s + %s", (root, absolute) => {
    expect(repoRelativePath(root, absolute)).toBeNull();
  });
});

describe("joinPath", () => {
  it("uses the separator of the parent folder", () => {
    expect(joinPath("/home/ana/code", "repo-feature")).toBe("/home/ana/code/repo-feature");
    expect(joinPath("/home/ana/code/", "repo-feature")).toBe("/home/ana/code/repo-feature");
    expect(joinPath("C:\\code", "repo-feature")).toBe("C:\\code\\repo-feature");
  });
});

describe("defaultWorktreeName", () => {
  it("combines the repository folder and the branch, flattening slashes", () => {
    expect(defaultWorktreeName("/work/oncode", "main")).toBe("oncode-main");
    expect(defaultWorktreeName("/work/oncode/", "feature/login/form")).toBe("oncode-feature-login-form");
    expect(defaultWorktreeName("C:\\work\\oncode", "fix/typo")).toBe("oncode-fix-typo");
  });
});

describe("isDirtyWorktreeError", () => {
  it("recognises the dirtyWorkTree code", () => {
    expect(isDirtyWorktreeError({ code: "dirtyWorkTree", message: "x", detail: null })).toBe(true);
  });

  it("recognises git's message when the code is unknown", () => {
    expect(isDirtyWorktreeError({ code: "unknown", message: "'/w/x' contains modified or untracked files, use --force to delete it", detail: null })).toBe(true);
    expect(isDirtyWorktreeError({ code: "unknown", message: "git failed", detail: "fatal: '/w/x' contains modified or untracked files, use --force to delete it" })).toBe(true);
  });

  it("does not treat other failures as dirty", () => {
    expect(isDirtyWorktreeError({ code: "unknown", message: "'/w/x' is not a working tree", detail: null })).toBe(false);
    expect(isDirtyWorktreeError({ code: "indexLocked", message: "index.lock exists", detail: null })).toBe(false);
    expect(isDirtyWorktreeError(new Error("boom"))).toBe(false);
  });
});
