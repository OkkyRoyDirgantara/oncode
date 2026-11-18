// Autentikasi HTTPS sungguhan: remote adalah server git smart-HTTP lokal yang mewajibkan Basic auth.
// Rantai yang diuji: git → oncode-helper (askpass) → dialog di aplikasi → jawaban kembali ke git.
import { mkdirSync } from "node:fs";
import { join } from "node:path";
import { gitHttpServer } from "./servers.mjs";

const USER = "ana";
const PASSWORD = "sandi-rahasia";

function setup(t) {
  const root = join(t.dir, "server");
  mkdirSync(join(root, "repo.git"), { recursive: true });
  t.git(join(root, "repo.git"), "init", "-q", "--bare", "-b", "main");
  const repo = t.repo();
  t.write(repo, "a.txt", "a\n");
  t.commitAll(repo, "Commit pertama");
  return { root, repo, bare: join(root, "repo.git") };
}

async function answer(app, title, value) {
  await app.waitForText(".oc-dialog-header", title, { timeout: 20000 });
  await app.setValue(".oc-dialog input", value);
  await app.clickText(".oc-dialog button", "Continue");
  await app.waitFor(`dialog "${title}" tertutup`, "return ![...document.querySelectorAll('.oc-dialog-header')].some((e) => e.textContent.includes(arguments[0]))", { args: [title] });
}

export const tests = {
  async "push ke remote HTTP meminta username dan password lewat dialog aplikasi"(t) {
    const { root, repo, bare } = setup(t);
    const server = await gitHttpServer(root, (user, password) => user === USER && password === PASSWORD);
    try {
      t.git(repo, "remote", "add", "origin", `${server.origin}/repo.git`);
      const app = await t.launch(repo);
      await app.waitForText(".oc-sidebar button", "Publish Branch");
      await app.clickText(".oc-sidebar button", "Publish Branch");

      await answer(app, "Git credentials: username", USER);
      // Kolom password harus tersembunyi.
      await app.waitForText(".oc-dialog-header", "Git credentials: password", { timeout: 20000 });
      t.assert.equal(await app.exec("return document.querySelector('.oc-dialog input').type"), "password", "masukan password disembunyikan");
      t.assert.includes(await app.text(".oc-dialog"), `${USER}@127.0.0.1`, "dialog menyebut akun dan host yang meminta");
      await answer(app, "Git credentials: password", PASSWORD);

      await app.waitFor("branch terpublikasi", "return ![...document.querySelectorAll('.oc-sidebar button')].some((b) => b.textContent.includes('Publish Branch'))", { timeout: 20000 });
      t.assert.equal(t.git(bare, "rev-parse", "main").trim(), t.git(repo, "rev-parse", "HEAD").trim(), "commit sampai di server");
      t.assert(server.attempts.some((a) => a.user === USER && a.password === PASSWORD), "kredensial dari dialog dipakai git");
      t.assert(!(await app.exec("return document.body.innerText.includes(arguments[0])", PASSWORD)), "password tidak tampil di layar maupun Git Output");
    } finally {
      await server.close();
    }
  },

  async "password salah menghasilkan galat autentikasi, membatalkan dialog menghentikan push"(t) {
    const { root, repo, bare } = setup(t);
    const server = await gitHttpServer(root, (user, password) => user === USER && password === PASSWORD);
    try {
      t.git(repo, "remote", "add", "origin", `${server.origin}/repo.git`);
      const app = await t.launch(repo);
      await app.waitForText(".oc-sidebar button", "Publish Branch");

      await app.clickText(".oc-sidebar button", "Publish Branch");
      await answer(app, "Git credentials: username", USER);
      await answer(app, "Git credentials: password", "salah");
      await app.waitForText(".oc-toast", "authentication was rejected", { timeout: 20000 });

      // Batalkan di dialog username: git berhenti rapi tanpa menunggu.
      await app.exec("document.querySelectorAll('.oc-toast button[aria-label=\"Dismiss\"]').forEach((b) => b.click())");
      await app.clickText(".oc-sidebar button", "Publish Branch");
      await app.waitForText(".oc-dialog-header", "Git credentials: username", { timeout: 20000 });
      await app.clickText(".oc-dialog button", "Cancel");
      await app.waitForText(".oc-toast", "authentication", { timeout: 20000 });
      await app.waitFor("tidak ada operasi yang menggantung", "return [...document.querySelectorAll('.oc-sidebar button')].some((b) => b.textContent.includes('Publish Branch') && !b.disabled)", { timeout: 20000 });
      let exists = true;
      try {
        t.git(bare, "rev-parse", "--verify", "main");
      } catch {
        exists = false;
      }
      t.assert(!exists, "tidak ada yang terdorong ke server");
    } finally {
      await server.close();
    }
  },
};
