# Panduan Pengembangan

OnCode adalah aplikasi native: Rust + [GPUI](https://github.com/zed-industries/zed) lewat [`gpui-kit`](https://gpui-kit.com) (komponen `gpui-component`). Rancangannya ada di [RANCANGAN.md](RANCANGAN.md). Versi Tauri (v0.2) tersimpan di tag git `tauri-v0.2`; rancangannya di [arsip/RANCANGAN-v0.2-tauri.md](arsip/RANCANGAN-v0.2-tauri.md).

## Prasyarat

| Kebutuhan | Catatan |
|---|---|
| Rust (stable, ≥ 1.88) | `rustup` |
| Git ≥ 2.40 | Dipakai aplikasi saat berjalan dan oleh uji |
| Compiler C | Untuk grammar tree-sitter |
| Pustaka sistem GPUI | Lihat di bawah |

**Linux (Debian/Ubuntu):**

```bash
sudo apt install build-essential pkg-config libwayland-dev libxkbcommon-dev libxkbcommon-x11-dev \
  libxcb1-dev libx11-dev libfontconfig-dev libfreetype-dev libssl-dev libvulkan1
```

Saat berjalan diperlukan driver Vulkan. Tanpa GPU, Mesa lavapipe (`mesa-vulkan-drivers`) bisa dipakai: `VK_DRIVER_FILES=/usr/share/vulkan/icd.d/lvp_icd.json`.

**Windows:** Microsoft C++ Build Tools. **macOS:** Xcode beserta Command Line Tools.

### Tanpa akses root (`.dev-sysroot`)

Bila paket di atas tidak bisa dipasang, paket `-dev` dan compiler C dapat diunduh dengan `apt-get download` lalu diekstrak ke `.dev-sysroot/` di dalam proyek (tidak ikut di-commit). Aktifkan sebelum menjalankan `cargo`:

```bash
source .dev-sysroot/env.sh
```

Pustaka yang hanya ada di sysroot (mis. `libxkbcommon-x11.so.0`) harus bisa ditemukan saat aplikasi dijalankan: `LD_LIBRARY_PATH=.dev-sysroot/runlib`.

## Perintah sehari-hari

| Tujuan | Perintah |
|---|---|
| Menjalankan | `cargo build -p oncode-helper && cargo run -p oncode -- [folder]` |
| Uji satu crate | `cargo test -p oncode-ui-scm` |
| Semua uji | `cargo test --workspace` |
| Periksa cepat | `cargo check -p oncode` |
| Paket `.deb` | `cargo install cargo-packager --locked`, lalu `cargo packager --release -f deb` di folder `app/` (hasil di `target/packages/`) |
| AppImage | `cargo packager --release -f appimage` di `app/`. Di mesin tanpa `libxkbcommon-x11` terpasang, `linuxdeploy` perlu `LD_LIBRARY_PATH` yang memuat pustaka itu |
| Installer NSIS dari Linux | Perlu `makensis` (paket `nsis`). `cargo packager --release -f nsis --target x86_64-pc-windows-gnu` di `app/`, dengan `CARGO_TARGET_DIR` menunjuk folder build Windows: `--binaries-dir` diabaikan perkakas. Binary Windows-nya harus dibangun dengan `--config "$ONCODE_WIN_PATCH"` (lihat `.dev-sysroot/env-win.sh`) |
| DMG dari Linux (tidak resmi) | Lihat komentar di `.dev-sysroot/env-mac.sh`: zig + macOS SDK, salinan `gpui-pre-apple` dengan shader saat aplikasi mulai, link dengan `rust-lld`, lalu `.dev-sysroot/mac/make-dmg.sh`. Rilis resmi tetap dibuat di macOS (`release.yml`) |
| Rilis | Dorong tag `v*`; `.github/workflows/release.yml` membuat deb/AppImage, NSIS, dan DMG (Apple Silicon dan Intel), lalu mengunggahnya sebagai draf rilis |

`oncode-helper` harus ada di samping binary `oncode-app` (di `target/debug/`); tanpa itu dialog kredensial dan interactive rebase tidak tersedia.

Binary utama sengaja tidak bernama `oncode`: di Windows dengan GPU Intel, nama exe 6 huruf membuat driver memuat cache shader milik Chrome/Edge ke memori aplikasi (± 150 MB). Penjelasan di `app/Cargo.toml`.

Uji LSP memakai language server tiruan (`crates/oncode-lsp/src/mock.rs`, program `mock_lsp_server`); uji membangunnya sendiri bila belum ada. Untuk mencobanya di aplikasi, arahkan `lsp.servers.<bahasa>.command` di `settings.json` ke `target/debug/examples/mock_lsp_server` (kata `ERROR`, `WARN`, `INFO`, `HINT` menghasilkan diagnostics; `def nama` membuat definisi).

## Peta crate

| Crate | Isi |
|---|---|
| `app` (binary `oncode-app`) | `main`: runtime tokio, `Core`, lalu `init` tiap crate UI |
| `crates/oncode-core` | State aplikasi tanpa UI dan seluruh API (`oncode_core::api::{app, fs, git, term, lsp, forge}`) |
| `crates/oncode-git`, `-workspace`, `-terminal`, `-lsp`, `-forge`, `-helper` | Backend; tidak bergantung pada UI |
| `crates/oncode-ui` | Kerangka UI: `Workbench`, `Shell`, command, keybinding, quick pick, dialog, editor dan buffer, store repositori |
| `crates/oncode-ui-<fitur>` | Satu fitur per crate: `explorer`, `search`, `diff`, `scm`, `graph`, `terminal`, `lsp`, `forge`, `settings` |

Crate fitur hanya bergantung pada `oncode-ui`, `oncode-core`, dan crate backend yang diperlukannya (pengecualian: `scm` dan `graph` memakai `oncode-ui-diff` untuk membuka tab diff).

## Aturan penting

### 1. Jangan mengarang API GPUI

GPUI dan `gpui-component` masih pra-1.0. Sebelum memakai sebuah method, **cari tanda tangannya di sumber** (`~/.cargo/registry/src/*/gpui-component-0.10.0`, `gpui-base-0.10.0`, `gpui-pre-0.3.12`). Dokumentasi per komponen: `https://gpui-kit.com/component/<nama>.md` (mis. `editor.md`, `diff.md`, `tree.md`, `list.md`, `virtual-list.md`, `menu.md`, `dialog.md`). Impor selalu lewat `gpui_kit::...`, bukan `gpui::...`.

### 2. Aturan entitas (peminjaman ganda = panik)

Entitas GPUI yang sedang di-`update` tidak boleh dibaca atau di-`update` lagi dari dalam pembaruan itu. Akibatnya:

- **Pembangun yang dipanggil `Workbench`** (`SidebarView::build`, `PanelView::build`, `StatusItem::build`, `TabKind::build`, `TabContent::can_close`, fungsi `register_feature`) berjalan selagi `Workbench` dipinjam. Di dalamnya **jangan** `workbench.read(cx)` / `workbench.update(cx, ..)`. Pakai data dari `Env`, atau tunda dengan `cx.defer_in(window, ..)` / `window.defer(cx, ..)`.
- **`render` sebuah tampilan fitur** berjalan di luar pinjaman `Workbench`, jadi boleh `workbench.read(cx)`.
- **Penangan command** (`Command::new(.., |args, window, cx| ..)`) berjalan di luar pinjaman apa pun dan boleh memperbarui entitas mana saja.
- Dari dalam `workbench.update(..)`, jalankan command lain dengan `oncode_ui::execute(id, args, window, cx)` (ditunda), bukan `execute_now`.
- Penangan event (`cx.subscribe`, `cx.observe`) berjalan setelah pembaruan selesai, jadi aman.

### 3. Pekerjaan backend tidak di thread utama

Crate backend memakai tokio; GPUI punya eksekutornya sendiri. Semua panggilan `oncode_core::api::*` dijalankan lewat `oncode_ui::spawn_core`:

```rust
let (core, root) = (env.core.clone(), root.clone());
let task = spawn_core(cx, async move { api::git::git_stage(&core, root, paths).await });
cx.spawn_in(window, async move |this, cx| {
    let result = task.await;
    let _ = this.update_in(cx, |this, window, cx| {
        if let Err(error) = result {
            oncode_ui::overlay::show_error(&error, window, cx);
        }
    });
})
.detach();
```

Jangan memanggil I/O yang menunggu (berkas, proses, jaringan) langsung dari `render` atau penangan event.

### 4. Bahasa

Teks UI dan pesan untuk pengguna berbahasa **Inggris**. Komentar kode dan dokumen berbahasa **Indonesia**. Id command sama dengan v0.2 agar `keybindings.json` pengguna tetap berlaku.

## Menambah fitur

Setiap crate fitur mengekspor `pub fn init(cx: &mut App)` yang dipanggil dari `app/src/main.rs`.

```rust
pub fn init(cx: &mut App) {
    // Jenis tab (global).
    oncode_ui::register_tab_kind(cx, "graph", GraphKind);
    // Per jendela: dipanggil untuk setiap `Workbench` baru (juga saat folder dibuka atau dipercaya).
    oncode_ui::register_feature(cx, |workbench, window, cx| {
        let env = workbench.env().clone();          // core, label jendela, root, trusted, repo_root
        if !env.trusted { return; }                 // mode terbatas: fitur Git/LSP tidak didaftarkan
        workbench.register_sidebar(SidebarView::new("scm", "Source Control", Lucide::GitBranch, 20, |env, window, cx| {
            cx.new(|cx| ScmPanel::new(env.clone(), window, cx)).into()
        }).badge(|cx| /* Option<usize> */ None));
        workbench.register_panel(PanelView::new("git-output", "Git Output", 30, |env, window, cx| ..));
        workbench.register_status(StatusItem::new("git.branch", StatusAlign::Left, 10, |env, window, cx| ..), window, cx);
        workbench.register_command(Command::new("git.commit", "Commit", |_, window, cx| ..).category("Git").key("Mod+Enter").when(|_, cx| ..));
    });
}
```

- **Sidebar, panel, status bar:** tampilan adalah entitas tersendiri (`AnyView`) yang mengamati state yang dibutuhkannya. Urutan: `order` kecil lebih dulu. Explorer 10, Search 20, Source Control 30, Graph 40, Pull Requests 50; panel: Terminal 10, Problems 20, Git Output 30.
- **Tab:** `workbench.open_tab(TabData::new(kind, title, json!({..})), OpenOptions::default(), window, cx)`. Isi tab dibangun `TabKind::build` dan mengimplementasikan `TabContent` (pakai `ViewTab(entity)` untuk tampilan sederhana). Tab bisa diseret (`DraggedTab`); fitur lain boleh menyeret `DraggedTab { from: None, .. }` ke area editor untuk membukanya sebagai tab (dipakai terminal). Jenis tab yang isinya tidak boleh tampil dua kali menimpa `TabKind::split_copy` (terminal: "Split Editor" membuka terminal baru).
- **State per jendela milik fitur:** `workbench.set_extension(entity)` dan `workbench.extension::<Entity<T>>()`.
- **Berkas:** `oncode_ui::editor::open_file(workbench, OpenFile::new(path).at(line, column, len), window, cx)`; buffer lewat `oncode_ui::editor::buffers(workbench)`; berkas aktif lewat `active_file(workbench, cx)`.
- **Repositori:** `oncode_ui::repos(cx)` (snapshot per akar, penanda berkas); berlangganan `ReposEvent`.
- **Event:** `WorkbenchEvent` (tab, tata letak, pengaturan, `FsChanged`, `FileSaved`) lewat `cx.subscribe_in(&workbench, window, ..)`; event backend lain lewat `oncode_ui::core_bus(cx)` (`CoreEvent`).
- **Dialog dan toast:** `oncode_ui::overlay::{confirm, choose, prompt_text, toast_*, show_error}`; pilihan dari daftar: `oncode_ui::quick_pick::quick_pick`. Aksi destruktif selalu lewat `confirm(..danger())` yang menyebut apa yang hilang.
- **Ikon:** `Icon::new(oncode_ui::Lucide::GitBranch)` (seluruh Lucide tersedia).

## Pengujian

Tiga tingkat, semuanya `cargo test`:

1. **Logika murni** (`#[test]`): penerjemahan pilihan, parser, model. Pisahkan logika dari tampilan agar bisa diuji tanpa jendela.
2. **Backend** (`#[tokio::test]` di crate backend dan `oncode-core`): terhadap git sungguhan di repositori sementara.
3. **Tampilan** (`#[gpui_kit::test]`): jendela tanpa layar, masukan disimulasikan, backend sungguhan. Lihat `crates/oncode-ui/tests/editor.rs`.

```rust
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::TestAppContext;
use oncode_ui::testing::{git, TestApp};

#[gpui_kit::test]
async fn stages_a_file(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    cx.update(|cx| oncode_ui_scm::init(cx));                 // daftarkan fitur yang diuji sebelum membuka jendela
    let project = app.git_project(&[("a.txt", "satu\n")]);   // repo git sementara, sudah dipercaya
    std::fs::write(project.join("a.txt"), "dua\n").unwrap();
    let window = app.open(Some(&project), cx).await;
    window.wait_until(cx, "status git", |_, cx| ..).await;   // menunggu backend (thread tokio sungguhan)
    window.update(cx, |window, cx| { window.render_frame(cx); window.click("scm-stage-a.txt", cx); });
    window.wait_until(cx, "ter-stage", |_, cx| ..).await;
    assert_eq!(git(&project, &["diff", "--cached", "--name-only"]).trim(), "a.txt");
}
```

Catatan uji tampilan:

- Jangan `use gpui_kit::*;` di modul uji (membayangi `#[test]`). Elemen yang diklik perlu `.id("...")`; elemen buatan sendiri yang ingin ditemukan `window.find(..)` perlu `.test_support()` (`gpui_kit::test::TestSupportExt`).
- `window.input("teks", cx)` mengetik ke elemen yang fokus; `window.press("enter", cx)` menekan tombol bernama; `window.run_command(cx, "id")` menjalankan command.
- Jangan menunggu dengan `cx.executor().timer(..)` di luar `wait_until`; waktu uji GPUI tidak berjalan sendiri.
- Periksa hasil akhirnya di luar aplikasi (isi berkas, `git`), bukan hanya state tampilan.

### Melihat tampilan tanpa layar

Build debug menerima `ONCODE_DEV_SCRIPT` (lihat `app/src/dev.rs`): daftar langkah JSON (`wait`, `open`, `command`, `type`, `key`) yang dijalankan setelah jendela terbuka. Dengan `Xvfb` dan lavapipe, hasilnya bisa ditangkap dengan `xwd`. Pakai `XDG_CONFIG_HOME`/`XDG_DATA_HOME` sementara agar pengaturan pengguna tidak tersentuh.
