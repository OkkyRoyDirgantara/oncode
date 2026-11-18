# Panduan Pengembangan

> **Sedang beralih ke native.** Panduan ini berlaku untuk **versi Tauri (v0.2)**, yaitu kode yang ada di repositori saat ini. Rancangan versi native ada di [RANCANGAN.md](RANCANGAN.md) dan belum diimplementasikan; panduan ini ditulis ulang pada tahap N0 rancangan tersebut. Rancangan versi Tauri diarsipkan di [arsip/RANCANGAN-v0.2-tauri.md](arsip/RANCANGAN-v0.2-tauri.md).

## Prasyarat

| Kebutuhan | Catatan |
|---|---|
| Rust (stable, ≥ 1.88) | `rustup` |
| Node.js ≥ 22 dan pnpm ≥ 12 | |
| Git ≥ 2.40 | Dipakai aplikasi saat berjalan dan oleh uji integrasi |
| Pustaka sistem Tauri | Lihat di bawah |

**Linux (Debian/Ubuntu):**

```bash
sudo apt install libwebkit2gtk-4.1-dev build-essential curl wget file \
  libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev
```

**Windows:** Microsoft C++ Build Tools dan WebView2 Runtime. **macOS:** Xcode Command Line Tools.

### Tanpa akses root (`.dev-sysroot`)

Bila paket di atas tidak bisa dipasang (tidak ada `sudo`), paket `-dev` dan compiler C dapat diunduh dengan `apt-get download` lalu diekstrak ke folder `.dev-sysroot/` di dalam proyek, tanpa mengubah sistem. Folder itu tidak ikut di-commit. Setelah tersedia, aktifkan sebelum menjalankan `cargo`:

```bash
source .dev-sysroot/env.sh
```

Ini hanya jalan pintas untuk mesin pengembangan; memasang paket sistem dengan `apt` tetap cara yang dianjurkan.

## Perintah sehari-hari

| Tujuan | Perintah |
|---|---|
| Mode pengembangan | `pnpm tauri dev` |
| Uji backend | `cargo test --workspace` |
| Uji satu crate | `cargo test -p oncode-git` |
| Periksa tipe frontend | `pnpm typecheck` |
| Uji frontend | `pnpm test` |
| Uji end-to-end | `cargo build -p oncode && pnpm e2e` |
| Perbarui binding TypeScript | `cargo test -p oncode export_bindings` |
| Build frontend produksi | `pnpm build` |
| Installer rilis | `pnpm tauri build` |

`pnpm tauri dev` dan `pnpm tauri build` otomatis membangun `oncode-helper` dan menyalinnya ke `src-tauri/binaries/` (lihat `scripts/build-helper.mjs`).

## Arsitektur singkat

- **`crates/oncode-git`** menjalankan `git` milik sistem dan mem-parse keluarannya. Tidak bergantung pada Tauri; uji integrasinya (`tests/repo.rs`) berjalan terhadap repositori sementara.
- **`crates/oncode-helper`** adalah binary kecil yang dipanggil git sebagai `GIT_ASKPASS`, `GIT_EDITOR`, dan `GIT_SEQUENCE_EDITOR`. Ia meneruskan permintaan ke aplikasi lewat socket lokal dengan token sekali pakai.
- **`crates/oncode-workspace`**, **`-terminal`**, **`-lsp`**, **`-forge`** menangani berkas/pencarian, PTY, proses language server, dan API GitHub/GitLab.
- **`src-tauri`** membungkus semuanya sebagai command IPC (`src-tauri/src/commands/`), menyimpan state (`state.rs`), dan menyiarkan event (`events.rs`).
- **`src/ipc/bindings.ts`** dihasilkan oleh `tauri-specta` dari command dan tipe Rust. **Jangan disunting**; perbarui dengan `cargo test -p oncode export_bindings` setiap kali command atau tipe berubah.
- **`src/app`** adalah kerangka frontend: registry fitur (`registry.ts`), command dan keybinding (`commands.ts`), tab (`workbench.ts`), dialog/menu/toast (`overlay.tsx`).
- **`src/features/<nama>`** berisi satu fitur. Tiap fitur mendaftarkan diri lewat `register<Nama>Feature()` di `src/features/index.ts`.

### Menambah command backend

1. Tulis fungsi `#[tauri::command] #[specta::specta]` di `src-tauri/src/commands/<domain>.rs`. Hindari nama parameter yang merupakan kata kunci TypeScript (`new`, `default`, …).
2. Daftarkan di `specta_builder()` dalam `src-tauri/src/lib.rs`.
3. Jalankan `cargo test -p oncode export_bindings`, lalu pakai dari frontend lewat `commands.<nama>` dari `@/ipc`.

Tipe yang dikirim ke UI tidak boleh memakai `i64`/`u64`/`usize` (ditolak ekspor TypeScript); pakai `u32`, atau `f64` dengan `#[specta(type = u32)]` untuk timestamp.

### Menambah fitur frontend

Buat `src/features/<nama>/index.tsx` yang mengekspor fungsi pendaftaran, lalu panggil dari `src/features/index.ts`. Catatan penting: **selector Zustand harus mengembalikan referensi yang stabil**. `useStore((s) => Object.keys(s.x))` membuat array baru tiap render dan berakhir dengan galat "Maximum update depth exceeded"; ambil nilai tersimpannya lalu turunkan dengan `useMemo`.

## Uji end-to-end

`pnpm e2e` (atau `node e2e/run.mjs e2e/scm.e2e.mjs` untuk satu berkas) meluncurkan binary debug lewat `tauri-driver` dan mengujinya dari luar: mengeklik UI lalu memeriksa hasilnya dengan `git` dan sistem berkas.

Prasyarat:

- Binary debug: `cargo build -p oncode`.
- `tauri-driver` (`cargo install tauri-driver --locked`) dan, di Linux, `WebKitWebDriver` (paket `webkitgtk-webdriver`).
- Tanpa layar (`DISPLAY` kosong), pelari memakai `xvfb-run` dan `dbus-run-session`.
- Untuk uji LSP: `pnpm --dir e2e/fixtures/ts-server install --ignore-workspace`. Tanpa itu uji LSP dilewati.
- Untuk uji SSH: `sshd` dan `ssh-keygen` (paket `openssh-server`). Tanpa itu uji SSH dilewati. `sshd` dijalankan sebagai pengguna biasa di port acak dengan kunci dan `known_hosts` sementara; `~/.ssh` tidak disentuh.
- Server GitHub/GitLab tiruan dan server git HTTP untuk uji autentikasi dijalankan oleh ujinya sendiri di `127.0.0.1`; tidak ada permintaan ke internet.

`E2E_SHOTS=1 pnpm e2e` menyimpan tangkapan layar akhir tiap kasus ke `e2e/screenshots/`. Kasus yang gagal selalu menyimpan tangkapan layar.

`tauri-driver` tidak mendukung macOS; di sana pengujian mengandalkan uji unit, uji integrasi, dan pemeriksaan manual.

## Membangun installer Windows dari Linux

Kompilasi silang ke Windows didukung Tauri secara eksperimental, lewat target `x86_64-pc-windows-gnu` (MinGW) dan NSIS:

```bash
rustup target add x86_64-pc-windows-gnu
sudo apt install gcc-mingw-w64-x86-64-posix g++-mingw-w64-x86-64-posix nsis
node scripts/build-helper.mjs --release --target=x86_64-pc-windows-gnu
pnpm tauri build --target x86_64-pc-windows-gnu --bundles nsis
```

Hasilnya `target/x86_64-pc-windows-gnu/release/bundle/nsis/OnCode_<versi>_x64-setup.exe`, berisi `oncode.exe`, `oncode-helper.exe`, dan `WebView2Loader.dll` (wajib ada di samping `oncode.exe` pada target GNU). Installer tidak ditandatangani.

Tanpa akses root, toolchain MinGW dan NSIS bisa diekstrak ke `.dev-sysroot/win/` seperti paket lain; aktifkan dengan `source .dev-sysroot/env.sh && source .dev-sysroot/env-win.sh`. Bundler Tauri menghapus `NSISDIR` dari lingkungan, jadi `makensis` hasil ekstrak harus dibungkus skrip yang menyetelnya (lihat `.dev-sysroot/win/wrap/makensis`).

Build silang hanya membuktikan aplikasi **terkompilasi dan terkemas**; perilakunya tetap harus dicoba di Windows. Untuk rilis resmi, bangun di Windows (target MSVC) lewat workflow `release.yml`.

## Login GitHub/GitLab lewat browser

Login device flow memerlukan **Client ID aplikasi OAuth** yang Anda daftarkan sendiri (GitHub: *Settings → Developer settings → OAuth Apps*, aktifkan *Device Flow*; GitLab: *Applications* dengan scope `api read_user`). Client ID bisa diisi di *Settings → Accounts*, atau ditanam saat build lewat variabel lingkungan `ONCODE_GITHUB_CLIENT_ID` dan `ONCODE_GITLAB_CLIENT_ID`. Tanpa Client ID, login dengan Personal Access Token tetap tersedia.

## Rilis

- Workflow `.github/workflows/release.yml` membangun installer tiga OS dari tag `v*` dan membuat draf rilis.
- **Penandatanganan kode** belum dikonfigurasi. Tanpa sertifikat Windows dan Apple Developer ID, pengguna akan melihat peringatan keamanan saat memasang. Secret yang dibaca workflow: `APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY`, `APPLE_ID`, `APPLE_PASSWORD`, `APPLE_TEAM_ID`.
- **Pembaruan otomatis** belum dipasang: plugin updater Tauri memerlukan pasangan kunci tanda tangan dan alamat manifes rilis yang hanya bisa ditentukan pemilik proyek. Langkahnya: `pnpm tauri signer generate`, tambahkan `tauri-plugin-updater` beserta `plugins.updater` (`pubkey`, `endpoints`) di `tauri.conf.json`.
- `identifier` aplikasi di `src-tauri/tauri.conf.json` (`dev.oncode.editor`) adalah nilai sementara; ganti dengan domain milik Anda sebelum rilis publik, karena mengubahnya setelah rilis memindahkan lokasi data pengguna.
