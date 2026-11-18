import { describe, expect, it, vi } from "vitest";
import { FlowControl, HIGH_WATERMARK, LOW_WATERMARK } from "./flow";

describe("FlowControl", () => {
  it("tidak menjeda selama antrean di bawah high watermark", () => {
    const setPaused = vi.fn();
    const flow = new FlowControl(setPaused, 100, 20);
    flow.written(60);
    flow.written(40);
    expect(flow.pendingChars).toBe(100);
    expect(setPaused).not.toHaveBeenCalled();
    flow.processed(100);
    expect(setPaused).not.toHaveBeenCalled();
  });

  it("menjeda sekali saat melewati high watermark dan melanjutkan saat turun di bawah low watermark", () => {
    const setPaused = vi.fn();
    const flow = new FlowControl(setPaused, 100, 20);
    flow.written(80);
    flow.written(80);
    expect(setPaused.mock.calls).toEqual([[true]]);
    expect(flow.isPaused).toBe(true);
    // Potongan yang datang selagi dijeda tidak memicu panggilan kedua.
    flow.written(50);
    expect(setPaused).toHaveBeenCalledTimes(1);

    flow.processed(80); // 130 tersisa
    flow.processed(80); // 50 tersisa: masih di atas low watermark
    expect(setPaused).toHaveBeenCalledTimes(1);
    flow.processed(31); // 19 tersisa
    expect(setPaused.mock.calls).toEqual([[true], [false]]);
    expect(flow.isPaused).toBe(false);
  });

  it("tepat di low watermark belum melanjutkan; antrean tidak pernah negatif", () => {
    const setPaused = vi.fn();
    const flow = new FlowControl(setPaused, 100, 20);
    flow.written(101);
    flow.processed(81); // tepat 20
    expect(flow.isPaused).toBe(true);
    flow.processed(1000);
    expect(flow.pendingChars).toBe(0);
    expect(flow.isPaused).toBe(false);
  });

  it("bisa menjeda lagi setelah dilanjutkan", () => {
    const setPaused = vi.fn();
    const flow = new FlowControl(setPaused, 100, 20);
    flow.written(150);
    flow.processed(150);
    flow.written(150);
    expect(setPaused.mock.calls).toEqual([[true], [false], [true]]);
  });

  it("memakai watermark bawaan dan menolak konfigurasi terbalik", () => {
    const setPaused = vi.fn();
    const flow = new FlowControl(setPaused);
    flow.written(HIGH_WATERMARK);
    expect(setPaused).not.toHaveBeenCalled();
    flow.written(1);
    expect(setPaused).toHaveBeenLastCalledWith(true);
    flow.processed(HIGH_WATERMARK + 1 - LOW_WATERMARK + 1);
    expect(setPaused).toHaveBeenLastCalledWith(false);
    expect(() => new FlowControl(setPaused, 10, 20)).toThrow();
  });
});
