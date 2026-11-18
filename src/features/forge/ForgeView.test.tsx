import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { clearMocks } from "@tauri-apps/api/mocks";
import { openUrl } from "@tauri-apps/plugin-opener";
import { events, mockCommands, type Account, type ForgeContext, type Page, type PrFilter, type PullRequest, type RepoRef } from "@/ipc";
import { OverlayHost, resetOverlayForTest } from "@/app/overlay";
import { useApp } from "@/app/store";
import { resetWorkbenchForTest, useWorkbench } from "@/app/workbench";
import { resetScmForTest } from "@/features/scm/store";
import { githubAccount, gitlabAccount, makeContext, makePr, makeRefs, makeRepoState, makeWorkspace, ROOT } from "./fixtures";
import { ForgeView } from "./ForgeView";
import { resetForgeForTest } from "./store";

vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn(async () => undefined) }));

type PrsArgs = { accountId: string; repo: RepoRef; filter: PrFilter; page: number };

function setup(options: { accounts?: Account[]; context?: ForgeContext | null; prs?: (args: PrsArgs) => Page<PullRequest> | Promise<Page<PullRequest>> } = {}) {
  const state = { accounts: options.accounts ?? [githubAccount], context: options.context === undefined ? makeContext() : options.context };
  const forgePrs = vi.fn(options.prs ?? ((): Page<PullRequest> => ({ items: [makePr(12), makePr(11)], nextPage: null })));
  const forgeContext = vi.fn((_args: { root: string }) => state.context);
  const forgeLoginStart = vi.fn((_args: { kind: string; host: string | null }) => ({ userCode: "AAAA-BBBB", verificationUri: "https://github.com/login/device", verificationUriComplete: null, interval: 5, expiresIn: 900 }));
  const forgeLogout = vi.fn((_args: { accountId: string }) => null);
  mockCommands({
    forgeAccounts: () => state.accounts,
    forgeContext,
    forgePrs,
    forgeLoginStart,
    forgeLoginPoll: () => ({ status: "pending" }),
    forgeLoginCancel: () => null,
    forgeLogout,
  });
  return { state, forgePrs, forgeContext, forgeLoginStart, forgeLogout };
}

function renderView() {
  return render(
    <>
      <ForgeView />
      <OverlayHost />
    </>,
  );
}

describe("ForgeView", () => {
  beforeEach(() => {
    resetForgeForTest();
    resetOverlayForTest();
    resetWorkbenchForTest();
    resetScmForTest({ [ROOT]: makeRepoState() }, ROOT);
    useApp.setState({ workspace: makeWorkspace() });
    vi.mocked(openUrl).mockClear();
  });
  afterEach(() => {
    clearMocks();
    useApp.setState({ workspace: null });
    resetScmForTest();
  });

  it("tanpa repositori aktif: tidak ada permintaan ke forge", async () => {
    resetScmForTest();
    const { forgeContext, forgePrs } = setup();
    renderView();
    expect(await screen.findByText("Open a Git repository to see its pull requests.")).toBeInTheDocument();
    expect(forgeContext).not.toHaveBeenCalled();
    expect(forgePrs).not.toHaveBeenCalled();
  });

  it("repositori tanpa remote forge yang dikenali", async () => {
    const { forgePrs } = setup({ context: null });
    renderView();
    expect(await screen.findByText("No remote of this repository points to GitHub or GitLab.")).toBeInTheDocument();
    expect(forgePrs).not.toHaveBeenCalled();
  });

  it("forge dikenali tetapi belum login: menawarkan login untuk host itu", async () => {
    const { forgePrs, forgeLoginStart } = setup({ accounts: [], context: makeContext({ account: null }) });
    renderView();
    const button = await screen.findByRole("button", { name: "Sign in to github.com" });
    expect(screen.getByText("No account is signed in.")).toBeInTheDocument();
    expect(forgePrs).not.toHaveBeenCalled();
    expect(screen.queryByLabelText("Filter")).toBeNull();

    fireEvent.click(button);
    await waitFor(() => expect(forgeLoginStart).toHaveBeenCalledWith({ kind: "github", host: null }));
    expect(await screen.findByLabelText("Sign-in code")).toHaveTextContent("AAAA-BBBB");
  });

  it("host self-hosted: login memakai origin host tersebut", async () => {
    const { forgeLoginStart } = setup({
      accounts: [],
      context: makeContext({ account: null, kind: "gitlab", repo: { host: "gitlab.example.com", owner: "team", name: "app" }, webUrl: "https://gitlab.example.com/team/app" }),
    });
    renderView();
    fireEvent.click(await screen.findByRole("button", { name: "Sign in to gitlab.example.com" }));
    await waitFor(() => expect(forgeLoginStart).toHaveBeenCalledWith({ kind: "gitlab", host: "https://gitlab.example.com" }));
    expect(screen.getByText(/Sign in to see its merge requests/)).toBeInTheDocument();
  });

  it("siap: menampilkan baris PR dengan nomor, judul, penulis, branch, dan label draft", async () => {
    const { forgePrs } = setup({
      prs: () => ({ items: [makePr(12, { title: "Add login form", author: "alice", sourceBranch: "feature/login" }), makePr(9, { title: "WIP: refactor", draft: true, author: "bob" })], nextPage: null }),
    });
    renderView();
    const first = (await screen.findByText("Add login form")).closest(".fg-pr") as HTMLElement;
    expect(forgePrs).toHaveBeenCalledWith({ accountId: githubAccount.id, repo: { host: "github.com", owner: "acme", name: "widgets" }, filter: "open", page: 1 });
    expect(within(first).getByText("#12")).toBeInTheDocument();
    expect(within(first).getByText("alice")).toBeInTheDocument();
    expect(within(first).getByText("feature/login → main")).toBeInTheDocument();
    expect(within(first).getByText("1 hour ago")).toBeInTheDocument();
    expect(within(first).queryByText("Draft")).toBeNull();
    const second = screen.getByText("WIP: refactor").closest(".fg-pr") as HTMLElement;
    expect(within(second).getByText("Draft")).toBeInTheDocument();
    expect(within(second).getByLabelText("Draft")).toHaveAttribute("data-state", "draft");
    // Akun ditampilkan dengan avatar https tanpa referrer.
    const avatar = document.querySelector("img.fg-avatar") as HTMLImageElement;
    expect(avatar.getAttribute("src")).toBe("https://avatars.example/u/1");
    expect(avatar.getAttribute("referrerpolicy")).toBe("no-referrer");
    expect(screen.queryByRole("button", { name: "Load more" })).toBeNull();
  });

  it("Load more menambahkan halaman berikutnya dan tetap ditawarkan selama nextPage ada", async () => {
    const pages: Record<number, Page<PullRequest>> = {
      1: { items: [makePr(30), makePr(29)], nextPage: 2 },
      // Halaman kosong dengan nextPage tetap terisi: tombol harus tetap ada.
      2: { items: [], nextPage: 3 },
      3: { items: [makePr(29), makePr(28)], nextPage: null },
    };
    const { forgePrs } = setup({ prs: ({ page }) => pages[page] });
    renderView();
    await screen.findByText("Change number 30");
    expect(document.querySelectorAll(".fg-pr")).toHaveLength(2);

    fireEvent.click(screen.getByRole("button", { name: "Load more" }));
    await waitFor(() => expect(forgePrs).toHaveBeenCalledTimes(2));
    expect(forgePrs.mock.calls[1][0].page).toBe(2);
    await waitFor(() => expect(screen.getByRole("button", { name: "Load more" })).toBeEnabled());
    expect(document.querySelectorAll(".fg-pr")).toHaveLength(2);

    fireEvent.click(screen.getByRole("button", { name: "Load more" }));
    await screen.findByText("Change number 28");
    expect(forgePrs.mock.calls[2][0].page).toBe(3);
    // #29 sudah ada; tidak digandakan.
    expect([...document.querySelectorAll(".fg-pr .fg-pr-title")].map((el) => el.textContent)).toEqual(["#30 Change number 30", "#29 Change number 29", "#28 Change number 28"]);
    expect(screen.queryByRole("button", { name: "Load more" })).toBeNull();
  });

  it("mengganti filter memuat ulang dari halaman pertama dengan filter itu", async () => {
    const { forgePrs } = setup({ prs: ({ filter }) => ({ items: filter === "mine" ? [makePr(5, { title: "Mine" })] : [makePr(12)], nextPage: null }) });
    renderView();
    await screen.findByText("Change number 12");
    fireEvent.change(screen.getByLabelText("Filter"), { target: { value: "mine" } });
    await screen.findByText("Mine");
    expect(forgePrs).toHaveBeenLastCalledWith(expect.objectContaining({ filter: "mine", page: 1 }));
    expect(screen.queryByText("Change number 12")).toBeNull();
    expect([...(screen.getByLabelText("Filter") as HTMLSelectElement).options].map((o) => o.textContent)).toEqual(["Open", "Created by me", "Review requested", "All"]);
  });

  it("GitLab memakai istilah Merge Request", async () => {
    setup({
      accounts: [gitlabAccount],
      context: makeContext({ kind: "gitlab", account: gitlabAccount, repo: { host: "gitlab.com", owner: "group/sub", name: "app" }, webUrl: "https://gitlab.com/group/sub/app" }),
      prs: () => ({ items: [makePr(7)], nextPage: null }),
    });
    renderView();
    await screen.findByText("Change number 7");
    expect(screen.getByText("!7")).toBeInTheDocument();
    expect(screen.getByText("Merge Requests")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Create Merge Request…/ })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: /Open on GitLab/ }));
    expect(openUrl).toHaveBeenCalledWith("https://gitlab.com/group/sub/app");
  });

  it("galat daftar PR ditampilkan dengan Retry", async () => {
    let fail = true;
    setup({
      prs: () => {
        if (fail) throw { code: "rateLimited", message: "Batas laju API terlampaui", detail: null };
        return { items: [makePr(3)], nextPage: null };
      },
    });
    renderView();
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent(/rate limit of github\.com was reached/);
    fail = false;
    fireEvent.click(within(alert).getByRole("button", { name: "Retry" }));
    expect(await screen.findByText("Change number 3")).toBeInTheDocument();
  });

  it("perubahan daftar akun me-resolve ulang konteks dan memuat PR", async () => {
    const { state, forgeContext, forgePrs } = setup({ accounts: [], context: makeContext({ account: null }) });
    renderView();
    await screen.findByRole("button", { name: "Sign in to github.com" });
    expect(forgePrs).not.toHaveBeenCalled();
    const before = forgeContext.mock.calls.length;

    state.accounts = [githubAccount];
    state.context = makeContext();
    await act(async () => {
      await events.accountsChanged.emit({});
    });
    expect(await screen.findByText("Change number 12")).toBeInTheDocument();
    expect(forgeContext.mock.calls.length).toBeGreaterThan(before);
    expect(screen.getByText("octocat")).toBeInTheDocument();
  });

  it("perubahan remote repositori me-resolve ulang konteks", async () => {
    const { state, forgeContext } = setup();
    renderView();
    await screen.findByText("Change number 12");
    const before = forgeContext.mock.calls.length;
    state.context = null;
    act(() => resetScmForTest({ [ROOT]: makeRepoState({ refs: makeRefs({ remotes: [{ name: "origin", fetchUrl: "https://git.internal/x.git", pushUrl: "https://git.internal/x.git" }] }) }) }, ROOT));
    expect(await screen.findByText("No remote of this repository points to GitHub or GitLab.")).toBeInTheDocument();
    expect(forgeContext.mock.calls.length).toBe(before + 1);
    expect(screen.queryByText("Change number 12")).toBeNull();
  });

  it("klik baris membuka tab pullRequest dengan parameter lengkap", async () => {
    setup({ prs: () => ({ items: [makePr(12, { title: "Add login form" })], nextPage: null }) });
    renderView();
    fireEvent.doubleClick(await screen.findByText("Add login form"));
    const tabs = useWorkbench.getState().groups[0].tabs;
    expect(tabs).toHaveLength(1);
    expect(tabs[0]).toMatchObject({
      kind: "pullRequest",
      title: "#12 Add login form",
      params: { root: ROOT, accountId: githubAccount.id, repo: { host: "github.com", owner: "acme", name: "widgets" }, kind: "github", remote: "origin", number: 12 },
    });
  });

  it("Sign Out meminta konfirmasi sebelum memanggil forgeLogout", async () => {
    const { forgeLogout } = setup();
    renderView();
    fireEvent.click(await screen.findByRole("button", { name: "Sign out octocat" }));
    const dialog = await screen.findByRole("dialog");
    expect(forgeLogout).not.toHaveBeenCalled();
    fireEvent.click(within(dialog).getByRole("button", { name: "Sign Out" }));
    await waitFor(() => expect(forgeLogout).toHaveBeenCalledWith({ accountId: githubAccount.id }));
  });
});
