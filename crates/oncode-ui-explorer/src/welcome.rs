//! Isi area editor saat tidak ada tab: layar awal (belum ada folder) atau petunjuk pintasan
//! (folder terbuka).

use std::path::PathBuf;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{h_flex, v_flex, ActiveTheme as _, Disableable as _, Icon, Sizable as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    div, px, App, Context, InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _, TestSupportExt as _,
    Window,
};
use oncode_core::api::app::{self, AppInfo};
use oncode_ui::{commands, keys, spawn_core, Env, Lucide, Shell};
use serde_json::Value;

use crate::tree::base_name;

/// Jumlah folder terakhir yang ditampilkan.
const RECENT_LIMIT: usize = 10;

/// Pintasan utama: id command, keterangan, dan pintasan bawaan bila command-nya belum terdaftar.
const SHORTCUTS: [(&str, &str, &str); 4] = [
    ("workbench.commandPalette", "Command palette", "Mod+Shift+P"),
    ("workbench.quickOpen", "Go to file", "Mod+P"),
    ("scm.focus", "Source Control", "Mod+Shift+G"),
    ("terminal.toggle", "Terminal", "Mod+`"),
];

pub struct Welcome {
    env: Env,
    info: Option<AppInfo>,
}

impl Welcome {
    pub fn new(env: Env, cx: &mut Context<Self>) -> Self {
        let mut this = Self { env, info: None };
        if this.env.root.is_none() {
            this.load_info(cx);
        }
        this
    }

    /// Info aplikasi (versi, git, folder terakhir); `None` selama belum dimuat.
    pub fn info(&self) -> Option<&AppInfo> {
        self.info.as_ref()
    }

    fn load_info(&mut self, cx: &mut Context<Self>) {
        let core = self.env.core.clone();
        let task = spawn_core(cx, async move { app::app_info(&core).await });
        cx.spawn(async move |this, cx| {
            if let Ok(info) = task.await {
                let _ = this.update(cx, |this, cx| {
                    this.info = Some(info);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn remove_recent(&mut self, path: String, cx: &mut Context<Self>) {
        // Langsung hilang dari daftar; daftar dari backend menyusul.
        if let Some(info) = &mut self.info {
            info.recent.retain(|recent| *recent != path);
        }
        cx.notify();
        let core = self.env.core.clone();
        let task = spawn_core(cx, async move {
            let _ = app::recent_remove(&core, path).await;
            app::app_info(&core).await
        });
        cx.spawn(async move |this, cx| {
            if let Ok(info) = task.await {
                let _ = this.update(cx, |this, cx| {
                    this.info = Some(info);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// Alasan fitur Git tidak tersedia, bila ada.
    fn git_problem(&self) -> Option<String> {
        let info = self.info.as_ref()?;
        match &info.git {
            None => Some(format!("Git features are unavailable: {}. Install Git 2.40 or newer and restart OnCode.", info.git_error.as_deref().unwrap_or("git was not found").trim_end_matches('.'))),
            Some(git) if !git.supported => Some(format!("Git features are unavailable: git {} is too old. Install Git 2.40 or newer and restart OnCode.", git.version)),
            Some(_) => None,
        }
    }

    fn render_shortcuts(&self, window: &Window, cx: &App) -> impl IntoElement {
        let theme = cx.theme();
        v_flex().gap_1p5().text_sm().text_color(theme.muted_foreground).children(SHORTCUTS.iter().filter_map(|(id, label, fallback)| {
            // Pintasan yang berlaku (termasuk penimpaan pengguna); command yang belum terdaftar memakai bawaannya.
            let binding = match commands::get(id, window, cx) {
                Some(command) => commands::keybindings_for(&command, cx).first().cloned(),
                None => Some(keys::canonical_keybinding(fallback)),
            }?;
            Some(
                h_flex()
                    .gap_3()
                    .child(
                        h_flex()
                            .flex_none()
                            .min_w(px(104.))
                            .child(div().px_1p5().rounded(theme.radius).border_1().border_color(theme.border).bg(theme.muted).text_xs().child(keys::format_keybinding(&binding))),
                    )
                    .child(*label),
            )
        }))
    }

    fn render_welcome(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let git_ready = self.info.as_ref().is_some_and(|info| info.git.as_ref().is_some_and(|git| git.supported));
        let can_clone = git_ready && commands::get("git.clone", window, cx).is_some();
        let recent: Vec<String> = self.info.as_ref().map(|info| info.recent.iter().take(RECENT_LIMIT).cloned().collect()).unwrap_or_default();
        let version = self.info.as_ref().map(|info| match &info.git {
            Some(git) => format!("Version {} · git {}", info.version, git.version),
            None => format!("Version {}", info.version),
        });
        let heading = |text: &'static str| div().mt_5().mb_2().text_xs().font_weight(gpui_kit::FontWeight::SEMIBOLD).text_color(theme.muted_foreground).child(text.to_uppercase());

        v_flex()
            .w_full()
            .max_w(px(560.))
            .px_6()
            .py_10()
            .child(div().id("welcome-title").test_support().text_3xl().font_weight(gpui_kit::FontWeight::BOLD).child("OnCode"))
            .child(div().mt_1().text_color(theme.muted_foreground).child("A lightweight code editor with a complete Git client."))
            .child(
                h_flex()
                    .mt_5()
                    .gap_2()
                    .flex_wrap()
                    .child(Button::new("welcome-open-folder").primary().icon(Icon::new(Lucide::FolderOpen)).label("Open Folder…").on_click(|_, window, cx| {
                        oncode_ui::execute("file.openFolder", Value::Null, window, cx);
                    }))
                    .child(Button::new("welcome-clone").outline().icon(Icon::new(Lucide::GitBranch)).label("Clone Repository…").disabled(!can_clone).on_click(|_, window, cx| {
                        oncode_ui::execute("git.clone", Value::Null, window, cx);
                    }))
                    .child(Button::new("welcome-settings").outline().icon(Icon::new(Lucide::Settings)).label("Settings").on_click(|_, window, cx| {
                        oncode_ui::execute("settings.open", Value::Null, window, cx);
                    })),
            )
            .when_some(self.git_problem(), |el, problem| {
                el.child(
                    h_flex()
                        .id("welcome-git-problem")
                        .test_support()
                        .mt_4()
                        .p_3()
                        .gap_3()
                        .items_start()
                        .rounded(theme.radius)
                        .border_1()
                        .border_color(theme.border)
                        .bg(theme.warning.opacity(0.12))
                        .text_sm()
                        .child(Icon::new(Lucide::TriangleAlert).small().flex_none().text_color(theme.warning))
                        .child(div().flex_1().min_w_0().child(SharedString::from(problem)))
                        .child(Button::new("welcome-git-settings").label("Open Settings").outline().small().flex_none().on_click(|_, window, cx| {
                            oncode_ui::execute("settings.open", Value::Null, window, cx);
                        })),
                )
            })
            .when(!recent.is_empty(), |el| {
                el.child(heading("Recent")).children(recent.into_iter().map(|path| {
                    let (open_path, remove_path) = (path.clone(), path.clone());
                    h_flex()
                        .id(SharedString::from(format!("welcome-recent-{path}")))
                        .test_support()
                        .w_full()
                        .gap_3()
                        .px_2()
                        .py_1()
                        .rounded(theme.radius)
                        .text_sm()
                        .hover(|style| style.bg(theme.list_hover))
                        .on_click(move |_, window, cx| {
                            if let Some(shell) = Shell::for_window(window, cx) {
                                let folder = PathBuf::from(&open_path);
                                shell.update(cx, |shell, cx| shell.open_folder(folder, window, cx));
                            }
                        })
                        .child(div().flex_none().child(SharedString::from(base_name(&path).to_string())))
                        .child(div().flex_1().min_w_0().truncate().text_xs().text_color(theme.muted_foreground).child(SharedString::from(path.clone())))
                        .child(Button::new(SharedString::from(format!("welcome-recent-remove-{path}"))).ghost().xsmall().flex_none().icon(Icon::new(Lucide::X)).tooltip("Remove from Recent").on_click(
                            cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.remove_recent(remove_path.clone(), cx);
                            }),
                        ))
                }))
            })
            .child(heading("Shortcuts"))
            .child(self.render_shortcuts(window, cx))
            .when_some(version, |el, version| el.child(div().mt_6().text_xs().text_color(theme.muted_foreground.opacity(0.7)).child(SharedString::from(version))))
    }
}

impl Render for Welcome {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.env.root.is_some() {
            // Folder terbuka tanpa tab: cukup pengingat pintasan utama.
            return v_flex()
                .id("editor-hint")
                .test_support()
                .size_full()
                .items_center()
                .justify_center()
                .gap_4()
                .child(div().text_color(cx.theme().muted_foreground).child("No file is open"))
                .child(self.render_shortcuts(window, cx))
                .into_any_element();
        }
        div()
            .id("welcome")
            .test_support()
            .size_full()
            .overflow_y_scroll()
            .child(h_flex().w_full().justify_center().child(self.render_welcome(window, cx)))
            .into_any_element()
    }
}
