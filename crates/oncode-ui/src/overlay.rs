//! Pembantu dialog dan pemberitahuan: konfirmasi, masukan teks, toast, dan penampil galat.

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::component::button::{Button, ButtonVariant, ButtonVariants as _};
use gpui_kit::component::dialog::DialogFooter;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::WindowExt as _;
use gpui_kit::{div, App, AppContext as _, ParentElement as _, SharedString, Styled as _, Task, Window};
use oncode_core::AppError;
use tokio::sync::oneshot;

pub fn toast_info(message: impl Into<SharedString>, window: &mut Window, cx: &mut App) {
    window.push_notification(Notification::info(message), cx);
}

pub fn toast_success(message: impl Into<SharedString>, window: &mut Window, cx: &mut App) {
    window.push_notification(Notification::success(message), cx);
}

pub fn toast_warning(message: impl Into<SharedString>, window: &mut Window, cx: &mut App) {
    window.push_notification(Notification::warning(message), cx);
}

pub fn toast_error(message: impl Into<SharedString>, window: &mut Window, cx: &mut App) {
    window.push_notification(Notification::error(message).autohide(false), cx);
}

/// Menampilkan galat backend sebagai toast; `detail` (mis. stderr git) ikut ditampilkan bila ada.
pub fn show_error(error: &AppError, window: &mut Window, cx: &mut App) {
    let message = match &error.detail {
        Some(detail) if !detail.trim().is_empty() && detail.trim() != error.message.trim() => format!("{}\n{}", error.message, detail.trim()),
        _ => error.message.clone(),
    };
    toast_error(message, window, cx);
}

pub struct Confirm {
    pub title: SharedString,
    pub message: SharedString,
    pub ok: SharedString,
    /// Tombol konfirmasi berwarna bahaya (untuk aksi yang menghilangkan pekerjaan).
    pub danger: bool,
}

impl Confirm {
    pub fn new(title: impl Into<SharedString>, message: impl Into<SharedString>, ok: impl Into<SharedString>) -> Self {
        Self { title: title.into(), message: message.into(), ok: ok.into(), danger: false }
    }

    pub fn danger(mut self) -> Self {
        self.danger = true;
        self
    }
}

/// Dialog konfirmasi. Hasil `true` hanya bila pengguna menekan tombol konfirmasi.
pub fn confirm(options: Confirm, window: &mut Window, cx: &mut App) -> Task<bool> {
    let (tx, rx) = oneshot::channel::<bool>();
    let tx = Rc::new(Cell::new(Some(tx)));
    let Confirm { title, message, ok, danger } = options;
    window.open_alert_dialog(cx, move |dialog, _, _| {
        let (ok_tx, cancel_tx) = (tx.clone(), tx.clone());
        dialog
            .title(title.clone())
            .description(message.clone())
            .confirm()
            .ok_text(ok.clone())
            .ok_variant(if danger { ButtonVariant::Danger } else { ButtonVariant::Primary })
            .on_ok(move |_, _, _| {
                if let Some(tx) = ok_tx.take() {
                    let _ = tx.send(true);
                }
                true
            })
            .on_cancel(move |_, _, _| {
                if let Some(tx) = cancel_tx.take() {
                    let _ = tx.send(false);
                }
                true
            })
    });
    cx.spawn(async move |_| rx.await.unwrap_or(false))
}

pub struct TextPrompt {
    pub title: SharedString,
    pub message: Option<SharedString>,
    pub initial: SharedString,
    pub placeholder: SharedString,
    pub ok: SharedString,
    /// Masukan disembunyikan (password, token).
    pub secret: bool,
}

impl TextPrompt {
    pub fn new(title: impl Into<SharedString>, ok: impl Into<SharedString>) -> Self {
        Self { title: title.into(), message: None, initial: "".into(), placeholder: "".into(), ok: ok.into(), secret: false }
    }

    pub fn message(mut self, message: impl Into<SharedString>) -> Self {
        self.message = Some(message.into());
        self
    }

    pub fn initial(mut self, initial: impl Into<SharedString>) -> Self {
        self.initial = initial.into();
        self
    }

    pub fn placeholder(mut self, placeholder: impl Into<SharedString>) -> Self {
        self.placeholder = placeholder.into();
        self
    }

    pub fn secret(mut self) -> Self {
        self.secret = true;
        self
    }
}

/// Dialog masukan satu baris. `None` bila dibatalkan.
pub fn prompt_text(options: TextPrompt, window: &mut Window, cx: &mut App) -> Task<Option<String>> {
    let (tx, rx) = oneshot::channel::<Option<String>>();
    let tx = Rc::new(Cell::new(Some(tx)));
    let TextPrompt { title, message, initial, placeholder, ok, secret } = options;
    let input = cx.new(|cx| {
        let mut state = InputState::new(window, cx).placeholder(placeholder).default_value(initial);
        if secret {
            state = state.masked(true);
        }
        state
    });
    let focus = input.clone();
    window.open_alert_dialog(cx, move |dialog, _, _| {
        let (ok_tx, cancel_tx) = (tx.clone(), tx.clone());
        let value = input.clone();
        dialog
            .title(title.clone())
            .when_some_description(message.clone())
            .child(Input::new(&input))
            .confirm()
            .ok_text(ok.clone())
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

trait DescriptionExt {
    fn when_some_description(self, message: Option<SharedString>) -> Self;
}

impl DescriptionExt for gpui_kit::component::dialog::AlertDialog {
    fn when_some_description(self, message: Option<SharedString>) -> Self {
        match message {
            Some(message) => self.description(message),
            None => self,
        }
    }
}

#[derive(Clone)]
pub struct Choice {
    pub id: &'static str,
    pub label: SharedString,
    pub variant: Option<ButtonVariant>,
}

impl Choice {
    pub fn new(id: &'static str, label: impl Into<SharedString>) -> Self {
        Self { id, label: label.into(), variant: None }
    }

    pub fn primary(mut self) -> Self {
        self.variant = Some(ButtonVariant::Primary);
        self
    }

    pub fn danger(mut self) -> Self {
        self.variant = Some(ButtonVariant::Danger);
        self
    }
}

/// Dialog dengan beberapa tombol pilihan (mis. Cancel / Don't Save / Save).
/// Mengembalikan id tombol yang ditekan, atau `None` bila dialog ditutup tanpa memilih.
pub fn choose(title: impl Into<SharedString>, message: impl Into<SharedString>, choices: Vec<Choice>, window: &mut Window, cx: &mut App) -> Task<Option<&'static str>> {
    let (tx, rx) = oneshot::channel::<&'static str>();
    let tx = Rc::new(Cell::new(Some(tx)));
    let (title, message) = (title.into(), message.into());
    window.open_dialog(cx, move |dialog, _, _| {
        let mut footer = DialogFooter::new();
        for choice in &choices {
            let tx = tx.clone();
            let id = choice.id;
            let mut button = Button::new(SharedString::from(format!("choice-{id}"))).label(choice.label.clone());
            button = match choice.variant {
                Some(variant) => button.with_variant(variant),
                None => button.outline(),
            };
            footer = footer.child(button.on_click(move |_, window, cx| {
                if let Some(tx) = tx.take() {
                    let _ = tx.send(id);
                }
                window.close_dialog(cx);
            }));
        }
        dialog.title(title.clone()).close_button(false).overlay_closable(false).child(div().text_sm().child(message.clone())).footer(footer)
    });
    cx.spawn(async move |_| rx.await.ok())
}
