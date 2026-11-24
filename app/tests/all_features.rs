//! Semua fitur aktif bersamaan dalam satu jendela, seperti di aplikasi sungguhan. Menangkap
//! tabrakan antarfitur (mis. peminjaman ganda entitas) yang tidak terlihat di uji per crate.

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::TestAppContext;
use oncode_ui::editor::{open_file, OpenFile};
use oncode_ui::repos;
use oncode_ui::testing::TestApp;

fn init_all(cx: &mut TestAppContext) {
    cx.update(|cx| {
        oncode_ui_explorer::init(cx);
        oncode_ui_search::init(cx);
        oncode_ui_diff::init(cx);
        oncode_ui_scm::init(cx);
        oncode_ui_graph::init(cx);
        oncode_ui_terminal::init(cx);
        oncode_ui_lsp::init(cx);
        oncode_ui_forge::init(cx);
        oncode_ui_settings::init(cx);
        oncode_ui_markdown::init(cx);
    });
}

#[gpui_kit::test]
async fn every_view_renders_together(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    init_all(cx);
    let project = app.git_project(&[("src/main.rs", "fn main() {}\n"), ("README.md", "# Demo\n")]);
    std::fs::write(project.join("README.md"), "# Demo\n\nUbah.\n").unwrap();
    let window = app.open(Some(&project), cx).await;
    let workbench = window.workbench(cx);
    let root = project.to_string_lossy().into_owned();
    window.wait_until(cx, "repositori terbuka", |_, cx| repos(cx).read(cx).snapshot(&root).is_some()).await;

    let path = project.join("src/main.rs").to_string_lossy().into_owned();
    window.update(cx, |window, cx| workbench.update(cx, |w, cx| open_file(w, OpenFile::new(path), window, cx)));
    cx.run_until_parked();

    for sidebar in ["explorer", "search", "scm", "graph", "pullRequests"] {
        cx.update(|cx| workbench.update(cx, |w, cx| w.show_sidebar(sidebar, cx)));
        window.update(cx, |window, cx| window.render_frame(cx));
        cx.run_until_parked();
        assert_eq!(cx.update(|cx| workbench.read(cx).active_sidebar().map(|s| s.to_string())).as_deref(), Some(sidebar), "sidebar {sidebar} terdaftar");
    }
    for panel in ["terminal", "problems", "git-output"] {
        cx.update(|cx| workbench.update(cx, |w, cx| w.show_panel(panel, cx)));
        window.update(cx, |window, cx| window.render_frame(cx));
        cx.run_until_parked();
        assert_eq!(cx.update(|cx| workbench.read(cx).active_panel().map(|s| s.to_string())).as_deref(), Some(panel), "panel {panel} terdaftar");
    }
    for command in ["settings.open", "git.showGraph", "git.showReflog", "workbench.split", "terminal.toggle", "workbench.commandPalette"] {
        assert!(window.run_command(cx, command), "command {command} berjalan");
        cx.run_until_parked();
        window.update(cx, |window, cx| window.render_frame(cx));
    }
}

#[gpui_kit::test]
async fn welcome_and_restricted_mode_render_with_all_features(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    init_all(cx);
    let window = app.open(None, cx).await;
    window.update(cx, |window, cx| window.render_frame(cx));
    assert!(window.run_command(cx, "settings.open"));
    cx.run_until_parked();

    let project = app.project(&[("a.txt", "a\n")]);
    let window = app.open(Some(&project), cx).await;
    window.update(cx, |window, cx| window.render_frame(cx));
    assert!(!window.run_command(cx, "git.stageAll"), "Git tidak aktif di mode terbatas");
    assert!(window.run_command(cx, "workspace.trust"));
    window.wait_until(cx, "folder dipercaya", |shell, cx| shell.read(cx).workspace().is_some_and(|w| w.trusted)).await;
    window.update(cx, |window, cx| window.render_frame(cx));
}
