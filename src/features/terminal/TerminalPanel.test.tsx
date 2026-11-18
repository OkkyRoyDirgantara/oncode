import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { clearMocks } from "@tauri-apps/api/mocks";
import { OverlayHost, resetOverlayForTest } from "@/app/overlay";
import { useApp } from "@/app/store";
import { mockCommands, type WorkspaceInfo } from "@/ipc";
import { createSession, resetTerminalsForTest, setAdapterFactory, useTerminals } from "./store";
import { TerminalActions, TerminalPanel } from "./TerminalPanel";
import { fakeAdapterFactory, type FakeAdapter } from "./testing";

const workspace = { root: "/proj", name: "proj", trusted: false, repoRoot: null } as unknown as WorkspaceInfo;

describe("TerminalPanel", () => {
  let adapters: FakeAdapter[];
  let spawned: number;
  let killed: number[];

  beforeEach(() => {
    resetTerminalsForTest();
    spawned = 0;
    killed = [];
    mockCommands({
      termSpawn: () => ++spawned,
      termKill: ({ id }) => {
        killed.push(id);
        return null;
      },
    });
    const fake = fakeAdapterFactory();
    adapters = fake.adapters;
    setAdapterFactory(fake.factory);
    useApp.setState({ workspace });
  });

  afterEach(() => {
    resetTerminalsForTest();
    resetOverlayForTest();
    useApp.setState({ workspace: null });
    clearMocks();
  });

  it("tanpa folder menampilkan petunjuk dan tidak memulai shell", () => {
    useApp.setState({ workspace: null });
    render(<TerminalPanel visible />);
    expect(screen.getByText("Open a folder to use the terminal")).toBeInTheDocument();
    expect(spawned).toBe(0);
  });

  it("tidak memulai shell selama panel tersembunyi; terminal pertama dibuat saat terlihat (juga di mode terbatas)", async () => {
    const view = render(<TerminalPanel visible={false} />);
    await act(async () => {});
    expect(spawned).toBe(0);

    view.rerender(<TerminalPanel visible />);
    await waitFor(() => expect(useTerminals.getState().sessions).toHaveLength(1));
    expect(spawned).toBe(1);
    // xterm dipasang ke panggung yang dirender komponen.
    expect(adapters[0].stage).toBe(view.container.querySelector(".tm-stage"));
    expect(adapters[0].visible).toBe(true);

    // Menyembunyikan panel tidak mematikan terminal, dan menampilkannya lagi tidak membuat yang baru.
    view.rerender(<TerminalPanel visible={false} />);
    expect(adapters[0].visible).toBe(false);
    expect(adapters[0].disposed).toBe(false);
    view.rerender(<TerminalPanel visible />);
    await act(async () => {});
    expect(spawned).toBe(1);
    expect(adapters[0].visible).toBe(true);
  });

  it("daftar sesi muncul bila lebih dari satu; klik memilih, tombol kill menutup", async () => {
    const view = render(<TerminalPanel visible />);
    await waitFor(() => expect(useTerminals.getState().sessions).toHaveLength(1));
    expect(view.container.querySelector(".tm-list")).toBeNull();

    await act(async () => void (await createSession()));
    const rows = screen.getAllByRole("option");
    expect(rows.map((r) => r.textContent)).toEqual(["Terminal 1", "Terminal 2"]);
    expect(rows[1]).toHaveAttribute("aria-selected", "true");

    fireEvent.click(rows[0]);
    expect(useTerminals.getState().activeId).toBe(1);
    expect(adapters.map((a) => a.visible)).toEqual([true, false]);

    fireEvent.click(screen.getByLabelText("Kill Terminal 2"));
    expect(killed).toEqual([2]);
    expect(useTerminals.getState().sessions.map((s) => s.id)).toEqual([1]);
    expect(view.container.querySelector(".tm-list")).toBeNull();
  });

  it("klik ganda mengganti nama lewat dialog", async () => {
    render(
      <>
        <TerminalPanel visible />
        <OverlayHost />
      </>,
    );
    await waitFor(() => expect(useTerminals.getState().sessions).toHaveLength(1));
    await act(async () => void (await createSession()));
    fireEvent.doubleClick(screen.getAllByRole("option")[0]);
    const input = await screen.findByDisplayValue("Terminal 1");
    fireEvent.change(input, { target: { value: "server" } });
    fireEvent.click(screen.getByRole("button", { name: "Rename" }));
    await waitFor(() => expect(useTerminals.getState().sessions[0].title).toBe("server"));
  });

  it("setelah terminal terakhir ditutup, tombol New Terminal membuat yang baru", async () => {
    render(
      <>
        <TerminalActions />
        <TerminalPanel visible />
      </>,
    );
    await waitFor(() => expect(useTerminals.getState().sessions).toHaveLength(1));
    fireEvent.click(screen.getByRole("button", { name: "Kill Terminal" }));
    expect(useTerminals.getState().sessions).toEqual([]);
    expect(screen.getByRole("button", { name: "Kill Terminal" })).toBeDisabled();

    fireEvent.click(screen.getByText("New Terminal"));
    await waitFor(() => expect(useTerminals.getState().sessions).toHaveLength(1));
    expect(spawned).toBe(2);
  });

  it("bilah cari terbuka atas permintaan terminal dan Escape menutupnya", async () => {
    render(<TerminalPanel visible />);
    await waitFor(() => expect(useTerminals.getState().sessions).toHaveLength(1));
    adapters[0].write("needle in output");
    act(() => adapters[0].events.onFind());
    const input = screen.getByLabelText("Find in terminal");
    expect(input).toHaveFocus();
    fireEvent.change(input, { target: { value: "haystack" } });
    fireEvent.keyDown(input, { key: "Enter" });
    expect(input.closest(".tm-find")).toHaveAttribute("data-missing", "true");
    fireEvent.change(input, { target: { value: "needle" } });
    fireEvent.keyDown(input, { key: "Enter" });
    expect(input.closest(".tm-find")).not.toHaveAttribute("data-missing");
    fireEvent.keyDown(input, { key: "Escape" });
    expect(screen.queryByLabelText("Find in terminal")).toBeNull();
  });
});
