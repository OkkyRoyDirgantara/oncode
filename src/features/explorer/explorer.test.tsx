import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { clearMocks } from "@tauri-apps/api/mocks";
import { Text } from "@codemirror/state";
import { OverlayHost, resetOverlayForTest } from "@/app/overlay";
import { useApp } from "@/app/store";
import { allTabs, openTab, resetWorkbenchForTest, useWorkbench } from "@/app/workbench";
import { events, mockCommands, type DirEntry, type RepoSnapshot, type WorkspaceInfo } from "@/ipc";
import { bufferText, ensureBuffer, getBuffer, isDirty, resetBuffersForTest, retainBuffer, setBufferDoc } from "../editor/buffers";
import { resetScmForTest } from "../scm/store";
import { ExplorerView } from "./ExplorerView";
import { applyRename } from "./actions";
import { watchFsChanges } from "./index";
import { expandDir, loadDir, resetExplorerForTest, revealPath, selectPath, setRoot, useExplorer } from "./store";

const dir = (path: string, extra: Partial<DirEntry> = {}): DirEntry => ({ name: path.slice(path.lastIndexOf("/") + 1), path, isDir: true, isSymlink: false, ignored: false, ...extra });
const file = (path: string, extra: Partial<DirEntry> = {}): DirEntry => ({ ...dir(path, extra), isDir: false });

/** Isi "disk" tiruan; pengujian mengubahnya untuk meniru perubahan dari luar. */
let disk: Record<string, DirEntry[]>;
const readDir = vi.fn(({ dir: path }: { dir: string }) => {
  const entries = disk[path];
  if (!entries) throw { code: "io", message: `Cannot read ${path}`, detail: null };
  return entries;
});
const readDirCalls = () => readDir.mock.calls.map(([args]) => args.dir);

const workspace = { root: "/ws", name: "ws", trusted: true, repoRoot: null, settings: {} } as unknown as WorkspaceInfo;

// jsdom tidak menghitung tata letak; virtualizer butuh tinggi area gulir.
const clientHeight = Object.getOwnPropertyDescriptor(HTMLElement.prototype, "clientHeight");
beforeAll(() => Object.defineProperty(HTMLElement.prototype, "clientHeight", { configurable: true, get: () => 600 }));
afterAll(() => {
  if (clientHeight) Object.defineProperty(HTMLElement.prototype, "clientHeight", clientHeight);
});

beforeEach(() => {
  disk = {
    "/ws": [dir("/ws/src"), dir("/ws/target", { ignored: true }), file("/ws/README.md")],
    "/ws/src": [dir("/ws/src/app"), file("/ws/src/main.ts")],
    "/ws/src/app": [file("/ws/src/app/store.ts")],
    "/ws/target": [],
  };
  readDir.mockClear();
  mockCommands({ fsReadDir: readDir });
  resetExplorerForTest();
  resetWorkbenchForTest();
  resetBuffersForTest();
  resetOverlayForTest();
  resetScmForTest();
  useApp.setState({ workspace, sidebarVisible: true, sidebarView: "explorer" });
});

afterEach(() => {
  clearMocks();
  useApp.setState({ workspace: null });
});

const row = (name: string) => screen.getByText(name).closest('[role="treeitem"]') as HTMLElement;
const names = () => screen.getAllByRole("treeitem").map((el) => el.textContent);

describe("ExplorerView", () => {
  it("lists the workspace folder and loads a folder's children only when it is expanded", async () => {
    render(<ExplorerView />);
    expect(await screen.findByText("README.md")).toBeInTheDocument();
    expect(names()).toEqual(["src", "target", "README.md"]);
    expect(readDirCalls()).toEqual(["/ws"]);
    expect(row("src")).toHaveAttribute("aria-expanded", "false");
    expect(row("src")).toHaveAttribute("aria-level", "1");

    fireEvent.click(screen.getByText("src"));
    expect(await screen.findByText("main.ts")).toBeInTheDocument();
    expect(readDirCalls()).toEqual(["/ws", "/ws/src"]);
    expect(names()).toEqual(["src", "app", "main.ts", "target", "README.md"]);
    expect(row("src")).toHaveAttribute("aria-expanded", "true");
    expect(row("main.ts")).toHaveAttribute("aria-level", "2");

    // Menutup lalu membuka lagi memakai isi yang sudah dimuat.
    fireEvent.click(screen.getByText("src"));
    expect(screen.queryByText("main.ts")).not.toBeInTheDocument();
    fireEvent.click(screen.getByText("src"));
    expect(screen.getByText("main.ts")).toBeInTheDocument();
    expect(readDirCalls()).toEqual(["/ws", "/ws/src"]);
  });

  it("opens a file as a preview on click and pins it on double click", async () => {
    render(<ExplorerView />);
    fireEvent.click(await screen.findByText("README.md"));
    expect(allTabs()).toMatchObject([{ id: "file:/ws/README.md", preview: true, params: { path: "/ws/README.md" } }]);
    fireEvent.doubleClick(screen.getByText("README.md"));
    expect(allTabs()).toMatchObject([{ id: "file:/ws/README.md", preview: false }]);
    expect(row("README.md")).toHaveAttribute("aria-selected", "true");
  });

  it("dims ignored entries and decorates changed files and their folders", async () => {
    const snapshot = {
      root: "/ws",
      head: { kind: "branch", name: "main" },
      headOid: null,
      upstream: null,
      operation: null,
      staged: [],
      unstaged: [{ path: "src/main.ts", origPath: null, kind: "modified", submodule: null }],
      untracked: [],
      conflicted: [],
      stashCount: 0,
    } as unknown as RepoSnapshot;
    resetScmForTest({ "/ws": { root: "/ws", snapshot, refs: null, stashes: [], worktrees: [], submodules: [], busy: null } }, "/ws");
    render(<ExplorerView />);
    fireEvent.click(await screen.findByText("src"));
    await screen.findByText("main.ts");

    expect(screen.getByText("target")).toHaveClass("oc-git-ignored");
    expect(screen.getByText("main.ts")).toHaveClass("oc-git-modified");
    expect(within(row("main.ts")).getByText("M")).toHaveClass("oc-status-letter");
    expect(screen.getByText("src")).toHaveClass("oc-git-modified");
    expect(within(row("src")).queryByText("M")).not.toBeInTheDocument();
    expect(screen.getByText("README.md")).not.toHaveClass("oc-git-modified");
  });

  it("shows the error inline under a folder that cannot be read", async () => {
    delete disk["/ws/target"];
    render(<ExplorerView />);
    fireEvent.click(await screen.findByText("target"));
    expect(await screen.findByText("Cannot read /ws/target")).toHaveAttribute("role", "alert");
    expect(names()).toEqual(["src", "target", "README.md"]);
  });

  it("moves the selection and expands or collapses with the keyboard", async () => {
    render(<ExplorerView />);
    await screen.findByText("README.md");
    const tree = screen.getByRole("tree");
    const selected = () => useExplorer.getState().selected;

    fireEvent.keyDown(tree, { key: "ArrowDown" });
    expect(selected()).toBe("/ws/src");
    fireEvent.keyDown(tree, { key: "ArrowRight" });
    await screen.findByText("main.ts");
    fireEvent.keyDown(tree, { key: "ArrowRight" });
    expect(selected()).toBe("/ws/src/app");
    fireEvent.keyDown(tree, { key: "ArrowDown" });
    expect(selected()).toBe("/ws/src/main.ts");
    // Kiri pada berkas pindah ke folder induknya; kiri sekali lagi menutup folder itu.
    fireEvent.keyDown(tree, { key: "ArrowLeft" });
    expect(selected()).toBe("/ws/src");
    fireEvent.keyDown(tree, { key: "ArrowLeft" });
    expect(screen.queryByText("main.ts")).not.toBeInTheDocument();
    fireEvent.keyDown(tree, { key: "ArrowUp" });
    expect(selected()).toBe("/ws/src");

    fireEvent.keyDown(tree, { key: "End" });
    expect(selected()).toBe("/ws/README.md");
    fireEvent.keyDown(tree, { key: "Enter" });
    expect(allTabs().map((t) => t.id)).toEqual(["file:/ws/README.md"]);
  });

  it("reveals the file of the active editor tab by expanding its ancestors", async () => {
    render(<ExplorerView />);
    await screen.findByText("README.md");
    act(() => void openTab({ id: "file:/ws/src/app/store.ts", kind: "file", title: "store.ts", params: { path: "/ws/src/app/store.ts" } }));

    expect(await screen.findByText("store.ts")).toBeInTheDocument();
    expect(readDirCalls()).toEqual(["/ws", "/ws/src", "/ws/src/app"]);
    expect(row("store.ts")).toHaveAttribute("data-active", "true");
    expect(row("store.ts")).toHaveAttribute("aria-selected", "true");
    expect(row("main.ts")).not.toHaveAttribute("data-active");
  });
});

describe("explorer store", () => {
  it("does not reveal paths outside the workspace or missing from the listing", async () => {
    setRoot("/ws");
    expect(await revealPath("/elsewhere/file.ts")).toBe(false);
    expect(await revealPath("/ws/src/missing.ts")).toBe(false);
    expect(useExplorer.getState().expanded).toEqual({});
    expect(await revealPath("/ws/src/main.ts")).toBe(true);
    expect(useExplorer.getState()).toMatchObject({ expanded: { "/ws/src": true }, selected: "/ws/src/main.ts" });
  });

  it("drops the tree when the workspace folder changes and ignores late answers from the old one", async () => {
    let release!: (entries: DirEntry[]) => void;
    readDir.mockImplementationOnce(() => new Promise<DirEntry[]>((resolve) => (release = resolve)) as unknown as DirEntry[]);
    setRoot("/ws");
    disk["/other"] = [file("/other/b.txt")];
    setRoot("/other");
    await waitFor(() => expect(useExplorer.getState().dirs["/other"]?.entries).toHaveLength(1));
    release([file("/ws/stale.txt")]);
    await Promise.resolve();
    expect(Object.keys(useExplorer.getState().dirs)).toEqual(["/other"]);
    expect(useExplorer.getState()).toMatchObject({ root: "/other", expanded: {}, selected: null });
  });
});

describe("external changes", () => {
  let stop: () => void;
  beforeEach(async () => {
    stop = watchFsChanges();
    setRoot("/ws");
    await loadDir("/ws");
    await expandDir("/ws/src");
    await expandDir("/ws/src/app");
    selectPath("/ws/src/main.ts");
    readDir.mockClear();
  });
  afterEach(() => stop());

  it("reloads only the expanded folder that contains the changed path, keeping expansion and selection", async () => {
    disk["/ws/src"] = [...disk["/ws/src"], file("/ws/src/new.ts")];
    await events.fsChanged.emit({ paths: ["/ws/src/new.ts"], overflow: false });
    expect(readDir).not.toHaveBeenCalled();

    await waitFor(() => expect(useExplorer.getState().dirs["/ws/src"].entries?.map((e) => e.name)).toEqual(["app", "main.ts", "new.ts"]));
    expect(readDirCalls()).toEqual(["/ws/src"]);
    expect(useExplorer.getState()).toMatchObject({ expanded: { "/ws/src": true, "/ws/src/app": true }, selected: "/ws/src/main.ts" });
  });

  it("coalesces a burst of events into one reload per folder", async () => {
    await events.fsChanged.emit({ paths: ["/ws/src/a.ts"], overflow: false });
    await events.fsChanged.emit({ paths: ["/ws/src/b.ts", "/ws/README.md"], overflow: false });
    await events.fsChanged.emit({ paths: ["/ws/src/a.ts"], overflow: false });
    await waitFor(() => expect(readDir).toHaveBeenCalled());
    await waitFor(() => expect(Object.values(useExplorer.getState().dirs).some((d) => d.loading)).toBe(false));
    expect(readDirCalls().sort()).toEqual(["/ws", "/ws/src"]);
  });

  it("forgets a cached but collapsed folder instead of reloading it", async () => {
    await expandDir("/ws/target");
    useExplorer.setState((s) => ({ expanded: { ...s.expanded, "/ws/target": false } }));
    readDir.mockClear();
    disk["/ws/target"] = [file("/ws/target/out.bin")];

    await events.fsChanged.emit({ paths: ["/ws/target/out.bin"], overflow: false });
    await waitFor(() => expect(useExplorer.getState().dirs["/ws/target"]).toBeUndefined());
    expect(readDir).not.toHaveBeenCalled();
    await expandDir("/ws/target");
    expect(useExplorer.getState().dirs["/ws/target"].entries?.map((e) => e.name)).toEqual(["out.bin"]);
  });

  it("reloads the root and every expanded folder on overflow", async () => {
    await events.fsChanged.emit({ paths: [], overflow: true });
    await waitFor(() => expect(readDirCalls().sort()).toEqual(["/ws", "/ws/src", "/ws/src/app"]));
  });

  it("drops expansion and selection under an entry that disappeared", async () => {
    selectPath("/ws/src/app/store.ts");
    disk["/ws/src"] = [file("/ws/src/main.ts")];
    await events.fsChanged.emit({ paths: ["/ws/src/app"], overflow: false });
    await waitFor(() => expect(useExplorer.getState().expanded).toEqual({ "/ws/src": true }));
    expect(useExplorer.getState().selected).toBeNull();
    expect(useExplorer.getState().dirs["/ws/src/app"]).toBeUndefined();
  });
});

describe("rename", () => {
  const fileTab = (path: string) => ({ id: `file:${path}`, kind: "file", title: path.slice(path.lastIndexOf("/") + 1), description: path, params: { path } });

  it("rewrites the tabs of every open file under a renamed folder, in every group", () => {
    openTab(fileTab("/ws/src/main.ts"));
    openTab(fileTab("/ws/src/app/store.ts"));
    openTab(fileTab("/ws/README.md"));
    openTab(fileTab("/ws/src/main.ts"), { toSide: true });
    const activeBefore = useWorkbench.getState().groups.map((g) => g.activeId);
    expect(activeBefore).toEqual(["file:/ws/README.md", "file:/ws/src/main.ts"]);

    applyRename("/ws/src", "/ws/lib");

    const groups = useWorkbench.getState().groups;
    expect(groups[0].tabs.map((t) => [t.id, t.title, t.description, t.params.path])).toEqual([
      ["file:/ws/lib/main.ts", "main.ts", "/ws/lib/main.ts", "/ws/lib/main.ts"],
      ["file:/ws/lib/app/store.ts", "store.ts", "/ws/lib/app/store.ts", "/ws/lib/app/store.ts"],
      ["file:/ws/README.md", "README.md", "/ws/README.md", "/ws/README.md"],
    ]);
    expect(groups[1].tabs.map((t) => t.id)).toEqual(["file:/ws/lib/main.ts"]);
    // Tab aktif tetap tab yang sama, dengan id barunya.
    expect(groups.map((g) => g.activeId)).toEqual(["file:/ws/README.md", "file:/ws/lib/main.ts"]);
  });

  it("keeps unsaved content attached to the new path", async () => {
    mockCommands({
      fsReadDir: readDir,
      fsOpen: () => ({ kind: "text", text: "on disk", encoding: "UTF-8", bom: false, eol: "lf", stat: { size: 7, mtimeMs: 1, readOnly: false } }),
    });
    retainBuffer("/ws/src/main.ts");
    await ensureBuffer("/ws/src/main.ts");
    setBufferDoc("/ws/src/main.ts", Text.of(["edited, not saved"]));
    openTab(fileTab("/ws/src/main.ts"));
    expect(isDirty("/ws/src/main.ts")).toBe(true);

    applyRename("/ws/src", "/ws/lib");

    expect(getBuffer("/ws/src/main.ts")).toBeUndefined();
    expect(bufferText("/ws/lib/main.ts")).toBe("edited, not saved");
    expect(isDirty("/ws/lib/main.ts")).toBe(true);
    expect(isDirty("/ws/src/main.ts")).toBe(false);
    expect(allTabs().map((t) => t.id)).toEqual(["file:/ws/lib/main.ts"]);
  });

  it("asks for the new name with the stem selected, renames on disk, and follows up in tabs and tree", async () => {
    const rename = vi.fn(({ from, to }: { from: string; to: string }) => {
      disk["/ws/src"] = disk["/ws/src"].map((e) => (e.path === from ? file(to) : e));
      return null;
    });
    mockCommands({ fsReadDir: readDir, fsRename: rename });
    openTab(fileTab("/ws/src/main.ts"));
    render(
      <>
        <ExplorerView />
        <OverlayHost />
      </>,
    );
    await screen.findByText("main.ts");
    const tree = screen.getByRole("tree");
    expect(useExplorer.getState().selected).toBe("/ws/src/main.ts");

    fireEvent.keyDown(tree, { key: "F2" });
    const input = (await screen.findByRole("textbox")) as HTMLInputElement;
    expect(input.value).toBe("main.ts");
    expect([input.selectionStart, input.selectionEnd]).toEqual([0, 4]);

    // Nama yang sudah dipakai saudaranya ditolak sebelum menyentuh disk.
    fireEvent.change(input, { target: { value: "app" } });
    expect(screen.getByText(/already exists/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Rename" })).toBeDisabled();

    fireEvent.change(input, { target: { value: "index.ts" } });
    fireEvent.click(screen.getByRole("button", { name: "Rename" }));

    expect(await screen.findByText("index.ts")).toBeInTheDocument();
    expect(rename).toHaveBeenCalledWith({ from: "/ws/src/main.ts", to: "/ws/src/index.ts" });
    expect(screen.queryByText("main.ts")).not.toBeInTheDocument();
    expect(allTabs()).toMatchObject([{ id: "file:/ws/src/index.ts", title: "index.ts", params: { path: "/ws/src/index.ts" } }]);
    await waitFor(() => expect(useExplorer.getState().selected).toBe("/ws/src/index.ts"));
  });

  it("keeps a renamed folder expanded and reloads its children under the new path", async () => {
    const rename = vi.fn(() => {
      disk["/ws"] = [dir("/ws/lib"), dir("/ws/target", { ignored: true }), file("/ws/README.md")];
      disk["/ws/lib"] = [dir("/ws/lib/app"), file("/ws/lib/main.ts")];
      disk["/ws/lib/app"] = [file("/ws/lib/app/store.ts")];
      delete disk["/ws/src"];
      delete disk["/ws/src/app"];
      return null;
    });
    mockCommands({ fsReadDir: readDir, fsRename: rename });
    render(
      <>
        <ExplorerView />
        <OverlayHost />
      </>,
    );
    await screen.findByText("README.md");
    await act(() => revealPath("/ws/src/app/store.ts"));
    act(() => selectPath("/ws/src"));

    fireEvent.keyDown(screen.getByRole("tree"), { key: "F2" });
    const input = (await screen.findByRole("textbox")) as HTMLInputElement;
    // Folder tidak punya ekstensi: seluruh nama terpilih.
    expect([input.selectionStart, input.selectionEnd]).toEqual([0, 3]);
    fireEvent.change(input, { target: { value: "lib" } });
    fireEvent.click(screen.getByRole("button", { name: "Rename" }));

    await waitFor(() => expect(names()).toEqual(["lib", "app", "store.ts", "main.ts", "target", "README.md"]));
    expect(rename).toHaveBeenCalledWith({ from: "/ws/src", to: "/ws/lib" });
    expect(useExplorer.getState()).toMatchObject({ expanded: { "/ws/lib": true, "/ws/lib/app": true }, selected: "/ws/lib" });
    expect(Object.keys(useExplorer.getState().dirs).sort()).toEqual(["/ws", "/ws/lib", "/ws/lib/app"]);
  });
});

describe("delete and create", () => {
  it("confirms, names the unsaved file, moves the folder to the trash, and closes the tabs under it", async () => {
    const trash = vi.fn(({ paths }: { paths: string[] }) => {
      disk["/ws"] = disk["/ws"].filter((e) => !paths.includes(e.path));
      return null;
    });
    mockCommands({
      fsReadDir: readDir,
      fsTrash: trash,
      fsOpen: () => ({ kind: "text", text: "on disk", encoding: "UTF-8", bom: false, eol: "lf", stat: { size: 7, mtimeMs: 1, readOnly: false } }),
    });
    retainBuffer("/ws/src/main.ts");
    await ensureBuffer("/ws/src/main.ts");
    setBufferDoc("/ws/src/main.ts", Text.of(["edited"]));
    openTab({ id: "file:/ws/README.md", kind: "file", title: "README.md", params: { path: "/ws/README.md" } });
    openTab({ id: "file:/ws/src/main.ts", kind: "file", title: "main.ts", params: { path: "/ws/src/main.ts" } });
    render(
      <>
        <ExplorerView />
        <OverlayHost />
      </>,
    );
    await screen.findByText("main.ts");
    act(() => selectPath("/ws/src"));

    fireEvent.keyDown(screen.getByRole("tree"), { key: "Delete" });
    const dialog = await screen.findByRole("dialog");
    expect(dialog).toHaveTextContent('Delete folder "src"?');
    expect(dialog).toHaveTextContent('"src" and everything inside it will be moved to the system trash.');
    expect(dialog).toHaveTextContent('"main.ts" has unsaved changes that will be lost.');
    expect(trash).not.toHaveBeenCalled();

    fireEvent.click(within(dialog).getByRole("button", { name: "Move to Trash" }));
    await waitFor(() => expect(screen.queryByText("src")).not.toBeInTheDocument());
    expect(trash).toHaveBeenCalledWith({ paths: ["/ws/src"] });
    expect(allTabs().map((t) => t.id)).toEqual(["file:/ws/README.md"]);
    expect(useExplorer.getState().expanded).toEqual({});
  });

  it("does nothing when the confirmation is cancelled", async () => {
    const trash = vi.fn(() => null);
    mockCommands({ fsReadDir: readDir, fsTrash: trash });
    render(
      <>
        <ExplorerView />
        <OverlayHost />
      </>,
    );
    await screen.findByText("README.md");
    act(() => selectPath("/ws/README.md"));
    fireEvent.keyDown(screen.getByRole("tree"), { key: "Delete" });
    const dialog = await screen.findByRole("dialog");
    expect(dialog).not.toHaveTextContent("unsaved");
    fireEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(trash).not.toHaveBeenCalled();
    expect(screen.getByText("README.md")).toBeInTheDocument();
  });

  it("creates a nested file inside the selected folder, reveals it, and opens it", async () => {
    const create = vi.fn(({ path }: { path: string }) => {
      disk["/ws/src"] = [dir("/ws/src/app"), dir("/ws/src/util"), file("/ws/src/main.ts")];
      disk["/ws/src/util"] = [file(path)];
      return null;
    });
    mockCommands({ fsReadDir: readDir, fsCreateFile: create });
    render(
      <>
        <ExplorerView />
        <OverlayHost />
      </>,
    );
    await screen.findByText("README.md");
    act(() => selectPath("/ws/src"));

    fireEvent.click(screen.getByRole("button", { name: "New File" }));
    const input = await screen.findByRole("textbox");
    expect(screen.getByRole("dialog")).toHaveTextContent("Name of the new file in src");
    fireEvent.change(input, { target: { value: "../escape.ts" } });
    expect(screen.getByRole("button", { name: "Create" })).toBeDisabled();
    fireEvent.change(input, { target: { value: "util/strings.ts" } });
    fireEvent.click(screen.getByRole("button", { name: "Create" }));

    expect(await screen.findByText("strings.ts")).toBeInTheDocument();
    expect(create).toHaveBeenCalledWith({ path: "/ws/src/util/strings.ts" });
    expect(useExplorer.getState()).toMatchObject({ selected: "/ws/src/util/strings.ts", expanded: { "/ws/src": true, "/ws/src/util": true } });
    expect(allTabs()).toMatchObject([{ id: "file:/ws/src/util/strings.ts", preview: false }]);
  });
});
