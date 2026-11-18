import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen, within } from "@testing-library/react";
import type { Channel } from "@tauri-apps/api/core";
import { clearMocks } from "@tauri-apps/api/mocks";
import { history, undo } from "@codemirror/commands";
import { EditorSelection, EditorState, Text } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { OverlayHost, resetOverlayForTest } from "@/app/overlay";
import { useApp } from "@/app/store";
import { allTabs, resetWorkbenchForTest } from "@/app/workbench";
import { mockCommands, type FileMatches, type LineMatch, type MockCommandHandlers, type SearchEvent, type SearchQuery, type WorkspaceInfo } from "@/ipc";
import { activeEditor } from "../editor/CodeEditor";
import { bufferText, ensureBuffer, getBuffer, isDirty, resetBuffersForTest, retainBuffer, setBufferDoc } from "../editor/buffers";
import { bufferSync } from "../editor/sync";
import { SearchView } from "./SearchView";
import { focusSearch } from "./index";
import { confirmationText } from "./replace";
import { buildQuery, dismissMatch, flattenResults, parseGlobs, resetSearch, searchNow, setFilter, setPattern, summaryText, toggleOption, useSearch } from "./store";

vi.mock("../editor/CodeEditor", () => ({ activeEditor: vi.fn(() => null) }));

// ----- Backend tiruan -----

interface Run {
  query: SearchQuery;
  operationId: string;
  /** Mengirim satu event lewat channel, seperti yang dilakukan backend. */
  send: (event: SearchEvent) => void;
}

let runs: Run[];
let cancelled: string[];

/** `searchStart` tiruan: mencatat tiap pencarian dan memberi akses ke channel-nya. */
function backend(extra: Partial<MockCommandHandlers> = {}) {
  mockCommands({
    searchStart: ({ query, operationId, onEvent }: { query: SearchQuery; operationId: string; onEvent: Channel<SearchEvent> }) => {
      // Mock IPC meneruskan objek Channel apa adanya; `onmessage` adalah penangan yang dipasang aplikasi.
      runs.push({ query, operationId, send: (event) => onEvent.onmessage(event) });
      return null;
    },
    appCancel: ({ operationId }) => {
      cancelled.push(operationId);
      return null;
    },
    ...extra,
  });
}

const match = (line: number, column: number, preview: string, length: number, previewOffset = column): LineMatch => ({ line, column, length, preview, previewOffset });
const matches = (path: string, ...list: LineMatch[]): SearchEvent => ({ kind: "matches", file: { path, matches: list } });
const done = (files: number, count: number, truncated = false): SearchEvent => ({ kind: "done", summary: { files, matches: count, truncated } });

const workspace = { root: "/ws", name: "ws", trusted: true, repoRoot: null, settings: {} } as unknown as WorkspaceInfo;

/** Memajukan waktu tiruan sambil membiarkan promise IPC dan React menyelesaikan pekerjaannya. */
const tick = (ms = 0) => act(() => vi.advanceTimersByTimeAsync(ms));

const clientHeight = Object.getOwnPropertyDescriptor(HTMLElement.prototype, "clientHeight");
beforeAll(() => Object.defineProperty(HTMLElement.prototype, "clientHeight", { configurable: true, get: () => 600 }));
afterAll(() => {
  if (clientHeight) Object.defineProperty(HTMLElement.prototype, "clientHeight", clientHeight);
});

beforeEach(() => {
  vi.useFakeTimers();
  runs = [];
  cancelled = [];
  backend();
  resetSearch();
  useSearch.setState({ isRegex: false, caseSensitive: false, wholeWord: false });
  resetWorkbenchForTest();
  resetBuffersForTest();
  resetOverlayForTest();
  vi.mocked(activeEditor).mockReturnValue(null);
  useApp.setState({ workspace, sidebarVisible: true, sidebarView: "search" });
});

afterEach(() => {
  resetSearch();
  clearMocks();
  vi.useRealTimers();
  useApp.setState({ workspace: null });
});

describe("query building", () => {
  it("splits comma-separated globs and drops blanks", () => {
    expect(parseGlobs(" src , *.ts,, docs/**/*.md ,")).toEqual(["src", "*.ts", "docs/**/*.md"]);
    expect(parseGlobs("  ")).toEqual([]);
  });

  it("carries the options and the result cap", () => {
    expect(buildQuery({ pattern: "a.b", isRegex: true, caseSensitive: true, wholeWord: false, include: "src", exclude: "dist, *.min.js" })).toEqual({
      pattern: "a.b",
      isRegex: true,
      caseSensitive: true,
      wholeWord: false,
      include: ["src"],
      exclude: ["dist", "*.min.js"],
      maxResults: 5000,
    });
  });

  it("words the summary with singular and plural forms", () => {
    const one: FileMatches[] = [{ path: "a", matches: [match(1, 0, "x", 1)] }];
    expect(summaryText(one, false)).toBe("1 result in 1 file");
    expect(summaryText([...one, { path: "b", matches: [match(1, 0, "x", 1), match(2, 0, "x", 1)] }], true)).toBe("3 results in 2 files — results truncated");
  });
});

describe("search runner", () => {
  it("waits 250 ms after the last keystroke and then searches once", async () => {
    setPattern("f");
    await tick(200);
    setPattern("fo");
    await tick(200);
    setPattern("foo");
    await tick(249);
    expect(runs).toHaveLength(0);
    expect(useSearch.getState()).toMatchObject({ status: "idle", pending: true });

    await tick(1);
    expect(runs).toHaveLength(1);
    expect(runs[0].query).toMatchObject({ pattern: "foo", maxResults: 5000, include: [], exclude: [] });
    expect(useSearch.getState()).toMatchObject({ status: "running", pending: false });
  });

  it("searches again when an option or a filter changes", async () => {
    setPattern("foo");
    await tick(250);
    toggleOption("wholeWord");
    setFilter("include", "src, *.ts");
    await tick(250);
    expect(runs).toHaveLength(2);
    expect(runs[1].query).toMatchObject({ pattern: "foo", wholeWord: true, include: ["src", "*.ts"] });
  });

  it("cancels the previous run and ignores everything it still sends", async () => {
    setPattern("foo");
    await tick(250);
    runs[0].send(matches("old.ts", match(1, 0, "foo", 3)));
    await tick(50);
    expect(useSearch.getState().files.map((f) => f.path)).toEqual(["old.ts"]);

    setPattern("foobar");
    await tick(250);
    expect(runs).toHaveLength(2);
    expect(cancelled).toEqual([runs[0].operationId]);
    expect(runs[1].operationId).not.toBe(runs[0].operationId);
    expect(useSearch.getState()).toMatchObject({ files: [], status: "running" });

    // Sisa event dari pencarian lama datang terlambat, termasuk "selesai"-nya.
    runs[0].send(matches("stale.ts", match(1, 0, "foo", 3)));
    runs[0].send(done(2, 2, true));
    runs[1].send(matches("new.ts", match(3, 4, "let foobar", 6)));
    await tick(50);
    expect(useSearch.getState().files.map((f) => f.path)).toEqual(["new.ts"]);
    expect(useSearch.getState()).toMatchObject({ status: "running", summary: null });

    runs[1].send(done(1, 1));
    await tick();
    expect(useSearch.getState()).toMatchObject({ status: "done", summary: { files: 1, matches: 1, truncated: false } });
  });

  it("does not cancel a run that already finished", async () => {
    setPattern("foo");
    await tick(250);
    runs[0].send(done(0, 0));
    setPattern("bar");
    await tick(250);
    expect(runs).toHaveLength(2);
    expect(cancelled).toEqual([]);
  });

  it("clears the results and cancels when the query becomes empty", async () => {
    setPattern("foo");
    await tick(250);
    runs[0].send(matches("a.ts", match(1, 0, "foo", 3)));
    await tick(50);
    setPattern("");
    await tick(250);
    expect(runs).toHaveLength(1);
    expect(cancelled).toEqual([runs[0].operationId]);
    expect(useSearch.getState()).toMatchObject({ status: "idle", files: [], summary: null, resultQuery: null });
    runs[0].send(matches("late.ts", match(1, 0, "foo", 3)));
    await tick(50);
    expect(useSearch.getState().files).toEqual([]);
  });

  it("reports a failure sent by the backend or a rejected command", async () => {
    useSearch.setState({ pattern: "(", isRegex: true });
    searchNow();
    await tick();
    runs[0].send({ kind: "failed", message: "Invalid pattern: unclosed group" });
    expect(useSearch.getState()).toMatchObject({ status: "failed", error: "Invalid pattern: unclosed group" });

    backend({
      searchStart: () => {
        throw { code: "noWorkspace", message: "No folder is open", detail: null };
      },
    });
    searchNow();
    await tick();
    expect(useSearch.getState()).toMatchObject({ status: "failed", error: "No folder is open" });
  });

  it("keeps files sorted by path however the backend orders them, and flattens them into rows", async () => {
    setPattern("x");
    await tick(250);
    runs[0].send(matches("src/z.ts", match(1, 0, "x", 1)));
    runs[0].send(matches("README.md", match(2, 0, "x", 1), match(5, 3, "abcx", 1)));
    await tick(50);
    runs[0].send(matches("src/a.ts", match(9, 0, "x", 1)));
    runs[0].send(done(3, 4));
    await tick();
    const { files } = useSearch.getState();
    expect(files.map((f) => f.path)).toEqual(["README.md", "src/a.ts", "src/z.ts"]);
    expect(flattenResults(files, { "src/a.ts": true }).map((r) => (r.kind === "file" ? r.file.path : `  ${r.match.line}`))).toEqual(["README.md", "  2", "  5", "src/a.ts", "src/z.ts", "  1"]);
  });
});

describe("SearchView", () => {
  const type = (value: string) => fireEvent.change(screen.getByRole("textbox", { name: "Search" }), { target: { value } });
  const rowTexts = () => screen.getAllByRole("treeitem").map((el) => el.textContent);

  it("shows streamed results grouped by file with the match highlighted, then the summary", async () => {
    render(<SearchView />);
    type("wörld");
    expect(screen.getByRole("status")).toHaveTextContent("");
    await tick(250);
    expect(screen.getByRole("status")).toHaveTextContent("Searching…");
    expect(screen.getByLabelText("Searching")).toBeInTheDocument();

    runs[0].send(matches("src/app/main.ts", match(12, 13, "    héllo 😀 wörld!", 5), match(40, 0, "wörld", 5)));
    await tick(50);
    expect(screen.getByRole("status")).toHaveTextContent("2 results in 1 file so far…");
    runs[0].send(matches("README.md", match(3, 2, "a wörld", 5)));
    runs[0].send(done(2, 3));
    await tick();

    expect(screen.getByRole("status")).toHaveTextContent(/^3 results in 2 files$/);
    expect(screen.queryByLabelText("Searching")).not.toBeInTheDocument();
    // Baris berkas: nama, folder, jumlah. Baris kecocokan: pratinjau tanpa lekukan awal.
    expect(rowTexts()).toEqual(["README.md1", "a wörld", "main.tssrc/app2", "héllo 😀 wörld!", "wörld"]);
    const marks = [...document.querySelectorAll("mark")].map((m) => m.textContent);
    expect(marks).toEqual(["wörld", "wörld", "wörld"]);
    expect(screen.getByText("src/app")).toHaveClass("sr-file-dir");
  });

  it("flags truncated results, an empty result, and a failed search", async () => {
    render(<SearchView />);
    type("foo");
    await tick(250);
    runs[0].send(matches("a.ts", match(1, 0, "foo", 3)));
    runs[0].send(done(1, 1, true));
    await tick();
    expect(screen.getByRole("status")).toHaveTextContent("1 result in 1 file — results truncated");

    type("nothing");
    await tick(250);
    runs[1].send(done(0, 0));
    await tick();
    expect(screen.getByRole("status")).toHaveTextContent("No results found.");
    expect(screen.queryAllByRole("treeitem")).toEqual([]);

    type("(");
    await tick(250);
    runs[2].send({ kind: "failed", message: "Invalid pattern: unclosed group" });
    await tick();
    expect(screen.getByRole("status")).toHaveTextContent("Invalid pattern: unclosed group");
  });

  it("searches immediately on Enter", async () => {
    render(<SearchView />);
    type("foo");
    fireEvent.keyDown(screen.getByRole("textbox", { name: "Search" }), { key: "Enter" });
    await tick();
    expect(runs.map((r) => r.query.pattern)).toEqual(["foo"]);
    await tick(300);
    expect(runs).toHaveLength(1);
  });

  it("toggles options with pressed-state buttons and sends include/exclude globs", async () => {
    render(<SearchView />);
    type("foo");
    const caseButton = screen.getByRole("button", { name: "Match Case" });
    const regexButton = screen.getByRole("button", { name: "Use Regular Expression" });
    expect(caseButton).toHaveAttribute("aria-pressed", "false");
    fireEvent.click(caseButton);
    fireEvent.click(regexButton);
    expect(caseButton).toHaveAttribute("aria-pressed", "true");
    expect(regexButton).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByRole("button", { name: "Match Whole Word" })).toHaveAttribute("aria-pressed", "false");

    expect(screen.queryByLabelText("files to include")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: /files to include/ }));
    fireEvent.change(screen.getByLabelText("files to include"), { target: { value: "src, *.ts" } });
    fireEvent.change(screen.getByLabelText("files to exclude"), { target: { value: "dist" } });
    await tick(250);
    expect(runs).toHaveLength(1);
    expect(runs[0].query).toEqual({ pattern: "foo", isRegex: true, caseSensitive: true, wholeWord: false, include: ["src", "*.ts"], exclude: ["dist"], maxResults: 5000 });
  });

  it("removes dismissed matches and files from the list and the summary, and folds a file", async () => {
    render(<SearchView />);
    type("foo");
    await tick(250);
    runs[0].send(matches("a.ts", match(1, 0, "foo one", 3), match(2, 0, "foo two", 3)));
    runs[0].send(matches("b.ts", match(7, 0, "foo three", 3)));
    runs[0].send(done(2, 3));
    await tick();

    fireEvent.click(screen.getByRole("button", { name: "Dismiss match on line 1" }));
    expect(rowTexts()).toEqual(["a.ts1", "foo two", "b.ts1", "foo three"]);
    expect(screen.getByRole("status")).toHaveTextContent("2 results in 2 files");
    expect(useSearch.getState().dismissed).toEqual({ "a.ts": [{ line: 1, column: 0, length: 3 }] });

    fireEvent.click(screen.getByText("b.ts"));
    expect(rowTexts()).toEqual(["a.ts1", "foo two", "b.ts1"]);
    expect(screen.getByText("b.ts").closest('[role="treeitem"]')).toHaveAttribute("aria-expanded", "false");

    fireEvent.click(screen.getByRole("button", { name: "Dismiss b.ts" }));
    expect(rowTexts()).toEqual(["a.ts1", "foo two"]);
    expect(screen.getByRole("status")).toHaveTextContent("1 result in 1 file");

    // Kecocokan terakhir sebuah berkas membawa berkasnya ikut hilang.
    fireEvent.click(screen.getByRole("button", { name: "Dismiss match on line 2" }));
    expect(screen.queryAllByRole("treeitem")).toEqual([]);
    expect(useSearch.getState().dismissed).toEqual({});
  });

  it("opens a clicked match as a preview tab and selects the match once the editor is there", async () => {
    const reveals: unknown[] = [];
    const onReveal = (e: Event) => reveals.push((e as CustomEvent).detail);
    window.addEventListener("oncode:reveal", onReveal);
    render(<SearchView />);
    type("wörld");
    await tick(250);
    runs[0].send(matches("src/main.ts", match(12, 13, "    héllo 😀 wörld!", 5)));
    runs[0].send(done(1, 1));
    await tick();

    fireEvent.click(screen.getByText("wörld"));
    expect(allTabs()).toMatchObject([{ id: "file:/ws/src/main.ts", preview: true, params: { path: "/ws/src/main.ts" } }]);
    // Editor belum terpasang: belum ada yang bisa menerima event.
    await tick(200);
    expect(reveals).toEqual([]);
    retainBuffer("/ws/src/main.ts");
    getBuffer("/ws/src/main.ts")!.views.add({} as EditorView);
    await tick(100);
    expect(reveals).toEqual([{ path: "/ws/src/main.ts", line: 12, column: 13, length: 5 }]);
    window.removeEventListener("oncode:reveal", onReveal);
  });

  it("walks the rows with the arrow keys and opens the selected match with Enter", async () => {
    render(<SearchView />);
    type("foo");
    await tick(250);
    runs[0].send(matches("a.ts", match(1, 0, "foo one", 3), match(2, 0, "foo two", 3)));
    runs[0].send(matches("b.ts", match(7, 0, "foo three", 3)));
    runs[0].send(done(2, 3));
    await tick();
    const tree = screen.getByRole("tree");
    const selected = () => screen.getAllByRole("treeitem").find((el) => el.getAttribute("aria-selected") === "true")?.textContent;

    fireEvent.keyDown(tree, { key: "ArrowDown" });
    expect(selected()).toBe("a.ts2");
    fireEvent.keyDown(tree, { key: "ArrowDown" });
    fireEvent.keyDown(tree, { key: "ArrowDown" });
    expect(selected()).toBe("foo two");
    fireEvent.keyDown(tree, { key: "ArrowUp" });
    expect(selected()).toBe("foo one");
    expect(allTabs()).toEqual([]);
    fireEvent.keyDown(tree, { key: "Enter" });
    expect(allTabs().map((t) => t.id)).toEqual(["file:/ws/a.ts"]);

    // Kiri pada kecocokan pindah ke berkasnya; Enter pada berkas melipatnya.
    fireEvent.keyDown(tree, { key: "ArrowLeft" });
    expect(selected()).toBe("a.ts2");
    fireEvent.keyDown(tree, { key: "Enter" });
    expect(screen.getAllByRole("treeitem").map((el) => el.textContent)).toEqual(["a.ts2", "b.ts1", "foo three"]);
  });

  it("previews the replacement next to each match, expanding capture references", async () => {
    render(<SearchView />);
    fireEvent.click(screen.getByRole("button", { name: "Toggle Replace" }));
    fireEvent.click(screen.getByRole("button", { name: "Use Regular Expression" }));
    fireEvent.change(screen.getByRole("textbox", { name: "Replace" }), { target: { value: "$2.$1" } });
    type("(\\w+)\\.(\\w+)");
    await tick(250);
    runs[0].send(matches("a.ts", match(1, 4, "use foo.bar;", 7)));
    runs[0].send(done(1, 1));
    await tick();
    expect(document.querySelector("mark")).toHaveTextContent("foo.bar");
    expect(document.querySelector("ins")).toHaveTextContent("bar.foo");
  });

  it("focuses the query and prefills it from a single-line editor selection", async () => {
    render(<SearchView />);
    const input = screen.getByRole("textbox", { name: "Search" }) as HTMLInputElement;
    const editorWith = (doc: string, from: number, to: number) =>
      vi.mocked(activeEditor).mockReturnValue({ path: "/ws/a.ts", view: { state: EditorState.create({ doc, selection: EditorSelection.single(from, to) }) } as EditorView });
    useApp.setState({ sidebarView: "explorer", sidebarVisible: false });

    editorWith("let total = price * 2;\nnext", 4, 9);
    act(() => focusSearch());
    await tick();
    expect(useApp.getState()).toMatchObject({ sidebarView: "search", sidebarVisible: true });
    expect(input).toHaveFocus();
    expect(input.value).toBe("total");
    expect([input.selectionStart, input.selectionEnd]).toEqual([0, 5]);
    expect(runs.map((r) => r.query.pattern)).toEqual(["total"]);

    // Seleksi multi-baris atau kosong tidak mengubah kueri.
    editorWith("let total = price * 2;\nnext", 4, 25);
    act(() => focusSearch());
    editorWith("let total", 3, 3);
    act(() => focusSearch());
    await tick();
    expect(input.value).toBe("total");
    expect(runs).toHaveLength(1);

    // Pada mode regex, teks terpilih dicari apa adanya.
    act(() => toggleOption("isRegex"));
    editorWith("price * 2;", 0, 9);
    act(() => focusSearch());
    await tick();
    expect(input.value).toBe("price \\* 2");
  });
});

describe("Replace All", () => {
  const fileText: Record<string, string> = {};
  let saved: { path: string; text: string }[];
  let replaceCalls: { query: SearchQuery; replacement: string; files: string[] }[];

  function replaceBackend(extra: Partial<MockCommandHandlers> = {}) {
    saved = [];
    replaceCalls = [];
    backend({
      fsOpen: ({ path }) => ({ kind: "text", text: fileText[path] ?? "", encoding: "UTF-8", bom: false, eol: "lf", stat: { size: 1, mtimeMs: 1, readOnly: false } }),
      fsSave: ({ path, text }) => {
        saved.push({ path, text });
        return { size: text.length, mtimeMs: 2, readOnly: false };
      },
      searchReplace: (args) => {
        replaceCalls.push(args);
        return { files: args.files.length, replacements: args.files.length * 2 };
      },
      ...extra,
    });
  }

  /** Membuka buffer dengan satu editor sungguhan yang terhubung lewat `bufferSync`, seperti tab berkas. */
  async function openInEditor(path: string, text: string): Promise<EditorView> {
    fileText[path] = text;
    retainBuffer(path);
    const buffer = await ensureBuffer(path);
    return new EditorView({ parent: document.body, state: EditorState.create({ doc: buffer.doc, extensions: [history(), bufferSync(buffer)] }) });
  }

  /** Mengisi kueri dan pengganti lewat UI, lalu mengalirkan hasil. */
  async function searched(pattern: string, replacement: string, events: SearchEvent[]) {
    render(
      <>
        <SearchView />
        <OverlayHost />
      </>,
    );
    fireEvent.click(screen.getByRole("button", { name: "Toggle Replace" }));
    fireEvent.change(screen.getByRole("textbox", { name: "Search" }), { target: { value: pattern } });
    fireEvent.change(screen.getByRole("textbox", { name: "Replace" }), { target: { value: replacement } });
    await tick(250);
    for (const event of events) runs[runs.length - 1].send(event);
    await tick();
  }

  const confirmReplace = async () => {
    fireEvent.click(within(screen.getByRole("dialog")).getByRole("button", { name: "Replace" }));
    await tick();
    await tick();
  };

  beforeEach(() => {
    for (const key of Object.keys(fileText)) delete fileText[key];
    replaceBackend();
  });

  it("is disabled until a finished search has results that match the inputs", async () => {
    render(<SearchView />);
    fireEvent.click(screen.getByRole("button", { name: "Toggle Replace" }));
    const button = screen.getByRole("button", { name: "Replace All" });
    expect(button).toBeDisabled();
    fireEvent.change(screen.getByRole("textbox", { name: "Search" }), { target: { value: "foo" } });
    await tick(250);
    runs[0].send(matches("a.ts", match(1, 0, "foo", 3)));
    await tick(50);
    expect(button).toBeDisabled();
    runs[0].send(done(1, 1));
    await tick();
    expect(button).toBeEnabled();
    // Kueri diubah: hasil yang tampil bukan lagi milik kueri itu.
    fireEvent.change(screen.getByRole("textbox", { name: "Search" }), { target: { value: "fo" } });
    expect(button).toBeDisabled();
  });

  it("asks with the counts of what is still listed", () => {
    const files: FileMatches[] = [
      { path: "a.ts", matches: [match(1, 0, "foo", 3), match(2, 0, "foo", 3)] },
      { path: "b.ts", matches: [match(1, 0, "foo", 3)] },
    ];
    expect(confirmationText(files, "bar", false).title).toBe("Replace 3 occurrences across 2 files?");
    expect(confirmationText(files, "bar", false).message).toContain('replaced with "bar"');
    expect(confirmationText([files[1]], "", false)).toMatchObject({ title: "Replace 1 occurrence across 1 file?", message: expect.stringContaining("deleted") });
    expect(confirmationText(files, "bar", true).message).toContain("truncated");
  });

  it("confirms with the remaining counts and does nothing when cancelled", async () => {
    await searched("foo", "bar", [matches("a.ts", match(1, 0, "foo", 3), match(2, 0, "foo", 3)), matches("b.ts", match(1, 0, "foo", 3)), matches("c.ts", match(4, 0, "foo", 3)), done(3, 4)]);
    fireEvent.click(screen.getByRole("button", { name: "Dismiss c.ts" }));
    fireEvent.click(screen.getByRole("button", { name: "Replace All" }));
    await tick();

    const dialog = screen.getByRole("dialog");
    expect(dialog).toHaveTextContent("Replace 3 occurrences across 2 files?");
    expect(dialog).toHaveTextContent('Each occurrence will be replaced with "bar".');
    fireEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));
    await tick();
    expect(replaceCalls).toEqual([]);
    expect(runs).toHaveLength(1);
    expect(screen.getAllByRole("treeitem")).toHaveLength(5);
  });

  it("sends closed files to the backend, leaves dismissed files out, reports the totals and searches again", async () => {
    await searched("foo", "bar", [matches("a.ts", match(1, 0, "foo", 3), match(2, 0, "foo", 3)), matches("b.ts", match(1, 0, "foo", 3)), matches("c.ts", match(4, 0, "foo", 3)), done(3, 4)]);
    fireEvent.click(screen.getByRole("button", { name: "Dismiss c.ts" }));
    fireEvent.click(screen.getByRole("button", { name: "Replace All" }));
    await tick();
    await confirmReplace();

    expect(replaceCalls).toEqual([{ query: { pattern: "foo", isRegex: false, caseSensitive: false, wholeWord: false, include: [], exclude: [], maxResults: 5000 }, replacement: "bar", files: ["a.ts", "b.ts"] }]);
    expect(saved).toEqual([]);
    // Jumlah di toast berasal dari backend (2 berkas, 4 penggantian pada tiruan ini).
    expect(screen.getByText("Replaced 4 occurrences across 2 files.")).toBeInTheDocument();
    expect(runs).toHaveLength(2);
    expect(runs[1].query.pattern).toBe("foo");
  });

  it("rewrites a closed file itself when some of its matches were dismissed", async () => {
    fileText["/ws/a.ts"] = "foo one\nfoo two foo\nlast foo";
    await searched("foo", "bar", [matches("a.ts", match(1, 0, "foo one", 3), match(2, 0, "foo two foo", 3), match(2, 8, "foo two foo", 3), match(3, 5, "last foo", 3)), matches("b.ts", match(1, 0, "foo", 3)), done(2, 5)]);
    act(() => dismissMatch("a.ts", { line: 2, column: 8, length: 3 }));
    fireEvent.click(screen.getByRole("button", { name: "Replace All" }));
    await tick();
    expect(screen.getByRole("dialog")).toHaveTextContent("Replace 4 occurrences across 2 files?");
    await confirmReplace();

    // Backend hanya bisa mengganti seluruh berkas, jadi a.ts tidak boleh dikirim ke sana.
    expect(replaceCalls.map((c) => c.files)).toEqual([["b.ts"]]);
    expect(saved).toEqual([{ path: "/ws/a.ts", text: "bar one\nbar two foo\nlast bar" }]);
    expect(screen.getByText("Replaced 5 occurrences across 2 files.")).toBeInTheDocument();
  });

  it("changes an open file through its editor in one undoable transaction and saves it if it was clean", async () => {
    const view = await openInEditor("/ws/src/open.ts", "let foo = 1;\nfoo(foo);\nnothing");
    const transactions: number[] = [];
    const original = view.dispatch.bind(view);
    view.dispatch = ((...specs: Parameters<EditorView["dispatch"]>) => {
      transactions.push(specs.length);
      return original(...specs);
    }) as EditorView["dispatch"];

    await searched("foo", "bar", [matches("src/open.ts", match(1, 4, "let foo = 1;", 3), match(2, 0, "foo(foo);", 3), match(2, 4, "foo(foo);", 3)), matches("closed.ts", match(1, 0, "foo", 3)), done(2, 4)]);
    fireEvent.click(screen.getByRole("button", { name: "Replace All" }));
    await tick();
    await confirmReplace();

    expect(view.state.doc.toString()).toBe("let bar = 1;\nbar(bar);\nnothing");
    expect(transactions).toEqual([1]);
    expect(bufferText("/ws/src/open.ts")).toBe("let bar = 1;\nbar(bar);\nnothing");
    // Berkas yang terbuka tidak dikirim ke backend; karena tadinya bersih, ia langsung disimpan.
    expect(replaceCalls.map((c) => c.files)).toEqual([["closed.ts"]]);
    expect(saved).toEqual([{ path: "/ws/src/open.ts", text: "let bar = 1;\nbar(bar);\nnothing" }]);
    expect(isDirty("/ws/src/open.ts")).toBe(false);
    expect(screen.getByText("Replaced 5 occurrences across 2 files.")).toBeInTheDocument();

    // Satu undo mengembalikan semua penggantian di berkas itu.
    undo(view);
    expect(view.state.doc.toString()).toBe("let foo = 1;\nfoo(foo);\nnothing");
    view.destroy();
  });

  it("does not save an open file that already had unsaved changes, and replaces in its current text", async () => {
    const view = await openInEditor("/ws/open.ts", "foo\nold line");
    view.dispatch({ changes: { from: view.state.doc.length, insert: "\nunsaved foo" } });
    expect(isDirty("/ws/open.ts")).toBe(true);

    await searched("foo", "bar", [matches("open.ts", match(1, 0, "foo", 3)), done(1, 1)]);
    fireEvent.click(screen.getByRole("button", { name: "Replace All" }));
    await tick();
    await confirmReplace();

    expect(view.state.doc.toString()).toBe("bar\nold line\nunsaved bar");
    expect(saved).toEqual([]);
    expect(replaceCalls).toEqual([]);
    expect(isDirty("/ws/open.ts")).toBe(true);
    expect(screen.getByText("Replaced 2 occurrences across 1 file. 1 open file was changed in the editor and left unsaved.")).toBeInTheDocument();
    view.destroy();
  });

  it("keeps dismissed matches of an open file untouched", async () => {
    const view = await openInEditor("/ws/open.ts", "foo foo\nfoo");
    await searched("foo", "bar", [matches("open.ts", match(1, 0, "foo foo", 3), match(1, 4, "foo foo", 3), match(2, 0, "foo", 3)), done(1, 3)]);
    fireEvent.click(screen.getByRole("button", { name: "Dismiss match on line 2" }));
    fireEvent.click(screen.getByRole("button", { name: "Replace All" }));
    await tick();
    await confirmReplace();
    expect(view.state.doc.toString()).toBe("bar bar\nfoo");
    view.destroy();
  });

  it("skips an open file when the regex cannot be reproduced in the editor, and says so", async () => {
    const view = await openInEditor("/ws/open.ts", "Foo foo");
    render(
      <>
        <SearchView />
        <OverlayHost />
      </>,
    );
    fireEvent.click(screen.getByRole("button", { name: "Toggle Replace" }));
    fireEvent.click(screen.getByRole("button", { name: "Use Regular Expression" }));
    fireEvent.change(screen.getByRole("textbox", { name: "Search" }), { target: { value: "(?i)foo" } });
    fireEvent.change(screen.getByRole("textbox", { name: "Replace" }), { target: { value: "bar" } });
    await tick(250);
    runs[0].send(matches("open.ts", match(1, 0, "Foo foo", 3), match(1, 4, "Foo foo", 3)));
    runs[0].send(matches("closed.ts", match(1, 0, "FOO", 3)));
    runs[0].send(done(2, 3));
    await tick();
    fireEvent.click(screen.getByRole("button", { name: "Replace All" }));
    await tick();
    await confirmReplace();

    expect(view.state.doc.toString()).toBe("Foo foo");
    expect(saved).toEqual([]);
    expect(replaceCalls.map((c) => c.files)).toEqual([["closed.ts"]]);
    expect(screen.getByText(/Skipped 1 open file: save and close it, then replace again\. Inline flags such as \(\?i\) cannot be applied inside the editor\./)).toBeInTheDocument();
    view.destroy();
  });

  it("falls back to the backend for a buffer that has no editor attached", async () => {
    fileText["/ws/held.ts"] = "foo";
    retainBuffer("/ws/held.ts");
    await ensureBuffer("/ws/held.ts");
    setBufferDoc("/ws/held.ts", Text.of(["foo"]));
    await searched("foo", "bar", [matches("held.ts", match(1, 0, "foo", 3)), done(1, 1)]);
    fireEvent.click(screen.getByRole("button", { name: "Replace All" }));
    await tick();
    await confirmReplace();
    expect(replaceCalls.map((c) => c.files)).toEqual([["held.ts"]]);
    expect(bufferText("/ws/held.ts")).toBe("foo");
  });
});
