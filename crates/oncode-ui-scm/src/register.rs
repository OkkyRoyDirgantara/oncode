//! Pendaftaran fitur Source Control ke setiap jendela: sidebar, panel Git Output, item status
//! bar, dan command (id dan pintasan sama dengan v0.2).

use std::path::PathBuf;
use std::sync::Arc;

use gpui_kit::{App, AppContext as _, AsyncWindowContext, Context, PathPromptOptions, Window};
use oncode_core::api;
use oncode_git::{DiffSource, MergeMode, OperationAction, PushOptions};
use oncode_ui::commands::Command;
use oncode_ui::editor::active_file;
use oncode_ui::overlay::{self, TextPrompt};
use oncode_ui::{spawn_core, Lucide, PanelView, Shell, SidebarView, StatusAlign, StatusItem, Workbench};
use serde_json::Value;

use crate::actions::{self, Git, StashRequest, GIT_OUTPUT_PANEL, SCM_SIDEBAR};
use crate::logic;
use crate::output::{self, GitOutputPanel};
use crate::status::{BranchStatus, SyncStatus};
use crate::store::ScmStore;
use crate::view::ScmPanel;

/// Command yang bekerja pada repositori jendela ini.
fn repo_command(id: &'static str, title: &'static str, flow: fn(Git, &mut Window, &mut App)) -> Command {
    Command::new(id, title, move |_: &Value, window: &mut Window, cx: &mut App| match Git::for_window(window, cx) {
        Some(git) => flow(git, window, cx),
        None => overlay::toast_info("This folder is not a Git repository.", window, cx),
    })
    .category("Git")
}

fn run(git: Git, window: &mut Window, cx: &mut App, flow: impl AsyncFnOnce(Git, &mut AsyncWindowContext) + 'static) {
    actions::spawn(window, cx, async move |cx| flow(git, cx).await);
}

pub(crate) fn setup(workbench: &mut Workbench, window: &mut Window, cx: &mut Context<Workbench>) {
    let env = workbench.env().clone();
    // Mode terbatas: Git dinonaktifkan sepenuhnya.
    if env.root.is_none() || !env.trusted {
        workbench.register_command(Command::new("git.clone", "Clone Repository…", |_, window, cx| clone_flow(window, cx)).category("Git"));
        return;
    }
    let store = cx.new(|cx| ScmStore::new(env.clone(), window, cx));
    workbench.set_extension(store.clone());

    let badge_store = store.clone();
    workbench.register_sidebar(
        SidebarView::new(SCM_SIDEBAR, "Source Control", Lucide::GitBranch, 30, {
            let store = store.clone();
            move |env, window, cx| {
                let (env, store) = (env.clone(), store.clone());
                cx.new(|cx| ScmPanel::new(env, store, window, cx)).into()
            }
        })
        .badge(move |cx| badge_store.read(cx).snapshot(cx).map(|s| logic::change_count(&s))),
    );
    workbench.register_panel(PanelView::new(GIT_OUTPUT_PANEL, "Git Output", 30, |_, _, cx| cx.new(GitOutputPanel::new).into()).actions(output::panel_actions));
    if env.repo_root.is_some() {
        let branch_store = store.clone();
        workbench.register_status(StatusItem::new("scm.branch", StatusAlign::Left, 10, move |_, _, cx| cx.new(|cx| BranchStatus::new(branch_store.clone(), cx)).into()), window, cx);
        let sync_store = store.clone();
        workbench.register_status(StatusItem::new("scm.sync", StatusAlign::Left, 11, move |_, _, cx| cx.new(|cx| SyncStatus::new(sync_store.clone(), cx)).into()), window, cx);
    }

    let weak = cx.weak_entity();
    workbench.register_command(
        Command::new("scm.focus", "Show Source Control", move |_, _, cx| {
            let _ = weak.update(cx, |workbench, cx| workbench.show_sidebar(SCM_SIDEBAR, cx));
        })
        .category("View")
        .key("Mod+Shift+G"),
    );
    let focus_store = store.clone();
    let weak = cx.weak_entity();
    workbench.register_command(
        Command::new("git.commit", "Commit…", move |_, _, cx| {
            let _ = weak.update(cx, |workbench, cx| workbench.show_sidebar(SCM_SIDEBAR, cx));
            focus_store.update(cx, |store, cx| store.request_commit_focus(cx));
        })
        .category("Git"),
    );
    let commands = [
        repo_command("git.stageAll", "Stage All Changes", |git, window, cx| run(git, window, cx, async |git, cx| { actions::stage_all(&git, cx).await; })),
        repo_command("git.unstageAll", "Unstage All Changes", |git, window, cx| run(git, window, cx, async |git, cx| { actions::unstage_all(&git, cx).await; })),
        repo_command("git.pull", "Pull", |git, window, cx| run(git, window, cx, async |git, cx| { actions::pull(&git, cx).await; })),
        repo_command("git.push", "Push", |git, window, cx| run(git, window, cx, async |git, cx| { actions::push(&git, PushOptions::default(), cx).await; })),
        repo_command("git.forcePush", "Force Push (with lease)…", |git, window, cx| {
            run(git, window, cx, async |git, cx| {
                actions::push(&git, PushOptions { force_with_lease: true, ..Default::default() }, cx).await;
            })
        }),
        repo_command("git.fetch", "Fetch", |git, window, cx| run(git, window, cx, async |git, cx| { actions::fetch(&git, None, cx).await; })),
        repo_command("git.sync", "Sync (Pull then Push)", |git, window, cx| run(git, window, cx, async |git, cx| { actions::sync(&git, cx).await; })),
        repo_command("git.switchBranch", "Switch Branch…", |git, window, cx| run(git, window, cx, async |git, cx| actions::pick_branch(&git, cx).await)),
        repo_command("git.createBranch", "Create Branch…", |git, window, cx| run(git, window, cx, async |git, cx| actions::create_branch(&git, None, cx).await)),
        repo_command("git.createTag", "Create Tag…", |git, window, cx| run(git, window, cx, async |git, cx| actions::create_tag(&git, None, cx).await)),
        repo_command("git.stash", "Stash Changes…", |git, window, cx| run(git, window, cx, async |git, cx| actions::stash_push(&git, StashRequest::default(), cx).await)),
        repo_command("git.stashPop", "Pop Latest Stash", |git, window, cx| run(git, window, cx, async |git, cx| actions::stash_pop_latest(&git, cx).await)),
        repo_command("git.undoLastCommit", "Undo Last Commit…", |git, window, cx| run(git, window, cx, async |git, cx| actions::undo_last_commit(&git, cx).await)),
        repo_command("git.merge", "Merge Branch into Current…", |git, window, cx| run(git, window, cx, async |git, cx| actions::merge(&git, None, MergeMode::Default, cx).await)),
        repo_command("git.rebase", "Rebase Current Branch onto…", |git, window, cx| run(git, window, cx, async |git, cx| actions::rebase_onto(&git, None, cx).await)),
        repo_command("git.continue", "Continue Merge/Rebase", |git, window, cx| run(git, window, cx, async |git, cx| actions::operation(&git, OperationAction::Continue, cx).await)),
        repo_command("git.abort", "Abort Merge/Rebase…", |git, window, cx| run(git, window, cx, async |git, cx| actions::operation(&git, OperationAction::Abort, cx).await)),
        repo_command("git.addRemote", "Add Remote…", |git, window, cx| run(git, window, cx, async |git, cx| actions::add_remote(&git, cx).await)),
        repo_command("git.removeRemote", "Remove Remote…", |git, window, cx| run(git, window, cx, async |git, cx| actions::remove_remote(&git, None, cx).await)),
        repo_command("git.renameRemote", "Rename Remote…", |git, window, cx| run(git, window, cx, async |git, cx| actions::rename_remote(&git, cx).await)),
        repo_command("git.setRemoteUrl", "Change Remote URL…", |git, window, cx| run(git, window, cx, async |git, cx| actions::set_remote_url(&git, cx).await)),
        Command::new("git.showOutput", "Show Git Output", {
            let weak = cx.weak_entity();
            move |_, _, cx| {
                let _ = weak.update(cx, |workbench, cx| workbench.show_panel(GIT_OUTPUT_PANEL, cx));
            }
        })
        .category("Git"),
        Command::new("git.openChanges", "Open Changes of Active File", {
            let weak = cx.weak_entity();
            move |_, window, cx| {
                let Some(git) = Git::for_window(window, cx) else { return };
                let Some(path) = weak.upgrade().and_then(|w| active_file(w.read(cx), cx)).map(|(_, buffer, _)| buffer.read(cx).path().to_string()) else { return };
                let root = git.root.trim_end_matches(['/', '\\']).to_string();
                let Some(relative) = path.strip_prefix(&root).map(|p| p.trim_start_matches(['/', '\\']).replace('\\', "/")) else {
                    overlay::toast_info("This file is not part of an open Git repository.", window, cx);
                    return;
                };
                git.open_diff(&relative, None, DiffSource::Worktree, false, window, cx);
            }
        })
        .category("Git"),
        Command::new("git.init", "Initialize Repository", |_, window, cx| init_flow(window, cx)).category("Git"),
        Command::new("git.clone", "Clone Repository…", |_, window, cx| clone_flow(window, cx)).category("Git"),
    ];
    for command in commands {
        workbench.register_command(command);
    }
}

/// `git init` di folder proyek, lalu workspace dimuat ulang agar repositorinya terbaca.
fn init_flow(window: &mut Window, cx: &mut App) {
    let Some(workbench) = Workbench::for_window(window, cx) else { return };
    let env = workbench.read(cx).env().clone();
    let Some(root) = env.root_string() else { return };
    let core = env.core.clone();
    let task = spawn_core(cx, async move { api::git::git_init(&core, root).await });
    window
        .spawn(cx, async move |cx| {
            let result = task.await;
            let _ = cx.update(|window, cx| match result {
                Ok(()) => {
                    if let Some(shell) = Shell::for_window(window, cx) {
                        shell.update(cx, |shell, cx| shell.set_trusted(true, window, cx));
                    }
                }
                Err(error) => overlay::show_error(&error, window, cx),
            });
        })
        .detach();
}

/// Clone: URL → folder induk → nama folder, dengan progres; lalu menawarkan untuk membukanya.
fn clone_flow(window: &mut Window, cx: &mut App) {
    let core = oncode_ui::core(cx);
    let url = overlay::prompt_text(TextPrompt::new("Clone Repository", "Next").placeholder("https://github.com/owner/repo.git or git@host:owner/repo.git"), window, cx);
    window
        .spawn(cx, async move |cx| {
            let Some(url) = url.await.map(|u| u.trim().to_string()).filter(|u| !u.is_empty()) else { return };
            if let Some(problem) = logic::validate_clone_url(&url) {
                let _ = cx.update(|window, cx| overlay::toast_error(problem, window, cx));
                return;
            }
            let Ok(paths) = cx.update(|_, cx| cx.prompt_for_paths(PathPromptOptions { files: false, directories: true, multiple: false, prompt: Some("Clone Into".into()) })) else { return };
            let Ok(Ok(Some(paths))) = paths.await else { return };
            let Some(parent) = paths.into_iter().next() else { return };
            let suggested = logic::repo_name_from_url(&url);
            let Ok(name) = cx.update(|window, cx| overlay::prompt_text(TextPrompt::new("Folder name", "Clone").initial(suggested), window, cx)) else { return };
            let Some(name) = name.await.map(|n| n.trim().to_string()) else { return };
            if let Some(problem) = logic::validate_folder_name(&name) {
                let _ = cx.update(|window, cx| overlay::toast_error(problem, window, cx));
                return;
            }
            let _ = cx.update(|window, cx| overlay::toast_info(format!("Cloning {url}…"), window, cx));
            let operation = format!("clone-{}", core.next_id());
            let (core2, parent_text) = (core.clone(), parent.to_string_lossy().into_owned());
            let Ok(task) = cx.update(|_, cx| spawn_core(cx, async move { api::git::git_clone(&core2, url, parent_text, name, true, operation, Arc::new(|_| {})).await })) else { return };
            let result = task.await;
            let _ = cx.update(|window, cx| match result {
                Ok(dest) => {
                    if let Some(shell) = Shell::for_window(window, cx) {
                        shell.update(cx, |shell, cx| shell.open_folder(PathBuf::from(dest), window, cx));
                    }
                }
                Err(error) => overlay::show_error(&error, window, cx),
            });
        })
        .detach();
}
