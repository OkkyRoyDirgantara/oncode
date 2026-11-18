// Integrasi GitLab di aplikasi sungguhan, terhadap server GitLab tiruan di mesin lokal:
// login dengan token, daftar merge request, detail, dan sanitasi deskripsi.
import { createServer } from "node:http";
import { Key } from "./driver.mjs";

const TOKEN = "glpat-uji-rahasia-123";

function mr(iid, title, extra = {}) {
  return {
    id: 1000 + iid,
    iid,
    title,
    state: "opened",
    draft: false,
    work_in_progress: false,
    author: { username: "budi", name: "Budi", avatar_url: null },
    source_branch: "fitur",
    target_branch: "main",
    sha: "8888888888888888888888888888888888888888",
    updated_at: "2026-11-01T19:34:56.000+07:00",
    web_url: `https://gitlab.contoh.id/tim/proyek/-/merge_requests/${iid}`,
    source_project_id: 42,
    target_project_id: 42,
    labels: ["perbaikan"],
    reviewers: [{ username: "ana", name: "Ana", avatar_url: null }],
    ...extra,
  };
}

/** Server GitLab tiruan; mencatat setiap permintaan untuk diperiksa oleh uji. */
async function mockGitLab() {
  const requests = [];
  const server = createServer((req, res) => {
    const url = new URL(req.url, "http://localhost");
    requests.push({ method: req.method, path: url.pathname, query: url.search, auth: req.headers.authorization ?? req.headers["private-token"] ?? null });
    const send = (status, body) => {
      res.writeHead(status, { "content-type": "application/json", "x-next-page": "" });
      res.end(JSON.stringify(body));
    };
    const authorized = (req.headers.authorization ?? "").includes(TOKEN) || req.headers["private-token"] === TOKEN;
    if (!url.pathname.startsWith("/api/v4/")) return send(404, { message: "404 Not Found" });
    if (!authorized) return send(401, { message: "401 Unauthorized" });
    const path = decodeURIComponent(url.pathname.slice("/api/v4".length));
    if (path === "/user") return send(200, { id: 7, username: "budi", name: "Budi Santoso", avatar_url: null });
    if (path === "/projects/tim/proyek") {
      return send(200, { id: 42, path_with_namespace: "tim/proyek", description: null, visibility: "private", http_url_to_repo: "", ssh_url_to_repo: "", default_branch: "main", last_activity_at: "2026-11-01T12:00:00.000Z", web_url: "https://gitlab.contoh.id/tim/proyek" });
    }
    if (path === "/projects/tim/proyek/merge_requests") {
      return send(200, [mr(5, "Tambah validasi login"), mr(6, "Rapikan modul auth", { draft: true })]);
    }
    if (path === "/projects/tim/proyek/merge_requests/5") {
      return send(200, mr(5, "Tambah validasi login", {
        description: "Menambah **validasi** pada formulir.\n\n<script>window.__xss = 1</script>\n<img src=x onerror=\"window.__xss = 2\">\n\n[dokumen](https://contoh.id/dok)",
        merge_status: "can_be_merged",
        changes_count: "3",
      }));
    }
    if (path === "/projects/tim/proyek/merge_requests/5/commits") return send(200, [{ id: "a" }, { id: "b" }]);
    if (path === "/projects/tim/proyek/pipelines") return send(200, [{ id: 900, status: "success", web_url: "https://gitlab.contoh.id/p/900" }]);
    if (path === "/projects/tim/proyek/pipelines/900/jobs") {
      return send(200, [{ id: 1, name: "uji", status: "success", web_url: "https://gitlab.contoh.id/j/1" }, { id: 2, name: "lint", status: "failed", web_url: "https://gitlab.contoh.id/j/2" }]);
    }
    return send(404, { message: "404 Not Found" });
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  return { origin: `http://127.0.0.1:${server.address().port}`, requests, close: () => new Promise((resolve) => server.close(resolve)) };
}

async function palette(app, text, pick) {
  await app.shortcut("P", { ctrl: true, shift: true });
  await app.waitForSelector(".oc-palette-input");
  await app.type(".oc-palette-input", text);
  await app.waitForText(".oc-palette-item", pick);
  await app.keys(Key.Enter);
}

export const tests = {
  async "login GitLab dengan token, daftar dan detail merge request"(t) {
    const gitlab = await mockGitLab();
    try {
      const repo = t.repo();
      t.write(repo, "a.txt", "a\n");
      t.commitAll(repo, "Commit pertama");
      t.git(repo, "remote", "add", "origin", `${gitlab.origin}/tim/proyek.git`);
      const app = await t.launch(repo);
      await app.waitForSelector(".scm-commit");

      // Login: host lain → GitLab self-hosted → alamat server → token (login lewat browser belum dikonfigurasi).
      await palette(app, "Sign In to", "Sign In to GitHub or GitLab");
      await app.waitForSelector(".oc-palette-input");
      await app.type(".oc-palette-input", "Other");
      await app.waitForText(".oc-palette-item", "Other host");
      await app.keys(Key.Enter);
      await app.waitForText(".oc-dialog", "Sign in to another host");
      await app.clickText(".oc-dialog button", "GitLab (self-hosted)");
      await app.waitForText(".oc-dialog", "Server address");
      await app.setValue(".oc-dialog input", gitlab.origin);
      await app.clickText(".oc-dialog button", "Continue");
      await app.waitForSelector('.oc-dialog input[type="password"]', { timeout: 15000 });
      await app.setValue('.oc-dialog input[type="password"]', TOKEN);
      await app.clickText(".oc-dialog button", "Sign In");
      await app.waitFor("dialog login tertutup", "return document.querySelector('.oc-dialog input[type=\"password\"]') === null", { timeout: 15000 });

      const userCalls = gitlab.requests.filter((r) => r.path === "/api/v4/user");
      t.assert(userCalls.length >= 1 && userCalls.every((r) => String(r.auth).includes(TOKEN)), "token dikirim ke server saat memeriksa akun");
      t.assert(!(await app.exec("return document.body.innerHTML.includes(arguments[0])", TOKEN)), "token tidak pernah muncul di halaman");

      // Tanpa layanan keychain (seperti di lingkungan uji), pengguna diberi tahu bahwa login hanya untuk sesi ini.
      if (await app.exec("return [...document.querySelectorAll('.oc-toast')].some((e) => e.textContent.includes('keychain'))")) {
        t.assert.includes((await app.texts(".oc-toast")).join(" "), "kept in memory only", "pemberitahuan cadangan keychain");
      }

      // Daftar merge request untuk repositori aktif.
      await app.click('.oc-activity[aria-label="Pull Requests"]');
      await app.waitForText(".fg-account", "budi", { timeout: 15000 });
      await app.waitForText(".fg-pr", "Tambah validasi login", { timeout: 15000 });
      const rows = await app.texts(".fg-pr-title");
      t.assert.equal(rows.map((r) => r.replace(/Draft$/, "")), ["!5 Tambah validasi login", "!6 Rapikan modul auth"], "nomor dan judul merge request");
      t.assert.includes((await app.texts(".fg-pr")).join(" "), "Draft", "merge request draf diberi tanda");
      t.assert.includes(await app.exec("return document.querySelector('.oc-sidebar').innerText"), "Merge Request", "istilah GitLab dipakai");

      // Detail: deskripsi Markdown dirender, tetapi skrip dan penangan event dari server dibuang.
      await app.clickText(".fg-pr", "Tambah validasi login");
      await app.waitForText(".oc-tab-label", "Tambah validasi login", { timeout: 15000 });
      await app.waitFor("deskripsi dirender", "return [...document.querySelectorAll('.oc-group strong')].some((e) => e.textContent === 'validasi')", { timeout: 15000 });
      t.assert.equal(await app.exec("return window.__xss ?? null"), null, "skrip dari deskripsi tidak dijalankan");
      t.assert(!(await app.exec("return document.querySelector('.oc-group script, .oc-group img[onerror]') !== null")), "elemen berbahaya tidak ada di DOM");
      await app.waitForText(".oc-group", "lint", { timeout: 15000 });
      t.assert.includes(await app.exec("return document.querySelector('.oc-group').innerText"), "uji", "status CI ditampilkan");
      t.assert(gitlab.requests.every((r) => r.path.startsWith("/api/v4/")), `hanya API yang dipanggil: ${JSON.stringify([...new Set(gitlab.requests.map((r) => r.path))])}`);
    } finally {
      await gitlab.close();
    }
  },
};
