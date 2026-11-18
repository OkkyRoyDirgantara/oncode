import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { clearMocks } from "@tauri-apps/api/mocks";
import { openUrl } from "@tauri-apps/plugin-opener";
import { mockCommands, type ForgeContext, type NewPullRequest, type PullRequest, type PushOptions, type RepoRef, type RepoSnapshot } from "@/ipc";
import { OverlayHost, resetOverlayForTest } from "@/app/overlay";
import { useApp } from "@/app/store";
import { resetScmForTest } from "@/features/scm/store";
import { createPullRequest, pushNeed, sourceBranchName, targetBranchOptions } from "./CreatePrDialog";
import { appError, githubAccount, gitlabAccount, makeContext, makePr, makeRefs, makeRepoState, makeSnapshot, makeWorkspace, ROOT } from "./fixtures";
import { resetForgeForTest } from "./store";

vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn(async () => undefined) }));

interface Options {
  snapshot?: Partial<RepoSnapshot>;
  context?: ForgeContext | null;
  defaultBranch?: string | (() => string);
  push?: () => null;
  create?: (input: NewPullRequest) => PullRequest;
}

function setup(options: Options = {}) {
  resetScmForTest({ [ROOT]: makeRepoState({ snapshot: makeSnapshot(options.snapshot) }) }, ROOT);
  const calls: string[] = [];
  const gitPush = vi.fn((_args: { root: string; options: PushOptions; operationId: string }) => {
    calls.push("push");
    return (options.push ?? (() => null))();
  });
  const forgePrCreate = vi.fn((args: { accountId: string; repo: RepoRef; input: NewPullRequest }) => {
    calls.push("create");
    return (options.create ?? (() => makePr(42, { webUrl: "https://github.com/acme/widgets/pull/42" })))(args.input);
  });
  const forgePrs = vi.fn(() => ({ items: [], nextPage: null }));
  mockCommands({
    forgeContext: () => (options.context === undefined ? makeContext() : options.context),
    forgeDefaultBranch: () => (typeof options.defaultBranch === "function" ? options.defaultBranch() : (options.defaultBranch ?? "main")),
    forgeAccounts: () => [githubAccount],
    gitPush,
    forgePrCreate,
    forgePrs,
  });
  return { calls, gitPush, forgePrCreate, forgePrs };
}

async function openCreateDialog() {
  render(<OverlayHost />);
  const box: { result?: PullRequest | null } = {};
  await act(async () => {
    void createPullRequest().then((r) => (box.result = r));
  });
  return box;
}

const submitButton = () => screen.getByRole("button", { name: /Create Pull Request$|Create Merge Request$/ });

describe("createPullRequest", () => {
  beforeEach(() => {
    resetForgeForTest();
    resetOverlayForTest();
    useApp.setState({ workspace: makeWorkspace() });
    vi.mocked(openUrl).mockClear();
  });
  afterEach(() => {
    clearMocks();
    useApp.setState({ workspace: null });
    resetScmForTest();
  });

  it("branch belum dipublikasikan: push dengan setUpstream lebih dulu, lalu membuat PR dengan isian yang benar", async () => {
    const { calls, gitPush, forgePrCreate } = setup({ snapshot: { upstream: null } });
    const box = await openCreateDialog();
    const title = (await screen.findByLabelText("Title")) as HTMLInputElement;
    // Judul bawaan = subjek commit terakhir branch.
    expect(title.value).toBe("Add login form");
    expect(screen.getByText(/has not been published yet/)).toBeInTheDocument();
    await waitFor(() => expect((screen.getByLabelText("Target branch") as HTMLSelectElement).value).toBe("main"));
    // Branch tujuan: branch remote tanpa awalan, tanpa HEAD dan tanpa branch sumber.
    expect([...(screen.getByLabelText("Target branch") as HTMLSelectElement).options].map((o) => o.value)).toEqual(["develop", "main"]);

    fireEvent.change(title, { target: { value: "  Add the login form  " } });
    fireEvent.change(screen.getByLabelText("Description (Markdown)"), { target: { value: "Closes #7\n\n- form\n- tests" } });
    fireEvent.click(screen.getByLabelText("Create as draft"));
    fireEvent.click(submitButton());

    await waitFor(() => expect(box.result).toBeDefined());
    expect(calls).toEqual(["push", "create"]);
    expect(gitPush).toHaveBeenCalledTimes(1);
    expect(gitPush.mock.calls[0][0]).toMatchObject({ root: ROOT, options: { setUpstream: true, remote: "origin", forceWithLease: false } });
    expect(forgePrCreate).toHaveBeenCalledWith({
      accountId: githubAccount.id,
      repo: { host: "github.com", owner: "acme", name: "widgets" },
      input: { title: "Add the login form", body: "Closes #7\n\n- form\n- tests", sourceBranch: "feature/login", targetBranch: "main", draft: true },
    });
    expect(box.result?.number).toBe(42);
    // Toast sukses dengan aksi membuka browser.
    expect(screen.getByText("Pull Request #42 created.")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Open in browser" }));
    await waitFor(() => expect(openUrl).toHaveBeenCalledWith("https://github.com/acme/widgets/pull/42"));
  });

  it("branch sudah sinkron: tidak ada push; branch tujuan bisa diganti", async () => {
    const { calls, gitPush, forgePrCreate } = setup({ snapshot: { upstream: { name: "origin/feature/login", ahead: 0, behind: 0, exists: true } } });
    const box = await openCreateDialog();
    await screen.findByLabelText("Title");
    expect(screen.queryByText(/has not been published yet/)).toBeNull();
    expect(screen.queryByText(/not on the remote yet/)).toBeNull();
    await waitFor(() => expect((screen.getByLabelText("Target branch") as HTMLSelectElement).value).toBe("main"));
    fireEvent.change(screen.getByLabelText("Target branch"), { target: { value: "develop" } });
    fireEvent.click(submitButton());
    await waitFor(() => expect(box.result).toBeDefined());
    expect(calls).toEqual(["create"]);
    expect(gitPush).not.toHaveBeenCalled();
    expect(forgePrCreate.mock.calls[0][0].input).toEqual({ title: "Add login form", body: "", sourceBranch: "feature/login", targetBranch: "develop", draft: false });
  });

  it("branch mendahului upstream: push biasa lebih dulu", async () => {
    const { calls, gitPush } = setup({ snapshot: { upstream: { name: "origin/feature/login", ahead: 2, behind: 0, exists: true } } });
    const box = await openCreateDialog();
    await screen.findByLabelText("Title");
    expect(screen.getByText(/has 2 commits that are not on the remote yet/)).toBeInTheDocument();
    await waitFor(() => expect((screen.getByLabelText("Target branch") as HTMLSelectElement).value).toBe("main"));
    fireEvent.click(screen.getByRole("button", { name: "Push & Create Pull Request" }));
    await waitFor(() => expect(box.result).toBeDefined());
    expect(calls).toEqual(["push", "create"]);
    expect(gitPush.mock.calls[0][0].options).toMatchObject({ setUpstream: false, forceWithLease: false });
  });

  it("push gagal: PR tidak dibuat dan dialog tetap terbuka", async () => {
    const { calls, forgePrCreate } = setup({
      snapshot: { upstream: null },
      push: () => {
        throw appError("network", "tidak bisa terhubung");
      },
    });
    const box = await openCreateDialog();
    await screen.findByLabelText("Title");
    await waitFor(() => expect((screen.getByLabelText("Target branch") as HTMLSelectElement).value).toBe("main"));
    fireEvent.click(screen.getByRole("button", { name: "Publish & Create Pull Request" }));
    expect(await screen.findByText(/could not be pushed, so the pull request was not created/)).toBeInTheDocument();
    expect(calls).toEqual(["push"]);
    expect(forgePrCreate).not.toHaveBeenCalled();
    expect(box.result).toBeUndefined();
    expect(screen.getByLabelText("Title")).toBeInTheDocument();
  });

  it("galat invalidInput (mis. PR sudah ada) ditampilkan di dalam dialog", async () => {
    const { forgePrCreate } = setup({
      snapshot: { upstream: { name: "origin/feature/login", ahead: 0, behind: 0, exists: true } },
      create: () => {
        throw appError("invalidInput", "Permintaan ditolak oleh GitHub. A pull request already exists for acme:feature/login.");
      },
    });
    const box = await openCreateDialog();
    await screen.findByLabelText("Title");
    await waitFor(() => expect((screen.getByLabelText("Target branch") as HTMLSelectElement).value).toBe("main"));
    fireEvent.click(submitButton());
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent(/GitHub rejected this pull request/);
    expect(alert).toHaveTextContent("A pull request already exists for acme:feature/login.");
    expect(forgePrCreate).toHaveBeenCalledTimes(1);
    expect(box.result).toBeUndefined();
    // Isian tidak hilang dan bisa dicoba lagi.
    expect((screen.getByLabelText("Title") as HTMLInputElement).value).toBe("Add login form");
    expect(submitButton()).toBeEnabled();
  });

  it("judul kosong atau tujuan yang sama dengan sumber tidak bisa dikirim", async () => {
    const { forgePrCreate } = setup({ snapshot: { head: { kind: "branch", name: "main" }, upstream: { name: "origin/main", ahead: 0, behind: 0, exists: true } } });
    await openCreateDialog();
    await screen.findByLabelText("Title");
    // Branch bawaan = branch sumber: tidak ada tebakan, tujuan harus dipilih.
    await waitFor(() => expect(screen.getByText("Choose a branch…")).toBeInTheDocument());
    expect(submitButton()).toBeDisabled();
    fireEvent.change(screen.getByLabelText("Target branch"), { target: { value: "develop" } });
    expect(submitButton()).toBeEnabled();
    fireEvent.change(screen.getByLabelText("Title"), { target: { value: "   " } });
    expect(submitButton()).toBeDisabled();
    fireEvent.submit(screen.getByLabelText("Title").closest("form")!);
    expect(forgePrCreate).not.toHaveBeenCalled();
  });

  it("HEAD terlepas: tidak membuka dialog", async () => {
    setup({ snapshot: { head: { kind: "detached", oid: "a".repeat(40) } } });
    const box = await openCreateDialog();
    await waitFor(() => expect(box.result).toBeNull());
    expect(screen.getByText(/You are not on a branch/)).toBeInTheDocument();
    expect(screen.queryByLabelText("Title")).toBeNull();
  });

  it("tanpa akun untuk host itu: menawarkan login, tidak membuka dialog", async () => {
    setup({ context: makeContext({ account: null }) });
    const box = await openCreateDialog();
    await waitFor(() => expect(box.result).toBeNull());
    expect(screen.getByText("Sign in to github.com to create a pull request.")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Sign in" })).toBeInTheDocument();
    expect(screen.queryByLabelText("Title")).toBeNull();
  });

  it("GitLab: istilah Merge Request", async () => {
    setup({ context: makeContext({ kind: "gitlab", account: gitlabAccount, repo: { host: "gitlab.com", owner: "g", name: "app" } }), snapshot: { upstream: { name: "origin/feature/login", ahead: 0, behind: 0, exists: true } } });
    await openCreateDialog();
    await screen.findByLabelText("Title");
    expect(screen.getByText("Create Merge Request", { selector: ".oc-dialog-header" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Create Merge Request" })).toBeInTheDocument();
  });
});

describe("pembantu dialog PR", () => {
  it("pushNeed", () => {
    expect(pushNeed({ upstream: null })).toEqual({ kind: "publish" });
    expect(pushNeed({ upstream: { name: "origin/x", ahead: 0, behind: 0, exists: false } })).toEqual({ kind: "publish" });
    expect(pushNeed({ upstream: { name: "origin/x", ahead: 3, behind: 1, exists: true } })).toEqual({ kind: "push", ahead: 3 });
    expect(pushNeed({ upstream: { name: "origin/x", ahead: 0, behind: 4, exists: true } })).toEqual({ kind: "none" });
  });

  it("sourceBranchName memakai nama branch di remote", () => {
    const refs = makeRefs({ remotes: [{ name: "origin", fetchUrl: "", pushUrl: "" }, { name: "origin/fork", fetchUrl: "", pushUrl: "" }] });
    expect(sourceBranchName("local", { upstream: null }, refs)).toBe("local");
    expect(sourceBranchName("local", { upstream: { name: "origin/feature/x", ahead: 0, behind: 0, exists: true } }, refs)).toBe("feature/x");
    expect(sourceBranchName("local", { upstream: { name: "origin/fork/topic", ahead: 0, behind: 0, exists: true } }, refs)).toBe("topic");
    expect(sourceBranchName("local", { upstream: { name: "origin/gone", ahead: 0, behind: 0, exists: false } }, refs)).toBe("local");
  });

  it("targetBranchOptions hanya dari remote yang bersangkutan, tanpa HEAD", () => {
    const refs = makeRefs({
      remoteBranches: [
        { name: "origin/HEAD", remote: "origin", oid: "" },
        { name: "origin/main", remote: "origin", oid: "" },
        { name: "origin/release/1.0", remote: "origin", oid: "" },
        { name: "upstream/next", remote: "upstream", oid: "" },
      ],
    });
    expect(targetBranchOptions(refs, "origin")).toEqual(["main", "release/1.0"]);
    expect(targetBranchOptions(refs, "origin", ["trunk", ""])).toEqual(["main", "release/1.0", "trunk"]);
    expect(targetBranchOptions(null, "origin", ["main"])).toEqual(["main"]);
  });
});
