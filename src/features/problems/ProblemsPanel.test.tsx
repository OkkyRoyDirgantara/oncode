import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, renderHook, screen, within } from "@testing-library/react";
import { useApp } from "@/app/store";
import { clearDiagnostics, publishDiagnostics, useDiagnostics, type LspDiagnostic } from "@/features/lsp/diagnostics";
import type { WorkspaceInfo } from "@/ipc";
import { ProblemsPanel, ProblemsStatus, VIRTUALIZE_THRESHOLD, useProblemsBadge } from "./ProblemsPanel";

const openFile = vi.hoisted(() => vi.fn());
vi.mock("@/app/coreCommands", () => ({ openFile }));

const diag = (line: number, character: number, severity: number, message: string, extra: Partial<LspDiagnostic> = {}): LspDiagnostic => ({
  range: { start: { line, character }, end: { line, character: character + 2 } },
  severity,
  message,
  ...extra,
});

function seed() {
  publishDiagnostics("rust-analyzer", {
    uri: "file:///proj/src/main.rs",
    diagnostics: [diag(2, 0, 2, "unused variable: `x`", { source: "rustc" }), diag(9, 4, 1, "mismatched types\nexpected u32", { source: "rustc", code: "E0308" })],
  });
  publishDiagnostics("gopls", { uri: "file:///proj/lib/util.go", diagnostics: [diag(0, 0, 3, "exported func should have comment")] });
}

const rowTexts = (container: HTMLElement) => [...container.querySelectorAll(".pb-row")].map((r) => r.textContent);

describe("ProblemsPanel", () => {
  beforeEach(() => {
    clearDiagnostics();
    openFile.mockClear();
    useApp.setState({ workspace: { root: "/proj", name: "proj", trusted: true, repoRoot: null } as unknown as WorkspaceInfo, panelVisible: false, panelView: "terminal" });
  });

  afterEach(() => {
    clearDiagnostics();
    useApp.setState({ workspace: null });
  });

  it("keadaan kosong", () => {
    render(<ProblemsPanel visible />);
    expect(screen.getByText("No problems have been detected in the workspace.")).toBeInTheDocument();
  });

  it("mengelompokkan per berkas dengan path relatif, jumlah, ikon severity, dan posisi", () => {
    seed();
    const { container } = render(<ProblemsPanel visible />);
    expect(rowTexts(container)).toEqual([
      "main.rssrc2",
      "mismatched typesrustc (E0308)[Ln 10, Col 5]",
      "unused variable: `x`rustc[Ln 3, Col 1]",
      "util.golib1",
      "exported func should have comment[Ln 1, Col 1]",
    ]);
    const error = screen.getByText("mismatched types").closest(".pb-row") as HTMLElement;
    expect(within(error).getByLabelText("Error")).toBeInTheDocument();
    expect(within(screen.getByText("unused variable: `x`").closest(".pb-row") as HTMLElement).getByLabelText("Warning")).toBeInTheDocument();
    expect(within(screen.getByText("exported func should have comment").closest(".pb-row") as HTMLElement).getByLabelText("Info")).toBeInTheDocument();
    expect(screen.getByLabelText("Errors: 1")).toBeInTheDocument();
    expect(screen.getByLabelText("Warnings: 1")).toBeInTheDocument();
    expect(screen.getByLabelText("Infos: 1")).toBeInTheDocument();
  });

  it("klik pada masalah membuka berkas di baris dan kolomnya", () => {
    seed();
    render(<ProblemsPanel visible />);
    fireEvent.click(screen.getByText("mismatched types"));
    expect(openFile).toHaveBeenCalledTimes(1);
    expect(openFile).toHaveBeenCalledWith("/proj/src/main.rs", { line: 10, column: 4 });
    fireEvent.keyDown(screen.getByText("exported func should have comment").closest(".pb-row")!, { key: "Enter" });
    expect(openFile).toHaveBeenLastCalledWith("/proj/lib/util.go", { line: 1, column: 0 });
  });

  it("klik pada judul berkas melipat dan membukanya, tanpa membuka berkas", () => {
    seed();
    const { container } = render(<ProblemsPanel visible />);
    const header = screen.getByText("main.rs").closest(".pb-row")!;
    fireEvent.click(header);
    expect(header).toHaveAttribute("aria-expanded", "false");
    expect(rowTexts(container)).toEqual(["main.rssrc2", "util.golib1", "exported func should have comment[Ln 1, Col 1]"]);
    expect(openFile).not.toHaveBeenCalled();
    fireEvent.click(header);
    expect(rowTexts(container)).toHaveLength(5);
  });

  it("kolom penyaring dan tombol severity menyaring daftar", () => {
    seed();
    const { container } = render(<ProblemsPanel visible />);
    fireEvent.change(screen.getByLabelText("Filter problems"), { target: { value: "unused" } });
    expect(rowTexts(container)).toEqual(["main.rssrc1", "unused variable: `x`rustc[Ln 3, Col 1]"]);
    fireEvent.click(screen.getByLabelText("Warnings: 1"));
    expect(screen.getByText("No problems match the current filter.")).toBeInTheDocument();
    expect(screen.getByLabelText("Warnings: 1")).toHaveAttribute("aria-pressed", "false");

    fireEvent.change(screen.getByLabelText("Filter problems"), { target: { value: "" } });
    fireEvent.click(screen.getByLabelText("Errors: 1"));
    expect(rowTexts(container)).toEqual(["util.golib1", "exported func should have comment[Ln 1, Col 1]"]);
  });

  it("mengikuti store: diagnostics baru muncul dan yang dihapus hilang", () => {
    seed();
    const { container } = render(<ProblemsPanel visible />);
    act(() => publishDiagnostics("rust-analyzer", { uri: "file:///proj/src/main.rs", diagnostics: [] }));
    expect(rowTexts(container)).toEqual(["util.golib1", "exported func should have comment[Ln 1, Col 1]"]);
    act(() => clearDiagnostics("gopls"));
    expect(screen.getByText("No problems have been detected in the workspace.")).toBeInTheDocument();
  });

  it("daftar panjang divirtualisasi dan tetap bisa dirender", () => {
    const many = Array.from({ length: VIRTUALIZE_THRESHOLD + 50 }, (_, i) => diag(i, 0, 2, `warning ${i}`));
    publishDiagnostics("s", { uri: "file:///proj/big.rs", diagnostics: many });
    const { container } = render(<ProblemsPanel visible />);
    const list = container.querySelector(".pb-list") as HTMLElement;
    expect(list).toHaveAttribute("data-virtual", "true");
    // Tinggi total mencerminkan semua baris; hanya sebagian (di jsdom: tanpa tata letak) yang dipasang.
    expect((list.firstElementChild as HTMLElement).style.height).toBe(`${(VIRTUALIZE_THRESHOLD + 51) * 24}px`);
    expect(container.querySelectorAll(".pb-row").length).toBeLessThan(VIRTUALIZE_THRESHOLD);
  });

  it("lencana tab = galat + peringatan; item status menampilkan jumlah dan membuka panel", () => {
    const badge = renderHook(() => useProblemsBadge());
    expect(badge.result.current).toBeUndefined();
    render(<ProblemsStatus />);
    expect(screen.getByLabelText("Problems: 0 errors, 0 warnings")).toHaveTextContent("00");

    act(() => seed());
    expect(badge.result.current).toBe(2);
    const status = screen.getByLabelText("Problems: 1 errors, 1 warnings");
    expect(status).toHaveTextContent("11");
    fireEvent.click(status);
    expect(useApp.getState().panelVisible).toBe(true);
    expect(useApp.getState().panelView).toBe("problems");
    expect(Object.keys(useDiagnostics.getState().files)).toHaveLength(2);
  });
});
