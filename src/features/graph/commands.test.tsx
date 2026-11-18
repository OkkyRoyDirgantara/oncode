import { StrictMode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { Channel } from "@tauri-apps/api/core";
import { clearMocks } from "@tauri-apps/api/mocks";
import { open as pickNative } from "@tauri-apps/plugin-dialog";
import { executeCommand, getCommand, keybindingsFor, resetCommandsForTest } from "@/app/commands";
import { OverlayHost, resetOverlayForTest } from "@/app/overlay";
import { getSidebarViews, getTabType } from "@/app/registry";
import { allTabs, resetWorkbenchForTest } from "@/app/workbench";
import { mockCommands, type Progress, type RebasePlan } from "@/ipc";
import { resetScmForTest } from "@/features/scm/store";
import { takeReveal } from "./bus";
import { registerGraphFeature } from "./index";
import { branch, oid, repoState, ROOT, snapshot } from "./testing";

// Editor (CodeMirror) tidak diperlukan di sini; cukup path berkas yang sedang aktif.
const editor = vi.hoisted(() => ({ path: null as string | null }));
vi.mock("@/features/editor/CodeEditor", () => ({ activeEditor: () => (editor.path ? { view: {}, path: editor.path } : null) }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));
const pickFolder = vi.mocked(pickNative);

const refsMocks = { gitRefs: () => repoState().refs!, gitStashList: () => [], gitWorktrees: () => [], gitSubmodules: () => [] };

beforeEach(() => {
  resetCommandsForTest();
  resetWorkbenchForTest();
  resetOverlayForTest();
  resetScmForTest({ [ROOT]: repoState() }, ROOT);
  editor.path = null;
  pickFolder.mockReset();
  registerGraphFeature();
  render(<OverlayHost />);
});
afterEach(() => clearMocks());

describe("registerGraphFeature", () => {
  it("registers the tab kinds, the sidebar view and the commands", () => {
    for (const kind of ["graph", "reflog", "fileHistory", "stash"]) expect(getTabType(kind), kind).toBeDefined();
    expect(getSidebarViews().find((v) => v.id === "graph")).toMatchObject({ title: "Graph", order: 40 });
    for (const id of ["git.showGraph", "git.showReflog", "git.interactiveRebase", "git.fileHistory", "git.fileHistoryActive", "git.showStash", "git.addWorktree", "git.removeWorktree", "git.updateSubmodules"]) {
      expect(getCommand(id), id).toBeDefined();
    }
    expect(keybindingsFor(getCommand("git.showGraph")!)).toEqual(["Mod+Shift+H"]);
    expect(getCommand("git.showGraph")).toMatchObject({ title: "Show Graph", category: "Git" });
    expect(getCommand("git.fileHistoryActive")).toMatchObject({ title: "Show File History", category: "Git" });
  });

  it("git.showGraph opens one Graph tab for the active repository", async () => {
    await executeCommand("git.showGraph");
    await executeCommand("git.showGraph");
    const tabs = allTabs().filter((t) => t.kind === "graph");
    expect(tabs).toHaveLength(1);
    expect(tabs[0]).toMatchObject({ id: `graph:${ROOT}`, title: "Graph", params: { root: ROOT } });
  });

  it("git.showGraph explains itself when there is no repository", async () => {
    resetScmForTest({}, null);
    await executeCommand("git.showGraph");
    expect(allTabs()).toHaveLength(0);
    expect(await screen.findByText("This folder is not a Git repository.")).toBeInTheDocument();
  });

  it("opens reflog, file history and stash tabs with their parameters", async () => {
    await executeCommand("git.showReflog");
    await executeCommand("git.fileHistory", ROOT, "src/login.rs");
    await executeCommand("git.showStash", ROOT, "stash@{2}");
    expect(allTabs().map((t) => [t.kind, t.params])).toEqual(
      expect.arrayContaining([
        ["reflog", { root: ROOT }],
        ["fileHistory", { root: ROOT, path: "src/login.rs" }],
        ["stash", { root: ROOT, name: "stash@{2}" }],
      ]),
    );
    expect(allTabs().find((t) => t.kind === "fileHistory")?.title).toBe("History: login.rs");
  });

  it("git.fileHistoryActive maps the active editor's absolute path into the repository", async () => {
    editor.path = `${ROOT}/src/features/login.rs`;
    await executeCommand("git.fileHistoryActive");
    expect(allTabs().find((t) => t.kind === "fileHistory")?.params).toEqual({ root: ROOT, path: "src/features/login.rs" });
  });

  it("git.fileHistoryActive picks the innermost repository for a file in a submodule", async () => {
    const sub = `${ROOT}/vendor/lib`;
    resetScmForTest({ [ROOT]: repoState(), [sub]: repoState({ root: sub }) }, ROOT);
    editor.path = `${sub}/src/x.c`;
    await executeCommand("git.fileHistoryActive");
    expect(allTabs().find((t) => t.kind === "fileHistory")?.params).toEqual({ root: sub, path: "src/x.c" });
  });

  it("git.fileHistoryActive does nothing without an editor or for a file outside the repository", async () => {
    await executeCommand("git.fileHistoryActive");
    expect(await screen.findByText("Open a file first to see its history.")).toBeInTheDocument();
    editor.path = "/tmp/elsewhere.txt";
    await executeCommand("git.fileHistoryActive");
    expect(await screen.findByText("This file is not inside an open Git repository.")).toBeInTheDocument();
    expect(allTabs()).toHaveLength(0);
  });

  it("git.interactiveRebase opens the dialog for the given base, or for the upstream when run from the palette", async () => {
    const plan = vi.fn((_: { root: string; base: string | null }): RebasePlan => ({ items: [{ action: "pick", oid: oid(1), subject: "One" }], published: 0 }));
    mockCommands({ gitRebasePlan: plan });

    void executeCommand("git.interactiveRebase", ROOT, oid(9));
    fireEvent.click(within(await screen.findByRole("dialog")).getByRole("button", { name: "Cancel" }));
    expect(plan).toHaveBeenLastCalledWith({ root: ROOT, base: oid(9) });

    void executeCommand("git.interactiveRebase", ROOT, null);
    fireEvent.click(within(await screen.findByRole("dialog")).getByRole("button", { name: "Cancel" }));
    expect(plan).toHaveBeenLastCalledWith({ root: ROOT, base: null });

    resetScmForTest({ [ROOT]: repoState({ snapshot: snapshot({ upstream: { name: "origin/main", ahead: 2, behind: 0, exists: true } }) }) }, ROOT);
    void executeCommand("git.interactiveRebase");
    fireEvent.click(within(await screen.findByRole("dialog")).getByRole("button", { name: "Cancel" }));
    expect(plan).toHaveBeenLastCalledWith({ root: ROOT, base: "origin/main" });
  });

  it("the sidebar view lists refs and opens the Graph on the chosen ref", () => {
    resetScmForTest({ [ROOT]: repoState({ refs: { branches: [branch("main", { isHead: true }), branch("feature/login", { oid: oid(7) })], remoteBranches: [], tags: [], remotes: [] } }) }, ROOT);
    const View = getSidebarViews().find((v) => v.id === "graph")!.component;
    const { container } = render(<View />);
    expect(within(container).getByText("main")).toBeInTheDocument();

    fireEvent.click(within(container).getByText("feature/login"));
    expect(allTabs().find((t) => t.kind === "graph")?.id).toBe(`graph:${ROOT}`);
    expect(takeReveal(ROOT)).toEqual({ oid: oid(7), label: "feature/login", hunt: true });

    resetWorkbenchForTest();
    fireEvent.click(within(container).getByRole("button", { name: "Open Graph" }));
    expect(allTabs().filter((t) => t.kind === "graph")).toHaveLength(1);
    expect(takeReveal(ROOT)).toBeNull();
  });
});

describe("sidebar view", () => {
  it("renders under StrictMode and survives store updates without render loops", () => {
    const errors = vi.spyOn(console, "error").mockImplementation(() => undefined);
    try {
      const View = getSidebarViews().find((v) => v.id === "graph")!.component;
      const { container } = render(
        <StrictMode>
          <View />
        </StrictMode>,
      );
      expect(within(container).getByText("main")).toBeInTheDocument();
      act(() => resetScmForTest({ [ROOT]: repoState({ refs: { branches: [branch("main", { isHead: true }), branch("next")], remoteBranches: [], tags: [], remotes: [] } }) }, ROOT));
      expect(within(container).getByText("next")).toBeInTheDocument();
      act(() => resetScmForTest({}, null));
      expect(within(container).getByText("Open a Git repository to see its commit graph.")).toBeInTheDocument();
      expect(errors).not.toHaveBeenCalled();
    } finally {
      errors.mockRestore();
    }
  });
});

describe("git.removeWorktree", () => {
  const path = "/work/repo-feature";

  it("removes a clean worktree without asking", async () => {
    const remove = vi.fn((_: { root: string; path: string; force: boolean }) => null);
    mockCommands({ gitWorktreeRemove: remove, ...refsMocks });
    await executeCommand("git.removeWorktree", ROOT, path);
    expect(remove).toHaveBeenCalledTimes(1);
    expect(remove).toHaveBeenCalledWith({ root: ROOT, path, force: false });
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it.each([
    ["the dirtyWorkTree code", { code: "dirtyWorkTree", message: "worktree has changes", detail: null }],
    ["git's message", { code: "unknown", message: `'${path}' contains modified or untracked files, use --force to delete it`, detail: null }],
  ])("asks before forcing when the worktree has changes (%s)", async (_label, error) => {
    const remove = vi.fn(({ force }: { root: string; path: string; force: boolean }) => {
      if (!force) throw error;
      return null;
    });
    mockCommands({ gitWorktreeRemove: remove, ...refsMocks });
    const done = executeCommand("git.removeWorktree", ROOT, path);

    const dialog = await screen.findByRole("dialog");
    expect(dialog).toHaveTextContent("uncommitted changes are lost permanently");
    expect(remove).toHaveBeenCalledTimes(1);
    const confirmButton = within(dialog).getByRole("button", { name: "Remove and Discard Changes" });
    expect(confirmButton).toHaveClass("oc-btn-danger");
    fireEvent.click(confirmButton);
    await done;
    expect(remove).toHaveBeenCalledTimes(2);
    expect(remove).toHaveBeenLastCalledWith({ root: ROOT, path, force: true });
  });

  it("does not force when the confirmation is cancelled", async () => {
    const remove = vi.fn((_: { root: string; path: string; force: boolean }): null => {
      throw { code: "dirtyWorkTree", message: "dirty", detail: null };
    });
    mockCommands({ gitWorktreeRemove: remove, ...refsMocks });
    const done = executeCommand("git.removeWorktree", ROOT, path);
    fireEvent.click(within(await screen.findByRole("dialog")).getByRole("button", { name: "Cancel" }));
    await done;
    expect(remove).toHaveBeenCalledTimes(1);
  });

  it("reports other failures without offering to force", async () => {
    const remove = vi.fn((_: { root: string; path: string; force: boolean }): null => {
      throw { code: "unknown", message: `'${path}' is not a working tree`, detail: null };
    });
    mockCommands({ gitWorktreeRemove: remove, ...refsMocks });
    await executeCommand("git.removeWorktree", ROOT, path);
    expect(await screen.findByText(`Remove worktree failed: '${path}' is not a working tree`)).toBeInTheDocument();
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(remove).toHaveBeenCalledTimes(1);
  });
});

describe("git.addWorktree", () => {
  function setup() {
    resetScmForTest(
      {
        [ROOT]: repoState({
          refs: {
            branches: [branch("main", { isHead: true }), branch("feature/login"), branch("busy", { worktree: "/work/repo-busy" })],
            remoteBranches: [],
            tags: [],
            remotes: [],
          },
        }),
      },
      ROOT,
    );
    const add = vi.fn((_: { root: string; path: string; newBranch: string | null; rev: string | null }) => null);
    const windowOpen = vi.fn((_: { folder: string | null }) => null);
    mockCommands({ gitWorktreeAdd: add, windowOpen, ...refsMocks });
    return { add, windowOpen };
  }

  it("checks out an existing free branch into <parent>/<repo>-<branch> and offers to open it", async () => {
    const { add, windowOpen } = setup();
    pickFolder.mockResolvedValue("/home/ana/code");
    const done = executeCommand("git.addWorktree");

    // Branch aktif dan branch yang dipakai worktree lain tidak ditawarkan.
    const options = (await screen.findAllByRole("option")).map((o) => o.textContent);
    expect(options).toEqual(["+ Create new branch…", "feature/login"]);
    fireEvent.click(screen.getByRole("option", { name: "feature/login" }));

    const dialog = await screen.findByRole("dialog");
    const input = within(dialog).getByLabelText<HTMLInputElement>("Folder name");
    expect(input.value).toBe("repo-feature-login");
    expect(dialog).toHaveTextContent("/home/ana/code");
    fireEvent.click(within(dialog).getByRole("button", { name: "Create Worktree" }));
    await done;

    expect(add).toHaveBeenCalledWith({ root: ROOT, path: "/home/ana/code/repo-feature-login", newBranch: null, rev: "feature/login" });
    fireEvent.click(await screen.findByRole("button", { name: "Open in New Window" }));
    await waitFor(() => expect(windowOpen).toHaveBeenCalledWith({ folder: "/home/ana/code/repo-feature-login" }));
  });

  it("creates a new branch when asked, using the edited folder name", async () => {
    const { add } = setup();
    pickFolder.mockResolvedValue("/home/ana/code");
    const done = executeCommand("git.addWorktree");
    fireEvent.click(await screen.findByRole("option", { name: "+ Create new branch…" }));

    let dialog = await screen.findByRole("dialog");
    const name = within(dialog).getByLabelText("New branch name");
    // Nama yang sudah dipakai ditolak sebelum git dijalankan.
    fireEvent.change(name, { target: { value: "main" } });
    expect(within(dialog).getByRole("button", { name: "OK" })).toBeDisabled();
    fireEvent.change(name, { target: { value: "spike/new-ui" } });
    fireEvent.click(within(dialog).getByRole("button", { name: "OK" }));

    dialog = await screen.findByRole("dialog", {}, { timeout: 2000 });
    const folder = await within(dialog).findByLabelText<HTMLInputElement>("Folder name");
    expect(folder.value).toBe("repo-spike-new-ui");
    fireEvent.change(folder, { target: { value: "a/b" } });
    expect(within(dialog).getByRole("button", { name: "Create Worktree" })).toBeDisabled();
    fireEvent.change(folder, { target: { value: "ui-spike" } });
    fireEvent.click(within(dialog).getByRole("button", { name: "Create Worktree" }));
    await done;

    expect(add).toHaveBeenCalledWith({ root: ROOT, path: "/home/ana/code/ui-spike", newBranch: "spike/new-ui", rev: null });
  });

  it("stops quietly when the folder picker is cancelled", async () => {
    const { add } = setup();
    pickFolder.mockResolvedValue(null);
    const done = executeCommand("git.addWorktree");
    fireEvent.click(await screen.findByRole("option", { name: "feature/login" }));
    await done;
    expect(add).not.toHaveBeenCalled();
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });
});

describe("git.updateSubmodules", () => {
  it("runs submodule update --init for the given paths with a cancellable operation", async () => {
    const update = vi.fn((_: { root: string; paths: string[]; init: boolean; operationId: string; onProgress: Channel<Progress> }) => null);
    mockCommands({ gitSubmoduleUpdate: update, ...refsMocks });
    await executeCommand("git.updateSubmodules", ROOT, ["vendor/lib"]);
    expect(update).toHaveBeenCalledTimes(1);
    const args = update.mock.calls[0][0];
    expect(args).toMatchObject({ root: ROOT, paths: ["vendor/lib"], init: true });
    expect(args.operationId).toMatch(/^op-/);
    expect(args.onProgress).toBeInstanceOf(Channel);
  });

  it("reports a failure", async () => {
    mockCommands({
      gitSubmoduleUpdate: () => {
        throw { code: "unknown", message: "clone failed", detail: null };
      },
      ...refsMocks,
    });
    await executeCommand("git.updateSubmodules", ROOT, []);
    expect(await screen.findByText("Update submodules failed: clone failed")).toBeInTheDocument();
  });
});
