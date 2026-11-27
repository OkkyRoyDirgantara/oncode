//! The editor feature: the `file` tab kind (text, or the picture of an image file), saving, the status bar,
//! and reacting to file changes from outside.

mod buffer;
pub mod format;
pub mod goto_symbol;
pub mod image;
mod language;
pub mod lines;
pub mod occurrences;
pub mod symbols;
mod view;

use std::rc::Rc;

use gpui_kit::component::input::{EditorState, RopeExt as _};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{h_flex, ActiveTheme as _, Sizable as _};
use gpui_kit::{div, App, AppContext as _, ClipboardItem, Context, Entity, Focusable as _, InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, TestSupportExt as _, WeakEntity, Window};
use oncode_core::settings::AutoSave;
use oncode_workspace::Eol;
use serde_json::Value;

pub use buffer::{
    file_path, is_untitled, AnnotationColumn, AnnotationProvider, Buffer, BufferEvent, BufferStatus, Buffers, BuffersEvent, ExternalChange, LineAnnotation, LineHintProvider, Reveal,
    SignaturePopup, LARGE_FILE_BYTES, UNTITLED_PREFIX,
};
pub use format::{format_buffer, register_format_provider, FormatOutcome, EXTERNAL_PRIORITY, LSP_PRIORITY};
pub use image::{file_mode, is_image, is_svg, set_file_mode, FileMode, ImageInfo, ImageView, Zoom};
pub use goto_symbol::{register_document_symbol_provider, register_workspace_symbol_provider, DocumentSymbolProvider, SymbolItem, WorkspaceSymbolProvider};
pub use language::{comment_tokens, language_for_path, LanguageInfo};
pub use symbols::{same_path, show_document_handler, symbol_definition_links, SymbolSearchProvider};
pub use view::{FileTab, FileView};

use crate::commands::Command;
use crate::quick_pick::{quick_pick, PickItem, QuickPickOptions};
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
    let mut tab = file_tab(&options.path);
    // Going to a position (a search match, a definition) needs the text of an SVG, not its picture.
    if options.reveal.is_some() && is_svg(&options.path) {
        set_file_mode(&mut tab, FileMode::Text);
    }
    workbench.open_tab(tab, OpenOptions { preview: options.preview, to_side: options.to_side, ..Default::default() }, window, cx);
}

/// The buffer registry of this window.
pub fn buffers(workbench: &Workbench) -> Option<Entity<Buffers>> {
    workbench.extension::<Entity<Buffers>>()
}

/// Like [`active_file`], but only while the file is shown as text, not as a picture: for features that work
/// on the editor (language servers, blame, editing commands).
pub fn active_text_file(workbench: &Workbench, cx: &App) -> Option<(Entity<FileView>, Entity<Buffer>, Entity<EditorState>)> {
    active_file(workbench, cx).filter(|(view, _, _)| view.read(cx).image().is_none())
}

/// The picture in the active tab, if it shows an image file as an image.
pub fn active_image(workbench: &Workbench, cx: &App) -> Option<Entity<ImageView>> {
    let (view, _, _) = active_file(workbench, cx)?;
    view.read(cx).image().cloned()
}

/// The file in the active tab: its view, buffer, and editor state. Also for a file shown as a picture
/// (see [`active_image`]); its editor state is then not visible (see [`active_text_file`]).
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
        let as_image = file_mode(tab) == FileMode::Image;
        let view = cx.new(|cx| FileView::new(&path, env, buffers, as_image, window, cx));
        let buffer = view.read(cx).buffer().clone();
        Box::new(FileTab { view, buffer })
    }

    fn icon(&self, _tab: &TabData) -> Option<Lucide> {
        Some(Lucide::FileText)
    }

    fn image(&self, tab: &TabData) -> Option<SharedString> {
        Some(crate::file_icons::file_icon_path(tab.param_str("path").unwrap_or_default()))
    }

    /// Switching between the picture and the text of an image file builds the view again.
    fn rebuild(&self, built: &TabData, now: &TabData) -> bool {
        file_mode(built) != file_mode(now)
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
    // The file is shown as text (not as a picture): editing commands apply.
    let has_text = {
        let weak = weak.clone();
        move |_: &Window, cx: &App| weak.upgrade().is_some_and(|w| active_text_file(w.read(cx), cx).is_some())
    };
    let with_file = |f: fn(Entity<FileView>, Entity<Buffer>, Entity<EditorState>, &mut Window, &mut App)| {
        let weak = weak.clone();
        move |_: &Value, window: &mut Window, cx: &mut App| {
            let Some(active) = weak.upgrade().and_then(|w| active_file(w.read(cx), cx)) else { return };
            f(active.0, active.1, active.2, window, cx);
        }
    };

    workbench.register_command(
        Command::new("file.save", "Save", with_file(|_, buffer, _, window, cx| buffer.update(cx, |buffer, cx| buffer.save(window, cx)).detach()))
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
        .when(has_text.clone()),
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
        .when(has_text.clone()),
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
        .when(has_text.clone())
        .keys_when(editor_focused.clone()),
    );
    workbench.register_command(
        Command::new("editor.format", "Format Document", with_file(|_, buffer, state, window, cx| format_buffer(&buffer, &state, false, window, cx).detach()))
            .category("Edit")
            .key("Shift+Alt+F")
            .when(has_text.clone())
            .keys_when(editor_focused.clone()),
    );
    workbench.register_command(
        Command::new("editor.moveLineUp", "Move Line Up", with_file(|_, _, state, window, cx| move_lines(&state, true, window, cx)))
            .category("Edit")
            .key("Ctrl+Up")
            .key("Alt+Up")
            .when(has_text.clone())
            .keys_when(editor_focused.clone()),
    );
    workbench.register_command(
        Command::new("editor.moveLineDown", "Move Line Down", with_file(|_, _, state, window, cx| move_lines(&state, false, window, cx)))
            .category("Edit")
            .key("Ctrl+Down")
            .key("Alt+Down")
            .when(has_text.clone())
            .keys_when(editor_focused.clone()),
    );
    // Copy Line Down/Up use VS Code's keys: Shift+Alt+↑/↓ on Windows and macOS, Ctrl+Shift+Alt+↑/↓ on
    // Linux, where Shift+Alt+↑/↓ adds a cursor (in VS Code and in the editor component alike).
    let copy_key = |direction: &str| if cfg!(any(target_os = "windows", target_os = "macos")) { format!("Shift+Alt+{direction}") } else { format!("Ctrl+Shift+Alt+{direction}") };
    workbench.register_command(
        Command::new("editor.copyLinesDown", "Copy Line Down", with_file(|_, _, state, window, cx| change_lines(&state, window, cx, |text, selection| Some(lines::copy_lines(text, selection, true)))))
            .category("Edit")
            .key(&copy_key("Down"))
            .when(has_text.clone())
            .keys_when(editor_focused.clone()),
    );
    workbench.register_command(
        Command::new("editor.copyLinesUp", "Copy Line Up", with_file(|_, _, state, window, cx| change_lines(&state, window, cx, |text, selection| Some(lines::copy_lines(text, selection, false)))))
            .category("Edit")
            .key(&copy_key("Up"))
            .when(has_text.clone())
            .keys_when(editor_focused.clone()),
    );
    workbench.register_command(
        Command::new("editor.deleteLines", "Delete Line", with_file(|_, _, state, window, cx| change_lines(&state, window, cx, |text, selection| Some(lines::delete_lines(text, selection)))))
            .category("Edit")
            .key("Mod+Shift+K")
            .when(has_text.clone())
            .keys_when(editor_focused.clone()),
    );
    workbench.register_command(
        Command::new("editor.toggleLineComment", "Toggle Line Comment", with_file(|_, buffer, state, window, cx| {
            let tokens = language::comment_tokens(buffer.read(cx).path());
            change_lines(&state, window, cx, |text, selection| lines::toggle_line_comment(text, selection, tokens));
        }))
        .category("Edit")
        .key("Mod+/")
        .when(has_text.clone())
        .keys_when(editor_focused.clone()),
    );
    workbench.register_command(
        Command::new("editor.toggleBlockComment", "Toggle Block Comment", with_file(|_, buffer, state, window, cx| {
            let tokens = language::comment_tokens(buffer.read(cx).path());
            change_lines(&state, window, cx, |text, selection| lines::toggle_block_comment(text, selection, tokens));
        }))
        .category("Edit")
        .key("Shift+Alt+A")
        .when(has_text.clone())
        .keys_when(editor_focused.clone()),
    );
    // The component already binds these keys; the commands make them visible in the palette and menus.
    let (above_key, below_key) = if cfg!(target_os = "macos") {
        ("Cmd+Alt+Up", "Cmd+Alt+Down")
    } else if cfg!(target_os = "windows") {
        ("Ctrl+Alt+Up", "Ctrl+Alt+Down")
    } else {
        ("Shift+Alt+Up", "Shift+Alt+Down")
    };
    workbench.register_command(
        Command::new("editor.addCursorAbove", "Add Cursor Above", with_file(|_, _, state, window, cx| {
            state.update(cx, |state, cx| state.focus(window, cx));
            window.dispatch_action(Box::new(gpui_kit::base::input::AddCursorAbove), cx);
        }))
        .category("Selection")
        .key(above_key)
        .when(has_text.clone())
        .keys_when(editor_focused.clone()),
    );
    workbench.register_command(
        Command::new("editor.addCursorBelow", "Add Cursor Below", with_file(|_, _, state, window, cx| {
            state.update(cx, |state, cx| state.focus(window, cx));
            window.dispatch_action(Box::new(gpui_kit::base::input::AddCursorBelow), cx);
        }))
        .category("Selection")
        .key(below_key)
        .when(has_text.clone())
        .keys_when(editor_focused.clone()),
    );
    workbench.register_command(
        Command::new("editor.addSelectionToNextFindMatch", "Add Selection to Next Find Match", with_file(|view, _, state, _, cx| add_next_occurrence(&view, &state, cx)))
            .category("Selection")
            .key("Mod+D")
            .when(has_text.clone())
            .keys_when(editor_focused.clone()),
    );
    workbench.register_command(
        Command::new("editor.selectHighlights", "Select All Occurrences", with_file(|view, _, state, window, cx| select_all_occurrences(&view, &state, window, cx)))
            .category("Selection")
            .key("Mod+Shift+L")
            .when(has_text.clone())
            .keys_when(editor_focused),
    );
    workbench.register_command(
        Command::new("editor.triggerParameterHints", "Trigger Parameter Hints", with_file(|_, _, state, window, cx| {
            match cx.try_global::<SignatureTriggerGlobal>().map(|trigger| trigger.0.clone()) {
                Some(trigger) => trigger(&state, window, cx),
                None => overlay::toast_info("Parameter hints come from a language server.", window, cx),
            }
        }))
        .category("Edit")
        .key("Mod+Shift+Space")
        .when(has_text.clone()),
    );
    workbench.register_command(
        Command::new("editor.gotoSymbol", "Go to Symbol in Editor…", with_file(|view, buffer, state, window, cx| goto_symbol::goto_symbol_in_editor(view, buffer, state, window, cx)))
            .category("Go")
            .key("Mod+Shift+O")
            .when(has_text.clone()),
    );
    workbench.register_command(
        Command::new("workbench.gotoSymbol", "Go to Symbol in Workspace…", {
            let weak = weak.clone();
            move |_: &Value, window: &mut Window, cx: &mut App| {
                if let Some(workbench) = weak.upgrade() {
                    goto_symbol::goto_symbol_in_workspace(&workbench, window, cx);
                }
            }
        })
        .category("Go")
        .key("Mod+T")
        .when({
            let weak = weak.clone();
            move |_: &Window, cx: &App| weak.upgrade().is_some_and(|w| w.read(cx).env().root.is_some())
        }),
    );
    workbench.register_command(
        Command::new("editor.copyPath", "Copy Path of Active File", with_file(|_, buffer, _, _, cx| {
            let path = buffer.read(cx).path().to_string();
            cx.write_to_clipboard(ClipboardItem::new_string(path));
        }))
        .category("File")
        .when(has_file),
    );

    workbench.register_command(
        Command::new("editor.changeEol", "Change End of Line Sequence", with_file(|_, buffer, _, window, cx| change_eol(buffer, window, cx)))
            .category("File")
            .when(has_text.clone()),
    );
    workbench.register_command(
        Command::new("editor.changeEncoding", "Change File Encoding", with_file(|_, buffer, _, window, cx| change_encoding(buffer, window, cx)))
            .category("File")
            .when(has_text.clone()),
    );

    image::register_commands(workbench, cx);

    workbench.register_status(StatusItem::new("editor.cursor", StatusAlign::Right, 10, |env, window, cx| cx.new(|cx| EditorStatus::new(env.workbench.clone(), window, cx)).into()), window, cx);

    // File changes from outside, and auto-save when the window loses focus.
    let watcher = cx.new(|cx| ExternalChanges::new(env, buffers, window, cx));
    workbench.set_extension(watcher);
}

/// Encodings offered by Change File Encoding: label, WHATWG name, and whether a BOM is written.
const ENCODINGS: &[(&str, &str, bool)] = &[
    ("UTF-8", "UTF-8", false),
    ("UTF-8 with BOM", "UTF-8", true),
    ("UTF-16 LE", "UTF-16LE", true),
    ("UTF-16 BE", "UTF-16BE", true),
    ("Western (Windows 1252)", "windows-1252", false),
    ("Central European (Windows 1250)", "windows-1250", false),
    ("Cyrillic (Windows 1251)", "windows-1251", false),
    ("Greek (Windows 1253)", "windows-1253", false),
    ("Turkish (Windows 1254)", "windows-1254", false),
    ("Japanese (Shift JIS)", "Shift_JIS", false),
    ("Japanese (EUC-JP)", "EUC-JP", false),
    ("Korean (EUC-KR)", "EUC-KR", false),
    ("Simplified Chinese (GBK)", "GBK", false),
    ("Traditional Chinese (Big5)", "Big5", false),
];

/// Change End of Line Sequence: LF or CRLF from the next save on (the buffer becomes unsaved).
fn change_eol(buffer: Entity<Buffer>, window: &mut Window, cx: &mut App) {
    let current = if buffer.read(cx).eol() == Eol::Crlf { "crlf" } else { "lf" };
    let items = vec![PickItem::new("LF", "lf").description("Unix, Linux, macOS"), PickItem::new("CRLF", "crlf").description("Windows")];
    quick_pick(
        QuickPickOptions::new("Select end of line sequence", items).selected(current),
        move |item, _, cx| {
            let Some(item) = item else { return };
            let eol = if item.value == "crlf" { Eol::Crlf } else { Eol::Lf };
            buffer.update(cx, |buffer, cx| buffer.set_eol(eol, cx));
        },
        window,
        cx,
    );
}

/// Change File Encoding: reopen the file decoded with another encoding, or save it with one.
fn change_encoding(buffer: Entity<Buffer>, window: &mut Window, cx: &mut App) {
    if buffer.read(cx).is_untitled() {
        overlay::toast_info("Save the file first; new files are saved as UTF-8 (or as .editorconfig says).", window, cx);
        return;
    }
    let actions = vec![PickItem::new("Reopen with Encoding", "reopen").description("Read the file again as another encoding"), PickItem::new("Save with Encoding", "save").description("Write the file in another encoding")];
    quick_pick(
        QuickPickOptions::new("Select action", actions),
        move |action, window, cx| {
            let Some(action) = action else { return };
            let reopen = action.value == "reopen";
            if reopen && buffer.read(cx).is_dirty() {
                overlay::toast_info("Save or undo your changes first: reopening reads the file from disk again.", window, cx);
                return;
            }
            let current = (buffer.read(cx).encoding().to_string(), buffer.read(cx).has_bom());
            let items = ENCODINGS
                .iter()
                .enumerate()
                .map(|(index, (label, name, bom))| {
                    let item = PickItem::new(*label, index.to_string()).description(*name);
                    if name.eq_ignore_ascii_case(&current.0) && (*bom == current.1 || !name.starts_with("UTF-8")) { item.hint("current") } else { item }
                })
                .collect();
            let buffer = buffer.clone();
            quick_pick(
                QuickPickOptions::new(if reopen { "Reopen with encoding" } else { "Save with encoding" }, items),
                move |item, window, cx| {
                    let Some((_, name, bom)) = item.and_then(|item| item.value.parse::<usize>().ok()).and_then(|index| ENCODINGS.get(index)) else { return };
                    buffer.update(cx, |buffer, cx| {
                        if reopen {
                            buffer.reopen_with_encoding(name.to_string(), window, cx);
                        } else {
                            buffer.set_encoding(name.to_string(), *bom, cx);
                            buffer.save(window, cx).detach();
                        }
                    });
                },
                window,
                cx,
            );
        },
        window,
        cx,
    );
}

/// Moves the line at the cursor (or all selected lines) one line up/down.
fn move_lines(state: &Entity<EditorState>, up: bool, window: &mut Window, cx: &mut App) {
    change_lines(state, window, cx, |text, selection| lines::move_lines(text, selection, up));
}

/// Applies a change computed by one of the pure functions in [`lines`] from the text and the (first)
/// selection: one edit, so one undo step, then the new selection.
fn change_lines(state: &Entity<EditorState>, window: &mut Window, cx: &mut App, change: impl FnOnce(&str, std::ops::Range<usize>) -> Option<lines::LineMove>) {
    state.update(cx, |state, cx| {
        let text = state.text().clone();
        let Some(change) = change(&text.to_string(), state.selected_range()) else { return };
        let range = lsp_types::Range { start: text.offset_to_position(change.replace.start), end: text.offset_to_position(change.replace.end) };
        state.apply_lsp_edits(&vec![lsp_types::TextEdit { range, new_text: change.text }], window, cx);
        state.set_selected_range(change.selection, cx);
    });
}

/// Shows signature help on request (Trigger Parameter Hints); installed by the language server feature.
pub type SignatureTrigger = Rc<dyn Fn(&Entity<EditorState>, &mut Window, &mut App)>;

struct SignatureTriggerGlobal(SignatureTrigger);

impl gpui_kit::Global for SignatureTriggerGlobal {}

/// Installs the handler for Trigger Parameter Hints (Ctrl+Shift+Space).
pub fn set_signature_trigger(trigger: impl Fn(&Entity<EditorState>, &mut Window, &mut App) + 'static, cx: &mut App) {
    cx.set_global(SignatureTriggerGlobal(Rc::new(trigger)));
}

/// Path relative to the project folder; a path outside the folder is returned as is.
pub(crate) fn relative_to(root: Option<&str>, path: &str) -> String {
    let Some(root) = root.map(|root| root.trim_end_matches(['/', '\\'])).filter(|root| !root.is_empty()) else { return path.to_string() };
    match path.strip_prefix(root) {
        Some(rest) if rest.starts_with(['/', '\\']) => rest[1..].replace('\\', "/"),
        _ => path.to_string(),
    }
}

/// Most selections Select All Occurrences creates.
const MAX_OCCURRENCES: usize = 10_000;

/// What Ctrl+D or Ctrl+Shift+L searches for, from the active selection: the selected text, or the word
/// under the cursor (whole words only). `None` when the cursor is not on a word.
fn occurrence_needle(text: &str, selection: &std::ops::Range<usize>) -> Option<(occurrences::Needle, std::ops::Range<usize>)> {
    if selection.is_empty() {
        let word = occurrences::word_at(text, selection.start)?;
        Some((occurrences::Needle { text: text[word.clone()].to_string(), whole_word: true }, word))
    } else {
        Some((occurrences::Needle { text: text.get(selection.clone())?.to_string(), whole_word: false }, selection.clone()))
    }
}

/// Add Selection to Next Find Match (Ctrl+D): the first press on a cursor selects the word under it; each
/// later press adds the next occurrence as another selection, wrapping around the end of the file.
fn add_next_occurrence(view: &Entity<FileView>, state: &Entity<EditorState>, cx: &mut App) {
    let text = state.read(cx).text().to_string();
    let selection = state.read(cx).selected_range();
    let tracker = view.read(cx).occurrences.clone().filter(|tracker| tracker.first == selection && text.get(tracker.last.clone()) == Some(tracker.needle.text.as_str()));
    let tracker = match tracker {
        Some(tracker) => tracker,
        None => {
            let Some((needle, first)) = occurrence_needle(&text, &selection) else { return };
            let tracker = occurrences::Tracker { needle, first: first.clone(), last: first.clone() };
            if selection.is_empty() {
                // The first press only selects the word.
                state.update(cx, |state, cx| state.set_selected_range(first.clone(), cx));
                view.update(cx, |view, _| view.occurrences = Some(tracker));
                return;
            }
            tracker
        }
    };
    let Some(next) = occurrences::find_next(&text, &tracker.needle, tracker.last.end, tracker.first.start) else { return };
    state.update(cx, |state, cx| state.add_selection(next.clone(), cx));
    view.update(cx, |view, _| view.occurrences = Some(occurrences::Tracker { last: next, ..tracker }));
}

/// Select All Occurrences (Ctrl+Shift+L): every occurrence of the selection, or of the word under the
/// cursor, becomes a selection. The one at the cursor stays the active selection.
fn select_all_occurrences(view: &Entity<FileView>, state: &Entity<EditorState>, window: &mut Window, cx: &mut App) {
    let text = state.read(cx).text().to_string();
    let selection = state.read(cx).selected_range();
    let Some((needle, current)) = occurrence_needle(&text, &selection) else { return };
    let all = occurrences::find_all(&text, &needle, MAX_OCCURRENCES);
    if all.len() == MAX_OCCURRENCES {
        overlay::toast_info(format!("Only the first {MAX_OCCURRENCES} occurrences were selected."), window, cx);
    }
    state.update(cx, |state, cx| {
        state.set_selected_range(current.clone(), cx);
        for range in all.into_iter().filter(|range| *range != current) {
            state.add_selection(range, cx);
        }
    });
    view.update(cx, |view, _| view.occurrences = None);
}

/// Reacts to file changes on disk and auto-saves when focus moves away.
struct ExternalChanges {
    _subscriptions: Vec<Subscription>,
}

impl ExternalChanges {
    fn new(env: Env, buffers: Entity<Buffers>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut subscriptions = Vec::new();
        if let Some(workbench) = env.workbench.upgrade() {
            let buffers = buffers.clone();
            let weak_workbench = env.workbench.clone();
            subscriptions.push(cx.subscribe_in(&workbench, window, move |_, _, event: &WorkbenchEvent, window, cx| {
                let WorkbenchEvent::FsChanged { paths, overflow } = event else { return };
                // An `.editorconfig` changed: every open file reads its properties again.
                if *overflow || paths.iter().any(|path| path.ends_with(".editorconfig")) {
                    for buffer in buffers.read(cx).all() {
                        buffer.update(cx, |buffer, cx| buffer.reload_editorconfig(cx));
                    }
                }
                let changed = buffers.read(cx).all().into_iter().filter(|buffer| *overflow || paths.iter().any(|p| p == buffer.read(cx).path())).collect();
                check_buffers(changed, &weak_workbench, window, cx);
            }));
        }
        let weak_workbench = env.workbench.clone();
        subscriptions.push(cx.observe_window_activation(window, move |_, window, cx| {
            if window.is_window_active() {
                // The watcher does not report gitignored files (`.env`, build output), and it can miss
                // changes while the computer sleeps: coming back to the window checks every open file.
                check_buffers(buffers.read(cx).all(), &weak_workbench, window, cx);
                return;
            }
            let on_focus_change = weak_workbench.upgrade().is_some_and(|w| w.read(cx).settings().files.auto_save == AutoSave::OnFocusChange);
            if !on_focus_change {
                return;
            }
            for buffer in buffers.read(cx).dirty(cx) {
                if !buffer.read(cx).has_conflict() {
                    buffer.update(cx, |buffer, cx| buffer.auto_save(window, cx)).detach();
                }
            }
        }));
        Self { _subscriptions: subscriptions }
    }
}

/// Compares open files with the disk: a clean buffer is reloaded, a dirty one is marked as conflicted,
/// and the tab of a clean file that was deleted is closed.
fn check_buffers(buffers: Vec<Entity<Buffer>>, workbench: &WeakEntity<Workbench>, window: &mut Window, cx: &mut App) {
    for buffer in buffers {
        let path = buffer.read(cx).path().to_string();
        let check = buffer.update(cx, |buffer, cx| buffer.check_external_change(window, cx));
        let workbench = workbench.clone();
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
                        let _ = workbench.update(cx, |workbench, cx| {
                            workbench.force_close_where(|tab| tab.kind == "file" && tab.param_str("path") == Some(path.as_str()), window, cx);
                        });
                    }
                });
            })
            .detach();
    }
}

/// Status bar item: cursor position, encoding, line ending, and language of the active file; for a picture,
/// its size in pixels, file size, and zoom.
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
                    // Observe the active editor's state (or picture) so the cursor position (or zoom) follows along.
                    this._state_subscription = active_file(workbench.read(cx), cx).map(|(view, _, state)| match view.read(cx).image().cloned() {
                        Some(image) => cx.observe(&image, |_, _, cx| cx.notify()),
                        None => cx.observe(&state, |_, _, cx| cx.notify()),
                    });
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
        if let Some(image) = view.read(cx).image().cloned() {
            return image_status(&image, cx);
        }
        let (line, column, selected) = view.read(cx).cursor_info(cx);
        let buffer = buffer.read(cx);
        let position = if selected > 0 { format!("Ln {line}, Col {column} ({selected} selected)") } else { format!("Ln {line}, Col {column}") };
        h_flex()
            .gap_3()
            .text_color(cx.theme().muted_foreground)
            .child(div().child(position))
            .child(
                div()
                    .id("status-encoding")
                    .test_support()
                    .cursor_pointer()
                    .child(if buffer.has_bom() { format!("{} with BOM", buffer.encoding()) } else { buffer.encoding().to_string() })
                    .on_click(|_, window, cx| crate::commands::execute("editor.changeEncoding", Value::Null, window, cx)),
            )
            .child(
                div()
                    .id("status-eol")
                    .test_support()
                    .cursor_pointer()
                    .child(if buffer.eol() == Eol::Crlf { "CRLF" } else { "LF" })
                    .on_click(|_, window, cx| crate::commands::execute("editor.changeEol", Value::Null, window, cx)),
            )
            .child(div().child(language_for_path(buffer.path()).name))
            .into_any_element()
    }
}

/// The status of a picture: "1920 × 1080", "245.3 KB", and its zoom, which switches between fitting the view and the actual size.
fn image_status(image: &Entity<ImageView>, cx: &mut Context<EditorStatus>) -> gpui_kit::AnyElement {
    let picture = image.read(cx);
    let Some(info) = picture.info() else { return div().into_any_element() };
    let zoom = picture.scale().map(|scale| format!("{:.0}%", scale * 100.)).unwrap_or_default();
    let tooltip = if picture.zoom() == Zoom::Fit { "Show at Actual Size" } else { "Zoom to Fit" };
    let image = image.clone();
    h_flex()
        .gap_3()
        .text_color(cx.theme().muted_foreground)
        .child(div().child(format!("{} × {}", info.width, info.height)))
        .child(div().child(view::format_size(u32::try_from(info.bytes).unwrap_or(u32::MAX))))
        .child(div().child(info.format))
        .child(Button::new("image-zoom").ghost().xsmall().label(zoom).tooltip(tooltip).on_click(move |_, window, cx| {
            image.update(cx, |image, cx| image.toggle_actual_size(window, cx));
        }))
        .into_any_element()
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
