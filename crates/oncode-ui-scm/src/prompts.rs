//! Dialogs and notifications of the Source Control feature: text input with live validation,
//! and toasts that carry follow-up buttons (e.g. "Pull, then push").

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::component::button::Button;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::notification::{Notification, NotificationType};
use gpui_kit::component::{h_flex, v_flex, ActiveTheme as _, Sizable as _, WindowExt as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::TestSupportExt as _;
use gpui_kit::{
    div, px, App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement, ParentElement as _, Render, SharedString, Styled as _, Subscription,
    Task, Window,
};
use tokio::sync::oneshot;

use crate::logic::NoticeLevel;

type Validate = Rc<dyn Fn(&str) -> Option<String>>;

/// A single-line input dialog with a label and validation.
pub struct InputPrompt {
    pub title: SharedString,
    pub label: SharedString,
    pub value: SharedString,
    pub placeholder: SharedString,
    pub ok: SharedString,
    /// An empty value is accepted (e.g. an optional stash message).
    pub allow_empty: bool,
    pub validate: Option<Validate>,
}

impl InputPrompt {
    pub fn new(title: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        Self { title: title.into(), label: label.into(), value: "".into(), placeholder: "".into(), ok: "OK".into(), allow_empty: false, validate: None }
    }

    pub fn value(mut self, value: impl Into<SharedString>) -> Self {
        self.value = value.into();
        self
    }

    pub fn placeholder(mut self, placeholder: impl Into<SharedString>) -> Self {
        self.placeholder = placeholder.into();
        self
    }

    pub fn ok(mut self, ok: impl Into<SharedString>) -> Self {
        self.ok = ok.into();
        self
    }

    pub fn allow_empty(mut self) -> Self {
        self.allow_empty = true;
        self
    }

    /// A function that returns an error message for an invalid value.
    pub fn validate(mut self, validate: impl Fn(&str) -> Option<String> + 'static) -> Self {
        self.validate = Some(Rc::new(validate));
        self
    }
}

/// Dialog content: the label, the input field, and the validation error message.
struct PromptBody {
    label: SharedString,
    input: Entity<InputState>,
    error: Option<SharedString>,
    allow_empty: bool,
    validate: Option<Validate>,
    _subscription: Subscription,
}

impl PromptBody {
    /// The error for the current value; `None` if it can be submitted.
    fn problem(&self, cx: &App) -> Option<SharedString> {
        let value = self.input.read(cx).value();
        if let Some(validate) = &self.validate {
            if let Some(message) = validate(&value) {
                return Some(message.into());
            }
        } else if !self.allow_empty && value.trim().is_empty() {
            return Some("Enter a value".into());
        }
        None
    }

    /// Checks the value, shows its error, and returns the value if valid.
    fn submit(&mut self, cx: &mut Context<Self>) -> Option<String> {
        self.error = self.problem(cx);
        cx.notify();
        self.error.is_none().then(|| self.input.read(cx).value().to_string())
    }
}

impl Render for PromptBody {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        v_flex()
            .w_full()
            .gap_1()
            .child(div().text_sm().text_color(theme.muted_foreground).child(self.label.clone()))
            .child(Input::new(&self.input).id("scm-prompt-input"))
            .when_some(self.error.clone(), |el, error| el.child(div().id("scm-prompt-error").test_support().text_sm().text_color(theme.danger).child(error)))
    }
}

/// Opens an input dialog. `None` if canceled. A value that fails validation cannot be submitted.
pub fn prompt_input(options: InputPrompt, window: &mut Window, cx: &mut App) -> Task<Option<String>> {
    let (tx, rx) = oneshot::channel::<Option<String>>();
    let tx = Rc::new(Cell::new(Some(tx)));
    let InputPrompt { title, label, value, placeholder, ok, allow_empty, validate } = options;
    let input = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder).default_value(value));
    let body = cx.new(|cx| {
        // The error updates while typing, but only appears after the first keystroke.
        let subscription = cx.subscribe(&input, |this: &mut PromptBody, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                this.error = this.problem(cx);
                cx.notify();
            }
        });
        PromptBody { label, input: input.clone(), error: None, allow_empty, validate, _subscription: subscription }
    });
    let focus = input.clone();
    window.open_alert_dialog(cx, move |dialog, _, _| {
        let (ok_tx, cancel_tx) = (tx.clone(), tx.clone());
        let submit = body.clone();
        dialog
            .title(title.clone())
            .child(body.clone())
            .confirm()
            .ok_text(ok.clone())
            .on_ok(move |_, _, cx| {
                let Some(value) = submit.update(cx, |body, cx| body.submit(cx)) else { return false };
                if let Some(tx) = ok_tx.take() {
                    let _ = tx.send(Some(value));
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

/// A follow-up button on a toast.
#[derive(Clone)]
pub struct ToastAction {
    /// Part of the element id: the button has the id `scm-toast-action-<slug>`.
    pub slug: SharedString,
    pub label: SharedString,
    pub run: Rc<dyn Fn(&mut Window, &mut App)>,
}

impl ToastAction {
    pub fn new(slug: impl Into<SharedString>, label: impl Into<SharedString>, run: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        Self { slug: slug.into(), label: label.into(), run: Rc::new(run) }
    }
}

/// Shows a toast. `key` becomes the marker id (`scm-toast-<key>`) so tests can wait for it.
/// Error toasts and toasts with buttons do not disappear on their own.
pub fn toast(level: NoticeLevel, key: &str, message: impl Into<SharedString>, actions: Vec<ToastAction>, window: &mut Window, cx: &mut App) {
    let kind = match level {
        NoticeLevel::Info => NotificationType::Info,
        NoticeLevel::Warning => NotificationType::Warning,
        NoticeLevel::Error => NotificationType::Error,
    };
    let marker: SharedString = format!("scm-toast-{key}").into();
    let autohide = level != NoticeLevel::Error && actions.is_empty();
    let note = Notification::new().message(message).with_type(kind).autohide(autohide).content(move |_, _, cx| {
        let this = cx.entity().downgrade();
        h_flex()
            .id(marker.clone())
            .test_support()
            .flex_wrap()
            .gap_1()
            .when(!actions.is_empty(), |el| el.pt_1p5())
            .when(actions.is_empty(), |el| el.h(px(1.)))
            .children(actions.iter().map(|action| {
                let (run, this) = (action.run.clone(), this.clone());
                Button::new(SharedString::from(format!("scm-toast-action-{}", action.slug))).outline().xsmall().label(action.label.clone()).on_click(move |_, window, cx| {
                    cx.stop_propagation();
                    let _ = this.update(cx, |note, cx| note.dismiss(window, cx));
                    run(window, cx);
                })
            }))
            .into_any_element()
    });
    window.push_notification(note, cx);
}

pub fn toast_info(key: &str, message: impl Into<SharedString>, window: &mut Window, cx: &mut App) {
    toast(NoticeLevel::Info, key, message, Vec::new(), window, cx);
}
