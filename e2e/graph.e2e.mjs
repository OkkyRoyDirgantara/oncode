// Graph dan interactive rebase di aplikasi sungguhan: UI → backend → git → oncode-helper → dialog pesan.
function historyRepo(t) {
  const repo = t.repo();
  t.write(repo, "base.txt", "base\n");
  t.commitAll(repo, "Dasar");
  for (const [file, subject] of [["a.txt", "Tambah validasi login"], ["b.txt", "wip"], ["c.txt", "perbaiki tes"], ["d.txt", "debug print"]]) {
    t.write(repo, file, `${file}\n`);
    t.commitAll(repo, subject);
  }
  return repo;
}

const subjects = (t, repo) => t.git(repo, "log", "--format=%s").trim().split("\n");

async function openGraph(app) {
  await app.waitForSelector(".scm-commit");
  await app.shortcut("H", { ctrl: true, shift: true });
  await app.waitForSelector(".gr-row .gr-subject");
}

export const tests = {
  async "graph menampilkan riwayat, ref, dan detail commit"(t) {
    const repo = historyRepo(t);
    t.git(repo, "switch", "-q", "-c", "fitur", "HEAD~2");
    t.write(repo, "fitur.txt", "f\n");
    t.commitAll(repo, "Di branch fitur");
    t.git(repo, "switch", "-q", "main");
    t.git(repo, "tag", "v1.0.0", "HEAD~1");
    const app = await t.launch(repo);
    await openGraph(app);

    const shown = await app.texts(".gr-row .gr-subject");
    for (const subject of ["debug print", "perbaiki tes", "wip", "Tambah validasi login", "Dasar", "Di branch fitur"]) {
      t.assert(shown.includes(subject), `baris untuk commit "${subject}" (terlihat: ${JSON.stringify(shown)})`);
    }
    const rowText = (subject) => app.exec("return [...document.querySelectorAll('.gr-row')].find((r) => r.textContent.includes(arguments[0]))?.textContent ?? ''", subject);
    t.assert.includes(await rowText("debug print"), "main", "branch main ditandai pada commit teratas");
    t.assert.includes(await rowText("perbaiki tes"), "v1.0.0", "tag ditandai pada commit-nya");
    t.assert.includes(await rowText("Di branch fitur"), "fitur", "branch fitur ditandai");
    // Dua garis riwayat berjalan berdampingan: ada baris yang titik commit-nya di jalur kedua.
    t.assert(await app.exec("return document.querySelectorAll('.gr-row svg path, .gr-row svg line').length > 0"), "garis graph digambar");

    await app.clickText(".gr-row", "Tambah validasi login");
    await app.waitForText(".gr-details", "Tambah validasi login");
    await app.waitForText(".gr-details", "a.txt");
    const hash = t.git(repo, "log", "--format=%H", "--grep=Tambah validasi login").trim();
    t.assert.includes(await app.text(".gr-details"), hash, "hash lengkap di panel detail");
  },

  async "interactive rebase: squash, reword, dan drop lewat dialog"(t) {
    const repo = historyRepo(t);
    const app = await t.launch(repo);
    await openGraph(app);

    await app.contextMenu(".gr-row", "Dasar");
    await app.clickText(".oc-menu-item", "Interactive Rebase from Here");
    await app.waitFor("empat baris rencana rebase", "return document.querySelectorAll('.gr-rebase-row').length === 4");
    t.assert.equal(
      await app.exec("return [...document.querySelectorAll('.gr-rebase-row')].map((r) => ['Tambah validasi login', 'wip', 'perbaiki tes', 'debug print'].find((s) => r.textContent.includes(s)))"),
      ["Tambah validasi login", "wip", "perbaiki tes", "debug print"],
      "urutan dari yang tertua ke yang terbaru",
    );

    const setAction = (index, action) =>
      app.exec(
        `const select = document.querySelectorAll('.gr-rebase-row select')[arguments[0]];
         Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype, "value").set.call(select, arguments[1]);
         select.dispatchEvent(new Event("change", { bubbles: true }));`,
        index,
        action,
      );
    // Squash pada baris pertama tidak sah: tombol mulai harus nonaktif.
    await setAction(0, "squash");
    await app.waitFor("tombol mulai nonaktif untuk rencana tidak sah", "return document.querySelector('.oc-dialog .oc-btn-primary').disabled === true");
    await setAction(0, "pick");
    await setAction(1, "squash");
    await setAction(2, "reword");
    await setAction(3, "drop");
    await app.waitFor("tombol mulai aktif", "return document.querySelector('.oc-dialog .oc-btn-primary').disabled === false");
    await app.click(".oc-dialog .oc-btn-primary");

    // Git meminta pesan untuk squash, lalu untuk reword; keduanya dijawab lewat dialog aplikasi.
    await app.waitForText(".oc-dialog-header", "Commit message", { timeout: 15000 });
    t.assert.includes(await app.exec("return document.querySelector('.oc-dialog textarea').value"), "wip", "dialog squash memuat pesan kedua commit");
    await app.setValue(".oc-dialog textarea", "Validasi login lengkap");
    await app.clickText(".oc-dialog button", "Use Message");

    await app.waitFor("dialog pesan kedua (reword)", "const t = document.querySelector('.oc-dialog textarea'); return t && t.value.includes('perbaiki tes')", { timeout: 15000 });
    await app.setValue(".oc-dialog textarea", "Perbaiki tes login");
    await app.clickText(".oc-dialog button", "Use Message");

    await app.waitFor("rebase selesai di graph", "return [...document.querySelectorAll('.gr-row .gr-subject')].some((e) => e.textContent === 'Perbaiki tes login')", { timeout: 15000 });
    t.assert.equal(subjects(t, repo), ["Perbaiki tes login", "Validasi login lengkap", "Dasar"], "riwayat setelah rebase");
    const fs = await import("node:fs");
    t.assert(fs.existsSync(`${repo}/a.txt`) && fs.existsSync(`${repo}/b.txt`) && fs.existsSync(`${repo}/c.txt`), "isi commit yang dipertahankan tetap ada");
    t.assert(!fs.existsSync(`${repo}/d.txt`), "commit yang di-drop hilang");
    t.assert.equal(t.git(repo, "status", "--porcelain").trim(), "", "tidak ada rebase yang tertinggal");
  },

  async "membatalkan dialog pesan menghentikan rebase dan bisa di-abort"(t) {
    const repo = historyRepo(t);
    const before = t.git(repo, "rev-parse", "HEAD").trim();
    const app = await t.launch(repo);
    await openGraph(app);
    await app.contextMenu(".gr-row", "perbaiki tes");
    await app.clickText(".oc-menu-item", "Interactive Rebase from Here");
    await app.waitFor("satu baris rencana", "return document.querySelectorAll('.gr-rebase-row').length === 1");
    await app.setValue(".gr-rebase-row select", "reword");
    await app.click(".oc-dialog .oc-btn-primary");
    await app.waitForText(".oc-dialog-header", "Commit message", { timeout: 15000 });
    await app.clickText(".oc-dialog button", "Cancel");

    // Rebase berhenti di tengah; banner di Source Control menawarkan Abort.
    await app.shortcut("G", { ctrl: true, shift: true });
    await app.waitForText(".scm-operation", "rebase in progress", { timeout: 15000 });
    await app.clickText(".scm-operation button", "Abort");
    await app.waitForText(".oc-dialog", "Abort rebase?");
    await app.clickText(".oc-dialog button", "Abort");
    await app.waitFor("banner operasi hilang", "return document.querySelector('.scm-operation') === null");
    t.assert.equal(t.git(repo, "rev-parse", "HEAD").trim(), before, "HEAD kembali seperti semula");
    t.assert.equal(subjects(t, repo)[0], "debug print", "riwayat tidak berubah");
  },
};
