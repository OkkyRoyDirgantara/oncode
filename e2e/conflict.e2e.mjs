// Konflik merge diselesaikan seluruhnya lewat UI: editor konflik, lalu Continue.
function conflictedRepo(t) {
  const repo = t.repo();
  t.write(repo, "config.toml", 'timeout = 30\nretries = 3\nlog = "info"\n');
  t.commitAll(repo, "Konfigurasi");
  t.git(repo, "switch", "-q", "-c", "fitur");
  t.write(repo, "config.toml", 'timeout = 60\nretries = 5\nlog = "info"\n');
  t.commitAll(repo, "Fitur ubah retries");
  t.git(repo, "switch", "-q", "main");
  t.write(repo, "config.toml", 'timeout = 30\nretries = 4\nlog = "debug"\n');
  t.commitAll(repo, "Main ubah retries");
  try {
    t.git(repo, "merge", "--no-edit", "fitur");
  } catch {
    // konflik memang diharapkan
  }
  return repo;
}

export const tests = {
  async "merge berkonflik: pilih sisi incoming, tandai selesai, lanjutkan"(t) {
    const repo = conflictedRepo(t);
    const app = await t.launch(repo);

    await app.waitForText(".scm-operation", "Merge in progress");
    await app.waitForText(".oc-section-header", "Conflicts");
    t.assert(await app.exec("return [...document.querySelectorAll('.scm-operation button')].find((b) => b.textContent === 'Continue').disabled"), "Continue nonaktif selama masih ada konflik");

    await app.clickText(".scm-file", "config.toml");
    await app.waitForSelector(".conflict-widget");
    t.assert.equal(await app.texts(".conflict-pane-title"), ["Current · main", "Result (editable)", "Incoming · fitur"], "label panel memakai nama branch sebenarnya");
    await app.waitForText(".diff-toolbar", "1 of 1 unresolved");

    await app.clickText(".conflict-widget button", "Use Incoming");
    await app.waitForText(".diff-toolbar", "All 1 resolved");
    await app.clickText(".diff-toolbar button", "Mark as Resolved");
    await app.waitFor("konflik hilang dari panel", "return ![...document.querySelectorAll('.oc-section-header')].some((e) => e.textContent.includes('Conflicts'))");

    // Ketiga baris berdekatan sehingga menjadi satu wilayah konflik; hasilnya mengikuti sisi incoming.
    const fs = await import("node:fs");
    t.assert.equal(fs.readFileSync(`${repo}/config.toml`, "utf8"), 'timeout = 60\nretries = 5\nlog = "info"\n', "isi berkas hasil");
    t.assert.equal(t.git(repo, "diff", "--name-only", "--diff-filter=U").trim(), "", "tidak ada berkas unmerged");

    await app.clickText(".scm-operation button", "Continue");
    await app.waitFor("banner operasi hilang", "return document.querySelector('.scm-operation') === null");
    t.assert.equal(t.git(repo, "rev-list", "--parents", "-n", "1", "HEAD").trim().split(" ").length, 3, "HEAD adalah merge commit");
    t.assert.equal(t.git(repo, "status", "--porcelain").trim(), "", "working tree bersih");
  },

  async "abort merge mengembalikan keadaan semula setelah konfirmasi"(t) {
    const repo = conflictedRepo(t);
    const before = t.git(repo, "rev-parse", "HEAD").trim();
    const app = await t.launch(repo);
    await app.waitForText(".scm-operation", "Merge in progress");
    await app.clickText(".scm-operation button", "Abort");
    await app.waitForText(".oc-dialog", "Abort merge?");
    await app.clickText(".oc-dialog button", "Abort");
    await app.waitFor("banner operasi hilang", "return document.querySelector('.scm-operation') === null");
    t.assert.equal(t.git(repo, "rev-parse", "HEAD").trim(), before, "HEAD tidak berubah");
    t.assert.equal(t.git(repo, "status", "--porcelain").trim(), "", "working tree bersih");
  },
};
