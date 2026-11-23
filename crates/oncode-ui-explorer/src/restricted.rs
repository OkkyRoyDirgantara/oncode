//! Mode terbatas: folder yang belum dipercaya dibuka tanpa Git, language server, dan pengaturan
//! proyek. Pemberitahuannya tampil di kepala Explorer dan sebagai item status bar.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{h_flex, v_flex, ActiveTheme as _, Icon, Sizable as _};
use gpui_kit::{
    div, App, Context, InteractiveElement as _, IntoElement, ParentElement as _, Render, StatefulInteractiveElement as _, Styled as _, TestSupportExt as _, Window,
};
use oncode_ui::overlay::{self, Confirm};
use oncode_ui::Lucide;
use serde_json::Value;

/// Penjelasan mode terbatas; dipakai banner dan dialog konfirmasi.
pub const RESTRICTED_MESSAGE: &str = "Git, language servers and project settings are disabled until you trust the authors of the files in this folder.";

fn trust(window: &mut Window, cx: &mut App) {
    oncode_ui::execute("workspace.trust", Value::Null, window, cx);
}

/// Banner di kepala Explorer.
pub fn banner(cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    v_flex()
        .id("restricted-banner")
        .test_support()
        .w_full()
        .flex_none()
        .px_3()
        .py_2()
        .gap_2()
        .text_xs()
        .bg(theme.warning.opacity(0.12))
        .border_b_1()
        .border_color(theme.border)
        .child(
            h_flex()
                .gap_1p5()
                .items_start()
                .child(Icon::new(Lucide::ShieldAlert).small().flex_none().text_color(theme.warning))
                .child(div().flex_1().min_w_0().child(div().font_weight(gpui_kit::FontWeight::SEMIBOLD).child("Restricted mode")).child(RESTRICTED_MESSAGE)),
        )
        .child(Button::new("restricted-trust").label("Trust Folder").primary().xsmall().on_click(|_, window, cx| trust(window, cx)))
}

/// Item status bar "Restricted Mode"; mengekliknya menawarkan untuk memercayai folder.
pub struct RestrictedStatus;

impl Render for RestrictedStatus {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        h_flex()
            .id("status-restricted")
            .test_support()
            .h_full()
            .px_1p5()
            .gap_1()
            .rounded(theme.radius)
            .text_color(theme.warning)
            .hover(|style| style.bg(theme.warning.opacity(0.15)))
            .tooltip(|window, cx| gpui_kit::component::tooltip::Tooltip::new(RESTRICTED_MESSAGE).build(window, cx))
            .on_click(|_, window, cx| {
                let confirmed = overlay::confirm(Confirm::new("Trust this folder?", format!("This folder is open in restricted mode. {RESTRICTED_MESSAGE}"), "Trust Folder"), window, cx);
                window
                    .spawn(cx, async move |cx| {
                        if confirmed.await {
                            let _ = cx.update(|window, cx| trust(window, cx));
                        }
                    })
                    .detach();
            })
            .child(Icon::new(Lucide::ShieldAlert).xsmall())
            .child("Restricted Mode")
    }
}
