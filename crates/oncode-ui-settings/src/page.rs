//! Halaman pengaturan: formulir di atas pengaturan tingkat PENGGUNA. Setiap perubahan langsung
//! disimpan (isian teks menunggu jeda singkat); nilai yang tidak sah tidak pernah disimpan.
//!
//! Alur data: `user` adalah pengaturan pengguna terbaru menurut halaman ini, `saved` yang terakhir
//! diketahui tersimpan. Penyimpanan berjalan berurutan dan selalu menulis `user` terbaru, jadi
//! perubahan yang beruntun tidak saling mendahului. Perubahan dari luar (`SettingsChanged`)
//! diambil hanya bila tidak ada perubahan lokal yang lebih baru.

use std::collections::HashMap;
use std::time::Duration;

use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState, NumberInput};
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::component::select::{Select, SelectEvent, SelectItem, SelectState};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{h_flex, v_flex, ActiveTheme as _, Disableable as _, Icon, IndexPath, Sizable as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    div, px, AnyElement, AnyView, App, AppContext as _, Context, Entity, FocusHandle, Focusable, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, Task, Window,
};
use oncode_core::api;
use oncode_core::api::app::AppInfo;
use oncode_core::api::lsp::LspServerInfo;
use oncode_core::settings::LspSettings;
use oncode_core::{CoreEvent, Settings};
use oncode_lsp::ServerSpec;
use oncode_ui::overlay::{self, Confirm};
use oncode_ui::{commands, core_bus, spawn_core, Env, Lucide, WorkbenchEvent};
use serde_json::Value;

use crate::model::{
    apply_default, command_line, count_non_default, default_label, differs_from_default, field_by_id, field_matches, field_text, is_overridden, is_server_overridden, join_args,
    matches_query, parse_number, set_server_override, split_args, validate_server_command, Field, FieldKind, OverrideContext, SectionId, FIELDS, SECTIONS,
};

/// Jeda sebelum isian teks disimpan.
pub const TEXT_DEBOUNCE: Duration = Duration::from_millis(400);

const SERVER_TABLE_KEYWORDS: &str = "language servers lsp table command arguments override installed customize reset";
const KEYBOARD_KEYWORDS: &str = "keyboard shortcuts keybindings keybindings.json keys hotkeys open";
const ACCOUNTS_KEYWORDS: &str = "accounts github gitlab sign in sign out login logout token";
const OVERRIDE_TEXT: &str = "overridden by project (.oncode/settings.json)";
const OVERRIDE_TOOLTIP: &str = "A project setting in .oncode/settings.json replaces this value while this folder is open. Changing it here affects other folders.";

/// Kotak isian yang dikelola halaman: kolom teks/angka, atau perintah/argumen penimpaan server.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum ControlKey {
    Field(&'static str),
    ServerCommand(String),
    ServerArgs(String),
}

impl ControlKey {
    fn element_id(&self) -> SharedString {
        match self {
            ControlKey::Field(id) => format!("st-{id}"),
            ControlKey::ServerCommand(language) => format!("server-{language}-command"),
            ControlKey::ServerArgs(language) => format!("server-{language}-args"),
        }
        .into()
    }
}

/// Isian yang sudah lolos validasi dan siap disimpan.
enum Parsed {
    Text(String),
    Number(Option<u32>),
    Command(String),
    Args(Vec<String>),
}

fn parse(key: &ControlKey, text: &str) -> Result<Parsed, String> {
    match key {
        ControlKey::Field(id) => match field_by_id(id).map(|field| &field.kind) {
            Some(FieldKind::Text(spec)) => match (spec.validate)(text) {
                Some(message) => Err(message.to_string()),
                None => Ok(Parsed::Text(text.to_string())),
            },
            Some(FieldKind::Number(spec)) => parse_number(spec, text).map(Parsed::Number),
            _ => Err("This setting cannot be edited as text".to_string()),
        },
        ControlKey::ServerCommand(_) => match validate_server_command(text) {
            Some(message) => Err(message.to_string()),
            None => Ok(Parsed::Command(text.trim().to_string())),
        },
        ControlKey::ServerArgs(_) => split_args(text).map(Parsed::Args),
    }
}

/// Penimpaan yang sedang berlaku untuk `language`, atau server bawaannya sebagai titik awal.
/// Selalu dibangun dari nilai terbaru di pengaturan, agar `env` dan kolom lain tidak hilang.
fn base_spec(settings: &Settings, language: &str, servers: Option<&[LspServerInfo]>) -> Option<ServerSpec> {
    settings.lsp.servers.get(language).cloned().or_else(|| {
        let info = servers?.iter().find(|info| info.language == language)?;
        Some(ServerSpec { command: info.command.clone(), args: info.args.clone(), env: Vec::new() })
    })
}

fn apply_parsed(key: &ControlKey, parsed: Parsed, settings: &mut Settings, servers: Option<&[LspServerInfo]>) {
    match (key, parsed) {
        (ControlKey::Field(id), Parsed::Text(text)) => {
            if let Some(FieldKind::Text(spec)) = field_by_id(id).map(|field| &field.kind) {
                (spec.set)(settings, &text);
            }
        }
        (ControlKey::Field(id), Parsed::Number(number)) => {
            if let Some(FieldKind::Number(spec)) = field_by_id(id).map(|field| &field.kind) {
                (spec.set)(settings, number);
            }
        }
        (ControlKey::ServerCommand(language), Parsed::Command(command)) => {
            let mut spec = base_spec(settings, language, servers).unwrap_or_default();
            spec.command = command;
            set_server_override(settings, language, Some(spec));
        }
        (ControlKey::ServerArgs(language), Parsed::Args(args)) => {
            if let Some(mut spec) = base_spec(settings, language, servers) {
                spec.args = args;
                set_server_override(settings, language, Some(spec));
            }
        }
        _ => {}
    }
}

/// Isi kotak isian menurut pengaturan; `None` bila kotaknya sudah tidak punya tempat (penimpaan dihapus).
fn canonical_text(key: &ControlKey, settings: &Settings) -> Option<String> {
    match key {
        ControlKey::Field(id) => field_by_id(id).map(|field| field_text(field, settings)),
        ControlKey::ServerCommand(language) => settings.lsp.servers.get(language).map(|spec| spec.command.clone()),
        ControlKey::ServerArgs(language) => settings.lsp.servers.get(language).map(|spec| join_args(&spec.args)),
    }
}

struct TextControl {
    state: Entity<InputState>,
    /// Ada ketikan yang belum tersimpan (termasuk isian tidak sah yang masih ditampilkan).
    /// Selama ini benar, nilai dari luar tidak menimpa isi kotak.
    dirty: bool,
    problem: Option<SharedString>,
    pending: Option<Parsed>,
    debounce: Option<Task<()>>,
    _subscription: Subscription,
}

#[derive(Clone)]
struct ChoiceItem {
    value: &'static str,
    label: SharedString,
}

impl SelectItem for ChoiceItem {
    type Value = &'static str;

    fn title(&self) -> SharedString {
        self.label.clone()
    }

    fn value(&self) -> &Self::Value {
        &self.value
    }
}

type ChoiceState = SelectState<Vec<ChoiceItem>>;

pub struct SettingsPage {
    env: Env,
    focus_handle: FocusHandle,
    /// Pengaturan pengguna terbaru menurut halaman ini (mungkin belum selesai disimpan).
    user: Settings,
    /// Pengaturan pengguna yang terakhir diketahui tersimpan; pembanding untuk penanda "ditimpa proyek".
    saved: Settings,
    defaults: Settings,
    info: Option<AppInfo>,
    servers: Option<Vec<LspServerInfo>>,
    /// Pengaturan LSP saat tabel server terakhir dimuat; berubah berarti tabel perlu dimuat ulang.
    servers_for: Option<LspSettings>,
    search: Entity<InputState>,
    query: String,
    texts: HashMap<ControlKey, TextControl>,
    selects: HashMap<&'static str, Entity<ChoiceState>>,
    saving: bool,
    /// Tampilan akun dari crate lain (lihat `register_accounts_view`).
    accounts: Option<AnyView>,
    _servers_task: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl Focusable for SettingsPage {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl SettingsPage {
    pub fn new(env: Env, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let user = env.core.user_settings();
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search settings"));
        let mut subscriptions = vec![
            cx.subscribe_in(&search, window, |this, state, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::Change) {
                    this.query = state.read(cx).value().to_string();
                    cx.notify();
                }
            }),
            cx.subscribe_in(&core_bus(cx), window, |this, _, event: &CoreEvent, window, cx| {
                if let CoreEvent::SettingsChanged { settings } = event {
                    this.on_external_change(settings.clone(), window, cx);
                }
            }),
            // Suntingan yang masih menunggu ikut disimpan saat halaman ditutup.
            cx.on_release(Self::save_on_release),
        ];
        // Penanda "ditimpa proyek" bergantung pada pengaturan efektif jendela.
        if let Some(workbench) = env.workbench.upgrade() {
            subscriptions.push(cx.subscribe_in(&workbench, window, |_, _, event: &WorkbenchEvent, _, cx| {
                if matches!(event, WorkbenchEvent::SettingsChanged) {
                    cx.notify();
                }
            }));
        }

        let mut this = Self {
            env,
            focus_handle: cx.focus_handle(),
            saved: user.clone(),
            user,
            defaults: Settings::default(),
            info: None,
            servers: None,
            servers_for: None,
            search,
            query: String::new(),
            texts: HashMap::new(),
            selects: HashMap::new(),
            saving: false,
            accounts: None,
            _servers_task: Task::ready(()),
            _subscriptions: subscriptions,
        };
        for field in &FIELDS {
            match &field.kind {
                FieldKind::Text(_) | FieldKind::Number(_) => {
                    let key = ControlKey::Field(field.id);
                    let control = this.new_text_control(key.clone(), window, cx);
                    this.texts.insert(key, control);
                }
                FieldKind::Select(spec) => {
                    let items: Vec<ChoiceItem> = spec.options.iter().map(|option| ChoiceItem { value: option.value, label: option.label.into() }).collect();
                    let current = (spec.get)(&this.user);
                    let selected = spec.options.iter().position(|option| option.value == current).map(IndexPath::new);
                    let state = cx.new(|cx| SelectState::new(items, selected, window, cx));
                    let id = field.id;
                    this._subscriptions.push(cx.subscribe_in(&state, window, move |this, _, event: &SelectEvent<Vec<ChoiceItem>>, window, cx| {
                        let SelectEvent::Confirm(Some(value)) = event else { return };
                        if let Some(FieldKind::Select(spec)) = field_by_id(id).map(|field| &field.kind) {
                            let value = *value;
                            this.commit(|settings| (spec.set)(settings, value), window, cx);
                        }
                    }));
                    this.selects.insert(field.id, state);
                }
                FieldKind::Checkbox(_) => {}
            }
        }
        this.sync_controls(window, cx);
        this.load_info(window, cx);
        this.load_servers(window, cx);
        // `Workbench` sedang dipinjam saat tab dibangun; tampilan akun dibuat setelahnya supaya
        // pembangunnya bebas membaca `Workbench`.
        cx.defer_in(window, |this, window, cx| {
            if let Some(builder) = crate::accounts_builder(cx) {
                this.accounts = Some(builder(&this.env, window, cx));
                cx.notify();
            }
        });
        this
    }

    // ----- Pembacaan (dipakai juga oleh uji) -----

    /// Pengaturan pengguna terbaru menurut halaman ini.
    pub fn user_settings(&self) -> &Settings {
        &self.user
    }

    /// Semua perubahan sudah tersimpan dan tidak ada isian yang menunggu jeda.
    pub fn is_settled(&self) -> bool {
        !self.saving && self.user == self.saved && self.texts.values().all(|control| control.pending.is_none())
    }

    /// Pesan galat yang sedang ditampilkan untuk sebuah kolom (id seperti `editor.fontSize`).
    pub fn problem(&self, field_id: &str) -> Option<String> {
        self.texts.iter().find(|(key, _)| matches!(key, ControlKey::Field(id) if *id == field_id)).and_then(|(_, control)| control.problem.as_ref().map(|p| p.to_string()))
    }

    /// Isi kotak isian sebuah kolom teks atau angka seperti yang terlihat.
    pub fn field_input(&self, field_id: &str, cx: &App) -> Option<String> {
        self.texts.iter().find(|(key, _)| matches!(key, ControlKey::Field(id) if *id == field_id)).map(|(_, control)| control.state.read(cx).value().to_string())
    }

    /// Tabel language server yang sudah dimuat.
    pub fn servers(&self) -> Option<&[LspServerInfo]> {
        self.servers.as_deref()
    }

    /// Informasi aplikasi (versi git atau alasan git tidak tersedia), bila sudah dimuat.
    pub fn app_info(&self) -> Option<&AppInfo> {
        self.info.as_ref()
    }

    // ----- Memuat data -----

    fn load_info(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let core = self.env.core.clone();
        let task = spawn_core(cx, async move { api::app::app_info(&core).await });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.info = result.ok();
                cx.notify();
            });
        })
        .detach();
    }

    /// Memuat tabel server. Status terpasang ikut berubah saat penimpaan atau sakelar LSP berubah.
    fn load_servers(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.servers_for = Some(self.saved.lsp.clone());
        let (core, label) = (self.env.core.clone(), self.env.window.to_string());
        let task = spawn_core(cx, async move { api::lsp::lsp_servers(&core, &label).await });
        self._servers_task = cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(list) => this.servers = Some(list),
                    Err(_) => {
                        this.servers.get_or_insert_with(Vec::new);
                    }
                }
                cx.notify();
            });
        });
    }

    fn reload_servers_if_needed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.servers_for.as_ref() != Some(&self.saved.lsp) {
            self.load_servers(window, cx);
        }
    }

    // ----- Menyimpan -----

    /// Menerapkan perubahan ke pengaturan terbaru lalu menyimpannya.
    fn commit(&mut self, change: impl FnOnce(&mut Settings), window: &mut Window, cx: &mut Context<Self>) {
        let mut next = self.user.clone();
        change(&mut next);
        self.set_user(next, window, cx);
    }

    fn set_user(&mut self, next: Settings, window: &mut Window, cx: &mut Context<Self>) {
        if next == self.user {
            return;
        }
        self.user = next;
        self.sync_controls(window, cx);
        self.start_saving(window, cx);
        cx.notify();
    }

    /// Menyimpan `user` sampai yang tersimpan sama dengan yang terbaru. Hanya satu putaran yang
    /// berjalan pada satu waktu, jadi penyimpanan tidak pernah saling mendahului.
    fn start_saving(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.saving {
            return;
        }
        self.saving = true;
        let core = self.env.core.clone();
        cx.spawn_in(window, async move |this, cx| loop {
            let Ok(next) = this.update(cx, |this, _| this.user.clone()) else { return };
            let (core, writing) = (core.clone(), next.clone());
            let Ok(task) = cx.update(|_, cx| spawn_core(cx, async move { api::app::settings_set(&core, writing).await })) else { return };
            let result = task.await;
            let finished = this.update_in(cx, |this, window, cx| match result {
                Ok(()) => {
                    this.saved = next;
                    this.reload_servers_if_needed(window, cx);
                    cx.notify();
                    if this.user == this.saved {
                        this.saving = false;
                    }
                    !this.saving
                }
                Err(error) => {
                    this.saving = false;
                    overlay::toast_error(format!("Could not save settings: {}", error.message), window, cx);
                    // Tampilan dikembalikan ke apa yang benar-benar tersimpan.
                    let stored = this.env.core.user_settings();
                    this.user = stored.clone();
                    this.saved = stored;
                    this.sync_controls(window, cx);
                    cx.notify();
                    true
                }
            });
            if !matches!(finished, Ok(false)) {
                return;
            }
        })
        .detach();
    }

    /// Jendela lain (atau penyimpanan dari sini) mengubah pengaturan pengguna.
    fn on_external_change(&mut self, settings: Settings, window: &mut Window, cx: &mut Context<Self>) {
        // Perubahan lokal yang belum tersimpan lebih baru; penyimpanannya akan menyamakan keadaan.
        if self.saving || self.user != self.saved {
            return;
        }
        if settings != self.user {
            self.user = settings.clone();
            self.saved = settings;
            self.sync_controls(window, cx);
            self.reload_servers_if_needed(window, cx);
            cx.notify();
        }
    }

    /// Pengaturan yang harus tersimpan bila halaman ditutup sekarang: `user` ditambah isian yang masih menunggu jeda.
    fn settings_with_pending(&mut self) -> Settings {
        let mut next = self.user.clone();
        let keys: Vec<ControlKey> = self.texts.keys().cloned().collect();
        for key in keys {
            if let Some(parsed) = self.texts.get_mut(&key).and_then(|control| control.pending.take()) {
                apply_parsed(&key, parsed, &mut next, self.servers.as_deref());
            }
        }
        next
    }

    fn save_on_release(&mut self, cx: &mut App) {
        let next = self.settings_with_pending();
        if next != self.saved {
            let core = self.env.core.clone();
            spawn_core(cx, async move {
                let _ = api::app::settings_set(&core, next).await;
            })
            .detach();
        }
    }

    // ----- Kotak isian -----

    fn new_text_control(&self, key: ControlKey, window: &mut Window, cx: &mut Context<Self>) -> TextControl {
        let text = canonical_text(&key, &self.user).unwrap_or_default();
        let state = cx.new(|cx| {
            let state = InputState::new(window, cx).default_value(text);
            match &key {
                ControlKey::Field(id) => match field_by_id(id).map(|field| &field.kind) {
                    Some(FieldKind::Text(spec)) => state.placeholder(spec.placeholder),
                    // Hanya digit yang bisa diketik; di luar rentang dijepit `NumberInput` saat fokus lepas.
                    Some(FieldKind::Number(spec)) => {
                        state.placeholder(spec.placeholder).validate(|text, _| text.chars().all(|c| c.is_ascii_digit())).min(spec.min as f64).max(spec.max as f64)
                    }
                    _ => state,
                },
                ControlKey::ServerCommand(_) => state.placeholder("Command"),
                ControlKey::ServerArgs(_) => state.placeholder("Arguments"),
            }
        });
        let subscription = cx.subscribe_in(&state, window, {
            let key = key.clone();
            move |this, _, event: &InputEvent, window, cx| match event {
                InputEvent::Change => this.text_changed(&key, true, window, cx),
                InputEvent::PressEnter { .. } => this.flush(&key, window, cx),
                InputEvent::Blur => this.text_blurred(&key, window, cx),
                InputEvent::Focus => {}
            }
        });
        TextControl { state, dirty: false, problem: None, pending: None, debounce: None, _subscription: subscription }
    }

    /// Isi kotak berubah: validasi sekarang, simpan setelah jeda (atau saat `flush`).
    fn text_changed(&mut self, key: &ControlKey, debounce: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(control) = self.texts.get_mut(key) else { return };
        let text = control.state.read(cx).value().to_string();
        control.dirty = true;
        control.debounce = None;
        match parse(key, &text) {
            Err(message) => {
                control.pending = None;
                control.problem = Some(message.into());
            }
            Ok(parsed) => {
                control.problem = None;
                control.pending = Some(parsed);
                if debounce {
                    let key = key.clone();
                    control.debounce = Some(cx.spawn_in(window, async move |this, cx| {
                        cx.background_executor().timer(TEXT_DEBOUNCE).await;
                        let _ = this.update_in(cx, |this, window, cx| this.flush(&key, window, cx));
                    }));
                }
            }
        }
        cx.notify();
    }

    /// Menyimpan isian yang menunggu tanpa menunggu jeda (Enter, fokus lepas, atau jeda habis).
    fn flush(&mut self, key: &ControlKey, window: &mut Window, cx: &mut Context<Self>) {
        let Some(control) = self.texts.get_mut(key) else { return };
        // Tugas jeda dilepas, bukan dibatalkan: `flush` bisa sedang dipanggil dari dalamnya.
        if let Some(task) = control.debounce.take() {
            task.detach();
        }
        let Some(parsed) = control.pending.take() else { return };
        control.dirty = false;
        let mut next = self.user.clone();
        apply_parsed(key, parsed, &mut next, self.servers.as_deref());
        self.set_user(next, window, cx);
        cx.notify();
    }

    fn text_blurred(&mut self, key: &ControlKey, window: &mut Window, cx: &mut Context<Self>) {
        // `NumberInput` menjepit nilai di luar rentang saat fokus lepas; isi kotak dinilai ulang.
        if self.texts.get(key).is_some_and(|control| control.dirty) {
            self.text_changed(key, false, window, cx);
        }
        self.flush(key, window, cx);
        self.sync_text(key, window, cx);
    }

    /// Menyamakan isi kotak dengan pengaturan, kecuali kotak itu sedang disunting.
    fn sync_text(&mut self, key: &ControlKey, window: &mut Window, cx: &mut Context<Self>) {
        let Some(text) = canonical_text(key, &self.user) else { return };
        let Some(control) = self.texts.get(key) else { return };
        if control.dirty || control.state.focus_handle(cx).is_focused(window) {
            return;
        }
        if control.state.read(cx).value().as_ref() != text {
            control.state.update(cx, |state, cx| state.set_value(text, window, cx));
        }
    }

    /// Membuang suntingan lokal sebuah kotak (termasuk isian tidak sah) dan menampilkan nilai pengaturan.
    fn reset_text(&mut self, key: &ControlKey, window: &mut Window, cx: &mut Context<Self>) {
        let Some(text) = canonical_text(key, &self.user) else { return };
        let Some(control) = self.texts.get_mut(key) else { return };
        control.debounce = None;
        control.pending = None;
        control.problem = None;
        control.dirty = false;
        control.state.update(cx, |state, cx| state.set_value(text, window, cx));
    }

    /// Menyamakan semua kontrol dengan `user`: kotak isian, pilihan, dan kotak penimpaan server.
    fn sync_controls(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let languages: Vec<String> = self.user.lsp.servers.keys().cloned().collect();
        self.texts.retain(|key, _| match key {
            ControlKey::ServerCommand(language) | ControlKey::ServerArgs(language) => languages.contains(language),
            ControlKey::Field(_) => true,
        });
        for language in languages {
            for key in [ControlKey::ServerCommand(language.clone()), ControlKey::ServerArgs(language.clone())] {
                if !self.texts.contains_key(&key) {
                    let control = self.new_text_control(key.clone(), window, cx);
                    self.texts.insert(key, control);
                }
            }
        }
        let keys: Vec<ControlKey> = self.texts.keys().cloned().collect();
        for key in keys {
            self.sync_text(&key, window, cx);
        }
        for field in &FIELDS {
            let FieldKind::Select(spec) = &field.kind else { continue };
            let Some(select) = self.selects.get(field.id) else { continue };
            let value = (spec.get)(&self.user);
            if select.read(cx).selected_value() != Some(&value) {
                select.update(cx, |state, cx| state.set_selected_value(&value, window, cx));
            }
        }
    }

    // ----- Reset -----

    fn reset_field(&mut self, field: &'static Field, window: &mut Window, cx: &mut Context<Self>) {
        let defaults = self.defaults.clone();
        self.commit(|settings| apply_default(field, settings, &defaults), window, cx);
        // Apa pun yang sedang diketik di kolom ini kalah oleh nilai yang baru ditetapkan.
        self.reset_text(&ControlKey::Field(field.id), window, cx);
        cx.notify();
    }

    fn reset_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let count = count_non_default(&self.user, &self.defaults);
        let message = format!(
            "{count} setting{} you changed will return to {}, including language server overrides and OAuth Client IDs. This cannot be undone.\n\nSigned-in accounts, keyboard shortcuts and project settings are not affected.",
            if count == 1 { "" } else { "s" },
            if count == 1 { "its default" } else { "their defaults" },
        );
        let confirmed = overlay::confirm(Confirm::new("Reset all settings to their defaults?", message, "Reset Settings").danger(), window, cx);
        cx.spawn_in(window, async move |this, cx| {
            if !confirmed.await {
                return;
            }
            let _ = this.update_in(cx, |this, window, cx| {
                let defaults = this.defaults.clone();
                this.commit(|settings| *settings = defaults, window, cx);
                let keys: Vec<ControlKey> = this.texts.keys().cloned().collect();
                for key in keys {
                    this.reset_text(&key, window, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    // ----- Penimpaan server -----

    /// Membuat penimpaan dari server yang sedang berlaku, untuk kemudian disunting.
    fn customize_server(&mut self, language: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(spec) = base_spec(&self.user, language, self.servers.as_deref()) else { return };
        self.commit(|settings| set_server_override(settings, language, Some(spec)), window, cx);
    }

    fn remove_server_override(&mut self, language: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.commit(|settings| set_server_override(settings, language, None), window, cx);
    }

    // ----- Tampilan -----

    fn render_override_marker(&self, id: SharedString, cx: &mut Context<Self>) -> AnyElement {
        div()
            .id(id)
            .flex_none()
            .text_xs()
            .text_color(cx.theme().warning)
            .child(OVERRIDE_TEXT)
            .tooltip(|window, cx| Tooltip::new(OVERRIDE_TOOLTIP).build(window, cx))
            .test_support()
            .into_any_element()
    }

    fn render_problem(&self, key: &ControlKey, cx: &mut Context<Self>) -> Option<AnyElement> {
        let problem = self.texts.get(key)?.problem.clone()?;
        Some(
            div()
                .id(SharedString::from(format!("{}-error", key.element_id())))
                .text_xs()
                .text_color(cx.theme().danger)
                .child(SharedString::from(format!("{problem} — not saved")))
                .test_support()
                .into_any_element(),
        )
    }

    /// Versi git yang terdeteksi, atau alasan fitur Git tidak tersedia.
    fn render_git_status(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let info = self.info.as_ref()?;
        let theme = cx.theme();
        let (color, icon, text) = match (&info.git, &info.git_error) {
            (Some(git), _) => (theme.muted_foreground, Lucide::Check, format!("Using git {} at {}", git.version, git.path)),
            (None, Some(error)) => (theme.danger, Lucide::CircleAlert, error.clone()),
            (None, None) => return None,
        };
        Some(
            h_flex()
                .id("st-git-status")
                .gap_1p5()
                .items_start()
                .text_xs()
                .text_color(color)
                .child(Icon::new(icon).xsmall().flex_none().mt(px(2.)))
                .child(div().min_w_0().child(SharedString::from(text)))
                .test_support()
                .into_any_element(),
        )
    }

    fn render_field(&self, field: &'static Field, overridden: bool, cx: &mut Context<Self>) -> AnyElement {
        let (muted, border, mono) = (cx.theme().muted_foreground, cx.theme().border, cx.theme().mono_font_family.clone());
        let key = ControlKey::Field(field.id);
        let control_id = key.element_id();
        let control: AnyElement = match &field.kind {
            FieldKind::Checkbox(spec) => Switch::new(control_id.clone())
                .checked((spec.get)(&self.user))
                .accessibility_label(field.label)
                .on_change(cx.listener(move |this, checked: &bool, window, cx| {
                    let checked = *checked;
                    this.commit(|settings| (spec.set)(settings, checked), window, cx);
                }))
                .into_any_element(),
            FieldKind::Select(_) => match self.selects.get(field.id) {
                Some(state) => Select::new(state).id(control_id.clone()).accessibility_label(field.label).w(px(220.)).into_any_element(),
                None => div().into_any_element(),
            },
            FieldKind::Number(spec) => match self.texts.get(&key) {
                Some(control) => h_flex()
                    .gap_2()
                    .items_center()
                    .child(div().id(control_id.clone()).w(px(140.)).child(NumberInput::new(&control.state).accessibility_label(field.label)).test_support())
                    .when(!spec.unit.is_empty(), |el| el.child(div().text_sm().text_color(muted).child(spec.unit)))
                    .into_any_element(),
                None => div().into_any_element(),
            },
            FieldKind::Text(spec) => match self.texts.get(&key) {
                Some(control) => Input::new(&control.state).id(control_id.clone()).aria_label(field.label).w(px(340.)).when(spec.monospace, |el| el.font_family(mono)).into_any_element(),
                None => div().into_any_element(),
            },
        };
        let reset = differs_from_default(field, &self.user, &self.defaults).then(|| {
            Button::new(SharedString::from(format!("{control_id}-reset")))
                .ghost()
                .xsmall()
                .icon(Icon::new(Lucide::RotateCcw))
                .accessibility_label(format!("Reset {} to default", field.label))
                .tooltip(format!("Reset to default ({})", default_label(field, &self.defaults)))
                .on_click(cx.listener(move |this, _, window, cx| this.reset_field(field, window, cx)))
        });
        let status = (field.id == "git.path").then(|| self.render_git_status(cx)).flatten();

        h_flex()
            .id(SharedString::from(format!("row-{}", field.id)))
            .w_full()
            .py_3()
            .gap_6()
            .items_start()
            .justify_between()
            .border_b_1()
            .border_color(border.opacity(0.5))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_1()
                    .child(
                        h_flex()
                            .gap_2()
                            .flex_wrap()
                            .child(div().text_sm().font_weight(FontWeight::MEDIUM).child(field.label))
                            .when(overridden, |el| el.child(self.render_override_marker(format!("{control_id}-override").into(), cx))),
                    )
                    .child(div().text_sm().text_color(muted).child(field.description))
                    .children(status),
            )
            .child(
                v_flex()
                    .flex_none()
                    .items_end()
                    .gap_1()
                    .child(h_flex().gap_1().items_center().child(control).children(reset))
                    .children(self.render_problem(&key, cx)),
            )
            .test_support()
            .into_any_element()
    }

    fn render_server_row(&self, info: &LspServerInfo, overridden: bool, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (muted, border, mono, success, danger) = (theme.muted_foreground, theme.border, theme.mono_font_family.clone(), theme.success, theme.danger);
        let language = info.language.clone();
        let has_override = self.user.lsp.servers.contains_key(&language);
        let command_key = ControlKey::ServerCommand(language.clone());
        let args_key = ControlKey::ServerArgs(language.clone());

        let status = if info.installed {
            h_flex().gap_1().items_center().text_color(success).child(Icon::new(Lucide::Check).xsmall()).child("Installed").into_any_element()
        } else {
            v_flex()
                .gap_1()
                .child(h_flex().gap_1().items_center().text_color(danger).child(Icon::new(Lucide::X).xsmall()).child("Not found"))
                .when(!info.install_hint.is_empty(), |el| el.child(div().text_xs().text_color(muted).child(SharedString::from(info.install_hint.clone()))))
                .into_any_element()
        };

        let editor: AnyElement = match (has_override, self.texts.get(&command_key), self.texts.get(&args_key)) {
            (true, Some(command), Some(args)) => v_flex()
                .gap_1()
                .child(Input::new(&command.state).id(command_key.element_id()).aria_label(format!("Command for {language}")).small().font_family(mono.clone()))
                .children(self.render_problem(&command_key, cx))
                .child(Input::new(&args.state).id(args_key.element_id()).aria_label(format!("Arguments for {language}")).small().font_family(mono.clone()))
                .children(self.render_problem(&args_key, cx))
                .child(
                    h_flex().child(
                        Button::new(SharedString::from(format!("server-{language}-reset")))
                            .outline()
                            .xsmall()
                            .label("Reset")
                            .tooltip("Remove this override and use the built-in server")
                            .on_click(cx.listener({
                                let language = language.clone();
                                move |this, _, window, cx| this.remove_server_override(&language, window, cx)
                            })),
                    ),
                )
                .into_any_element(),
            _ => h_flex()
                .child(
                    Button::new(SharedString::from(format!("server-{language}-customize")))
                        .outline()
                        .xsmall()
                        .label("Customize")
                        .tooltip("Use a different command or arguments for this language")
                        .on_click(cx.listener({
                            let language = language.clone();
                            move |this, _, window, cx| this.customize_server(&language, window, cx)
                        })),
                )
                .into_any_element(),
        };

        h_flex()
            .id(SharedString::from(format!("server-{language}")))
            .w_full()
            .py_2()
            .gap_3()
            .items_start()
            .text_sm()
            .border_b_1()
            .border_color(border.opacity(0.5))
            .child(
                v_flex()
                    .w(px(110.))
                    .flex_none()
                    .gap_1()
                    .child(SharedString::from(language.clone()))
                    .when(overridden, |el| el.child(self.render_override_marker(format!("server-{language}-override").into(), cx))),
            )
            .child(div().flex_1().min_w_0().font_family(mono).text_xs().child(SharedString::from(command_line(&info.command, &info.args))))
            .child(div().w(px(200.)).flex_none().child(status))
            .child(div().w(px(240.)).flex_none().child(editor))
            .test_support()
            .into_any_element()
    }

    fn render_servers(&self, visible: &[&LspServerInfo], context: OverrideContext, cx: &mut Context<Self>) -> AnyElement {
        let (muted, border) = (cx.theme().muted_foreground, cx.theme().border);
        let body: AnyElement = match &self.servers {
            None => div().py_2().text_sm().text_color(muted).child("Loading language servers…").into_any_element(),
            Some(_) => v_flex()
                .w_full()
                .child(
                    h_flex()
                        .w_full()
                        .py_1p5()
                        .gap_3()
                        .text_xs()
                        .text_color(muted)
                        .border_b_1()
                        .border_color(border)
                        .child(div().w(px(110.)).flex_none().child("Language"))
                        .child(div().flex_1().min_w_0().child("Command"))
                        .child(div().w(px(200.)).flex_none().child("Status"))
                        .child(div().w(px(240.)).flex_none().child("Your override")),
                )
                .children(visible.iter().map(|info| self.render_server_row(info, is_server_overridden(&info.language, context), cx)))
                .into_any_element(),
        };
        v_flex()
            .id("row-lsp.servers")
            .w_full()
            .py_3()
            .gap_2()
            .child(body)
            .child(div().text_sm().text_color(muted).child(
                "Language servers are not bundled with OnCode. Install the ones you need and make sure they are on PATH, or point a language to a specific program with an override. Other languages can be added under lsp.servers in settings.json.",
            ))
            .test_support()
            .into_any_element()
    }

    fn render_keyboard(&self, cx: &mut Context<Self>) -> AnyElement {
        let (muted, border) = (cx.theme().muted_foreground, cx.theme().border);
        let path = self.env.core.keybindings_path().to_string_lossy().into_owned();
        h_flex()
            .id("row-keybindings")
            .w_full()
            .py_3()
            .gap_6()
            .items_start()
            .justify_between()
            .border_b_1()
            .border_color(border.opacity(0.5))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_1()
                    .child(div().text_sm().font_weight(FontWeight::MEDIUM).child("Keyboard shortcuts"))
                    .child(div().text_sm().text_color(muted).child(
                        "Shortcuts are customised in keybindings.json, an object that maps a command id to a shortcut such as \"Mod+Shift+K\" (or a list of them). Saving the file applies it.",
                    )),
            )
            .child(Button::new("settings-open-keybindings").outline().small().icon(Icon::new(Lucide::Keyboard)).label("Open keybindings.json").tooltip(path).on_click(|_, window, cx| {
                // Command yang tidak terdaftar menghasilkan galat yang terlihat, bukan diam.
                if commands::get("keybindings.openJson", window, cx).is_some() {
                    commands::execute("keybindings.openJson", Value::Null, window, cx);
                } else {
                    overlay::toast_error("Could not open keybindings.json: the command is not available", window, cx);
                }
            }))
            .test_support()
            .into_any_element()
    }

    /// Daftar akun dan login: tampilan dari fitur forge bila terdaftar, selain itu tombol yang
    /// menjalankan command masuknya (bila ada).
    fn render_accounts(&self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (muted, border) = (cx.theme().muted_foreground, cx.theme().border);
        if let Some(view) = &self.accounts {
            return Some(div().id("row-accounts").w_full().py_3().border_b_1().border_color(border.opacity(0.5)).child(view.clone()).test_support().into_any_element());
        }
        commands::get("forge.signIn", window, cx)?;
        Some(
            h_flex()
                .id("row-accounts")
                .w_full()
                .py_3()
                .gap_6()
                .items_start()
                .justify_between()
                .border_b_1()
                .border_color(border.opacity(0.5))
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .gap_1()
                        .child(div().text_sm().font_weight(FontWeight::MEDIUM).child("Accounts"))
                        .child(div().text_sm().text_color(muted).child("Sign in to GitHub or GitLab to list and create pull requests and to clone your repositories.")),
                )
                .child(Button::new("settings-sign-in").outline().small().icon(Icon::new(Lucide::LogIn)).label("Sign In…").on_click(|_, window, cx| {
                    commands::execute("forge.signIn", Value::Null, window, cx);
                }))
                .test_support()
                .into_any_element(),
        )
    }

    fn render_header(&self, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let changed = count_non_default(&self.user, &self.defaults);
        let settings_path = self.env.core.settings_path().to_string_lossy().into_owned();
        let scope = if self.env.root.is_none() {
            ""
        } else if self.env.trusted {
            " A project can override them in .oncode/settings.json."
        } else {
            " Project settings are ignored in restricted mode."
        };
        v_flex()
            .w_full()
            .gap_3()
            .child(div().text_2xl().font_weight(FontWeight::SEMIBOLD).child("Settings"))
            .child(
                h_flex()
                    .w_full()
                    .gap_2()
                    .items_center()
                    .child(div().flex_1().min_w_0().child(Input::new(&self.search).id("settings-search").aria_label("Search settings").prefix(Icon::new(Lucide::Search).small()).cleanable(true)))
                    .child(Button::new("settings-open-json").outline().icon(Icon::new(Lucide::Braces)).label("Open settings.json").tooltip(settings_path).on_click(|_, window, cx| {
                        commands::execute("settings.openJson", Value::Null, window, cx);
                    }))
                    .child(
                        Button::new("settings-reset-all")
                            .outline()
                            .icon(Icon::new(Lucide::RotateCcw))
                            .label("Reset to defaults")
                            .disabled(changed == 0)
                            .tooltip(if changed == 0 {
                                "Every setting already has its default value".to_string()
                            } else {
                                format!("Reset {changed} changed setting{}", if changed == 1 { "" } else { "s" })
                            })
                            .on_click(cx.listener(|this, _, window, cx| this.reset_all(window, cx))),
                    ),
            )
            .child(div().text_sm().text_color(muted).child(SharedString::from(format!("These are your user settings; changes are saved automatically and apply to every folder.{scope}"))))
            .into_any_element()
    }
}

impl Render for SettingsPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // `render` berjalan di luar pinjaman `Workbench`, jadi pengaturan efektif boleh dibaca di sini.
        let effective = self.env.workbench.upgrade().map(|workbench| workbench.read(cx).settings().clone());
        let saved = self.saved.clone();
        let context = OverrideContext { trusted: self.env.trusted, user: &saved, effective: effective.as_ref() };
        let query = self.query.clone();
        let (muted, border, background) = (cx.theme().muted_foreground, cx.theme().border, cx.theme().background);

        let table_matches = matches_query(&query, &[SERVER_TABLE_KEYWORDS]);
        let servers = self.servers.clone().unwrap_or_default();
        let visible_servers: Vec<&LspServerInfo> =
            servers.iter().filter(|info| table_matches || matches_query(&query, &[&info.language, &info.command, &join_args(&info.args)])).collect();

        let mut sections: Vec<AnyElement> = Vec::new();
        for section in SECTIONS {
            let mut rows: Vec<AnyElement> = FIELDS
                .iter()
                .filter(|field| field.section == section && field_matches(field, &query))
                .map(|field| self.render_field(field, is_overridden(field, context), cx))
                .collect();
            match section {
                SectionId::Lsp if table_matches || !visible_servers.is_empty() => rows.push(self.render_servers(&visible_servers, context, cx)),
                SectionId::Accounts if matches_query(&query, &[ACCOUNTS_KEYWORDS]) => rows.extend(self.render_accounts(window, cx)),
                SectionId::Keyboard if matches_query(&query, &[KEYBOARD_KEYWORDS]) => rows.push(self.render_keyboard(cx)),
                _ => {}
            }
            if rows.is_empty() {
                continue;
            }
            sections.push(
                v_flex()
                    .id(SharedString::from(format!("section-{}", section.key())))
                    .w_full()
                    .child(div().pb_1p5().text_lg().font_weight(FontWeight::SEMIBOLD).border_b_1().border_color(border).child(section.title()))
                    .children(rows)
                    .test_support()
                    .into_any_element(),
            );
        }
        let nothing = sections.is_empty();

        div().id("settings-page").test_support().size_full().track_focus(&self.focus_handle).bg(background).child(
            div()
                .size_full()
                .child(
                    v_flex()
                        .w_full()
                        .max_w(px(960.))
                        .mx_auto()
                        .px_6()
                        .py_5()
                        .gap_6()
                        .child(self.render_header(cx))
                        .children(sections)
                        .when(nothing, |el| el.child(div().id("settings-no-match").text_sm().text_color(muted).child(SharedString::from(format!("No settings match “{query}”."))).test_support())),
                )
                .overflow_y_scrollbar(),
        )
    }
}
