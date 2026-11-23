//! Panel Git Output: setiap perintah git yang dijalankan aplikasi beserta keluarannya.
//! Riwayatnya global (semua jendela) dan dibatasi [`MAX_ENTRIES`] perintah.

use std::ops::Range;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::{h_flex, ActiveTheme as _, Icon, Sizable as _};
use gpui_kit::{
    div, px, uniform_list, App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable, Global, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, ScrollStrategy, SharedString, Styled as _, Subscription, UniformListScrollHandle, Window,
};
use oncode_core::CoreEvent;
use oncode_git::GitLogEntry;
use oncode_ui::{core_bus, Lucide};

pub const MAX_ENTRIES: usize = 500;
/// Keluaran satu perintah yang lebih panjang dari ini dipotong di tampilan.
const MAX_LINES_PER_ENTRY: usize = 400;
const ROW_HEIGHT: f32 = 18.;

#[derive(Debug, Clone, PartialEq)]
pub struct OutputEntry {
    pub entry: GitLogEntry,
    /// Jam setempat saat perintah selesai, mis. "14:03:27".
    pub time: SharedString,
}

/// Satu baris tampilan: kepala perintah atau satu baris keluarannya.
#[derive(Debug, Clone, PartialEq)]
pub enum OutputLine {
    Command { time: SharedString, command: SharedString, meta: SharedString, failed: bool },
    Text(SharedString),
}

/// Keterangan di belakang perintah: durasi, dan kode keluar bila gagal.
pub fn entry_meta(entry: &GitLogEntry) -> String {
    if entry.exit_code != 0 {
        format!("[{} ms, exit {}]", entry.duration_ms, entry.exit_code)
    } else {
        format!("[{} ms]", entry.duration_ms)
    }
}

/// Meratakan riwayat menjadi baris-baris berketinggian sama untuk daftar virtual.
pub fn flatten(entries: &[OutputEntry]) -> Vec<OutputLine> {
    let mut lines = Vec::new();
    for item in entries {
        lines.push(OutputLine::Command {
            time: item.time.clone(),
            command: format!("> {}", item.entry.command).into(),
            meta: entry_meta(&item.entry).into(),
            failed: item.entry.exit_code != 0,
        });
        let output = item.entry.output.trim_end();
        if output.is_empty() {
            continue;
        }
        let total = output.lines().count();
        for line in output.lines().take(MAX_LINES_PER_ENTRY) {
            // Baris progres git memakai `\r`; hanya keadaan terakhirnya yang berarti.
            let line = line.rsplit('\r').find(|part| !part.is_empty()).unwrap_or("");
            lines.push(OutputLine::Text(line.replace('\t', "    ").into()));
        }
        if total > MAX_LINES_PER_ENTRY {
            lines.push(OutputLine::Text(format!("… {} more lines", total - MAX_LINES_PER_ENTRY).into()));
        }
    }
    lines
}

#[derive(Default)]
pub struct GitOutput {
    entries: Vec<OutputEntry>,
    _subscriptions: Vec<Subscription>,
}

/// Riwayat berubah (perintah baru atau dikosongkan).
pub struct OutputChanged;

impl EventEmitter<OutputChanged> for GitOutput {}

impl GitOutput {
    pub fn entries(&self) -> &[OutputEntry] {
        &self.entries
    }

    pub fn push(&mut self, entry: GitLogEntry, time: SharedString, cx: &mut Context<Self>) {
        if self.entries.len() >= MAX_ENTRIES {
            let excess = self.entries.len() + 1 - MAX_ENTRIES;
            self.entries.drain(..excess);
        }
        self.entries.push(OutputEntry { entry, time });
        cx.emit(OutputChanged);
        cx.notify();
    }

    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.entries.clear();
        cx.emit(OutputChanged);
        cx.notify();
    }
}

struct GitOutputGlobal(Entity<GitOutput>);

impl Global for GitOutputGlobal {}

/// Riwayat perintah git global.
pub fn git_output(cx: &App) -> Entity<GitOutput> {
    cx.global::<GitOutputGlobal>().0.clone()
}

pub(crate) fn install(cx: &mut App) {
    let bus = core_bus(cx);
    let log = cx.new(|cx| {
        let subscription = cx.subscribe(&bus, |this: &mut GitOutput, _, event: &CoreEvent, cx| {
            if let CoreEvent::GitOutput { entry } = event {
                let time = chrono::Local::now().format("%H:%M:%S").to_string();
                this.push(entry.clone(), time.into(), cx);
            }
        });
        GitOutput { entries: Vec::new(), _subscriptions: vec![subscription] }
    });
    cx.set_global(GitOutputGlobal(log));
}

/// Tombol "Clear output" di bilah tab panel.
pub fn panel_actions(_: &mut Window, _: &mut App) -> gpui_kit::AnyElement {
    Button::new("git-output-clear")
        .ghost()
        .xsmall()
        .icon(Icon::new(Lucide::Trash))
        .tooltip("Clear output")
        .on_click(|_, _, cx| git_output(cx).update(cx, |log, cx| log.clear(cx)))
        .into_any_element()
}

pub struct GitOutputPanel {
    lines: Vec<OutputLine>,
    scroll: UniformListScrollHandle,
    focus_handle: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl GitOutputPanel {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let log = git_output(cx);
        let subscription = cx.subscribe(&log, |this: &mut Self, log, _: &OutputChanged, cx| {
            this.lines = flatten(log.read(cx).entries());
            // Perintah terbaru selalu terlihat.
            if !this.lines.is_empty() {
                this.scroll.scroll_to_item(this.lines.len() - 1, ScrollStrategy::Bottom);
            }
            cx.notify();
        });
        let lines = flatten(log.read(cx).entries());
        let scroll = UniformListScrollHandle::new();
        if !lines.is_empty() {
            scroll.scroll_to_item(lines.len() - 1, ScrollStrategy::Bottom);
        }
        Self { lines, scroll, focus_handle: cx.focus_handle(), _subscriptions: vec![subscription] }
    }

    fn render_lines(&mut self, range: Range<usize>, _: &mut Window, cx: &mut Context<Self>) -> Vec<gpui_kit::AnyElement> {
        let theme = cx.theme();
        range
            .filter_map(|index| {
                let row = h_flex().h(px(ROW_HEIGHT)).w_full().px_3().gap_2().whitespace_nowrap();
                Some(match self.lines.get(index)? {
                    OutputLine::Command { time, command, meta, failed } => row
                        .child(div().flex_none().text_color(theme.muted_foreground).child(time.clone()))
                        .child(div().flex_none().text_color(if *failed { theme.danger } else { theme.foreground }).child(command.clone()))
                        .child(div().flex_none().text_color(theme.muted_foreground).child(meta.clone()))
                        .into_any_element(),
                    OutputLine::Text(text) => row.pl_6().text_color(theme.muted_foreground).child(text.clone()).into_any_element(),
                })
            })
            .collect()
    }
}

impl Focusable for GitOutputPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for GitOutputPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let base = div().id("git-output").size_full().track_focus(&self.focus_handle).font_family(theme.mono_font_family.clone()).text_size(px(12.));
        if self.lines.is_empty() {
            return base.flex().items_center().justify_center().text_color(theme.muted_foreground).child("Git commands run by OnCode appear here.");
        }
        base.child(uniform_list("git-output-lines", self.lines.len(), cx.processor(Self::render_lines)).track_scroll(&self.scroll).size_full())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(command: &str, exit_code: i32, output: &str) -> OutputEntry {
        OutputEntry { entry: GitLogEntry { command: command.into(), cwd: "/repo".into(), duration_ms: 12, exit_code, output: output.into() }, time: "10:00:00".into() }
    }

    #[test]
    fn entries_flatten_to_command_and_output_lines() {
        let lines = flatten(&[entry("git add -A -- a.txt", 0, ""), entry("git push", 1, "remote: ditolak\nerror: failed to push\n")]);
        assert_eq!(lines.len(), 4);
        assert_eq!(lines[0], OutputLine::Command { time: "10:00:00".into(), command: "> git add -A -- a.txt".into(), meta: "[12 ms]".into(), failed: false });
        assert_eq!(lines[1], OutputLine::Command { time: "10:00:00".into(), command: "> git push".into(), meta: "[12 ms, exit 1]".into(), failed: true });
        assert_eq!(lines[2], OutputLine::Text("remote: ditolak".into()));
        assert_eq!(lines[3], OutputLine::Text("error: failed to push".into()));
    }

    #[test]
    fn progress_lines_keep_their_last_state_and_long_output_is_capped() {
        let lines = flatten(&[entry("git fetch", 0, "Receiving objects:  10%\rReceiving objects: 100%\rdone.\n\tindented")]);
        assert_eq!(lines[1], OutputLine::Text("done.".into()));
        assert_eq!(lines[2], OutputLine::Text("    indented".into()));

        let long: String = (0..MAX_LINES_PER_ENTRY + 25).map(|i| format!("baris {i}\n")).collect();
        let lines = flatten(&[entry("git log", 0, &long)]);
        assert_eq!(lines.len(), 1 + MAX_LINES_PER_ENTRY + 1);
        assert_eq!(lines.last(), Some(&OutputLine::Text("… 25 more lines".into())));
    }
}
