//! Uji tampilan tab diff: stage/unstage/discard per perubahan dan per baris, tampilan unified,
//! dan tampilan pengganti. Hasil akhirnya diperiksa dengan git, bukan hanya dari state tampilan.

use std::path::Path;

use gpui_kit::component::diff::{DiffLineRange, DiffMode, DiffSide};
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{Entity, TestAppContext};
use oncode_git::DiffSource;
use oncode_ui::testing::{git, TestApp, TestWindow};
use oncode_ui::{repos, ViewTab, Workbench};
use oncode_ui_diff::{open_diff, DiffRequest, DiffView};

const README: &str = "# Demo\n\nBaris satu\nBaris dua\nBaris tiga\n";
const README_CHANGED: &str = "# Demo\n\nBaris SATU\nBaris dua\nBaris tiga\nBaris empat\n";

/// Baris yang berubah pada keluaran `git diff -U0`.
fn changed_lines(diff: &str) -> Vec<String> {
    diff.lines().filter(|line| (line.starts_with('+') || line.starts_with('-')) && !line.starts_with("+++") && !line.starts_with("---")).map(String::from).collect()
}

async fn open(app: &TestApp, project: &Path, cx: &mut TestAppContext) -> (TestWindow, Entity<Workbench>, String) {
    cx.update(|cx| oncode_ui_diff::init(cx));
    let window = app.open(Some(project), cx).await;
    let workbench = window.workbench(cx);
    let root = project.to_string_lossy().into_owned();
    window.wait_until(cx, "repositori terbuka", |_, cx| repos(cx).read(cx).snapshot(&root).is_some()).await;
    (window, workbench, root)
}

fn active_diff(workbench: &Entity<Workbench>, cx: &gpui_kit::App) -> Option<Entity<DiffView>> {
    workbench.read(cx).active_content()?.as_any().downcast_ref::<ViewTab<DiffView>>().map(|tab| tab.0.clone())
}

/// Membuka tab diff dan menunggu sampai isinya dimuat.
async fn open_diff_tab(window: &TestWindow, workbench: &Entity<Workbench>, root: &str, path: &str, source: DiffSource, cx: &mut TestAppContext) -> Entity<DiffView> {
    let request = DiffRequest { root: root.to_string(), path: path.to_string(), orig_path: None, source, preview: false };
    window.update(cx, |window, cx| workbench.update(cx, |workbench, cx| open_diff(workbench, request, window, cx)));
    window.wait_until(cx, "diff dimuat", |_, cx| active_diff(workbench, cx).is_some_and(|view| view.read(cx).is_loaded())).await;
    cx.update(|cx| active_diff(workbench, cx).unwrap())
}

fn click(window: &TestWindow, cx: &mut TestAppContext, id: &str) {
    let id = id.to_string();
    window.update(cx, |window, cx| {
        window.render_frame(cx);
        window.click(gpui_kit::SharedString::from(id), cx);
    });
}

#[gpui_kit::test]
async fn stages_one_change_and_keeps_the_other(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let project = app.git_project(&[("README.md", README)]);
    std::fs::write(project.join("README.md"), README_CHANGED).unwrap();
    let (window, workbench, root) = open(&app, &project, cx).await;

    let view = open_diff_tab(&window, &workbench, &root, "README.md", DiffSource::Worktree, cx).await;
    assert_eq!(cx.update(|cx| view.read(cx).change_count()), 2, "dua perubahan dengan tombol aksi");
    let tab = cx.update(|cx| workbench.read(cx).model().active().map(|(_, tab)| (tab.title.clone(), tab.kind.clone())).unwrap());
    assert_eq!(tab, ("README.md (Working Tree)".to_string(), "diff".to_string()));

    click(&window, cx, "diff-stage-0");
    window.wait_until(cx, "perubahan pertama masuk index", |_, _| !git(&project, &["diff", "--cached", "--name-only"]).trim().is_empty()).await;
    assert_eq!(changed_lines(&git(&project, &["diff", "--cached", "-U0"])), ["-Baris satu", "+Baris SATU"], "hanya perubahan pertama yang masuk index");
    assert_eq!(changed_lines(&git(&project, &["diff", "-U0"])), ["+Baris empat"], "perubahan kedua tetap belum di-stage");
    window.wait_until(cx, "diff tinggal satu perubahan", |_, cx| view.read(cx).change_count() == 1).await;

    // Perubahan yang tersisa bisa di-commit terpisah.
    git(&project, &["commit", "-q", "-m", "Ubah baris satu"]);
    assert_eq!(git(&project, &["show", "HEAD:README.md"]), "# Demo\n\nBaris SATU\nBaris dua\nBaris tiga\n");
}

#[gpui_kit::test]
async fn unstages_one_change_then_discards_the_other_after_confirming(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let project = app.git_project(&[("README.md", README)]);
    std::fs::write(project.join("README.md"), README_CHANGED).unwrap();
    git(&project, &["add", "README.md"]);
    let (window, workbench, root) = open(&app, &project, cx).await;

    // Diff staged: lepaskan perubahan pertama dari index.
    let staged = open_diff_tab(&window, &workbench, &root, "README.md", DiffSource::Staged, cx).await;
    assert_eq!(cx.update(|cx| staged.read(cx).change_count()), 2);
    click(&window, cx, "diff-unstage-0");
    window.wait_until(cx, "perubahan pertama keluar dari index", |_, _| changed_lines(&git(&project, &["diff", "-U0"])) == ["-Baris satu", "+Baris SATU"]).await;
    assert_eq!(changed_lines(&git(&project, &["diff", "--cached", "-U0"])), ["+Baris empat"]);

    // Diff working tree: buang perubahan itu; dibatalkan dulu, lalu dikonfirmasi.
    let worktree = open_diff_tab(&window, &workbench, &root, "README.md", DiffSource::Worktree, cx).await;
    window.wait_until(cx, "diff working tree", |_, cx| worktree.read(cx).change_count() == 1).await;
    click(&window, cx, "diff-discard-0");
    cx.run_until_parked();
    click(&window, cx, "cancel");
    cx.run_until_parked();
    assert!(std::fs::read_to_string(project.join("README.md")).unwrap().contains("Baris SATU"), "dibatalkan: berkas tidak berubah");
    click(&window, cx, "diff-discard-0");
    cx.run_until_parked();
    click(&window, cx, "ok");
    window.wait_until(cx, "perubahan dibuang", |_, _| std::fs::read_to_string(project.join("README.md")).is_ok_and(|t| !t.contains("Baris SATU"))).await;
    assert_eq!(std::fs::read_to_string(project.join("README.md")).unwrap(), "# Demo\n\nBaris satu\nBaris dua\nBaris tiga\nBaris empat\n", "perubahan yang di-stage tetap ada");
}

#[gpui_kit::test]
async fn stages_selected_lines_and_switches_to_unified(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let project = app.git_project(&[("a.txt", "satu\n")]);
    std::fs::write(project.join("a.txt"), "satu\ndua\ntiga\n").unwrap();
    let (window, workbench, root) = open(&app, &project, cx).await;
    let view = open_diff_tab(&window, &workbench, &root, "a.txt", DiffSource::Worktree, cx).await;

    // Pilih hanya baris "dua" (baris 2 sisi baru) lalu stage pilihan.
    cx.update(|cx| {
        let state = view.read(cx).state().clone();
        state.update(cx, |state, cx| state.set_selected_lines(Some(DiffLineRange::new(oncode_ui_diff::diff_tab::PATCH_PATH, DiffSide::Modified, 2, 2)), cx));
    });
    click(&window, cx, "diff-selected-stage");
    window.wait_until(cx, "satu baris masuk index", |_, _| git(&project, &["show", ":a.txt"]) == "satu\ndua\n").await;
    assert_eq!(changed_lines(&git(&project, &["diff", "-U0"])), ["+tiga"]);

    // Beralih tampilan berdampingan ↔ unified.
    let before = cx.update(|cx| view.read(cx).mode(cx));
    click(&window, cx, "diff-toggle-layout");
    cx.run_until_parked();
    let after = cx.update(|cx| view.read(cx).mode(cx));
    assert_ne!(before, after);
    assert!(matches!(after, DiffMode::Unified | DiffMode::Split));
}
