//! Signature help: typing a trigger character (`(` or `,` for most servers) asks the language server for
//! the signature of the call at the cursor, shown above the cursor by the editor view (see
//! `oncode_ui::editor::SignaturePopup`). While it is shown, every edit and cursor move asks again, so the
//! current parameter follows the cursor and the popup closes when the server says the cursor left the
//! call. Escape closes it too (handled by the editor view).

use gpui_kit::component::input::{EditorState, RopeExt as _};
use gpui_kit::{Context, Entity, Subscription, Task, WeakEntity, Window};
use lsp_types::request::SignatureHelpRequest;
use lsp_types::{
    Documentation, ParameterLabel, SignatureHelp, SignatureHelpContext, SignatureHelpParams, SignatureHelpTriggerKind, TextDocumentIdentifier, TextDocumentPositionParams, Uri,
    WorkDoneProgressParams,
};
use oncode_lsp::Feature;
use oncode_ui::editor::{Buffers, BuffersEvent, SignaturePopup};
use oncode_ui::spawn_core;

use crate::hub::LspHub;

/// How a request was started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Trigger {
    /// Trigger Parameter Hints (Ctrl+Shift+Space).
    Manual,
    /// A trigger character was typed.
    Character(String),
    /// The popup is open and the text or the cursor changed.
    Update,
}

/// Watches one editor for signature help.
pub(crate) struct SignatureWatch {
    hub: WeakEntity<LspHub>,
    buffers: WeakEntity<Buffers>,
    state: WeakEntity<EditorState>,
    language: String,
    path: String,
    uri: Uri,
    /// Text length and cursor at the last change seen.
    last: (usize, usize),
    open: bool,
    request: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl SignatureWatch {
    pub(crate) fn new(hub: WeakEntity<LspHub>, buffers: WeakEntity<Buffers>, state: &Entity<EditorState>, language: String, path: String, uri: Uri, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut subscriptions = vec![cx.observe_in(state, window, |this: &mut Self, state, window, cx| this.changed(&state, window, cx))];
        if let Some(registry) = buffers.upgrade() {
            subscriptions.push(cx.subscribe(&registry, |this: &mut Self, _, event: &BuffersEvent, cx| {
                if matches!(event, BuffersEvent::SignatureChanged) {
                    this.closed_elsewhere(cx);
                }
            }));
        }
        let last = {
            let state = state.read(cx);
            (state.text().len(), state.cursor())
        };
        Self { hub, buffers, state: state.downgrade(), language, path, uri, last, open: false, request: Task::ready(()), _subscriptions: subscriptions }
    }

    pub(crate) fn is_alive(&self) -> bool {
        self.state.upgrade().is_some()
    }

    pub(crate) fn watches(&self, state: &Entity<EditorState>) -> bool {
        self.state.upgrade().is_some_and(|own| own == *state)
    }

    fn changed(&mut self, state: &Entity<EditorState>, window: &mut Window, cx: &mut Context<Self>) {
        let (length, cursor, before) = {
            let state = state.read(cx);
            let cursor = state.cursor();
            (state.text().len(), cursor, cursor.checked_sub(1).and_then(|at| state.text().char_at(at)))
        };
        if (length, cursor) == self.last {
            return;
        }
        let typed = length > self.last.0;
        self.last = (length, cursor);
        if self.open {
            self.request(Trigger::Update, window, cx);
            return;
        }
        let Some(before) = before.filter(|_| typed) else { return };
        let Some(client) = self.hub.upgrade().and_then(|hub| hub.read(cx).server(&self.language).and_then(|server| server.client())) else { return };
        let character = before.to_string();
        if client.signature_trigger_characters().contains(&character) {
            self.request(Trigger::Character(character), window, cx);
        }
    }

    /// Asks the server for the signature at the cursor. Returns `false` if the server cannot answer.
    pub(crate) fn request(&mut self, trigger: Trigger, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let (Some(state), Some(hub)) = (self.state.upgrade(), self.hub.upgrade()) else { return false };
        let (text, cursor) = {
            let state = state.read(cx);
            (state.text().clone(), state.cursor())
        };
        let Some((client, position)) = hub.update(cx, |hub, _| hub.request_context(&self.language, &self.path, &text, cursor)) else { return false };
        if !client.supports(Feature::SignatureHelp) {
            return false;
        }
        let context = SignatureHelpContext {
            trigger_kind: match trigger {
                Trigger::Manual => SignatureHelpTriggerKind::INVOKED,
                Trigger::Character(_) => SignatureHelpTriggerKind::TRIGGER_CHARACTER,
                Trigger::Update => SignatureHelpTriggerKind::CONTENT_CHANGE,
            },
            trigger_character: match &trigger {
                Trigger::Character(character) => Some(character.clone()),
                _ => None,
            },
            is_retrigger: self.open,
            active_signature_help: None,
        };
        let params = SignatureHelpParams {
            context: Some(context),
            text_document_position_params: TextDocumentPositionParams { text_document: TextDocumentIdentifier { uri: self.uri.clone() }, position },
            work_done_progress_params: WorkDoneProgressParams::default(),
        };
        let task = spawn_core(cx, client.request::<SignatureHelpRequest>(params));
        // Replacing the task drops a request still on its way: only the newest answer is shown.
        self.request = cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                // The cursor moved on since this request: the request for the new position decides.
                let current = this.state.upgrade().map(|state| state.read(cx).cursor());
                if current != Some(cursor) {
                    return;
                }
                this.show(result.ok().flatten().and_then(popup_from), cx);
            });
        });
        true
    }

    fn show(&mut self, popup: Option<SignaturePopup>, cx: &mut Context<Self>) {
        self.open = popup.is_some();
        let Some(state) = self.state.upgrade() else { return };
        let id = state.entity_id();
        if let Some(buffers) = self.buffers.upgrade() {
            buffers.update(cx, |buffers, cx| buffers.set_signature(id, popup, cx));
        }
    }

    /// Escape (or anything else outside) closed the popup: stop updating it until the next trigger.
    fn closed_elsewhere(&mut self, cx: &Context<Self>) {
        let shown = self.state.upgrade().zip(self.buffers.upgrade()).is_some_and(|(state, buffers)| buffers.read(cx).signature(state.entity_id()).is_some());
        if !shown {
            self.open = false;
        }
    }
}

fn documentation_text(documentation: &Documentation) -> String {
    match documentation {
        Documentation::String(text) => text.clone(),
        Documentation::MarkupContent(content) => content.value.clone(),
    }
}

/// The byte range of `[start, end)` counted in UTF-16 units within `text`.
fn utf16_range(text: &str, start: u32, end: u32) -> Option<std::ops::Range<usize>> {
    let mut units = 0u32;
    let (mut from, mut to) = (None, None);
    for (index, ch) in text.char_indices() {
        if units == start {
            from = Some(index);
        }
        if units == end {
            to = Some(index);
        }
        units += ch.len_utf16() as u32;
    }
    if units == start {
        from = Some(text.len());
    }
    if units == end {
        to = Some(text.len());
    }
    Some(from?..to?)
}

/// What the editor shows for a server's signature help; `None` when there is no signature.
pub(crate) fn popup_from(help: SignatureHelp) -> Option<SignaturePopup> {
    let count = help.signatures.len();
    if count == 0 {
        return None;
    }
    let index = (help.active_signature.unwrap_or(0) as usize).min(count - 1);
    let signature = &help.signatures[index];
    let parameter_index = signature.active_parameter.or(help.active_parameter).unwrap_or(0) as usize;
    let parameter = signature.parameters.as_ref().and_then(|parameters| parameters.get(parameter_index));
    let label = signature.label.clone();
    let active = parameter.and_then(|parameter| match &parameter.label {
        ParameterLabel::Simple(name) => {
            // Searched after the opening parenthesis, so a parameter named like the function is not taken for it.
            let open = label.find('(').map_or(0, |at| at + 1);
            label[open..].find(name.as_str()).map(|at| open + at..open + at + name.len())
        }
        ParameterLabel::LabelOffsets([start, end]) => utf16_range(&label, *start, *end),
    });
    let documentation = parameter.and_then(|parameter| parameter.documentation.as_ref()).or(signature.documentation.as_ref()).map(documentation_text);
    Some(SignaturePopup { label, active, documentation, overload: (count > 1).then_some((index, count)) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_types::{ParameterInformation, SignatureInformation};

    fn signature(label: &str, parameters: Vec<ParameterLabel>) -> SignatureInformation {
        SignatureInformation {
            label: label.into(),
            documentation: Some(Documentation::String("Copies a file".into())),
            parameters: Some(parameters.into_iter().map(|label| ParameterInformation { label, documentation: None }).collect()),
            active_parameter: None,
        }
    }

    #[test]
    fn highlights_the_active_parameter_by_name_or_offsets() {
        let help = SignatureHelp { signatures: vec![signature("copy(from, to)", vec![ParameterLabel::Simple("from".into()), ParameterLabel::Simple("to".into())])], active_signature: None, active_parameter: Some(1) };
        let popup = popup_from(help).unwrap();
        assert_eq!(&popup.label[popup.active.clone().unwrap()], "to");
        assert_eq!(popup.documentation.as_deref(), Some("Copies a file"));
        assert_eq!(popup.overload, None);

        // Offsets in UTF-16 units, after a non-ASCII character.
        let help = SignatureHelp { signatures: vec![signature("é(a, b)", vec![ParameterLabel::LabelOffsets([2, 3]), ParameterLabel::LabelOffsets([5, 6])])], active_signature: None, active_parameter: Some(1) };
        let popup = popup_from(help).unwrap();
        assert_eq!(&popup.label[popup.active.clone().unwrap()], "b");
    }

    #[test]
    fn overloads_and_empty_answers() {
        let help = SignatureHelp { signatures: vec![signature("f()", vec![]), signature("f(x)", vec![ParameterLabel::Simple("x".into())])], active_signature: Some(1), active_parameter: Some(0) };
        let popup = popup_from(help).unwrap();
        assert_eq!((popup.label.as_str(), popup.overload), ("f(x)", Some((1, 2))));
        assert_eq!(popup_from(SignatureHelp { signatures: vec![], active_signature: None, active_parameter: None }), None);
    }
}
