// Editor, Explorer, pencarian, terminal, pengaturan, dan pemulihan sesi di aplikasi sungguhan.
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { Key } from "./driver.mjs";

function project(t) {
  const repo = t.repo();
  t.write(repo, "README.md", "# Demo\n\nBaris satu\n");
  t.write(repo, "src/main.rs", 'fn main() {\n    println!("halo");\n}\n');
  t.write(repo, "src/util.rs", "pub fn util() {}\n");
  t.write(repo, ".gitignore", "target/\n");
  t.write(repo, "target/build.log", "println! tidak boleh ditemukan di sini\n");
  t.commitAll(repo, "Commit pertama");
  return repo;
}

async function waitForFile(t, path, predicate, description) {
  for (let i = 0; i < 60; i++) {
    if (existsSync(path) && predicate(readFileSync(path, "utf8"))) return readFileSync(path, "utf8");
    await t.sleep(150);
  }
  throw new Error(`Menunggu terlalu lama: ${description} (${existsSync(path) ? JSON.stringify(readFileSync(path, "utf8")) : "berkas tidak ada"})`);
}

const treeItem = (name) => `[role="treeitem"][data-path$="${name}"]`;

export const tests = {
  async "explorer membuka berkas; sunting, simpan, dan penanda Git mengikuti"(t) {
    const repo = project(t);
    const app = await t.launch(repo);
    await app.shortcut("E", { ctrl: true, shift: true });
    await app.waitForSelector(treeItem("README.md"));
    t.assert(await app.exists(treeItem("src")), "folder src tampil");
    t.assert(!(await app.exists(treeItem("main.rs"))), "isi folder belum dimuat sebelum dibuka");

    await app.clickText('[role="treeitem"]', "src");
    await app.waitForSelector(treeItem("main.rs"));
    await app.exec("document.querySelector(arguments[0]).dispatchEvent(new MouseEvent('dblclick', { bubbles: true }))", treeItem("README.md"));
    await app.waitForText(".oc-tab-label", "README.md");
    await app.waitForText(".oc-group .cm-content", "Baris satu");

    await app.click(".oc-group .cm-content");
    await app.type(".oc-group .cm-content", "Baris baru");
    await app.keys(Key.Enter);
    await app.waitForSelector('.oc-tab[data-dirty="true"]');
    t.assert.equal(readFileSync(`${repo}/README.md`, "utf8"), "# Demo\n\nBaris satu\n", "belum ditulis ke disk sebelum disimpan");
    await app.waitForSelector(".oc-group .git-mark-added");

    await app.shortcut("s", { ctrl: true });
    await waitForFile(t, `${repo}/README.md`, (text) => text.includes("Baris baru"), "berkas tersimpan");
    await app.waitFor("tanda belum-disimpan hilang", "return document.querySelector('.oc-tab[data-dirty=\"true\"]') === null");
    t.assert.equal(t.git(repo, "status", "--porcelain").trim(), "M README.md", "git melihat perubahan");
    await app.waitFor("explorer menandai berkas berubah", `return document.querySelector(arguments[0])?.textContent.includes("M")`, { args: [treeItem("README.md")] });
    await app.waitFor("lencana Source Control", "return [...document.querySelectorAll('.oc-badge')].some((b) => b.textContent === '1')");
  },

  async "perubahan dari luar memuat ulang buffer yang bersih"(t) {
    const repo = project(t);
    const app = await t.launch(repo);
    await app.shortcut("p", { ctrl: true });
    await app.waitForSelector(".oc-palette-input");
    await app.type(".oc-palette-input", "utilrs");
    await app.waitForText(".oc-palette-item", "util.rs");
    await app.keys(Key.Enter);
    await app.waitForText(".oc-group .cm-content", "pub fn util()");

    writeFileSync(`${repo}/src/util.rs`, "pub fn diubah_dari_luar() {}\n");
    await app.waitForText(".oc-group .cm-content", "diubah_dari_luar", { timeout: 15000 });
    t.assert(!(await app.exists('.oc-tab[data-dirty="true"]')), "buffer tetap bersih setelah dimuat ulang");
  },

  async "pencarian proyek menghormati gitignore dan melompat ke kecocokan"(t) {
    const repo = project(t);
    const app = await t.launch(repo);
    await app.shortcut("F", { ctrl: true, shift: true });
    await app.waitForSelector('input[placeholder="Search"]');
    await app.click('input[placeholder="Search"]');
    await app.type('input[placeholder="Search"]', "println");
    await app.waitForText(".sr-summary", "1 result in 1 file", { timeout: 15000 });
    t.assert.equal(await app.texts(".sr-file-name"), ["main.rs"], "hanya berkas yang tidak diabaikan");

    await app.clickText('.sr-rows [role="treeitem"]', "println!");
    await app.waitForText(".oc-tab-label", "main.rs");
    await app.waitFor("kecocokan terpilih di editor", "return window.getSelection().toString() === 'println' || [...document.querySelectorAll('.cm-selectionBackground')].length > 0", { timeout: 10000 });
    await app.waitForText(".oc-statusbar", "Ln 2");
  },

  async "ganti di seluruh proyek: berkas tertutup dan berkas terbuka"(t) {
    const repo = project(t);
    t.write(repo, "src/lain.rs", 'fn lain() {\n    println!("lain");\n}\n');
    t.commitAll(repo, "Berkas kedua");
    const app = await t.launch(repo);

    // main.rs dibuka di editor (penggantian lewat editor); lain.rs tetap tertutup (lewat backend).
    await app.shortcut("p", { ctrl: true });
    await app.waitForSelector(".oc-palette-input");
    await app.type(".oc-palette-input", "mainrs");
    await app.waitForText(".oc-palette-item", "main.rs");
    await app.keys(Key.Enter);
    await app.waitForText(".oc-group .cm-content", "println!");

    await app.shortcut("F", { ctrl: true, shift: true });
    await app.waitForSelector('input[placeholder="Search"]');
    await app.click('input[placeholder="Search"]');
    await app.type('input[placeholder="Search"]', "println!");
    await app.waitForText(".sr-summary", "2 results in 2 files", { timeout: 15000 });
    await app.click('button[aria-label="Toggle Replace"]');
    await app.waitForSelector('input[placeholder="Replace"]');
    await app.click('input[placeholder="Replace"]');
    await app.type('input[placeholder="Replace"]', "eprintln!");
    await app.click('button[aria-label="Replace All"]');
    await app.waitForText(".oc-dialog", "Replace 2 occurrences across 2 files?");
    await app.exec("[...document.querySelectorAll('.oc-dialog-footer button')].at(-1).click()");

    await waitForFile(t, `${repo}/src/lain.rs`, (v) => v.includes('eprintln!("lain")'), "berkas tertutup diganti di disk");
    await waitForFile(t, `${repo}/src/main.rs`, (v) => v.includes('eprintln!("halo")'), "berkas terbuka diganti dan disimpan");
    await app.waitForText(".oc-group .cm-content", 'eprintln!("halo")');
    t.assert(!(await app.exists('.oc-tab[data-dirty="true"]')), "berkas yang tadinya bersih disimpan kembali");
    t.assert.equal(t.git(repo, "diff", "--name-only").trim().split("\n").sort(), ["src/lain.rs", "src/main.rs"], "hanya dua berkas itu yang berubah");
  },

  async "tombol Terminal di status bar dan tombol panel di activity bar"(t) {
    const repo = project(t);
    const app = await t.launch(repo);
    await app.waitForText("button.oc-status-item", "Terminal");
    const panelOpen = () => app.exec("const p = document.querySelector('.oc-panel'); return !!p && p.getBoundingClientRect().height > 60");
    t.assert(!(await panelOpen()), "panel bawah awalnya tersembunyi");

    // Tanpa pintasan keyboard: tombol di status bar membuka terminal.
    await app.clickText("button.oc-status-item", "Terminal");
    await app.waitForSelector(".tm-stage .xterm", { timeout: 15000 });
    t.assert(await panelOpen(), "panel bawah terbuka");
    t.assert.equal(await app.exec("return [...document.querySelectorAll('.oc-panel-tab')].find((b) => b.getAttribute('aria-selected') === 'true')?.textContent.trim()"), "Terminal", "tab Terminal aktif");
    await app.clickText("button.oc-status-item", "Terminal");
    await app.waitFor("panel tersembunyi lagi", "const p = document.querySelector('.oc-panel'); return !p || p.getBoundingClientRect().height < 10");

    // Tombol panel di activity bar membuka panel yang sama (terminal tetap hidup).
    await app.click('.oc-activity[aria-label="Toggle Panel"]');
    await app.waitFor("panel terbuka lewat activity bar", "const p = document.querySelector('.oc-panel'); return !!p && p.getBoundingClientRect().height > 60");
    t.assert.equal(await app.exec("return document.querySelectorAll('.tm-stage .xterm').length"), 1, "terminal yang sama, bukan terminal baru");
  },

  async "terminal menjalankan perintah di folder proyek"(t) {
    const repo = project(t);
    const app = await t.launch(repo);
    await app.shortcut("`", { ctrl: true });
    await app.waitForSelector(".tm-stage .xterm", { timeout: 15000 });
    await app.waitForSelector(".xterm-helper-textarea");
    await t.sleep(800);
    await app.exec("document.querySelector('.xterm-helper-textarea').focus()");
    await app.type(".xterm-helper-textarea", "echo oncode-$((6*7)) > keluaran.txt");
    await app.keys(Key.Enter);
    const text = await waitForFile(t, `${repo}/keluaran.txt`, (v) => v.includes("oncode-"), "perintah terminal menulis berkas");
    t.assert.equal(text.trim(), "oncode-42", "shell berjalan di folder proyek dan mengevaluasi perintah");
  },

  async "pengaturan tersimpan ke settings.json dan langsung berlaku"(t) {
    const repo = project(t);
    const app = await t.launch(repo);
    await app.shortcut(",", { ctrl: true });
    await app.waitForSelector('[data-setting="editor.wordWrap"] input');
    t.assert(!(await app.exists(".cm-lineWrapping")), "awalnya tanpa word wrap");
    await app.click('[data-setting="editor.wordWrap"] input');
    const saved = await waitForFile(t, `${t.configDir}/settings.json`, (v) => /"wordWrap": true/.test(v), "settings.json diperbarui");
    t.assert.includes(saved, '"tabSize": 4', "pengaturan lain tetap utuh");

    // Editor yang dibuka memakai pengaturan baru.
    await app.shortcut("p", { ctrl: true });
    await app.waitForSelector(".oc-palette-input");
    await app.type(".oc-palette-input", "README");
    await app.waitForText(".oc-palette-item", "README.md");
    await app.keys(Key.Enter);
    await app.waitForSelector(".oc-group .cm-content.cm-lineWrapping");
    await app.shortcut(",", { ctrl: true });
    await app.click('[data-setting="editor.wordWrap"] input');
    await waitForFile(t, `${t.configDir}/settings.json`, (v) => /"wordWrap": false/.test(v), "pengaturan dikembalikan");
    // Tanpa .oncode/settings.json di proyek, tidak boleh ada penanda "overridden by project" yang menetap.
    await app.waitFor("penanda override tidak menetap", "return document.querySelector('.st-override') === null", { timeout: 5000 });
  },

  async "blame: gutter muncul hanya saat dinyalakan"(t) {
    const repo = project(t);
    const app = await t.launch(repo);
    await app.shortcut("p", { ctrl: true });
    await app.waitForSelector(".oc-palette-input");
    await app.type(".oc-palette-input", "mainrs");
    await app.waitForText(".oc-palette-item", "main.rs");
    await app.keys(Key.Enter);
    await app.waitForText(".oc-group .cm-content", "println!");
    t.assert(!(await app.exists(".cm-blameGutter")), "gutter blame tidak ada sebelum dinyalakan");

    await app.shortcut("P", { ctrl: true, shift: true });
    await app.waitForSelector(".oc-palette-input");
    await app.type(".oc-palette-input", "Toggle Blame");
    await app.waitForText(".oc-palette-item", "Toggle Blame");
    await app.keys(Key.Enter);
    await app.waitForText(".cm-blameGutter", "Penguji", { timeout: 15000 });

    await app.shortcut("P", { ctrl: true, shift: true });
    await app.waitForSelector(".oc-palette-input");
    await app.type(".oc-palette-input", "Toggle Blame");
    await app.waitForText(".oc-palette-item", "Toggle Blame");
    await app.keys(Key.Enter);
    await app.waitFor("gutter blame hilang", "return document.querySelector('.cm-blameGutter') === null");
  },

  async "tab berkas dipulihkan saat folder dibuka lagi"(t) {
    const repo = project(t);
    let app = await t.launch(repo);
    await app.shortcut("E", { ctrl: true, shift: true });
    await app.waitForSelector(treeItem("README.md"));
    await app.exec("document.querySelector(arguments[0]).dispatchEvent(new MouseEvent('dblclick', { bubbles: true }))", treeItem("README.md"));
    await app.waitForText(".oc-tab-label", "README.md");
    await t.sleep(1200);
    await app.quit();

    app = await t.launch(repo);
    await app.waitForText(".oc-tab-label", "README.md", { timeout: 15000 });
    await app.waitForText(".oc-group .cm-content", "Baris satu");
  },
};
