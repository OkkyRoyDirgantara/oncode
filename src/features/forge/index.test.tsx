import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { clearMocks } from "@tauri-apps/api/mocks";
import { mockCommands, type AppInfo } from "@/ipc";
import { executeCommand, getCommand, resetCommandsForTest } from "@/app/commands";
import { resetOverlayForTest } from "@/app/overlay";
import { getSidebarViews, getTabType } from "@/app/registry";
import { useApp } from "@/app/store";
import { resetScmForTest } from "@/features/scm/store";
import { makeWorkspace, ROOT } from "./fixtures";
import { registerForgeFeature } from "./index";
import { resetForgeForTest } from "./store";

vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn(async () => undefined) }));

const info: AppInfo = { version: "0.1.0", platform: "linux", git: { path: "/usr/bin/git", version: "2.45.0", supported: true }, gitError: null, recent: [], settingsPath: "/cfg/settings.json", keybindingsPath: "/config/keybindings.json", homeDir: "/home/me" };

describe("registerForgeFeature", () => {
  beforeEach(() => {
    resetCommandsForTest();
    resetForgeForTest();
    resetOverlayForTest();
    resetScmForTest();
  });
  afterEach(() => {
    clearMocks();
    useApp.setState({ info: null, workspace: null });
  });

  it("mendaftarkan tampilan sidebar, jenis tab, dan semua command", () => {
    mockCommands({ forgeAccounts: () => [] });
    registerForgeFeature();
    const view = getSidebarViews().find((v) => v.id === "pullRequests");
    expect(view).toMatchObject({ title: "Pull Requests", order: 50 });
    expect(getTabType("pullRequest")).toBeDefined();
    const ids = ["forge.signIn", "forge.signOut", "forge.refresh", "forge.openInBrowser", "forge.createPullRequest", "git.clone", "git.init"];
    for (const id of ids) expect(getCommand(id), id).toBeDefined();
    expect(getCommand("forge.createPullRequest")).toMatchObject({ category: "Pull Requests", title: "Create Pull Request…" });
    expect(getCommand("git.clone")?.category).toBe("Git");
    expect(getCommand("git.init")).toMatchObject({ category: "Git", title: "Initialize Repository" });
  });

  it("git.init: menginisialisasi folder proyek lalu membuka ulang workspace", async () => {
    const calls: string[] = [];
    const reopened = makeWorkspace({ repoRoot: ROOT });
    mockCommands({
      forgeAccounts: () => [],
      gitInit: ({ path }) => {
        calls.push(`init:${path}`);
        return null;
      },
      workspaceOpen: ({ path }) => {
        calls.push(`open:${path}`);
        return reopened;
      },
      appInfo: () => info,
      gitSnapshot: () => {
        throw { code: "x", message: "x", detail: null };
      },
    });
    registerForgeFeature();
    useApp.setState({ info, workspace: makeWorkspace({ repoRoot: null }) });
    await executeCommand("git.init");
    expect(calls).toEqual([`init:${ROOT}`, `open:${ROOT}`]);
    expect(useApp.getState().workspace?.repoRoot).toBe(ROOT);
  });

  it("git.init tidak berjalan di mode terbatas atau bila folder sudah repositori", async () => {
    const gitInit = vi.fn(() => null);
    mockCommands({ forgeAccounts: () => [], gitInit });
    registerForgeFeature();
    useApp.setState({ info, workspace: makeWorkspace({ trusted: false, repoRoot: null }) });
    await executeCommand("git.init");
    useApp.setState({ workspace: makeWorkspace({ repoRoot: ROOT }) });
    await executeCommand("git.init");
    expect(gitInit).not.toHaveBeenCalled();
  });
});
