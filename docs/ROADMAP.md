# OnCode Roadmap: After v0.2

| | |
|---|---|
| **Status** | Implemented (all phases), not released yet. Where the code differs from the design below is listed in [section 12](#12-implementation-notes) |
| **Date** | November 27, 2026 |
| **Builds on** | [Design v1.0 (native)](DESIGN.md), which is implemented and released as v0.2.0 |
| **Scope** | The features planned after v0.2, in phases, with a design for each feature |

The native rewrite reached parity with the Tauri version, and every open issue is closed. Most of the gaps that remain are not in Git, which is OnCode's main feature. They are **basic editor actions that people use every day**: toggle comment, `Ctrl+D`, duplicate line, and Go to Symbol. This document plans those first, then reliability, then the next Git features, pull requests, and the terminal.

Code references use a file path and a symbol name, not line numbers, so they stay valid while the code moves. They were checked against the tree on the date above.

## Table of Contents

1. [Summary](#1-summary)
2. [Shared Constraints](#2-shared-constraints)
3. [Phase 0: Fixes Found During Research](#3-phase-0-fixes-found-during-research)
4. [Phase 1: Editor Basics](#4-phase-1-editor-basics)
5. [Phase 2: Reliability](#5-phase-2-reliability)
6. [Phase 3: Git](#6-phase-3-git)
7. [Phase 4: Pull Requests](#7-phase-4-pull-requests)
8. [Phase 5: Terminal](#8-phase-5-terminal)
9. [Not Planned, or Needs the Project Owner](#9-not-planned-or-needs-the-project-owner)
10. [Open Questions](#10-open-questions)
11. [Changes to the Design Document](#11-changes-to-the-design-document)
12. [Implementation Notes](#12-implementation-notes)

---

## 1. Summary

Sizes: **S** is a day or less of work including tests, **M** is a few days, **L** is a week or more.

| Phase | Contents | Size | Target release |
|---|---|---|---|
| **P0 Fixes** | Bugs and dead code found while researching this document | S each | v0.3 |
| **P1 Editor basics** | Toggle comment; multi-cursor commands (`Ctrl+D`, select all occurrences, add cursor above/below); duplicate and delete line; Go to Symbol in file and in workspace; signature help | M–L in total | v0.3 |
| **P2 Reliability** | Hot exit; `.editorconfig`; change end of line and encoding | L in total | v0.3 |
| **P3 Git** | Graph search by hash, author, and date; line history (`git log -L`); bisect; blame column; drag a branch onto a branch; drag to reorder in interactive rebase; small extras | L in total | v0.4 |
| **P4 Pull requests** | Changed files and diff in the PR tab; merge from the app; inline review comments | L in total | v0.5 |
| **P5 Terminal** | Mouse reporting to TUI programs; IME; inline find bar | M in total | v0.5 |

The order is the order of work. P0 comes first because P2 builds on one of its fixes (the single save pipeline). P1 and P2 are what users notice every day. **P3, P4, and P5 do not depend on one another** and can be done in any order; small items such as terminal mouse reporting can be pulled forward.

---

## 2. Shared Constraints

These apply to every feature below.

**The editor component.** The editor is `EditorState`, which is gpui-base's `InputBaseState<EditorMode>` re-exported through gpui-component 0.10.0. Three of its limits shape most of the editor designs:

- Its atomic multi-range edit (`replace_text_in_ranges`) and its undo transactions are `pub(crate)`, so OnCode cannot call them. The public `apply_lsp_edits` applies edits one after another, each against the text the previous edit produced. **Rule: send one edit that covers the whole block of lines**, as `move_lines` in `crates/oncode-ui/src/editor/mod.rs` already does (pure function in `editor/lines.rs`, then one `apply_lsp_edits` and one `set_selected_range`). One edit is also one undo step. When several edits are unavoidable, sort them from the end of the file backwards, as `apply_workspace_edit` in `crates/oncode-ui-lsp/src/edits.rs` does.
- Only the first selection can be read (`selected_range`), and `set_selected_range` removes the extra cursors. `add_selection` adds a selection and keeps the first one first. There is no public way to read or replace all selections.
- Popovers anchored at the caret are `pub(crate)`, and the `Lsp` struct has no slot for signature help.

If a feature is truly blocked by these limits, the fallback is a patched copy of gpui-base through `[patch.crates-io]`. It is not needed for anything in this document.

**Shortcuts.** OnCode commands catch keys in `intercept_keystrokes` (`crates/oncode-ui/src/commands.rs`) before GPUI's own key bindings, so an OnCode binding wins over a component binding. Default keys follow **VS Code's defaults per platform**, so people who switch keep their muscle memory. Every new command gets a command id in the same style as the existing ones, so it can be rebound in `keybindings.json`, and appears in the command palette and the relevant menu.

**Drawing at a text position.** `FileView::line_hint_layer` (`crates/oncode-ui/src/editor/view.rs`) draws inline blame in a `canvas` laid over the editor, positioned after the editor's prepaint through `line_end`, which uses `bounds_for_range`. The public `range_to_bounds` gives the pixel bounds of any byte range. The signature help popup and the blame column both reuse this pattern.

**Tests.** Every feature gets headless view tests (`#[gpui_kit::test]`) against the real backend, as in [DEVELOPMENT.md](DEVELOPMENT.md). LSP features extend the mock server in `crates/oncode-lsp/src/mock.rs`. Forge features add `wiremock` fixtures in the test file that needs them; there is no shared mock module. Tests never wait on GPUI timers.

---

## 3. Phase 0: Fixes Found During Research

All of these are size S.

| Problem | Where | Fix |
|---|---|---|
| **"File History" in the Source Control row menu shows the wrong file.** The menu sends `[root, path]`, but the `git.fileHistory` command ignores its arguments and shows the active editor's file | Menu: `common_menu` in `crates/oncode-ui-scm/src/view.rs`. Command: `file_history_of_active` in `crates/oncode-ui-graph/src/lib.rs` | Read `root` and `path` from the arguments; fall back to the active file only when there are none |
| **Format on Save is skipped** by Save All, by "Save" in the close-tab prompt, and by both auto-save modes. Only `file.save` goes through `save_with_format` | `save_with_format` in `crates/oncode-ui/src/editor/format.rs`; `file.saveAll` in `editor/mod.rs`; `FileTab::can_close` in `editor/view.rs`; auto-save in `editor/buffer.rs` and `editor/mod.rs` | **One save pipeline:** a single "before save" step in `Buffer::save_checked` that edits the `EditorState` first (the way `apply_formatted` does), so every save path formats. `.editorconfig` ([5.2](#52-editorconfig)) hooks into the same step |
| **Merge state is fetched but never shown** in the PR tab, and HTTP 405 (GitHub's answer when a PR cannot be merged) falls through to "Unexpected response" | `PullRequestTab::render` in `crates/oncode-ui-forge/src/views.rs` (the unused `mergeable_text` is in `logic.rs`); `status_error` in `crates/oncode-forge/src/http.rs` | Show the merge state line; map 405 to a clear message. Both are needed for [7.2](#72-merge-from-the-app) anyway |
| **Git Output records only stderr.** Commands whose result is on stdout show an empty entry | The logger call in `crates/oncode-git/src/runner.rs` | Record stdout for write commands too (trimmed and capped). Bisect ([6.3](#63-bisect)) depends on this |
| **Terminal dead code.** `FlowControl` and `term_set_paused` are never used, so PTY back-pressure is never applied; `Emulator::flush_sync` is never called, so a synchronized update that a program never closes is not ended at its deadline | `crates/oncode-ui-terminal/src/flow.rs`, `emulator.rs`; `crates/oncode-core/src/api/term.rs` | Wire both in, or delete them. Wiring is preferred: a program that floods output (`cat` of a huge file) should not freeze the UI |
| **Doc comments describe things that do not exist**: `key_to_bytes` says ordinary text "arrives through the text input/IME path", and the terminal mentions a find bar | `crates/oncode-ui-terminal/src/keys.rs` | Correct them now; [8.2](#82-ime) and [8.3](#83-inline-find-bar) make them true later |

---

## 4. Phase 1: Editor Basics

### 4.1 Toggle line and block comment

**What.** `Ctrl+/` (`Cmd+/` on macOS) comments or uncomments the selected lines with the language's line comment. `Shift+Alt+A` wraps or unwraps the selection in the block comment. If every non-blank selected line is already commented, the command uncomments; otherwise it comments all of them, at the smallest indentation of the block, as VS Code does.

**Design.**

- No comment tokens exist anywhere today: the component's `LanguageConfig` has only brackets, auto-closing pairs, and indentation rules, and its grammar config has none either. Add `line_comment: Option<&'static str>` and `block_comment: Option<(&'static str, &'static str)>` to `LanguageInfo` in `crates/oncode-ui/src/editor/language.rs`, and fill them for every language in the table (`jsonc` gets `//`; plain `json` gets none, and the command does nothing).
- Pure functions `toggle_line_comment` and `toggle_block_comment` in `editor/lines.rs` return the same shape as `move_lines`: one replacement for the whole line block plus the new selection.
- Commands `editor.toggleLineComment` and `editor.toggleBlockComment`, registered like `editor.moveLineUp` (`with_file`, `when(has_text)`, `keys_when(editor_focused)`), added to the Edit menu.

**Limits.** With several cursors the command applies to the first selection only ([section 2](#2-shared-constraints)). Files with embedded languages (`<script>` in HTML or Vue, code blocks in Markdown) use the file's own language; using the syntax tree to find the inner language is left for later.

**Tests.** Unit tests for the pure functions (indentation, blank lines, mixed commented and uncommented lines, CRLF, a selection ending at column 0 of the next line); a view test that toggles twice, gets the original text back, and undoes each toggle in one step.

**Size.** M.

### 4.2 Multi-cursor commands

**What.**

| Command | Key | Today |
|---|---|---|
| `editor.addCursorAbove` / `editor.addCursorBelow` | Linux `Shift+Alt+↑/↓`, Windows `Ctrl+Alt+↑/↓`, macOS `Cmd+Alt+↑/↓` | Already works through the component's own bindings, but has no command, palette entry, or menu item |
| `editor.addSelectionToNextFindMatch` | `Ctrl+D` / `Cmd+D` | Missing |
| `editor.selectHighlights` (select all occurrences) | `Ctrl+Shift+L` / `Cmd+Shift+L` | Missing |

**Design.**

- **Add cursor above/below:** commands that dispatch the component's `AddCursorAbove` / `AddCursorBelow` actions (from `gpui_kit::base::input`, not re-exported by gpui-component), the same way `editor.goToDefinition` dispatches an action.
- **`Ctrl+D`:** with an empty selection, select the word under the cursor (`RopeExt::word_range`); later presses search whole-word and case-sensitive. With a non-empty selection, search the selected text literally. Each press finds the next occurrence after the last one added, wrapping at the end of the file, and calls `add_selection`. Because all selections cannot be read, a small tracker per editor remembers the last range added and the text revision it was added at; it resets when `selected_range` changes on its own (a click, or typing).
- **`Ctrl+Shift+L`:** find every occurrence, `set_selected_range` the first, `add_selection` the rest. Capped (e.g. 10,000) with a toast when the cap is hit.

**Tests.** View tests: `Ctrl+D` three times then typing replaces three occurrences; the tracker resets after a click; `Ctrl+Shift+L` on a word with occurrences inside other words selects only whole words.

**Size.** S for add cursor; M for `Ctrl+D` and select all.

### 4.3 Duplicate and delete line

**What.** `editor.copyLinesDown` / `editor.copyLinesUp` duplicate the lines touched by the selection; `editor.deleteLines` deletes them.

| Command | Linux | Windows | macOS |
|---|---|---|---|
| Copy lines down/up | `Ctrl+Shift+Alt+↓/↑` | `Shift+Alt+↓/↑` | `Shift+Option+↓/↑` |
| Delete lines | `Ctrl+Shift+K` | `Ctrl+Shift+K` | `Cmd+Shift+K` |

These are VS Code's defaults. On Linux, `Shift+Alt+↑/↓` is add-cursor in both VS Code and the editor component, so copy lines takes `Ctrl+Shift+Alt` there and nothing conflicts.

**Design.** Pure `duplicate_lines` and `delete_lines` in `editor/lines.rs`, registered like move line, added to the Edit menu (the Selection section).

**Tests.** Unit tests (last line without a trailing newline, CRLF, selection across lines); a view test for each command including one undo step.

**Size.** S.

### 4.4 Go to Symbol

**What.** `Ctrl+Shift+O` lists the symbols of the current file; moving through the list previews the location in the editor, and Enter jumps there. `Ctrl+T` searches symbols across the workspace. Both work without a language server through a simpler fallback.

**Design.**

- **LSP:** `crates/oncode-lsp` has neither `textDocument/documentSymbol` nor `workspace/symbol`. Add both to the `Feature` enum, the client capabilities (hierarchical document symbols supported), and `server_supports` in `capabilities.rs`. Requests use the generic typed `request`.
- **UI:** commands in `crates/oncode-ui-lsp/src/lib.rs`, modeled on `find_references` (request through the hub, then a quick pick). The file list uses `QuickPickOptions::on_highlight` for the live preview, shows nesting as indentation, and the symbol kind as the description. The workspace list uses `QuickPickOptions::with_search`, sends the query to every running server (`LspHub::servers`), and merges the results.
- **Fallback without a server:** the "Go to Definition without a language server" extension already finds definitions with a keyword regex in `crates/oncode-workspace/src/symbols.rs` (`definition_pattern`, `score`). Generalize it to capture any defined name: over the current buffer's text for the file list, and through the project search for the workspace list. Results are labeled "(text match)" so users know they are approximate.
- **Later upgrade:** a tree-sitter outline. The grammar crates in the lockfile ship tags queries (`TAGS_QUERY`) for Rust, Python, JavaScript, TypeScript, Go, and Java; using them needs a direct dependency on `tree-sitter`. Not part of this phase.

**Tests.** Extend the mock LSP server with both methods; view tests for both lists with a server, and for the fallback without one.

**Size.** M for each list, including the fallback.

### 4.5 Signature help

**What.** Typing `(` or `,` in a call shows the function's signature above the caret, with the current parameter highlighted. It closes when the caret leaves the call or on Escape. `Ctrl+Shift+Space` shows it on demand. Today this is listed as missing in [DESIGN.md §15.4](DESIGN.md#154-different-from-v02-or-not-yet-done).

**Design.**

- Already present: the client capability, `Feature::SignatureHelp`, `signature_trigger_characters` in `crates/oncode-lsp/src/client.rs`, and mock server support.
- **Trigger:** in `LspHub::attach` (`crates/oncode-ui-lsp/src/hub.rs`), subscribe to the editor's edit events (as `editor/buffer.rs` does) and send `textDocument/signatureHelp` when a trigger character is typed. Results that arrive after the caret moved on are dropped.
- **Display:** a new extension point next to `LineHintProvider` in `editor/buffer.rs`: a caret popup per path (content as Markdown, the active parameter's range). `FileView` renders it with `deferred(anchored(...))` above the caret, placed by `range_to_bounds` of the caret. Above, because the completion menu opens below.
- **Closing:** observe the editor state and close when the caret leaves the call's parentheses or the file changes; Escape is caught with `commands::capture_keys`.
- Not used: the component's public `present_hover` could show the text, but a mouse hover replaces it immediately.

**Tests.** View tests with the mock server: typing `(` shows the popup with the right active parameter; `,` moves it; Escape and moving out close it; a late reply after the caret moved is ignored.

**Size.** M–L.

---

## 5. Phase 2: Reliability

### 5.1 Hot exit

**What.** Closing a window or quitting with unsaved changes does not ask; the changes, including untitled files, are kept and come back the next time the folder opens. A new setting `files.hotExit` (`on` / `off`, default `on`, as in VS Code) turns it off; when off, today's prompt stays. Today this is listed as out of scope in [DESIGN.md §2.4](DESIGN.md#24-still-out-of-scope).

**Design.**

- **Where backups go:** `Dirs::data_dir` / `backups/<hash of the project root>/`, one file per buffer plus a small index (path or untitled id, encoding, line ending, and the file's modification time and size when the backup was made). **Never `state.json`:** `Store` rewrites the whole file on every change, and `session_set` refuses sessions over 512 KB.
- **When:** written with a debounce (about 1 s) from `Buffer::on_changed` while hot exit is on, and once more, synchronously, on quit. A backup is deleted when its buffer is saved or reverted.
- **Session:** `SessionData` in `crates/oncode-ui/src/shell.rs` skips untitled tabs today. It gains untitled entries and the backup id of each dirty tab.
- **Close and quit:** with hot exit on, `confirm_close` writes the backups and closes without the per-tab prompt (`close_all_tabs(true)`). Closing a single tab still asks, as in VS Code.
- **Restore:** `restore_session` opens each tab and, when a backup exists, a new `Buffer::restore(text, base_stat)` loads the backup text as a dirty buffer on top of the file. If the file changed on disk after the backup was made, the existing external-change flow asks which version to keep.
- **Dependency:** the last folder is **not reopened at launch** today; only a folder given on the command line is opened. Without reopening, hot exit restores nothing until the user opens the folder again. See [open questions](#10-open-questions).

**Tests.** View tests: edit, close the window, reopen the folder, and the text is back and dirty; an untitled file comes back; saving deletes the backup; a file changed on disk in between triggers the conflict flow; with `files.hotExit: off` the prompt appears as before.

**Size.** L (M more if launch also reopens the last folders).

### 5.2 `.editorconfig`

**What.** Projects that ship an `.editorconfig` get its indentation, line endings, charset, trailing-whitespace trimming, and final newline, per file, overriding the user's settings. Today this is listed as out of scope in [DESIGN.md §2.4](DESIGN.md#24-still-out-of-scope).

**Design.**

- **Parsing:** the `ec4rs` crate, in `oncode-core`. New API `editor_overrides(label, path)` that walks up from the file until `root = true` or the workspace root, cached per directory and dropped when an `.editorconfig` changes (the watcher already reports file changes).
- **Applying**, per property:

| Property | Where |
|---|---|
| `indent_style`, `indent_size`, `tab_width` | `FileView::apply_settings` (per path, on top of the window's settings), and the formatting options the LSP format request sends (`crates/oncode-ui-lsp/src/lib.rs`), which today use the window's tab size |
| `end_of_line`, `charset` | New and untitled files only. Existing files keep their detected line ending and encoding (DESIGN.md: "encoding and line endings preserved") |
| `trim_trailing_whitespace`, `insert_final_newline` | The single "before save" step from [Phase 0](#3-phase-0-fixes-found-during-research), so they apply to every save path |

- **Restricted mode:** `.editorconfig` cannot choose programs or run code, so it applies in untrusted folders too.

**Tests.** Unit tests for the overrides lookup (nested sections, `root = true`); view tests: a Go file gets tabs and a Python file four spaces in the same project; trailing whitespace is trimmed through Save, Save All, and auto-save.

**Size.** M.

### 5.3 Change end of line and encoding

**What.** Clicking the line-ending or encoding item in the status bar, or the commands `editor.changeEol` and `editor.changeEncoding`, chooses LF or CRLF, and "Reopen with Encoding" or "Save with Encoding". Today the status bar only shows them.

**Design.** A quick pick from the status bar items in `editor/mod.rs`; the line ending and encoding are already stored on the `Buffer` and converted back when writing (`crates/oncode-workspace/src/fs.rs`), so this only changes the stored values and marks the buffer dirty. Reopen with an encoding reads the file again.

**Size.** S.

---

## 6. Phase 3: Git

### 6.1 Graph search by hash, author, and date

**What.** The graph search box finds commits by message today (`--grep`, case-insensitive, literal). It also finds a commit by hash, and new filter controls choose an author and a date range.

**Design.**

- **Author and dates:** `LogQuery` already has `author`, `since`, and `until`, and `log_args` already turns them into arguments (`crates/oncode-git/src/model.rs`, `log.rs`), but `GraphTab::reload` passes an empty author and no dates. Add an author field and a date dropdown (Any time, Last 7 days, Last 30 days, Last year, Custom… where custom takes git's own date text such as `2 weeks ago`). gpui-component 0.10 has no date picker.
- **Hash:** when the input is 4–40 hex characters, resolve it with `rev_parse` (in `crates/oncode-git/src/ops.rs`; it needs a core API) and reveal it with the existing `GraphTab::reveal`, which loads pages until the commit appears. On very large repositories, cap the pages loaded while hunting and offer to open the commit's details directly when the cap is reached. If the input is not a commit, fall back to the message search.
- The unused helpers `scope_label` and `scope_exists` in `crates/oncode-ui-graph/src/query.rs` are either used by this work or deleted.

**Tests.** View tests: hash search reveals and selects an old commit; author and date filters narrow the rows.

**Size.** S.

### 6.2 Line history

**What.** Select lines in the editor, then **Git: Show Line History** (command palette, or the editor's right-click menu) lists only the commits that changed those lines (`git log -L`).

**Design.**

- **Query:** `lines: Option<(u32, u32)>` in `LogQuery`, counted in `is_filtered`. In `log_args`, emit `-L <start>,<end>:<path>` with `--no-patch` instead of `--follow` and the path, starting from HEAD only. Check that `--no-patch` suppresses the `-L` patch on the minimum Git version (2.40, `MIN_GIT_VERSION` in `runner.rs`).
- **Command:** `git.lineHistory` reads the selection (`selected_range`, converted to lines) and opens a history tab like `open_file_history` in `crates/oncode-ui-graph/src/lib.rs`, with the range in the tab id (`lineHistory:{root}:{path}:{start}-{end}`).
- **Dirty buffers:** `-L` counts lines in HEAD. When the buffer has unsaved or uncommitted changes, map buffer lines to HEAD lines with `LineDiff` (`crates/oncode-ui-diff/src/lines.rs`); `ChangeMarkers` already keeps the base text.
- **Right-click menu:** the editor's `EditorState::on_context_menu` hook is unused today. It hosts "Show Line History", and later "Toggle Blame" and the multi-cursor commands.
- **Viewing a commit:** reuses the commit details and per-file diff that file history already uses, scrolled to the range.

**Tests.** Backend test against real git (a file with three commits, only two touching the selected lines); view test from selection to the tab.

**Size.** M.

### 6.3 Bisect

**What.** From the graph's commit menu, **Start Bisect** (with this commit as bad and a chosen commit as good). While bisecting, the Source Control panel shows a banner like the merge and rebase banners, with **Good**, **Bad**, **Skip**, and **Reset** buttons, the number of steps left, and finally the first bad commit with a button to show it in the graph. The graph labels the commits already marked good and bad. Today this is listed as out of scope in [DESIGN.md §2.4](DESIGN.md#24-still-out-of-scope).

**Design.**

- **Backend:** ops `bisect_start`, `bisect_mark(good | bad | skip)`, and `bisect_reset` in `crates/oncode-git/src/ops.rs`, built with `Cmd` and `.write()` like `cherry_pick`. Parse stdout for "Bisecting: N revisions left" and "is the first bad commit" and return them; they also go to Git Output (needs the [Phase 0](#3-phase-0-fixes-found-during-research) stdout fix).
- **State:** `Operation::Bisect` in `detect_operation` (`crates/oncode-git/src/status.rs`), from `BISECT_START` (which holds the original branch), checked after the rebase and merge checks because a bisect can contain a cherry-pick. `RefKind::Bisect` in `parse_refs` (`log.rs`) for `refs/bisect/*`, which is dropped today. The watcher's operation markers (`crates/oncode-workspace/src/watcher.rs`) gain the `BISECT_*` files.
- **UI:** the banner extends `render_operation` and `operation_text` in `crates/oncode-ui-scm`; `git.abort` maps to reset. Menu items in `commit_menu` (`crates/oncode-ui-graph/src/menus.rs`): Start Bisect, Mark Good, Mark Bad, Skip, enabled according to `snapshot.operation`. Commands `git.bisectStart`, `git.bisectGood`, `git.bisectBad`, `git.bisectSkip`, `git.bisectReset`.
- Every `match` on `Operation` (the SCM logic and `Repo::operation`) gets the new variant.

**Tests.** Backend test against real git with a known first bad commit; view test from Start to the result banner, and Reset returning to the original branch.

**Size.** M.

### 6.4 Blame column

**What.** **Git: Toggle Blame** shows a column to the left of the line numbers with the author, date, and commit summary of every line, grouped so a run of lines from one commit shows its text once. Clicking an entry opens the commit. Today blame is shown only for the cursor line ([DESIGN.md §15.4](DESIGN.md#154-different-from-v02-or-not-yet-done)).

**Design.**

- **No new Git work:** `ChangeMarkers` (`crates/oncode-ui-diff/src/markers.rs`) already loads whole-file blame for inline blame and reloads it 600 ms after edits.
- **Drawing:** the component's gutter decorations are color bars without text, and it has no inlay or block API. The column is a sibling element to the left of the `Editor` in `FileView`, the same width for the whole file, with each visible row placed by `bounds_for_range` (which handles soft wrap and folds) using the same canvas pattern as `line_hint_layer`. It scrolls with the editor because it is laid out from the editor's own positions on every frame.
- `git.toggleBlame` switches between the column and today's status-bar-only mode; the setting is remembered per window.

**Tests.** View test: the column shows one entry per run of lines and updates after an edit; clicking opens the commit.

**Size.** M–L.

### 6.5 Drag a branch onto a branch

**What.** In the graph, drag a branch label (from the sidebar or a commit row) onto another branch; a small menu offers **Merge X into Y** and **Rebase X onto Y**. Today this is listed as out of scope in [DESIGN.md §2.4](DESIGN.md#24-still-out-of-scope).

**Design.** A `DraggedRef { name, kind }` payload with `on_drag` on the sidebar items and the row badges (the badges need element ids), and `drag_over` plus `on_drop` on the drop targets, following `DraggedTab` and `TabDragPreview` in `crates/oncode-ui/src/workbench.rs`. Merge and rebase act on the current branch (`GitAction::Merge`, `RebaseOnto`), so when Y is not checked out the menu says it will switch to Y first, and the switch goes through the normal checkout path, which already refuses or asks when there are local changes.

**Tests.** View test: drag `feature` onto `main`, choose Merge, and `main` gains the merge commit.

**Size.** M.

### 6.6 Drag to reorder in interactive rebase

**What.** Rows in the interactive rebase dialog can be dragged to a new position. The up and down buttons stay for keyboard use. Today reordering uses only the buttons ([DESIGN.md §15.4](DESIGN.md#154-different-from-v02-or-not-yet-done)).

**Design.** Each row already has an id (`rebase-item-{index}` in `crates/oncode-ui-graph/src/rebase_dialog.rs`). Add `on_drag` with a row index payload, `drag_over` for the drop highlight, and `on_drop` calling `rebase_plan::move_item(from, to)`, like `TabDrop::Before` for tabs.

**Size.** S.

### 6.7 Small Git extras

Found during research; each is S unless noted.

- **Sign commits with GPG/SSH:** the backend already has `CommitOptions.gpg_sign` (adds `-S`), but the commit box never sets it; add a checkbox next to Signed-off-by. A signing program that asks for a passphrase in the terminal cannot work (Git runs with no terminal), so the checkbox's tooltip says a GUI pinentry or agent is needed. Showing signature status in the graph (`%G?`) is a separate, later item.
- **Git LFS files in diffs:** blobs are read with `cat-file blob`, so an LFS file's diff shows the pointer text. Read with `--filters` (or `--textconv`) so the real content is compared (`crates/oncode-git/src/content.rs`).
- **Compare a branch with the current branch:** a branch menu item that diffs merge-base..branch, using the unused `git_merge_base` core API, plus the ahead/behind commit lists. M.

---

## 7. Phase 4: Pull Requests

### 7.1 Changed files and diff in the PR tab

**What.** The PR tab lists the commits and changed files and opens their diff. **This is a v0.2 parity gap:** the v0.2 design lists "commit list, and changed files ... displayed with the same diff viewer" ([archive §7.6](archive/DESIGN-v0.2-tauri.md#76-github-and-gitlab-integration)), but the native tab shows only the counts. Helpers for it are still in `crates/oncode-ui-forge/src/logic.rs`, unused (`change_letter`, `base_ref_for`).

**Design.** Fetch the PR head without switching branches, into a hidden ref (reusing the refspec that `forge_pr_checkout` in `crates/oncode-core/src/api/forge.rs` uses: `refs/pull/<n>/head` or `refs/merge-requests/<n>/head`). Diff merge-base..head locally with `git_merge_base` and `git_diff_files`, and open diffs with `DiffSource::Range`, as the graph's compare view does (`crates/oncode-ui-graph/src/graph_tab.rs`). Local diffing avoids the forges' truncated `patch` fields and works the same for GitHub and GitLab. Requires a trusted folder (it runs `git fetch`).

**Tests.** UI test against the existing smart-HTTP test server plus wiremock for the API.

**Size.** M.

### 7.2 Merge from the app

**What.** A **Merge** section in the PR tab: method (merge commit, squash, rebase, limited to what the repository allows), title and message for squash and merge commits, "delete branch after merge", and the current state (ready, checks failing, conflicts, behind, draft, still computing). A confirmation dialog, then merge, then the tab and list refresh. Today this is listed as out of scope in [DESIGN.md §2.4](DESIGN.md#24-still-out-of-scope).

**Design.**

- `crates/oncode-forge/src/http.rs` has only `get` and `post`; add `put`. Map 405 ([Phase 0](#3-phase-0-fixes-found-during-research)).
- A `MergeState { mergeable, state, allowed_methods }` in `model.rs`. GitHub: `mergeable` and `mergeable_state` from the pull, `allow_merge_commit`, `allow_squash_merge`, and `allow_rebase_merge` from the repository. GitLab: `detailed_merge_status`, and the project's `merge_method` and `squash_option`.
- `merge_pull_request(repo, number, MergeInput { method, sha, title, message, delete_branch })` on the `ForgeProvider` trait (`provider.rs`). GitHub: `PUT /repos/{owner}/{repo}/pulls/{n}/merge`, then delete the branch if asked. GitLab: `PUT /projects/:id/merge_requests/:iid/merge` with `squash`, `sha`, and `should_remove_source_branch`. Passing `sha` makes the merge fail if someone pushed in the meantime.
- Both forges compute mergeability lazily; while it is `null` the tab polls a few times.
- **Scopes:** the current scopes (`repo` on GitHub, `api` on GitLab) cover merging; no new login is needed. Not yet verified: whether GitHub requires the `workflow` scope to merge a PR that changes `.github/workflows/`. If it does, the error message says so.

**Tests.** Provider tests with wiremock for both forges (success, 405 not mergeable, 409 head changed); a UI test for the dialog.

**Size.** M.

### 7.3 Inline review comments

**What.** In the PR's diff ([7.1](#71-changed-files-and-diff-in-the-pr-tab)), existing review comments appear under the lines they belong to; selecting lines opens a comment box; comments collect in a pending review that is submitted as **Comment**, **Approve**, or **Request changes**. Comments whose lines no longer exist are listed separately as outdated. Today this is listed as out of scope in [DESIGN.md §2.4](DESIGN.md#24-still-out-of-scope).

**Design.**

- **The diff component already has what the UI needs:** `DiffAnnotation::line` renders content under a source line (OnCode already uses it for the per-change action bars in `crates/oncode-ui-diff/src/diff_tab.rs`), gutter markers, multi-file patches, and a line-range selection event (`DiffEvent::SelectionEnded`).
- **Reading:** GitHub `GET /pulls/{n}/comments` (`path`, `line`, `side`, `start_line`, `in_reply_to_id`) and the reviews list, which needs `GhReview` to keep `state` and `body` (it keeps only `user` today). GitLab `GET /merge_requests/:iid/discussions` (`position.new_line` / `old_line`, `new_path` / `old_path`), which needs the MR's `diff_refs`.
- **Writing:** GitHub `POST /pulls/{n}/reviews` with `commit_id`, `comments[]`, and `event`. GitLab draft notes plus bulk publish, and `POST /approve`.
- Replies to a thread, and resolving threads, are part of this item; editing and deleting comments are not.

**Tests.** Provider tests with wiremock for each endpoint; a UI test that adds two comments and submits an approval.

**Size.** L.

---

## 8. Phase 5: Terminal

### 8.1 Mouse reporting

**What.** TUI programs that ask for the mouse (htop, lazygit, vim and neovim with `mouse=a`, tmux) receive clicks, drags, and motion. Holding Shift keeps selecting text locally, as in other terminals. Today only the scroll wheel is reported ([DESIGN.md §15.4](DESIGN.md#154-different-from-v02-or-not-yet-done)).

**Design.** The encoder already exists and is tested: `mouse_report` in `crates/oncode-ui-terminal/src/mouse.rs` encodes press, release, and motion in normal, UTF-8, and SGR forms, and `on_scroll` in `view.rs` already reads the terminal's mouse mode for the wheel. What is missing is in `on_mouse_down`, `on_mouse_move`, and mouse-up in `view.rs`: when the program enabled mouse mode and Shift is not held, map the position to a cell and report press and release (`MOUSE_REPORT_CLICK`), motion with a button held (`MOUSE_DRAG`), or all motion (`MOUSE_MOTION`), sending each cell once. Bind the middle and right buttons too. Ctrl+click on a link still opens it. Focus in/out reporting (`FOCUS_IN_OUT`) is added at the same time.

**Tests.** Feed `ESC[?1000h ESC[?1006h` through the emulator, click, and check the bytes written to the PTY; Shift+drag selects instead.

**Size.** S.

### 8.2 IME

**What.** Input methods for Chinese, Japanese, Korean, and others work in the terminal: the composition (preedit) text is shown underlined at the cursor, and the candidate window appears next to it. Today text comes only from `key_char` ([DESIGN.md §15.4](DESIGN.md#154-different-from-v02-or-not-yet-done)).

**Design.** Implement GPUI's `EntityInputHandler` for `TerminalSession`, with a `preedit: Option<String>` field: committed text is written to the PTY (and clears the selection and scroll position, like typing); marked text is stored and drawn; `bounds_for_range` returns the cursor cell, so the candidate window is placed there. Register it with `window.handle_input` in the terminal's paint closure, as gpui-base's editor does in its element. `on_key_down` stops writing `key_char` for plain text and stops consuming those events, so GPUI routes them to the handler. Control keys and app shortcuts keep going through `key_to_bytes`.

**Tests.** gpui-kit's test helpers (`compose_text`, `commit_text`, `unmark_text`) for composition; existing typing tests keep working because `TestWindow::input` dispatches through the same path.

**Size.** M.

### 8.3 Inline find bar

**What.** `Ctrl+F` in the terminal opens a find bar at the top right of the terminal instead of a dialog: incremental search, case, regex, and whole-word toggles, "3 of 17", all matches highlighted, Enter / Shift+Enter / F3 for next and previous, Escape to close and return focus.

**Design.** A find-bar element owned by the session, overlaid on the grid. Matches come from alacritty's `RegexIter`, capped (scrollback is 10,000 lines), and are drawn from a new `matches` list beside `selections` in the grid frame. New output invalidates the matches. gpui-component's editor `SearchPanel` is `pub(super)`, so its layout is copied, not reused. Replaces `TerminalSession::find`, which uses `prompt_text` today.

**Size.** M.

---

## 9. Not Planned, or Needs the Project Owner

| Item | Why |
|---|---|
| **Automatic updates** | Nothing exists today: no update check, manifest, or signing key, and the release workflow publishes a draft. It needs a signing key owned by the project, a step that publishes the manifest after the draft is released, and per-format handling (a `.deb` in `/usr/bin` cannot replace itself without root, so it would only notify). Size L once the key exists |
| **Code signing and notarization** | Need certificates owned by the project (Apple Developer ID, a Windows code-signing certificate) |
| **Editable diff view** | Stays out, as decided in [DESIGN.md §2.3](DESIGN.md#23-intentional-differences-from-v02); "Open File" covers it |
| **Comment toggling inside embedded languages**, tree-sitter outline, signature status in the graph | Later upgrades of items in this document, noted in their sections |
| **Extension marketplace, debugger, remote development, notebooks, settings sync, AI assistant in the editor** | Non-goals, unchanged ([DESIGN.md §1.3](DESIGN.md#13-non-goals)) |

---

## 10. Open Questions

Decisions for the project owner. Each has a recommendation; the designs above assume it, and the implementation followed the recommendations (all of them can be changed in Settings, except the Linux keys, which can be rebound in `keybindings.json`).

| Question | Recommendation |
|---|---|
| Should OnCode reopen the last folder(s) at launch when started without a path? Hot exit restores nothing without it | Yes: a new setting `window.restoreFolders` (`last` / `none`), default `last`, as VS Code does |
| Hot exit on by default? | Yes (`files.hotExit: on`), as in VS Code |
| Copy lines on Linux: VS Code's `Ctrl+Shift+Alt+↑/↓`, or take `Shift+Alt+↑/↓` from add-cursor? | VS Code's keys; nothing conflicts and switchers keep their habits |
| Patch gpui-base to read and set all selections, or accept "first selection only" for toggle comment and similar commands? | Accept it for now; patch only when a feature is blocked, since a patched crate has to be carried through every gpui-kit upgrade |
| Do the target releases in [section 1](#1-summary) fit the release plan? | P0–P2 in v0.3, P3 in v0.4, P4–P5 in v0.5 |

---

## 11. Changes to the Design Document

[DESIGN.md §2.4](DESIGN.md#24-still-out-of-scope) lists several items from this document as out of scope (graph hash search, date filters, dragging a branch onto a branch, bisect, `git log -L`, `.editorconfig`, hot exit, merging pull requests, and inline review comments). That section now points here for them. As each feature lands, its row in [DESIGN.md §15.4](DESIGN.md#154-different-from-v02-or-not-yet-done) is removed or updated and the README's feature list is extended, so the design document keeps describing what the code does and this document keeps describing what is planned.

---

## 12. Implementation Notes

All phases are implemented, with headless view tests (and backend tests against real git and mock servers) for each feature. Where the code differs from the designs above:

| Item | Design | Implementation | Reason |
|---|---|---|---|
| Save pipeline ([P0](#3-phase-0-fixes-found-during-research)) | One "before save" step for every save | As designed, plus two exceptions: auto-save after a delay and Replace in Files (`Buffer::save_as_is`) write the text as it is | Formatting while the user is still typing would move text under the cursor (VS Code does the same); Replace in Files stands for a file written by the backend |
| Go to Symbol ([4.4](#44-go-to-symbol)) | LSP, then a keyword-regex fallback | As designed. The fallback is `oncode_workspace::outline` (regex per line, nesting from indentation, Markdown headings). Command ids `editor.gotoSymbol` and `workbench.gotoSymbol` | — |
| Signature help ([4.5](#45-signature-help)) | Popup above the caret | As designed; the popup is stored per editor in `Buffers` (`SignaturePopup`) and placed from `EditorState::cursor_layout()` | — |
| Restore folders ([5.1](#51-hot-exit), [10](#10-open-questions)) | `window.restoreFolders` | `workbench.restoreFolders` (`last` / `none`, default `last`) | There is no `window` settings section; the setting sits with the other workbench settings |
| `.editorconfig` ([5.2](#52-editorconfig)) | The `ec4rs` crate | Own parser, `oncode_workspace::editorconfig` (globs through `globset`; numeric ranges such as `{1..3}` are not supported) | No new dependency; the needed subset is small |
| Change encoding ([5.3](#53-change-end-of-line-and-encoding)) | Reopen with Encoding discards unsaved changes after asking | Reopen with Encoding refuses while there are unsaved changes | Nothing to lose by accident; the user saves or undoes first |
| Line history ([6.2](#62-line-history)) | Also in the editor's right-click menu | In the Git menu and the command palette only | `EditorState::on_context_menu` replaces the editor's whole menu (Cut, Copy, Paste, …); rebuilding it is separate work |
| Bisect ([6.3](#63-bisect)) | Labels for `refs/bisect/*` | As designed; the Graph log now passes `--clear-decorations` (Git 2.39+, below the 2.40 minimum) because git hides `refs/bisect/*` from decorations by default | — |
| Blame column ([6.4](#64-blame-column)) | A column beside the editor | As designed, through a general "annotation column" extension point in `Buffers` (`AnnotationColumn`); clicking an entry runs the hidden `git.showCommit` command | — |
| Git LFS ([6.7](#67-small-git-extras)) | Real content through filters | As designed (only for showing diffs); per-change and selected-line actions are turned off for such a file, which is staged or discarded as a whole | Its index holds the pointer, not the lines on screen |
| Sign commits ([6.7](#67-small-git-extras)) | A checkbox next to Signed-off-by | An item in the Commit button's menu, next to "Add Signed-off-by" | Signed-off-by is a menu item there too |
| Inline review comments ([7.3](#73-inline-review-comments)) | Comments as annotations inside the diff view | Comments, replies, the pending review, and Comment / Approve / Request Changes are in the PR tab, next to its changed files (which open the diff) | The diff view lives in another crate with no extension point for annotations yet; the PR tab gives the same actions. Showing them inside the diff is a later upgrade |
| PR merge and changed files ([7.1](#71-changed-files-and-diff-in-the-pr-tab), [7.2](#72-merge-from-the-app)) | — | As designed. The PR head is fetched into `refs/oncode/pr/<n>`; the Graph excludes `refs/oncode/*` | — |
| Restoring tabs at startup ([5.1](#51-hot-exit)) | — | Fixed an older bug that reopening the last folder made common: an editor created before its repository was known (a tab restored with the session) never got change markers or blame. `ChangeMarkers` now keeps such editors and attaches them when the repository's snapshot arrives | Found while checking the app with screenshots |
| Terminal ([8](#8-phase-5-terminal)) | — | As designed. Typed text now always goes through the terminal's input handler (also without an IME); the old dialog search (`Emulator::find`) is replaced by `find_all` | — |

