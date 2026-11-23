//! Quick pick: kotak pencarian dengan daftar pilihan, dipakai command palette, quick open,
//! pemilih branch, dan sejenisnya.

use std::rc::Rc;

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{h_flex, v_flex, ActiveTheme as _, WindowExt as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    div, px, App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, KeyDownEvent, ParentElement as _, Render, ScrollHandle, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Task, Window,
};

#[derive(Clone, Debug, PartialEq)]
pub struct PickItem {
    pub label: SharedString,
    /// Teks redup di samping label (mis. folder berkas).
    pub description: Option<SharedString>,
    /// Teks di sisi kanan (mis. pintasan keyboard).
    pub hint: Option<SharedString>,
    /// Nilai yang dikembalikan saat dipilih.
    pub value: String,
}

impl PickItem {
    pub fn new(label: impl Into<SharedString>, value: impl Into<String>) -> Self {
        Self { label: label.into(), description: None, hint: None, value: value.into() }
    }

    pub fn description(mut self, description: impl Into<SharedString>) -> Self {
        self.description = Some(description.into());
        self
    }

    pub fn hint(mut self, hint: impl Into<SharedString>) -> Self {
        self.hint = Some(hint.into());
        self
    }
}

/// Sumber pilihan: daftar tetap yang disaring di tempat, atau pencarian asinkron.
#[derive(Clone)]
pub enum PickSource {
    Static(Rc<Vec<PickItem>>),
    Async(Rc<dyn Fn(String, &mut App) -> Task<Vec<PickItem>>>),
}

pub struct QuickPickOptions {
    pub placeholder: SharedString,
    pub empty_text: SharedString,
    pub source: PickSource,
    /// Izinkan memilih teks yang diketik apa adanya (mis. nama branch baru); labelnya dari fungsi ini.
    pub allow_custom: Option<Rc<dyn Fn(&str) -> Option<PickItem>>>,
}

impl QuickPickOptions {
    pub fn new(placeholder: impl Into<SharedString>, items: Vec<PickItem>) -> Self {
        Self { placeholder: placeholder.into(), empty_text: "No matches".into(), source: PickSource::Static(Rc::new(items)), allow_custom: None }
    }

    pub fn with_search(placeholder: impl Into<SharedString>, search: impl Fn(String, &mut App) -> Task<Vec<PickItem>> + 'static) -> Self {
        Self { placeholder: placeholder.into(), empty_text: "No matches".into(), source: PickSource::Async(Rc::new(search)), allow_custom: None }
    }

    pub fn empty_text(mut self, text: impl Into<SharedString>) -> Self {
        self.empty_text = text.into();
        self
    }

    pub fn allow_custom(mut self, make: impl Fn(&str) -> Option<PickItem> + 'static) -> Self {
        self.allow_custom = Some(Rc::new(make));
        self
    }
}

/// Skor kecocokan subsekuens tanpa membedakan huruf besar; `None` bila tidak cocok.
/// Kecocokan di awal kata dan yang berurutan dinilai lebih tinggi.
pub fn fuzzy_score(query: &str, text: &str) -> Option<i32> {
    if query.is_empty() {
        return Some(0);
    }
    let text_chars: Vec<char> = text.chars().collect();
    let lower: Vec<char> = text.to_lowercase().chars().collect();
    let mut score = 0;
    let mut position = 0usize;
    let mut previous: Option<usize> = None;
    for q in query.to_lowercase().chars() {
        if q.is_whitespace() {
            continue;
        }
        let found = (position..lower.len()).find(|&i| lower[i] == q)?;
        score += 1;
        if previous.is_some_and(|p| p + 1 == found) {
            score += 4;
        }
        let word_start = found == 0 || !text_chars[found - 1].is_alphanumeric() || (text_chars[found].is_uppercase() && text_chars[found - 1].is_lowercase());
        if word_start {
            score += 3;
        }
        previous = Some(found);
        position = found + 1;
    }
    Some(score * 16 - text_chars.len().min(255) as i32 / 8)
}

/// Menyaring dan mengurutkan pilihan untuk sebuah kueri.
pub fn filter_items(items: &[PickItem], query: &str, limit: usize) -> Vec<PickItem> {
    let query = query.trim();
    if query.is_empty() {
        return items.iter().take(limit).cloned().collect();
    }
    let mut scored: Vec<(i32, usize)> = items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            let label = fuzzy_score(query, &item.label);
            let description = item.description.as_ref().and_then(|d| fuzzy_score(query, d)).map(|s| s - 8);
            label.max(description).map(|score| (score, index))
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored.into_iter().take(limit).map(|(_, index)| items[index].clone()).collect()
}

const LIMIT: usize = 80;

pub struct QuickPick {
    input: Entity<InputState>,
    options: QuickPickOptions,
    items: Vec<PickItem>,
    selected: usize,
    loading: bool,
    scroll: ScrollHandle,
    on_pick: Rc<dyn Fn(Option<PickItem>, &mut Window, &mut App)>,
    done: bool,
    revision: usize,
    _search: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl QuickPick {
    fn new(options: QuickPickOptions, on_pick: Rc<dyn Fn(Option<PickItem>, &mut Window, &mut App)>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(options.placeholder.clone()));
        let subscription = cx.subscribe_in(&input, window, |this: &mut Self, _, event: &InputEvent, window, cx| match event {
            InputEvent::Change => this.refresh(cx),
            InputEvent::PressEnter { .. } => this.confirm(window, cx),
            _ => {}
        });
        let mut this = Self {
            input,
            options,
            items: Vec::new(),
            selected: 0,
            loading: false,
            scroll: ScrollHandle::new(),
            on_pick,
            done: false,
            revision: 0,
            _search: Task::ready(()),
            _subscriptions: vec![subscription],
        };
        this.refresh(cx);
        this
    }

    fn with_custom(&self, mut items: Vec<PickItem>, query: &str) -> Vec<PickItem> {
        if let Some(make) = &self.options.allow_custom {
            let query = query.trim();
            if !query.is_empty() && !items.iter().any(|i| i.value == query) {
                if let Some(custom) = make(query) {
                    items.push(custom);
                }
            }
        }
        items
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let query = self.input.read(cx).value().to_string();
        self.revision += 1;
        let revision = self.revision;
        match self.options.source.clone() {
            PickSource::Static(all) => {
                self.items = self.with_custom(filter_items(&all, &query, LIMIT), &query);
                self.selected = 0;
                self.loading = false;
            }
            PickSource::Async(search) => {
                self.loading = true;
                let task = search(query.clone(), cx);
                self._search = cx.spawn(async move |this, cx| {
                    let items = task.await;
                    let _ = this.update(cx, |this, cx| {
                        if this.revision != revision {
                            return;
                        }
                        this.items = this.with_custom(items, &query);
                        this.selected = 0;
                        this.loading = false;
                        cx.notify();
                    });
                });
            }
        }
        cx.notify();
    }

    fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        if self.items.is_empty() {
            return;
        }
        let len = self.items.len() as isize;
        self.selected = (self.selected as isize + delta).rem_euclid(len) as usize;
        self.scroll.scroll_to_item(self.selected);
        cx.notify();
    }

    fn finish(&mut self, item: Option<PickItem>, window: &mut Window, cx: &mut Context<Self>) {
        if self.done {
            return;
        }
        self.done = true;
        window.close_dialog(cx);
        let on_pick = self.on_pick.clone();
        // Dijalankan setelah dialog tertutup, agar penangan boleh membuka dialog lain.
        window.defer(cx, move |window, cx| on_pick(item, window, cx));
    }

    fn confirm(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.loading {
            return;
        }
        let item = self.items.get(self.selected).cloned();
        if item.is_some() {
            self.finish(item, window, cx);
        }
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            "down" => self.move_selection(1, cx),
            "up" => self.move_selection(-1, cx),
            "pagedown" => self.move_selection(8, cx),
            "pageup" => self.move_selection(-8, cx),
            "escape" => self.finish(None, window, cx),
            _ => return,
        }
        cx.stop_propagation();
    }
}

impl Render for QuickPick {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        v_flex()
            .id("quick-pick")
            .w_full()
            .gap_2()
            .capture_key_down(cx.listener(Self::on_key_down))
            .child(Input::new(&self.input).id("quick-pick-input"))
            .child(
                v_flex()
                    .id("quick-pick-list")
                    .w_full()
                    .max_h(px(360.))
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .when(self.items.is_empty(), |el| {
                        el.child(div().px_2().py_3().text_sm().text_color(theme.muted_foreground).child(if self.loading { SharedString::from("Searching…") } else { self.options.empty_text.clone() }))
                    })
                    .children(self.items.iter().enumerate().map(|(index, item)| {
                        let selected = index == self.selected;
                        let picked = item.clone();
                        h_flex()
                            .id(("quick-pick-item", index))
                            .w_full()
                            .flex_none()
                            .px_2()
                            .py_1()
                            .gap_2()
                            .rounded(theme.radius)
                            .text_sm()
                            .when(selected, |el| el.bg(theme.list_active))
                            .when(!selected, |el| el.hover(|s| s.bg(theme.list_hover)))
                            .on_click(cx.listener(move |this, _, window, cx| this.finish(Some(picked.clone()), window, cx)))
                            .child(div().flex_none().child(item.label.clone()))
                            .when_some(item.description.clone(), |el, description| {
                                el.child(div().flex_1().min_w_0().truncate().text_color(theme.muted_foreground).text_xs().child(description))
                            })
                            .when(item.description.is_none(), |el| el.child(div().flex_1()))
                            .when_some(item.hint.clone(), |el, hint| el.child(div().flex_none().text_xs().text_color(theme.muted_foreground).child(hint)))
                    })),
            )
    }
}

/// Membuka quick pick. `on_pick` menerima pilihan, atau `None` bila dibatalkan dengan Escape.
pub fn quick_pick(options: QuickPickOptions, on_pick: impl Fn(Option<PickItem>, &mut Window, &mut App) + 'static, window: &mut Window, cx: &mut App) {
    let on_pick: Rc<dyn Fn(Option<PickItem>, &mut Window, &mut App)> = Rc::new(on_pick);
    let picker = cx.new(|cx| QuickPick::new(options, on_pick, window, cx));
    let focus = picker.read(cx).input.clone();
    window.open_dialog(cx, move |dialog, _, _| dialog.close_button(false).margin_top(px(80.)).w(px(620.)).child(picker.clone()));
    window.defer(cx, move |window, cx| focus.update(cx, |state, cx| state.focus(window, cx)));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items(labels: &[&str]) -> Vec<PickItem> {
        labels.iter().map(|l| PickItem::new(*l, *l)).collect()
    }

    fn labels(items: &[PickItem]) -> Vec<String> {
        items.iter().map(|i| i.label.to_string()).collect()
    }

    #[test]
    fn empty_query_keeps_order_and_limit() {
        let all = items(&["b", "a", "c"]);
        assert_eq!(labels(&filter_items(&all, "", 2)), ["b", "a"]);
    }

    #[test]
    fn subsequence_must_match_in_order() {
        assert!(fuzzy_score("gco", "Git: Commit").is_some());
        assert!(fuzzy_score("xyz", "Git: Commit").is_none());
        assert!(fuzzy_score("tig", "Git").is_none());
    }

    #[test]
    fn word_starts_and_runs_rank_higher() {
        let all = items(&["Toggle Sidebar", "Git: Stage All", "View: Show Search"]);
        assert_eq!(labels(&filter_items(&all, "sta", 10))[0], "Git: Stage All");
        let all = items(&["src/commands/git.rs", "git.rs", "crates/oncode-git/src/graph.rs"]);
        assert_eq!(labels(&filter_items(&all, "git.rs", 10))[0], "git.rs");
    }

    #[test]
    fn description_matches_but_ranks_below_label() {
        let all = vec![PickItem::new("main.rs", "a").description("src/app"), PickItem::new("app.rs", "b").description("src")];
        assert_eq!(labels(&filter_items(&all, "app", 10)), ["app.rs", "main.rs"]);
    }
}
