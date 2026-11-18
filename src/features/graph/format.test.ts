import { describe, expect, it } from "vitest";
import { plural, relativeDate, shortOid } from "./format";

describe("relativeDate", () => {
  const now = Date.UTC(2026, 10, 17, 12, 0, 0);
  const ago = (seconds: number) => relativeDate(now / 1000 - seconds, now);
  const MIN = 60;
  const HOUR = 3600;
  const DAY = 86400;

  it.each([
    [0, "just now"],
    [59, "just now"],
    [MIN, "1m ago"],
    [59 * MIN + 59, "59m ago"],
    [HOUR, "1h ago"],
    [23 * HOUR + 59 * MIN, "23h ago"],
    [DAY, "1d ago"],
    [29 * DAY, "29d ago"],
    [30 * DAY, "1mo ago"],
    [364 * DAY, "12mo ago"],
    [365 * DAY, "1y ago"],
    [800 * DAY, "2y ago"],
  ])("formats %d seconds ago as %s", (seconds, expected) => {
    expect(ago(seconds)).toBe(expected);
  });

  it("treats timestamps in the future (clock skew) as just now", () => {
    expect(ago(-500)).toBe("just now");
  });

  it("returns an empty string for a timestamp that is not a number", () => {
    expect(relativeDate(Number.NaN, now)).toBe("");
  });
});

describe("shortOid and plural", () => {
  it("shortens a hash to eight characters", () => {
    expect(shortOid("0123456789abcdef0123456789abcdef01234567")).toBe("01234567");
  });

  it("adds an s except for exactly one", () => {
    expect(plural(1, "commit")).toBe("1 commit");
    expect(plural(0, "commit")).toBe("0 commits");
    expect(plural(3, "changed file")).toBe("3 changed files");
  });
});
