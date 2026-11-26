//! Replacement: open files are changed through their editor (so undo works and the
//! "unsaved" indicator is correct), the rest are written by the backend.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui_kit::component::input::{EditorState, Rope, RopeExt as _};
use gpui_kit::{AsyncWindowContext, Context, Entity, Window};
use oncode_core::api::fs;
use oncode_core::AppError;
use oncode_ui::editor::{buffers, Buffer};
use oncode_ui::{spawn_core, Env};
use oncode_workspace::{OpenedFile, SearchQuery};

use crate::matcher::{apply_edits, compute_edits, LineMatcher, MatchKey, MatchOptions, Pick, TextEdit};
use crate::model::ReplaceOutcome;

/// Which matches in a file get replaced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Picked {
    All,
    /// All except those the user dismissed.
    Except(Vec<MatchKey>),
    /// Only this one match.
    Only(MatchKey),
}

impl Picked {
    fn as_pick(&self) -> Pick<'_> {
        match self {
            Picked::All => Pick::All,
            Picked::Except(dismissed) => Pick::Except(dismissed),
            Picked::Only(key) => Pick::Only(key),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Target {
    /// Path relative to the project folder, with forward slashes.
    pub path: String,
    pub picked: Picked,
}

pub(crate) struct Performed {
    pub outcome: ReplaceOutcome,
    /// The text inserted by the first replacement in the last file processed here
    /// (used to update the list after a single match is replaced).
    pub inserted: Option<String>,
}

/// Absolute path of a search result, using the platform's separator.
pub(crate) fn absolute_path(root: &Path, relative: &str) -> String {
    let mut path = PathBuf::from(root);
    path.extend(relative.split('/').filter(|part| !part.is_empty()));
    path.to_string_lossy().into_owned()
}

struct OpenBuffer {
    buffer: Entity<Buffer>,
    state: Entity<EditorState>,
    snapshot: Rope,
    was_dirty: bool,
    read_only: bool,
}

/// The buffer for `path` if it is ready and has an editor; otherwise the file is considered closed.
fn open_buffer(env: &Env, path: &str, cx: &gpui_kit::App) -> Option<OpenBuffer> {
    let workbench = env.workbench.upgrade()?;
    let registry = buffers(workbench.read(cx))?;
    let registry = registry.read(cx);
    let buffer = registry.get(path).or_else(|| registry.all().into_iter().find(|buffer| Path::new(buffer.read(cx).path()) == Path::new(path)))?;
    let (state, was_dirty, read_only) = {
        let buffer = buffer.read(cx);
        if !buffer.is_ready() {
            return None;
        }
        (buffer.primary()?, buffer.is_dirty(), buffer.is_read_only())
    };
    let snapshot = state.read(cx).text().clone();
    Some(OpenBuffer { buffer, state, snapshot, was_dirty, read_only })
}

/// Applies all edits as ONE change on the editor (the buffer propagates it to the
/// other editors), so a single undo reverts them all. Returns the number of replacements.
fn apply_to_editor(state: &mut EditorState, edits: &[TextEdit], window: &mut Window, cx: &mut Context<EditorState>) -> usize {
    let (Some(first), Some(last)) = (edits.first(), edits.last()) else { return 0 };
    let text = state.text().clone();
    let span = first.range.start..last.range.end;
    if span.end > text.len() {
        return 0;
    }
    // The new text for the range from the first match to the last; the parts in between are copied as they are.
    let mut new_text = String::new();
    let mut at = span.start;
    for edit in edits {
        new_text.push_str(&text.slice(at..edit.range.start).to_string());
        new_text.push_str(&edit.insert);
        at = edit.range.end;
    }
    let range = lsp_types::Range { start: text.offset_to_position(span.start), end: text.offset_to_position(span.end) };
    state.apply_lsp_edits(&vec![lsp_types::TextEdit { range, new_text }], window, cx);
    edits.len()
}

/// For a closed file where only some of the matches are replaced: the backend can only replace
/// the whole file content, so the file is read, changed, and saved again with its original encoding.
async fn replace_on_disk(env: &Env, path: String, matcher: Arc<LineMatcher>, replacement: String, interpolate: bool, picked: Picked, cx: &mut AsyncWindowContext) -> Result<(usize, Option<String>), AppError> {
    let (core, label) = (env.core.clone(), env.window.to_string());
    let task = cx.update(|_, cx| {
        spawn_core(cx, async move {
            let OpenedFile::Text(file) = fs::fs_open(&core, &label, path.clone()).await? else { return Ok((0, None)) };
            let edits = compute_edits(&matcher, &file.text, &replacement, interpolate, picked.as_pick());
            if edits.is_empty() {
                return Ok((0, None));
            }
            let text = apply_edits(&file.text, &edits);
            fs::fs_save(&core, &label, path, text, file.encoding, file.bom, file.eol).await?;
            Ok((edits.len(), edits.first().map(|edit| edit.insert.clone())))
        })
    });
    match task {
        Ok(task) => task.await,
        Err(_) => Err(AppError::new("io", "The window was closed")),
    }
}

/// Performs the replacement for `targets` (search results still displayed) without confirmation
/// or notification.
pub(crate) async fn perform(env: &Env, query: SearchQuery, replacement: String, targets: Vec<Target>, cx: &mut AsyncWindowContext) -> Performed {
    let mut outcome = ReplaceOutcome::default();
    let mut inserted = None;
    let matcher = match LineMatcher::new(&MatchOptions::from(&query)) {
        Ok(matcher) => Arc::new(matcher),
        Err(reason) => {
            outcome.errors.push(reason);
            return Performed { outcome, inserted };
        }
    };
    let Some(root) = env.root.clone() else { return Performed { outcome, inserted } };
    let interpolate = query.is_regex;
    let mut backend: Vec<String> = Vec::new();

    for target in targets {
        let path = absolute_path(&root, &target.path);
        let open = cx.update(|_, cx| open_buffer(env, &path, cx)).ok().flatten();
        match open {
            Some(open) if open.read_only => outcome.skipped_read_only.push(target.path),
            Some(open) => {
                // Matches are computed off the main thread from a copy of the editor content.
                let edits = cx
                    .background_executor()
                    .spawn({
                        let (matcher, replacement, picked, snapshot) = (matcher.clone(), replacement.clone(), target.picked.clone(), open.snapshot.clone());
                        async move { compute_edits(&matcher, &snapshot.to_string(), &replacement, interpolate, picked.as_pick()) }
                    })
                    .await;
                let applied = cx.update(|window, cx| {
                    open.state.update(cx, |state, cx| {
                        // The content changed while computing (the user is typing): recompute from the current content.
                        let edits = if *state.text() == open.snapshot { edits } else { compute_edits(&matcher, &state.text().to_string(), &replacement, interpolate, target.picked.as_pick()) };
                        (apply_to_editor(state, &edits, window, cx), edits.first().map(|edit| edit.insert.clone()))
                    })
                });
                let Ok((count, first)) = applied else { continue };
                if !outcome.count(count) {
                    continue;
                }
                inserted = first;
                // A buffer that was clean is saved right away (equivalent to a file written by the
                // backend). A buffer that already contains other changes is not saved without the user's permission.
                if open.was_dirty {
                    outcome.unsaved += 1;
                } else {
                    let saved = match cx.update(|window, cx| open.buffer.update(cx, |buffer, cx| buffer.save(window, cx))) {
                        Ok(task) => task.await,
                        Err(_) => false,
                    };
                    if !saved {
                        outcome.unsaved += 1;
                    }
                }
            }
            None if target.picked == Picked::All => backend.push(target.path),
            None => match replace_on_disk(env, path, matcher.clone(), replacement.clone(), interpolate, target.picked, cx).await {
                Ok((count, first)) => {
                    if outcome.count(count) {
                        inserted = first;
                    }
                }
                Err(error) => outcome.errors.push(format!("{}: {}", target.path, error.message)),
            },
        }
    }

    if !backend.is_empty() {
        let (core, label) = (env.core.clone(), env.window.to_string());
        let task = cx.update(|_, cx| spawn_core(cx, async move { fs::search_replace(&core, &label, query, replacement, backend).await }));
        if let Ok(task) = task {
            match task.await {
                Ok(summary) => {
                    outcome.replacements += summary.replacements as usize;
                    outcome.files += summary.files as usize;
                }
                Err(error) => outcome.errors.push(error.message),
            }
        }
    }
    Performed { outcome, inserted }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn result_paths_are_joined_onto_the_project_root() {
        let root = Path::new(if cfg!(windows) { "C:\\project" } else { "/project" });
        assert_eq!(Path::new(&absolute_path(root, "src/main.rs")), root.join("src").join("main.rs"));
        assert_eq!(Path::new(&absolute_path(root, "a.txt")), root.join("a.txt"));
    }
}
