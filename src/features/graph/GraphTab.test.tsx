import { StrictMode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { clearMocks } from "@tauri-apps/api/mocks";
import { OverlayHost, resetOverlayForTest } from "@/app/overlay";
import type { Tab } from "@/app/registry";
import { useApp } from "@/app/store";
import { allTabs, resetWorkbenchForTest } from "@/app/workbench";
import { events, mockCommands, type CommitDetail, type CommitFile, type GraphRow, type LogPage, type LogQuery } from "@/ipc";
import { installScmStore, resetScmForTest } from "@/features/scm/store";
import { GraphTab } from "./GraphTab";
import { ROW_HEIGHT } from "./geometry";
import { branch, commit, linearRows, oid, repoState, ROOT, snapshot, stubViewport } from "./testing";

const TAB: Tab = { id: `graph:${ROOT}`, kind: "graph", title: "Graph", params: { root: ROOT } };
const FILES: CommitFile[] = [
  { path: "src/auth.rs", origPath: null, kind: "modified", additions: 12, deletions: 3 },
  { path: "src/new_name.rs", origPath: "src/old_name.rs", kind: "renamed", additions: 0, deletions: 0 },
  { path: "logo.png", origPath: null, kind: "added", additions: null, deletions: null },
];

/** Riwayat palsu: commit 0 (HEAD, banyak ref), commit 1 adalah merge, sisanya lurus. */
function history(total: number): GraphRow[] {
  const rows = linearRows(0, total);
  rows[0] = {
    ...rows[0],
    commit: commit(0, {
      subject: "Add login validation",
      refs: [
        { kind: "head", name: "HEAD" },
        { kind: "branch", name: "main" },
        { kind: "remote", name: "origin/main" },
        { kind: "tag", name: "v1.2.0" },
        { kind: "tag", name: "latest" },
      ],
    }),
  };
  rows[1] = {
    ...rows[1],
    commit: commit(1, { subject: "Merge branch 'hotfix'", parents: [oid(2), oid(40)] }),
    edges: [
      { from: 0, to: 0, color: 0, kind: "top" },
      { from: 0, to: 0, color: 0, kind: "bottom" },
      { from: 0, to: 1, color: 1, kind: "bottom" },
    ],
  };
  rows[2] = { ...rows[2], edges: [...rows[2].edges, { from: 1, to: 1, color: 1, kind: "through" }] };
  return rows;
}

function backend(total = 1000) {
  const all = history(total);
  let nextId = 1;
  const cursors = new Map<number, number>();
  const calls = {
    open: vi.fn((_: { root: string; query: LogQuery }) => {
      const id = nextId++;
      cursors.set(id, 0);
      return id;
    }),
    next: vi.fn(({ streamId, count }: { streamId: number; count: number }): LogPage | Promise<LogPage> => {
      const at = cursors.get(streamId);
      if (at === undefined) throw { code: "invalid", message: "stream closed", detail: null };
      const rows = all.slice(at, at + count);
      cursors.set(streamId, at + rows.length);
      return { rows, done: at + rows.length >= all.length };
    }),
    close: vi.fn(({ streamId }: { streamId: number }) => {
      cursors.delete(streamId);
      return null;
    }),
    detail: vi.fn(({ oid: id }: { root: string; oid: string }): CommitDetail => {
      const found = all.find((r) => r.commit.oid === id)?.commit ?? commit(9999, { oid: id });
      return {
        commit: { ...found, committer: { name: "Budi", email: "budi@example.com", time: found.author.time + 5 } },
        body: `${found.subject}\n\nLonger explanation of the change.`,
        files: FILES,
      };
    }),
    diffFiles: vi.fn((_: { root: string; from: string; to: string }) => FILES.slice(0, 1)),
    cherryPick: vi.fn((_: { root: string; oids: string[] }) => null),
  };
  mockCommands({
    gitLogOpen: calls.open,
    gitLogNext: calls.next,
    gitLogClose: calls.close,
    gitCommitDetail: calls.detail,
    gitDiffFiles: calls.diffFiles,
    gitCherryPick: calls.cherryPick,
    gitRefs: () => repoState().refs!,
    gitStashList: () => [],
    gitWorktrees: () => [],
    gitSubmodules: () => [],
  });
  return {
    ...calls,
    openStreams: () => [...cursors.keys()],
    /** Menambah commit baru di puncak riwayat (terlihat oleh stream yang dibuka setelahnya). */
    prepend: (count: number) => all.unshift(...linearRows(5000, count).map((r, i) => ({ ...r, commit: { ...r.commit, subject: `New commit ${i}` } }))),
  };
}

function renderTab(visible = true) {
  const view = render(
    <>
      <GraphTab tab={TAB} groupId="g1" visible={visible} focused={visible} />
      <OverlayHost />
    </>,
  );
  const list = () => screen.getByRole("listbox", { name: "Commits" });
  const row = (n: number) => view.container.querySelector<HTMLElement>(`[data-oid="${oid(n)}"]`);
  const scrollToRow = (n: number) => {
    const el = list();
    el.scrollTop = n * ROW_HEIGHT;
    fireEvent.scroll(el);
  };
  return { ...view, list, row, scrollToRow };
}

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

describe("GraphTab", () => {
  it("opens one log stream and renders the first page with graph and ref badges", async () => {
    const api = backend();
    const { row, list } = renderTab();

    expect(await screen.findByText("Add login validation")).toBeInTheDocument();
    expect(api.open).toHaveBeenCalledTimes(1);
    expect(api.open).toHaveBeenCalledWith({ root: ROOT, query: { all: true } });
    expect(api.next).toHaveBeenCalledTimes(1);
    expect(api.next).toHaveBeenCalledWith({ streamId: 1, count: 300 });
    expect(screen.getByText("300 commits+")).toBeInTheDocument();

    // Hanya baris di sekitar area terlihat yang ada di DOM (520px / 26px = 20 baris + overscan).
    const rendered = within(list()).getAllByRole("option");
    expect(rendered.length).toBeGreaterThanOrEqual(20);
    expect(rendered.length).toBeLessThan(60);

    // Lencana: tiga pertama ditampilkan dengan gaya menurut jenisnya, sisanya diringkas "+2".
    const head = row(0)!;
    const badges = [...head.querySelectorAll(".gr-badge")].map((b) => [b.getAttribute("data-kind"), b.textContent]);
    expect(badges).toEqual([
      ["head", "HEAD"],
      ["branch", "main"],
      ["remote", "origin/main"],
      ["more", "+2"],
    ]);
    expect(head.querySelector('.gr-badge[data-kind="more"]')).toHaveAttribute("title", "v1.2.0\nlatest");
    expect(within(head).getByText("Ana")).toBeInTheDocument();
    expect(within(head).getByText(oid(0).slice(0, 8))).toBeInTheDocument();

    // Commit biasa: titik penuh dan satu garis per edge; commit merge: titik berongga.
    expect(head.querySelectorAll("svg path")).toHaveLength(1);
    expect(head.querySelector("svg circle")).toHaveAttribute("fill", "var(--lane-0)");
    const merge = row(1)!;
    expect(merge.querySelector("svg circle.gr-dot-merge")).toHaveAttribute("stroke", "var(--lane-0)");
    expect([...merge.querySelectorAll("svg path")].map((p) => p.getAttribute("d"))).toEqual(["M 7 0 V 13", "M 7 13 V 26", "M 7 13 H 14 Q 21 13 21 20 V 26"]);
    expect(merge.querySelectorAll("svg path")[2]).toHaveAttribute("stroke", "var(--lane-1)");
    // Lebar SVG mengikuti jalur terkanan yang dimuat (2 jalur × 14px).
    expect(head.querySelector("svg")).toHaveAttribute("width", "28");
  });

  it("requests the next page only when scrolled near the end", async () => {
    const api = backend();
    const { scrollToRow, row } = renderTab();
    await screen.findByText("Add login validation");

    // Di tengah daftar: masih jauh dari akhir, tidak ada permintaan baru.
    scrollToRow(100);
    await waitFor(() => expect(row(100)).toBeInTheDocument());
    expect(api.next).toHaveBeenCalledTimes(1);

    // Baris terakhir yang terlihat kini kurang dari 60 baris dari akhir.
    scrollToRow(240);
    await waitFor(() => expect(api.next).toHaveBeenCalledTimes(2));
    expect(api.next).toHaveBeenLastCalledWith({ streamId: 1, count: 300 });
    expect(await screen.findByText("600 commits+")).toBeInTheDocument();
    expect(api.open).toHaveBeenCalledTimes(1);

    scrollToRow(560);
    await waitFor(() => expect(api.next).toHaveBeenCalledTimes(3));
    expect(await screen.findByText("900 commits+")).toBeInTheDocument();
    await waitFor(() => expect(row(560)).toBeInTheDocument());
  });

  it("shows a loading row while a page is in flight and stops paging at the end of history", async () => {
    const api = backend(320);
    const { scrollToRow, list } = renderTab();
    await screen.findByText("Add login validation");

    // Halaman kedua ditahan agar keadaan "sedang memuat" bisa diamati.
    const page = api.next.getMockImplementation()!;
    let release!: () => void;
    const held = new Promise<void>((resolve) => (release = resolve));
    api.next.mockImplementationOnce(async (args) => {
      await held;
      return page(args);
    });

    scrollToRow(290);
    expect(await screen.findByText("Loading more commits…")).toBeInTheDocument();
    expect(list()).toHaveAttribute("aria-busy", "true");
    expect(api.next).toHaveBeenCalledTimes(2);
    // Menggulir lagi selagi menunggu tidak mengirim permintaan ganda.
    scrollToRow(295);
    await act(async () => {});
    expect(api.next).toHaveBeenCalledTimes(2);

    await act(async () => release());
    expect(await screen.findByText("320 commits")).toBeInTheDocument();
    expect(screen.queryByText("Loading more commits…")).not.toBeInTheDocument();
    expect(list()).toHaveAttribute("aria-busy", "false");

    // Riwayat sudah habis: menggulir ke akhir tidak meminta halaman lagi.
    scrollToRow(319);
    await act(async () => {});
    expect(api.next).toHaveBeenCalledTimes(2);
  });

  it("closes the stream when the tab is unmounted", async () => {
    const api = backend();
    const { unmount } = renderTab();
    await screen.findByText("Add login validation");
    expect(api.openStreams()).toEqual([1]);
    unmount();
    await waitFor(() => expect(api.close).toHaveBeenCalledWith({ streamId: 1 }));
    expect(api.openStreams()).toEqual([]);
  });

  it("reopens the stream with grep and author after typing, closing the previous one", async () => {
    const api = backend();
    renderTab();
    await screen.findByText("Add login validation");

    fireEvent.change(screen.getByLabelText("Search commit messages"), { target: { value: "login" } });
    await waitFor(() => expect(api.open).toHaveBeenLastCalledWith({ root: ROOT, query: { all: true, grep: "login" } }), { timeout: 2000 });
    await waitFor(() => expect(api.close).toHaveBeenCalledWith({ streamId: 1 }));

    fireEvent.change(screen.getByLabelText("Filter by author"), { target: { value: "ana" } });
    await waitFor(() => expect(api.open).toHaveBeenLastCalledWith({ root: ROOT, query: { all: true, grep: "login", author: "ana" } }), { timeout: 2000 });
    await waitFor(() => expect(api.openStreams()).toEqual([3]));
  });

  it("maps the branch scope select to the log query", async () => {
    const api = backend();
    resetScmForTest({ [ROOT]: repoState({ refs: { branches: [branch("main", { isHead: true }), branch("feature/login")], remoteBranches: [], tags: [], remotes: [] } }) }, ROOT);
    renderTab();
    await screen.findByText("Add login validation");

    fireEvent.change(screen.getByLabelText("Branch scope"), { target: { value: "current" } });
    await waitFor(() => expect(api.open).toHaveBeenLastCalledWith({ root: ROOT, query: {} }));
    fireEvent.change(screen.getByLabelText("Branch scope"), { target: { value: "ref:feature/login" } });
    await waitFor(() => expect(api.open).toHaveBeenLastCalledWith({ root: ROOT, query: { revs: ["feature/login"] } }));
    await waitFor(() => expect(api.openStreams()).toEqual([3]));
  });

  it("loads and shows commit details when a commit is selected", async () => {
    const api = backend();
    const { row } = renderTab();
    await screen.findByText("Add login validation");
    expect(screen.getByText(/Select a commit to see its details/)).toBeInTheDocument();

    fireEvent.click(row(3)!);
    expect(row(3)).toHaveAttribute("aria-selected", "true");
    const details = await screen.findByLabelText(`Commit ${oid(3).slice(0, 8)}`);
    expect(api.detail).toHaveBeenCalledWith({ root: ROOT, oid: oid(3) });

    expect(within(details).getByText(oid(3))).toBeInTheDocument();
    expect(within(details).getByText(/Longer explanation of the change\./)).toBeInTheDocument();
    expect(within(details).getByText("Author")).toBeInTheDocument();
    expect(within(details).getByText(/ana@example\.com/)).toBeInTheDocument();
    // Committer berbeda dari penulis, jadi ikut ditampilkan.
    expect(within(details).getByText("Committer")).toBeInTheDocument();
    expect(within(details).getByText(/budi@example\.com/)).toBeInTheDocument();
    expect(within(details).getByText("3 changed files")).toBeInTheDocument();

    const files = within(details).getAllByRole("listitem");
    expect(files).toHaveLength(3);
    expect(within(files[0]).getByText("M")).toHaveClass("oc-git-modified");
    expect(within(files[0]).getByText("+12")).toBeInTheDocument();
    expect(within(files[0]).getByText("−3")).toBeInTheDocument();
    expect(files[1]).toHaveTextContent("src/old_name.rs → src/new_name.rs");
    expect(within(files[1]).getByText("R")).toHaveClass("oc-git-renamed");
    expect(within(files[2]).getByText("binary")).toBeInTheDocument();

    // Klik berkas membuka diff commit itu terhadap induknya.
    fireEvent.click(files[0]);
    const diff = allTabs().find((t) => t.kind === "diff");
    expect(diff?.params).toEqual({ root: ROOT, path: "src/auth.rs", origPath: null, source: { kind: "commit", oid: oid(3), parent: null } });
    fireEvent.click(files[1]);
    expect(allTabs().find((t) => t.kind === "diff")?.params).toMatchObject({ path: "src/new_name.rs", origPath: "src/old_name.rs" });
  });

  it("selects the parent commit when a parent hash is clicked", async () => {
    const api = backend();
    const { row } = renderTab();
    await screen.findByText("Add login validation");
    fireEvent.click(row(3)!);
    const details = await screen.findByLabelText(`Commit ${oid(3).slice(0, 8)}`);

    fireEvent.click(within(details).getByRole("button", { name: oid(4).slice(0, 8) }));
    await screen.findByLabelText(`Commit ${oid(4).slice(0, 8)}`);
    expect(api.detail).toHaveBeenLastCalledWith({ root: ROOT, oid: oid(4) });
    expect(row(4)).toHaveAttribute("aria-selected", "true");
    expect(row(3)).toHaveAttribute("aria-selected", "false");
  });

  it("shows an error with retry when the details cannot be loaded", async () => {
    const api = backend();
    api.detail.mockImplementationOnce(() => {
      throw { code: "unknown", message: "bad object", detail: null };
    });
    const { row } = renderTab();
    await screen.findByText("Add login validation");
    fireEvent.click(row(2)!);
    expect(await screen.findByRole("alert")).toHaveTextContent(`Could not load commit ${oid(2).slice(0, 8)}: bad object`);

    fireEvent.click(screen.getByRole("button", { name: /Retry/ }));
    expect(await screen.findByLabelText(`Commit ${oid(2).slice(0, 8)}`)).toBeInTheDocument();
    expect(api.detail).toHaveBeenCalledTimes(2);
  });

  it("compares two commits on Ctrl-click, older to newer, and opens range diffs", async () => {
    const api = backend();
    const { row } = renderTab();
    await screen.findByText("Add login validation");

    // Pilih yang lebih baru dulu, lalu yang lebih lama: urutan perbandingan tetap lama..baru.
    fireEvent.click(row(3)!);
    fireEvent.click(row(8)!, { ctrlKey: true });
    expect(row(3)).toHaveAttribute("aria-selected", "true");
    expect(row(8)).toHaveAttribute("aria-selected", "true");

    const compare = await screen.findByLabelText("Comparison");
    expect(compare).toHaveTextContent(`Comparing ${oid(8).slice(0, 8)}..${oid(3).slice(0, 8)}`);
    await waitFor(() => expect(api.diffFiles).toHaveBeenCalledWith({ root: ROOT, from: oid(8), to: oid(3) }));

    fireEvent.click(await within(compare).findByRole("listitem"));
    expect(allTabs().find((t) => t.kind === "diff")?.params).toEqual({ root: ROOT, path: "src/auth.rs", origPath: null, source: { kind: "range", from: oid(8), to: oid(3) } });

    // Ctrl-klik lagi melepas commit kedua dan kembali ke detail satu commit.
    fireEvent.click(row(8)!, { ctrlKey: true });
    expect(await screen.findByLabelText(`Commit ${oid(3).slice(0, 8)}`)).toBeInTheDocument();
    expect(row(8)).toHaveAttribute("aria-selected", "false");
  });

  it("shows an uncommitted-changes row that reveals Source Control", async () => {
    backend();
    resetScmForTest(
      {
        [ROOT]: repoState({
          snapshot: snapshot({
            unstaged: [{ path: "a.txt", origPath: null, kind: "modified", submodule: null }],
            staged: [{ path: "a.txt", origPath: null, kind: "modified", submodule: null }],
            untracked: ["b.txt"],
          }),
        }),
      },
      ROOT,
    );
    useApp.setState({ sidebarView: "explorer", sidebarVisible: false });
    const { list } = renderTab();
    await screen.findByText("Add login validation");

    // a.txt (staged + unstaged) dihitung sekali, ditambah b.txt.
    const first = within(list()).getAllByRole("option")[0];
    expect(first).toHaveTextContent("Uncommitted changes (2)");
    fireEvent.click(first);
    expect(useApp.getState()).toMatchObject({ sidebarView: "scm", sidebarVisible: true });
  });

  it("has no uncommitted row when the working tree is clean", async () => {
    backend();
    const { list } = renderTab();
    await screen.findByText("Add login validation");
    expect(within(list()).getAllByRole("option")[0]).toHaveTextContent("Add login validation");
    expect(screen.queryByText(/Uncommitted changes/)).not.toBeInTheDocument();
  });

  it("scrolls to a ref's commit from the references pane, loading pages until it is found", async () => {
    const api = backend();
    resetScmForTest(
      {
        [ROOT]: repoState({
          refs: {
            branches: [branch("main", { isHead: true }), branch("old-work", { oid: oid(450), ahead: 2, behind: 1 }), branch("orphan", { oid: "f".repeat(40) })],
            remoteBranches: [],
            tags: [],
            remotes: [],
          },
        }),
      },
      ROOT,
    );
    renderTab();
    await screen.findByText("Add login validation");
    const local = within(screen.getByLabelText("References")).getByRole("group", { name: "Local" });

    fireEvent.click(within(local).getByText("old-work"));
    await screen.findByLabelText(`Commit ${oid(450).slice(0, 8)}`);
    expect(api.next).toHaveBeenCalledTimes(2);
    expect(api.detail).toHaveBeenCalledWith({ root: ROOT, oid: oid(450) });

    // Ref yang commit-nya tidak ada di riwayat: seluruh stream dibaca lalu pengguna diberi tahu.
    fireEvent.click(within(local).getByText("orphan"));
    expect(await screen.findByText(/orphan was not found in the graph/)).toBeInTheDocument();
    expect(api.next).toHaveBeenCalledTimes(4);
    // Pilihan sebelumnya tidak berubah.
    expect(screen.getByLabelText(`Commit ${oid(450).slice(0, 8)}`)).toBeInTheDocument();
  });

  it("offers commit actions in the context menu and runs them on the clicked commit", async () => {
    const api = backend();
    const { row } = renderTab();
    await screen.findByText("Add login validation");

    fireEvent.contextMenu(row(5)!);
    const menu = await screen.findByRole("menu");
    expect(row(5)).toHaveAttribute("aria-selected", "true");
    const labels = within(menu)
      .getAllByRole("menuitem")
      .map((item) => item.textContent);
    expect(labels).toEqual([
      "Checkout (detached)",
      "Create Branch Here…",
      "Create Tag Here…",
      "Cherry-pick",
      "Revert…",
      "Reset Current Branch to Here (Soft)…",
      "Reset Current Branch to Here (Mixed)…",
      "Reset Current Branch to Here (Hard)…",
      "Interactive Rebase from Here…",
      "Copy Hash",
      "Copy Subject",
    ]);
    // Commit 5 adalah leluhur HEAD (commit 0), jadi rebase dari sini tersedia.
    expect(within(menu).getByRole("menuitem", { name: "Interactive Rebase from Here…" })).toBeEnabled();

    fireEvent.click(within(menu).getByRole("menuitem", { name: "Cherry-pick" }));
    await waitFor(() => expect(api.cherryPick).toHaveBeenCalledWith({ root: ROOT, oids: [oid(5)] }));
  });

  it("disables rebase and soft/mixed reset on the HEAD commit itself", async () => {
    backend();
    const { row } = renderTab();
    await screen.findByText("Add login validation");
    fireEvent.contextMenu(row(0)!);
    const menu = await screen.findByRole("menu");
    expect(within(menu).getByRole("menuitem", { name: "Interactive Rebase from Here…" })).toBeDisabled();
    expect(within(menu).getByRole("menuitem", { name: "Reset Current Branch to Here (Soft)…" })).toBeDisabled();
    expect(within(menu).getByRole("menuitem", { name: "Cherry-pick" })).toBeDisabled();
    expect(within(menu).getByRole("menuitem", { name: "Reset Current Branch to Here (Hard)…" })).toBeEnabled();
  });

  it("reloads through a new stream when refs change, keeping the selection", async () => {
    const api = backend();
    installScmStore();
    const { row } = renderTab();
    await screen.findByText("Add login validation");
    fireEvent.click(row(3)!);
    await screen.findByLabelText(`Commit ${oid(3).slice(0, 8)}`);

    await act(async () => {
      await events.refsChanged.emit({ root: ROOT });
    });
    await waitFor(() => expect(api.open).toHaveBeenCalledTimes(2), { timeout: 2000 });
    await waitFor(() => expect(api.openStreams()).toEqual([2]));
    expect(api.close).toHaveBeenCalledWith({ streamId: 1 });
    await waitFor(() => expect(row(3)).toHaveAttribute("aria-selected", "true"));
    expect(screen.getByLabelText(`Commit ${oid(3).slice(0, 8)}`)).toBeInTheDocument();

    // Perubahan di repositori lain tidak memuat ulang graph ini.
    await act(async () => {
      await events.refsChanged.emit({ root: "/work/other" });
      await new Promise((resolve) => setTimeout(resolve, 250));
    });
    expect(api.open).toHaveBeenCalledTimes(2);
  });

  it("keeps the same commit at the top of the viewport when a reload brings new commits", async () => {
    const api = backend();
    const { list, scrollToRow, row } = renderTab();
    await screen.findByText("Add login validation");

    scrollToRow(50);
    list().scrollTop += 5;
    fireEvent.scroll(list());
    await waitFor(() => expect(row(50)).toBeInTheDocument());

    api.prepend(2);
    fireEvent.click(screen.getByRole("button", { name: "Refresh" }));
    await waitFor(() => expect(api.open).toHaveBeenCalledTimes(2));
    // Commit 50 sekarang berada dua baris lebih bawah; posisi gulir ikut bergeser sebesar itu.
    await waitFor(() => expect(list().scrollTop).toBe(52 * ROW_HEIGHT + 5));
    expect(row(50)).toBeInTheDocument();
  });

  it("stays at the top after a reload so new commits are visible", async () => {
    const api = backend();
    const { list } = renderTab();
    await screen.findByText("Add login validation");
    api.prepend(1);
    fireEvent.click(screen.getByRole("button", { name: "Refresh" }));
    expect(await screen.findByText("New commit 0")).toBeInTheDocument();
    expect(list().scrollTop).toBe(0);
  });

  it("restores the scroll position when the tab becomes visible again", async () => {
    backend();
    const { list, scrollToRow, row, rerender } = renderTab();
    await screen.findByText("Add login validation");
    scrollToRow(80);
    await waitFor(() => expect(row(80)).toBeInTheDocument());

    const tabAt = (visible: boolean) => (
      <>
        <GraphTab tab={TAB} groupId="g1" visible={visible} focused={visible} />
        <OverlayHost />
      </>
    );
    rerender(tabAt(false));
    // Webview membuang posisi gulir elemen yang disembunyikan (tanpa peristiwa scroll).
    list().scrollTop = 0;
    rerender(tabAt(true));
    expect(list().scrollTop).toBe(80 * ROW_HEIGHT);
  });

  it("postpones the reload of a hidden tab until it is shown", async () => {
    const api = backend();
    installScmStore();
    const { rerender } = renderTab(false);
    await waitFor(() => expect(api.next).toHaveBeenCalledTimes(1));

    await act(async () => {
      await events.refsChanged.emit({ root: ROOT });
      await new Promise((resolve) => setTimeout(resolve, 250));
    });
    expect(api.open).toHaveBeenCalledTimes(1);

    rerender(
      <>
        <GraphTab tab={TAB} groupId="g1" visible focused />
        <OverlayHost />
      </>,
    );
    await waitFor(() => expect(api.open).toHaveBeenCalledTimes(2));
    await waitFor(() => expect(api.openStreams()).toEqual([2]));
  });

  it("renders under StrictMode without render loops or unstable store snapshots", async () => {
    const api = backend();
    const errors = vi.spyOn(console, "error").mockImplementation(() => undefined);
    try {
      const { container, unmount } = render(
        <StrictMode>
          <GraphTab tab={TAB} groupId="g1" visible focused />
        </StrictMode>,
      );
      await screen.findByText("Add login validation");
      fireEvent.click(container.querySelector<HTMLElement>(`[data-oid="${oid(2)}"]`)!);
      await screen.findByLabelText(`Commit ${oid(2).slice(0, 8)}`);
      // Perubahan store yang tidak menyentuh repositori ini tidak boleh memicu putaran render.
      act(() => resetScmForTest({ [ROOT]: repoState(), "/work/other": repoState({ root: "/work/other" }) }, ROOT));
      await act(async () => {});
      expect(errors).not.toHaveBeenCalled();

      // StrictMode memasang efek dua kali: stream pertama ditutup, hanya satu yang tetap terbuka.
      await waitFor(() => expect(api.openStreams()).toHaveLength(1));
      unmount();
      await waitFor(() => expect(api.openStreams()).toEqual([]));
      expect(errors).not.toHaveBeenCalled();
    } finally {
      errors.mockRestore();
    }
  });

  it("shows the error and a retry when the history cannot be loaded", async () => {
    const api = backend();
    api.open.mockImplementationOnce(() => {
      throw { code: "unknown", message: "not a git repository", detail: null };
    });
    renderTab();
    expect(await screen.findByRole("alert")).toHaveTextContent("Could not load the commit history: not a git repository");
    fireEvent.click(screen.getByRole("button", { name: /Retry/ }));
    expect(await screen.findByText("Add login validation")).toBeInTheDocument();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });
});
