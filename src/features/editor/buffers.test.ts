import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { clearMocks } from "@tauri-apps/api/mocks";
import { mockCommands, type OpenedFile } from "@/ipc";
import { ensureBuffer, getBuffer, handleExternalChange, isDirty, releaseBuffer, renameBuffer, resetBuffersForTest, retainBuffer, saveBuffer, setBufferDoc, useBuffers } from "./buffers";
import { Text } from "@codemirror/state";

const textFile = (text: string, mtimeMs = 1): OpenedFile => ({
  kind: "text",
  text,
  encoding: "UTF-8",
  bom: false,
  eol: "lf",
  stat: { size: text.length, mtimeMs, readOnly: false },
});

describe("buffer berkas", () => {
  beforeEach(() => resetBuffersForTest());
  afterEach(() => clearMocks());

  it("memuat isi dan menandai kotor hanya bila berbeda dari yang tersimpan", async () => {
    mockCommands({ fsOpen: () => textFile("satu\ndua\n") });
    retainBuffer("/p/a.txt");
    const buffer = await ensureBuffer("/p/a.txt");
    expect(buffer.doc.toString()).toBe("satu\ndua\n");
    expect(isDirty("/p/a.txt")).toBe(false);
    setBufferDoc("/p/a.txt", Text.of(["satu", "DUA", ""]));
    expect(isDirty("/p/a.txt")).toBe(true);
    // Kembali ke isi semula menghilangkan tanda kotor.
    setBufferDoc("/p/a.txt", Text.of(["satu", "dua", ""]));
    expect(isDirty("/p/a.txt")).toBe(false);
  });

  it("pasang-lepas-pasang cepat (StrictMode) tetap menghasilkan buffer yang terisi", async () => {
    let resolveFirst!: (file: OpenedFile) => void;
    let calls = 0;
    mockCommands({
      fsOpen: () => {
        calls += 1;
        return calls === 1 ? new Promise<OpenedFile>((resolve) => (resolveFirst = resolve)) : textFile("isi berkas\n");
      },
    });
    retainBuffer("/p/a.txt");
    const first = ensureBuffer("/p/a.txt");
    releaseBuffer("/p/a.txt");
    retainBuffer("/p/a.txt");
    const second = ensureBuffer("/p/a.txt");
    resolveFirst(textFile("isi berkas\n"));
    await first;
    const buffer = await second;
    expect(buffer).toBe(getBuffer("/p/a.txt"));
    expect(buffer.status).toBe("ready");
    expect(buffer.doc.toString()).toBe("isi berkas\n");
    expect(useBuffers.getState().meta["/p/a.txt"].status).toBe("ready");
  });

  it("buffer yang dilepas selagi dimuat tidak meninggalkan metadata", async () => {
    mockCommands({ fsOpen: () => textFile("x\n") });
    retainBuffer("/p/a.txt");
    const pending = ensureBuffer("/p/a.txt");
    releaseBuffer("/p/a.txt");
    await pending;
    expect(getBuffer("/p/a.txt")).toBeUndefined();
    expect(useBuffers.getState().meta["/p/a.txt"]).toBeUndefined();
  });

  it("menyimpan dengan encoding dan akhir baris asli", async () => {
    const saved: unknown[] = [];
    mockCommands({
      fsOpen: () => ({ ...textFile("a\nb\n"), encoding: "windows-1252", eol: "crlf", bom: false }),
      fsSave: (args) => {
        saved.push(args);
        return { size: 6, mtimeMs: 2, readOnly: false };
      },
    });
    retainBuffer("/p/a.txt");
    await ensureBuffer("/p/a.txt");
    setBufferDoc("/p/a.txt", Text.of(["a", "B", ""]));
    expect(await saveBuffer("/p/a.txt")).toBe(true);
    expect(saved).toEqual([{ path: "/p/a.txt", text: "a\nB\n", encoding: "windows-1252", bom: false, eol: "crlf" }]);
    expect(isDirty("/p/a.txt")).toBe(false);
  });

  it("perubahan dari luar: buffer bersih dimuat ulang, buffer kotor ditandai konflik", async () => {
    let disk = textFile("lama\n", 1);
    mockCommands({ fsOpen: () => disk, fsStat: () => (disk.kind === "text" ? disk.stat : null) });
    retainBuffer("/p/a.txt");
    await ensureBuffer("/p/a.txt");
    expect(await handleExternalChange("/p/a.txt")).toBe("unchanged");

    disk = textFile("baru dari luar\n", 2);
    expect(await handleExternalChange("/p/a.txt")).toBe("reloaded");
    expect(getBuffer("/p/a.txt")!.doc.toString()).toBe("baru dari luar\n");

    setBufferDoc("/p/a.txt", Text.of(["suntingan saya", ""]));
    disk = textFile("berubah lagi\n", 3);
    expect(await handleExternalChange("/p/a.txt")).toBe("conflict");
    expect(getBuffer("/p/a.txt")!.doc.toString()).toBe("suntingan saya\n");
    expect(useBuffers.getState().meta["/p/a.txt"].conflict).toBe(true);
  });

  it("rename memindahkan buffer beserta isi yang belum disimpan dan tidak melipatgandakan hitungan", async () => {
    mockCommands({ fsOpen: () => textFile("x\n") });
    retainBuffer("/p/lama.txt");
    await ensureBuffer("/p/lama.txt");
    setBufferDoc("/p/lama.txt", Text.of(["belum disimpan", ""]));
    renameBuffer("/p/lama.txt", "/p/baru.txt");
    // Tab dipasang ulang dengan path baru: yang lama melepas, yang baru menahan.
    releaseBuffer("/p/lama.txt");
    retainBuffer("/p/baru.txt");
    expect(getBuffer("/p/baru.txt")!.doc.toString()).toBe("belum disimpan\n");
    expect(isDirty("/p/baru.txt")).toBe(true);
    // Menutup satu-satunya tab melepas buffer sepenuhnya.
    releaseBuffer("/p/baru.txt");
    expect(getBuffer("/p/baru.txt")).toBeUndefined();
  });
});
