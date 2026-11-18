import { afterEach, beforeAll, beforeEach, describe, expect, it } from "vitest";
import { clearMocks } from "@tauri-apps/api/mocks";
import { executeCommand, getCommand, keybindingsFor } from "@/app/commands";
import { getPanelViews } from "@/app/registry";
import { openFolder, useApp } from "@/app/store";
import { mockCommands, type AppInfo, type Settings, type WorkspaceInfo } from "@/ipc";
import { registerTerminalFeature } from "./index";
import { createSession, resetTerminalsForTest, setAdapterFactory, setPanelVisible, useTerminals } from "./store";
import { fakeAdapterFactory, type FakeAdapter } from "./testing";

const settings = { workbench: { theme: "dark", startupView: "explorer" } } as unknown as Settings;
const info = (root: string, trusted = true): WorkspaceInfo => ({ root, name: root.slice(1), trusted, repoRoot: null, settings });
const flush = () => new Promise((resolve) => setTimeout(resolve, 0));

describe("fitur terminal", () => {
  let adapters: FakeAdapter[];
  let spawned: number;
  let killed: number[];
  let nextWorkspace: WorkspaceInfo;

  beforeAll(() => registerTerminalFeature());

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
      workspaceOpen: () => nextWorkspace,
      appInfo: () => ({ recent: [] }) as unknown as AppInfo,
    });
    const fake = fakeAdapterFactory();
    adapters = fake.adapters;
    setAdapterFactory(fake.factory);
    useApp.setState({ workspace: null, panelVisible: false, panelView: "terminal" });
  });

  afterEach(() => {
    resetTerminalsForTest();
    useApp.setState({ workspace: null, panelVisible: false });
    clearMocks();
  });

  it("mendaftarkan panel dan command dengan keybinding yang diminta", () => {
    const view = getPanelViews().find((v) => v.id === "terminal")!;
    expect(view.title).toBe("Terminal");
    expect(view.order).toBe(10);
    expect(view.actions).toBeDefined();
    expect(keybindingsFor(getCommand("terminal.toggle")!)).toEqual(["Mod+`"]);
    expect(keybindingsFor(getCommand("terminal.new")!)).toEqual(["Mod+Shift+`"]);
    for (const id of ["terminal.kill", "terminal.clear", "terminal.focusNext"]) expect(getCommand(id)?.category).toBe("Terminal");
  });

  it("terminal.toggle menampilkan panel terminal dan memfokuskan terminal aktif, lalu menyembunyikannya", async () => {
    nextWorkspace = info("/a");
    await openFolder("/a");
    setPanelVisible(false);
    await createSession();
    const before = adapters[0].focusCount;

    await executeCommand("terminal.toggle");
    expect(useApp.getState().panelVisible).toBe(true);
    expect(useApp.getState().panelView).toBe("terminal");
    // Komponen panel melaporkan bahwa ia terlihat; barulah fokus diberikan.
    setPanelVisible(true);
    expect(adapters[0].focusCount).toBe(before + 1);

    await executeCommand("terminal.toggle");
    expect(useApp.getState().panelVisible).toBe(false);
  });

  it("terminal.new menampilkan panel dan membuat sesi; terminal.kill menutup yang aktif", async () => {
    nextWorkspace = info("/a");
    await openFolder("/a");
    await executeCommand("terminal.new");
    await executeCommand("terminal.new");
    expect(useApp.getState().panelVisible).toBe(true);
    expect(useTerminals.getState().sessions.map((s) => s.id)).toEqual([1, 2]);
    await executeCommand("terminal.focusNext");
    expect(useTerminals.getState().activeId).toBe(1);
    await executeCommand("terminal.kill");
    expect(killed).toEqual([1]);
    expect(useTerminals.getState().sessions.map((s) => s.id)).toEqual([2]);
  });

  it("mengganti folder mematikan semua sesi; bila panel terlihat, terminal baru dibuat di folder baru", async () => {
    nextWorkspace = info("/a");
    await openFolder("/a");
    await createSession();
    await createSession();
    expect(useTerminals.getState().sessions).toHaveLength(2);

    nextWorkspace = info("/b");
    await openFolder("/b");
    await flush();
    expect(killed).toEqual([1, 2]);
    expect(adapters[0].disposed && adapters[1].disposed).toBe(true);
    // Panel tersembunyi: tidak ada terminal baru.
    expect(useTerminals.getState().sessions).toEqual([]);

    useApp.setState({ panelVisible: true, panelView: "terminal" });
    await createSession();
    nextWorkspace = info("/c");
    await openFolder("/c");
    await flush();
    await flush();
    expect(killed).toEqual([1, 2, 3]);
    expect(useTerminals.getState().sessions.map((s) => s.id)).toEqual([4]);
  });
});
