// Publish, fetch, pull, dan push terhadap remote lokal (repositori bare), lewat tombol di UI.
import { existsSync, mkdirSync } from "node:fs";
import { join } from "node:path";

function withRemote(t) {
  const remote = join(t.dir, "remote.git");
  mkdirSync(remote, { recursive: true });
  t.git(remote, "init", "-q", "--bare", "-b", "main");
  const repo = t.repo();
  t.write(repo, "a.txt", "a\n");
  t.commitAll(repo, "Commit pertama");
  t.git(repo, "remote", "add", "origin", remote);
  return { repo, remote };
}

export const tests = {
  async "publish branch, lalu sinkron dengan commit dari orang lain"(t) {
    const { repo, remote } = withRemote(t);
    const app = await t.launch(repo);

    await app.waitForText(".oc-sidebar button", "Publish Branch");
    await app.clickText(".oc-sidebar button", "Publish Branch");
    await app.waitFor("tombol publish hilang setelah branch punya upstream", "return ![...document.querySelectorAll('.oc-sidebar button')].some((b) => b.textContent.includes('Publish Branch'))", { timeout: 15000 });
    t.assert.equal(t.git(remote, "rev-parse", "main").trim(), t.git(repo, "rev-parse", "HEAD").trim(), "remote menerima branch");
    t.assert.equal(t.git(repo, "rev-parse", "--abbrev-ref", "main@{upstream}").trim(), "origin/main", "upstream terpasang");

    // Orang lain mendorong commit baru.
    const other = join(t.dir, "lain");
    t.git(t.dir, "clone", "-q", remote, other);
    t.git(other, "config", "user.name", "Rekan");
    t.git(other, "config", "user.email", "rekan@contoh.id");
    t.write(other, "dari-rekan.txt", "halo\n");
    t.commitAll(other, "Dari rekan");
    t.git(other, "push", "-q");

    // Commit lokal lewat UI, sehingga riwayat bercabang: 1 di depan, 1 di belakang setelah fetch.
    t.write(repo, "lokal.txt", "lokal\n");
    await app.waitForText(".scm-file", "lokal.txt");
    await app.clickDom('.scm-file button[aria-label="Stage"]');
    await app.waitForText(".oc-section-header", "Staged Changes");
    await app.type('textarea[aria-label="Commit message"]', "Commit lokal");
    await app.clickText(".scm-split-btn button", "Commit");
    await app.waitFor("commit lokal selesai", "return ![...document.querySelectorAll('.oc-section-header')].some((e) => e.textContent.includes('Staged'))");

    await app.click('.oc-sidebar button[aria-label="Fetch"]');
    await app.waitForText(".oc-sidebar button", "Sync Changes", { timeout: 15000 });
    await app.waitFor("status bar menunjukkan 1 di belakang dan 1 di depan", "return [...document.querySelectorAll('.oc-status-item')].some((e) => e.textContent.replace(/\\s/g, '') === '11')", { timeout: 15000 });

    // Pull ff-only tidak mungkin; aplikasi menawarkan Merge atau Rebase.
    await app.clickText(".oc-sidebar button", "Sync Changes");
    await app.waitForText(".oc-dialog", "Branches have diverged", { timeout: 15000 });
    await app.clickText(".oc-dialog button", "Rebase");
    await app.waitFor("sinkron selesai", "return ![...document.querySelectorAll('.oc-sidebar button')].some((b) => b.textContent.includes('Sync Changes'))", { timeout: 20000 });

    t.assert(existsSync(join(repo, "dari-rekan.txt")), "commit rekan ikut tertarik");
    t.assert.equal(t.git(repo, "log", "--format=%s", "-2").trim().split("\n"), ["Commit lokal", "Dari rekan"], "commit lokal di-rebase di atas commit rekan");
    t.assert.equal(t.git(remote, "log", "--format=%s", "-1", "main").trim(), "Commit lokal", "hasilnya terdorong ke remote");
  },

  async "berpindah dan membuat branch dari status bar"(t) {
    const { repo } = withRemote(t);
    t.git(repo, "branch", "fitur/login");
    const app = await t.launch(repo);
    await app.waitForText(".oc-status-item", "main");
    await app.clickText("button.oc-status-item", "main");
    await app.waitForSelector(".oc-palette-input");
    await app.type(".oc-palette-input", "fitur");
    await app.waitForText(".oc-palette-item", "fitur/login");
    await app.clickText(".oc-palette-item", "fitur/login");
    await app.waitForText("button.oc-status-item", "fitur/login");
    t.assert.equal(t.git(repo, "rev-parse", "--abbrev-ref", "HEAD").trim(), "fitur/login", "branch berpindah");

    await app.clickText("button.oc-status-item", "fitur/login");
    await app.waitForSelector(".oc-palette-input");
    await app.clickText(".oc-palette-item", "Create new branch");
    await app.waitForText(".oc-dialog-header", "Create branch");
    await app.setValue(".oc-dialog input", "nama tidak sah");
    await app.waitForText(".oc-dialog .oc-error-text", "not a valid Git ref name");
    await app.setValue(".oc-dialog input", "perbaikan/typo");
    await app.clickText(".oc-dialog button", "OK");
    await app.waitForText("button.oc-status-item", "perbaikan/typo");
    t.assert.equal(t.git(repo, "rev-parse", "--abbrev-ref", "HEAD").trim(), "perbaikan/typo", "branch baru dibuat dan aktif");
  },

  async "stash dan pop mengembalikan perubahan"(t) {
    const { repo } = withRemote(t);
    t.write(repo, "a.txt", "a\nubah\n");
    const app = await t.launch(repo);
    await app.waitForText(".scm-file", "a.txt");
    await app.clickDom('.oc-section-header button[aria-label="Stash all changes…"]');
    await app.waitForText(".oc-dialog-header", "Stash changes");
    await app.setValue(".oc-dialog input", "kerja setengah jadi");
    await app.clickText(".oc-dialog button", "Stash");
    await app.waitForText(".oc-section-header", "Stashes");
    t.assert.equal(t.git(repo, "status", "--porcelain").trim(), "", "working tree bersih setelah stash");
    t.assert.includes(t.git(repo, "stash", "list"), "kerja setengah jadi", "stash tercatat dengan pesannya");

    await app.clickText(".oc-section-header", "Stashes");
    await app.waitForText(".oc-list-row", "kerja setengah jadi");
    await app.clickDom('.oc-list-row button[aria-label="Pop (apply and drop)"]');
    await app.waitForText(".scm-file", "a.txt");
    t.assert.equal(t.git(repo, "status", "--porcelain").trim(), "M a.txt", "perubahan kembali");
    t.assert.equal(t.git(repo, "stash", "list").trim(), "", "stash terhapus setelah pop");
  },
};
