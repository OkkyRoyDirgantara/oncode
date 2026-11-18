# OnCode

Editor kode desktop yang lebih sederhana daripada Visual Studio Code, dengan **Git client lengkap** sebagai fitur utama. Dibangun dengan Tauri 2 (Rust) dan CodeMirror 6.

> **Sedang beralih ke native.** OnCode dirancang ulang sebagai aplikasi native (GPUI, tanpa WebView); rancangannya ada di [docs/RANCANGAN.md](docs/RANCANGAN.md) dan belum diimplementasikan. Isi README ini dan kode di repositori masih menggambarkan **versi Tauri (v0.2)**, yang rancangannya diarsipkan di [docs/arsip/RANCANGAN-v0.2-tauri.md](docs/arsip/RANCANGAN-v0.2-tauri.md).

Panduan pengembangan versi Tauri ada di [docs/PENGEMBANGAN.md](docs/PENGEMBANGAN.md).

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
- Highlight sintaks (dimuat saat dibutuhkan), cari/ganti, multi-kursor, lipat kode
- Pencarian dan penggantian teks di seluruh proyek (menghormati `.gitignore`)
- Deteksi encoding dan akhir baris, deteksi perubahan dari luar, pemulihan tab saat folder dibuka lagi
- Terminal terintegrasi
- LSP: completion, hover, diagnostics, go to definition, references, rename, format (language server dipasang sendiri)
- GitHub dan GitLab: login, clone dari akun, daftar / detail / checkout / buat pull request

**Yang sengaja tidak ada:** marketplace ekstensi, debugger, remote development, notebook, sinkronisasi pengaturan, fitur AI.

## Menjalankan

Prasyarat: Rust (stable), Node.js 22+ dengan pnpm, Git 2.40+, dan pustaka sistem Tauri (lihat [docs/PENGEMBANGAN.md](docs/PENGEMBANGAN.md#prasyarat)).

```bash
pnpm install
pnpm tauri dev          # mode pengembangan
pnpm tauri build        # installer rilis
```

Membuka folder tertentu: `oncode /path/ke/proyek`.

## Pengujian

```bash
cargo test --workspace  # backend, termasuk uji integrasi terhadap repositori git sungguhan
pnpm test               # frontend (Vitest)
pnpm e2e                # end-to-end terhadap aplikasi sungguhan (Linux/Windows)
```

## Struktur

```
src/            Frontend React + TypeScript (features/: editor, scm, graph, explorer, search, terminal, lsp, problems, forge, settings)
src-tauri/      Shell Tauri: command IPC, state, event
crates/         oncode-git, oncode-workspace, oncode-terminal, oncode-lsp, oncode-forge, oncode-helper
e2e/            Uji end-to-end (tauri-driver)
docs/           Rancangan dan panduan pengembangan
```

## Keamanan

Folder yang baru pertama kali dibuka berjalan dalam **mode terbatas**: Git, language server, dan pengaturan proyek dinonaktifkan sampai Anda memercayai folder itu, karena repositori yang tidak dikenal dapat menjalankan kode lewat konfigurasi Git atau language server. Token akun disimpan hanya di keychain sistem operasi.

## Lisensi

MIT
