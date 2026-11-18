import { describe, expect, it } from "vitest";
import type { RebaseAction, RebaseTodoItem } from "@/ipc";
import { moveItem, REBASE_ACTIONS, setAction, validatePlan } from "./rebasePlan";
import { oid } from "./testing";

const plan = (...actions: RebaseAction[]): RebaseTodoItem[] => actions.map((action, i) => ({ action, oid: oid(i + 1), subject: `c${i + 1}` }));

describe("validatePlan", () => {
  // Tabel ini mencerminkan `build_todo` di crates/oncode-git/src/ops.rs:
  // squash/fixup butuh pick/reword/edit sebelumnya; minimal satu commit tersisa.
  it.each<[RebaseAction[]]>([
    [["pick"]],
    [["pick", "pick", "pick"]],
    [["reword"]],
    [["edit"]],
    [["pick", "squash"]],
    [["pick", "fixup", "fixup"]],
    [["reword", "squash"]],
    [["edit", "fixup"]],
    [["drop", "pick"]],
    [["drop", "pick", "squash"]],
    // Commit yang di-drop di antaranya tidak menghapus dasar yang sudah ada.
    [["pick", "drop", "squash"]],
    [["pick", "drop", "drop"]],
    [["pick", "squash", "drop", "fixup", "reword"]],
  ])("accepts %j", (actions) => {
    expect(validatePlan(plan(...actions))).toBeNull();
  });

  it.each<[RebaseAction[], RegExp]>([
    [[], /no commits/i],
    [["squash"], /squash.*needs an earlier commit/i],
    [["fixup"], /fixup.*needs an earlier commit/i],
    [["squash", "pick"], /squash.*needs an earlier commit/i],
    [["fixup", "pick"], /fixup.*needs an earlier commit/i],
    // Drop tidak dihitung sebagai commit sebelumnya.
    [["drop", "squash"], /squash.*needs an earlier commit/i],
    [["drop", "drop", "fixup", "pick"], /fixup.*needs an earlier commit/i],
    [["drop"], /every commit is dropped/i],
    [["drop", "drop", "drop"], /every commit is dropped/i],
  ])("rejects %j", (actions, message) => {
    expect(validatePlan(plan(...actions))).toMatch(message);
  });

  it("names the offending commit in the message", () => {
    expect(validatePlan(plan("drop", "squash"))).toContain(oid(2).slice(0, 8));
  });

  it("offers exactly the six git actions", () => {
    expect([...REBASE_ACTIONS]).toEqual(["pick", "reword", "edit", "squash", "fixup", "drop"]);
  });
});

describe("moveItem", () => {
  const items = ["a", "b", "c", "d"];

  it.each([
    [0, 1, "bacd"],
    [0, 3, "bcda"],
    [3, 0, "dabc"],
    [2, 1, "acbd"],
    [1, 1, "abcd"],
  ])("moves index %d to %d", (from, to, expected) => {
    expect(moveItem(items, from, to).join("")).toBe(expected);
  });

  it("ignores indexes outside the list", () => {
    expect(moveItem(items, -1, 2)).toEqual(items);
    expect(moveItem(items, 0, 4)).toEqual(items);
    expect(moveItem(items, 9, 0)).toEqual(items);
  });

  it("returns a new array and leaves the input alone", () => {
    const moved = moveItem(items, 0, 1);
    expect(moved).not.toBe(items);
    expect(items).toEqual(["a", "b", "c", "d"]);
  });
});

describe("setAction", () => {
  it("changes one item and keeps the others and the order", () => {
    const before = plan("pick", "pick", "pick");
    const after = setAction(before, 1, "squash");
    expect(after.map((i) => i.action)).toEqual(["pick", "squash", "pick"]);
    expect(after.map((i) => i.oid)).toEqual(before.map((i) => i.oid));
    expect(before[1].action).toBe("pick");
  });
});
