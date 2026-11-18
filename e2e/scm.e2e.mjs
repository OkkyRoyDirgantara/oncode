// Alur harian Source Control di aplikasi sungguhan; hasilnya diperiksa dengan git, bukan hanya dari layar.
import { existsSync, readFileSync } from "node:fs";
const README = "# Demo\n\nBaris satu\nBaris dua\nBaris tiga\n";

function sampleRepo(t) {
  const repo = t.repo();
  t.write(repo, "README.md", README);
  t.write(repo, "src/main.rs", 'fn main() {\n    println!("halo");\n}\n');
  t.commitAll(repo, "Commit pertama");
  return repo;
}

const changedLines = (diff) => diff.split("\n").filter((l) => /^[+-][^+-]/.test(l) || /^[+-]$/.test(l));

export const tests = {
  async "folder baru dibuka dalam mode terbatas sampai dipercaya"(t) {
    const repo = sampleRepo(t);
    const app = await t.launch(repo, { trust: false });
    await app.waitForText(".oc-banner", "Restricted mode");
    t.assert(!(await app.exists(".scm-commit")), "kotak commit tidak boleh ada sebelum folder dipercaya");
    await app.clickText("button", "Trust folder");
    await app.waitForSelector(".scm-commit");
    t.assert((await app.texts(".oc-status-item")).includes("main"), "status bar menampilkan branch");
    t.assert(await app.exec("return document.querySelector('.scm-commit').offsetParent !== null"), "Source Control menjadi tampilan aktif setelah folder dipercaya");
  },

  async "stage satu hunk, lalu commit"(t) {
    const repo = sampleRepo(t);
    t.write(repo, "README.md", "# Demo\n\nBaris SATU\nBaris dua\nBaris tiga\nBaris empat\n");
    t.write(repo, "catatan.txt", "catatan\n");
    const app = await t.launch(repo);

    await app.waitForText(".scm-file", "README.md");
    t.assert.equal((await app.texts(".scm-file")).sort(), ["README.mdM", "catatan.txtU"], "daftar perubahan");
    t.assert.equal(await app.text(".oc-badge"), "2", "lencana jumlah perubahan");

    await app.clickText(".scm-file", "README.md");
    await app.waitFor("dua hunk dengan tombol aksi", "return document.querySelectorAll('.diff-actions').length === 2");
    await app.clickText(".diff-actions button", "Stage");
    await app.waitForText(".oc-section-header", "Staged Changes");

    t.assert.equal(changedLines(t.git(repo, "diff", "--cached", "-U0")), ["-Baris satu", "+Baris SATU"], "hanya hunk pertama yang masuk index");
    t.assert.equal(changedLines(t.git(repo, "diff", "-U0")), ["+Baris empat"], "hunk kedua tetap belum di-stage");
    await app.waitFor("diff tinggal satu hunk", "return document.querySelectorAll('.diff-actions').length === 1");

    await app.type('textarea[aria-label="Commit message"]', "Ubah baris satu");
    await app.clickText(".scm-split-btn button", "Commit");
    await app.waitFor("bagian staged hilang setelah commit", "return ![...document.querySelectorAll('.oc-section-header')].some((e) => e.textContent.includes('Staged'))");
    t.assert.equal(t.git(repo, "log", "-1", "--format=%s").trim(), "Ubah baris satu", "commit tercatat");
    t.assert.equal(t.git(repo, "show", "--stat", "--format=", "HEAD").includes("catatan.txt"), false, "berkas untracked tidak ikut ter-commit");
    t.assert.equal(await app.exec("return document.querySelector('textarea[aria-label=\"Commit message\"]').value"), "", "kotak commit dikosongkan");
  },

  async "unstage hunk dan discard hunk dengan konfirmasi"(t) {
    const repo = sampleRepo(t);
    t.write(repo, "README.md", "# Demo\n\nBaris SATU\nBaris dua\nBaris tiga\nBaris empat\n");
    t.git(repo, "add", "README.md");
    const app = await t.launch(repo);

    await app.waitForText(".oc-section-header", "Staged Changes");
    await app.clickText(".scm-file", "README.md");
    await app.waitFor("diff staged dengan dua hunk", "return document.querySelectorAll('.diff-actions').length === 2");
    t.assert.equal(await app.texts(".diff-actions button"), ["Unstage", "Unstage"], "diff staged hanya menawarkan Unstage");
    await app.clickText(".diff-actions button", "Unstage");
    await app.waitForText(".oc-section-header", "Changes");
    await app.waitFor("index tinggal satu perubahan", "return document.querySelectorAll('.diff-actions').length === 1");
    t.assert.equal(changedLines(t.git(repo, "diff", "--cached", "-U0")), ["+Baris empat"], "hunk pertama keluar dari index");
    t.assert.equal(changedLines(t.git(repo, "diff", "-U0")), ["-Baris satu", "+Baris SATU"], "perubahannya tetap ada di working tree");

    // Discard dari diff working tree meminta konfirmasi; membatalkan tidak mengubah apa pun.
    await app.clickText(".oc-section-header + .scm-file, .scm-file", "README.md");
    await app.exec("[...document.querySelectorAll('.scm-file')].at(-1).click()");
    await app.waitForText(".diff-actions button", "Discard");
    await app.clickText(".diff-actions button", "Discard");
    await app.waitForText(".oc-dialog", "Discard this change?");
    await app.clickText(".oc-dialog button", "Cancel");
    await t.sleep(300);
    t.assert.includes(t.git(repo, "diff"), "+Baris SATU", "membatalkan konfirmasi tidak membuang perubahan");

    await app.clickText(".diff-actions button", "Discard");
    await app.waitForText(".oc-dialog", "Discard this change?");
    await app.clickText(".oc-dialog button", "Discard");
    await app.waitFor("perubahan working tree hilang", "return ![...document.querySelectorAll('.oc-section-header')].some((e) => /^Changes[1-9]/.test(e.textContent))");
    t.assert.equal(t.git(repo, "diff").trim(), "", "working tree sama dengan index");
    t.assert.equal(changedLines(t.git(repo, "diff", "--cached", "-U0")), ["+Baris empat"], "perubahan yang sudah di-stage tidak tersentuh");
  },

  async "stage berkas untracked dan batalkan lewat tombol baris"(t) {
    const repo = sampleRepo(t);
    t.write(repo, "baru dengan spasi.txt", "satu\ndua\n");
    const app = await t.launch(repo);
    await app.waitForText(".scm-file", "baru dengan spasi.txt");
    await app.clickDom('.scm-file button[aria-label="Stage"]');
    await app.waitForText(".oc-section-header", "Staged Changes");
    t.assert.equal(t.git(repo, "status", "--porcelain").trim(), "A  baru dengan spasi.txt".replace("baru dengan spasi.txt", '"baru dengan spasi.txt"'), "berkas ter-stage");
    await app.clickDom('.scm-file button[aria-label="Unstage"]');
    await app.waitFor("kembali untracked", "return ![...document.querySelectorAll('.oc-section-header')].some((e) => e.textContent.includes('Staged'))");
    t.assert.equal(t.git(repo, "status", "--porcelain").trim(), '?? "baru dengan spasi.txt"', "berkas kembali untracked");
  },

  async "stage hanya baris terpilih, dan stage hunk pada tampilan unified"(t) {
    const repo = t.repo();
    t.write(repo, "daftar.txt", "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n11\n12\n");
    t.commitAll(repo, "Daftar");
    // Satu hunk berisi tiga baris baru (A, B, C), dan satu hunk terpisah di akhir.
    t.write(repo, "daftar.txt", "1\nA\nB\nC\n2\n3\n4\n5\n6\n7\n8\n9\n10\n11\nDUA BELAS\n");
    const app = await t.launch(repo);
    await app.waitForText(".scm-file", "daftar.txt");
    await app.clickText(".scm-file", "daftar.txt");
    await app.waitFor("dua hunk", "return document.querySelectorAll('.diff-actions').length === 2");

    // Kursor pada baris "B" di sisi working tree, lalu stage baris terpilih saja.
    await app.exec(
      `const line = [...document.querySelectorAll('.cm-merge-b .cm-line')].find((l) => l.textContent.replace(/Stage|Discard/g, '') === 'B');
       const r = line.getBoundingClientRect();
       const init = { bubbles: true, cancelable: true, view: window, clientX: r.left + 30, clientY: r.top + r.height / 2, button: 0, detail: 1 };
       line.dispatchEvent(new MouseEvent('mousedown', init)); line.dispatchEvent(new MouseEvent('mouseup', init)); line.dispatchEvent(new MouseEvent('click', init));`,
    );
    await app.clickText(".diff-toolbar button", "Stage Selected Lines");
    await app.waitForText(".oc-section-header", "Staged Changes");
    t.assert.equal(t.git(repo, "show", ":daftar.txt"), "1\nB\n2\n3\n4\n5\n6\n7\n8\n9\n10\n11\n12\n", "hanya baris B yang masuk index");

    // Tampilan unified: hunk yang tersisa tetap bisa di-stage per hunk.
    await app.click('.diff-toolbar button[aria-label="Toggle diff layout"]');
    await app.waitFor("tampilan unified", "return document.querySelector('.cm-mergeView') === null && document.querySelectorAll('.diff-actions').length >= 2");
    const before = await app.exec("return document.querySelectorAll('.diff-actions').length");
    await app.exec("[...document.querySelectorAll('.diff-actions')].at(-1).querySelector('button').click()");
    await app.waitFor("satu hunk berkurang", "return document.querySelectorAll('.diff-actions').length === arguments[0] - 1", { args: [before] });
    t.assert.equal(t.git(repo, "show", ":daftar.txt"), "1\nB\n2\n3\n4\n5\n6\n7\n8\n9\n10\n11\nDUA BELAS\n", "hunk terakhir masuk index lewat tampilan unified");
    t.assert.equal(readFileSync(`${repo}/daftar.txt`, "utf8"), "1\nA\nB\nC\n2\n3\n4\n5\n6\n7\n8\n9\n10\n11\nDUA BELAS\n", "working tree tidak berubah");
    await app.click('.diff-toolbar button[aria-label="Toggle diff layout"]');
    await app.waitForSelector(".cm-mergeView");
  },

  async "membuang berkas untracked meminta konfirmasi dan memakai tempat sampah"(t) {
    const repo = sampleRepo(t);
    t.write(repo, "sementara.txt", "tidak penting\n");
    t.write(repo, "README.md", README + "tambahan\n");
    const app = await t.launch(repo);
    await app.waitForText(".scm-file", "sementara.txt");

    await app.exec("[...document.querySelectorAll('.scm-file')].find((r) => r.textContent.includes('sementara.txt')).querySelector('button[aria-label^=\"Delete\"]').click()");
    await app.waitForText(".oc-dialog", "moved to the trash");
    t.assert.includes(await app.text(".oc-dialog"), "sementara.txt", "konfirmasi menyebut berkasnya");
    await app.clickText(".oc-dialog button", "Cancel");
    await t.sleep(300);
    t.assert(existsSync(`${repo}/sementara.txt`), "membatalkan tidak menghapus berkas");

    await app.exec("[...document.querySelectorAll('.scm-file')].find((r) => r.textContent.includes('sementara.txt')).querySelector('button[aria-label^=\"Delete\"]').click()");
    await app.waitForText(".oc-dialog", "moved to the trash");
    await app.clickText(".oc-dialog button", "Discard");
    await app.waitFor("berkas hilang dari daftar", "return ![...document.querySelectorAll('.scm-file')].some((r) => r.textContent.includes('sementara.txt'))");
    t.assert(!existsSync(`${repo}/sementara.txt`), "berkas tidak lagi di working tree");
    t.assert.equal(t.git(repo, "status", "--porcelain").trim(), "M README.md", "perubahan lain tidak tersentuh");
  },

  async "perintah git yang dijalankan tercatat di Git Output"(t) {
    const repo = sampleRepo(t);
    t.write(repo, "a.txt", "a\n");
    const app = await t.launch(repo);
    await app.waitForText(".scm-file", "a.txt");
    await app.clickDom('.scm-file button[aria-label="Stage"]');
    await app.waitForText(".oc-section-header", "Staged Changes");
    await app.exec("document.querySelector('.oc-status-item')?.blur()");
    await app.exec("window.dispatchEvent(new KeyboardEvent('keydown', { key: 'j', code: 'KeyJ', ctrlKey: true, bubbles: true, cancelable: true }))");
    await app.waitForSelector(".oc-panel-tab");
    await app.clickText(".oc-panel-tab", "Git Output");
    await app.waitForText(".gitout-cmd", "git add -A -- a.txt");
  },
};
