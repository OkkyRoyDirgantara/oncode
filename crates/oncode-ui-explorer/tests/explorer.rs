//! Uji tampilan Explorer, layar awal, dan mode terbatas lewat jendela sungguhan (tanpa layar)
//! dengan backend sungguhan. Hasil akhirnya diperiksa di disk dan lewat `git`.

use std::path::Path;

use gpui_kit::test::{TestEventExt as _, TestQueryExt as _, TestWindowExt as _};
use gpui_kit::{App, AppContext as _, Entity, Focusable, IntoElement, MouseButton, ParentElement as _, Render, Styled as _, TestAppContext, Window};
use oncode_ui::editor::{active_file, buffers, open_file, OpenFile};
use oncode_ui::testing::{git, TestApp, TestWindow};
use oncode_ui::{Command, OpenOptions, SidebarView, TabContent, TabData, TabKind, ViewTab, Workbench};
use oncode_ui_explorer::tree::Row;
use oncode_ui_explorer::Explorer;

/// Path sebagai teks dalam bentuk yang dipakai tab dan buffer (`\` di Windows), sehingga
/// `project.join("src/app")` sama dengan path yang dibuat explorer.
fn text(path: &Path) -> String {
    oncode_ui::editor::file_path(&path.to_string_lossy())
}

/// Baris pohon sebagai teks: path relatif, `/` untuk folder, lekukan per tingkat.
fn shape(explorer: &Entity<Explorer>, root: &Path, cx: &App) -> Vec<String> {
    let root = text(root);
    explorer
        .read(cx)
        .rows()
        .iter()
        .map(|row| match row {
            Row::Entry { entry, depth, .. } => {
                // Path entri memakai pemisah sistem (`\` di Windows); bentuk teksnya selalu `/`.
                let relative = entry.path.strip_prefix(&root).unwrap_or(&entry.path).trim_start_matches(['/', '\\']).replace('\\', "/");
                format!("{}{relative}{}", "  ".repeat(*depth), if entry.is_dir { "/" } else { "" })
            }
            Row::Note { depth, text, .. } => format!("{}({text})", "  ".repeat(*depth)),
        })
        .collect()
}

struct Fixture {
    window: TestWindow,
    workbench: Entity<Workbench>,
    explorer: Entity<Explorer>,
}

impl Fixture {
    /// Membuka folder dan menunggu sampai isi akar pohon termuat.
    async fn open(app: &TestApp, project: &Path, cx: &mut TestAppContext) -> Self {
        let window = app.open(Some(project), cx).await;
        let fixture = Self::current(window, cx);
        fixture.settle(cx).await;
        fixture
    }

    /// Workbench dan Explorer yang sedang terpasang (berganti saat folder dipercaya).
    fn current(window: TestWindow, cx: &mut TestAppContext) -> Self {
        let workbench = window.workbench(cx);
        let explorer = cx.update(|cx| oncode_ui_explorer::explorer(workbench.read(cx)).expect("explorer terdaftar"));
        Self { window, workbench, explorer }
    }

    /// Menunggu sampai tidak ada folder yang sedang dimuat.
    async fn settle(&self, cx: &mut TestAppContext) {
        let explorer = self.explorer.clone();
        self.window.wait_until(cx, "pohon termuat", move |_, cx| !explorer.read(cx).rows().is_empty() && !explorer.read(cx).is_loading()).await;
    }

    async fn wait_for_rows(&self, cx: &mut TestAppContext, what: &str, root: &Path, condition: impl Fn(&[String]) -> bool) {
        let explorer = self.explorer.clone();
        let root = root.to_path_buf();
        self.window.wait_until(cx, what, move |_, cx| condition(&shape(&explorer, &root, cx))).await;
    }

    fn click(&self, cx: &mut TestAppContext, id: &str) {
        let id = id.to_string();
        self.window.update(cx, |window, cx| {
            window.render_frame(cx);
            window.click(gpui_kit::SharedString::from(id), cx);
        });
    }

    fn double_click(&self, cx: &mut TestAppContext, id: &str) {
        let id = id.to_string();
        self.window.update(cx, |window, cx| {
            window.render_frame(cx);
            window.double_click(gpui_kit::SharedString::from(id), cx);
        });
    }

    fn press(&self, cx: &mut TestAppContext, key: &str) {
        self.window.update(cx, |window, cx| {
            window.render_frame(cx);
            window.press(key, cx);
        });
        cx.run_until_parked();
    }

    fn input(&self, cx: &mut TestAppContext, text: &str) {
        self.window.update(cx, |window, cx| {
            window.render_frame(cx);
            window.input(text, cx);
        });
        cx.run_until_parked();
    }

    fn select(&self, cx: &mut TestAppContext, path: &Path) {
        let path = text(path);
        self.explorer.update(cx, |explorer, cx| explorer.select(Some(path), cx));
    }

    fn selected(&self, cx: &mut TestAppContext) -> Option<String> {
        cx.update(|cx| self.explorer.read(cx).selected().map(String::from))
    }

    /// Memindahkan fokus keyboard ke pohon lewat command, seperti Ctrl+Shift+E.
    fn focus_tree(&self, cx: &mut TestAppContext) {
        assert!(self.window.run_command(cx, "explorer.focus"));
        self.window.update(cx, |window, cx| window.render_frame(cx));
        assert!(self.window.update(cx, |window, cx| self.explorer.read(cx).tree_focused(window)), "fokus berada di pohon");
    }

    async fn wait_for_active_file(&self, cx: &mut TestAppContext, path: &Path) {
        let (workbench, path) = (self.workbench.clone(), text(path));
        self.window
            .wait_until(cx, "berkas aktif dimuat", move |_, cx| active_file(workbench.read(cx), cx).is_some_and(|(_, buffer, _)| buffer.read(cx).path() == path && buffer.read(cx).is_ready()))
            .await;
    }

    fn active_tab(&self, cx: &mut TestAppContext) -> Option<TabData> {
        cx.update(|cx| self.workbench.read(cx).model().active().map(|(_, tab)| tab.clone()))
    }

    fn tab_ids(&self, cx: &mut TestAppContext) -> Vec<String> {
        cx.update(|cx| self.workbench.read(cx).model().all_tabs().map(|tab| tab.id.clone()).collect())
    }
}

fn setup(cx: &mut TestAppContext) -> TestApp {
    let app = TestApp::new(cx);
    cx.update(oncode_ui_explorer::init);
    app
}

// ----- Skenario e2e v0.2: explorer membuka berkas; sunting, simpan, dan penanda Git mengikuti -----

#[gpui_kit::test]
async fn opens_a_file_from_the_tree_and_git_marks_follow_the_edit(cx: &mut TestAppContext) {
    let app = setup(cx);
    let project = app.git_project(&[
        ("README.md", "# Demo\n\nBaris satu\n"),
        ("src/main.rs", "fn main() {}\n"),
        ("src/util.rs", "pub fn util() {}\n"),
        (".gitignore", "target/\n"),
        ("target/build.log", "keluaran\n"),
    ]);
    let f = Fixture::open(&app, &project, cx).await;

    let rows = cx.update(|cx| shape(&f.explorer, &project, cx));
    assert!(rows.contains(&"src/".to_string()) && rows.contains(&"README.md".to_string()), "isi akar tampil: {rows:?}");
    assert!(!rows.iter().any(|row| row.contains("main.rs")), "isi folder belum dimuat sebelum dibuka: {rows:?}");
    let folders_first = rows.iter().position(|row| row == "src/").unwrap() < rows.iter().position(|row| row == "README.md").unwrap();
    assert!(folders_first, "folder lebih dulu: {rows:?}");
    // Entri yang diabaikan git ditandai agar digambar redup.
    let ignored = cx.update(|cx| f.explorer.read(cx).rows().iter().filter_map(Row::entry).find(|entry| entry.name == "target").map(|entry| entry.ignored));
    assert_eq!(ignored, Some(true));

    f.click(cx, "explorer-row-src");
    f.wait_for_rows(cx, "isi src dimuat", &project, |rows| rows.contains(&"  src/main.rs".to_string())).await;

    // Klik tunggal: tab pratinjau.
    f.click(cx, "explorer-row-src/main.rs");
    f.wait_for_active_file(cx, &project.join("src/main.rs")).await;
    assert!(f.active_tab(cx).unwrap().preview, "klik tunggal membuka pratinjau");
    assert_eq!(f.selected(cx), Some(text(&project.join("src/main.rs"))));

    // Klik ganda: tab tetap.
    f.double_click(cx, "explorer-row-README.md");
    f.wait_for_active_file(cx, &project.join("README.md")).await;
    let tab = f.active_tab(cx).unwrap();
    assert_eq!((tab.title.as_str(), tab.preview), ("README.md", false));

    f.input(cx, "Baris baru\n");
    let (_, buffer, state) = cx.update(|cx| active_file(f.workbench.read(cx), cx).unwrap());
    assert!(cx.update(|cx| buffer.read(cx).is_dirty()));
    assert_eq!(std::fs::read_to_string(project.join("README.md")).unwrap(), "# Demo\n\nBaris satu\n", "belum ditulis ke disk sebelum disimpan");

    assert!(f.window.run_command(cx, "file.save"));
    f.window.wait_until(cx, "tersimpan", |_, cx| !buffer.read(cx).is_dirty()).await;
    assert_eq!(std::fs::read_to_string(project.join("README.md")).unwrap(), cx.update(|cx| state.read(cx).value().to_string()));
    assert_eq!(git(&project, &["status", "--porcelain"]).trim(), "M README.md", "git melihat perubahan");

    let readme = text(&project.join("README.md"));
    let explorer = f.explorer.clone();
    f.window.wait_until(cx, "explorer menandai berkas berubah", move |_, cx| explorer.read(cx).decoration(&readme, false).is_some_and(|d| d.letter == "M")).await;

    // Folder menunjukkan ada perubahan di dalamnya; berkas baru bertanda U.
    std::fs::write(project.join("src/baru.rs"), "// baru\n").unwrap();
    let (src, new_file) = (text(&project.join("src")), text(&project.join("src/baru.rs")));
    let explorer = f.explorer.clone();
    f.window
        .wait_until(cx, "folder dan berkas baru bertanda", move |_, cx| {
            let explorer = explorer.read(cx);
            explorer.decoration(&src, true).is_some_and(|d| d.letter.is_empty()) && explorer.decoration(&new_file, false).is_some_and(|d| d.letter == "U")
        })
        .await;
}

// ----- Navigasi keyboard -----

#[gpui_kit::test]
async fn moves_the_selection_and_expands_or_collapses_with_the_keyboard(cx: &mut TestAppContext) {
    let app = setup(cx);
    let project = app.project(&[("docs/a.md", "a"), ("src/app/store.ts", "s"), ("src/main.ts", "m"), ("README.md", "# r\n")]);
    let f = Fixture::open(&app, &project, cx).await;
    assert_eq!(cx.update(|cx| shape(&f.explorer, &project, cx)), ["docs/", "src/", "README.md"]);

    f.focus_tree(cx);
    f.press(cx, "down");
    assert_eq!(f.selected(cx), Some(text(&project.join("docs"))));
    f.press(cx, "up");
    assert_eq!(f.selected(cx), Some(text(&project.join("docs"))), "tidak melewati baris pertama");
    f.press(cx, "down");
    assert_eq!(f.selected(cx), Some(text(&project.join("src"))));

    // Kanan membuka folder; kanan lagi masuk ke anak pertamanya.
    f.press(cx, "right");
    f.wait_for_rows(cx, "src terbuka", &project, |rows| rows == ["docs/", "src/", "  src/app/", "  src/main.ts", "README.md"]).await;
    assert_eq!(f.selected(cx), Some(text(&project.join("src"))));
    f.press(cx, "right");
    assert_eq!(f.selected(cx), Some(text(&project.join("src/app"))));

    // Kiri pada folder tertutup pindah ke induknya; kiri pada folder terbuka menutupnya.
    f.press(cx, "left");
    assert_eq!(f.selected(cx), Some(text(&project.join("src"))));
    f.press(cx, "left");
    assert_eq!(cx.update(|cx| shape(&f.explorer, &project, cx)), ["docs/", "src/", "README.md"]);

    f.press(cx, "end");
    assert_eq!(f.selected(cx), Some(text(&project.join("README.md"))));
    f.press(cx, "home");
    assert_eq!(f.selected(cx), Some(text(&project.join("docs"))));

    // Enter membuka/menutup folder dan membuka berkas sebagai pratinjau.
    f.press(cx, "enter");
    f.wait_for_rows(cx, "docs terbuka", &project, |rows| rows.contains(&"  docs/a.md".to_string())).await;
    f.press(cx, "end");
    f.press(cx, "enter");
    f.wait_for_active_file(cx, &project.join("README.md")).await;
    assert!(f.active_tab(cx).unwrap().preview);
}

// ----- Berkas di tab aktif disorot -----

#[gpui_kit::test]
async fn reveals_the_file_of_the_active_tab_by_expanding_its_ancestors(cx: &mut TestAppContext) {
    let app = setup(cx);
    let project = app.project(&[("src/app/deep/store.ts", "s"), ("src/main.ts", "m"), ("README.md", "# r\n")]);
    let f = Fixture::open(&app, &project, cx).await;
    let store = project.join("src/app/deep/store.ts");

    let path = text(&store);
    f.window.update(cx, |window, cx| f.workbench.update(cx, |workbench, cx| open_file(workbench, OpenFile::new(&path), window, cx)));
    f.wait_for_rows(cx, "leluhur terbuka", &project, |rows| rows == ["src/", "  src/app/", "    src/app/deep/", "      src/app/deep/store.ts", "  src/main.ts", "README.md"]).await;
    assert_eq!(f.selected(cx), Some(text(&store)));

    // Path di luar proyek tidak mengubah pohon.
    let outside = text(&app.dirs.path().join("luar.txt"));
    f.explorer.update(cx, |explorer, cx| explorer.reveal(&outside, cx));
    cx.run_until_parked();
    assert_eq!(f.selected(cx), Some(text(&store)));
}

// ----- Entri baru -----

#[gpui_kit::test]
async fn creates_a_nested_file_inside_the_selected_folder_reveals_and_opens_it(cx: &mut TestAppContext) {
    let app = setup(cx);
    let project = app.project(&[("src/a.txt", "a\n"), ("README.md", "# r\n")]);
    let f = Fixture::open(&app, &project, cx).await;

    f.select(cx, &project.join("src"));
    assert!(f.window.run_command(cx, "explorer.newFile"));
    assert!(cx.update(|cx| f.explorer.read(cx).is_editing()));
    f.settle(cx).await;
    assert!(cx.update(|cx| f.explorer.read(cx).is_expanded(&text(&project.join("src")))), "folder tujuan dibuka");

    // Nama yang sudah ada ditolak dengan pesan, dan Enter tidak membuat apa pun.
    f.input(cx, "a.txt");
    assert!(cx.update(|cx| f.explorer.read(cx).edit_error()).unwrap().contains("already exists"));
    f.press(cx, "enter");
    assert!(cx.update(|cx| f.explorer.read(cx).is_editing()), "masukan tetap terbuka selama namanya tidak sah");

    // Escape membatalkan.
    f.press(cx, "escape");
    assert!(!cx.update(|cx| f.explorer.read(cx).is_editing()));

    assert!(f.window.run_command(cx, "explorer.newFile"));
    f.input(cx, "lib/baru.rs");
    assert_eq!(cx.update(|cx| f.explorer.read(cx).edit_error()), None);
    f.press(cx, "enter");

    let created = project.join("src/lib/baru.rs");
    f.wait_for_active_file(cx, &created).await;
    assert!(created.is_file(), "berkas dibuat beserta folder antaranya");
    assert!(!f.active_tab(cx).unwrap().preview, "berkas baru dibuka sebagai tab tetap");
    f.wait_for_rows(cx, "berkas baru tampil", &project, |rows| rows == ["src/", "  src/lib/", "    src/lib/baru.rs", "  src/a.txt", "README.md"]).await;
    assert_eq!(f.selected(cx), Some(text(&created)));
}

#[gpui_kit::test]
async fn creates_a_folder_at_the_project_root_from_the_toolbar(cx: &mut TestAppContext) {
    let app = setup(cx);
    let project = app.project(&[("README.md", "# r\n")]);
    let f = Fixture::open(&app, &project, cx).await;

    f.click(cx, "explorer-new-folder");
    assert!(cx.update(|cx| f.explorer.read(cx).is_editing()));
    f.input(cx, "docs");
    f.press(cx, "enter");
    f.wait_for_rows(cx, "folder baru tampil", &project, |rows| rows == ["docs/", "README.md"]).await;
    assert!(project.join("docs").is_dir());
    assert_eq!(f.selected(cx), Some(text(&project.join("docs"))));
    assert!(f.tab_ids(cx).is_empty(), "folder baru tidak membuka tab");
}

// ----- Ganti nama -----

#[gpui_kit::test]
async fn renames_a_file_with_f2_and_keeps_unsaved_content_on_the_new_path(cx: &mut TestAppContext) {
    let app = setup(cx);
    let project = app.project(&[("notes.txt", "satu\n"), ("lain.txt", "lain\n")]);
    let f = Fixture::open(&app, &project, cx).await;

    f.double_click(cx, "explorer-row-notes.txt");
    f.wait_for_active_file(cx, &project.join("notes.txt")).await;
    f.input(cx, "dua ");
    assert!(cx.update(|cx| f.workbench.read(cx).has_dirty_tabs(cx)));

    f.focus_tree(cx);
    f.press(cx, "f2");
    assert!(cx.update(|cx| f.explorer.read(cx).is_editing()), "F2 membuka masukan nama");
    // Nama tanpa ekstensi terpilih, jadi mengetik hanya mengganti bagian itu.
    f.input(cx, "catatan");
    f.press(cx, "enter");

    let renamed = project.join("catatan.txt");
    f.wait_for_rows(cx, "nama baru tampil", &project, |rows| rows == ["catatan.txt", "lain.txt"]).await;
    assert!(renamed.is_file() && !project.join("notes.txt").exists(), "berkas diganti nama di disk");
    assert_eq!(f.selected(cx), Some(text(&renamed)));
    assert_eq!(f.tab_ids(cx), [format!("file:{}", text(&renamed))]);
    assert_eq!(f.active_tab(cx).unwrap().title, "catatan.txt");

    // Isi yang belum disimpan ikut pindah, dan menyimpan menulis ke path baru.
    f.wait_for_active_file(cx, &renamed).await;
    let (_, buffer, state) = cx.update(|cx| active_file(f.workbench.read(cx), cx).unwrap());
    assert_eq!(cx.update(|cx| state.read(cx).value().to_string()), "dua satu\n");
    assert!(cx.update(|cx| buffer.read(cx).is_dirty()));
    assert_eq!(std::fs::read_to_string(&renamed).unwrap(), "satu\n");
    buffer.update_in_window(&f.window, cx);
    f.window.wait_until(cx, "tersimpan", |_, cx| !buffer.read(cx).is_dirty()).await;
    assert_eq!(std::fs::read_to_string(&renamed).unwrap(), "dua satu\n");
    assert!(!project.join("notes.txt").exists(), "path lama tidak ditulis lagi");
}

/// Menyimpan buffer lewat jendela uji.
trait SaveInWindow {
    fn update_in_window(&self, window: &TestWindow, cx: &mut TestAppContext);
}

impl SaveInWindow for Entity<oncode_ui::editor::Buffer> {
    fn update_in_window(&self, window: &TestWindow, cx: &mut TestAppContext) {
        window.update(cx, |window, cx| self.update(cx, |buffer, cx| buffer.save(window, cx)).detach());
    }
}

#[gpui_kit::test]
async fn renaming_a_folder_rewrites_open_tabs_and_keeps_it_expanded(cx: &mut TestAppContext) {
    let app = setup(cx);
    let project = app.project(&[("src/deep/a.ts", "a\n"), ("src/b.ts", "b\n"), ("other.ts", "o\n")]);
    let f = Fixture::open(&app, &project, cx).await;

    for file in ["src/deep/a.ts", "src/b.ts", "other.ts"] {
        let path = text(&project.join(file));
        f.window.update(cx, |window, cx| f.workbench.update(cx, |workbench, cx| open_file(workbench, OpenFile::new(&path), window, cx)));
        f.wait_for_active_file(cx, &project.join(file)).await;
        // Tunggu reveal selesai (folder leluhur dimuat dan dibuka) sebelum berkas berikutnya:
        // reveal yang belum selesai digantikan oleh berkas aktif yang baru.
        let explorer = f.explorer.clone();
        f.window.wait_until(cx, "berkas terpilih di pohon", move |_, cx| explorer.read(cx).selected() == Some(path.as_str())).await;
    }
    f.wait_for_rows(cx, "pohon terbuka", &project, |rows| rows == ["src/", "  src/deep/", "    src/deep/a.ts", "  src/b.ts", "other.ts"]).await;

    f.select(cx, &project.join("src"));
    f.focus_tree(cx);
    f.press(cx, "f2");
    // Nama folder terpilih seluruhnya.
    f.input(cx, "lib");
    f.press(cx, "enter");

    f.wait_for_rows(cx, "folder dimuat di path baru", &project, |rows| rows == ["lib/", "  lib/deep/", "    lib/deep/a.ts", "  lib/b.ts", "other.ts"]).await;
    assert!(project.join("lib/deep/a.ts").is_file() && !project.join("src").exists());
    assert_eq!(f.selected(cx), Some(text(&project.join("lib"))));
    let mut ids = f.tab_ids(cx);
    ids.sort();
    let mut expected: Vec<String> = ["lib/b.ts", "lib/deep/a.ts", "other.ts"].iter().map(|file| format!("file:{}", text(&project.join(file)))).collect();
    expected.sort();
    assert_eq!(ids, expected, "tab berkas di dalam folder ditulis ulang, tab lain tidak");

    // Tab yang ditulis ulang tetap bisa dimuat dari path barunya.
    let path = text(&project.join("lib/b.ts"));
    f.window.update(cx, |window, cx| f.workbench.update(cx, |workbench, cx| open_file(workbench, OpenFile::new(&path), window, cx)));
    f.wait_for_active_file(cx, &project.join("lib/b.ts")).await;
    let (_, _, state) = cx.update(|cx| active_file(f.workbench.read(cx), cx).unwrap());
    assert_eq!(cx.update(|cx| state.read(cx).value().to_string()), "b\n");
    assert!(cx.update(|cx| buffers(f.workbench.read(cx)).unwrap().read(cx).dirty(cx).is_empty()));
}

// ----- Perubahan dari luar -----

#[gpui_kit::test]
async fn follows_files_created_and_removed_outside_the_app(cx: &mut TestAppContext) {
    let app = setup(cx);
    let project = app.project(&[("src/app/store.ts", "s"), ("src/a.ts", "a"), ("README.md", "# r\n")]);
    let f = Fixture::open(&app, &project, cx).await;

    f.click(cx, "explorer-row-src");
    f.wait_for_rows(cx, "src terbuka", &project, |rows| rows.contains(&"  src/a.ts".to_string())).await;
    f.click(cx, "explorer-row-src/app");
    f.wait_for_rows(cx, "app terbuka", &project, |rows| rows.contains(&"    src/app/store.ts".to_string())).await;
    f.select(cx, &project.join("src/app/store.ts"));

    std::fs::write(project.join("src/baru.ts"), "b").unwrap();
    f.wait_for_rows(cx, "berkas dari luar tampil", &project, |rows| rows == ["src/", "  src/app/", "    src/app/store.ts", "  src/a.ts", "  src/baru.ts", "README.md"]).await;
    assert_eq!(f.selected(cx), Some(text(&project.join("src/app/store.ts"))), "bentangan dan pilihan dipertahankan");

    // Folder yang hilang membawa pergi bentangan dan pilihan di bawahnya.
    std::fs::remove_dir_all(project.join("src/app")).unwrap();
    f.wait_for_rows(cx, "folder terhapus hilang", &project, |rows| rows == ["src/", "  src/a.ts", "  src/baru.ts", "README.md"]).await;
    assert_eq!(f.selected(cx), None);
    assert!(!cx.update(|cx| f.explorer.read(cx).is_expanded(&text(&project.join("src/app")))));

    // Refresh dan Collapse All lewat command.
    assert!(f.window.run_command(cx, "explorer.refresh"));
    f.settle(cx).await;
    assert!(f.window.run_command(cx, "explorer.collapseAll"));
    assert_eq!(cx.update(|cx| shape(&f.explorer, &project, cx)), ["src/", "README.md"]);
}

// ----- Salin path dan menu konteks -----

#[gpui_kit::test]
async fn copies_the_absolute_and_relative_path_of_the_selection(cx: &mut TestAppContext) {
    let app = setup(cx);
    let project = app.project(&[("src/a.ts", "a")]);
    let f = Fixture::open(&app, &project, cx).await;
    assert!(!f.window.run_command(cx, "explorer.copyPath"), "tanpa pilihan command tidak aktif");

    f.click(cx, "explorer-row-src");
    f.wait_for_rows(cx, "src terbuka", &project, |rows| rows.contains(&"  src/a.ts".to_string())).await;
    f.select(cx, &project.join("src/a.ts"));

    assert!(f.window.run_command(cx, "explorer.copyRelativePath"));
    assert_eq!(cx.read_from_clipboard().and_then(|item| item.text()), Some("src/a.ts".to_string()));
    assert!(f.window.run_command(cx, "explorer.copyPath"));
    assert_eq!(cx.read_from_clipboard().and_then(|item| item.text()), Some(text(&project.join("src/a.ts"))));
}

#[gpui_kit::test]
async fn the_context_menu_acts_on_the_row_under_the_pointer(cx: &mut TestAppContext) {
    let app = setup(cx);
    let project = app.project(&[("src/a.ts", "a"), ("README.md", "# r\n")]);
    let f = Fixture::open(&app, &project, cx).await;

    f.window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.right_click("explorer-row-README.md", cx);
    });
    cx.run_until_parked();
    assert_eq!(f.selected(cx), Some(text(&project.join("README.md"))), "klik kanan memilih barisnya");

    let item = f.window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.find_by_label("Copy Relative Path").bounds().center()
    });
    f.window.update(cx, |window, cx| {
        window.pointer_move(item, None, cx);
        window.pointer_down(item, MouseButton::Left, cx);
        window.pointer_up(item, MouseButton::Left, cx);
    });
    cx.run_until_parked();
    assert_eq!(cx.read_from_clipboard().and_then(|item| item.text()), Some("README.md".to_string()));

    // Menu untuk folder menawarkan pembuatan di dalamnya, bukan "Open".
    f.window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.right_click("explorer-row-src", cx);
    });
    cx.run_until_parked();
    let (has_new_file, has_open) = f.window.update(cx, |window, cx| {
        window.render_frame(cx);
        (window.try_find_by_label("New File…").is_some(), window.try_find_by_label("Open").is_some())
    });
    assert!(has_new_file && !has_open);
}

// ----- Skenario e2e v0.2: layar awal tanpa folder; Settings tetap bisa dibuka -----

struct FakeSettings {
    focus_handle: gpui_kit::FocusHandle,
}

impl Focusable for FakeSettings {
    fn focus_handle(&self, _: &App) -> gpui_kit::FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for FakeSettings {
    fn render(&mut self, _: &mut Window, _: &mut gpui_kit::Context<Self>) -> impl IntoElement {
        gpui_kit::div().size_full().child("Settings")
    }
}

struct FakeSettingsKind;

impl TabKind for FakeSettingsKind {
    fn build(&self, _: &TabData, _: &oncode_ui::Env, _: &mut Window, cx: &mut App) -> Box<dyn TabContent> {
        Box::new(ViewTab(cx.new(|cx| FakeSettings { focus_handle: cx.focus_handle() })))
    }
}

/// Pengganti crate pengaturan: command `settings.open` yang membuka sebuah tab.
fn register_fake_settings(cx: &mut App) {
    oncode_ui::register_tab_kind(cx, "settings", FakeSettingsKind);
    oncode_ui::register_feature(cx, |workbench, _, cx| {
        let weak = cx.weak_entity();
        workbench.register_command(Command::new("settings.open", "Open Settings", move |_, window, cx| {
            let _ = weak.update(cx, |workbench, cx| {
                workbench.open_tab(TabData::new("settings", "Settings", serde_json::Value::Null), OpenOptions::default(), window, cx);
            });
        }));
    });
}

#[gpui_kit::test]
async fn welcome_screen_without_a_folder_still_opens_settings(cx: &mut TestAppContext) {
    let app = setup(cx);
    cx.update(register_fake_settings);
    let window = app.open(None, cx).await;
    let workbench = window.workbench(cx);
    let welcome = cx.update(|cx| oncode_ui_explorer::welcome(workbench.read(cx)).unwrap());
    window.wait_until(cx, "info aplikasi dimuat", |_, cx| welcome.read(cx).info().is_some()).await;

    let visible = |cx: &mut TestAppContext, id: &'static str| {
        window.update(cx, |window, cx| {
            window.render_frame(cx);
            window.try_find(id).is_some_and(|element| element.visible())
        })
    };
    assert!(visible(cx, "welcome-title"), "layar awal tampil");
    assert!(visible(cx, "welcome-open-folder") && visible(cx, "welcome-clone") && visible(cx, "welcome-settings"));
    assert!(!visible(cx, "welcome-git-problem"), "git tersedia di mesin uji");
    assert!(visible(cx, "explorer-open-folder"), "sidebar menawarkan membuka folder");
    // Tanpa crate Source Control, `git.clone` tidak terdaftar dan tombolnya nonaktif.
    assert_eq!(window.update(cx, |window, _| window.find("welcome-clone").disabled()), Some(true));

    window.update(cx, |window, cx| window.click("welcome-settings", cx));
    cx.run_until_parked();
    assert_eq!(cx.update(|cx| workbench.read(cx).model().active().map(|(_, tab)| tab.kind.clone())), Some("settings".to_string()));
    assert!(!visible(cx, "welcome-title"), "tab Settings menggantikan layar awal");

    // Menutup tab terakhir kembali ke layar awal.
    assert!(window.run_command(cx, "workbench.closeTab"));
    cx.run_until_parked();
    assert!(visible(cx, "welcome-title"));
}

#[gpui_kit::test]
async fn welcome_lists_recent_folders_removes_one_and_opens_another(cx: &mut TestAppContext) {
    let app = setup(cx);
    let first = app.project(&[("a.txt", "a")]);
    let second = app.project(&[("b.txt", "b")]);
    app.core.store.add_recent(&text(&first));
    app.core.store.add_recent(&text(&second));

    let window = app.open(None, cx).await;
    let workbench = window.workbench(cx);
    let welcome = cx.update(|cx| oncode_ui_explorer::welcome(workbench.read(cx)).unwrap());
    window.wait_until(cx, "folder terakhir dimuat", |_, cx| welcome.read(cx).info().is_some_and(|info| info.recent.len() == 2)).await;

    let remove = format!("welcome-recent-remove-{}", text(&first));
    window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.click(gpui_kit::SharedString::from(remove), cx);
    });
    let core = app.core.clone();
    let kept = text(&second);
    window.wait_until(cx, "folder dibuang dari daftar", move |_, _| core.store.recent() == [kept.clone()]).await;
    assert!(cx.update(|cx| workbench.read(cx).env().root.is_none()), "membuang dari daftar tidak membuka folder");

    let open = format!("welcome-recent-{}", text(&second));
    window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.click(gpui_kit::SharedString::from(open), cx);
    });
    window.wait_until(cx, "folder terakhir dibuka", |shell, cx| shell.read(cx).workspace().is_some_and(|workspace| workspace.root == text(&second))).await;

    // Folder terbuka tanpa tab: petunjuk pintasan, bukan layar awal.
    let (hint, title) = window.update(cx, |window, cx| {
        window.render_frame(cx);
        (window.try_find("editor-hint").is_some(), window.try_find("welcome-title").is_some())
    });
    assert!(hint && !title);
}

// ----- Skenario e2e v0.2: folder baru dibuka dalam mode terbatas sampai dipercaya -----

struct FakeScm {
    focus_handle: gpui_kit::FocusHandle,
}

impl Render for FakeScm {
    fn render(&mut self, _: &mut Window, _: &mut gpui_kit::Context<Self>) -> impl IntoElement {
        gpui_kit::InteractiveElement::track_focus(gpui_kit::div(), &self.focus_handle).child("Source Control")
    }
}

/// Pengganti crate Source Control: seperti aslinya, hanya terdaftar bila folder dipercaya.
fn register_fake_scm(cx: &mut App) {
    oncode_ui::register_feature(cx, |workbench, _, _| {
        if !workbench.env().trusted {
            return;
        }
        workbench.register_sidebar(SidebarView::new("scm", "Source Control", oncode_ui::Lucide::GitBranch, 30, |_, _, cx| cx.new(|cx| FakeScm { focus_handle: cx.focus_handle() }).into()));
    });
}

#[gpui_kit::test]
async fn an_untrusted_folder_opens_in_restricted_mode_until_trusted(cx: &mut TestAppContext) {
    let app = setup(cx);
    cx.update(register_fake_scm);
    // Repositori git yang belum dipercaya.
    let project = app.project(&[("README.md", "# Demo\n"), ("src/main.rs", "fn main() {}\n")]);
    git(&project, &["init", "-q", "-b", "main"]);
    git(&project, &["add", "-A"]);
    git(&project, &["commit", "-q", "-m", "Commit pertama"]);

    let f = Fixture::open(&app, &project, cx).await;
    let env = cx.update(|cx| f.workbench.read(cx).env().clone());
    assert!(!env.trusted && env.repo_root.is_none(), "folder baru belum dipercaya");
    let (banner, status) = f.window.update(cx, |window, cx| {
        window.render_frame(cx);
        (window.try_find("restricted-banner").is_some_and(|e| e.visible()), window.try_find("status-restricted").is_some_and(|e| e.visible()))
    });
    assert!(banner && status, "banner di Explorer dan item status bar tampil");
    assert_eq!(cx.update(|cx| f.workbench.read(cx).active_sidebar().map(|id| id.to_string())), Some("explorer".to_string()));
    assert!(cx.update(|cx| oncode_ui::repos(cx).read(cx).roots().is_empty()), "Git tidak dijalankan di mode terbatas");

    // Pohon tetap bisa dipakai; bentangannya harus bertahan setelah folder dipercaya.
    f.click(cx, "explorer-row-src");
    f.wait_for_rows(cx, "src terbuka", &project, |rows| rows.contains(&"  src/main.rs".to_string())).await;

    f.click(cx, "restricted-trust");
    f.window.wait_until(cx, "folder dipercaya", |shell, cx| shell.read(cx).workspace().is_some_and(|workspace| workspace.trusted && workspace.repo_root.is_some())).await;
    assert!(app.core.store.is_trusted(&project));

    let f = Fixture::current(f.window, cx);
    f.settle(cx).await;
    let (banner, status) = f.window.update(cx, |window, cx| {
        window.render_frame(cx);
        (window.try_find("restricted-banner").is_some(), window.try_find("status-restricted").is_some())
    });
    assert!(!banner && !status, "pemberitahuan mode terbatas hilang");
    assert_eq!(cx.update(|cx| f.workbench.read(cx).active_sidebar().map(|id| id.to_string())), Some("scm".to_string()), "Source Control menjadi tampilan aktif setelah folder dipercaya");
    f.wait_for_rows(cx, "bentangan dipertahankan", &project, |rows| rows.contains(&"  src/main.rs".to_string())).await;
    assert!(!f.window.run_command(cx, "workspace.trust"), "command percaya tidak lagi aktif");
}

#[gpui_kit::test]
async fn startup_view_setting_keeps_the_explorer_for_a_repository(cx: &mut TestAppContext) {
    let app = setup(cx);
    cx.update(register_fake_scm);
    let mut settings = app.core.user_settings();
    settings.workbench.startup_view = oncode_core::settings::StartupView::Explorer;
    app.core.set_user_settings(settings).unwrap();

    let project = app.git_project(&[("a.txt", "a\n")]);
    let f = Fixture::open(&app, &project, cx).await;
    assert!(cx.update(|cx| f.workbench.read(cx).env().repo_root.is_some()));
    assert_eq!(cx.update(|cx| f.workbench.read(cx).active_sidebar().map(|id| id.to_string())), Some("explorer".to_string()));
}

#[gpui_kit::test]
async fn a_trusted_repository_starts_on_source_control_by_default(cx: &mut TestAppContext) {
    let app = setup(cx);
    cx.update(register_fake_scm);
    let project = app.git_project(&[("a.txt", "a\n")]);
    let f = Fixture::open(&app, &project, cx).await;
    assert_eq!(cx.update(|cx| f.workbench.read(cx).active_sidebar().map(|id| id.to_string())), Some("scm".to_string()));

    // Explorer tetap bisa dipanggil dan dipakai lewat command.
    f.focus_tree(cx);
    assert_eq!(cx.update(|cx| f.workbench.read(cx).active_sidebar().map(|id| id.to_string())), Some("explorer".to_string()));
}
