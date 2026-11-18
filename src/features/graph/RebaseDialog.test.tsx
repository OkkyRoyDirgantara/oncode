import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { clearMocks } from "@tauri-apps/api/mocks";
import { OverlayHost, resetOverlayForTest } from "@/app/overlay";
import { mockCommands, type RebaseAction, type RebasePlan, type RebaseTodoItem } from "@/ipc";
import { resetScmForTest, useScm } from "@/features/scm/store";
import { interactiveRebase } from "./RebaseDialog";
import { oid, repoState, ROOT } from "./testing";

const BASE = oid(100);
const short = (n: number) => oid(n).slice(0, 8);

function plan(count: number, published = 0): RebasePlan {
  return { items: Array.from({ length: count }, (_, i) => ({ action: "pick" as const, oid: oid(i + 1), subject: `Change ${i + 1}` })), published };
}

function backend(loaded: RebasePlan) {
  const calls = {
    plan: vi.fn((_: { root: string; base: string | null }) => loaded),
    rebase: vi.fn((_: { root: string; base: string | null; items: RebaseTodoItem[] }) => null),
  };
  mockCommands({ gitRebasePlan: calls.plan, gitRebaseInteractive: calls.rebase });
  return calls;
}

/** Membuka dialog seperti command `git.interactiveRebase` dan menunggu rencananya tampil. */
async function open(base: string | null = BASE) {
  render(<OverlayHost />);
  const result = interactiveRebase(ROOT, base);
  const dialog = await screen.findByRole("dialog");
  const rows = () => within(dialog).queryAllByRole("listitem");
  const order = () => rows().map((r) => `${within(r).getByRole<HTMLSelectElement>("combobox").value} ${r.querySelector(".gr-rebase-hash")!.textContent}`);
  const setAction = (n: number, action: RebaseAction) => fireEvent.change(within(dialog).getByLabelText(`Action for ${short(n)}`), { target: { value: action } });
  const start = () => within(dialog).getByRole<HTMLButtonElement>("button", { name: "Start Rebase" });
  return { result, dialog, rows, order, setAction, start };
}

beforeEach(() => {
  resetOverlayForTest();
  resetScmForTest({ [ROOT]: repoState() }, ROOT);
});
afterEach(() => clearMocks());

describe("interactive rebase dialog", () => {
  it("loads the plan for the base and lists commits oldest first, all pick", async () => {
    const api = backend(plan(3));
    const { dialog, order, start } = await open();
    await waitFor(() => expect(order()).toEqual([`pick ${short(1)}`, `pick ${short(2)}`, `pick ${short(3)}`]));
    expect(api.plan).toHaveBeenCalledWith({ root: ROOT, base: BASE });
    expect(dialog).toHaveTextContent(`Interactive rebase: 3 commits on top of ${BASE.slice(0, 8)}`);
    expect(within(dialog).getByText("Change 2")).toBeInTheDocument();
    expect(within(dialog).getByText(/applied top to bottom/)).toBeInTheDocument();
    // Tiap baris menawarkan keenam aksi.
    const options = [...within(dialog).getByLabelText<HTMLSelectElement>(`Action for ${short(1)}`).options].map((o) => o.value);
    expect(options).toEqual(["pick", "reword", "edit", "squash", "fixup", "drop"]);
    expect(start()).toBeEnabled();
    expect(within(dialog).queryByRole("alert")).not.toBeInTheDocument();
    expect(within(dialog).queryByText(/already on a remote/)).not.toBeInTheDocument();
  });

  it("disables Start and explains why when squash has nothing above it", async () => {
    const api = backend(plan(3));
    const { dialog, setAction, start } = await open();
    await within(dialog).findByText("Change 1");

    setAction(1, "squash");
    expect(start()).toBeDisabled();
    expect(within(dialog).getByRole("alert")).toHaveTextContent(`"squash" on ${short(1)} needs an earlier commit`);

    // Tombol yang nonaktif tidak menjalankan apa pun.
    fireEvent.click(start());
    expect(screen.getByRole("dialog")).toBeInTheDocument();
    expect(api.rebase).not.toHaveBeenCalled();

    // Memperbaikinya mengaktifkan kembali tombol Start.
    setAction(1, "pick");
    expect(start()).toBeEnabled();
    expect(within(dialog).queryByRole("alert")).not.toBeInTheDocument();
  });

  it("disables Start for fixup after only dropped commits and when everything is dropped", async () => {
    backend(plan(3));
    const { dialog, setAction, start } = await open();
    await within(dialog).findByText("Change 1");

    setAction(1, "drop");
    setAction(2, "fixup");
    expect(start()).toBeDisabled();
    expect(within(dialog).getByRole("alert")).toHaveTextContent(`"fixup" on ${short(2)} needs an earlier commit`);

    setAction(2, "drop");
    setAction(3, "drop");
    expect(start()).toBeDisabled();
    expect(within(dialog).getByRole("alert")).toHaveTextContent("Every commit is dropped");

    setAction(3, "reword");
    expect(start()).toBeEnabled();
  });

  it("re-validates after reordering: moving a squash to the top makes the plan invalid", async () => {
    backend(plan(2));
    const { dialog, setAction, start, order } = await open();
    await within(dialog).findByText("Change 1");
    setAction(2, "squash");
    expect(start()).toBeEnabled();

    fireEvent.click(within(dialog).getByRole("button", { name: `Move ${short(2)} up` }));
    expect(order()).toEqual([`squash ${short(2)}`, `pick ${short(1)}`]);
    expect(start()).toBeDisabled();
  });

  it("sends the items in the displayed order with the chosen actions, after closing the dialog", async () => {
    const api = backend(plan(4));
    const { dialog, result, setAction, start, order } = await open();
    await within(dialog).findByText("Change 1");

    // Tombol panah: 4 naik dua kali, 1 turun sekali.
    fireEvent.click(within(dialog).getByRole("button", { name: `Move ${short(4)} up` }));
    fireEvent.click(within(dialog).getByRole("button", { name: `Move ${short(4)} up` }));
    fireEvent.click(within(dialog).getByRole("button", { name: `Move ${short(1)} down` }));
    setAction(1, "squash");
    setAction(2, "reword");
    setAction(3, "drop");
    expect(order()).toEqual([`pick ${short(4)}`, `squash ${short(1)}`, `reword ${short(2)}`, `drop ${short(3)}`]);

    // Dialog harus sudah tertutup saat rebase dijalankan (backend bisa meminta pesan commit lewat dialog lain).
    let dialogOpenAtStart: boolean | null = null;
    api.rebase.mockImplementationOnce(() => {
      dialogOpenAtStart = screen.queryByRole("dialog") !== null;
      return null;
    });
    fireEvent.click(start());

    await expect(result).resolves.toBe(true);
    expect(api.rebase).toHaveBeenCalledTimes(1);
    expect(api.rebase).toHaveBeenCalledWith({
      root: ROOT,
      base: BASE,
      items: [
        { action: "pick", oid: oid(4), subject: "Change 4" },
        { action: "squash", oid: oid(1), subject: "Change 1" },
        { action: "reword", oid: oid(2), subject: "Change 2" },
        { action: "drop", oid: oid(3), subject: "Change 3" },
      ],
    });
    expect(dialogOpenAtStart).toBe(false);
    expect(useScm.getState().repos[ROOT].busy).toBeNull();
  });

  it("reorders by dragging the handle onto another row", async () => {
    const api = backend(plan(3));
    const { dialog, result, rows, order, start } = await open();
    await within(dialog).findByText("Change 1");

    // Seret baris pertama ke posisi baris ketiga.
    fireEvent.dragStart(rows()[0].querySelector(".gr-rebase-handle")!);
    fireEvent.dragOver(rows()[2]);
    expect(rows()[2]).toHaveAttribute("data-drop", "after");
    fireEvent.drop(rows()[2]);
    expect(order()).toEqual([`pick ${short(2)}`, `pick ${short(3)}`, `pick ${short(1)}`]);
    expect(rows().some((r) => r.hasAttribute("data-drop") || r.hasAttribute("data-dragging"))).toBe(false);

    // Seret baris terakhir ke atas.
    fireEvent.dragStart(rows()[2].querySelector(".gr-rebase-handle")!);
    fireEvent.dragOver(rows()[0]);
    expect(rows()[0]).toHaveAttribute("data-drop", "before");
    fireEvent.drop(rows()[0]);
    expect(order()).toEqual([`pick ${short(1)}`, `pick ${short(2)}`, `pick ${short(3)}`]);

    fireEvent.dragStart(rows()[1].querySelector(".gr-rebase-handle")!);
    fireEvent.drop(rows()[0]);
    fireEvent.click(start());
    await result;
    expect(api.rebase.mock.calls[0][0].items.map((i) => i.oid)).toEqual([oid(2), oid(1), oid(3)]);
  });

  it("disables the arrows at the ends of the list", async () => {
    backend(plan(2));
    const { dialog } = await open();
    await within(dialog).findByText("Change 1");
    expect(within(dialog).getByRole("button", { name: `Move ${short(1)} up` })).toBeDisabled();
    expect(within(dialog).getByRole("button", { name: `Move ${short(1)} down` })).toBeEnabled();
    expect(within(dialog).getByRole("button", { name: `Move ${short(2)} up` })).toBeEnabled();
    expect(within(dialog).getByRole("button", { name: `Move ${short(2)} down` })).toBeDisabled();
  });

  it("warns that a force push will be needed when commits are already on a remote", async () => {
    backend(plan(5, 2));
    const { dialog } = await open();
    expect(await within(dialog).findByRole("status")).toHaveTextContent("2 of these commits are already on a remote; you will need to force push afterwards.");
  });

  it("passes a null base for a rebase from the root commit", async () => {
    const api = backend(plan(2));
    const { dialog, result, start } = await open(null);
    await within(dialog).findByText("Change 1");
    expect(api.plan).toHaveBeenCalledWith({ root: ROOT, base: null });
    expect(dialog).toHaveTextContent("Interactive rebase: 2 commits from the first commit");
    fireEvent.click(start());
    await result;
    expect(api.rebase).toHaveBeenCalledWith(expect.objectContaining({ root: ROOT, base: null }));
  });

  it("does nothing when cancelled", async () => {
    const api = backend(plan(2));
    const { dialog, result } = await open();
    await within(dialog).findByText("Change 1");
    fireEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));
    await expect(result).resolves.toBe(false);
    expect(api.rebase).not.toHaveBeenCalled();
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("cannot start with an empty plan or when the plan fails to load", async () => {
    backend(plan(0));
    const empty = await open();
    expect(await within(empty.dialog).findByText(/no commits to rebase here/)).toBeInTheDocument();
    expect(empty.start()).toBeDisabled();
    fireEvent.click(within(empty.dialog).getByRole("button", { name: "Cancel" }));
    await empty.result;

    mockCommands({
      gitRebasePlan: () => {
        throw { code: "unknown", message: "bad revision", detail: null };
      },
    });
    const failed = interactiveRebase(ROOT, BASE);
    expect(await screen.findByRole("alert")).toHaveTextContent("Could not prepare the rebase: bad revision");
    expect(screen.getByRole<HTMLButtonElement>("button", { name: "Start Rebase" })).toBeDisabled();
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    await failed;
  });

  it("reports a failed rebase and resolves false", async () => {
    const api = backend(plan(2));
    api.rebase.mockImplementationOnce(() => {
      throw { code: "unknown", message: "could not start", detail: null };
    });
    const { dialog, result, start } = await open();
    await within(dialog).findByText("Change 1");
    fireEvent.click(start());
    await expect(result).resolves.toBe(false);
    expect(await screen.findByText("Interactive rebase failed: could not start")).toBeInTheDocument();
  });
});
