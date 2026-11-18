import { describe, expect, it } from "vitest";
import { joinPath, repoNameFromUrl, validateCloneUrl, validateFolderName } from "./clone";

describe("repoNameFromUrl", () => {
  it.each([
    ["https://github.com/owner/repo.git", "repo"],
    ["https://github.com/owner/repo", "repo"],
    ["https://github.com/owner/repo/", "repo"],
    ["https://github.com/owner/repo.git/", "repo"],
    ["https://github.com/owner/repo.GIT", "repo"],
    ["  https://github.com/owner/repo.git  ", "repo"],
    ["https://gitlab.com/group/sub/group/project.git", "project"],
    ["https://user@host.example:8443/scm/my.repo.git", "my.repo"],
    ["https://github.com/owner/repo.git?ref=main#readme", "repo"],
    // Bentuk scp.
    ["git@github.com:owner/repo.git", "repo"],
    ["git@github.com:owner/repo", "repo"],
    ["git@github.com:repo.git", "repo"],
    ["git@gitlab.com:group/sub/project.git/", "project"],
    ["ssh://git@github.com/owner/repo.git", "repo"],
    ["ssh://git@host.example:2222/owner/repo.git", "repo"],
    ["git://host.example/pub/thing.git", "thing"],
    ["file:///home/me/src/local.git", "local"],
    ["/home/me/src/local-repo/", "local-repo"],
    // Seperti git: menunjuk ke direktori `.git` berarti repositori induknya.
    ["/home/me/src/local-repo/.git", "local-repo"],
    ["C:\\repos\\win-repo", "win-repo"],
  ])("%s → %s", (url, expected) => {
    expect(repoNameFromUrl(url)).toBe(expected);
  });

  it.each(["", "   ", "https://github.com", "https://github.com/", "ssh://git@host.example:2222", "https://host/.."])("tidak menebak nama dari %j", (url) => {
    expect(repoNameFromUrl(url)).toBe("");
  });
});

describe("validateFolderName", () => {
  it.each(["repo", "my.repo", "repo-2", "Proyek Baru", ".dotfiles", "a"])("menerima %j", (name) => {
    expect(validateFolderName(name)).toBeNull();
  });

  it.each([
    ["", /Enter a folder name/],
    ["   ", /Enter a folder name/],
    ["a/b", /path separator/],
    ["a\\b", /path separator/],
    ["/abs", /path separator/],
    ["..", /reserved/],
    [".", /reserved/],
    [" repo", /space/],
    ["repo ", /space/],
    ["re\tpo", /control/],
    ["re\u0000po", /control/],
    ["re:po", /cannot contain/],
    ['re"po', /cannot contain/],
    ["re*po", /cannot contain/],
    ["-rf", /dash/],
    ["repo.", /dot/],
    ["x".repeat(256), /too long/],
  ])("menolak %j", (name, message) => {
    expect(validateFolderName(name)).toMatch(message);
  });
});

describe("validateCloneUrl", () => {
  it.each([
    "https://github.com/owner/repo.git",
    "http://git.internal/owner/repo",
    "ssh://git@github.com/owner/repo.git",
    "git://host.example/pub/thing.git",
    "file:///home/me/src/local.git",
    "git@github.com:owner/repo.git",
    "github.com:owner/repo.git",
    "/home/me/src/local-repo",
    "C:\\repos\\win-repo",
  ])("menerima %s", (url) => {
    expect(validateCloneUrl(url)).toBeNull();
  });

  it.each([
    ["", /Enter a repository URL/],
    ["https://github.com/owner/my repo", /spaces/],
    ["--upload-pack=touch /tmp/pwned", /spaces/],
    ["--upload-pack=x", /dash/],
    ["-u", /dash/],
    // Transport pembantu menjalankan program; tidak boleh lolos.
    ["ext::sh -c touch% /tmp/pwned", /spaces/],
    ["ext::sh", /not supported/],
    ["fd::17", /not supported/],
    ["ftp://host/repo.git", /https/],
    ["javascript://x/y", /https/],
    ["owner/repo", /full URL/],
    ["repo", /full URL/],
  ])("menolak %j", (url, message) => {
    expect(validateCloneUrl(url)).toMatch(message);
  });
});

describe("joinPath", () => {
  it("memakai pemisah milik folder induk", () => {
    expect(joinPath("/home/me/src", "repo")).toBe("/home/me/src/repo");
    expect(joinPath("/home/me/src/", "repo")).toBe("/home/me/src/repo");
    expect(joinPath("C:\\Users\\me", "repo")).toBe("C:\\Users\\me\\repo");
    expect(joinPath("", "repo")).toBe("repo");
  });
});
