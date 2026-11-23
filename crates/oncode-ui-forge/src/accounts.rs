//! Akun GitHub dan GitLab: masuk (device flow atau token), keluar, dan daftar akun untuk halaman
//! pengaturan.

use std::time::{Duration, Instant};

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{h_flex, v_flex, ActiveTheme as _, Icon, Sizable as _, WindowExt as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    div, px, App, AppContext as _, AsyncWindowContext, ClipboardItem, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString,
    Styled as _, Subscription, Task, Window,
};
use oncode_core::api::forge as forge_api;
use oncode_core::{AppError, CoreEvent};
use oncode_forge::{Account, ForgeKind};
use oncode_ui::overlay::{self, Choice, Confirm, TextPrompt};
use oncode_ui::quick_pick::{quick_pick, PickItem, QuickPickOptions};
use oncode_ui::{core, core_bus, spawn_core, Lucide};

use crate::login::{scopes_sentence, DevicePoller, LoginOutcome, PollNext};
use crate::util::{describe_forge_error, forge_host, forge_name, login_host_for, normalize_host, open_external};

/// Memilih dari daftar lewat quick pick, menunggu hasilnya.
pub(crate) async fn pick(cx: &mut AsyncWindowContext, options: QuickPickOptions) -> Option<PickItem> {
    let (tx, rx) = tokio::sync::oneshot::channel::<Option<PickItem>>();
    let tx = std::cell::Cell::new(Some(tx));
    cx.update(|window, cx| {
        quick_pick(
            options,
            move |item, _, _| {
                if let Some(tx) = tx.take() {
                    let _ = tx.send(item);
                }
            },
            window,
            cx,
        )
    })
    .ok()?;
    rx.await.ok().flatten()
}

pub(crate) async fn core_call<T: Send + 'static>(cx: &mut AsyncWindowContext, future: impl std::future::Future<Output = Result<T, AppError>> + Send + 'static) -> Result<T, AppError> {
    match cx.update(|_, cx| spawn_core(cx, future)) {
        Ok(task) => task.await,
        Err(_) => Err(AppError::new("cancelled", "The window was closed")),
    }
}

/// Alur "Sign In": pilih layanan, host (untuk instance sendiri), lalu cara masuk.
pub fn sign_in(window: &mut Window, cx: &mut App) {
    window
        .spawn(cx, async move |cx| {
            let items = vec![
                PickItem::new("GitHub", "github"),
                PickItem::new("GitLab", "gitlab"),
                PickItem::new("GitHub Enterprise…", "github-host"),
                PickItem::new("GitLab (self-hosted)…", "gitlab-host"),
            ];
            let Some(choice) = pick(cx, QuickPickOptions::new("Sign in to", items)).await else { return };
            let kind = if choice.value.starts_with("github") { ForgeKind::GitHub } else { ForgeKind::GitLab };
            let host = if choice.value.ends_with("-host") {
                let answer = cx.update(|window, cx| overlay::prompt_text(TextPrompt::new("Server address", "Next").placeholder("https://git.example.com"), window, cx));
                let Ok(answer) = answer else { return };
                let Some(text) = answer.await else { return };
                match normalize_host(&text) {
                    Some(host) => Some(host),
                    None => {
                        let _ = cx.update(|window, cx| overlay::toast_error("Enter a valid server address, e.g. https://git.example.com", window, cx));
                        return;
                    }
                }
            } else {
                None
            };
            let methods = vec![PickItem::new("Sign in with the browser", "device").description("Recommended"), PickItem::new("Use a personal access token", "token")];
            let Some(method) = pick(cx, QuickPickOptions::new(format!("How do you want to sign in to {}?", forge_name(Some(kind))), methods)).await else { return };
            if method.value == "token" {
                token_flow(kind, host, cx).await;
            } else {
                device_flow(kind, host, cx).await;
            }
        })
        .detach();
}

async fn token_flow(kind: ForgeKind, host: Option<String>, cx: &mut AsyncWindowContext) {
    let message = format!("Create a token with the {} in your {} account settings, then paste it here.", scopes_sentence(kind), forge_name(Some(kind)));
    paste_token_and_login(kind, host, message, cx).await;
}

/// Login lewat browser tanpa aplikasi OAuth: membuka halaman pembuatan token (nama dan scope
/// sudah terisi), lalu meminta token yang dibuat di sana.
async fn browser_token_flow(kind: ForgeKind, host: Option<String>, cx: &mut AsyncWindowContext) {
    let url = forge_host(kind, host.as_deref()).token_creation_url();
    let opened = cx.update(|_, cx| open_external(&url, cx)).unwrap_or(false);
    let lead = if opened { "A page to create a token opened in your browser" } else { "Open this page in your browser to create a token" };
    let message = format!("{lead}, with the name and the {} already filled in. Create the token, then paste it here.\n\n{url}", scopes_sentence(kind));
    paste_token_and_login(kind, host, message, cx).await;
}

async fn paste_token_and_login(kind: ForgeKind, host: Option<String>, message: String, cx: &mut AsyncWindowContext) {
    let Ok(answer) = cx.update(|window, cx| overlay::prompt_text(TextPrompt::new("Personal access token", "Sign In").message(message).secret(), window, cx)) else { return };
    let Some(token) = answer.await.map(|t| t.trim().to_string()).filter(|t| !t.is_empty()) else { return };
    let core = match cx.update(|_, cx| core(cx)) {
        Ok(core) => core,
        Err(_) => return,
    };
    let login_host = host.clone();
    let result = core_call(cx, async move { forge_api::forge_login_token(&core, kind, login_host, token).await }).await;
    finish(result.map(LoginOutcome::Done).unwrap_or_else(LoginOutcome::Error), host.as_deref(), cx);
}

fn finish(outcome: LoginOutcome, host: Option<&str>, cx: &mut AsyncWindowContext) {
    let host = host.map(String::from);
    let _ = cx.update(|window, cx| match outcome {
        LoginOutcome::Done(account) => overlay::toast_success(format!("Signed in to {} as {}.", forge_name(Some(account.kind)), account.username), window, cx),
        LoginOutcome::Denied => overlay::toast_warning("Sign-in was cancelled in the browser.", window, cx),
        LoginOutcome::Expired => overlay::toast_warning("The sign-in code expired. Start again to get a new code.", window, cx),
        LoginOutcome::Error(error) if error.code == "cancelled" => {}
        LoginOutcome::Error(error) => overlay::toast_error(describe_forge_error(&error, host.as_deref()).line(), window, cx),
    });
}

/// Isi dialog device flow: kode yang harus dimasukkan pengguna di browser.
struct DeviceDialog {
    code: SharedString,
    url: SharedString,
    status: SharedString,
}

impl Render for DeviceDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (code, url) = (self.code.clone(), self.url.clone());
        v_flex()
            .gap_3()
            .text_sm()
            .child("Enter this code in the browser page that opens:")
            .child(
                h_flex()
                    .gap_2()
                    .child(div().id("device-code").px_3().py_1().rounded(theme.radius).bg(theme.secondary).font_family(theme.mono_font_family.clone()).text_xl().child(code.clone()))
                    .child(Button::new("device-copy").ghost().small().icon(Icon::new(Lucide::Copy)).tooltip("Copy code").on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(code.to_string())))),
            )
            .child(Button::new("device-open").outline().small().icon(Icon::new(Lucide::ExternalLink)).label("Open Browser").on_click(move |_, _, cx| {
                open_external(&url, cx);
            }))
            .child(div().id("device-status").text_color(theme.muted_foreground).child(self.status.clone()))
    }
}

async fn device_flow(kind: ForgeKind, host: Option<String>, cx: &mut AsyncWindowContext) {
    let Ok(core) = cx.update(|_, cx| core(cx)) else { return };
    let start = loop {
        let (start_core, start_host) = (core.clone(), host.clone());
        let error = match core_call(cx, async move { forge_api::forge_login_start(&start_core, kind, start_host).await }).await {
            Ok(start) => break start,
            Err(error) if error.code == "network" || error.code == "cancelled" => return finish(LoginOutcome::Error(error), host.as_deref(), cx),
            Err(error) => error,
        };
        // Device flow tidak bisa dipakai di server ini (belum ada Client ID, atau ditolak server):
        // tawarkan token lewat browser atau Client ID lain. Tiap putaran menunggu pilihan pengguna.
        match browser_sign_in_fallback(kind, host.as_deref(), &error, cx).await {
            Some(Fallback::Token) => return browser_token_flow(kind, host, cx).await,
            Some(Fallback::ClientId) if ask_client_id(kind, host.clone(), cx).await => continue,
            _ => return,
        }
    };
    let url = start.verification_uri_complete.clone().unwrap_or_else(|| start.verification_uri.clone());
    let dialog = match cx.update(|window, cx| {
        let dialog = cx.new(|_| DeviceDialog { code: start.user_code.clone().into(), url: url.clone().into(), status: "Waiting for you to approve in the browser…".into() });
        let shown = dialog.clone();
        let (cancel_core, user_code) = (core.clone(), start.user_code.clone());
        window.open_dialog(cx, move |d, _, _| {
            let (cancel_core, user_code) = (cancel_core.clone(), user_code.clone());
            d.title("Sign in with the browser").w(px(460.)).child(shown.clone()).on_close(move |_, _, cx| {
                let (core, code) = (cancel_core.clone(), user_code.clone());
                spawn_core(cx, async move { forge_api::forge_login_cancel(&core, code).await }).detach();
            })
        });
        open_external(&url, cx);
        dialog
    }) {
        Ok(dialog) => dialog,
        Err(_) => return,
    };
    let mut poller = DevicePoller::new(start.interval, start.expires_in);
    let started = Instant::now();
    let outcome = loop {
        let Some(delay) = poller.next_delay(started.elapsed()) else { break LoginOutcome::Expired };
        cx.background_executor().timer(delay).await;
        // Dialog ditutup pengguna: berhenti.
        if cx.update(|window, cx| window.has_active_dialog(cx)).unwrap_or(false) == false {
            return;
        }
        if poller.is_expired(started.elapsed()) {
            break LoginOutcome::Expired;
        }
        let (poll_core, code) = (core.clone(), start.user_code.clone());
        let answer = core_call(cx, async move { forge_api::forge_login_poll(&poll_core, code).await }).await;
        match poller.on_answer(answer) {
            PollNext::Continue { retrying } => {
                let status = if retrying { "Connection problem; retrying…" } else { "Waiting for you to approve in the browser…" };
                let _ = cx.update(|_, cx| dialog.update(cx, |d, cx| {
                    d.status = status.into();
                    cx.notify();
                }));
            }
            PollNext::Finish(outcome) => break outcome,
        }
    };
    let _ = cx.update(|window, cx| window.close_dialog(cx));
    finish(outcome, host.as_deref(), cx);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fallback {
    /// Buat token di halaman web server, lalu tempel.
    Token,
    /// Isi Client ID aplikasi OAuth milik server itu, lalu coba device flow lagi.
    ClientId,
}

/// Pilihan saat device flow tidak bisa dimulai. Aplikasi OAuth hanya dikenal oleh server tempat
/// ia didaftarkan, jadi server self-hosted tidak bisa memakai Client ID bawaan OnCode.
async fn browser_sign_in_fallback(kind: ForgeKind, host: Option<&str>, error: &AppError, cx: &mut AsyncWindowContext) -> Option<Fallback> {
    let server = forge_host(kind, host).hostname();
    let (title, message) = if error.code == "noClientId" {
        (
            format!("Sign in to {server} with the browser"),
            format!(
                "Signing in with a code needs an OAuth application registered on {server}, and OnCode has no Client ID for it yet.\n\n\
                 Create Token in Browser opens the token page of {server} with the name and scopes filled in, so no setup is needed. \
                 Or enter the Client ID of an OAuth application you registered on {server}."
            ),
        )
    } else {
        (
            format!("Browser sign-in to {server} is not available"),
            format!("{}\n\nYou can create a token in the browser instead, or enter a different Client ID.", describe_forge_error(error, host).line()),
        )
    };
    let choices = vec![Choice::new("cancel", "Cancel"), Choice::new("client-id", "Enter Client ID…"), Choice::new("token", "Create Token in Browser").primary()];
    let answer = cx.update(|window, cx| overlay::choose(title, message, choices, window, cx)).ok()?.await?;
    match answer {
        "token" => Some(Fallback::Token),
        "client-id" => Some(Fallback::ClientId),
        _ => None,
    }
}

/// Meminta Client ID aplikasi OAuth untuk server itu lalu menyimpannya (`forge.clientIds`).
/// `false` bila dibatalkan atau gagal disimpan.
async fn ask_client_id(kind: ForgeKind, host: Option<String>, cx: &mut AsyncWindowContext) -> bool {
    let server = forge_host(kind, host.as_deref()).hostname();
    let message = match kind {
        ForgeKind::GitLab => format!(
            "On {server}, open User Settings → Applications (or Admin Area → Applications) and add an application with \"Confidential\" unticked \
             and the scopes api and read_user. Any redirect URI will do; it is not used. Paste its Application ID here."
        ),
        ForgeKind::GitHub => format!("On {server}, open Settings → Developer settings → OAuth Apps and register an app with \"Enable Device Flow\" ticked. Paste its Client ID here."),
    };
    let prompt = TextPrompt::new("OAuth Client ID", "Continue").message(message).placeholder("Application ID");
    let Ok(answer) = cx.update(|window, cx| overlay::prompt_text(prompt, window, cx)) else { return false };
    let Some(client_id) = answer.await.map(|t| t.trim().to_string()).filter(|t| !t.is_empty()) else { return false };
    let Ok(core) = cx.update(|_, cx| core(cx)) else { return false };
    match core_call(cx, async move { forge_api::forge_set_client_id(&core, kind, host, client_id).await }).await {
        Ok(()) => true,
        Err(error) => {
            let _ = cx.update(|window, cx| overlay::toast_error(error.message, window, cx));
            false
        }
    }
}

/// Alur "Sign Out": pilih akun lalu konfirmasi.
pub fn sign_out(window: &mut Window, cx: &mut App) {
    window
        .spawn(cx, async move |cx| {
            let Ok(core) = cx.update(|_, cx| core(cx)) else { return };
            let list_core = core.clone();
            let accounts = core_call(cx, async move { forge_api::forge_accounts(&list_core).await }).await.unwrap_or_default();
            if accounts.is_empty() {
                let _ = cx.update(|window, cx| overlay::toast_info("You are not signed in to any account.", window, cx));
                return;
            }
            let items = accounts.iter().map(|a| PickItem::new(format!("{} ({})", a.username, forge_name(Some(a.kind))), a.id.clone()).description(a.host.clone())).collect();
            let Some(choice) = pick(cx, QuickPickOptions::new("Sign out of which account?", items)).await else { return };
            sign_out_account(choice.value, cx).await;
        })
        .detach();
}

async fn sign_out_account(account_id: String, cx: &mut AsyncWindowContext) {
    let Ok(confirmed) = cx.update(|window, cx| overlay::confirm(Confirm::new("Sign out?", "The token is removed from this computer. You can sign in again at any time.", "Sign Out"), window, cx)) else { return };
    if !confirmed.await {
        return;
    }
    let Ok(core) = cx.update(|_, cx| core(cx)) else { return };
    if let Err(error) = core_call(cx, async move { forge_api::forge_logout(&core, account_id).await }).await {
        let _ = cx.update(|window, cx| overlay::show_error(&error, window, cx));
    }
}

/// Daftar akun untuk halaman pengaturan (dan sidebar): akun yang masuk, tombol masuk/keluar.
pub struct AccountsView {
    accounts: Vec<Account>,
    memory_only: bool,
    _load: Task<()>,
    _subscription: Subscription,
}

impl AccountsView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let subscription = cx.subscribe(&core_bus(cx), |this: &mut Self, _, event: &CoreEvent, cx| {
            if matches!(event, CoreEvent::AccountsChanged) {
                this.reload(cx);
            }
        });
        let memory_only = core(cx).tokens.is_memory_only();
        let mut this = Self { accounts: Vec::new(), memory_only, _load: Task::ready(()), _subscription: subscription };
        this.reload(cx);
        this
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        let core = core(cx);
        let task = spawn_core(cx, async move { forge_api::forge_accounts(&core).await.unwrap_or_default() });
        self._load = cx.spawn(async move |this, cx| {
            let accounts = task.await;
            let _ = this.update(cx, |this, cx| {
                this.accounts = accounts;
                cx.notify();
            });
        });
    }

    pub fn accounts(&self) -> &[Account] {
        &self.accounts
    }
}

impl Render for AccountsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        v_flex()
            .id("forge-accounts")
            .w_full()
            .gap_2()
            .text_sm()
            .when(self.accounts.is_empty(), |el| el.child(div().text_color(theme.muted_foreground).child("You are not signed in to GitHub or GitLab.")))
            .children(self.accounts.iter().map(|account| {
                let id = account.id.clone();
                h_flex()
                    .gap_2()
                    .child(Icon::new(if account.kind == ForgeKind::GitHub { Lucide::Github } else { Lucide::GitMerge }).small())
                    .child(div().flex_1().child(format!("{} — {}", account.username, account.host)))
                    .child(Button::new(SharedString::from(format!("forge-signout-{}", account.id))).outline().xsmall().label("Sign Out").on_click(move |_, window, cx| {
                        let id = id.clone();
                        window.spawn(cx, async move |cx| sign_out_account(id, cx).await).detach();
                    }))
            }))
            .when(self.memory_only, |el| {
                el.child(div().text_xs().text_color(theme.warning).child("The system keychain is not available, so sign-ins last only until OnCode closes."))
            })
            .child(h_flex().child(Button::new("forge-signin").small().primary().icon(Icon::new(Lucide::LogIn)).label("Sign In…").on_click(|_, window, cx| sign_in(window, cx))))
    }
}

/// Akun yang cocok untuk host repositori, untuk dipakai alur lain.
pub fn account_for_host<'a>(accounts: &'a [Account], kind: ForgeKind, host: Option<&str>) -> Option<&'a Account> {
    let wanted = login_host_for(kind, host)?;
    accounts.iter().find(|account| account.kind == kind && account.host.eq_ignore_ascii_case(&wanted))
}

/// Jeda kecil di antara polling uji; dipakai agar `DevicePoller` tetap satu-satunya sumber waktu.
pub const _POLL_GUARD: Duration = Duration::from_millis(1);

/// Entity daftar akun baru.
pub fn accounts_view(cx: &mut App) -> Entity<AccountsView> {
    cx.new(AccountsView::new)
}
