import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import type { LSPClient } from "@codemirror/lsp-client";
import { OverlayHost, resetOverlayForTest } from "@/app/overlay";
import { skippedRenameUris, warnAboutSkippedRenameEdits } from "./rename";
import { showReferences, toReferences } from "./references";

const openFile = vi.hoisted(() => vi.fn());
vi.mock("@/app/coreCommands", () => ({ openFile }));

const edit = (line: number) => ({ range: { start: { line, character: 0 }, end: { line, character: 3 } }, newText: "new" });
const open = new Set(["file:///proj/open.rs"]);
const isOpen = (uri: string) => open.has(uri);

describe("rename: berkas yang terlewat", () => {
  beforeEach(() => openFile.mockClear());
  afterEach(() => resetOverlayForTest());

  it("tidak ada yang terlewat bila semua berkas terbuka", () => {
    expect(skippedRenameUris({ changes: { "file:///proj/open.rs": [edit(1)] } }, isOpen)).toEqual([]);
    expect(skippedRenameUris(null, isOpen)).toEqual([]);
    expect(skippedRenameUris({}, isOpen)).toEqual([]);
  });

  it("berkas yang tidak terbuka dilaporkan; daftar suntingan kosong tidak dihitung", () => {
    expect(
      skippedRenameUris({ changes: { "file:///proj/open.rs": [edit(1)], "file:///proj/closed.rs": [edit(2)], "file:///proj/untouched.rs": [] } }, isOpen),
    ).toEqual(["file:///proj/closed.rs"]);
  });

  it("suntingan yang hanya ada di documentChanges tidak diterapkan pustaka, jadi semuanya dilaporkan", () => {
    const result = skippedRenameUris(
      { documentChanges: [{ textDocument: { uri: "file:///proj/open.rs" }, edits: [edit(1)] }, { textDocument: { uri: "file:///proj/closed.rs" }, edits: [edit(2)] }] },
      isOpen,
    );
    expect(result).toEqual(["file:///proj/open.rs", "file:///proj/closed.rs"]);
  });

  it("menampilkan peringatan dengan nama berkas dan aksi untuk membukanya", () => {
    render(<OverlayHost />);
    const client = { workspace: { getFile: (uri: string) => (isOpen(uri) ? {} : null) } } as unknown as LSPClient;
    warnAboutSkippedRenameEdits(client, { changes: { "file:///proj/open.rs": [edit(1)] } });
    expect(screen.queryByText(/Rename only changed/)).toBeNull();

    warnAboutSkippedRenameEdits(client, { changes: { "file:///proj/open.rs": [edit(1)], "file:///proj/src/closed%20one.rs": [edit(2)] } });
    return screen.findByText(/1 other file still uses the old name: closed one\.rs\./).then(() => {
      screen.getByRole("button", { name: "Open Files" }).click();
      expect(openFile).toHaveBeenCalledWith("/proj/src/closed one.rs");
    });
  });
});

describe("references", () => {
  const loc = (uri: string, line: number, character: number) => ({ uri, range: { start: { line, character }, end: { line, character: character + 3 } } });

  beforeEach(() => openFile.mockClear());
  afterEach(() => resetOverlayForTest());

  it("mengurutkan menurut berkas lalu posisi dan membuang duplikat", () => {
    expect(
      toReferences([loc("file:///proj/b.rs", 4, 2), loc("file:///proj/a%20x.rs", 9, 0), loc("file:///proj/b.rs", 1, 7), loc("file:///proj/b.rs", 4, 2), loc("file:///proj/b.rs", 1, 3)]),
    ).toEqual([
      { path: "/proj/a x.rs", line: 10, column: 0 },
      { path: "/proj/b.rs", line: 2, column: 3 },
      { path: "/proj/b.rs", line: 2, column: 7 },
      { path: "/proj/b.rs", line: 5, column: 2 },
    ]);
  });

  it("tanpa hasil hanya memberi tahu", async () => {
    render(<OverlayHost />);
    await showReferences([]);
    expect(await screen.findByText("No references found.")).toBeInTheDocument();
    expect(openFile).not.toHaveBeenCalled();
  });
});
