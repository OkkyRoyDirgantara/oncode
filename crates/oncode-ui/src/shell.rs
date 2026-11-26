//! The content of a window: holds the active `Workbench` and replaces it when a folder
//! is opened, closed, or its trust status changes. Also bridges `oncode-core` events
//! (settings, notices, git credential requests, file changes) to this window.

use std::path::PathBuf;
use std::time::Duration;

use gpui_kit::component::input::{Editor, EditorState};
use gpui_kit::component::{v_flex, Theme, ThemeMode, TitleBar, WindowExt as _};
use gpui_kit::{
    div, px, App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, PathPromptOptions, Render, SharedString, Styled as _, Subscription, Task,
    WeakEntity, Window,
};
use oncode_core::api::app::{self, WorkspaceInfo};
use oncode_core::settings::Theme as ThemeSetting;
use oncode_core::{CoreEvent, NoticeKind, PromptKind, PromptRequest, Settings, Shared};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::commands::{self, Command};
use crate::model::{OpenOptions, TabData};
use crate::overlay::{self, Confirm, TextPrompt};
use crate::quick_pick::{quick_pick, PickItem, QuickPickOptions};
use crate::workbench::{Workbench, WorkbenchEvent};
use crate::{core_bus, keys, spawn_core};

/// The per-project session: open file tabs are restored when the folder is opened again.
/// Same format as version 0.2.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionData {
    /// Absolute paths of the open files, in tab order.
    #[serde(default)]
    pub files: Vec<String>,
    #[serde(default)]
    pub active: Option<String>,
}

impl SessionData {
    pub fn parse(json: Option<&str>) -> Self {
        let mut data: SessionData = json.and_then(|j| serde_json::from_str(j).ok()).unwrap_or_default();
        data.files.truncate(50);
        if data.active.as_ref().is_some_and(|a| !data.files.contains(a)) {
            data.active = None;
        }
        data
    }

    /// Permanent (non-preview) file tabs from all groups.
    pub fn collect(model: &crate::model::Model) -> Self {
        let mut files: Vec<String> = Vec::new();
        for tab in model.all_tabs() {
            if tab.kind == "file" && !tab.preview {
                // A new file that has not been saved cannot be reopened from disk.
                if let Some(path) = tab.param_str("path").filter(|path| !crate::editor::is_untitled(path)) {
                    if !files.iter().any(|f| f == path) {
                        files.push(path.to_string());
                    }
                }
            }
        }
        let active = model.active().filter(|(_, t)| t.kind == "file").and_then(|(_, t)| t.param_str("path")).map(String::from);
        Self { files, active }
    }
}

/// A file tab for an absolute path, or a new file (`untitled:Untitled-1`).
pub fn file_tab(path: &str) -> TabData {
    let path = &crate::editor::file_path(path);
    if let Some(name) = path.strip_prefix(crate::editor::UNTITLED_PREFIX) {
        return TabData::new("file", name.to_string(), json!({ "path": path })).with_id(format!("file:{path}")).with_description("New file, not saved yet".to_string());
    }
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path).to_string();
    TabData::new("file", name, json!({ "path": path })).with_id(format!("file:{path}")).with_description(path.to_string())
}

pub struct Shell {
    core: Shared,
    label: SharedString,
    workbench: Entity<Workbench>,
    workspace: Option<WorkspaceInfo>,
    /// The session is not saved while the folder is switching or being restored.
    session_frozen: bool,
    last_session: String,
    prompts: Vec<PromptRequest>,
    prompt_open: bool,
    _save_session: Task<()>,
    _workbench_subscription: Option<Subscription>,
    _subscriptions: Vec<Subscription>,
}

#[derive(Default)]
struct Shells(std::collections::HashMap<gpui_kit::WindowId, WeakEntity<Shell>>);

impl gpui_kit::Global for Shells {}

impl Shell {
    pub fn new(folder: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let core = crate::core(cx);
        let label: SharedString = format!("window-{}", window.window_handle().window_id().as_u64()).into();
        let settings = core.user_settings();
        apply_theme(&settings, window, cx);
        let workbench = Self::build_workbench(&core, &label, None, settings, window, cx);

        let weak = cx.weak_entity();
        cx.default_global::<Shells>().0.insert(window.window_handle().window_id(), weak);

        let bus = core_bus(cx);
        let subscriptions = vec![
            cx.subscribe_in(&bus, window, Self::on_core_event),
            // The macOS menu bar follows the active window; redraw so `app_menu::sync` runs.
            cx.observe_window_activation(window, |_, _, cx| cx.notify()),
        ];
        window.on_window_should_close(cx, {
            let weak = cx.weak_entity();
            move |window, cx| {
                let Some(shell) = weak.upgrade() else { return true };
                shell.update(cx, |this, cx| this.should_close(window, cx))
            }
        });

        let mut this = Self {
            core,
            label,
            workbench,
            workspace: None,
            session_frozen: true,
            last_session: String::new(),
            prompts: Vec::new(),
            prompt_open: false,
            _save_session: Task::ready(()),
            _workbench_subscription: None,
            _subscriptions: subscriptions,
        };
        this.watch_workbench(window, cx);
        this.update_title(window);
        if let Some(folder) = folder {
            this.open_folder(folder, window, cx);
        }
        this
    }

    pub fn for_window(window: &Window, cx: &App) -> Option<Entity<Shell>> {
        cx.try_global::<Shells>()?.0.get(&window.window_handle().window_id())?.upgrade()
    }

    pub fn workbench(&self) -> &Entity<Workbench> {
        &self.workbench
    }

    pub fn workspace(&self) -> Option<&WorkspaceInfo> {
        self.workspace.as_ref()
    }

    fn build_workbench(core: &Shared, label: &SharedString, workspace: Option<&WorkspaceInfo>, settings: Settings, window: &mut Window, cx: &mut Context<Self>) -> Entity<Workbench> {
        let workbench = cx.new(|cx| Workbench::new(core.clone(), label.clone(), workspace, settings, window, cx));
        workbench.update(cx, |workbench, cx| register_shell_commands(workbench, cx));
        workbench
    }

    fn watch_workbench(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self._workbench_subscription = Some(cx.subscribe_in(&self.workbench, window, |this, _, event: &WorkbenchEvent, window, cx| match event {
            WorkbenchEvent::TabsChanged | WorkbenchEvent::ActiveTabChanged => this.schedule_session_save(window, cx),
            WorkbenchEvent::FileSaved(path) => {
                if path.ends_with("keybindings.json") && PathBuf::from(path) == this.core.keybindings_path() {
                    this.load_keybindings(true, window, cx);
                }
            }
            _ => {}
        }));
    }

    fn update_title(&self, window: &mut Window) {
        let title = match &self.workspace {
            Some(workspace) => format!("{} — OnCode", workspace.name),
            None => "OnCode".to_string(),
        };
        window.set_window_title(&title);
    }

    /// Installs a new `Workbench` for this workspace state, then restores the session.
    fn install(&mut self, workspace: Option<WorkspaceInfo>, window: &mut Window, cx: &mut Context<Self>) {
        self.session_frozen = true;
        let settings = workspace.as_ref().map(|w| w.settings.clone()).unwrap_or_else(|| self.core.user_settings());
        apply_theme(&settings, window, cx);
        self.workbench = Self::build_workbench(&self.core, &self.label, workspace.as_ref(), settings, window, cx);
        self.workspace = workspace;
        self.watch_workbench(window, cx);
        self.update_title(window);
        self.restore_session(window, cx);
        cx.notify();
    }

    // ----- Folders -----

    pub fn pick_and_open_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions { files: false, directories: true, multiple: false, prompt: Some("Open Folder".into()) });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else { return };
            let Some(path) = paths.into_iter().next() else { return };
            let _ = this.update_in(cx, |this, window, cx| this.open_folder(path, window, cx));
        })
        .detach();
    }

    /// Closes all tabs (with confirmation for unsaved ones), then opens the folder.
    pub fn open_folder(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.save_session_now(cx);
        self.session_frozen = true;
        let closed = self.workbench.update(cx, |workbench, cx| workbench.close_all_tabs(false, window, cx));
        let (core, label) = (self.core.clone(), self.label.to_string());
        cx.spawn_in(window, async move |this, cx| {
            if !closed.await {
                let _ = this.update(cx, |this, _| this.session_frozen = false);
                return;
            }
            let Ok(task) = cx.update(|_, cx| spawn_core(cx, async move { app::workspace_open(&core, &label, path.to_string_lossy().into_owned()).await })) else { return };
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| match result {
                Ok(info) => this.install(Some(info), window, cx),
                Err(error) => {
                    this.session_frozen = false;
                    overlay::show_error(&error, window, cx);
                }
            });
        })
        .detach();
    }

    pub fn close_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.workspace.is_none() {
            return;
        }
        self.save_session_now(cx);
        self.session_frozen = true;
        let closed = self.workbench.update(cx, |workbench, cx| workbench.close_all_tabs(false, window, cx));
        let (core, label) = (self.core.clone(), self.label.to_string());
        cx.spawn_in(window, async move |this, cx| {
            if !closed.await {
                let _ = this.update(cx, |this, _| this.session_frozen = false);
                return;
            }
            let Ok(task) = cx.update(|_, cx| spawn_core(cx, async move { app::workspace_close(&core, &label).await })) else { return };
            let _ = task.await;
            let _ = this.update_in(cx, |this, window, cx| this.install(None, window, cx));
        })
        .detach();
    }

    /// Changes the folder's trust status. Open tabs are preserved through the session.
    pub fn set_trusted(&mut self, trusted: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.workspace.is_none() {
            return;
        }
        self.save_session_now(cx);
        let (core, label) = (self.core.clone(), self.label.to_string());
        let task = spawn_core(cx, async move { app::workspace_set_trusted(&core, &label, trusted).await });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| match result {
                Ok(info) => this.install(Some(info), window, cx),
                Err(error) => overlay::show_error(&error, window, cx),
            });
        })
        .detach();
    }

    fn should_close(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(closed) = self.confirm_close(window, cx) else {
            self.release_workspace(cx);
            return true;
        };
        cx.spawn_in(window, async move |this, cx| {
            if closed.await {
                let _ = this.update_in(cx, |this, window, cx| {
                    this.release_workspace(cx);
                    window.remove_window();
                });
            }
        })
        .detach();
        false
    }

    /// Saves the session. `None` if the window may be closed right away; otherwise all tabs are closed
    /// with a question for unsaved tabs, and the result is `true` if the user did not cancel.
    fn confirm_close(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<Task<bool>> {
        self.save_session_now(cx);
        if !self.workbench.read(cx).has_dirty_tabs(cx) {
            return None;
        }
        Some(self.workbench.update(cx, |workbench, cx| workbench.close_all_tabs(false, window, cx)))
    }

    fn release_workspace(&self, cx: &mut Context<Self>) {
        let (core, label) = (self.core.clone(), self.label.to_string());
        spawn_core(cx, async move { app::workspace_close(&core, &label).await }).detach();
    }

    // ----- Session -----

    fn schedule_session_save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.session_frozen || self.workspace.is_none() {
            return;
        }
        self._save_session = cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(500)).await;
            let _ = this.update(cx, |this, cx| this.save_session_now(cx));
        });
    }

    fn save_session_now(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.workspace.as_ref().map(|w| w.root.clone()) else { return };
        if self.session_frozen {
            return;
        }
        let Ok(json) = serde_json::to_string(&SessionData::collect(self.workbench.read(cx).model())) else { return };
        if json == self.last_session {
            return;
        }
        self.last_session = json.clone();
        let core = self.core.clone();
        spawn_core(cx, async move { app::session_set(&core, root, json).await }).detach();
    }

    fn restore_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(root) = self.workspace.as_ref().map(|w| w.root.clone()) else {
            self.session_frozen = false;
            return;
        };
        let core = self.core.clone();
        let expected = root.clone();
        let task = spawn_core(cx, async move {
            let session = SessionData::parse(app::session_get(&core, root).await.ok().flatten().as_deref());
            // Files that no longer exist are skipped.
            let files: Vec<String> = session.files.iter().filter(|f| std::path::Path::new(f).is_file()).cloned().collect();
            SessionData { active: session.active.filter(|a| files.contains(a)), files }
        });
        cx.spawn_in(window, async move |this, cx| {
            let session = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.workspace.as_ref().map(|w| &w.root) != Some(&expected) {
                    return;
                }
                this.last_session = serde_json::to_string(&session).unwrap_or_default();
                this.workbench.update(cx, |workbench, cx| {
                    for path in &session.files {
                        workbench.open_tab(file_tab(path), OpenOptions { background: true, ..Default::default() }, window, cx);
                    }
                    if let Some(active) = &session.active {
                        workbench.open_tab(file_tab(active), OpenOptions::default(), window, cx);
                    }
                });
                this.session_frozen = false;
            });
        })
        .detach();
    }

    // ----- Events from the core -----

    fn on_core_event(&mut self, _: &Entity<crate::CoreBus>, event: &CoreEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            CoreEvent::SettingsChanged { .. } => {
                let settings = self.core.effective_settings(&self.label);
                apply_theme(&settings, window, cx);
                self.workbench.update(cx, |workbench, cx| workbench.set_settings(settings, cx));
            }
            CoreEvent::FsChanged { window: target, paths, overflow } if target.as_str() == self.label.as_ref() => {
                let event = WorkbenchEvent::FsChanged { paths: paths.clone(), overflow: *overflow };
                self.workbench.update(cx, |workbench, cx| workbench.broadcast(event, cx));
            }
            CoreEvent::Notice { kind, message } => {
                if window.is_window_active() {
                    match kind {
                        NoticeKind::Warning => overlay::toast_warning(message.clone(), window, cx),
                        NoticeKind::Error => overlay::toast_error(message.clone(), window, cx),
                    }
                }
            }
            CoreEvent::Prompt { window: target, request } => {
                let mine = match target {
                    Some(target) => target.as_str() == self.label.as_ref(),
                    None => window.is_window_active(),
                };
                if mine {
                    self.prompts.push(request.clone());
                    self.next_prompt(window, cx);
                }
            }
            _ => {}
        }
    }

    /// Shows the next git request (credentials, host confirmation, commit message), one at a time.
    fn next_prompt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.prompt_open || self.prompts.is_empty() {
            return;
        }
        let request = self.prompts.remove(0);
        self.prompt_open = true;
        let answer: Task<Option<String>> = match request.kind {
            PromptKind::Text => overlay::prompt_text(TextPrompt::new("Git needs your input", "OK").message(request.prompt.trim().to_string()), window, cx),
            PromptKind::Secret => overlay::prompt_text(TextPrompt::new("Git needs your input", "OK").message(request.prompt.trim().to_string()).secret(), window, cx),
            PromptKind::Confirm => {
                let confirmed = overlay::confirm(Confirm::new("Confirm", request.prompt.trim().to_string(), "Yes"), window, cx);
                cx.spawn(async move |_, _| Some(if confirmed.await { "yes".to_string() } else { "no".to_string() }))
            }
            PromptKind::Message => prompt_message(request.initial.clone(), window, cx),
        };
        let (core, id) = (self.core.clone(), request.id.clone());
        cx.spawn_in(window, async move |this, cx| {
            let value = answer.await;
            if let Ok(task) = cx.update(|_, cx| spawn_core(cx, async move { app::prompt_respond(&core, id, value).await })) {
                let _ = task.await;
            }
            let _ = this.update_in(cx, |this, window, cx| {
                this.prompt_open = false;
                this.next_prompt(window, cx);
            });
        })
        .detach();
    }

    // ----- Keyboard shortcuts -----

    /// Reads `keybindings.json` and applies it to all windows.
    pub fn load_keybindings(&mut self, notify: bool, window: &mut Window, cx: &mut Context<Self>) {
        let core = self.core.clone();
        let task = spawn_core(cx, async move { app::keybindings_get(&core).await.unwrap_or_else(|_| "{}".into()) });
        cx.spawn_in(window, async move |_, cx| {
            let text = task.await;
            let _ = cx.update(|window, cx| match keys::parse_keybindings(&text) {
                Ok(overrides) => {
                    commands::set_keybinding_overrides(overrides, cx);
                    if notify {
                        overlay::toast_success("Keyboard shortcuts reloaded.", window, cx);
                    }
                }
                Err(error) => overlay::toast_error(format!("keybindings.json could not be read: {error}"), window, cx),
            });
        })
        .detach();
    }
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The title bar is drawn by us: on Wayland (GNOME) there are no server-side decorations, and
        // it looks the same on all three OSes. The close button goes through the unsaved tabs check.
        // The File, Edit, etc. menus are in the title bar, except on macOS: there it is the system menu bar.
        // The folder's name is in the search box in the middle (and in the OS window title).
        let menu_in_title_bar = !cfg!(target_os = "macos");
        if !menu_in_title_bar {
            crate::app_menu::sync(window, cx);
        }
        let menus = menu_in_title_bar.then(|| crate::menubar::menu_bar(window, cx).into_any_element());
        let folder = self.workspace.as_ref().map(|workspace| workspace.name.clone());
        let weak = cx.weak_entity();
        v_flex()
            .size_full()
            .child(
                TitleBar::new()
                    .on_close_window(move |_, window, cx| {
                        let allowed = weak.update(cx, |this, cx| this.should_close(window, cx)).unwrap_or(true);
                        if allowed {
                            window.remove_window();
                        }
                    })
                    .child(crate::command_center::title_bar_content(menus, folder.as_deref(), window, cx)),
            )
            .child(div().flex_1().min_h_0().w_full().child(self.workbench.clone()))
    }
}

/// Closes all windows like the close button (asking if there are unsaved tabs), then quits.
/// Canceled, with the remaining windows left open, if the user cancels one of the questions.
pub(crate) fn quit(cx: &mut App) {
    let windows = cx.windows();
    cx.spawn(async move |cx| {
        for handle in windows {
            let closing = handle.update(cx, |_, window, cx| {
                let shell = cx.try_global::<Shells>()?.0.get(&window.window_handle().window_id())?.upgrade()?;
                window.activate_window();
                Some(shell.update(cx, |shell, cx| (shell.confirm_close(window, cx), cx.weak_entity())))
            });
            let Ok(Some((confirm, shell))) = closing else { continue };
            if let Some(confirm) = confirm {
                if !confirm.await {
                    return;
                }
            }
            let _ = shell.update(cx, |shell, cx| shell.release_workspace(cx));
        }
        cx.update(|cx| cx.quit());
    })
    .detach();
}

fn apply_theme(settings: &Settings, window: &mut Window, cx: &mut App) {
    match settings.workbench.theme {
        ThemeSetting::System => Theme::sync_system_appearance(Some(window), cx),
        ThemeSetting::Dark => Theme::change(ThemeMode::Dark, Some(window), cx),
        ThemeSetting::Light => Theme::change(ThemeMode::Light, Some(window), cx),
    }
}

/// A multi-line message dialog (the commit message during rebase, merge, and the like). `None` cancels the git operation.
fn prompt_message(initial: String, window: &mut Window, cx: &mut App) -> Task<Option<String>> {
    let (tx, rx) = tokio::sync::oneshot::channel::<Option<String>>();
    let tx = std::rc::Rc::new(std::cell::Cell::new(Some(tx)));
    let editor = cx.new(|cx| EditorState::new(window, cx).line_number(false).soft_wrap(true).default_value(initial));
    let focus = editor.clone();
    window.open_alert_dialog(cx, move |dialog, _, _| {
        let (ok_tx, cancel_tx) = (tx.clone(), tx.clone());
        let value = editor.clone();
        dialog
            .title("Commit message")
            .description("Lines starting with # are ignored. Cancel aborts the operation.")
            .width(px(640.))
            .child(Editor::new(&editor).h(px(260.)))
            .confirm()
            .ok_text("Continue")
            .on_ok(move |_, _, cx| {
                if let Some(tx) = ok_tx.take() {
                    let _ = tx.send(Some(value.read(cx).value().to_string()));
                }
                true
            })
            .on_cancel(move |_, _, _| {
                if let Some(tx) = cancel_tx.take() {
                    let _ = tx.send(None);
                }
                true
            })
    });
    window.defer(cx, move |window, cx| focus.update(cx, |state, cx| state.focus(window, cx)));
    cx.spawn(async move |_| rx.await.ok().flatten())
}

/// The window's own commands: palette, quick open, folders, new window, keyboard shortcuts.
fn register_shell_commands(workbench: &mut Workbench, cx: &mut Context<Workbench>) {
    let has_workspace = workbench.env().root.is_some();
    let core = workbench.env().core.clone();
    let label = workbench.env().window.clone();
    let weak = cx.weak_entity();

    fn with_shell(f: impl Fn(&mut Shell, &mut Window, &mut Context<Shell>) + 'static) -> impl Fn(&Value, &mut Window, &mut App) + 'static {
        move |_, window, cx| {
            if let Some(shell) = Shell::for_window(window, cx) {
                shell.update(cx, |shell, cx| f(shell, window, cx));
            }
        }
    }

    workbench.register_command(Command::new("workbench.commandPalette", "Show All Commands", |_, window, cx| show_command_palette(window, cx)).category("View").key("Mod+Shift+P").key("F1").hidden());
    workbench.register_command(
        Command::new("workbench.quickOpen", "Go to File…", {
            let (core, label, weak) = (core.clone(), label.clone(), weak.clone());
            move |_, window, cx| show_quick_open(core.clone(), label.clone(), weak.clone(), window, cx)
        })
        .category("File")
        .key("Mod+P")
        .when(move |_, _| has_workspace),
    );
    workbench.register_command(Command::new("file.openFolder", "Open Folder…", with_shell(|shell, window, cx| shell.pick_and_open_folder(window, cx))).category("File").key("Mod+O"));
    workbench.register_command(Command::new("file.closeFolder", "Close Folder", with_shell(|shell, window, cx| shell.close_folder(window, cx))).category("File").when(move |_, _| has_workspace));
    workbench.register_command(
        Command::new("file.openRecent", "Open Recent…", {
            let core = core.clone();
            move |_, window, cx| {
                let items: Vec<PickItem> = core.store.recent().into_iter().map(|p| PickItem::new(p.rsplit(['/', '\\']).next().unwrap_or(&p).to_string(), p.clone()).description(p)).collect();
                quick_pick(
                    QuickPickOptions::new("Open recent folder", items).empty_text("No recent folders"),
                    |item, window, cx| {
                        if let (Some(item), Some(shell)) = (item, Shell::for_window(window, cx)) {
                            shell.update(cx, |shell, cx| shell.open_folder(PathBuf::from(item.value), window, cx));
                        }
                    },
                    window,
                    cx,
                );
            }
        })
        .category("File"),
    );
    workbench.register_command(Command::new("window.new", "New Window", |_, _, cx| crate::open_window(None, cx)).category("Window").key("Mod+Shift+N"));
    workbench.register_command(
        Command::new("workspace.trust", "Trust This Folder", with_shell(|shell, window, cx| shell.set_trusted(true, window, cx)))
            .category("Workspace")
            .when({
                let trusted = workbench.env().trusted;
                move |_, _| has_workspace && !trusted
            }),
    );
    workbench.register_command(
        Command::new("keybindings.openJson", "Open Keyboard Shortcuts (JSON)", {
            let (core, weak) = (core.clone(), weak.clone());
            move |_, window, cx| {
                let core = core.clone();
                let task = spawn_core(cx, async move { app::keybindings_ensure(&core).await });
                let weak = weak.clone();
                window
                    .spawn(cx, async move |cx| {
                        let result = task.await;
                        let _ = cx.update(|window, cx| match result {
                            Ok(path) => {
                                let _ = weak.update(cx, |workbench, cx| {
                                    workbench.open_tab(file_tab(&path), OpenOptions::default(), window, cx);
                                });
                            }
                            Err(error) => overlay::show_error(&error, window, cx),
                        });
                    })
                    .detach();
            }
        })
        .category("Preferences"),
    );
    workbench.register_command(Command::new("keybindings.reload", "Reload Keyboard Shortcuts", with_shell(|shell, window, cx| shell.load_keybindings(true, window, cx))).category("Preferences"));
    for (install, id, title) in [(true, "shellCommand.install", "Install 'oncode' Command in PATH"), (false, "shellCommand.uninstall", "Uninstall 'oncode' Command from PATH")] {
        workbench.register_command(Command::new(id, title, move |_, window, cx| set_shell_command(install, window, cx)).category("Shell Command"));
    }
}

/// The `oncode .` command in the terminal (see `oncode-helper/src/shell_command.rs`).
fn set_shell_command(install: bool, window: &mut Window, cx: &mut App) {
    let task = spawn_core(cx, app::shell_command_set(install));
    window
        .spawn(cx, async move |cx| {
            let result = task.await;
            let _ = cx.update(|window, cx| match result {
                Ok(message) => overlay::toast_success(message, window, cx),
                Err(error) => overlay::show_error(&error, window, cx),
            });
        })
        .detach();
}

/// The window that currently has `folder` open. Paths are compared after normalization, because `oncode .`
/// may give a path with different casing or links than the stored one.
pub(crate) fn window_with_folder(folder: &std::path::Path, cx: &App) -> Option<gpui_kit::AnyWindowHandle> {
    let wanted = std::fs::canonicalize(folder).ok()?;
    let shells = cx.try_global::<Shells>()?;
    cx.windows().into_iter().find(|handle| {
        let root = shells.0.get(&handle.window_id()).and_then(WeakEntity::upgrade).and_then(|shell| shell.read(cx).workspace().map(|w| w.root.clone()));
        root.is_some_and(|root| std::fs::canonicalize(root).is_ok_and(|root| root == wanted))
    })
}

fn show_command_palette(window: &mut Window, cx: &mut App) {
    let mut items: Vec<PickItem> = commands::all(window, cx)
        .into_iter()
        .filter(|c| !c.hidden && c.when.as_ref().is_none_or(|when| when(window, cx)))
        .map(|c| {
            let item = PickItem::new(c.label(), c.id.to_string());
            match commands::keybindings_for(&c, cx).first() {
                Some(binding) => item.hint(keys::format_keybinding(binding)),
                None => item,
            }
        })
        .collect();
    items.sort_by(|a, b| a.label.cmp(&b.label));
    quick_pick(
        QuickPickOptions::new("Type a command", items).empty_text("No matching commands"),
        |item, window, cx| {
            if let Some(item) = item {
                commands::execute_now(&item.value, &Value::Null, window, cx);
            }
        },
        window,
        cx,
    );
}

fn show_quick_open(core: Shared, label: SharedString, workbench: WeakEntity<Workbench>, window: &mut Window, cx: &mut App) {
    let root = core.workspace_root(&label).ok();
    let search = move |query: String, cx: &mut App| {
        let (core, label) = (core.clone(), label.clone());
        spawn_core(cx, async move {
            let results = oncode_core::api::fs::quick_open(&core, &label, query, 60).await.unwrap_or_default();
            results
                .into_iter()
                .map(|result| {
                    let (folder, name) = match result.path.rsplit_once('/') {
                        Some((folder, name)) => (folder.to_string(), name.to_string()),
                        None => (String::new(), result.path.clone()),
                    };
                    let image = crate::file_icons::file_icon_path(&name);
                    PickItem::new(name, result.path).description(folder).image(image)
                })
                .collect()
        })
    };
    quick_pick(
        QuickPickOptions::with_search("Go to file", search).empty_text("No matching files"),
        move |item, window, cx| {
            let (Some(item), Some(root)) = (item, root.clone()) else { return };
            let path = root.join(&item.value).to_string_lossy().into_owned();
            let _ = workbench.update(cx, |workbench, cx| {
                workbench.open_tab(file_tab(&path), OpenOptions::default(), window, cx);
            });
        },
        window,
        cx,
    );
}
