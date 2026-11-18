import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { clearMocks } from "@tauri-apps/api/mocks";
import { OverlayHost, resetOverlayForTest } from "@/app/overlay";
import type { Tab } from "@/app/registry";
import { allTabs, resetWorkbenchForTest } from "@/app/workbench";
import { events, mockCommands, type Commit, type CommitFile, type LogQuery, type ReflogEntry, type ResetMode, type Stash } from "@/ipc";
import { installScmStore, resetScmForTest } from "@/features/scm/store";
import { takeReveal } from "./bus";
import { FileHistoryTab, ReflogTab, StashTab } from "./ListTabs";
import { commit, oid, repoState, ROOT, stubViewport } from "./testing";

const tab = (kind: string, params: Record<string, unknown>): Tab => ({ id: `${kind}:test`, kind, title: kind, params });
const show = (node: React.ReactNode) =>
  render(
    <>
      {node}
      <OverlayHost />
    </>,
  );

let restoreViewport: () => void;
beforeEach(() => {
  restoreViewport = stubViewport(520);
  resetWorkbenchForTest();
  resetOverlayForTest();
  resetScmForTest({ [ROOT]: repoState() }, ROOT);
});
afterEach(() => {
  restoreViewport();
  clearMocks();
});

describe("ReflogTab", () => {
  const entries: ReflogEntry[] = [
    { oid: oid(1), selector: "HEAD@{0}", message: "reset: moving to HEAD~3", time: 1_700_000_000 },
    { oid: oid(2), selector: "HEAD@{1}", message: "commit: Add login validation", time: 1_699_999_000 },
    { oid: oid(3), selector: "HEAD@{2}", message: "checkout: moving from main to feature", time: 1_699_990_000 },
  ];

  function backend() {
    const calls = {
      reflog: vi.fn((_: { root: string; limit: number }) => entries),
      reset: vi.fn((_: { root: string; rev: string; mode: ResetMode }) => null),
      branchCreate: vi.fn((_: { root: string; name: string; start: string | null; checkout: boolean }) => null),
    };
    mockCommands({ gitReflog: calls.reflog, gitReset: calls.reset, gitBranchCreate: calls.branchCreate, gitRefs: () => repoState().refs!, gitStashList: () => [], gitWorktrees: () => [], gitSubmodules: () => [] });
    return calls;
  }

  it("lists the HEAD movements with selector, hash and message, and explains what the reflog is for", async () => {
    const api = backend();
    show(<ReflogTab tab={tab("reflog", { root: ROOT })} groupId="g" visible focused />);
    const list = await screen.findByRole("list", { name: "Reflog" });
    expect(api.reflog).toHaveBeenCalledWith({ root: ROOT, limit: 500 });
    const rows = within(list).getAllByRole("listitem");
    expect(rows).toHaveLength(3);
    expect(rows[1]).toHaveTextContent("HEAD@{1}");
    expect(rows[1]).toHaveTextContent(oid(2).slice(0, 8));
    expect(rows[1]).toHaveTextContent("commit: Add login validation");
    expect(screen.getByText(/After a bad reset or rebase, find the entry from before it/)).toBeInTheDocument();
    expect(screen.getByText("3 entries")).toBeInTheDocument();
  });

  it("creates a branch at an entry through the row button", async () => {
    const api = backend();
    show(<ReflogTab tab={tab("reflog", { root: ROOT })} groupId="g" visible focused />);
    await screen.findByRole("list", { name: "Reflog" });

    fireEvent.click(screen.getByRole("button", { name: "Create branch at HEAD@{1}" }));
    const dialog = await screen.findByRole("dialog");
    fireEvent.change(within(dialog).getByLabelText("Branch name"), { target: { value: "rescue" } });
    fireEvent.click(within(dialog).getByRole("button", { name: "OK" }));
    await waitFor(() => expect(api.branchCreate).toHaveBeenCalledWith({ root: ROOT, name: "rescue", start: oid(2), checkout: true }));
  });

  it("resets hard only after the confirmation, and offers a mixed reset in the context menu", async () => {
    const api = backend();
    show(<ReflogTab tab={tab("reflog", { root: ROOT })} groupId="g" visible focused />);
    const list = await screen.findByRole("list", { name: "Reflog" });

    fireEvent.click(screen.getByRole("button", { name: "Reset current branch to HEAD@{1}" }));
    let dialog = await screen.findByRole("dialog");
    expect(dialog).toHaveTextContent(`Reset main to ${oid(2).slice(0, 8)} (hard)?`);
    expect(api.reset).not.toHaveBeenCalled();
    fireEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(api.reset).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole("button", { name: "Reset current branch to HEAD@{1}" }));
    dialog = await screen.findByRole("dialog");
    fireEvent.click(within(dialog).getByRole("button", { name: "Reset and Discard" }));
    await waitFor(() => expect(api.reset).toHaveBeenCalledWith({ root: ROOT, rev: oid(2), mode: "hard" }));

    fireEvent.contextMenu(within(list).getAllByRole("listitem")[2].firstElementChild!);
    const menu = await screen.findByRole("menu");
    expect(within(menu).getAllByRole("menuitem").map((i) => i.textContent)).toEqual([
      "Show in Graph",
      "Create Branch Here…",
      "Checkout (detached)",
      "Reset Current Branch to Here (Mixed)…",
      "Reset Current Branch to Here (Hard)…",
      `Copy Hash (${oid(3).slice(0, 8)})`,
    ]);
    fireEvent.click(within(menu).getByRole("menuitem", { name: "Reset Current Branch to Here (Mixed)…" }));
    dialog = await screen.findByRole("dialog");
    fireEvent.click(within(dialog).getByRole("button", { name: "Reset" }));
    await waitFor(() => expect(api.reset).toHaveBeenLastCalledWith({ root: ROOT, rev: oid(3), mode: "mixed" }));
  });

  it("reloads when the refs of this repository change", async () => {
    const api = backend();
    installScmStore();
    show(<ReflogTab tab={tab("reflog", { root: ROOT })} groupId="g" visible focused />);
    await screen.findByRole("list", { name: "Reflog" });
    expect(api.reflog).toHaveBeenCalledTimes(1);
    await act(async () => {
      await events.refsChanged.emit({ root: ROOT });
    });
    await waitFor(() => expect(api.reflog).toHaveBeenCalledTimes(2), { timeout: 2000 });
  });

  it("shows an error with retry", async () => {
    const api = backend();
    api.reflog.mockImplementationOnce(() => {
      throw { code: "unknown", message: "no reflog", detail: null };
    });
    show(<ReflogTab tab={tab("reflog", { root: ROOT })} groupId="g" visible focused />);
    expect(await screen.findByRole("alert")).toHaveTextContent("Could not load the reflog: no reflog");
    fireEvent.click(screen.getByRole("button", { name: /Retry/ }));
    expect(await screen.findByRole("list", { name: "Reflog" })).toBeInTheDocument();
  });
});

describe("FileHistoryTab", () => {
  const commits: Commit[] = [commit(1, { subject: "Tighten validation" }), commit(2, { subject: "Add login form" })];

  it("lists the commits that touched the file and opens the file's diff at a commit", async () => {
    const log = vi.fn((_: { root: string; query: LogQuery; limit: number }) => commits);
    mockCommands({ gitLog: log });
    show(<FileHistoryTab tab={tab("fileHistory", { root: ROOT, path: "src/login.rs" })} groupId="g" visible focused />);

    const list = await screen.findByRole("list", { name: "History of src/login.rs" });
    expect(log).toHaveBeenCalledWith({ root: ROOT, query: { path: "src/login.rs" }, limit: 500 });
    const rows = within(list).getAllByRole("button");
    expect(rows).toHaveLength(2);
    expect(rows[1]).toHaveTextContent("Add login form");
    expect(rows[1]).toHaveTextContent("Ana");
    expect(rows[1]).toHaveTextContent(oid(2).slice(0, 8));
    expect(screen.getByText("2 commits")).toBeInTheDocument();

    fireEvent.click(rows[1]);
    expect(rows[1]).toHaveAttribute("aria-pressed", "true");
    const diff = allTabs().find((t) => t.kind === "diff");
    expect(diff?.params).toEqual({ root: ROOT, path: "src/login.rs", origPath: null, source: { kind: "commit", oid: oid(2), parent: null } });
    expect(diff?.preview).toBe(true);
  });

  it("can show an entry in the graph", async () => {
    mockCommands({ gitLog: () => commits });
    show(<FileHistoryTab tab={tab("fileHistory", { root: ROOT, path: "src/login.rs" })} groupId="g" visible focused />);
    const list = await screen.findByRole("list", { name: "History of src/login.rs" });
    fireEvent.contextMenu(within(list).getAllByRole("button")[0]);
    fireEvent.click(await screen.findByRole("menuitem", { name: "Show in Graph" }));
    await waitFor(() => expect(allTabs().find((t) => t.kind === "graph")?.id).toBe(`graph:${ROOT}`));
    expect(takeReveal(ROOT)).toEqual({ oid: oid(1), label: oid(1).slice(0, 8), hunt: true });
  });

  it("says so when no commit touches the file", async () => {
    mockCommands({ gitLog: () => [] });
    show(<FileHistoryTab tab={tab("fileHistory", { root: ROOT, path: "new.txt" })} groupId="g" visible focused />);
    expect(await screen.findByText("No commits touch this file yet.")).toBeInTheDocument();
  });
});

describe("StashTab", () => {
  const files: CommitFile[] = [
    { path: "src/login.rs", origPath: null, kind: "modified", additions: null, deletions: null },
    { path: "notes.txt", origPath: null, kind: "added", additions: null, deletions: null },
  ];
  const stash = (index: number, id: number, message: string): Stash => ({ index, name: `stash@{${index}}`, oid: oid(id), message, time: 1_700_000_000 });

  function backend() {
    const calls = {
      files: vi.fn((_: { root: string; name: string }) => files),
      apply: vi.fn((_: { root: string; name: string; pop: boolean }) => null),
      drop: vi.fn((_: { root: string; name: string }) => null),
    };
    mockCommands({ gitStashFiles: calls.files, gitStashApply: calls.apply, gitStashDrop: calls.drop });
    return calls;
  }
  const withStashes = (...stashes: Stash[]) => resetScmForTest({ [ROOT]: repoState({ stashes }) }, ROOT);

  it("lists the stashed files and opens each as a stash diff", async () => {
    const api = backend();
    withStashes(stash(0, 50, "WIP on main: newest"), stash(1, 51, "On main: login work"));
    show(<StashTab tab={tab("stash", { root: ROOT, name: "stash@{1}" })} groupId="g" visible focused />);

    const items = await screen.findAllByRole("listitem");
    expect(api.files).toHaveBeenCalledWith({ root: ROOT, name: "stash@{1}" });
    expect(items).toHaveLength(2);
    expect(screen.getByText("2 changed files")).toBeInTheDocument();
    expect(screen.getByText(/On main: login work/)).toBeInTheDocument();
    // Stash tidak menyediakan jumlah baris, jadi berkas tidak dilabeli "binary".
    expect(screen.queryByText("binary")).not.toBeInTheDocument();

    fireEvent.click(items[0]);
    expect(allTabs().find((t) => t.kind === "diff")?.params).toEqual({ root: ROOT, path: "src/login.rs", origPath: null, source: { kind: "stash", name: "stash@{1}" } });
  });

  it("applies and pops the stash", async () => {
    const api = backend();
    withStashes(stash(0, 50, "WIP"));
    show(<StashTab tab={tab("stash", { root: ROOT, name: "stash@{0}" })} groupId="g" visible focused />);
    await screen.findAllByRole("listitem");
    fireEvent.click(screen.getByRole("button", { name: "Apply" }));
    await waitFor(() => expect(api.apply).toHaveBeenCalledWith({ root: ROOT, name: "stash@{0}", pop: false }));
    fireEvent.click(screen.getByRole("button", { name: "Pop" }));
    await waitFor(() => expect(api.apply).toHaveBeenLastCalledWith({ root: ROOT, name: "stash@{0}", pop: true }));
  });

  it("drops the stash only after confirmation", async () => {
    const api = backend();
    withStashes(stash(0, 50, "WIP on main: experiment"));
    show(<StashTab tab={tab("stash", { root: ROOT, name: "stash@{0}" })} groupId="g" visible focused />);
    await screen.findAllByRole("listitem");

    fireEvent.click(screen.getByRole("button", { name: "Drop…" }));
    const dialog = await screen.findByRole("dialog");
    expect(dialog).toHaveTextContent("Drop stash@{0}?");
    expect(dialog).toHaveTextContent("WIP on main: experiment");
    expect(api.drop).not.toHaveBeenCalled();
    fireEvent.click(within(dialog).getByRole("button", { name: "Drop Stash" }));
    await waitFor(() => expect(api.drop).toHaveBeenCalledWith({ root: ROOT, name: "stash@{0}" }));
  });

  it("follows the stash when its name shifts, and disables actions once it is gone", async () => {
    const api = backend();
    withStashes(stash(0, 50, "WIP: newest"), stash(1, 51, "On main: login work"));
    show(<StashTab tab={tab("stash", { root: ROOT, name: "stash@{1}" })} groupId="g" visible focused />);
    await screen.findAllByRole("listitem");

    // Stash terbaru dihapus di tempat lain: stash ini kini bernama stash@{0}.
    act(() => withStashes(stash(0, 51, "On main: login work")));
    await waitFor(() => expect(api.files).toHaveBeenLastCalledWith({ root: ROOT, name: "stash@{0}" }));
    fireEvent.click(screen.getByRole("button", { name: "Pop" }));
    await waitFor(() => expect(api.apply).toHaveBeenCalledWith({ root: ROOT, name: "stash@{0}", pop: true }));

    act(() => withStashes());
    expect(await screen.findByText(/This stash no longer exists/)).toBeInTheDocument();
    for (const name of ["Apply", "Pop", "Drop…"]) expect(screen.getByRole("button", { name })).toBeDisabled();
  });
});
