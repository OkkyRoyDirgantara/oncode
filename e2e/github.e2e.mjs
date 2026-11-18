// Alur GitHub di aplikasi sungguhan terhadap server GitHub Enterprise tiruan: login lewat browser
// (device flow), daftar pull request, dan push yang memakai token akun tanpa dialog kredensial.
import { mkdirSync, rmSync, writeFileSync } from "node:fs";
import { createServer } from "node:http";
import { join } from "node:path";
import { Key } from "./driver.mjs";
import { gitHttpServer } from "./servers.mjs";

const TOKEN = "gho_token-uji-rahasia";

async function mockGitHub() {
  const state = { polls: 0, starts: 0, requests: [] };
  const server = createServer((req, res) => {
    const url = new URL(req.url, "http://localhost");
    let body = "";
    req.on("data", (chunk) => (body += chunk));
    req.on("end", () => {
      state.requests.push({ method: req.method, path: url.pathname, body, auth: req.headers.authorization ?? null });
      const send = (status, value) => {
        res.writeHead(status, { "content-type": "application/json" });
        res.end(JSON.stringify(value));
      };
      const origin = `http://${req.headers.host}`;
      if (req.method === "POST" && url.pathname === "/login/device/code") {
        // Seperti server sungguhan, tiap permintaan mendapat kode yang berbeda.
        state.starts += 1;
        return send(200, { device_code: `kode-perangkat-rahasia-${state.starts}`, user_code: `ABCD-${1000 + state.starts}`, verification_uri: `${origin}/login/device`, expires_in: 900, interval: 1 });
      }
      if (req.method === "POST" && url.pathname === "/login/oauth/access_token") {
        state.polls += 1;
        // Polling pertama: pengguna belum menyetujui. Polling kedua: disetujui.
        return state.polls < 2 ? send(200, { error: "authorization_pending" }) : send(200, { access_token: TOKEN, token_type: "bearer", scope: "repo,read:user" });
      }
      if (!url.pathname.startsWith("/api/v3/")) return send(404, { message: "Not Found" });
      if (req.headers.authorization !== `Bearer ${TOKEN}`) return send(401, { message: "Bad credentials" });
      const path = url.pathname.slice("/api/v3".length);
      if (path === "/user") return send(200, { login: "ana", id: 1, name: "Ana", avatar_url: "" });
      if (path === "/repos/tim/proyek/pulls") {
        return send(200, [
          {
            number: 12,
            title: "Perbaiki validasi",
            state: "open",
            draft: false,
            merged_at: null,
            user: { login: "budi", avatar_url: "" },
            head: { ref: "fitur", sha: "6dcb09b5b57875f334f61aebed695e2e4193db5e", repo: { full_name: "tim/proyek" } },
            base: { ref: "main", sha: "aaaa09b5b57875f334f61aebed695e2e4193db5e", repo: { full_name: "tim/proyek" } },
            updated_at: "2026-11-01T19:01:12Z",
            html_url: "https://github.contoh.id/tim/proyek/pull/12",
          },
        ]);
      }
      return send(404, { message: "Not Found" });
    });
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  return { origin: `http://127.0.0.1:${server.address().port}`, state, close: () => new Promise((resolve) => server.close(resolve)) };
}

export const tests = {
  async "login device flow, daftar pull request, dan push memakai token akun"(t) {
    // Client ID OAuth diisi lewat pengaturan agar login lewat browser tersedia.
    mkdirSync(t.configDir, { recursive: true });
    const settingsFile = join(t.configDir, "settings.json");
    writeFileSync(settingsFile, JSON.stringify({ forge: { githubClientId: "Iv1.uji" } }));
    const github = await mockGitHub();
    const root = join(t.dir, "server");
    mkdirSync(join(root, "tim", "proyek.git"), { recursive: true });
    t.git(join(root, "tim", "proyek.git"), "init", "-q", "--bare", "-b", "main");
    // Server git hanya menerima kredensial yang dipakai aplikasi untuk akun GitHub: x-access-token + token.
    const gitServer = await gitHttpServer(root, (user, password) => user === "x-access-token" && password === TOKEN);
    try {
      const repo = t.repo();
      t.write(repo, "a.txt", "a\n");
      t.commitAll(repo, "Commit pertama");
      t.git(repo, "remote", "add", "origin", `${gitServer.origin}/tim/proyek.git`);
      const app = await t.launch(repo);
      await app.waitForSelector(".scm-commit");

      await app.shortcut("P", { ctrl: true, shift: true });
      await app.waitForSelector(".oc-palette-input");
      await app.type(".oc-palette-input", "Sign In to");
      await app.waitForText(".oc-palette-item", "Sign In to GitHub or GitLab");
      await app.keys(Key.Enter);
      await app.waitForSelector(".oc-palette-input");
      await app.type(".oc-palette-input", "Other");
      await app.waitForText(".oc-palette-item", "Other host");
      await app.keys(Key.Enter);
      await app.waitForText(".oc-dialog", "Sign in to another host");
      await app.clickText(".oc-dialog button", "GitHub Enterprise");
      await app.waitForText(".oc-dialog", "Server address");
      await app.setValue(".oc-dialog input", github.origin);
      await app.clickText(".oc-dialog button", "Continue");

      // Dialog menampilkan kode untuk dimasukkan di browser, lalu polling sampai disetujui.
      await app.waitFor("kode pengguna ditampilkan", "return [...document.querySelectorAll('.oc-dialog')].some((d) => /ABCD-\\d{4}/.test(d.textContent))", { timeout: 15000 });
      t.assert(!(await app.exec("return document.body.innerText.includes('kode-perangkat-rahasia')")), "device code tidak pernah sampai ke halaman");
      await app.waitFor("login selesai setelah disetujui", "return ![...document.querySelectorAll('.oc-dialog')].some((d) => d.textContent.includes('ABCD-'))", { timeout: 20000 });
      t.assert(github.state.polls >= 2, "aplikasi melakukan polling sampai token diberikan");
      const start = github.state.requests.find((r) => r.path === "/login/device/code");
      t.assert.includes(start.body, "client_id=Iv1.uji", "Client ID dari pengaturan dipakai");

      await app.click('.oc-activity[aria-label="Pull Requests"]');
      await app.waitForText(".fg-account", "ana", { timeout: 15000 });
      await app.waitForText(".fg-pr", "Perbaiki validasi", { timeout: 15000 });
      t.assert.includes(await app.exec("return document.querySelector('.oc-sidebar').innerText"), "Pull Request", "istilah GitHub dipakai");

      // Push ke host yang sama memakai token akun: tidak boleh ada dialog kredensial.
      await app.shortcut("G", { ctrl: true, shift: true });
      await app.waitForText(".oc-sidebar button", "Publish Branch");
      await app.clickText(".oc-sidebar button", "Publish Branch");
      await app.waitFor("branch terpublikasi", "return ![...document.querySelectorAll('.oc-sidebar button')].some((b) => b.textContent.includes('Publish Branch'))", { timeout: 20000 });
      t.assert(!(await app.exists(".oc-dialog")), "tidak ada dialog kredensial");
      t.assert.equal(t.git(join(root, "tim", "proyek.git"), "rev-parse", "main").trim(), t.git(repo, "rev-parse", "HEAD").trim(), "commit sampai di server");
      t.assert(gitServer.attempts.every((a) => a.user === "x-access-token"), "git memakai kredensial akun, bukan menanyai pengguna");
      t.assert(!(await app.exec("return document.body.innerHTML.includes(arguments[0])", TOKEN)), "token tidak pernah muncul di halaman");
    } finally {
      await github.close();
      await gitServer.close();
      rmSync(settingsFile, { force: true });
    }
  },
};
