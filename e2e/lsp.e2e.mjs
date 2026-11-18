// LSP dengan language server sungguhan (typescript-language-server). Dilewati bila server belum dipasang:
//   pnpm --dir e2e/fixtures/ts-server install --ignore-workspace
import { existsSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { Key } from "./driver.mjs";

const server = join(dirname(fileURLToPath(import.meta.url)), "fixtures", "ts-server", "node_modules", ".bin", "typescript-language-server");

function tsProject(t) {
  const repo = t.repo();
  t.write(repo, "src/hitung.ts", "export function tambah(a: number, b: number): number {\n  return a + b;\n}\n");
  t.write(repo, "src/app.ts", 'import { tambah } from "./hitung";\n\nconst hasil: string = tambah(1, 2);\nconsole.log(hasil);\n');
  t.write(repo, "tsconfig.json", '{ "compilerOptions": { "strict": true, "module": "esnext", "moduleResolution": "bundler" }, "include": ["src"] }\n');
  t.commitAll(repo, "Proyek TypeScript");
  return repo;
}

async function openFile(app, query, name) {
  await app.shortcut("p", { ctrl: true });
  await app.waitForSelector(".oc-palette-input");
  await app.type(".oc-palette-input", query);
  await app.waitForText(".oc-palette-item", name);
  await app.keys(Key.Enter);
  await app.waitForText(".oc-tab-label", name);
}

export const tests = {
  async "diagnostik dari server muncul di editor dan panel Problems"(t) {
    if (!existsSync(server)) t.skip("typescript-language-server belum dipasang");
    const repo = tsProject(t);
    const app = await t.launch(repo);
    await openFile(app, "appts", "app.ts");
    await app.waitForText(".oc-statusbar", "typescript-language-server", { timeout: 20000 });
    // Baris 3 menetapkan number ke string: server harus melaporkannya.
    await app.waitForSelector(".oc-group .cm-lintRange-error", { timeout: 30000 });
    await app.waitFor("ringkasan masalah di status bar", "return [...document.querySelectorAll('.oc-status-item')].some((e) => /^1\\s*0$/.test(e.textContent.trim()))", { timeout: 15000 });

    await app.shortcut("j", { ctrl: true });
    await app.waitForSelector(".oc-panel-tab");
    await app.clickText(".oc-panel-tab", "Problems");
    await app.waitForText(".oc-panel-body", "not assignable to type 'string'", { timeout: 15000 });
    await app.waitForText(".oc-panel-body", "app.ts");
  },

  async "completion dan go to definition lintas berkas"(t) {
    if (!existsSync(server)) t.skip("typescript-language-server belum dipasang");
    const repo = tsProject(t);
    const app = await t.launch(repo);
    await openFile(app, "appts", "app.ts");
    await app.waitForSelector(".oc-group .cm-lintRange-error", { timeout: 30000 });

    // Ketik di akhir dokumen dan minta saran: fungsi yang diimpor harus ditawarkan oleh server.
    await app.click(".oc-group .cm-content");
    await app.keys(Key.Ctrl, Key.End);
    await app.type(".oc-group .cm-content", "tam");
    await app.waitForText(".cm-tooltip-autocomplete", "tambah", { timeout: 20000 });
    await app.keys(Key.Escape);

    // Kursor pada pemakaian `tambah` di baris 3 (lewat keyboard), lalu F12 membuka berkas definisinya.
    await app.keys(Key.Ctrl, Key.Home);
    await app.keys(Key.Down);
    await app.keys(Key.Down);
    await app.keys(Key.Home);
    for (let i = 0; i < "const hasil: string = ta".length; i++) await app.keys(Key.Right);
    await app.waitForText(".oc-statusbar", "Ln 3, Col 25");
    await app.keys(Key.F12);
    await app.waitForText(".oc-tab-label", "hitung.ts", { timeout: 20000 });
    await app.waitForText(".oc-statusbar", "Ln 1", { timeout: 10000 });
  },
};
