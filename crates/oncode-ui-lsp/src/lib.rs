//! LSP client for the editor, the Problems panel, and the LSP commands (go to definition, references,
//! rename, format, restart).

pub mod diagnostics;
pub mod edits;
pub mod hub;
pub mod model;

use std::collections::HashSet;
use std::ops::Range;

use gpui_kit::component::{h_flex, v_flex, ActiveTheme as _, Icon, Sizable as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    div, px, uniform_list, AnyElement, App, AppContext as _, Context, Entity, FocusHandle, Focusable, InteractiveElement as _, IntoElement, ParentElement as _, Render,
    SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Task, TestSupportExt as _, Window,
};
use lsp_types::request::{Formatting, References, Rename};
use lsp_types::{DocumentFormattingParams, FormattingOptions, ReferenceContext, ReferenceParams, RenameParams, TextDocumentIdentifier, TextDocumentPositionParams, WorkDoneProgressParams};
use oncode_lsp::position::position_at;
use oncode_lsp::uri::{path_to_lsp_uri, uri_to_path};
use oncode_lsp::Feature;
use oncode_ui::commands::Command;
use oncode_ui::editor::{active_file, active_text_file, language_for_path, open_file, Buffer, FormatOutcome, OpenFile};
use oncode_ui::overlay::{self, TextPrompt};
use oncode_ui::quick_pick::{quick_pick, PickItem, QuickPickOptions};
use oncode_ui::{spawn_core, Lucide, PanelView, StatusAlign, StatusItem, Workbench};
use serde_json::Value;

use diagnostics::{Diagnostics, Severity};
use hub::{HubEvent, LspHub, ServerStatus};
use model::{build_groups, flatten_rows, relative_path, ProblemFilter, ProblemGroup, ProblemRow};

pub const PROBLEMS_PANEL: &str = "problems";

/// Registers this feature. Called once from `app/src/main.rs` after `oncode_ui::init`.
pub fn init(cx: &mut App) {
    oncode_ui::editor::register_format_provider(oncode_ui::editor::LSP_PRIORITY, lsp_format_provider, cx);
    oncode_ui::register_feature(cx, setup);
}

/// This window's diagnostics store.
pub fn diagnostics(workbench: &Workbench) -> Option<Entity<Diagnostics>> {
    workbench.extension::<Entity<Diagnostics>>()
}

/// This window's language server manager; `None` if the folder is not trusted.
pub fn hub(workbench: &Workbench) -> Option<Entity<LspHub>> {
    workbench.extension::<Entity<LspHub>>()
}

fn setup(workbench: &mut Workbench, window: &mut Window, cx: &mut Context<Workbench>) {
    let env = workbench.env().clone();
    if env.root.is_none() {
        return;
    }
    let diagnostics = cx.new(|_| Diagnostics::default());
    workbench.set_extension(diagnostics.clone());
    workbench.register_panel(
        PanelView::new(PROBLEMS_PANEL, "Problems", 20, {
            let diagnostics = diagnostics.clone();
            move |env, window, cx| {
                let (diagnostics, root) = (diagnostics.clone(), env.root_string());
                cx.new(|cx| ProblemsPanel::new(diagnostics, root, window, cx)).into()
            }
        })
        .badge({
            let diagnostics = diagnostics.clone();
            move |cx| {
                let counts = diagnostics.read(cx).counts();
                Some(counts.errors + counts.warnings)
            }
        }),
    );
    workbench.register_status(
        StatusItem::new("problems.summary", StatusAlign::Left, 20, {
            let diagnostics = diagnostics.clone();
            move |_, _, cx| {
                let diagnostics = diagnostics.clone();
                cx.new(|cx| ProblemsStatus::new(diagnostics, cx)).into()
            }
        }),
        window,
        cx,
    );
    // Language servers only run for trusted folders.
    if !env.trusted {
        return;
    }
    let Some(buffers) = oncode_ui::editor::buffers(workbench) else { return };
    let hub = cx.new(|cx| LspHub::new(env.clone(), buffers, diagnostics, window, cx));
    workbench.set_extension(hub.clone());
    workbench.register_status(
        StatusItem::new("lsp.server", StatusAlign::Right, 30, {
            let hub = hub.clone();
            move |env, _, cx| {
                let (hub, workbench) = (hub.clone(), env.workbench.clone());
                cx.new(|cx| LspStatus::new(hub, workbench, cx)).into()
            }
        }),
        window,
        cx,
    );

    // Available when the active file has a running server; the shortcut only applies while the editor
    // is focused (F2 is also used by the explorer).
    let has_server = {
        let hub = hub.clone();
        move |window: &Window, cx: &App| {
            let Some(workbench) = Workbench::for_window(window, cx) else { return false };
            let Some((_, buffer, _)) = active_text_file(workbench.read(cx), cx) else { return false };
            hub.read(cx).server_for_path(buffer.read(cx).path()).is_some()
        }
    };
    let editor_focused = |window: &Window, cx: &App| {
        let Some(workbench) = Workbench::for_window(window, cx) else { return false };
        active_text_file(workbench.read(cx), cx).is_some_and(|(_, _, state)| state.focus_handle(cx).is_focused(window))
    };
    // "Go to Definition" (F12) and "Format Document" are registered by the editor: both also work
    // without a server (project search, the Formatters extension); this server is one of their providers.
    workbench.register_command(
        Command::new("editor.findReferences", "Find All References", find_references).category("Edit").key("Shift+F12").when(has_server.clone()).keys_when(editor_focused),
    );
    workbench.register_command(Command::new("editor.rename", "Rename Symbol", rename_symbol).category("Edit").key("F2").when(has_server).keys_when(editor_focused));
    workbench.register_command(
        Command::new("lsp.restart", "Restart Language Server", |_, window, cx| {
            let Some(workbench) = Workbench::for_window(window, cx) else { return };
            let Some((_, buffer, _)) = active_file(workbench.read(cx), cx) else { return };
            let language = language_for_path(buffer.read(cx).path()).lsp.to_string();
            if let Some(hub) = crate::hub(workbench.read(cx)) {
                hub.update(cx, |hub, cx| hub.restart(&language, window, cx));
            }
        })
        .category("LSP"),
    );
}

/// Request context for the active file: hub, path, uri, client, and cursor position.
struct Active {
    workbench: Entity<Workbench>,
    hub: Entity<LspHub>,
    state: Entity<gpui_kit::component::input::EditorState>,
    client: std::sync::Arc<oncode_lsp::Client>,
    position: lsp_types::Position,
    uri: lsp_types::Uri,
}

fn active(feature: Feature, unsupported: &str, window: &mut Window, cx: &mut App) -> Option<Active> {
    let workbench = Workbench::for_window(window, cx)?;
    let hub = hub(workbench.read(cx))?;
    let (_, buffer, state) = active_text_file(workbench.read(cx), cx)?;
    let path = buffer.read(cx).path().to_string();
    let language = language_for_path(&path).lsp.to_string();
    let text = state.read(cx).text().clone();
    let offset = state.read(cx).cursor();
    let (client, position) = hub.update(cx, |hub, _| hub.request_context(&language, &path, &text, offset))?;
    if !client.supports(feature) {
        overlay::toast_info(unsupported.to_string(), window, cx);
        return None;
    }
    let uri = path_to_lsp_uri(&path)?;
    Some(Active { workbench, hub, state, client, position, uri })
}

fn find_references(_: &Value, window: &mut Window, cx: &mut App) {
    let Some(active) = active(Feature::References, "The language server does not support Find References.", window, cx) else { return };
    let params = ReferenceParams {
        text_document_position: TextDocumentPositionParams { text_document: TextDocumentIdentifier { uri: active.uri.clone() }, position: active.position },
        work_done_progress_params: WorkDoneProgressParams::default(),
        partial_result_params: Default::default(),
        context: ReferenceContext { include_declaration: true },
    };
    let task = spawn_core(cx, active.client.request::<References>(params));
    let root = active.workbench.read(cx).env().root_string();
    let workbench = active.workbench.downgrade();
    window
        .spawn(cx, async move |cx| {
            let result = task.await;
            let _ = cx.update(|window, cx| match result {
                Ok(Some(locations)) if !locations.is_empty() => {
                    let items = locations
                        .iter()
                        .map(|location| {
                            let path = uri_to_path(location.uri.as_str());
                            let start = location.range.start;
                            let value = format!("{}\u{0}{}\u{0}{}", path, start.line + 1, start.character);
                            PickItem::new(format!("{}:{}", relative_path(root.as_deref(), &path), start.line + 1), value)
                        })
                        .collect();
                    let workbench = workbench.clone();
                    quick_pick(
                        QuickPickOptions::new(format!("{} references", locations.len()), items),
                        move |item, window, cx| {
                            let Some(item) = item else { return };
                            let mut parts = item.value.split('\u{0}');
                            let (Some(path), Some(line), Some(column)) = (parts.next(), parts.next(), parts.next()) else { return };
                            let options = OpenFile::new(path).at(line.parse().unwrap_or(1), column.parse().unwrap_or(0), None);
                            let _ = workbench.update(cx, |workbench, cx| open_file(workbench, options, window, cx));
                        },
                        window,
                        cx,
                    );
                }
                Ok(_) => overlay::toast_info("No references found.", window, cx),
                Err(error) => overlay::toast_error(format!("Find References failed: {error}"), window, cx),
            });
        })
        .detach();
}

fn rename_symbol(_: &Value, window: &mut Window, cx: &mut App) {
    let Some(active) = active(Feature::Rename, "The language server does not support Rename.", window, cx) else { return };
    let text = active.state.read(cx).text().clone();
    let current = text.word_at_offset(active.state.read(cx).cursor());
    let answer = overlay::prompt_text(TextPrompt::new("Rename Symbol", "Rename").initial(current), window, cx);
    let hub = active.hub.downgrade();
    window
        .spawn(cx, async move |cx| {
            let Some(name) = answer.await.map(|n| n.trim().to_string()).filter(|n| !n.is_empty()) else { return };
            let params = RenameParams {
                text_document_position: TextDocumentPositionParams { text_document: TextDocumentIdentifier { uri: active.uri.clone() }, position: active.position },
                new_name: name,
                work_done_progress_params: WorkDoneProgressParams::default(),
            };
            let Ok(task) = cx.update(|_, cx| spawn_core(cx, active.client.request::<Rename>(params))) else { return };
            let result = task.await;
            let _ = cx.update(|window, cx| match result {
                Ok(Some(edit)) => {
                    edits::apply_workspace_edit(hub.clone(), edit, window, cx).detach();
                }
                Ok(None) => overlay::toast_info("Place the cursor on a symbol the language server can rename.", window, cx),
                Err(error) => overlay::toast_error(format!("Rename failed: {error}"), window, cx),
            });
        })
        .detach();
}

/// The language server's format provider: used when the Formatters extension has no formatter for
/// this file. `None` if the server is not running or does not support formatting.
fn lsp_format_provider(buffer: &Entity<Buffer>, state: &Entity<gpui_kit::component::input::EditorState>, window: &mut Window, cx: &mut App) -> Option<Task<FormatOutcome>> {
    let workbench = Workbench::for_window(window, cx)?;
    let hub = hub(workbench.read(cx))?;
    let path = buffer.read(cx).path().to_string();
    let language = language_for_path(&path).lsp.to_string();
    let text = state.read(cx).text().clone();
    let (client, _) = hub.update(cx, |hub, _| hub.request_context(&language, &path, &text, 0))?;
    if !client.supports(Feature::Formatting) {
        return None;
    }
    let settings = workbench.read(cx).settings().editor.clone();
    let params = DocumentFormattingParams {
        text_document: TextDocumentIdentifier { uri: path_to_lsp_uri(&path)? },
        options: FormattingOptions { tab_size: settings.tab_size, insert_spaces: settings.insert_spaces, ..Default::default() },
        work_done_progress_params: WorkDoneProgressParams::default(),
    };
    let encoding = client.encoding();
    let task = spawn_core(cx, client.request::<Formatting>(params));
    let original = text.to_string();
    Some(cx.background_spawn(async move {
        match task.await {
            Ok(Some(edits)) if !edits.is_empty() => FormatOutcome::Formatted(oncode_lsp::position::apply_text_edits(&original, &edits, encoding)),
            Ok(_) => FormatOutcome::Unchanged,
            Err(error) => FormatOutcome::Failed(format!("Format Document failed: {error}")),
        }
    }))
}

/// The word under the cursor, to prefill the rename box.
trait WordAt {
    fn word_at_offset(&self, offset: usize) -> String;
}

impl WordAt for gpui_kit::component::input::Rope {
    fn word_at_offset(&self, offset: usize) -> String {
        let text = self.to_string();
        let is_word = |c: char| c.is_alphanumeric() || c == '_';
        let offset = offset.min(text.len());
        let start = text[..offset].char_indices().rev().take_while(|(_, c)| is_word(*c)).last().map_or(offset, |(i, _)| i);
        let end = text[offset..].char_indices().take_while(|(_, c)| is_word(*c)).last().map_or(offset, |(i, c)| offset + i + c.len_utf8());
        text[start..end].to_string()
    }
}

// ----- Problems panel -----

pub struct ProblemsPanel {
    diagnostics: Entity<Diagnostics>,
    root: Option<String>,
    filter: ProblemFilter,
    collapsed: HashSet<String>,
    groups: Vec<ProblemGroup>,
    rows: Vec<ProblemRow>,
    focus_handle: FocusHandle,
    _subscription: Subscription,
}

impl Focusable for ProblemsPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl ProblemsPanel {
    pub fn new(diagnostics: Entity<Diagnostics>, root: Option<String>, _: &mut Window, cx: &mut Context<Self>) -> Self {
        let subscription = cx.observe(&diagnostics, |this, _, cx| this.rebuild(cx));
        let mut this = Self { diagnostics, root, filter: ProblemFilter::default(), collapsed: HashSet::new(), groups: Vec::new(), rows: Vec::new(), focus_handle: cx.focus_handle(), _subscription: subscription };
        this.rebuild(cx);
        this
    }

    fn rebuild(&mut self, cx: &mut Context<Self>) {
        let store = self.diagnostics.read(cx);
        let root = self.root.clone();
        self.groups = build_groups(store.files().iter().map(|(path, file)| (path.as_str(), file.problems.as_slice())), &self.filter, |path| relative_path(root.as_deref(), path));
        self.rows = flatten_rows(&self.groups, &self.collapsed);
        cx.notify();
    }

    fn render_rows(&mut self, range: Range<usize>, _: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let theme = cx.theme();
        range
            .filter_map(|index| {
                let row = self.rows.get(index)?.clone();
                Some(match row {
                    ProblemRow::File(group) => {
                        let group = &self.groups[group];
                        let path = group.path.clone();
                        h_flex()
                            .id(SharedString::from(format!("problems-file-{}", group.path)))
                            .h(px(24.))
                            .px_2()
                            .gap_2()
                            .text_sm()
                            .hover(|s| s.bg(theme.list_hover))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if !this.collapsed.remove(&path) {
                                    this.collapsed.insert(path.clone());
                                }
                                this.rebuild(cx);
                            }))
                            .child(Icon::new(if self.collapsed.contains(&group.path) { Lucide::ChevronRight } else { Lucide::ChevronDown }).xsmall())
                            .child(div().child(group.name.clone()))
                            .child(div().text_xs().text_color(theme.muted_foreground).child(group.dir.clone()))
                            .child(div().flex_1())
                            .child(div().px_1().rounded_full().bg(theme.secondary).text_xs().child(group.items.len().to_string()))
                            .into_any_element()
                    }
                    ProblemRow::Problem(group, item) => {
                        let path = self.groups[group].path.clone();
                        let problem = self.groups[group].items[item].clone();
                        let (icon, color) = match problem.severity {
                            Severity::Error => (Lucide::CircleX, theme.danger),
                            Severity::Warning => (Lucide::TriangleAlert, theme.warning),
                            Severity::Info | Severity::Hint => (Lucide::Info, theme.info),
                        };
                        let detail = match (&problem.source, &problem.code) {
                            (Some(source), Some(code)) => format!("{source}({code})"),
                            (Some(source), None) => source.clone(),
                            (None, Some(code)) => code.clone(),
                            (None, None) => String::new(),
                        };
                        let (line, column) = (problem.line, problem.column);
                        h_flex()
                            .id(SharedString::from(format!("problem-{path}-{line}-{column}-{item}")))
                            .test_support()
                            .h(px(24.))
                            .pl_8()
                            .pr_2()
                            .gap_2()
                            .text_sm()
                            .hover(|s| s.bg(theme.list_hover))
                            .on_click(move |_, window, cx| {
                                let Some(workbench) = Workbench::for_window(window, cx) else { return };
                                let options = OpenFile::new(path.clone()).at(line, column, None);
                                workbench.update(cx, |workbench, cx| open_file(workbench, options, window, cx));
                            })
                            .child(Icon::new(icon).xsmall().text_color(color))
                            .child(div().flex_1().min_w_0().truncate().child(problem.message.clone()))
                            .child(div().flex_none().text_xs().text_color(theme.muted_foreground).child(format!("{detail} [Ln {line}, Col {}]", column + 1)))
                            .into_any_element()
                    }
                })
            })
            .collect()
    }
}

impl Render for ProblemsPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let count = self.rows.len();
        v_flex().id("problems-panel").size_full().track_focus(&self.focus_handle).map(|el| {
            if count == 0 {
                el.p_3().text_sm().text_color(cx.theme().muted_foreground).child("No problems have been detected in the workspace.")
            } else {
                el.child(uniform_list("problems-list", count, cx.processor(Self::render_rows)).size_full())
            }
        })
    }
}

/// Status bar item: the error and warning counts; clicking opens the Problems panel.
struct ProblemsStatus {
    diagnostics: Entity<Diagnostics>,
    _subscription: Subscription,
}

impl ProblemsStatus {
    fn new(diagnostics: Entity<Diagnostics>, cx: &mut Context<Self>) -> Self {
        let subscription = cx.observe(&diagnostics, |_, _, cx| cx.notify());
        Self { diagnostics, _subscription: subscription }
    }
}

impl Render for ProblemsStatus {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let counts = self.diagnostics.read(cx).counts();
        let theme = cx.theme();
        h_flex()
            .id("status-problems")
            .test_support()
            .gap_1()
            .px_1()
            .rounded(theme.radius)
            .hover(|s| s.bg(theme.secondary_hover))
            .text_color(theme.muted_foreground)
            .on_click(|_, window, cx| {
                if let Some(workbench) = Workbench::for_window(window, cx) {
                    workbench.update(cx, |workbench, cx| workbench.show_panel(PROBLEMS_PANEL, cx));
                }
            })
            .child(Icon::new(Lucide::CircleX).xsmall())
            .child(counts.errors.to_string())
            .child(Icon::new(Lucide::TriangleAlert).xsmall())
            .child(counts.warnings.to_string())
    }
}

/// Status bar item: the language server status of the active file.
struct LspStatus {
    hub: Entity<LspHub>,
    workbench: gpui_kit::WeakEntity<Workbench>,
    _subscriptions: Vec<Subscription>,
}

impl LspStatus {
    fn new(hub: Entity<LspHub>, workbench: gpui_kit::WeakEntity<Workbench>, cx: &mut Context<Self>) -> Self {
        let mut subscriptions = vec![cx.subscribe(&hub, |_, _, _: &HubEvent, cx| cx.notify())];
        if let Some(entity) = workbench.upgrade() {
            subscriptions.push(cx.subscribe(&entity, |_, _, _: &oncode_ui::WorkbenchEvent, cx| cx.notify()));
        }
        Self { hub, workbench, _subscriptions: subscriptions }
    }
}

impl Render for LspStatus {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(path) = self.workbench.upgrade().and_then(|w| active_text_file(w.read(cx), cx)).map(|(_, buffer, _)| buffer.read(cx).path().to_string()) else {
            return div().into_any_element();
        };
        let language = language_for_path(&path).lsp;
        let Some(server) = self.hub.read(cx).server(language) else { return div().into_any_element() };
        let theme = cx.theme();
        let (text, color) = match &server.status {
            ServerStatus::Starting => (format!("{language}: starting…"), theme.muted_foreground),
            ServerStatus::Running => (language.to_string(), theme.muted_foreground),
            ServerStatus::Failed(_) => (format!("{language}: unavailable"), theme.warning),
            ServerStatus::Stopped => (format!("{language}: stopped"), theme.muted_foreground),
        };
        let tooltip = match &server.status {
            ServerStatus::Failed(message) => message.clone(),
            _ => "Language server".into(),
        };
        div()
            .id("status-lsp")
            .text_color(color)
            .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(tooltip.clone()).build(window, cx))
            .child(text)
            .into_any_element()
    }
}

/// The active editor's cursor position in the server's units, for tests.
pub fn cursor_position(text: &str, offset: usize, encoding: oncode_lsp::PositionEncoding) -> lsp_types::Position {
    position_at(text, offset, encoding)
}
