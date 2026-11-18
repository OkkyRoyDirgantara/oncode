// Pelari uji end-to-end: menjalankan aplikasi sungguhan lewat tauri-driver dan mengujinya dari luar.
//
//   node e2e/run.mjs [berkas.e2e.mjs ...]
//
// Prasyarat: binary debug sudah dibangun (`cargo build -p oncode`), `tauri-driver` dan
// `WebKitWebDriver` tersedia (PATH, atau TAURI_DRIVER / WEBKIT_DRIVER). Tanpa DISPLAY,
// aplikasi dijalankan di layar virtual lewat `xvfb-run`. Hanya Linux dan Windows:
// tauri-driver tidak mendukung macOS.
import { execFileSync, spawn } from "node:child_process";
import { existsSync, mkdirSync, mkdtempSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { App, sleep } from "./driver.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, "..");
const exe = process.platform === "win32" ? ".exe" : "";
const application = process.env.ONCODE_BINARY ?? join(root, "target", "debug", `oncode${exe}`);

function firstExisting(...candidates) {
  return candidates.find((c) => c && existsSync(c));
}

function onPath(name) {
  for (const dir of (process.env.PATH ?? "").split(process.platform === "win32" ? ";" : ":")) {
    const candidate = join(dir, name + exe);
    if (dir && existsSync(candidate)) return candidate;
  }
  return null;
}

async function reachable(url) {
  try {
    await fetch(url, { signal: AbortSignal.timeout(1500) });
    return true;
  } catch {
    return false;
  }
}

async function waitUntil(description, check, timeout = 30000) {
  const started = Date.now();
  while (!(await check())) {
    if (Date.now() - started > timeout) throw new Error(`Tidak siap dalam ${timeout} ms: ${description}`);
    await sleep(250);
  }
}

const children = [];
function start(command, args, options = {}) {
  const child = spawn(command, args, { stdio: "ignore", detached: process.platform !== "win32", ...options });
  children.push(child);
  return child;
}
function stopAll() {
  for (const child of children) {
    try {
      if (process.platform === "win32") child.kill();
      else process.kill(-child.pid, "SIGTERM");
    } catch {
      // sudah berhenti
    }
  }
}

/** Menjalankan git di `cwd` tanpa terpengaruh konfigurasi pengguna. */
export function git(cwd, ...args) {
  return execFileSync("git", args, {
    cwd,
    encoding: "utf8",
    env: { ...process.env, GIT_CONFIG_GLOBAL: process.platform === "win32" ? "NUL" : "/dev/null", GIT_CONFIG_NOSYSTEM: "1", GIT_TERMINAL_PROMPT: "0" },
  });
}

function assert(condition, message) {
  if (!condition) throw new Error(`Assertion gagal: ${message}`);
}
assert.equal = (actual, expected, message) => {
  const a = JSON.stringify(actual);
  const e = JSON.stringify(expected);
  if (a !== e) throw new Error(`Assertion gagal: ${message}\n  didapat : ${a}\n  harapan : ${e}`);
};
assert.includes = (haystack, needle, message) => {
  if (!haystack.includes(needle)) throw new Error(`Assertion gagal: ${message}\n  tidak ada ${JSON.stringify(needle)} di ${JSON.stringify(haystack)}`);
};

async function main() {
  if (!existsSync(application)) throw new Error(`Binary tidak ditemukan: ${application}. Jalankan \`cargo build -p oncode\` dulu.`);
  const files = process.argv.slice(2).length
    ? process.argv.slice(2).map((f) => resolve(f))
    : readdirSync(here).filter((f) => f.endsWith(".e2e.mjs")).sort().map((f) => join(here, f));

  const work = mkdtempSync(join(tmpdir(), "oncode-e2e-"));
  const shots = join(root, "e2e", "screenshots");
  mkdirSync(shots, { recursive: true });

  // Frontend: binary debug memuat halaman dari server pengembangan Vite.
  if (!(await reachable("http://localhost:1420/"))) {
    start(process.platform === "win32" ? "pnpm.cmd" : "pnpm", ["dev"], { cwd: root });
    await waitUntil("server Vite di port 1420", () => reachable("http://localhost:1420/"), 60000);
  }

  // tauri-driver, dengan data aplikasi terisolasi dari milik pengguna.
  if (!(await reachable("http://127.0.0.1:4444/status"))) {
    const tauriDriver = firstExisting(process.env.TAURI_DRIVER, onPath("tauri-driver"), join(root, ".dev-sysroot", "cargo", "bin", "tauri-driver"));
    if (!tauriDriver) throw new Error("tauri-driver tidak ditemukan (pasang dengan `cargo install tauri-driver --locked`)");
    const nativeDriver = firstExisting(process.env.WEBKIT_DRIVER, onPath("WebKitWebDriver"), join(root, ".dev-sysroot", "root", "usr", "bin", "WebKitWebDriver"));
    const driverArgs = nativeDriver ? ["--native-driver", nativeDriver] : [];
    // Git milik aplikasi juga diisolasi dari konfigurasi pengguna, agar credential helper pengguna
    // tidak ikut menyimpan kredensial uji.
    const gitConfig = join(work, "gitconfig");
    writeFileSync(gitConfig, "");
    // ssh milik aplikasi memakai konfigurasi uji (kunci, known_hosts) alih-alih ~/.ssh pengguna.
    mkdirSync(join(work, "ssh"), { recursive: true });
    writeFileSync(join(work, "ssh", "config"), "");
    const env = {
      ...process.env,
      GIT_SSH_COMMAND: `ssh -F ${join(work, "ssh", "config")}`,
      XDG_DATA_HOME: join(work, "data"),
      XDG_CONFIG_HOME: join(work, "config"),
      APPDATA: join(work, "config"),
      LOCALAPPDATA: join(work, "data"),
      GIT_CONFIG_GLOBAL: gitConfig,
      GIT_CONFIG_NOSYSTEM: "1",
    };
    // Language server untuk uji LSP (opsional): lihat e2e/fixtures/ts-server/package.json.
    const lspBin = join(here, "fixtures", "ts-server", "node_modules", ".bin");
    if (existsSync(lspBin)) env.PATH = `${lspBin}${process.platform === "win32" ? ";" : ":"}${process.env.PATH ?? ""}`;
    const headless = process.platform === "linux" && !process.env.DISPLAY && !process.env.WAYLAND_DISPLAY;
    if (headless) {
      // Sesi D-Bus tersendiri: tanpa sesi desktop yang cocok, aplikasi GTK menunggu ~30 detik pada bus pengguna.
      const xvfb = ["xvfb-run", "-a", "-s", "-screen 0 1440x900x24", tauriDriver, ...driverArgs];
      const dbus = onPath("dbus-run-session");
      if (dbus) start(dbus, ["--", ...xvfb], { env });
      else start(xvfb[0], xvfb.slice(1), { env });
    }
    else start(tauriDriver, driverArgs, { env });
    await waitUntil("tauri-driver di port 4444", () => reachable("http://127.0.0.1:4444/status"));
  }

  let failed = 0;
  let passed = 0;
  let skipped = 0;
  for (const file of files) {
    const module = await import(pathToFileURL(file).href);
    for (const [name, test] of Object.entries(module.tests ?? {})) {
      const label = `${file.slice(here.length + 1)} › ${name}`;
      const dir = mkdtempSync(join(work, "case-"));
      const apps = [];
      const context = {
        dir,
        /** Direktori konfigurasi aplikasi yang dipakai selama uji (settings.json, keybindings.json). */
        configDir: join(work, "config", "dev.oncode.editor"),
        /** Direktori konfigurasi ssh yang dipakai aplikasi selama uji; berkas `config` di dalamnya dibaca ssh. */
        sshDir: join(work, "ssh"),
        assert,
        git,
        sleep,
        /** Repositori baru di direktori sementara, dengan identitas uji. */
        repo(name = "repo") {
          const path = join(dir, name);
          mkdirSync(path, { recursive: true });
          git(path, "init", "-q", "-b", "main");
          git(path, "config", "user.name", "Penguji");
          git(path, "config", "user.email", "penguji@contoh.id");
          git(path, "config", "commit.gpgsign", "false");
          return path;
        },
        write(base, rel, content) {
          const full = join(base, rel);
          mkdirSync(dirname(full), { recursive: true });
          writeFileSync(full, content);
        },
        commitAll(base, message) {
          git(base, "add", "-A");
          git(base, "commit", "-q", "-m", message);
        },
        /** Kasus yang prasyaratnya tidak ada melempar ini agar dihitung sebagai dilewati. */
        skip(reason) {
          const error = new Error(reason);
          error.skip = true;
          throw error;
        },
        /** Meluncurkan aplikasi pada `folder`; folder langsung dipercaya kecuali `trust: false`. */
        async launch(folder, { trust = true } = {}) {
          const app = await App.launch(application, folder ? [folder] : []);
          apps.push(app);
          if (folder && trust) {
            await app.waitFor("jendela siap", "return document.querySelector('.oc-app, .oc-welcome') !== null", { timeout: 20000 });
            if (await app.exec("return [...document.querySelectorAll('button')].some((b) => b.textContent.includes('Trust folder'))")) {
              await app.clickText("button", "Trust folder");
              await app.waitFor("mode terbatas berakhir", "return ![...document.querySelectorAll('button')].some((b) => b.textContent.includes('Trust folder'))");
            }
          }
          return app;
        },
      };
      const started = Date.now();
      try {
        await test(context);
        // E2E_SHOTS=1 menyimpan tangkapan layar akhir tiap kasus, untuk pemeriksaan visual.
        if (process.env.E2E_SHOTS) await apps.at(-1)?.screenshot(join(shots, `ok-${name.replace(/[^a-z0-9]+/gi, "-")}.png`)).catch(() => {});
        passed += 1;
        console.log(`  ✓ ${label} (${Date.now() - started} ms)`);
      } catch (error) {
        if (error.skip) {
          skipped += 1;
          console.log(`  - ${label} (dilewati: ${error.message})`);
          continue;
        }
        failed += 1;
        console.log(`  ✗ ${label}\n    ${String(error.message).replace(/\n/g, "\n    ")}`);
        const shot = join(shots, `${name.replace(/[^a-z0-9]+/gi, "-")}.png`);
        await apps.at(-1)?.screenshot(shot).then(() => console.log(`    tangkapan layar: ${shot}`)).catch(() => {});
        const visible = await apps.at(-1)?.exec("return document.body.innerText.slice(0, 600)").catch(() => null);
        if (visible) console.log(`    teks di layar: ${JSON.stringify(visible)}`);
      } finally {
        for (const app of apps) await app.quit();
      }
    }
  }
  rmSync(work, { recursive: true, force: true });
  console.log(`\n${passed} lolos, ${failed} gagal${skipped ? `, ${skipped} dilewati` : ""}`);
  return failed === 0 ? 0 : 1;
}

let code = 1;
try {
  code = await main();
} catch (error) {
  console.error(error.message);
} finally {
  stopAll();
}
process.exit(code);
