import { describe, expect, it } from "vitest";
import type { GraphRow } from "@/ipc";
import { DOT_RADIUS, edgePath, graphWidth, LANE_WIDTH, laneColor, laneX, MAX_LANES, maxLane, ROW_HEIGHT } from "./geometry";
import { linearRows } from "./testing";

describe("laneX", () => {
  it("places a lane at the centre of its 14px column", () => {
    expect(LANE_WIDTH).toBe(14);
    expect(laneX(0)).toBe(7);
    expect(laneX(1)).toBe(21);
    expect(laneX(3)).toBe(49);
  });

  it("leaves room for the dot inside the lane and the row", () => {
    expect(DOT_RADIUS * 2).toBeLessThan(LANE_WIDTH);
    expect(DOT_RADIUS * 2).toBeLessThan(ROW_HEIGHT);
  });
});

describe("laneColor", () => {
  it("maps colours 0–7 to theme variables and wraps anything else", () => {
    expect(laneColor(0)).toBe("var(--lane-0)");
    expect(laneColor(7)).toBe("var(--lane-7)");
    expect(laneColor(8)).toBe("var(--lane-0)");
    expect(laneColor(-1)).toBe("var(--lane-7)");
  });
});

describe("edgePath", () => {
  const h = ROW_HEIGHT; // 26 → tengah baris di y = 13

  it("draws a through edge straight from the top to the bottom of its lane", () => {
    expect(edgePath({ from: 2, to: 2, kind: "through" })).toBe(`M 35 0 V ${h}`);
  });

  it("draws a top edge in the commit's own lane from the top edge to the dot", () => {
    expect(edgePath({ from: 0, to: 0, kind: "top" })).toBe("M 7 0 V 13");
  });

  it("draws a bottom edge in the commit's own lane from the dot to the bottom edge", () => {
    expect(edgePath({ from: 1, to: 1, kind: "bottom" })).toBe(`M 21 13 V ${h}`);
  });

  it("starts a top edge at (from, top) and ends it at the dot (to, middle)", () => {
    // Jalur 2 menyatu ke titik di jalur 0: turun, belok ke kiri, mendatar.
    const d = edgePath({ from: 2, to: 0, kind: "top" });
    expect(d).toBe("M 35 0 V 6 Q 35 13 28 13 H 7");
    expect(d.startsWith(`M ${laneX(2)} 0`)).toBe(true);
    expect(d.endsWith(`H ${laneX(0)}`)).toBe(true);
  });

  it("bends a top edge the other way when the dot is to the right", () => {
    expect(edgePath({ from: 0, to: 1, kind: "top" })).toBe("M 7 0 V 6 Q 7 13 14 13 H 21");
  });

  it("starts a bottom edge at the dot (from, middle) and ends it at (to, bottom)", () => {
    // Titik di jalur 0 bercabang ke jalur 2: mendatar, belok, turun.
    const d = edgePath({ from: 0, to: 2, kind: "bottom" });
    expect(d).toBe(`M 7 13 H 28 Q 35 13 35 20 V ${h}`);
    expect(d.startsWith(`M ${laneX(0)} 13`)).toBe(true);
    expect(d.endsWith(`V ${h}`)).toBe(true);
  });

  it("bends a bottom edge the other way when the target lane is to the left", () => {
    expect(edgePath({ from: 3, to: 1, kind: "bottom" })).toBe(`M 49 13 H 28 Q 21 13 21 20 V ${h}`);
  });

  it("scales with a custom row height", () => {
    expect(edgePath({ from: 0, to: 0, kind: "through" }, 40)).toBe("M 7 0 V 40");
    expect(edgePath({ from: 0, to: 0, kind: "top" }, 40)).toBe("M 7 0 V 20");
    expect(edgePath({ from: 1, to: 0, kind: "top" }, 40)).toBe("M 21 0 V 13 Q 21 20 14 20 H 7");
  });
});

describe("maxLane and graphWidth", () => {
  it("returns -1 for no rows and still reserves one lane of width", () => {
    expect(maxLane([])).toBe(-1);
    expect(graphWidth(-1)).toBe(LANE_WIDTH);
  });

  it("considers the dot column and both ends of every edge", () => {
    const rows: GraphRow[] = [
      ...linearRows(0, 2),
      { ...linearRows(2, 1)[0], column: 1, edges: [{ from: 1, to: 4, color: 2, kind: "bottom" }] },
      { ...linearRows(3, 1)[0], edges: [{ from: 3, to: 0, color: 1, kind: "top" }] },
    ];
    expect(maxLane(rows)).toBe(4);
    expect(graphWidth(4)).toBe(5 * LANE_WIDTH);
  });

  it("caps the width at MAX_LANES lanes", () => {
    expect(graphWidth(MAX_LANES - 1)).toBe(MAX_LANES * LANE_WIDTH);
    expect(graphWidth(200)).toBe(MAX_LANES * LANE_WIDTH);
  });
});
