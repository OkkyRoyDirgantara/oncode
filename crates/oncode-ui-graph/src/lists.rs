//! Tab daftar sederhana: reflog repositori dan isi sebuah stash.

use gpui_kit::component::menu::{ContextMenuExt as _, PopupMenuItem};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{h_flex, v_flex, ActiveTheme as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    div, px, uniform_list, AnyElement, App, Context, FocusHandle, Focusable, InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Task, Window,
};
use oncode_core::api::git;
use oncode_git::{CommitFile, DiffSource, ReflogEntry};
use oncode_ui::spawn_core;
use oncode_ui_diff::{open_diff, DiffRequest};

use crate::actions::{run_action, Ctx};
use crate::format::{base_name, relative_date, short_oid};
use crate::menus::{reflog_menu, GitAction, MenuEntry};
use crate::store::RevealTarget;

enum Load<T> {
    Loading,
    Ready(Vec<T>),
    Failed(String),
}

pub struct ReflogTab {
    ctx: Ctx,
    entries: Load<ReflogEntry>,
    focus_handle: FocusHandle,
    _load: Task<()>,
}

impl Focusable for ReflogTab {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl ReflogTab {
    pub fn new(ctx: Ctx, cx: &mut Context<Self>) -> Self {
        let (core, root) = (ctx.core(cx), ctx.root.clone());
        let task = spawn_core(cx, async move { git::git_reflog(&core, root, 500).await });
        let load = cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.entries = match result {
                    Ok(entries) => Load::Ready(entries),
                    Err(error) => Load::Failed(error.message),
                };
                cx.notify();
            });
        });
        Self { ctx, entries: Load::Loading, focus_handle: cx.focus_handle(), _load: load }
    }

    fn render_rows(&mut self, range: std::ops::Range<usize>, _: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Load::Ready(entries) = &self.entries else { return Vec::new() };
        let theme = cx.theme();
        range
            .filter_map(|index| {
                let entry = entries.get(index)?.clone();
                let ctx = self.ctx.clone();
                let menu_ctx = self.ctx.clone();
                let oid = entry.oid.clone();
                let label = entry.selector.clone();
                Some(
                    h_flex()
                        .id(SharedString::from(format!("reflog-{index}")))
                        .w_full()
                        .h(px(26.))
                        .px_3()
                        .gap_3()
                        .text_sm()
                        .hover(|s| s.bg(theme.list_hover))
                        .on_click(move |_, window, cx| run_action(&ctx, GitAction::ShowInGraph { oid: oid.clone(), label: label.clone() }, window, cx))
                        .context_menu({
                            let oid = entry.oid.clone();
                            move |menu, _, _| menu_items(&menu_ctx, reflog_menu(&oid), menu)
                        })
                        .child(div().flex_none().w(px(90.)).text_color(theme.muted_foreground).child(entry.selector.clone()))
                        .child(div().flex_none().w(px(64.)).font_family(theme.mono_font_family.clone()).text_xs().child(short_oid(&entry.oid).to_string()))
                        .child(div().flex_1().min_w_0().truncate().child(entry.message.clone()))
                        .child(div().flex_none().text_color(theme.muted_foreground).child(relative_date(entry.time)))
                        .into_any_element(),
                )
            })
            .collect()
    }
}

pub(crate) fn menu_items(ctx: &Ctx, entries: Vec<MenuEntry>, mut menu: gpui_kit::component::menu::PopupMenu) -> gpui_kit::component::menu::PopupMenu {
    for entry in entries {
        menu = match entry {
            MenuEntry::Separator => menu.separator(),
            MenuEntry::Item { label, disabled, action, .. } => {
                let ctx = ctx.clone();
                menu.item(PopupMenuItem::new(label).disabled(disabled).on_click(move |_, window, cx| run_action(&ctx, action.clone(), window, cx)))
            }
        };
    }
    menu
}

impl Render for ReflogTab {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let count = match &self.entries {
            Load::Ready(entries) => entries.len(),
            _ => 0,
        };
        v_flex()
            .id("reflog-tab")
            .size_full()
            .track_focus(&self.focus_handle)
            .bg(cx.theme().background)
            .child(div().px_3().py_2().text_sm().text_color(cx.theme().muted_foreground).child("Every position HEAD has been at. Use it to recover commits after a reset or rebase."))
            .map(|el| match &self.entries {
                Load::Loading => el.child(div().p_3().child(Spinner::new())),
                Load::Failed(message) => el.child(div().p_3().text_color(cx.theme().danger).child(message.clone())),
                Load::Ready(_) => el.child(div().flex_1().min_h_0().child(uniform_list("reflog-list", count, cx.processor(Self::render_rows)).size_full())),
            })
    }
}

/// Isi sebuah stash: berkas yang berubah; klik membuka diff-nya.
pub struct StashTab {
    ctx: Ctx,
    name: String,
    files: Load<CommitFile>,
    focus_handle: FocusHandle,
    _load: Task<()>,
}

impl Focusable for StashTab {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl StashTab {
    pub fn new(ctx: Ctx, name: String, cx: &mut Context<Self>) -> Self {
        let (core, root, stash) = (ctx.core(cx), ctx.root.clone(), name.clone());
        let task = spawn_core(cx, async move { git::git_stash_files(&core, root, stash).await });
        let load = cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.files = match result {
                    Ok(files) => Load::Ready(files),
                    Err(error) => Load::Failed(error.message),
                };
                cx.notify();
            });
        });
        Self { ctx, name, files: Load::Loading, focus_handle: cx.focus_handle(), _load: load }
    }
}

impl Render for StashTab {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let store = self.ctx.store.clone();
        v_flex()
            .id("stash-tab")
            .size_full()
            .p_3()
            .gap_1()
            .track_focus(&self.focus_handle)
            .bg(theme.background)
            .child(div().pb_2().font_weight(gpui_kit::FontWeight::SEMIBOLD).child(self.name.clone()))
            .map(|el| match &self.files {
                Load::Loading => el.child(Spinner::new()),
                Load::Failed(message) => el.child(div().text_color(theme.danger).child(message.clone())),
                Load::Ready(files) => el.children(files.iter().map(|file| {
                    let request = DiffRequest {
                        root: self.ctx.root.clone(),
                        path: file.path.clone(),
                        orig_path: file.orig_path.clone(),
                        source: DiffSource::Stash { name: self.name.clone() },
                        preview: true,
                    };
                    let store = store.clone();
                    h_flex()
                        .id(SharedString::from(format!("stash-file-{}", file.path)))
                        .gap_2()
                        .px_2()
                        .py_0p5()
                        .text_sm()
                        .rounded(theme.radius)
                        .hover(|s| s.bg(theme.list_hover))
                        .on_click(move |_, window, cx| {
                            let workbench = store.read(cx).env().workbench.clone();
                            let request = request.clone();
                            let _ = workbench.update(cx, |workbench, cx| open_diff(workbench, request, window, cx));
                        })
                        .child(div().child(base_name(&file.path).to_string()))
                        .child(div().text_xs().text_color(theme.muted_foreground).child(file.path.clone()))
                })),
            })
    }
}

/// Permintaan menampilkan commit di Graph untuk dipakai `run_action`.
pub fn reveal_target(oid: String, label: String) -> RevealTarget {
    RevealTarget { oid, label, hunt: true }
}
