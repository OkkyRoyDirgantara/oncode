import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen, within } from "@testing-library/react";
import { clearMocks } from "@tauri-apps/api/mocks";
import { events, mockCommands, type AppInfo, type LspServerInfo, type Settings, type WorkspaceInfo } from "@/ipc";
import { registerCommand, resetCommandsForTest } from "@/app/commands";
import { OverlayHost, resetOverlayForTest } from "@/app/overlay";
import { useApp } from "@/app/store";
import { resetWorkbenchForTest, useWorkbench } from "@/app/workbench";
import { cloneSettings, defaults } from "./fixtures";
import { SettingsPage, TEXT_DEBOUNCE_MS } from "./SettingsTab";

const info: AppInfo = { version: "0.1.0", platform: "linux", git: null, gitError: null, recent: [], settingsPath: "/cfg/oncode/settings.json", keybindingsPath: "/config/keybindings.json", homeDir: "/home/me" };

const serverList: LspServerInfo[] = [
  { language: "rust", command: "rust-analyzer", args: [], installed: true, installHint: "rustup component add rust-analyzer" },
  { language: "typescript", command: "typescript-language-server", args: ["--stdio"], installed: false, installHint: "npm install -g typescript-language-server typescript" },
];

function workspace(overrides: Partial<WorkspaceInfo> = {}): WorkspaceInfo {
  return { root: "/work/project", name: "project", trusted: true, repoRoot: null, settings: cloneSettings(defaults), ...overrides };
}

interface Backend {
  user: Settings;
  saves: Settings[];
  servers: LspServerInfo[];
  failSave: boolean;
}

function setup(options: { user?: Settings; effective?: Settings; workspace?: WorkspaceInfo | null; defaultsFail?: boolean } = {}): Backend {
  const backend: Backend = { user: cloneSettings(options.user ?? defaults), saves: [], servers: serverList, failSave: false };
  mockCommands({
    settingsGetUser: () => cloneSettings(backend.user),
    settingsSet: ({ settings }) => {
      if (backend.failSave) throw { code: "io", message: "disk full", detail: null };
      backend.saves.push(cloneSettings(settings));
      backend.user = cloneSettings(settings);
      return null;
    },
    lspServers: () => backend.servers,
    settingsDefaults: () => {
      if (options.defaultsFail) throw { code: "io", message: "x", detail: null };
      return cloneSettings(defaults);
    },
  });
  useApp.setState({ info, workspace: options.workspace === undefined ? workspace() : options.workspace, settings: cloneSettings(options.effective ?? options.user ?? defaults) });
  return backend;
}

/** Menjalankan timer palsu sekaligus membiarkan promise IPC selesai. */
async function flush(ms = 0) {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(ms);
  });
}

async function renderPage() {
  const view = render(
    <>
      <SettingsPage />
      <OverlayHost />
    </>,
  );
  await flush();
  await flush();
  return view;
}

const row = (id: string) => document.querySelector(`[data-setting="${id}"]`) as HTMLElement | null;
const control = (id: string) => document.getElementById(`st-${id}`) as HTMLInputElement & HTMLSelectElement;
const visibleIds = () => [...document.querySelectorAll("[data-setting]")].map((el) => el.getAttribute("data-setting"));

/** Mengetik lalu menunggu jeda debounce. */
async function typeAndWait(id: string, text: string) {
  fireEvent.focus(control(id));
  fireEvent.change(control(id), { target: { value: text } });
  await flush(TEXT_DEBOUNCE_MS);
}

function patched(change: (s: Settings) => void, base: Settings = defaults): Settings {
  const next = cloneSettings(base);
  change(next);
  return next;
}

describe("SettingsPage", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    resetOverlayForTest();
    resetWorkbenchForTest();
    resetCommandsForTest();
  });
  afterEach(() => {
    vi.useRealTimers();
    clearMocks();
    useApp.setState({ info: null, workspace: null, settings: null });
  });

  it("menampilkan nilai pengguna (bukan nilai efektif proyek) di setiap kontrol", async () => {
    const user = patched((s) => {
      s.editor.fontSize = 16;
      s.git.path = "/opt/git/bin/git";
      s.files.autoSave = "afterDelay";
      s.editor.wordWrap = true;
    });
    setup({ user, effective: patched((s) => void (s.editor.fontSize = 20), user) });
    await renderPage();
    expect(control("editor.fontSize").value).toBe("16");
    expect(control("editor.fontFamily").value).toBe('"JetBrains Mono", monospace');
    expect(control("git.path").value).toBe("/opt/git/bin/git");
    expect(control("files.autoSave").value).toBe("afterDelay");
    expect(control("editor.wordWrap").checked).toBe(true);
    expect(control("editor.inlineBlame").checked).toBe(false);
    expect(control("terminal.fontSize").value).toBe("");
    expect(control("terminal.shell").placeholder).toBe("Default shell");
    // Setiap baris punya penjelasan.
    for (const el of document.querySelectorAll(".st-row")) expect(el.querySelector(".st-description")?.textContent?.length ?? 0).toBeGreaterThan(10);
  });

  describe("setiap kontrol menulis objek Settings yang diharapkan lewat settingsSet", () => {
    it.each([
      ["editor.insertSpaces", (s: Settings) => void (s.editor.insertSpaces = false)],
      ["editor.wordWrap", (s: Settings) => void (s.editor.wordWrap = true)],
      ["editor.lineNumbers", (s: Settings) => void (s.editor.lineNumbers = false)],
      ["editor.inlineBlame", (s: Settings) => void (s.editor.inlineBlame = true)],
      ["git.confirmForcePush", (s: Settings) => void (s.git.confirmForcePush = false)],
      ["lsp.enabled", (s: Settings) => void (s.lsp.enabled = false)],
    ])("kotak centang %s langsung disimpan", async (id, change) => {
      const backend = setup();
      await renderPage();
      fireEvent.click(control(id));
      // Tanpa jeda debounce: cukup membiarkan promise selesai.
      await flush();
      expect(backend.saves).toEqual([patched(change)]);
    });

    it.each([
      ["files.autoSave", "onFocusChange", (s: Settings) => void (s.files.autoSave = "onFocusChange")],
      ["files.autoSave", "afterDelay", (s: Settings) => void (s.files.autoSave = "afterDelay")],
      ["workbench.theme", "dark", (s: Settings) => void (s.workbench.theme = "dark")],
      ["workbench.theme", "light", (s: Settings) => void (s.workbench.theme = "light")],
      ["workbench.startupView", "explorer", (s: Settings) => void (s.workbench.startupView = "explorer")],
      ["git.pullStrategy", "merge", (s: Settings) => void (s.git.pullStrategy = "merge")],
      ["git.pullStrategy", "rebase", (s: Settings) => void (s.git.pullStrategy = "rebase")],
    ])("pilihan %s = %s langsung disimpan", async (id, value, change) => {
      const backend = setup();
      await renderPage();
      fireEvent.change(control(id), { target: { value } });
      await flush();
      expect(backend.saves).toEqual([patched(change)]);
    });

    it.each([
      ["editor.fontFamily", "  Fira Code, monospace ", (s: Settings) => void (s.editor.fontFamily = "Fira Code, monospace")],
      ["editor.fontSize", "18", (s: Settings) => void (s.editor.fontSize = 18)],
      ["editor.fontSize", "8", (s: Settings) => void (s.editor.fontSize = 8)],
      ["editor.fontSize", "40", (s: Settings) => void (s.editor.fontSize = 40)],
      ["editor.tabSize", "2", (s: Settings) => void (s.editor.tabSize = 2)],
      ["editor.tabSize", "16", (s: Settings) => void (s.editor.tabSize = 16)],
      ["git.path", " /opt/git/bin/git ", (s: Settings) => void (s.git.path = "/opt/git/bin/git")],
      ["git.autoFetchMinutes", "0", (s: Settings) => void (s.git.autoFetchMinutes = 0)],
      ["git.autoFetchMinutes", "30", (s: Settings) => void (s.git.autoFetchMinutes = 30)],
      ["terminal.shell", "/usr/bin/fish", (s: Settings) => void (s.terminal.shell = "/usr/bin/fish")],
      ["terminal.fontSize", "12", (s: Settings) => void (s.terminal.fontSize = 12)],
      ["forge.githubClientId", "Iv1.0123456789abcdef", (s: Settings) => void (s.forge.githubClientId = "Iv1.0123456789abcdef")],
      ["forge.gitlabClientId", "0123abcd", (s: Settings) => void (s.forge.gitlabClientId = "0123abcd")],
    ])("isian %s = %j disimpan setelah jeda", async (id, text, change) => {
      const backend = setup();
      await renderPage();
      await typeAndWait(id, text);
      expect(backend.saves).toEqual([patched(change)]);
    });

    it("mengosongkan kolom yang boleh kosong menyimpan null", async () => {
      const user = patched((s) => {
        s.git.path = "/opt/git";
        s.terminal.shell = "/bin/zsh";
        s.terminal.fontSize = 12;
        s.forge.githubClientId = "abc";
      });
      const backend = setup({ user });
      await renderPage();
      await typeAndWait("git.path", "");
      await typeAndWait("terminal.shell", "   ");
      await typeAndWait("terminal.fontSize", "");
      await typeAndWait("forge.githubClientId", "");
      expect(backend.saves).toHaveLength(4);
      expect(backend.saves[3]).toEqual(defaults);
    });
  });

  it("isian teks di-debounce: ketikan beruntun hanya menyimpan sekali, dengan nilai terakhir", async () => {
    const backend = setup();
    await renderPage();
    const input = control("editor.fontFamily");
    fireEvent.focus(input);
    fireEvent.change(input, { target: { value: "F" } });
    await flush(TEXT_DEBOUNCE_MS - 100);
    fireEvent.change(input, { target: { value: "Fi" } });
    await flush(TEXT_DEBOUNCE_MS - 100);
    fireEvent.change(input, { target: { value: "Fira Code" } });
    await flush(TEXT_DEBOUNCE_MS - 1);
    expect(backend.saves).toEqual([]);
    await flush(1);
    expect(backend.saves).toEqual([patched((s) => void (s.editor.fontFamily = "Fira Code"))]);
    // Tidak ada penyimpanan susulan.
    await flush(5000);
    expect(backend.saves).toHaveLength(1);
  });

  it("meninggalkan kolom (blur) atau menekan Enter menyimpan tanpa menunggu jeda", async () => {
    const backend = setup();
    await renderPage();
    fireEvent.focus(control("editor.tabSize"));
    fireEvent.change(control("editor.tabSize"), { target: { value: "8" } });
    fireEvent.blur(control("editor.tabSize"));
    await flush();
    expect(backend.saves).toEqual([patched((s) => void (s.editor.tabSize = 8))]);

    fireEvent.focus(control("terminal.shell"));
    fireEvent.change(control("terminal.shell"), { target: { value: "/bin/zsh" } });
    fireEvent.keyDown(control("terminal.shell"), { key: "Enter" });
    await flush();
    expect(backend.saves).toHaveLength(2);
    expect(backend.saves[1].terminal.shell).toBe("/bin/zsh");
    await flush(5000);
    expect(backend.saves).toHaveLength(2);
  });

  it("nilai yang sama dengan yang tersimpan tidak disimpan ulang", async () => {
    const backend = setup();
    await renderPage();
    await typeAndWait("editor.fontSize", "014");
    await typeAndWait("git.path", "   ");
    expect(backend.saves).toEqual([]);
    fireEvent.blur(control("editor.fontSize"));
    await flush();
    expect(control("editor.fontSize").value).toBe("14");
  });

  it.each([
    ["editor.fontSize", "7", /from 8 to 40/],
    ["editor.fontSize", "41", /from 8 to 40/],
    ["editor.fontSize", "", /Enter a number/],
    ["editor.fontSize", "12.5", /whole number/],
    ["editor.fontSize", "big", /whole number/],
    ["editor.tabSize", "0", /from 1 to 16/],
    ["editor.tabSize", "17", /from 1 to 16/],
    ["git.autoFetchMinutes", "-1", /whole number/],
    ["terminal.fontSize", "200", /from 8 to 40/],
    ["editor.fontFamily", "   ", /at least one font/],
    ["forge.githubClientId", "has space", /cannot contain spaces/],
  ])("validasi: %s = %j menampilkan pesan dan tidak disimpan", async (id, text, message) => {
    const backend = setup();
    await renderPage();
    await typeAndWait(id, text);
    fireEvent.blur(control(id));
    await flush(5000);
    expect(backend.saves).toEqual([]);
    const alert = within(row(id)!).getByRole("alert");
    expect(alert).toHaveTextContent(message);
    expect(alert).toHaveTextContent("not saved");
    expect(control(id)).toHaveAttribute("aria-invalid", "true");
    // Isian yang salah tetap terlihat agar bisa diperbaiki.
    expect(control(id).value).toBe(text);

    // Setelah diperbaiki, pesan hilang dan nilai tersimpan.
    const fixed = id === "editor.fontFamily" ? "monospace" : id === "forge.githubClientId" ? "abc" : "10";
    await typeAndWait(id, fixed);
    expect(within(row(id)!).queryByRole("alert")).toBeNull();
    expect(backend.saves).toHaveLength(1);
  });

  it("penyimpanan berurutan dan saling membangun, bukan saling menimpa", async () => {
    const backend = setup();
    await renderPage();
    fireEvent.click(control("editor.wordWrap"));
    fireEvent.change(control("workbench.theme"), { target: { value: "light" } });
    fireEvent.focus(control("editor.tabSize"));
    fireEvent.change(control("editor.tabSize"), { target: { value: "2" } });
    await flush(TEXT_DEBOUNCE_MS);
    expect(backend.saves).toHaveLength(3);
    expect(backend.saves[0]).toEqual(patched((s) => void (s.editor.wordWrap = true)));
    expect(backend.saves[2]).toEqual(
      patched((s) => {
        s.editor.wordWrap = true;
        s.workbench.theme = "light";
        s.editor.tabSize = 2;
      }),
    );
  });

  it("kotak pencarian menyembunyikan baris dan bagian yang tidak cocok", async () => {
    setup();
    await renderPage();
    expect(visibleIds()).toHaveLength(21);
    const search = screen.getByLabelText("Search settings");

    fireEvent.change(search, { target: { value: "font size" } });
    expect(visibleIds()).toEqual(["editor.fontSize", "terminal.fontSize"]);
    expect(screen.getAllByRole("heading", { level: 2 }).map((h) => h.textContent)).toEqual(["Editor", "Terminal"]);
    expect(row("editor.tabSize")).toBeNull();

    fireEvent.change(search, { target: { value: "rebase" } });
    expect(visibleIds()).toEqual(["git.pullStrategy"]);

    // Baris tabel server ikut disaring menurut bahasa/perintah.
    fireEvent.change(search, { target: { value: "typescript" } });
    expect(visibleIds()).toEqual(["lsp.servers"]);
    expect([...document.querySelectorAll("tr[data-language]")].map((tr) => tr.getAttribute("data-language"))).toEqual(["typescript"]);

    fireEvent.change(search, { target: { value: "language servers" } });
    expect(visibleIds()).toEqual(["lsp.enabled", "lsp.servers"]);
    expect(document.querySelectorAll("tr[data-language]")).toHaveLength(2);

    fireEvent.change(search, { target: { value: "zzz-nothing" } });
    expect(visibleIds()).toEqual([]);
    expect(screen.getByText(/No settings match/)).toBeInTheDocument();

    fireEvent.change(search, { target: { value: "" } });
    expect(visibleIds()).toHaveLength(21);
  });

  describe("penanda ditimpa proyek", () => {
    const effective = patched((s) => {
      s.editor.tabSize = 2;
      s.git.pullStrategy = "rebase";
      s.lsp.servers = { rust: { command: "ra-project" } };
    });
    const marked = () => [...document.querySelectorAll(".st-override")].map((el) => el.closest("[data-setting]")?.getAttribute("data-setting") + ":" + (el.closest("tr")?.getAttribute("data-language") ?? ""));

    it("muncul hanya pada kolom yang nilainya berbeda dari pengaturan efektif", async () => {
      setup({ effective });
      await renderPage();
      expect(marked()).toEqual(["editor.tabSize:", "git.pullStrategy:", "lsp.servers:rust"]);
      expect(within(row("editor.tabSize")!).getByText("overridden by project (.oncode/settings.json)")).toBeInTheDocument();
      // Kontrol tetap menampilkan nilai pengguna.
      expect(control("editor.tabSize").value).toBe("4");
    });

    it("tidak muncul di folder yang tidak dipercaya atau tanpa folder", async () => {
      setup({ effective, workspace: workspace({ trusted: false }) });
      const view = await renderPage();
      expect(marked()).toEqual([]);
      view.unmount();
      setup({ effective, workspace: null });
      await renderPage();
      expect(marked()).toEqual([]);
    });

    it("hilang saat nilai pengguna disamakan, dan mengikuti perubahan pengaturan efektif", async () => {
      setup({ effective });
      await renderPage();
      await typeAndWait("editor.tabSize", "2");
      expect(marked()).toEqual(["git.pullStrategy:", "lsp.servers:rust"]);
      act(() => useApp.setState({ settings: patched((s) => void (s.editor.tabSize = 2)) }));
      expect(marked()).toEqual([]);
    });
  });

  describe("perubahan dari luar (settingsChanged)", () => {
    it("memperbarui kolom lain tetapi tidak menimpa kolom yang sedang diketik", async () => {
      const backend = setup();
      await renderPage();
      const family = control("editor.fontFamily");
      fireEvent.focus(family);
      fireEvent.change(family, { target: { value: "Fira Co" } });
      await flush(100);

      // Jendela lain mengubah tema, ukuran tab, DAN font yang sedang diketik di sini.
      backend.user = patched((s) => {
        s.workbench.theme = "light";
        s.editor.tabSize = 8;
        s.editor.fontFamily = "Other Window Font";
      });
      await act(async () => {
        await events.settingsChanged.emit({ settings: cloneSettings(backend.user) });
      });
      await flush();

      expect(control("workbench.theme").value).toBe("light");
      expect(control("editor.tabSize").value).toBe("8");
      expect(family.value).toBe("Fira Co");
      expect(backend.saves).toEqual([]);

      // Ketikan dilanjutkan; yang tersimpan memuat perubahan dari luar plus kolom yang diketik.
      fireEvent.change(family, { target: { value: "Fira Code" } });
      await flush(TEXT_DEBOUNCE_MS);
      expect(backend.saves).toEqual([
        patched((s) => {
          s.workbench.theme = "light";
          s.editor.tabSize = 8;
          s.editor.fontFamily = "Fira Code";
        }),
      ]);
      expect(family.value).toBe("Fira Code");
    });

    it("kolom yang hanya difokus (belum disunting) mengikuti nilai luar setelah fokus lepas", async () => {
      const backend = setup();
      await renderPage();
      const size = control("editor.fontSize");
      fireEvent.focus(size);
      backend.user = patched((s) => void (s.editor.fontSize = 22));
      await act(async () => {
        await events.settingsChanged.emit({ settings: cloneSettings(backend.user) });
      });
      await flush();
      expect(size.value).toBe("14");
      fireEvent.blur(size);
      await flush();
      expect(size.value).toBe("22");
      expect(backend.saves).toEqual([]);
    });

    it("kolom yang tidak difokus langsung mengikuti nilai luar, termasuk kotak centang dan pilihan", async () => {
      const backend = setup();
      await renderPage();
      backend.user = patched((s) => {
        s.editor.fontSize = 19;
        s.editor.lineNumbers = false;
        s.git.pullStrategy = "merge";
        s.git.path = "/x/git";
      });
      await act(async () => {
        await events.settingsChanged.emit({ settings: cloneSettings(backend.user) });
      });
      await flush();
      expect(control("editor.fontSize").value).toBe("19");
      expect(control("editor.lineNumbers").checked).toBe(false);
      expect(control("git.pullStrategy").value).toBe("merge");
      expect(control("git.path").value).toBe("/x/git");
      expect(backend.saves).toEqual([]);
    });
  });

  describe("tabel language server", () => {
    const tr = (language: string) => document.querySelector(`tr[data-language="${language}"]`) as HTMLElement;

    it("menampilkan bahasa, perintah, status terpasang, dan petunjuk pemasangan", async () => {
      setup();
      await renderPage();
      expect(within(tr("rust")).getByText("rust-analyzer")).toBeInTheDocument();
      expect(within(tr("rust")).getByText("Installed")).toBeInTheDocument();
      expect(within(tr("typescript")).getByText("typescript-language-server --stdio")).toBeInTheDocument();
      expect(within(tr("typescript")).getByText("Not found")).toBeInTheDocument();
      expect(within(tr("typescript")).getByText("npm install -g typescript-language-server typescript")).toBeInTheDocument();
      expect(within(tr("typescript")).getByText("Not found").closest("[title]")).toHaveAttribute("title", "npm install -g typescript-language-server typescript");
      expect(screen.getByText(/Language servers are not bundled with OnCode/)).toBeInTheDocument();
    });

    it("Customize membuat penimpaan; perintah dan argumen tersimpan di lsp.servers[bahasa]; Reset menghapusnya", async () => {
      const backend = setup();
      await renderPage();
      fireEvent.click(within(tr("typescript")).getByRole("button", { name: "Customize" }));
      await flush();
      expect(backend.saves).toEqual([patched((s) => void (s.lsp.servers = { typescript: { command: "typescript-language-server", args: ["--stdio"] } }))]);

      const command = within(tr("typescript")).getByLabelText("Command for typescript") as HTMLInputElement;
      const args = within(tr("typescript")).getByLabelText("Arguments for typescript") as HTMLInputElement;
      expect(command.value).toBe("typescript-language-server");
      expect(args.value).toBe("--stdio");

      fireEvent.focus(command);
      fireEvent.change(command, { target: { value: "/opt/node/bin/tsls" } });
      await flush(TEXT_DEBOUNCE_MS);
      fireEvent.blur(command);
      fireEvent.focus(args);
      fireEvent.change(args, { target: { value: '--stdio --log "/tmp/my logs/ts.log"' } });
      await flush(TEXT_DEBOUNCE_MS);
      expect(backend.saves).toHaveLength(3);
      expect(backend.saves[2]).toEqual(patched((s) => void (s.lsp.servers = { typescript: { command: "/opt/node/bin/tsls", args: ["--stdio", "--log", "/tmp/my logs/ts.log"] } })));

      // Tidak sah: perintah kosong atau kutip tidak ditutup.
      fireEvent.change(args, { target: { value: '--log "unclosed' } });
      await flush(TEXT_DEBOUNCE_MS);
      fireEvent.blur(args);
      fireEvent.focus(command);
      fireEvent.change(command, { target: { value: "  " } });
      await flush(TEXT_DEBOUNCE_MS);
      expect(backend.saves).toHaveLength(3);
      expect(within(tr("typescript")).getAllByRole("alert").map((a) => a.textContent)).toEqual(["Enter a command — not saved", "A quote is not closed — not saved"]);

      fireEvent.click(within(tr("typescript")).getByRole("button", { name: "Reset" }));
      await flush();
      expect(backend.saves).toHaveLength(4);
      expect(backend.saves[3]).toEqual(defaults);
      expect(within(tr("typescript")).getByRole("button", { name: "Customize" })).toBeInTheDocument();
    });

    it("penimpaan yang sudah ada mempertahankan env saat perintah diubah", async () => {
      const user = patched((s) => void (s.lsp.servers = { rust: { command: "ra", args: ["--x"], env: [["RA_LOG", "info"]] } }));
      const backend = setup({ user });
      await renderPage();
      const command = within(tr("rust")).getByLabelText("Command for rust");
      fireEvent.focus(command);
      fireEvent.change(command, { target: { value: "ra-nightly" } });
      await flush(TEXT_DEBOUNCE_MS);
      expect(backend.saves).toEqual([patched((s) => void (s.lsp.servers = { rust: { command: "ra-nightly", args: ["--x"], env: [["RA_LOG", "info"]] } }))]);
    });
  });

  describe("reset ke nilai bawaan", () => {
    const resetButton = (id: string) => row(id)!.querySelector<HTMLButtonElement>(".st-reset");
    const changed = patched((s) => {
      s.editor.fontSize = 18;
      s.editor.wordWrap = true;
      s.workbench.theme = "dark";
      s.git.path = "/opt/git";
      s.terminal.fontSize = 12;
      s.lsp.servers = { rust: { command: "ra-custom", args: [] } };
      s.forge.githubClientId = "abc";
    });

    it("tombol reset per kolom hanya muncul pada kolom yang berbeda dari bawaan", async () => {
      setup({ user: changed });
      await renderPage();
      expect([...document.querySelectorAll(".st-reset")].map((b) => b.closest("[data-setting]")?.getAttribute("data-setting"))).toEqual([
        "editor.fontSize",
        "editor.wordWrap",
        "workbench.theme",
        "git.path",
        "terminal.fontSize",
        "forge.githubClientId",
      ]);
      expect(resetButton("editor.fontSize")).toHaveAttribute("title", "Reset to default (14)");
      expect(resetButton("editor.fontSize")).toHaveAccessibleName("Reset Font size to default");
      expect(resetButton("workbench.theme")).toHaveAttribute("title", "Reset to default (System)");
      expect(resetButton("editor.wordWrap")).toHaveAttribute("title", "Reset to default (off)");
      expect(resetButton("git.path")).toHaveAttribute("title", "Reset to default (empty)");
      expect(resetButton("editor.tabSize")).toBeNull();
    });

    it.each([
      ["editor.fontSize", (s: Settings) => void (s.editor.fontSize = 14)],
      ["editor.wordWrap", (s: Settings) => void (s.editor.wordWrap = false)],
      ["workbench.theme", (s: Settings) => void (s.workbench.theme = "system")],
      ["git.path", (s: Settings) => void (s.git.path = null)],
      ["terminal.fontSize", (s: Settings) => void (s.terminal.fontSize = null)],
      ["forge.githubClientId", (s: Settings) => void (s.forge.githubClientId = null)],
    ])("reset %s hanya mengembalikan kolom itu, lalu tombolnya hilang", async (id, change) => {
      const backend = setup({ user: changed });
      await renderPage();
      fireEvent.click(resetButton(id)!);
      await flush();
      expect(backend.saves).toEqual([patched(change, changed)]);
      expect(resetButton(id)).toBeNull();
      // Kolom lain tetap punya tombolnya.
      expect(document.querySelectorAll(".st-reset")).toHaveLength(5);
      const field: HTMLInputElement = control(id);
      if (field.type === "checkbox") expect(field.checked).toBe(false);
      else expect(field.value).toBe(id === "editor.fontSize" ? "14" : id === "workbench.theme" ? "system" : "");
    });

    it("tombol muncul begitu nilai diubah, dan reset membuang isian tidak sah yang sedang ditampilkan", async () => {
      const backend = setup();
      await renderPage();
      expect(document.querySelectorAll(".st-reset")).toHaveLength(0);
      await typeAndWait("editor.tabSize", "8");
      expect(resetButton("editor.tabSize")).not.toBeNull();
      // Isian berikutnya tidak sah: tidak disimpan, tetapi masih terlihat beserta pesannya.
      await typeAndWait("editor.tabSize", "99");
      fireEvent.blur(control("editor.tabSize"));
      expect(within(row("editor.tabSize")!).getByRole("alert")).toBeInTheDocument();

      fireEvent.click(resetButton("editor.tabSize")!);
      await flush();
      expect(control("editor.tabSize").value).toBe("4");
      expect(within(row("editor.tabSize")!).queryByRole("alert")).toBeNull();
      expect(backend.saves).toEqual([patched((s) => void (s.editor.tabSize = 8)), defaults]);
      await flush(5000);
      expect(backend.saves).toHaveLength(2);
    });

    it("Reset to defaults: meminta konfirmasi (aksi berbahaya); batal tidak mengubah apa pun", async () => {
      const backend = setup({ user: changed });
      await renderPage();
      fireEvent.click(screen.getByRole("button", { name: /Reset to defaults/ }));
      const dialog = screen.getByRole("dialog");
      // Enam kolom + satu penimpaan language server.
      expect(dialog).toHaveTextContent("Reset all settings to their defaults?");
      expect(dialog).toHaveTextContent("7 settings you changed will return to their defaults");
      expect(within(dialog).getByRole("button", { name: "Reset Settings" })).toHaveClass("oc-btn-danger");
      fireEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));
      await flush();
      expect(backend.saves).toEqual([]);
      expect(control("editor.fontSize").value).toBe("18");
    });

    it("Reset to defaults: setelah konfirmasi menyimpan persis nilai bawaan dan memperbarui semua kontrol", async () => {
      const backend = setup({ user: changed });
      await renderPage();
      // Ada isian tidak sah yang masih terlihat; reset juga membuangnya.
      await typeAndWait("editor.tabSize", "0");
      fireEvent.blur(control("editor.tabSize"));
      expect(document.querySelector('tr[data-language="rust"] input')).not.toBeNull();

      fireEvent.click(screen.getByRole("button", { name: /Reset to defaults/ }));
      fireEvent.click(within(screen.getByRole("dialog")).getByRole("button", { name: "Reset Settings" }));
      await flush();
      await flush();
      expect(backend.saves).toEqual([defaults]);
      expect(control("editor.fontSize").value).toBe("14");
      expect(control("editor.tabSize").value).toBe("4");
      expect(control("editor.wordWrap").checked).toBe(false);
      expect(control("workbench.theme").value).toBe("system");
      expect(control("git.path").value).toBe("");
      expect(control("forge.githubClientId").value).toBe("");
      expect(document.querySelector('tr[data-language="rust"] input')).toBeNull();
      expect(document.querySelectorAll(".st-reset")).toHaveLength(0);
      expect(document.querySelectorAll('[role="alert"]')).toHaveLength(0);
      expect(screen.getByRole("button", { name: /Reset to defaults/ })).toBeDisabled();
    });

    it("Reset to defaults nonaktif bila semua sudah bawaan; penimpaan server saja sudah mengaktifkannya", async () => {
      const backend = setup();
      await renderPage();
      expect(screen.getByRole("button", { name: /Reset to defaults/ })).toBeDisabled();
      fireEvent.click(within(document.querySelector('tr[data-language="rust"]') as HTMLElement).getByRole("button", { name: "Customize" }));
      await flush();
      expect(backend.saves).toHaveLength(1);
      expect(screen.getByRole("button", { name: /Reset to defaults/ })).toBeEnabled();
      expect(screen.getByRole("button", { name: /Reset to defaults/ })).toHaveAttribute("title", "Reset 1 changed setting");
    });

    it("tanpa nilai bawaan dari backend, tidak ada tombol reset tetapi formulir tetap bekerja", async () => {
      const backend = setup({ user: changed, defaultsFail: true });
      await renderPage();
      expect(document.querySelectorAll(".st-reset")).toHaveLength(0);
      expect(screen.queryByRole("button", { name: /Reset to defaults/ })).toBeNull();
      fireEvent.click(control("editor.lineNumbers"));
      await flush();
      expect(backend.saves).toEqual([patched((s) => void (s.editor.lineNumbers = false), changed)]);
    });
  });

  it("Keyboard Shortcuts: tombol menjalankan command keybindings.openJson", async () => {
    let runs = 0;
    registerCommand({ id: "keybindings.openJson", title: "Open Keyboard Shortcuts (JSON)", run: () => void (runs += 1) });
    setup();
    await renderPage();
    const button = within(row("keybindings")!).getByRole("button", { name: /Open keybindings\.json/ });
    expect(button).toHaveAttribute("title", "/config/keybindings.json");
    fireEvent.click(button);
    await flush();
    expect(runs).toBe(1);

    fireEvent.change(screen.getByLabelText("Search settings"), { target: { value: "keyboard shortcuts" } });
    expect(visibleIds()).toEqual(["keybindings"]);
  });

  it("Keyboard Shortcuts: command yang belum terdaftar menghasilkan galat yang terlihat, bukan diam", async () => {
    setup();
    await renderPage();
    fireEvent.click(within(row("keybindings")!).getByRole("button", { name: /Open keybindings\.json/ }));
    await flush();
    expect(document.querySelector('.oc-toast[data-kind="error"]')).toHaveTextContent(/Could not open keybindings\.json/);
  });

  it("tanpa folder terbuka: formulir bekerja penuh dan tidak ada penanda proyek", async () => {
    const backend = setup({ workspace: null, effective: patched((s) => void (s.editor.tabSize = 2)) });
    await renderPage();
    expect(document.querySelectorAll(".st-override")).toHaveLength(0);
    expect(visibleIds()).toHaveLength(21);
    fireEvent.click(control("editor.wordWrap"));
    await typeAndWait("editor.fontSize", "16");
    expect(backend.saves).toHaveLength(2);
    expect(backend.saves[1]).toEqual(
      patched((s) => {
        s.editor.wordWrap = true;
        s.editor.fontSize = 16;
      }),
    );
    expect(document.querySelectorAll(".st-override")).toHaveLength(0);
  });

  it("Open settings.json membuka berkas pengaturan di editor", async () => {
    setup();
    await renderPage();
    fireEvent.click(screen.getByRole("button", { name: /Open settings\.json/ }));
    const tabs = useWorkbench.getState().groups[0].tabs;
    expect(tabs).toHaveLength(1);
    expect(tabs[0]).toMatchObject({ kind: "file", params: { path: "/cfg/oncode/settings.json" } });
  });

  it("gagal menyimpan: galat ditampilkan dan tampilan kembali ke yang tersimpan", async () => {
    const backend = setup();
    await renderPage();
    backend.failSave = true;
    fireEvent.click(control("editor.wordWrap"));
    await flush();
    await flush();
    expect(screen.getByText("Could not save settings: disk full")).toBeInTheDocument();
    expect(control("editor.wordWrap").checked).toBe(false);
  });

  it("suntingan yang masih menunggu ikut disimpan saat halaman ditutup", async () => {
    const backend = setup();
    const view = await renderPage();
    fireEvent.focus(control("editor.tabSize"));
    fireEvent.change(control("editor.tabSize"), { target: { value: "3" } });
    view.unmount();
    await flush();
    expect(backend.saves).toEqual([patched((s) => void (s.editor.tabSize = 3))]);
  });
});
