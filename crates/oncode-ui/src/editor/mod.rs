//! The editor feature: the `file` tab kind, saving, the status bar, and reacting to file
//! changes from outside.

mod buffer;
pub mod format;
mod language;
pub mod lines;
pub mod symbols;
mod view;

use gpui_kit::component::input::{EditorState, RopeExt as _};
use gpui_kit::component::{h_flex, ActiveTheme as _};
use gpui_kit::{div, App, AppContext as _, ClipboardItem, Context, Entity, Focusable as _, IntoElement, ParentElement as _, Render, SharedString, Styled as _, Subscription, WeakEntity, Window};
use oncode_core::settings::AutoSave;
use oncode_workspace::Eol;
use serde_json::Value;

pub use buffer::{file_path, is_untitled, Buffer, BufferEvent, BufferStatus, Buffers, BuffersEvent, ExternalChange, LineHintProvider, Reveal, LARGE_FILE_BYTES, UNTITLED_PREFIX};
pub use format::{format_buffer, register_format_provider, save_with_format, FormatOutcome, EXTERNAL_PRIORITY, LSP_PRIORITY};
pub use language::{language_for_path, LanguageInfo};
pub use symbols::{same_path, show_document_handler, symbol_definition_links, SymbolSearchProvider};
pub use view::{FileTab, FileView};

use crate::commands::Command;
use crate::model::{OpenOptions, TabData};
use crate::overlay::{self, TextPrompt};
use crate::shell::file_tab;
use crate::workbench::{register_feature, register_tab_kind, Env, StatusAlign, StatusItem, TabContent, TabKind, Workbench, WorkbenchEvent};
use crate::Lucide;

/// How to open a file.
#[derive(Debug, Clone, Default)]
pub struct OpenFile {
    /// Absolute path.
    pub path: String,
    pub preview: bool,
    pub to_side: bool,
    /// The position to show right away.
    pub reveal: Option<Reveal>,
}

impl OpenFile {
    pub fn new(path: impl Into<String>) -> Self {
        Self { path: path.into(), ..Default::default() }
    }

    pub fn preview(mut self) -> Self {
        self.preview = true;
        self
    }

    pub fn to_side(mut self) -> Self {
        self.to_side = true;
        self
    }

    /// `line` starts at 1, `column` starts at 0 (characters).
    pub fn at(mut self, line: u32, column: u32, length: Option<u32>) -> Self {
        self.reveal = Some(Reveal { line, column, length });
        self
    }
}

/// Opens a file in the editor, optionally jumping straight to a position.
pub fn open_file(workbench: &mut Workbench, mut options: OpenFile, window: &mut Window, cx: &mut Context<Workbench>) {
    options.path = file_path(&options.path);
    if let (Some(reveal), Some(buffers)) = (options.reveal.clone(), workbench.extension::<Entity<Buffers>>()) {
        let path = options.path.clone();
        buffers.update(cx, |buffers, cx| buffers.set_reveal(&path, reveal, cx));
    }
    workbench.open_tab(file_tab(&options.path), OpenOptions { preview: options.preview, to_side: options.to_side, ..Default::default() }, window, cx);
}

/// The buffer registry of this window.
pub fn buffers(workbench: &Workbench) -> Option<Entity<Buffers>> {
    workbench.extension::<Entity<Buffers>>()
}

/// The file in the active tab: its view, buffer, and editor state.
pub fn active_file(workbench: &Workbench, cx: &App) -> Option<(Entity<FileView>, Entity<Buffer>, Entity<EditorState>)> {
    let tab = workbench.active_content()?.as_any().downcast_ref::<FileTab>()?;
    let state = tab.view.read(cx).state().clone();
    Some((tab.view.clone(), tab.buffer.clone(), state))
}

struct FileKind;

impl TabKind for FileKind {
    fn build(&self, tab: &TabData, env: &Env, window: &mut Window, cx: &mut App) -> Box<dyn TabContent> {
        let path = tab.param_str("path").unwrap_or_default().to_string();
        // The buffer registry is installed by `setup` before any tab is built.
        let buffers = cx.global::<PendingBuffers>().0.clone().expect("buffer registry is not installed yet");
        let env = env.clone();
        let view = cx.new(|cx| FileView::new(&path, env, buffers, window, cx));
        let buffer = view.read(cx).buffer().clone();
        Box::new(FileTab { view, buffer })
    }

    fn icon(&self, _tab: &TabData) -> Option<Lucide> {
        Some(Lucide::FileText)
    }

    fn image(&self, tab: &TabData) -> Option<SharedString> {
        Some(crate::file_icons::file_icon_path(tab.param_str("path").unwrap_or_default()))
    }
}

/// The buffer registry of the `Workbench` that is currently building tabs (see `Workbench::sync_slots`).
#[derive(Default)]
struct PendingBuffers(Option<Entity<Buffers>>);

impl gpui_kit::Global for PendingBuffers {}

/// Registers grammars not provided by gpui-kit with the highlighter (once per process).
fn register_grammars() {
    use gpui_kit::component::highlighter::{GrammarConfig, LanguageRegistry};
    LanguageRegistry::singleton().register(
        "dart",
        &GrammarConfig {
            name: "dart".into(),
            language: Some(tree_sitter_dart::LANGUAGE.into()),
            injection_languages: Vec::new(),
            highlights: tree_sitter_dart::HIGHLIGHTS_QUERY.into(),
            injections: "".into(),
            locals: tree_sitter_dart::LOCALS_QUERY.into(),
        },
    );
}

pub(crate) fn install(cx: &mut App) {
    register_grammars();
    format::register_format_provider(format::EXTERNAL_PRIORITY, format::external_provider, cx);
    cx.set_global(PendingBuffers::default());
    register_tab_kind(cx, "file", FileKind);
    register_feature(cx, setup);
}

fn setup(workbench: &mut Workbench, window: &mut Window, cx: &mut Context<Workbench>) {
    let env = workbench.env().clone();
    let buffers = cx.new(|_| Buffers::new(env.clone()));
    workbench.set_extension(buffers.clone());
    workbench.on_before_build_tabs({
        let buffers = buffers.clone();
        move |cx| cx.global_mut::<PendingBuffers>().0 = Some(buffers.clone())
    });

    let weak = cx.weak_entity();
    let has_file = {
        let weak = weak.clone();
        move |_: &Window, cx: &App| weak.upgrade().is_some_and(|w| active_file(w.read(cx), cx).is_some())
    };
    let with_file = |f: fn(Entity<FileView>, Entity<Buffer>, Entity<EditorState>, &mut Window, &mut App)| {
        let weak = weak.clone();
        move |_: &Value, window: &mut Window, cx: &mut App| {
            let Some(active) = weak.upgrade().and_then(|w| active_file(w.read(cx), cx)) else { return };
            f(active.0, active.1, active.2, window, cx);
        }
    };

    workbench.register_command(
        Command::new("file.save", "Save", with_file(|_, buffer, state, window, cx| save_with_format(&buffer, &state, window, cx)))
            .category("File")
            .key("Mod+S")
            .when(has_file.clone()),
    );
    workbench.register_command(
        Command::new("file.newUntitled", "New Text File", {
            let (weak, buffers) = (weak.clone(), buffers.clone());
            move |_, window, cx| {
                let path = buffers.read(cx).next_untitled_path();
                let _ = weak.update(cx, |workbench, cx| workbench.open_tab(file_tab(&path), OpenOptions::default(), window, cx));
            }
        })
        .category("File")
        .key("Mod+N"),
    );
    workbench.register_command(
        Command::new("file.saveAll", "Save All", {
            let buffers = buffers.clone();
            move |_, window, cx| {
                for buffer in buffers.read(cx).dirty(cx) {
                    buffer.update(cx, |buffer, cx| buffer.save(window, cx)).detach();
                }
            }
        })
        .category("File")
        .key("Mod+Alt+S"),
    );
    workbench.register_command(
        Command::new("editor.find", "Find", with_file(|_, _, state, window, cx| state.update(cx, |state, cx| {
            state.open_search(false, cx);
            state.focus(window, cx);
        })))
        .category("Edit")
        .when(has_file.clone()),
    );
    workbench.register_command(
        Command::new("editor.gotoLine", "Go to Line…", with_file(|view, _, _, window, cx| {
            let (line, column, _) = view.read(cx).cursor_info(cx);
            let answer = overlay::prompt_text(TextPrompt::new("Go to line", "Go").placeholder(format!("{line}:{column}")), window, cx);
            window
                .spawn(cx, async move |cx| {
                    let Some(text) = answer.await else { return };
                    let mut parts = text.split(':').map(|part| part.trim().parse::<u32>().ok());
                    let Some(Some(line)) = parts.next() else { return };
                    let column = parts.next().flatten().unwrap_or(1).saturating_sub(1);
                    let _ = view.update_in(cx, |view, window, cx| view.reveal(&Reveal { line: line.max(1), column, length: None }, window, cx));
                })
                .detach();
        }))
        .category("Edit")
        .key("Mod+G")
        .when(has_file.clone()),
    );
    // Only when the editor is actually focused: in other inputs (terminal, commit message) the keys stay with that input.
    let editor_focused = {
        let weak = weak.clone();
        move |window: &Window, cx: &App| weak.upgrade().and_then(|w| active_file(w.read(cx), cx)).is_some_and(|(_, _, state)| state.focus_handle(cx).is_focused(window))
    };
    workbench.register_command(
        // The language server if there is one; otherwise a project-wide definition search (Symbol Search extension).
        Command::new("editor.goToDefinition", "Go to Definition", with_file(|_, _, state, window, cx| {
            // The editor action is sent to the editor itself, also when picked from the menu.
            state.update(cx, |state, cx| state.focus(window, cx));
            window.dispatch_action(Box::new(gpui_kit::component::input::GoToDefinition), cx);
        }))
        .category("Edit")
        .key("F12")
        .when(has_file.clone())
        .keys_when(editor_focused.clone()),
    );
    workbench.register_command(
        Command::new("editor.format", "Format Document", with_file(|_, buffer, state, window, cx| format_buffer(&buffer, &state, false, window, cx).detach()))
            .category("Edit")
            .key("Shift+Alt+F")
            .when(has_file.clone())
            .keys_when(editor_focused.clone()),
    );
    workbench.register_command(
        Command::new("editor.moveLineUp", "Move Line Up", with_file(|_, _, state, window, cx| move_lines(&state, true, window, cx)))
            .category("Edit")
            .key("Ctrl+Up")
            .key("Alt+Up")
            .when(has_file.clone())
            .keys_when(editor_focused.clone()),
    );
    workbench.register_command(
        Command::new("editor.moveLineDown", "Move Line Down", with_file(|_, _, state, window, cx| move_lines(&state, false, window, cx)))
            .category("Edit")
            .key("Ctrl+Down")
            .key("Alt+Down")
            .when(has_file.clone())
            .keys_when(editor_focused),
    );
    workbench.register_command(
        Command::new("editor.copyPath", "Copy Path of Active File", with_file(|_, buffer, _, _, cx| {
            let path = buffer.read(cx).path().to_string();
            cx.write_to_clipboard(ClipboardItem::new_string(path));
        }))
        .category("File")
        .when(has_file),
    );

    workbench.register_status(StatusItem::new("editor.cursor", StatusAlign::Right, 10, |env, window, cx| cx.new(|cx| EditorStatus::new(env.workbench.clone(), window, cx)).into()), window, cx);

    // File changes from outside, and auto-save when the window loses focus.
    let watcher = cx.new(|cx| ExternalChanges::new(env, buffers, window, cx));
    workbench.set_extension(watcher);
}

/// Moves the line at the cursor (or all selected lines) one line up/down.
fn move_lines(state: &Entity<EditorState>, up: bool, window: &mut Window, cx: &mut App) {
    state.update(cx, |state, cx| {
        let text = state.text().clone();
        let Some(change) = lines::move_lines(&text.to_string(), state.selected_range(), up) else { return };
        let range = lsp_types::Range { start: text.offset_to_position(change.replace.start), end: text.offset_to_position(change.replace.end) };
        state.apply_lsp_edits(&vec![lsp_types::TextEdit { range, new_text: change.text }], window, cx);
        state.set_selected_range(change.selection, cx);
    });
}

/// Reacts to file changes on disk and auto-saves when focus moves away.
struct ExternalChanges {
    _subscriptions: Vec<Subscription>,
}

impl ExternalChanges {
    fn new(env: Env, buffers: Entity<Buffers>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut subscriptions = Vec::new();
        if let Some(workbench) = env.workbench.upgrade() {
            let buffers_for_fs = buffers.clone();
            let weak_workbench = env.workbench.clone();
            subscriptions.push(cx.subscribe_in(&workbench, window, move |_, _, event: &WorkbenchEvent, window, cx| {
                let WorkbenchEvent::FsChanged { paths, overflow } = event else { return };
                let open = buffers_for_fs.read(cx).all();
                for buffer in open {
                    let path = buffer.read(cx).path().to_string();
                    if !*overflow && !paths.iter().any(|p| p == &path) {
                        continue;
                    }
                    let check = buffer.update(cx, |buffer, cx| buffer.check_external_change(window, cx));
                    let weak_workbench = weak_workbench.clone();
                    let buffer = buffer.clone();
                    window
                        .spawn(cx, async move |cx| {
                            if check.await != ExternalChange::Deleted {
                                return;
                            }
                            let _ = cx.update(|window, cx| {
                                let (dirty, name) = (buffer.read(cx).is_dirty(), buffer.read(cx).file_name().to_string());
                                if dirty {
                                    overlay::toast_warning(format!("{name} was deleted on disk. Your unsaved changes are still open; save to restore the file."), window, cx);
                                } else {
                                    let _ = weak_workbench.update(cx, |workbench, cx| {
                                        workbench.force_close_where(|tab| tab.kind == "file" && tab.param_str("path") == Some(path.as_str()), window, cx);
                                    });
                                }
                            });
                        })
                        .detach();
                }
            }));
        }
        let weak_workbench = env.workbench.clone();
        subscriptions.push(cx.observe_window_activation(window, move |_, window, cx| {
            if window.is_window_active() {
                return;
            }
            let on_focus_change = weak_workbench.upgrade().is_some_and(|w| w.read(cx).settings().files.auto_save == AutoSave::OnFocusChange);
            if !on_focus_change {
                return;
            }
            for buffer in buffers.read(cx).dirty(cx) {
                if !buffer.read(cx).has_conflict() {
                    buffer.update(cx, |buffer, cx| buffer.save(window, cx)).detach();
                }
            }
        }));
        Self { _subscriptions: subscriptions }
    }
}

/// Status bar item: cursor position, encoding, line ending, and language of the active file.
struct EditorStatus {
    workbench: WeakEntity<Workbench>,
    _subscriptions: Vec<Subscription>,
    _state_subscription: Option<Subscription>,
}

impl EditorStatus {
    fn new(workbench: WeakEntity<Workbench>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut subscriptions = Vec::new();
        if let Some(entity) = workbench.upgrade() {
            subscriptions.push(cx.subscribe_in(&entity, window, |this, workbench, event: &WorkbenchEvent, _, cx| {
                if matches!(event, WorkbenchEvent::ActiveTabChanged | WorkbenchEvent::TabsChanged) {
                    // Observe the active editor's state so the cursor position follows along.
                    this._state_subscription = active_file(workbench.read(cx), cx).map(|(_, _, state)| cx.observe(&state, |_, _, cx| cx.notify()));
                    cx.notify();
                }
            }));
        }
        Self { workbench, _subscriptions: subscriptions, _state_subscription: None }
    }
}

impl Render for EditorStatus {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some((view, buffer, _)) = self.workbench.upgrade().and_then(|w| active_file(w.read(cx), cx)) else { return div().into_any_element() };
        let (line, column, selected) = view.read(cx).cursor_info(cx);
        let buffer = buffer.read(cx);
        let position = if selected > 0 { format!("Ln {line}, Col {column} ({selected} selected)") } else { format!("Ln {line}, Col {column}") };
        h_flex()
            .gap_3()
            .text_color(cx.theme().muted_foreground)
            .child(div().child(position))
            .child(div().child(buffer.encoding().to_string()))
            .child(div().child(if buffer.eol() == Eol::Crlf { "CRLF" } else { "LF" }))
            .child(div().child(language_for_path(buffer.path()).name))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use gpui_kit::component::highlighter::{HighlightTheme, SyntaxHighlighter};
    use gpui_kit::component::input::Rope;

    /// The Dart grammar is installed in the highlighter: its code parses without errors and gets colored.
    #[test]
    fn dart_grammar_highlights_flutter_code() {
        super::register_grammars();
        let code = "import 'package:flutter/material.dart';\n\nclass App extends StatelessWidget {\n  @override\n  Widget build(BuildContext context) => const Text('Hello');\n}\n";
        let mut highlighter = SyntaxHighlighter::new("dart");
        assert_eq!(highlighter.language().as_ref(), "dart", "not a fallback to plain text");
        highlighter.update(None, &Rope::from(code), None);
        assert!(highlighter.tree().is_some_and(|tree| !tree.root_node().has_error()), "Dart code parses without errors");
        let theme = HighlightTheme::default_dark();
        let styles = highlighter.styles(&(0..code.len()), &*theme);
        assert!(styles.len() > 5, "keywords, strings, and types are colored: {}", styles.len());
    }
}

#[cfg(test)]
mod extra_language_tests {
    use gpui_kit::component::highlighter::SyntaxHighlighter;

    /// The grammars from the gpui-kit features for the "Extra languages" extension are really compiled in.
    #[test]
    fn extra_grammars_are_compiled_in() {
        for language in ["kotlin", "csharp", "php", "lua", "swift", "scala", "graphql", "proto", "make", "cmake", "zig", "elixir", "svelte"] {
            assert_eq!(SyntaxHighlighter::new(language).language().as_ref(), language, "{language}");
        }
    }
}
