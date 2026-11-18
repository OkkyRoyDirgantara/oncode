// Autentikasi SSH sungguhan terhadap sshd lokal: konfirmasi sidik jari host dan passphrase kunci
// diminta lewat dialog aplikasi (ssh → oncode-helper sebagai SSH_ASKPASS → aplikasi).
import { execFileSync, spawn } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { createServer } from "node:net";
import { homedir, userInfo } from "node:os";
import { join } from "node:path";

const SSHD = "/usr/sbin/sshd";
const PASSPHRASE = "frasa-rahasia-kunci";

function freePort() {
  return new Promise((resolve) => {
    const probe = createServer();
    probe.listen(0, "127.0.0.1", () => {
      const { port } = probe.address();
      probe.close(() => resolve(port));
    });
  });
}

const digest = (path) => (existsSync(path) ? createHash("sha256").update(readFileSync(path)).digest("hex") : null);

/** sshd pribadi di port acak, hanya menerima kunci uji yang dilindungi passphrase. */
async function startSshd(t) {
  const dir = join(t.dir, "sshd");
  mkdirSync(dir, { recursive: true });
  execFileSync("ssh-keygen", ["-q", "-t", "ed25519", "-N", "", "-f", join(dir, "host_key")]);
  // Direktori ssh dipakai bersama antar-kasus; kunci dari kasus sebelumnya dibuang dulu.
  for (const name of ["id_uji", "id_uji.pub"]) rmSync(join(t.sshDir, name), { force: true });
  execFileSync("ssh-keygen", ["-q", "-t", "ed25519", "-N", PASSPHRASE, "-f", join(t.sshDir, "id_uji")]);
  writeFileSync(join(dir, "authorized_keys"), readFileSync(join(t.sshDir, "id_uji.pub")), { mode: 0o600 });
  const port = await freePort();
  writeFileSync(
    join(dir, "sshd_config"),
    [`Port ${port}`, "ListenAddress 127.0.0.1", `HostKey ${join(dir, "host_key")}`, `PidFile ${join(dir, "sshd.pid")}`, `AuthorizedKeysFile ${join(dir, "authorized_keys")}`, "PasswordAuthentication no", "KbdInteractiveAuthentication no", "UsePAM no", "StrictModes no", ""].join("\n"),
  );
  const child = spawn(SSHD, ["-D", "-e", "-f", join(dir, "sshd_config")], { stdio: "ignore" });
  for (let i = 0; i < 40 && !existsSync(join(dir, "sshd.pid")); i++) await t.sleep(100);
  // Konfigurasi ssh yang dibaca aplikasi: kunci uji dan known_hosts tersendiri, tanpa ssh-agent.
  writeFileSync(
    join(t.sshDir, "config"),
    ["Host *", `  UserKnownHostsFile ${join(t.sshDir, "known_hosts")}`, `  IdentityFile ${join(t.sshDir, "id_uji")}`, "  IdentitiesOnly yes", "  IdentityAgent none", ""].join("\n"),
  );
  return { port, stop: () => child.kill() };
}

export const tests = {
  async "push lewat SSH: konfirmasi host dan passphrase kunci lewat dialog"(t) {
    if (process.platform === "win32" || !existsSync(SSHD)) t.skip("sshd tidak tersedia");
    const userKnownHosts = join(homedir(), ".ssh", "known_hosts");
    const before = digest(userKnownHosts);
    const sshd = await startSshd(t);
    try {
      const bare = join(t.dir, "remote.git");
      mkdirSync(bare, { recursive: true });
      t.git(bare, "init", "-q", "--bare", "-b", "main");
      const repo = t.repo();
      t.write(repo, "a.txt", "a\n");
      t.commitAll(repo, "Commit pertama");
      t.git(repo, "remote", "add", "origin", `ssh://${userInfo().username}@127.0.0.1:${sshd.port}${bare}`);

      const app = await t.launch(repo);
      await app.waitForText(".oc-sidebar button", "Publish Branch");
      await app.clickText(".oc-sidebar button", "Publish Branch");

      // Host belum dikenal: sidik jarinya ditampilkan dan harus disetujui secara eksplisit.
      await app.waitForText(".oc-dialog-header", "Confirm host", { timeout: 20000 });
      const question = await app.text(".oc-dialog");
      t.assert.includes(question, "authenticity of host", "pertanyaan dari ssh diteruskan apa adanya");
      t.assert.includes(question, "SHA256:", "sidik jari kunci host ditampilkan");
      await app.clickText(".oc-dialog button", "Yes, Continue");

      await app.waitForText(".oc-dialog-header", "SSH key passphrase", { timeout: 20000 });
      t.assert.equal(await app.exec("return document.querySelector('.oc-dialog input').type"), "password", "passphrase disembunyikan");
      await app.setValue(".oc-dialog input", PASSPHRASE);
      await app.clickText(".oc-dialog button", "Continue");

      await app.waitFor("branch terpublikasi", "return ![...document.querySelectorAll('.oc-sidebar button')].some((b) => b.textContent.includes('Publish Branch'))", { timeout: 30000 });
      t.assert.equal(t.git(bare, "rev-parse", "main").trim(), t.git(repo, "rev-parse", "HEAD").trim(), "commit sampai di server");
      t.assert.includes(readFileSync(join(t.sshDir, "known_hosts"), "utf8"), `[127.0.0.1]:${sshd.port}`, "host yang disetujui dicatat");
      t.assert(!(await app.exec("return document.body.innerText.includes(arguments[0])", PASSPHRASE)), "passphrase tidak tampil di layar");
      t.assert.equal(digest(userKnownHosts), before, "known_hosts milik pengguna tidak tersentuh");
    } finally {
      sshd.stop();
    }
  },

  async "menolak host yang tidak dikenal menghentikan push"(t) {
    if (process.platform === "win32" || !existsSync(SSHD)) t.skip("sshd tidak tersedia");
    const sshd = await startSshd(t);
    try {
      const bare = join(t.dir, "remote.git");
      mkdirSync(bare, { recursive: true });
      t.git(bare, "init", "-q", "--bare", "-b", "main");
      const repo = t.repo();
      t.write(repo, "a.txt", "a\n");
      t.commitAll(repo, "Commit pertama");
      t.git(repo, "remote", "add", "origin", `ssh://${userInfo().username}@127.0.0.1:${sshd.port}${bare}`);
      // known_hosts dari kasus sebelumnya tidak boleh dipakai: port berbeda, jadi host ini tetap belum dikenal.
      const app = await t.launch(repo);
      await app.waitForText(".oc-sidebar button", "Publish Branch");
      await app.clickText(".oc-sidebar button", "Publish Branch");
      await app.waitForText(".oc-dialog-header", "Confirm host", { timeout: 20000 });
      await app.clickText(".oc-dialog button", "No");
      await app.waitForSelector(".oc-toast", { timeout: 20000 });
      await app.waitFor("operasi selesai", "return [...document.querySelectorAll('.oc-sidebar button')].some((b) => b.textContent.includes('Publish Branch') && !b.disabled)", { timeout: 20000 });
      let pushed = true;
      try {
        t.git(bare, "rev-parse", "--verify", "--quiet", "main");
      } catch {
        pushed = false;
      }
      t.assert(!pushed, "tidak ada yang terdorong ke host yang ditolak");
      const known = existsSync(join(t.sshDir, "known_hosts")) ? readFileSync(join(t.sshDir, "known_hosts"), "utf8") : "";
      t.assert(!known.includes(`[127.0.0.1]:${sshd.port}`), "host yang ditolak tidak dicatat");
    } finally {
      sshd.stop();
    }
  },
};
