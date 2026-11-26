//! Markdown Preview extension: a `markdownPreview` tab that shows a Markdown file as a rendered
//! document and updates as the file is edited (even before it is saved).
//! Can be turned off in Settings → Extensions (`extensions.markdownPreview`).

use std::time::Duration;

use gpui_kit::component::text::TextView;
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::{
    div, App, AppContext as _, Context, Entity, FocusHandle, Focusable, InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Task, WeakEntity, Window,
};
use oncode_ui::commands::Command;
use oncode_ui::editor::{active_file, buffers, language_for_path, Buffer, BufferEvent};
use oncode_ui::editor::{open_file, OpenFile};
use oncode_ui::{register_feature, register_tab_kind, EditorAction, Env, Lucide, OpenOptions, TabContent, TabData, TabKind, ViewTab, Workbench};
use serde_json::{json, Value};

/// The preview tab kind.
pub const KIND: &str = "markdownPreview";
/// Delay after the last keystroke before the preview is redrawn.
const REFRESH_DELAY: Duration = Duration::from_millis(150);

/// Registers this extension. Called once from `app/src/main.rs` after `oncode_ui::init`.
pub fn init(cx: &mut App) {
    register_tab_kind(cx, KIND, PreviewKind);
    register_feature(cx, setup);
}

/// A preview tab for a Markdown file.
pub fn preview_tab(path: &str) -> TabData {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    TabData::new(KIND, format!("Preview {name}"), json!({ "path": path })).with_id(format!("{KIND}:{path}")).with_description(path.to_string())
}

fn is_markdown(path: &str) -> bool {
    language_for_path(path).lsp == "markdown"
}

fn setup(workbench: &mut Workbench, _: &mut Window, cx: &mut Context<Workbench>) {
    let weak = cx.weak_entity();
    // Active when the extension is on and the active tab is a Markdown file.
    let markdown_active = {
        let weak = weak.clone();
        move |_: &Window, cx: &App| {
            weak.upgrade().is_some_and(|workbench| {
                let workbench = workbench.read(cx);
                workbench.settings().extensions.markdown_preview && active_file(workbench, cx).is_some_and(|(_, buffer, _)| is_markdown(buffer.read(cx).path()))
            })
        }
    };
    let open = |to_side: bool, weak: WeakEntity<Workbench>| {
        move |_: &Value, window: &mut Window, cx: &mut App| {
            let _ = weak.update(cx, |workbench, cx| {
                let Some(path) = active_file(workbench, cx).map(|(_, buffer, _)| buffer.read(cx).path().to_string()) else { return };
                workbench.open_tab(preview_tab(&path), OpenOptions { to_side, ..Default::default() }, window, cx);
            });
        }
    };
    workbench.register_command(
        Command::new("markdown.showPreviewToSide", "Open Preview to the Side", open(true, weak.clone()))
            .category("Markdown")
            .key("Mod+Shift+V")
            .when(markdown_active.clone()),
    );
    workbench.register_command(Command::new("markdown.showPreview", "Open Preview", open(false, weak.clone())).category("Markdown").when(markdown_active));

    // From the preview back to its Markdown file.
    let preview_active = {
        let weak = weak.clone();
        move |_: &Window, cx: &App| weak.upgrade().is_some_and(|workbench| workbench.read(cx).model().active().is_some_and(|(_, tab)| tab.kind == KIND))
    };
    workbench.register_command(
        Command::new("markdown.showSource", "Open Source", move |_: &Value, window: &mut Window, cx: &mut App| {
            let _ = weak.update(cx, |workbench, cx| {
                let Some(path) = workbench.model().active().filter(|(_, tab)| tab.kind == KIND).and_then(|(_, tab)| tab.param_str("path").map(String::from)) else { return };
                // A file tab that is already open in another group is activated, not opened again.
                open_file(workbench, OpenFile::new(path), window, cx);
            });
        })
        .category("Markdown")
        .when(preview_active),
    );

    // Buttons in the tab bar, like VS Code.
    workbench.register_editor_action(EditorAction::new(
        "markdown-preview",
        Lucide::Eye,
        "Open Preview to the Side (Ctrl+Shift+V)",
        "markdown.showPreviewToSide",
        |tab, settings| settings.extensions.markdown_preview && tab.kind == "file" && tab.param_str("path").is_some_and(is_markdown),
    ));
    workbench.register_editor_action(EditorAction::new("markdown-source", Lucide::FileText, "Open Source", "markdown.showSource", |tab, _| tab.kind == KIND));
}

struct PreviewKind;

impl TabKind for PreviewKind {
    fn build(&self, tab: &TabData, env: &Env, window: &mut Window, cx: &mut App) -> Box<dyn TabContent> {
        let path = tab.param_str("path").unwrap_or_default().to_string();
        let env = env.clone();
        Box::new(ViewTab(cx.new(|cx| MarkdownPreview::new(path, env, window, cx))))
    }

    fn icon(&self, _tab: &TabData) -> Option<Lucide> {
        Some(Lucide::Eye)
    }
}

/// The preview view. Holds the file's buffer, so the preview stays live even if its editor tab
/// is closed.
pub struct MarkdownPreview {
    env: Env,
    path: String,
    text: SharedString,
    buffer: Option<Entity<Buffer>>,
    focus_handle: FocusHandle,
    _refresh: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl MarkdownPreview {
    fn new(path: String, env: Env, window: &mut Window, cx: &mut Context<Self>) -> Self {
        // `Workbench` is borrowed while the tab is being built; the buffer is fetched afterwards.
        cx.defer_in(window, |this, window, cx| this.attach(window, cx));
        Self { env, path, text: SharedString::default(), buffer: None, focus_handle: cx.focus_handle(), _refresh: Task::ready(()), _subscriptions: Vec::new() }
    }

    /// The Markdown text currently shown.
    pub fn text(&self) -> &str {
        &self.text
    }

    fn attach(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(registry) = self.env.workbench.upgrade().and_then(|workbench| buffers(workbench.read(cx))) else { return };
        let path = self.path.clone();
        let buffer = registry.update(cx, |registry, cx| registry.open(&path, window, cx));
        self._subscriptions = vec![cx.subscribe_in(&buffer, window, |this, _, event: &BufferEvent, window, cx| match event {
            BufferEvent::Loaded => this.refresh(cx),
            BufferEvent::Edited => this.schedule_refresh(window, cx),
            BufferEvent::Saved => {}
        })];
        self.buffer = Some(buffer);
        self.refresh(cx);
    }

    fn schedule_refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self._refresh = cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(REFRESH_DELAY).await;
            let _ = this.update(cx, |this, cx| this.refresh(cx));
        });
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let Some(text) = self.buffer.as_ref().and_then(|buffer| buffer.read(cx).text(cx)) else { return };
        self.text = text.to_string().into();
        cx.notify();
    }
}

impl Focusable for MarkdownPreview {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for MarkdownPreview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div().id("markdown-preview").track_focus(&self.focus_handle).size_full().bg(cx.theme().background).overflow_y_scroll().child(
            div()
                .w_full()
                .max_w(gpui_kit::px(860.))
                .mx_auto()
                .px_8()
                .py_6()
                .child(TextView::markdown(SharedString::from(format!("markdown-preview-{}", self.path)), self.text.clone())),
        )
    }
}
