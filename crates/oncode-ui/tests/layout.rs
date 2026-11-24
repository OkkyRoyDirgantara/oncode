//! Uji tata letak workbench: sidebar dan activity bar bisa dipindah ke kanan.

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::TestAppContext;
use oncode_core::settings::SidebarPosition;
use oncode_ui::testing::{TestApp, TestWindow};

/// Posisi kiri activity bar dan lebar jendela.
fn activity_bar_x(window: &TestWindow, cx: &mut TestAppContext) -> (f32, f32) {
    window.update(cx, |window, cx| {
        window.render_frame(cx);
        let bar = window.find("activity-bar").bounds();
        (f32::from(bar.origin.x), f32::from(window.viewport_size().width))
    })
}

#[gpui_kit::test]
async fn sidebar_moves_to_the_right_and_back(cx: &mut TestAppContext) {
    let app = TestApp::new(cx);
    let project = app.project(&[("a.txt", "a\n")]);
    let window = app.open(Some(&project), cx).await;
    let workbench = window.workbench(cx);

    let (x, width) = activity_bar_x(&window, cx);
    assert!(x < 1.0, "awalnya di tepi kiri: x={x}");

    assert!(window.run_command(cx, "workbench.toggleSidebarPosition"));
    window.wait_until(cx, "pengaturan terbaca", |_, cx| workbench.read(cx).sidebar_on_right()).await;
    let (x, _) = activity_bar_x(&window, cx);
    assert!((x - (width - 44.0)).abs() < 2.0, "pindah ke tepi kanan: x={x}, lebar={width}");
    assert_eq!(app.core.user_settings().workbench.sidebar_position, SidebarPosition::Right, "tersimpan di pengaturan");

    assert!(window.run_command(cx, "workbench.toggleSidebarPosition"));
    window.wait_until(cx, "kembali ke kiri", |_, cx| !workbench.read(cx).sidebar_on_right()).await;
    assert!(activity_bar_x(&window, cx).0 < 1.0);
}
