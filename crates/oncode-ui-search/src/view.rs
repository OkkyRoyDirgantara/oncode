//! The Search sidebar view: query and replacement inputs, how the search runs (typing delay,
//! cancellation, streamed results), and the virtualized per-file result tree.

use std::ops::Range;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{h_flex, v_flex, ActiveTheme as _, Disableable as _, Icon, Selectable as _, Sizable as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    div, px, uniform_list, AnyElement, App, AppContext as _, Context, Entity, FocusHandle, HighlightStyle, InteractiveElement as _, IntoElement, KeyDownEvent, ParentElement as _,
    Pixels, Render, ScrollStrategy, SharedString, StatefulInteractiveElement as _, StrikethroughStyle, Styled as _, StyledText, Subscription, Task, UniformListScrollHandle, Window,
};
use oncode_core::api::fs::SearchEvent;
use oncode_core::api::{app, fs};
use oncode_ui::editor::{open_file, OpenFile};
use oncode_ui::overlay::{self, Confirm};
use oncode_ui::{spawn_core, Env, Lucide};
use oncode_workspace::{FileMatches, LineMatch};

use crate::matcher::{LineMatcher, MatchKey, MatchOptions};
use crate::model::{
    build_query, confirmation_text, counts_of, match_key, replacement_preview, report, reveal_position, row_text, Inputs, Results, Row, RowKey, SearchStatus, FLUSH_MS,
    SEARCH_DEBOUNCE_MS,
};
use crate::replace::{self, absolute_path, Picked, Target};

const ROW_HEIGHT: Pixels = px(24.);

/// Operation ids must be unique across the whole app (used to cancel a search in the backend).
static NEXT_OPERATION: AtomicU64 = AtomicU64::new(1);

/// Search options that can be turned on and off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchOption {
    CaseSensitive,
    WholeWord,
    Regex,
}

/// The search currently (or most recently) running.
struct Run {
    seq: u64,
    operation_id: String,
    finished: bool,
}

/// The scope of a replacement.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ReplaceScope {
    All,
    File(String),
    Match(String, MatchKey),
}

pub struct SearchView {
    env: Env,
    query: Entity<InputState>,
    replacement: Entity<InputState>,
    include: Entity<InputState>,
    exclude: Entity<InputState>,
    is_regex: bool,
    case_sensitive: bool,
    whole_word: bool,
    show_replace: bool,
    show_filters: bool,
    results: Results,
    /// `results` flattened; recomputed every time the results change.
    rows: Vec<Row>,
    /// The replacement preview in regex mode needs a matcher to fill in `$1` etc.
    preview_matcher: Option<LineMatcher>,
    replacing: bool,
    run: Option<Run>,
    run_seq: u64,
    debounce: Task<()>,
    pump: Task<()>,
    scroll: UniformListScrollHandle,
    results_focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl SearchView {
    pub fn new(env: Env, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let query = cx.new(|cx| InputState::new(window, cx).placeholder("Search"));
        let replacement = cx.new(|cx| InputState::new(window, cx).placeholder("Replace"));
        let include = cx.new(|cx| InputState::new(window, cx).placeholder("e.g. src, *.ts"));
        let exclude = cx.new(|cx| InputState::new(window, cx).placeholder("e.g. dist, *.min.js"));

        let searches = |this: &mut Self, _: &Entity<InputState>, event: &InputEvent, window: &mut Window, cx: &mut Context<Self>| match event {
            InputEvent::Change => this.schedule_search(window, cx),
            InputEvent::PressEnter { .. } => this.search_now(window, cx),
            _ => {}
        };
        let subscriptions = vec![
            cx.subscribe_in(&query, window, searches),
            cx.subscribe_in(&include, window, searches),
            cx.subscribe_in(&exclude, window, searches),
            // The replacement text only changes the preview.
            cx.subscribe_in(&replacement, window, |_, _, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            }),
        ];
        // A search still running in the backend is canceled when the view is released (the folder changed).
        cx.on_release(|this, cx| this.cancel_current(cx)).detach();

        Self {
            env,
            query,
            replacement,
            include,
            exclude,
            is_regex: false,
            case_sensitive: false,
            whole_word: false,
            show_replace: false,
            show_filters: false,
            results: Results::default(),
            rows: Vec::new(),
            preview_matcher: None,
            replacing: false,
            run: None,
            run_seq: 0,
            debounce: Task::ready(()),
            pump: Task::ready(()),
            scroll: UniformListScrollHandle::new(),
            results_focus: cx.focus_handle(),
            _subscriptions: subscriptions,
        }
    }

    // ----- State that other features and tests may read -----

    pub fn results(&self) -> &Results {
        &self.results
    }

    pub fn is_replacing(&self) -> bool {
        self.replacing
    }

    pub fn option(&self, option: SearchOption) -> bool {
        match option {
            SearchOption::CaseSensitive => self.case_sensitive,
            SearchOption::WholeWord => self.whole_word,
            SearchOption::Regex => self.is_regex,
        }
    }

    pub fn replace_shown(&self) -> bool {
        self.show_replace
    }

    /// The current form inputs.
    pub fn inputs(&self, cx: &App) -> Inputs {
        Inputs {
            pattern: self.query.read(cx).value().to_string(),
            is_regex: self.is_regex,
            case_sensitive: self.case_sensitive,
            whole_word: self.whole_word,
            include: self.include.read(cx).value().to_string(),
            exclude: self.exclude.read(cx).value().to_string(),
        }
    }

    fn sync_rows(&mut self, cx: &mut Context<Self>) {
        self.rows = self.results.rows();
        cx.notify();
    }

    // ----- Inputs -----

    /// Focuses the query field and selects its content; `prefill` (e.g. text selected in the editor)
    /// replaces the query and is searched right away.
    pub fn focus_query(&mut self, prefill: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(prefill) = prefill.filter(|prefill| prefill.as_str() != self.query.read(cx).value().as_ref()) {
            self.set_query(prefill, window, cx);
        }
        self.query.update(cx, |state, cx| {
            state.focus(window, cx);
            state.select_all(window, cx);
        });
    }

    /// Replaces the query and searches right away (without the typing delay).
    pub fn set_query(&mut self, pattern: impl Into<SharedString>, window: &mut Window, cx: &mut Context<Self>) {
        let pattern: SharedString = pattern.into();
        // `set_value` does not emit `Change`, so the search is started here.
        self.query.update(cx, |state, cx| state.set_value(pattern, window, cx));
        self.search_now(window, cx);
    }

    pub fn set_replacement(&mut self, replacement: impl Into<SharedString>, window: &mut Window, cx: &mut Context<Self>) {
        let replacement: SharedString = replacement.into();
        self.replacement.update(cx, |state, cx| state.set_value(replacement, window, cx));
        cx.notify();
    }

    pub fn toggle_option(&mut self, option: SearchOption, window: &mut Window, cx: &mut Context<Self>) {
        match option {
            SearchOption::CaseSensitive => self.case_sensitive = !self.case_sensitive,
            SearchOption::WholeWord => self.whole_word = !self.whole_word,
            SearchOption::Regex => self.is_regex = !self.is_regex,
        }
        self.schedule_search(window, cx);
    }

    /// Shows the replacement row; focuses the replacement field if the query is already filled in.
    pub fn show_replace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.show_replace = true;
        let target = if self.query.read(cx).value().is_empty() { self.query.clone() } else { self.replacement.clone() };
        target.update(cx, |state, cx| {
            state.focus(window, cx);
            state.select_all(window, cx);
        });
        cx.notify();
    }

    fn toggle_replace(&mut self, cx: &mut Context<Self>) {
        self.show_replace = !self.show_replace;
        cx.notify();
    }

    fn toggle_filters(&mut self, cx: &mut Context<Self>) {
        self.show_filters = !self.show_filters;
        cx.notify();
    }

    /// Clears the query and results; the search options are kept.
    pub fn clear(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.set_query("", window, cx);
    }

    // ----- How the search runs -----

    /// Schedules a search after the typing delay.
    fn schedule_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.results.pending = true;
        self.debounce = cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(SEARCH_DEBOUNCE_MS)).await;
            let _ = this.update_in(cx, |this, window, cx| this.search_now(window, cx));
        });
        cx.notify();
    }

    /// Cancels the running search; anything it still sends is ignored.
    fn cancel_current(&mut self, cx: &mut App) {
        self.pump = Task::ready(());
        let Some(run) = self.run.take() else { return };
        if !run.finished {
            let core = self.env.core.clone();
            spawn_core(cx, async move {
                let _ = app::app_cancel(&core, run.operation_id).await;
            })
            .detach();
        }
    }

    /// Starts a search right now with the current inputs; the previous search is canceled.
    /// An empty query clears the results.
    pub fn search_now(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.debounce = Task::ready(());
        self.cancel_current(cx);
        let inputs = self.inputs(cx);
        if inputs.pattern.is_empty() || self.env.root.is_none() {
            self.results.clear();
            self.preview_matcher = None;
            self.sync_rows(cx);
            return;
        }
        let query = build_query(&inputs);
        self.run_seq += 1;
        let seq = self.run_seq;
        let operation_id = format!("search-{}", NEXT_OPERATION.fetch_add(1, Ordering::Relaxed));
        self.run = Some(Run { seq, operation_id: operation_id.clone(), finished: false });
        self.preview_matcher = if query.is_regex { LineMatcher::new(&MatchOptions::from(&query)).ok() } else { None };
        self.results.start(query.clone());
        self.sync_rows(cx);

        // Events arrive from a backend thread; they are forwarded to the main thread through a channel.
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<SearchEvent>();
        let (core, label) = (self.env.core.clone(), self.env.window.to_string());
        spawn_core(cx, async move {
            let sink = tx.clone();
            let on_event: oncode_core::api::Callback<SearchEvent> = Arc::new(move |event| {
                let _ = sink.send(event);
            });
            if let Err(error) = fs::search_start(&core, &label, query, operation_id, on_event).await {
                let _ = tx.send(SearchEvent::Failed { message: error.message });
            }
        })
        .detach();

        self.pump = cx.spawn_in(window, async move |this, cx| {
            while let Some(first) = rx.recv().await {
                let mut batch = vec![first];
                while let Ok(event) = rx.try_recv() {
                    batch.push(event);
                }
                match this.update(cx, |this, cx| this.apply_events(seq, batch, cx)) {
                    Ok(true) => {}
                    _ => break,
                }
                // Results that arrive in succession are briefly batched so the list isn't redrawn per file.
                cx.background_executor().timer(Duration::from_millis(FLUSH_MS)).await;
            }
        });
    }

    /// Applies events from the backend. Returning `false` means this search has finished or
    /// has been replaced by another search.
    fn apply_events(&mut self, seq: u64, events: Vec<SearchEvent>, cx: &mut Context<Self>) -> bool {
        let Some(run) = self.run.as_mut().filter(|run| run.seq == seq && !run.finished) else { return false };
        let mut incoming: Vec<FileMatches> = Vec::new();
        let mut end = None;
        for event in events {
            match event {
                SearchEvent::Matches { file } => incoming.push(file),
                done_or_failed => {
                    end = Some(done_or_failed);
                    break;
                }
            }
        }
        if end.is_some() {
            run.finished = true;
        }
        self.results.merge(incoming);
        match end {
            Some(SearchEvent::Done { summary }) => self.results.finish(summary),
            Some(SearchEvent::Failed { message }) => self.results.fail(message),
            _ => {}
        }
        self.sync_rows(cx);
        self.results.status == SearchStatus::Running
    }

    // ----- Results -----

    fn selected_index(&self) -> Option<usize> {
        let selected = self.results.selected.as_ref()?;
        self.rows.iter().position(|row| self.results.row_key(*row).as_ref() == Some(selected))
    }

    fn select_index(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(row) = self.rows.get(index.min(self.rows.len().saturating_sub(1))).copied() else { return };
        self.results.selected = self.results.row_key(row);
        self.scroll.scroll_to_item(index.min(self.rows.len() - 1), ScrollStrategy::Nearest);
        cx.notify();
    }

    fn set_collapsed(&mut self, path: &str, collapsed: bool, cx: &mut Context<Self>) {
        self.results.set_collapsed(path, collapsed);
        self.sync_rows(cx);
    }

    fn toggle_all_collapsed(&mut self, cx: &mut Context<Self>) {
        let collapse = !self.results.all_collapsed();
        self.results.set_all_collapsed(collapse);
        self.sync_rows(cx);
    }

    fn click_file(&mut self, path: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.results.selected = Some(RowKey::File(path.to_string()));
        self.results_focus.focus(window, cx);
        let collapsed = self.results.is_collapsed(path);
        self.set_collapsed(path, !collapsed, cx);
    }

    fn click_match(&mut self, path: &str, key: MatchKey, window: &mut Window, cx: &mut Context<Self>) {
        self.results.selected = Some(RowKey::Match { path: path.to_string(), line: key.line, column: key.column });
        self.open_match(path, key, window, cx);
        cx.notify();
    }

    /// Opens the file as a preview tab and selects its match in the editor.
    fn open_match(&mut self, path: &str, key: MatchKey, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.env.root.as_ref() else { return };
        let Some(found) = self.results.file(path).and_then(|file| file.matches.iter().find(|m| m.line == key.line && m.column == key.column)) else { return };
        let (line, column, length) = reveal_position(found);
        let absolute = absolute_path(root, path);
        let Some(workbench) = self.env.workbench.upgrade() else { return };
        workbench.update(cx, |workbench, cx| open_file(workbench, OpenFile::new(absolute).preview().at(line, column, Some(length)), window, cx));
    }

    fn dismiss_file(&mut self, path: &str, cx: &mut Context<Self>) {
        self.results.dismiss_file(path);
        self.sync_rows(cx);
    }

    fn dismiss_match(&mut self, path: &str, key: MatchKey, cx: &mut Context<Self>) {
        self.results.dismiss_match(path, key);
        self.sync_rows(cx);
    }

    fn on_results_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let modifiers = event.keystroke.modifiers;
        if modifiers.control || modifiers.platform || modifiers.alt {
            return;
        }
        let index = self.selected_index();
        let row = index.and_then(|index| self.rows.get(index).copied());
        let file_path = |this: &Self, file: usize| this.results.files.get(file).map(|f| f.path.clone());
        match event.keystroke.key.as_str() {
            "down" => self.select_index(index.map_or(0, |index| index + 1), cx),
            "up" => self.select_index(index.map_or(0, |index| index.saturating_sub(1)), cx),
            "right" => {
                if let Some(Row::File { file }) = row {
                    if let Some(path) = file_path(self, file).filter(|path| self.results.is_collapsed(path)) {
                        self.set_collapsed(&path, false, cx);
                    }
                }
            }
            "left" => match row {
                Some(Row::File { file }) => {
                    if let Some(path) = file_path(self, file).filter(|path| !self.results.is_collapsed(path)) {
                        self.set_collapsed(&path, true, cx);
                    }
                }
                Some(Row::Match { file, .. }) => {
                    if let Some(path) = file_path(self, file) {
                        self.results.selected = Some(RowKey::File(path));
                        if let Some(index) = self.selected_index() {
                            self.scroll.scroll_to_item(index, ScrollStrategy::Nearest);
                        }
                        cx.notify();
                    }
                }
                None => {}
            },
            "enter" => match row {
                Some(Row::File { file }) => {
                    if let Some(path) = file_path(self, file) {
                        let collapsed = self.results.is_collapsed(&path);
                        self.set_collapsed(&path, !collapsed, cx);
                    }
                }
                Some(Row::Match { file, index }) => {
                    let found = self.results.files.get(file).and_then(|f| f.matches.get(index).map(|m| (f.path.clone(), match_key(m))));
                    if let Some((path, key)) = found {
                        self.open_match(&path, key, window, cx);
                    }
                }
                None => {}
            },
            _ => return,
        }
        cx.stop_propagation();
    }

    // ----- Replacement -----

    /// Replaces all matches still displayed, after confirmation; then searches again and
    /// reports the count.
    pub fn replace_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.results.can_replace(self.replacing) {
            return;
        }
        let replacement = self.replacement.read(cx).value().to_string();
        let (title, message) = confirmation_text(self.results.counts(), &replacement, self.results.truncated());
        self.confirm_replace(ReplaceScope::All, title, message, window, cx);
    }

    /// Replaces all matches still displayed in a single file, after confirmation.
    fn replace_file(&mut self, path: &str, window: &mut Window, cx: &mut Context<Self>) {
        if !self.results.can_replace(self.replacing) {
            return;
        }
        let Some(file) = self.results.file(path) else { return };
        let replacement = self.replacement.read(cx).value().to_string();
        let (title, message) = confirmation_text(counts_of(std::slice::from_ref(file)), &replacement, self.results.truncated());
        self.confirm_replace(ReplaceScope::File(path.to_string()), title, message, window, cx);
    }

    /// Replaces a single match. No confirmation: its row already shows exactly what changes.
    fn replace_match(&mut self, path: &str, key: MatchKey, window: &mut Window, cx: &mut Context<Self>) {
        if !self.results.can_replace(self.replacing) {
            return;
        }
        self.start_replace(ReplaceScope::Match(path.to_string(), key), window, cx);
    }

    fn confirm_replace(&mut self, scope: ReplaceScope, title: String, message: String, window: &mut Window, cx: &mut Context<Self>) {
        let answer = overlay::confirm(Confirm::new(title, message, "Replace"), window, cx);
        cx.spawn_in(window, async move |this, cx| {
            if answer.await {
                let _ = this.update_in(cx, |this, window, cx| this.start_replace(scope, window, cx));
            }
        })
        .detach();
    }

    fn start_replace(&mut self, scope: ReplaceScope, window: &mut Window, cx: &mut Context<Self>) {
        // The state may change while the confirmation dialog is open.
        if !self.results.can_replace(self.replacing) {
            return;
        }
        let Some(query) = self.results.query.clone() else { return };
        let picked = |path: &str| match self.results.dismissed.get(path) {
            Some(dismissed) if !dismissed.is_empty() => Picked::Except(dismissed.clone()),
            _ => Picked::All,
        };
        let targets: Vec<Target> = match &scope {
            ReplaceScope::All => self.results.files.iter().map(|file| Target { path: file.path.clone(), picked: picked(&file.path) }).collect(),
            ReplaceScope::File(path) => self.results.file(path).map(|file| Target { path: file.path.clone(), picked: picked(&file.path) }).into_iter().collect(),
            ReplaceScope::Match(path, key) => vec![Target { path: path.clone(), picked: Picked::Only(*key) }],
        };
        if targets.is_empty() {
            return;
        }
        let replacement = self.replacement.read(cx).value().to_string();
        let env = self.env.clone();
        self.replacing = true;
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let performed = replace::perform(&env, query, replacement, targets, cx).await;
            let _ = this.update_in(cx, |this, window, cx| this.replace_finished(scope, performed, window, cx));
        })
        .detach();
    }

    fn replace_finished(&mut self, scope: ReplaceScope, performed: replace::Performed, window: &mut Window, cx: &mut Context<Self>) {
        self.replacing = false;
        let outcome = performed.outcome;
        let text = report(&outcome);
        let single = matches!(scope, ReplaceScope::Match(..));
        if let Some(message) = text.success.filter(|_| !single || outcome.unsaved > 0) {
            overlay::toast_success(message, window, cx);
        }
        if let Some(message) = text.info {
            let message = if single { "This match is no longer there; the results were refreshed.".to_string() } else { message };
            overlay::toast_info(message, window, cx);
        }
        if let Some(message) = text.warning {
            overlay::toast_warning(message, window, cx);
        }
        if let Some((message, detail)) = text.error {
            overlay::show_error(&oncode_core::AppError { code: "replace".into(), message, detail: Some(detail) }, window, cx);
        }

        let clean = outcome.replacements > 0 && outcome.errors.is_empty() && outcome.skipped_read_only.is_empty();
        match scope {
            // The list is adjusted in place so the scroll position and collapsed state aren't lost.
            ReplaceScope::File(path) if clean => {
                self.results.dismiss_file(&path);
                self.sync_rows(cx);
            }
            ReplaceScope::Match(path, key) if clean => {
                self.results.match_replaced(&path, key, performed.inserted.as_deref().unwrap_or_default());
                self.sync_rows(cx);
            }
            _ => self.search_now(window, cx),
        }
    }

    // ----- View -----

    fn render_toolbar(&self, cx: &Context<Self>) -> impl IntoElement {
        let empty = self.query.read(cx).value().is_empty();
        let all_collapsed = self.results.all_collapsed();
        h_flex()
            .w_full()
            .flex_none()
            .justify_end()
            .gap_0p5()
            .px_2()
            .child(Button::new("search-again").ghost().xsmall().icon(Icon::new(Lucide::RefreshCw)).tooltip("Search Again").disabled(empty).on_click(cx.listener(|this, _, window, cx| this.search_now(window, cx))))
            .child(
                Button::new("search-clear")
                    .ghost()
                    .xsmall()
                    .icon(Icon::new(Lucide::Eraser))
                    .tooltip("Clear Search Results")
                    .disabled(empty)
                    .on_click(cx.listener(|this, _, window, cx| this.clear(window, cx))),
            )
            .child(
                Button::new("search-collapse-all")
                    .ghost()
                    .xsmall()
                    .icon(Icon::new(Lucide::ChevronsDownUp))
                    .tooltip(if all_collapsed { "Expand All" } else { "Collapse All" })
                    .disabled(self.results.files.is_empty())
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_all_collapsed(cx))),
            )
    }

    fn option_button(&self, id: &'static str, option: SearchOption, icon: Lucide, title: &'static str, cx: &Context<Self>) -> Button {
        Button::new(id)
            .ghost()
            .xsmall()
            .icon(Icon::new(icon))
            .tooltip(title)
            .selected(self.option(option))
            .on_click(cx.listener(move |this, _, window, cx| this.toggle_option(option, window, cx)))
    }

    fn render_form(&self, cx: &Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let primary = theme.primary;
        let filters_active = !self.show_filters && (!self.include.read(cx).value().trim().is_empty() || !self.exclude.read(cx).value().trim().is_empty());
        let options = h_flex()
            .gap_0p5()
            .child(self.option_button("search-option-case", SearchOption::CaseSensitive, Lucide::CaseSensitive, "Match Case", cx))
            .child(self.option_button("search-option-word", SearchOption::WholeWord, Lucide::WholeWord, "Match Whole Word", cx))
            .child(self.option_button("search-option-regex", SearchOption::Regex, Lucide::Regex, "Use Regular Expression", cx));

        v_flex()
            .w_full()
            .flex_none()
            .gap_1()
            .px_2()
            .pb_1()
            .child(
                h_flex()
                    .w_full()
                    .items_start()
                    .gap_1()
                    .child(
                        Button::new("search-toggle-replace")
                            .ghost()
                            .xsmall()
                            .icon(Icon::new(if self.show_replace { Lucide::ChevronDown } else { Lucide::ChevronRight }))
                            .tooltip("Toggle Replace")
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_replace(cx))),
                    )
                    .child(
                        v_flex().flex_1().min_w_0().gap_1().child(Input::new(&self.query).id("search-query").small().suffix(options)).when(self.show_replace, |el| {
                            el.child(
                                h_flex().w_full().gap_1().child(div().flex_1().min_w_0().child(Input::new(&self.replacement).id("search-replacement").small())).child(
                                    Button::new("search-replace-all")
                                        .ghost()
                                        .xsmall()
                                        .icon(Icon::new(Lucide::ReplaceAll))
                                        .tooltip("Replace All")
                                        .disabled(!self.results.can_replace(self.replacing))
                                        .on_click(cx.listener(|this, _, window, cx| this.replace_all(window, cx))),
                                ),
                            )
                        }),
                    ),
            )
            .child(
                h_flex()
                    .id("search-toggle-filters")
                    .test_support()
                    .gap_1()
                    .text_xs()
                    .text_color(muted)
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_filters(cx)))
                    .child(Icon::new(if self.show_filters { Lucide::ChevronDown } else { Lucide::ChevronRight }).xsmall())
                    .child("files to include / exclude")
                    .when(filters_active, |el| el.child(div().text_color(primary).child("active"))),
            )
            .when(self.show_filters, |el| {
                el.child(div().text_xs().text_color(muted).child("files to include"))
                    .child(Input::new(&self.include).id("search-include").small())
                    .child(div().text_xs().text_color(muted).child("files to exclude"))
                    .child(Input::new(&self.exclude).id("search-exclude").small())
            })
    }

    fn render_summary(&self, cx: &Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let busy = self.replacing || self.results.status == SearchStatus::Running;
        let line = self.results.status_line(self.replacing);
        h_flex()
            .id("search-summary")
            .test_support()
            .w_full()
            .min_h(px(22.))
            .flex_none()
            .gap_1p5()
            .px_3()
            .text_xs()
            .text_color(theme.muted_foreground)
            .when(busy, |el| el.child(Spinner::new().xsmall()))
            .when_some(line, |el, (text, is_error)| el.child(div().flex_1().min_w_0().when(is_error, |el| el.text_color(theme.danger)).child(SharedString::from(text))))
    }

    /// Action buttons on the right side of a row, shown when the row is hovered.
    fn row_actions(&self, cx: &App) -> gpui_kit::Div {
        h_flex().absolute().top_0().right_1().h_full().gap_0p5().bg(cx.theme().sidebar).invisible().group_hover("sr-row", |style| style.visible())
    }

    fn render_file_row(&self, file: &FileMatches, selected: bool, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (dir, name) = match file.path.rfind('/') {
            Some(slash) => (&file.path[..slash], &file.path[slash + 1..]),
            None => ("", file.path.as_str()),
        };
        let collapsed = self.results.is_collapsed(&file.path);
        let can_replace = self.show_replace && self.results.can_replace(self.replacing);
        let path: SharedString = file.path.clone().into();
        let (click_path, replace_path, dismiss_path, tooltip) = (path.clone(), path.clone(), path.clone(), path.clone());

        h_flex()
            .id(SharedString::from(format!("sr-file-{path}")))
            .test_support()
            .group("sr-row")
            .relative()
            .w_full()
            .h(ROW_HEIGHT)
            .px_1()
            .gap_1()
            .text_sm()
            .when(selected, |el| el.bg(theme.list_active))
            .when(!selected, |el| el.hover(|style| style.bg(theme.list_hover)))
            .tooltip(move |window, cx| Tooltip::new(tooltip.clone()).build(window, cx))
            .on_click(cx.listener(move |this, _, window, cx| this.click_file(&click_path, window, cx)))
            .child(Icon::new(if collapsed { Lucide::ChevronRight } else { Lucide::ChevronDown }).small().flex_none())
            .child(div().flex_none().child(SharedString::from(name.to_string())))
            .child(div().flex_1().min_w_0().truncate().text_xs().text_color(theme.muted_foreground).child(SharedString::from(dir.to_string())))
            .child(div().flex_none().px_1p5().rounded_full().bg(theme.muted).text_xs().child(file.matches.len().to_string()))
            .child(
                self.row_actions(cx)
                    .when(can_replace, |el| {
                        el.child(Button::new(SharedString::from(format!("sr-replace-{path}"))).ghost().xsmall().icon(Icon::new(Lucide::ReplaceAll)).tooltip("Replace All in File").on_click(cx.listener(
                            move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.replace_file(&replace_path, window, cx);
                            },
                        )))
                    })
                    .child(Button::new(SharedString::from(format!("sr-dismiss-{path}"))).ghost().xsmall().icon(Icon::new(Lucide::X)).tooltip("Dismiss file").on_click(cx.listener(
                        move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.dismiss_file(&dismiss_path, cx);
                        },
                    ))),
            )
            .into_any_element()
    }

    fn render_match_row(&self, file: &FileMatches, found: &LineMatch, selected: bool, replacement: Option<&str>, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let key = match_key(found);
        let can_replace = self.show_replace && self.results.can_replace(self.replacing);
        let path: SharedString = file.path.clone().into();
        let (click_path, replace_path, dismiss_path) = (path.clone(), path.clone(), path.clone());
        let suffix = format!("{path}:{}:{}", found.line, found.column);

        // In replace mode, the replacement text is shown next to the match (which is struck through).
        let inserted = replacement.map(|replacement| replacement_preview(self.preview_matcher.as_ref(), self.results.query.as_ref().is_some_and(|q| q.is_regex), replacement, found));
        let row = row_text(found, inserted.as_deref());
        let mut highlights = Vec::with_capacity(2);
        if !row.hit.is_empty() {
            let style = if inserted.is_some() {
                HighlightStyle { background_color: Some(theme.danger.opacity(0.25)), strikethrough: Some(StrikethroughStyle { thickness: px(1.), color: None }), ..Default::default() }
            } else {
                HighlightStyle { background_color: Some(theme.warning.opacity(0.35)), ..Default::default() }
            };
            highlights.push((row.hit.clone(), style));
        }
        if let Some(range) = row.inserted.clone() {
            highlights.push((range, HighlightStyle { background_color: Some(theme.success.opacity(0.25)), ..Default::default() }));
        }

        h_flex()
            .id(SharedString::from(format!("sr-match-{suffix}")))
            .test_support()
            .group("sr-row")
            .relative()
            .w_full()
            .h(ROW_HEIGHT)
            .pl_6()
            .pr_1()
            .gap_2()
            .when(selected, |el| el.bg(theme.list_active))
            .when(!selected, |el| el.hover(|style| style.bg(theme.list_hover)))
            .on_click(cx.listener(move |this, _, window, cx| this.click_match(&click_path, key, window, cx)))
            .child(div().flex_none().min_w(px(22.)).text_xs().text_color(theme.muted_foreground).child(found.line.to_string()))
            .child(div().flex_1().min_w_0().truncate().text_xs().font_family(theme.mono_font_family.clone()).child(StyledText::new(row.text).with_highlights(highlights)))
            .child(
                self.row_actions(cx)
                    .when(can_replace, |el| {
                        el.child(Button::new(SharedString::from(format!("sr-replace-{suffix}"))).ghost().xsmall().icon(Icon::new(Lucide::Replace)).tooltip("Replace").on_click(cx.listener(
                            move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.replace_match(&replace_path, key, window, cx);
                            },
                        )))
                    })
                    .child(Button::new(SharedString::from(format!("sr-dismiss-{suffix}"))).ghost().xsmall().icon(Icon::new(Lucide::X)).tooltip("Dismiss match").on_click(cx.listener(
                        move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.dismiss_match(&dismiss_path, key, cx);
                        },
                    ))),
            )
            .into_any_element()
    }

    fn render_rows(&self, range: Range<usize>, cx: &Context<Self>) -> Vec<AnyElement> {
        let replacement = self.show_replace.then(|| self.replacement.read(cx).value());
        let selected = self.selected_index();
        range
            .map(|index| {
                let is_selected = selected == Some(index);
                match self.rows.get(index).copied() {
                    Some(Row::File { file }) => match self.results.files.get(file) {
                        Some(file) => self.render_file_row(file, is_selected, cx),
                        None => div().h(ROW_HEIGHT).into_any_element(),
                    },
                    Some(Row::Match { file, index }) => match self.results.files.get(file).and_then(|file| file.matches.get(index).map(|found| (file, found))) {
                        Some((file, found)) => self.render_match_row(file, found, is_selected, replacement.as_deref(), cx),
                        None => div().h(ROW_HEIGHT).into_any_element(),
                    },
                    None => div().h(ROW_HEIGHT).into_any_element(),
                }
            })
            .collect()
    }
}

impl Render for SearchView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.env.root.is_none() {
            return div().id("search-view").size_full().px_3().py_2().text_sm().text_color(cx.theme().muted_foreground).child("Open a folder to search in its files.").into_any_element();
        }
        v_flex()
            .id("search-view")
            .size_full()
            .child(self.render_toolbar(cx))
            .child(self.render_form(cx))
            .child(self.render_summary(cx))
            .child(
                div()
                    .id("search-results")
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .track_focus(&self.results_focus)
                    .on_key_down(cx.listener(Self::on_results_key))
                    .child(uniform_list("search-rows", self.rows.len(), cx.processor(|this, range: Range<usize>, _, cx| this.render_rows(range, cx))).track_scroll(&self.scroll).size_full()),
            )
            .into_any_element()
    }
}
