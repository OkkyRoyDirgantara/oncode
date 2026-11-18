import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { clearMocks } from "@tauri-apps/api/mocks";
import { openUrl } from "@tauri-apps/plugin-opener";
import { mockCommands, type Check, type CommitFile, type PullRequestDetail } from "@/ipc";
import { OverlayHost, resetOverlayForTest } from "@/app/overlay";
import type { Tab } from "@/app/registry";
import { resetWorkbenchForTest, useWorkbench } from "@/app/workbench";
import { resetScmForTest } from "@/features/scm/store";
import { githubAccount, gitlabAccount, makeBranch, makePr, makeRefs, makeRepoState, ROOT } from "./fixtures";
import type { PrTabParams } from "./ForgeView";
import { baseRefFor, PullRequestTab, summarizeChecks } from "./PrTab";
import { resetForgeForTest, useForge } from "./store";

vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn(async () => undefined) }));

const params: PrTabParams = { root: ROOT, accountId: githubAccount.id, repo: { host: "github.com", owner: "acme", name: "widgets" }, kind: "github", remote: "origin", number: 12 };

function makeDetail(overrides: Partial<PullRequestDetail> = {}): PullRequestDetail {
  return {
    pullRequest: makePr(12, { title: "Add login form", author: "alice", authorAvatar: "http://insecure.example/a.png", sourceBranch: "feature/login", headSha: "f".repeat(40) }),
    body: "Fixes the **login** flow.\n\n<script>window.pwned = 1</script>\n\n[spec](https://example.com/spec) [bad](javascript:alert(1))",
    reviewers: ["bob", "carol"],
    labels: ["bug", "ui"],
    mergeable: true,
    commits: 3,
    changedFiles: 2,
    additions: 40,
    deletions: 7,
    ...overrides,
  };
}

const checks: Check[] = [
  { name: "build", state: "success", url: "https://ci.example/build/1" },
  { name: "lint", state: "failure", url: "javascript:alert(1)" },
  { name: "deploy", state: "running", url: null },
];

const files: CommitFile[] = [
  { path: "src/login.ts", origPath: null, kind: "added", additions: 30, deletions: 0 },
  { path: "src/app.ts", origPath: "src/main.ts", kind: "renamed", additions: 10, deletions: 7 },
];

const MERGE_BASE = "9".repeat(40);

function setup(options: { detail?: PullRequestDetail; tabParams?: PrTabParams; refs?: ReturnType<typeof makeRefs>; mergeBase?: () => string | null } = {}) {
  resetScmForTest({ [ROOT]: makeRepoState({ refs: options.refs ?? makeRefs() }) }, ROOT);
  useForge.setState({ accounts: [githubAccount, gitlabAccount] });
  const forgePr = vi.fn(() => options.detail ?? makeDetail());
  const forgeChecks = vi.fn((_args: { sha: string }) => checks);
  const forgePrCheckout = vi.fn((_args: { root: string; kind: string; remote: string; number: number; operationId: string }) => "pr/12");
  const gitDiffFiles = vi.fn((_args: { root: string; from: string; to: string }) => files);
  const gitMergeBase = vi.fn((_args: { root: string; a: string; b: string }) => (options.mergeBase ?? (() => MERGE_BASE))());
  mockCommands({ forgePr, forgeChecks, forgePrCheckout, gitDiffFiles, gitMergeBase });
  const tab: Tab = { id: "pr", kind: "pullRequest", title: "#12", params: { ...(options.tabParams ?? params) } };
  render(
    <>
      <PullRequestTab tab={tab} groupId="g" visible focused />
      <OverlayHost />
    </>,
  );
  return { forgePr, forgeChecks, forgePrCheckout, gitDiffFiles, gitMergeBase };
}

describe("PullRequestTab", () => {
  beforeEach(() => {
    resetForgeForTest();
    resetOverlayForTest();
    resetWorkbenchForTest();
    vi.mocked(openUrl).mockClear();
    delete (window as unknown as { pwned?: number }).pwned;
  });
  afterEach(() => {
    clearMocks();
    resetScmForTest();
  });

  it("menampilkan kepala, statistik, label, reviewer, dan pemeriksaan CI", async () => {
    const { forgePr, forgeChecks } = setup();
    expect(await screen.findByRole("heading", { level: 1 })).toHaveTextContent("Add login form #12");
    expect(forgePr).toHaveBeenCalledWith({ accountId: githubAccount.id, repo: params.repo, number: 12 });
    await waitFor(() => expect(forgeChecks).toHaveBeenCalledWith({ accountId: githubAccount.id, repo: params.repo, sha: "f".repeat(40) }));
    expect(screen.getByText("Open", { selector: ".fg-state-badge" })).toHaveAttribute("data-state", "open");
    expect(screen.getByText("alice")).toBeInTheDocument();
    expect(screen.getByText("feature/login")).toBeInTheDocument();
    expect(screen.getByText("3 commits")).toBeInTheDocument();
    expect(screen.getByText("2 files changed")).toBeInTheDocument();
    expect(screen.getByText("+40")).toBeInTheDocument();
    expect(screen.getByText("−7")).toBeInTheDocument();
    expect(screen.getByText("No conflicts with the target branch")).toBeInTheDocument();
    expect(within(screen.getByLabelText("Labels")).getAllByText(/bug|ui/)).toHaveLength(2);
    expect(screen.getByText("bob, carol")).toBeInTheDocument();
    // Avatar http (bukan https) tidak dimuat: diganti inisial.
    expect(document.querySelector("img.fg-avatar")).toBeNull();

    const section = await screen.findByRole("region", { name: "Checks" });
    await within(section).findByText("build");
    expect(within(section).getByText("1 failed, 1 in progress, 1 passed")).toBeInTheDocument();
    expect(within(section).getByLabelText("Passed")).toHaveAttribute("data-state", "success");
    expect(within(section).getByLabelText("Failed")).toHaveAttribute("data-state", "failure");
    // Hanya URL http(s) yang menjadi tautan.
    const links = within(section).getAllByRole("button", { name: "Details" });
    expect(links).toHaveLength(1);
    fireEvent.click(links[0]);
    expect(openUrl).toHaveBeenCalledWith("https://ci.example/build/1");
  });

  it("deskripsi dirender sebagai Markdown yang disanitasi", async () => {
    setup();
    const section = await screen.findByRole("region", { name: "Description" });
    expect(section.querySelector("strong")?.textContent).toBe("login");
    expect(section.querySelector("script")).toBeNull();
    expect((window as unknown as { pwned?: number }).pwned).toBeUndefined();
    const anchors = [...section.querySelectorAll("a")];
    // Tautan `javascript:` tidak pernah menjadi tautan sama sekali.
    expect(anchors.map((a) => a.getAttribute("href"))).toEqual(["https://example.com/spec"]);
    expect(section.querySelector('[href^="javascript"]')).toBeNull();
    expect(fireEvent.click(anchors[0])).toBe(false);
    expect(openUrl).toHaveBeenCalledWith("https://example.com/spec");
  });

  it("GitLab: jumlah baris disembunyikan bila keduanya 0, dan istilahnya Merge Request", async () => {
    setup({
      detail: makeDetail({ additions: 0, deletions: 0, body: "", mergeable: null }),
      tabParams: { ...params, accountId: gitlabAccount.id, kind: "gitlab", repo: { host: "gitlab.com", owner: "g", name: "app" } },
    });
    expect(await screen.findByRole("heading", { level: 1 })).toHaveTextContent("Add login form !12");
    expect(screen.queryByText("+0")).toBeNull();
    expect(screen.queryByText("−0")).toBeNull();
    expect(screen.getByText("No description provided.")).toBeInTheDocument();
    expect(screen.getByText("Mergeability not known yet")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Open on GitLab/ })).toBeInTheDocument();
  });

  it("Checkout memanggil forgePrCheckout dan menyebut nama branch di toast", async () => {
    const { forgePrCheckout, gitDiffFiles } = setup();
    fireEvent.click(await screen.findByRole("button", { name: "Checkout" }));
    await waitFor(() => expect(forgePrCheckout).toHaveBeenCalledTimes(1));
    expect(forgePrCheckout.mock.calls[0][0]).toMatchObject({ root: ROOT, kind: "github", remote: "origin", number: 12 });
    expect(forgePrCheckout.mock.calls[0][0].operationId).toMatch(/^op-/);
    expect(await screen.findByText("Checked out pull request #12 as local branch pr/12.")).toBeInTheDocument();
    expect(gitDiffFiles).not.toHaveBeenCalled();
  });

  it("belum di-checkout: tombolnya 'Checkout to view changes', lalu berkas dibandingkan dari merge base dengan origin/<target>", async () => {
    const { forgePrCheckout, gitDiffFiles, gitMergeBase } = setup();
    expect(await screen.findByText(/Files can be compared after this PR is checked out/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "View Changes" })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Checkout to view changes" }));
    await waitFor(() => expect(gitDiffFiles).toHaveBeenCalledTimes(1));
    expect(forgePrCheckout).toHaveBeenCalledTimes(1);
    expect(gitMergeBase).toHaveBeenCalledWith({ root: ROOT, a: "origin/main", b: "pr/12" });
    // Diff dimulai dari merge base, bukan dari ujung branch tujuan.
    expect(gitDiffFiles).toHaveBeenCalledWith({ root: ROOT, from: MERGE_BASE, to: "pr/12" });

    const row = (await screen.findByText("src/app.ts")).closest(".fg-file") as HTMLElement;
    expect(within(row).getByText("R")).toBeInTheDocument();
    expect(within(row).getByText("+10")).toBeInTheDocument();
    const note = document.querySelector(".fg-compare-note") as HTMLElement;
    expect(note).toHaveTextContent("changed since it branched off origin/main (merge base 99999999)");
    expect(note).not.toHaveTextContent(/no common ancestor/);

    fireEvent.click(row);
    const tabs = useWorkbench.getState().groups[0].tabs;
    expect(tabs).toHaveLength(1);
    expect(tabs[0]).toMatchObject({ kind: "diff", params: { root: ROOT, path: "src/app.ts", origPath: "src/main.ts", source: { kind: "range", from: MERGE_BASE, to: "pr/12" } } });
  });

  it.each([
    ["tidak ada merge base (null)", () => null],
    [
      "gitMergeBase gagal",
      () => {
        throw { code: "git", message: "x", detail: null };
      },
    ],
  ])("%s: kembali ke perbandingan dua titik dan mengatakannya", async (_name, mergeBase) => {
    const refs = makeRefs();
    refs.branches.push(makeBranch("pr/12", { oid: "f".repeat(40) }));
    const { gitDiffFiles } = setup({ refs, mergeBase });
    fireEvent.click(await screen.findByRole("button", { name: "View Changes" }));
    const row = (await screen.findByText("src/login.ts")).closest(".fg-file") as HTMLElement;
    expect(gitDiffFiles).toHaveBeenCalledWith({ root: ROOT, from: "origin/main", to: "pr/12" });
    expect(document.querySelector(".fg-compare-note")).toHaveTextContent(/have no common ancestor, so this is a direct comparison/);
    fireEvent.click(row);
    expect(useWorkbench.getState().groups[0].tabs[0]).toMatchObject({ kind: "diff", params: { source: { kind: "range", from: "origin/main", to: "pr/12" } } });
  });

  it("sudah di-checkout: View Changes tanpa checkout ulang, dan peringatan bila branch lokal tertinggal", async () => {
    const refs = makeRefs();
    refs.branches.push(makeBranch("pr/12", { oid: "1".repeat(40) }));
    const { forgePrCheckout, gitDiffFiles } = setup({ refs });
    fireEvent.click(await screen.findByRole("button", { name: "View Changes" }));
    await screen.findByText("src/login.ts");
    expect(gitDiffFiles).toHaveBeenCalledWith({ root: ROOT, from: MERGE_BASE, to: "pr/12" });
    expect(forgePrCheckout).not.toHaveBeenCalled();
    expect(screen.getByText(/The local branch pr\/12 is at 11111111, but the PR head is ffffffff/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Update & Checkout" })).toBeInTheDocument();
  });

  it("branch tujuan tidak ada secara lokal: pesan yang jelas, tanpa memanggil git", async () => {
    const refs = makeRefs({ branches: [makeBranch("feature/login", { isHead: true }), makeBranch("pr/12", { oid: "f".repeat(40) })], remoteBranches: [] });
    const { gitDiffFiles, gitMergeBase } = setup({ refs });
    fireEvent.click(await screen.findByRole("button", { name: "View Changes" }));
    expect(await screen.findByText(/The target branch main is not available locally/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Fetch origin and retry" })).toBeInTheDocument();
    expect(gitDiffFiles).not.toHaveBeenCalled();
    expect(gitMergeBase).not.toHaveBeenCalled();
  });

  it("galat memuat PR menampilkan pesan dengan Retry", async () => {
    resetScmForTest({ [ROOT]: makeRepoState() }, ROOT);
    let fail = true;
    mockCommands({
      forgePr: () => {
        if (fail) throw { code: "notFound", message: "Tidak ditemukan", detail: null };
        return makeDetail();
      },
      forgeChecks: () => [],
    });
    render(<PullRequestTab tab={{ id: "pr", kind: "pullRequest", title: "#12", params: { ...params } }} groupId="g" visible focused />);
    expect(await screen.findByRole("alert")).toHaveTextContent(/Not found on the server, or this account has no access to it/);
    fail = false;
    fireEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(await screen.findByRole("heading", { level: 1 })).toHaveTextContent("Add login form");
    expect(await screen.findByText(/No checks reported for ffffffff/)).toBeInTheDocument();
  });
});

describe("pembantu tab PR", () => {
  it("baseRefFor memilih remote/target, lalu branch lokal, lalu menyerah", () => {
    const refs = makeRefs();
    expect(baseRefFor("origin", "main", refs)).toBe("origin/main");
    expect(baseRefFor("origin", "feature/login", refs)).toBe("feature/login");
    expect(baseRefFor("origin", "release", refs)).toBeNull();
    expect(baseRefFor("upstream", "main", refs)).toBe("main");
    expect(baseRefFor("origin", "main", null)).toBeNull();
  });

  it("summarizeChecks", () => {
    expect(summarizeChecks([])).toBe("");
    expect(summarizeChecks(checks)).toBe("1 failed, 1 in progress, 1 passed");
    expect(summarizeChecks([{ name: "a", state: "skipped", url: null }, { name: "b", state: "success", url: null }])).toBe("1 passed, 1 skipped or cancelled");
  });
});
