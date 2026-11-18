import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen } from "@testing-library/react";
import type { Channel } from "@tauri-apps/api/core";
import { clearMocks } from "@tauri-apps/api/mocks";
import { open as pickNative } from "@tauri-apps/plugin-dialog";
import { mockCommands, type AppInfo, type Page, type Progress, type RemoteRepo } from "@/ipc";
import { OverlayHost, resetOverlayForTest } from "@/app/overlay";
import { useApp } from "@/app/store";
import { CloneDialog, cloneRepository, REPO_SEARCH_DEBOUNCE_MS } from "./CloneDialog";
import { appError, githubAccount, makeRemoteRepo } from "./fixtures";
import { resetForgeForTest } from "./store";

vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn(async () => "/picked/folder") }));

const info: AppInfo = { version: "0.1.0", platform: "linux", git: { path: "/usr/bin/git", version: "2.45.0", supported: true }, gitError: null, recent: [], settingsPath: "/cfg/settings.json", keybindingsPath: "/config/keybindings.json", homeDir: "/home/me" };

type CloneArgs = { url: string; parentDir: string; name: string; recurseSubmodules: boolean; operationId: string; onProgress: Channel<Progress> };

async function flush(ms = 0) {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(ms);
  });
}

describe("CloneDialog", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    resetForgeForTest();
    resetOverlayForTest();
    window.localStorage.clear();
    useApp.setState({ info, workspace: null });
    vi.mocked(pickNative).mockClear();
  });
  afterEach(() => {
    vi.useRealTimers();
    clearMocks();
    useApp.setState({ info: null });
  });

  it("nama folder diturunkan dari URL sampai disunting sendiri, lalu gitClone dipanggil dengan isian itu", async () => {
    const gitClone = vi.fn((args: CloneArgs) => `${args.parentDir}/${args.name}`);
    mockCommands({ forgeAccounts: () => [], gitClone });
    const close = vi.fn();
    render(<CloneDialog close={close} />);
    await flush();
    // Tanpa akun, tidak ada daftar repositori.
    expect(screen.queryByLabelText("Search repositories")).toBeNull();
    const url = screen.getByLabelText("Repository URL");
    const name = screen.getByLabelText("Folder name") as HTMLInputElement;
    expect((screen.getByLabelText("Destination") as HTMLInputElement).value).toBe("/home/me");

    fireEvent.change(url, { target: { value: "git@github.com:acme/widgets.git" } });
    expect(name.value).toBe("widgets");
    fireEvent.change(url, { target: { value: "https://gitlab.com/group/sub/app/" } });
    expect(name.value).toBe("app");

    fireEvent.change(name, { target: { value: "my-copy" } });
    fireEvent.change(url, { target: { value: "https://github.com/acme/other.git" } });
    expect(name.value).toBe("my-copy");

    fireEvent.click(screen.getByRole("button", { name: /Browse/ }));
    await flush();
    expect((screen.getByLabelText("Destination") as HTMLInputElement).value).toBe("/picked/folder");
    expect(screen.getByText("Clones into /picked/folder/my-copy")).toBeInTheDocument();
    fireEvent.click(screen.getByLabelText("Clone submodules"));
    fireEvent.click(screen.getByRole("button", { name: "Clone" }));
    await flush();

    expect(gitClone).toHaveBeenCalledTimes(1);
    expect(gitClone.mock.calls[0][0]).toMatchObject({ url: "https://github.com/acme/other.git", parentDir: "/picked/folder", name: "my-copy", recurseSubmodules: true });
    expect(gitClone.mock.calls[0][0].operationId).toMatch(/^op-/);
    expect(close).toHaveBeenCalledWith("/picked/folder/my-copy");
    // Folder tujuan diingat untuk clone berikutnya.
    expect(window.localStorage.getItem("oncode.clone.parentDir")).toBe("/picked/folder");
  });

  it("masukan tidak sah tidak memanggil gitClone", async () => {
    const gitClone = vi.fn(() => "/x");
    mockCommands({ forgeAccounts: () => [], gitClone });
    render(<CloneDialog close={vi.fn()} />);
    await flush();
    fireEvent.click(screen.getByRole("button", { name: "Clone" }));
    expect(screen.getByText("Enter a repository URL")).toBeInTheDocument();
    expect(screen.getByText("Enter a folder name")).toBeInTheDocument();

    fireEvent.change(screen.getByLabelText("Repository URL"), { target: { value: "https://github.com/acme/widgets.git" } });
    fireEvent.change(screen.getByLabelText("Folder name"), { target: { value: "../escape" } });
    expect(screen.getByText("The name cannot contain a path separator")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Clone" })).toBeDisabled();
    fireEvent.submit(screen.getByLabelText("Folder name").closest("form")!);
    await flush();
    expect(gitClone).not.toHaveBeenCalled();
  });

  it("daftar repositori akun: pencarian di-debounce, Load more menambah, memilih mengisi URL HTTPS", async () => {
    const forgeRepos = vi.fn(({ query, page }: { accountId: string; query: string; page: number }): Page<RemoteRepo> => {
      if (query === "wid") return { items: [makeRemoteRepo("acme/widgets", { private: true })], nextPage: null };
      return page === 1 ? { items: [makeRemoteRepo("acme/alpha"), makeRemoteRepo("acme/beta")], nextPage: 2 } : { items: [makeRemoteRepo("acme/gamma")], nextPage: null };
    });
    mockCommands({ forgeAccounts: () => [githubAccount], forgeRepos, gitClone: () => "/x" });
    render(<CloneDialog close={vi.fn()} />);
    await flush();
    await flush();
    expect(forgeRepos).toHaveBeenCalledTimes(1);
    expect(forgeRepos).toHaveBeenLastCalledWith({ accountId: githubAccount.id, query: "", page: 1 });
    expect(screen.getAllByRole("option").map((o) => o.querySelector(".fg-repo-name span")?.textContent)).toEqual(["acme/alpha", "acme/beta"]);

    fireEvent.click(screen.getByRole("button", { name: "Load more" }));
    await flush();
    expect(forgeRepos).toHaveBeenLastCalledWith({ accountId: githubAccount.id, query: "", page: 2 });
    expect(screen.getAllByRole("option")).toHaveLength(3);
    expect(screen.queryByRole("button", { name: "Load more" })).toBeNull();

    // Ketikan beruntun hanya menghasilkan satu permintaan setelah jeda.
    const search = screen.getByLabelText("Search repositories");
    fireEvent.change(search, { target: { value: "w" } });
    await flush(REPO_SEARCH_DEBOUNCE_MS - 50);
    fireEvent.change(search, { target: { value: "wi" } });
    await flush(REPO_SEARCH_DEBOUNCE_MS - 50);
    fireEvent.change(search, { target: { value: "wid" } });
    await flush(REPO_SEARCH_DEBOUNCE_MS - 1);
    expect(forgeRepos).toHaveBeenCalledTimes(2);
    await flush(1);
    expect(forgeRepos).toHaveBeenCalledTimes(3);
    expect(forgeRepos).toHaveBeenLastCalledWith({ accountId: githubAccount.id, query: "wid", page: 1 });
    expect(screen.getAllByRole("option")).toHaveLength(1);

    fireEvent.click(screen.getByRole("option"));
    expect((screen.getByLabelText("Repository URL") as HTMLInputElement).value).toBe("https://github.com/acme/widgets.git");
    expect((screen.getByLabelText("Folder name") as HTMLInputElement).value).toBe("widgets");
  });

  it("menampilkan progres dan Cancel memanggil appCancel dengan id operasi yang sama", async () => {
    let finish!: (value: string) => void;
    let fail!: (error: unknown) => void;
    let channel!: Channel<Progress>;
    const gitClone = vi.fn((args: CloneArgs) => {
      channel = args.onProgress;
      return new Promise<string>((resolve, reject) => {
        finish = resolve;
        fail = reject;
      });
    });
    const appCancel = vi.fn((_args: { operationId: string }) => null);
    mockCommands({ forgeAccounts: () => [], gitClone, appCancel });
    const close = vi.fn();
    render(
      <>
        <CloneDialog close={close} initialUrl="https://github.com/acme/widgets.git" />
        <OverlayHost />
      </>,
    );
    await flush();
    expect((screen.getByLabelText("Folder name") as HTMLInputElement).value).toBe("widgets");
    fireEvent.click(screen.getByRole("button", { name: "Clone" }));
    await flush();
    expect(screen.getByText("Starting clone")).toBeInTheDocument();
    expect(screen.getByLabelText("Repository URL")).toBeDisabled();

    act(() => channel.onmessage({ phase: "Receiving objects", percent: 42, text: "Receiving objects:  42% (420/1000)" }));
    expect(screen.getByText("Receiving objects")).toBeInTheDocument();
    expect(screen.getByText("42%")).toBeInTheDocument();
    expect(screen.getByRole("progressbar")).toHaveAttribute("aria-valuenow", "42");

    fireEvent.click(screen.getByRole("button", { name: "Cancel Clone" }));
    await flush();
    expect(appCancel).toHaveBeenCalledWith({ operationId: gitClone.mock.calls[0][0].operationId });
    expect(screen.getByText("Cancelling…")).toBeInTheDocument();

    await act(async () => {
      fail(appError("cancelled", "dibatalkan"));
      await vi.advanceTimersByTimeAsync(0);
    });
    expect(close).not.toHaveBeenCalled();
    expect(screen.getByText("Clone cancelled.")).toBeInTheDocument();
    // Kembali ke formulir dengan isian yang sama.
    expect(screen.getByLabelText("Repository URL")).toBeEnabled();
    expect((screen.getByLabelText("Repository URL") as HTMLInputElement).value).toBe("https://github.com/acme/widgets.git");
    void finish;
  });

  it("folder tujuan yang sudah ada ditampilkan sebagai galat di formulir", async () => {
    mockCommands({
      forgeAccounts: () => [],
      gitClone: () => {
        throw appError("alreadyExists", "Folder tujuan sudah ada dan tidak kosong: /home/me/widgets");
      },
    });
    const close = vi.fn();
    render(<CloneDialog close={close} initialUrl="https://github.com/acme/widgets.git" />);
    await flush();
    fireEvent.click(screen.getByRole("button", { name: "Clone" }));
    await flush();
    expect(screen.getByRole("alert")).toHaveTextContent("/home/me/widgets already exists and is not empty");
    expect(close).not.toHaveBeenCalled();
  });

  it("setelah clone: Open in New Window memanggil windowOpen dengan folder hasil", async () => {
    const windowOpen = vi.fn((_args: { folder: string | null }) => null);
    mockCommands({ forgeAccounts: () => [], gitClone: (args: CloneArgs) => `${args.parentDir}/${args.name}`, windowOpen });
    render(<OverlayHost />);
    let result: string | null | undefined;
    act(() => {
      void cloneRepository("https://github.com/acme/widgets.git").then((r) => (result = r));
    });
    await flush();
    fireEvent.click(screen.getByRole("button", { name: "Clone" }));
    await flush();
    expect(screen.getByText("Repository cloned")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Open" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Open in New Window" }));
    await flush();
    expect(windowOpen).toHaveBeenCalledWith({ folder: "/home/me/widgets" });
    expect(result).toBe("/home/me/widgets");
  });
});
