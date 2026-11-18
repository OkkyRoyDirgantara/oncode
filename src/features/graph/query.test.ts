import { describe, expect, it } from "vitest";
import type { Refs } from "@/ipc";
import { buildQuery, nextSelection, orderOldestFirst, scopeExists } from "./query";
import { branch, linearRows, oid } from "./testing";

describe("buildQuery", () => {
  it("maps the scope to --all, HEAD, or one ref", () => {
    expect(buildQuery("all", "", "")).toEqual({ all: true });
    expect(buildQuery("current", "", "")).toEqual({});
    expect(buildQuery("ref:feature/login", "", "")).toEqual({ revs: ["feature/login"] });
  });

  it("adds trimmed message and author filters only when given", () => {
    expect(buildQuery("all", "  fix ", " ana ")).toEqual({ all: true, grep: "fix", author: "ana" });
    expect(buildQuery("current", "   ", "")).toEqual({});
  });
});

describe("scopeExists", () => {
  const refs: Refs = { branches: [branch("main")], remoteBranches: [{ name: "origin/main", remote: "origin", oid: oid(0) }], tags: [{ name: "v1", oid: oid(0), annotated: false, message: "" }], remotes: [] };

  it("accepts the fixed scopes and existing refs of every kind", () => {
    for (const scope of ["all", "current", "ref:main", "ref:origin/main", "ref:v1"]) expect(scopeExists(scope, refs)).toBe(true);
  });

  it("rejects a ref that was deleted, but not before refs are loaded", () => {
    expect(scopeExists("ref:gone", refs)).toBe(false);
    expect(scopeExists("ref:gone", null)).toBe(true);
  });
});

describe("nextSelection", () => {
  const [a, b, c] = [oid(1), oid(2), oid(3)];

  it("selects exactly one commit on a plain click", () => {
    expect(nextSelection([], a, false)).toEqual([a]);
    expect(nextSelection([a, b], c, false)).toEqual([c]);
    expect(nextSelection([a], a, false)).toEqual([a]);
  });

  it("adds a second commit on Ctrl/Cmd-click", () => {
    expect(nextSelection([], a, true)).toEqual([a]);
    expect(nextSelection([a], b, true)).toEqual([a, b]);
  });

  it("toggles a selected commit off on Ctrl/Cmd-click", () => {
    expect(nextSelection([a, b], b, true)).toEqual([a]);
    expect(nextSelection([a, b], a, true)).toEqual([b]);
    expect(nextSelection([a], a, true)).toEqual([]);
  });

  it("never keeps more than two: a third replaces the second", () => {
    expect(nextSelection([a, b], c, true)).toEqual([a, c]);
  });
});

describe("orderOldestFirst", () => {
  const rows = linearRows(0, 10); // baris 0 = terbaru

  it("orders by position in the list: lower rows are older", () => {
    expect(orderOldestFirst(rows, [oid(2), oid(7)])).toEqual([oid(7), oid(2)]);
    expect(orderOldestFirst(rows, [oid(7), oid(2)])).toEqual([oid(7), oid(2)]);
  });

  it("keeps the selection order when a commit is not loaded", () => {
    expect(orderOldestFirst(rows, [oid(2), oid(99)])).toEqual([oid(2), oid(99)]);
  });
});
