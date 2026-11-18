import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { commandForKeybinding, executeCommand, getCommand, resetCommandsForTest } from "@/app/commands";
import { resetOverlayForTest } from "@/app/overlay";
import { getTabType } from "@/app/registry";
import { useApp } from "@/app/store";
import { resetWorkbenchForTest, useWorkbench } from "@/app/workbench";
import type { WorkspaceInfo } from "@/ipc";
import { cloneSettings, defaults } from "./fixtures";
import { registerSettingsFeature } from "./index";

const workspace: WorkspaceInfo = { root: "/work/project", name: "project", trusted: true, repoRoot: null, settings: cloneSettings(defaults) };

describe("registerSettingsFeature", () => {
  beforeEach(() => {
    resetCommandsForTest();
    resetWorkbenchForTest();
    resetOverlayForTest();
    registerSettingsFeature();
  });
  afterEach(() => useApp.setState({ workspace: null }));

  it("mendaftarkan tab settings dan command settings.open dengan Mod+,", () => {
    expect(getTabType("settings")).toBeDefined();
    expect(getCommand("settings.open")).toMatchObject({ title: "Open Settings", category: "Preferences", keybinding: "Mod+," });
    expect(commandForKeybinding("Mod+,")?.id).toBe("settings.open");
  });

  it("settings.open membuka satu tab dengan id tetap, tidak menggandakannya", async () => {
    useApp.setState({ workspace });
    await executeCommand("settings.open");
    await executeCommand("settings.open");
    const tabs = useWorkbench.getState().groups[0].tabs;
    expect(tabs).toHaveLength(1);
    expect(tabs[0]).toMatchObject({ id: "settings", kind: "settings", title: "Settings" });
  });

  it("tanpa folder terbuka pun tab pengaturan tetap dibuka", async () => {
    expect(useApp.getState().workspace).toBeNull();
    await executeCommand("settings.open");
    const tabs = useWorkbench.getState().groups[0].tabs;
    expect(tabs).toHaveLength(1);
    expect(tabs[0]).toMatchObject({ id: "settings", kind: "settings" });
  });

  it("settings.openJson membuka berkas pengaturan, juga tanpa folder", async () => {
    useApp.setState({ info: { version: "0", platform: "linux", git: null, gitError: null, recent: [], settingsPath: "/cfg/settings.json", keybindingsPath: "/cfg/keybindings.json", homeDir: null } });
    await executeCommand("settings.openJson");
    expect(useWorkbench.getState().groups[0].tabs[0]).toMatchObject({ kind: "file", params: { path: "/cfg/settings.json" } });
    useApp.setState({ info: null });
  });
});
