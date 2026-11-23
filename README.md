# OnCode

Editor kode desktop **native** yang lebih sederhana daripada Visual Studio Code, dengan **Git client lengkap** sebagai fitur utama. Ditulis dalam Rust dan digambar langsung lewat GPU dengan [GPUI](https://github.com/zed-industries/zed) (kerangka UI milik editor Zed), tanpa WebView dan tanpa JavaScript.

Rancangan lengkap ada di [docs/RANCANGAN.md](docs/RANCANGAN.md); panduan pengembangan di [docs/PENGEMBANGAN.md](docs/PENGEMBANGAN.md).

> Versi sebelumnya (v0.2) dibangun dengan Tauri + React. Kodenya tersimpan di tag git `tauri-v0.2` dan rancangannya di [docs/arsip/RANCANGAN-v0.2-tauri.md](docs/arsip/RANCANGAN-v0.2-tauri.md).

## Fitur

**Source control**

- Status perubahan, stage / unstage / discard per berkas, **per hunk, dan per baris**
- Commit (amend, sign-off), branch, merge, fetch / pull / push dengan dialog kredensial di dalam aplikasi
- Graph riwayat commit dengan pencarian, detail commit, dan perbandingan dua commit
- **Interactive rebase** visual (pick, reword, edit, squash, fixup, drop, ubah urutan)
- **Editor konflik tiga panel** (Current | Result | Incoming)
- Stash, tag, cherry-pick, revert, reset, blame, riwayat berkas, reflog
- Worktree dan submodule
- Semua perintah Git yang dijalankan tercatat di panel **Git Output**

**Editor dan workspace**

- Explorer dengan penanda status Git, tab dan split editor, quick open (`Ctrl+P`), command palette (`Ctrl+Shift+P`)
- Highlight sintaks (tree-sitter), cari/ganti, multi-kursor, lipat kode
- Pencarian dan penggantian teks di seluruh proyek (menghormati `.gitignore`)
- Deteksi encoding dan akhir baris, deteksi perubahan dari luar, pemulihan tab saat folder dibuka lagi
- Terminal terintegrasi
- LSP: completion, hover, diagnostics, go to definition, references, rename, format (language server dipasang sendiri)
- GitHub dan GitLab: login, clone dari akun, daftar / detail / checkout / buat pull request

**Yang sengaja tidak ada:** marketplace ekstensi, debugger, remote development, notebook, sinkronisasi pengaturan, fitur AI.

## Menjalankan

Prasyarat: Rust (stable), Git 2.40+, compiler C, dan pustaka sistem GPUI (lihat [docs/PENGEMBANGAN.md](docs/PENGEMBANGAN.md#prasyarat)). Saat berjalan diperlukan GPU dengan Vulkan (Linux), DirectX 11 (Windows), atau Metal (macOS).

```bash
cargo build -p oncode-helper        # binary pendamping untuk dialog kredensial dan interactive rebase
cargo run -p oncode                 # mode pengembangan
cargo run -p oncode -- /path/proyek # langsung membuka sebuah folder
```

## Pengujian

```bash
cargo test --workspace  # backend (terhadap git sungguhan), logika UI, dan uji tampilan tanpa layar
```

## Struktur

```
app/            Binary `oncode`: titik masuk
crates/
  oncode-core/      State aplikasi tanpa UI dan seluruh API
  oncode-ui/        Kerangka UI: jendela, tab, command, keybinding, editor
  oncode-ui-*/      Satu fitur per crate: explorer, search, diff, scm, graph, terminal, lsp, forge, settings
  oncode-git/ oncode-workspace/ oncode-terminal/ oncode-lsp/ oncode-forge/ oncode-helper/   Backend
docs/           Rancangan dan panduan pengembangan
```

## Keamanan

Folder yang baru pertama kali dibuka berjalan dalam **mode terbatas**: Git, language server, dan pengaturan proyek dinonaktifkan sampai Anda memercayai folder itu, karena repositori yang tidak dikenal dapat menjalankan kode lewat konfigurasi Git atau language server. Token akun disimpan hanya di keychain sistem operasi.

## Lisensi

MIT
