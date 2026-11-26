//! Interactive rebase dialog: reorder commits, pick an action per commit, then run it.

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::{h_flex, v_flex, ActiveTheme as _, Disableable as _, Icon, Sizable as _, WindowExt as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{div, px, App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _, Window};
use oncode_core::api::git;
use oncode_git::{RebaseAction, RebaseTodoItem};
use oncode_ui::{overlay, spawn_core, Lucide};

use crate::actions::{handle_git_error, Ctx};
use crate::format::short_oid;
use crate::rebase_plan::{action_hint, move_item, set_action, validate_plan, REBASE_ACTIONS};

pub struct RebaseDialog {
    ctx: Ctx,
    base: Option<String>,
    /// From the oldest (top) to the newest.
    items: Vec<RebaseTodoItem>,
    published: u32,
    running: bool,
}

fn action_label(action: RebaseAction) -> &'static str {
    match action {
        RebaseAction::Pick => "pick",
        RebaseAction::Reword => "reword",
        RebaseAction::Edit => "edit",
        RebaseAction::Squash => "squash",
        RebaseAction::Fixup => "fixup",
        RebaseAction::Drop => "drop",
    }
}

impl RebaseDialog {
    pub fn items(&self) -> &[RebaseTodoItem] {
        &self.items
    }

    fn set(&mut self, index: usize, action: RebaseAction, cx: &mut Context<Self>) {
        set_action(&mut self.items, index, action);
        cx.notify();
    }

    fn shift(&mut self, index: usize, delta: isize, cx: &mut Context<Self>) {
        let target = index as isize + delta;
        if target < 0 || target as usize >= self.items.len() {
            return;
        }
        move_item(&mut self.items, index, target as usize);
        cx.notify();
    }

    fn start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.running || validate_plan(&self.items).is_some() {
            return;
        }
        self.running = true;
        cx.notify();
        let (ctx, base, items) = (self.ctx.clone(), self.base.clone(), self.items.clone());
        let core = ctx.core(cx);
        let root = ctx.root.clone();
        let task = spawn_core(cx, async move { git::git_rebase_interactive(&core, root, base, items).await });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.running = false;
                window.close_dialog(cx);
                match result {
                    Ok(()) => overlay::toast_success("Rebase finished.", window, cx),
                    Err(error) => handle_git_error(&ctx, &error, "Rebase", None, window, cx),
                }
            });
        })
        .detach();
    }
}

impl Render for RebaseDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let problem = validate_plan(&self.items);
        let count = self.items.len();
        v_flex()
            .id("rebase-dialog")
            .w_full()
            .gap_2()
            .text_sm()
            .child(div().text_color(theme.muted_foreground).child("Oldest commit at the top. Commits are applied from top to bottom."))
            .when(self.published > 0, |el| {
                el.child(div().text_color(theme.warning).child(format!(
                    "{} of these commits are already on the remote; you will need to force push after rewriting them.",
                    self.published
                )))
            })
            .child(v_flex().id("rebase-list").max_h(px(380.)).overflow_y_scroll().gap_0p5().children(self.items.iter().enumerate().map(|(index, item)| {
                let action = item.action;
                let entity = cx.entity();
                let dropped = action == RebaseAction::Drop;
                h_flex()
                    .id(SharedString::from(format!("rebase-item-{index}")))
                    .w_full()
                    .gap_2()
                    .px_1()
                    .py_0p5()
                    .rounded(theme.radius)
                    .hover(|s| s.bg(theme.list_hover))
                    .child(
                        Button::new(SharedString::from(format!("rebase-action-{index}")))
                            .xsmall()
                            .outline()
                            .w(px(84.))
                            .label(action_label(action))
                            .tooltip(action_hint(action))
                            .dropdown_menu(move |menu, _, _| {
                                let mut menu = menu;
                                for choice in REBASE_ACTIONS {
                                    let entity = entity.clone();
                                    menu = menu.item(PopupMenuItem::new(action_label(choice)).checked(choice == action).on_click(move |_, _, cx| {
                                        entity.update(cx, |this, cx| this.set(index, choice, cx));
                                    }));
                                }
                                menu
                            }),
                    )
                    .child(div().flex_none().font_family(theme.mono_font_family.clone()).text_xs().text_color(theme.muted_foreground).child(short_oid(&item.oid).to_string()))
                    .child(div().flex_1().min_w_0().truncate().when(dropped, |el| el.line_through().text_color(theme.muted_foreground)).child(item.subject.clone()))
                    .child(
                        Button::new(SharedString::from(format!("rebase-up-{index}")))
                            .ghost()
                            .xsmall()
                            .icon(Icon::new(Lucide::ArrowUp))
                            .tooltip("Move up")
                            .disabled(index == 0)
                            .on_click(cx.listener(move |this, _, _, cx| this.shift(index, -1, cx))),
                    )
                    .child(
                        Button::new(SharedString::from(format!("rebase-down-{index}")))
                            .ghost()
                            .xsmall()
                            .icon(Icon::new(Lucide::ArrowDown))
                            .tooltip("Move down")
                            .disabled(index + 1 == count)
                            .on_click(cx.listener(move |this, _, _, cx| this.shift(index, 1, cx))),
                    )
            })))
            .when_some(problem.clone(), |el, problem| el.child(div().id("rebase-problem").text_color(theme.danger).child(problem)))
            .child(
                h_flex()
                    .justify_end()
                    .gap_2()
                    .pt_2()
                    .child(Button::new("rebase-cancel").label("Cancel").outline().small().on_click(|_, window, cx| window.close_dialog(cx)))
                    .child(
                        Button::new("rebase-start")
                            .label("Start Rebase")
                            .primary()
                            .small()
                            .loading(self.running)
                            .disabled(problem.is_some() || self.running)
                            .on_click(cx.listener(|this, _, window, cx| this.start(window, cx))),
                    ),
            )
    }
}

/// Loads the rebase plan from `base` (or from the first commit) and then opens its dialog.
pub fn open(ctx: Ctx, base: Option<String>, window: &mut Window, cx: &mut App) {
    let core = ctx.core(cx);
    let (root, plan_base) = (ctx.root.clone(), base.clone());
    let task = spawn_core(cx, async move { git::git_rebase_plan(&core, root, plan_base).await });
    window
        .spawn(cx, async move |cx| {
            let result = task.await;
            let _ = cx.update(|window, cx| match result {
                Ok(plan) if plan.items.is_empty() => overlay::toast_info("There are no commits to rebase.", window, cx),
                Ok(plan) => {
                    let dialog: Entity<RebaseDialog> = cx.new(|_| RebaseDialog { ctx, base, items: plan.items, published: plan.published, running: false });
                    window.open_dialog(cx, move |d, _, _| d.title("Interactive Rebase").w(px(720.)).child(dialog.clone()));
                }
                Err(error) => handle_git_error(&ctx, &error, "Interactive rebase", None, window, cx),
            });
        })
        .detach();
}
