import { describe, expect, it } from "vitest";
import { getPanelViews, getStatusItems } from "@/app/registry";
import { registerProblemsFeature } from "./index";

describe("fitur Problems", () => {
  it("mendaftarkan panel `problems` (urutan 20, dengan lencana) dan item status kiri (urutan 20)", () => {
    registerProblemsFeature();
    const view = getPanelViews().find((v) => v.id === "problems")!;
    expect(view.title).toBe("Problems");
    expect(view.order).toBe(20);
    expect(view.useBadge).toBeDefined();
    const item = getStatusItems("left").find((i) => i.id === "problems.summary")!;
    expect(item.order).toBe(20);
  });
});
