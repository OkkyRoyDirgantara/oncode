import { describe, expect, it } from "vitest";
import type { GraphRow, RefLabel, Refs, Stash } from "@/ipc";
import { countRemoteBranches, groupRefs, isAncestorOfHead, remoteShortName, summarizeLabels } from "./refs";
import { branch, commit, oid } from "./testing";

const refs: Refs = {
  branches: [branch("main", { isHead: true }), branch("feature/login"), branch("fix/typo")],
  remoteBranches: [
    { name: "origin/HEAD", remote: "origin", oid: oid(0) },
    { name: "origin/main", remote: "origin", oid: oid(0) },
    { name: "origin/feature/login", remote: "origin", oid: oid(1) },
    { name: "upstream/main", remote: "upstream", oid: oid(2) },
    { name: "fork/wip", remote: "fork", oid: oid(3) },
  ],
  tags: [
    { name: "v1.0.0", oid: oid(4), annotated: true, message: "First release" },
    { name: "v1.1.0-login", oid: oid(5), annotated: false, message: "" },
  ],
  remotes: [
    { name: "upstream", fetchUrl: "u", pushUrl: "u" },
    { name: "origin", fetchUrl: "o", pushUrl: "o" },
    { name: "empty", fetchUrl: "e", pushUrl: "e" },
  ],
};
const stashes: Stash[] = [
  { index: 0, name: "stash@{0}", oid: oid(6), message: "WIP on main: login form", time: 0 },
  { index: 1, name: "stash@{1}", oid: oid(7), message: "On fix/typo: spelling", time: 0 },
];

describe("groupRefs", () => {
  it("returns everything without a filter, grouping remote branches by remote in configured order", () => {
    const groups = groupRefs(refs, stashes, "");
    expect(groups.branches.map((b) => b.name)).toEqual(["main", "feature/login", "fix/typo"]);
    // Urutan mengikuti `refs.remotes`; remote yang tidak terdaftar (fork) menyusul di belakang.
    expect(groups.remotes.map((g) => g.remote)).toEqual(["upstream", "origin", "empty", "fork"]);
    expect(groups.remotes.find((g) => g.remote === "origin")?.branches.map((b) => b.name)).toEqual(["origin/main", "origin/feature/login"]);
    expect(groups.tags).toHaveLength(2);
    expect(groups.stashes).toHaveLength(2);
    expect(countRemoteBranches(groups.remotes)).toBe(4);
  });

  it("never lists <remote>/HEAD", () => {
    const names = groupRefs(refs, stashes, "").remotes.flatMap((g) => g.branches.map((b) => b.name));
    expect(names).not.toContain("origin/HEAD");
  });

  it("filters every group by case-insensitive substring", () => {
    const groups = groupRefs(refs, stashes, "  LOGIN ");
    expect(groups.branches.map((b) => b.name)).toEqual(["feature/login"]);
    expect(groups.remotes).toEqual([{ remote: "origin", branches: [{ name: "origin/feature/login", remote: "origin", oid: oid(1) }] }]);
    expect(groups.tags.map((t) => t.name)).toEqual(["v1.1.0-login"]);
    // Stash dicocokkan lewat pesannya juga.
    expect(groups.stashes.map((s) => s.name)).toEqual(["stash@{0}"]);
  });

  it("matches a remote branch by its remote name and a stash by its ref name", () => {
    expect(groupRefs(refs, stashes, "upstream").remotes.map((g) => g.remote)).toEqual(["upstream"]);
    expect(groupRefs(refs, stashes, "stash@{1}").stashes.map((s) => s.oid)).toEqual([oid(7)]);
  });

  it("hides remotes without matching branches only while filtering", () => {
    expect(groupRefs(refs, stashes, "").remotes.some((g) => g.remote === "empty")).toBe(true);
    expect(groupRefs(refs, stashes, "main").remotes.map((g) => g.remote)).toEqual(["upstream", "origin"]);
  });

  it("returns empty groups when nothing matches or refs are not loaded yet", () => {
    const none = groupRefs(refs, stashes, "zzz");
    expect([none.branches, none.remotes, none.tags, none.stashes]).toEqual([[], [], [], []]);
    const unloaded = groupRefs(null, [], "");
    expect([unloaded.branches, unloaded.remotes, unloaded.tags, unloaded.stashes]).toEqual([[], [], [], []]);
  });
});

describe("remoteShortName", () => {
  it("strips only the remote prefix", () => {
    expect(remoteShortName({ name: "origin/feature/login", remote: "origin", oid: "" })).toBe("feature/login");
    expect(remoteShortName({ name: "my/remote/main", remote: "my/remote", oid: "" })).toBe("main");
    expect(remoteShortName({ name: "odd", remote: "origin", oid: "" })).toBe("odd");
  });
});

describe("summarizeLabels", () => {
  const labels: RefLabel[] = [
    { kind: "head", name: "HEAD" },
    { kind: "branch", name: "main" },
    { kind: "remote", name: "origin/main" },
    { kind: "tag", name: "v1" },
    { kind: "tag", name: "v2" },
  ];

  it("shows everything when the list fits", () => {
    expect(summarizeLabels(labels.slice(0, 3), 3)).toEqual({ visible: labels.slice(0, 3), hidden: [] });
  });

  it("keeps the first labels and reports the rest as hidden", () => {
    const { visible, hidden } = summarizeLabels(labels, 3);
    expect(visible.map((l) => l.name)).toEqual(["HEAD", "main", "origin/main"]);
    expect(hidden.map((l) => l.name)).toEqual(["v1", "v2"]);
  });
});

describe("isAncestorOfHead", () => {
  // Riwayat (terbaru di atas):  side(1) ─┐
  //                             head(2)  │   head → 3 → 4 ; side → 4
  const row = (n: number, parents: number[]): GraphRow => ({ commit: commit(n, { parents: parents.map(oid) }), column: 0, color: 0, edges: [] });
  const rows = [row(1, [4]), row(2, [3]), row(3, [4]), row(4, [5]), row(5, [])];

  it("is true for HEAD itself and for commits reachable through parents", () => {
    expect(isAncestorOfHead(rows, oid(2), oid(2))).toBe(true);
    expect(isAncestorOfHead(rows, oid(2), oid(3))).toBe(true);
    expect(isAncestorOfHead(rows, oid(2), oid(5))).toBe(true);
  });

  it("follows every parent of a merge commit", () => {
    const merged = [row(0, [2, 1]), ...rows];
    expect(isAncestorOfHead(merged, oid(0), oid(1))).toBe(true);
  });

  it("is false for a commit on another branch that is listed after HEAD", () => {
    const other = [row(2, [3]), row(1, [4]), row(3, [4]), row(4, [])];
    expect(isAncestorOfHead(other, oid(2), oid(1))).toBe(false);
  });

  it("is unknown when HEAD is not listed before the commit, or the commit is not loaded", () => {
    expect(isAncestorOfHead(rows, oid(2), oid(1))).toBeNull();
    expect(isAncestorOfHead(rows, oid(9), oid(3))).toBeNull();
    expect(isAncestorOfHead(rows, oid(2), oid(8))).toBeNull();
    expect(isAncestorOfHead(rows, null, oid(3))).toBeNull();
  });
});
