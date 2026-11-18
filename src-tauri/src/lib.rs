//! Shell Tauri OnCode: merakit state, command, event, dan jendela.

mod commands;
mod error;
mod events;
mod settings;
mod state;
mod store;
mod tokens;

use std::sync::Arc;

use tauri::{Manager, RunEvent, WindowEvent};
use tauri_specta::{collect_commands, collect_events, Builder};

use commands::{app, forge, fs, git, lsp, term};
use state::{AppState, Shared};

/// Seluruh command dan event; dipakai aplikasi dan ekspor binding TypeScript.
fn specta_builder() -> Builder<tauri::Wry> {
    Builder::<tauri::Wry>::new()
        .commands(collect_commands![
            app::app_info,
            app::workspace_open,
            app::workspace_set_trusted,
            app::workspace_close,
            app::workspace_trust_path,
            app::window_open,
            app::recent_remove,
            app::app_cancel,
            app::prompt_respond,
            app::settings_get,
            app::settings_get_user,
            app::settings_set,
            app::settings_defaults,
            app::keybindings_get,
            app::keybindings_ensure,
            app::session_get,
            app::session_set,
            fs::fs_read_dir,
            fs::fs_open,
            fs::fs_save,
            fs::fs_stat,
            fs::fs_create_file,
            fs::fs_create_dir,
            fs::fs_rename,
            fs::fs_trash,
            fs::quick_open,
            fs::search_start,
            fs::search_replace,
            git::git_open,
            git::git_snapshot,
            git::git_refs,
            git::git_init,
            git::git_clone,
            git::git_stage,
            git::git_stage_all,
            git::git_unstage,
            git::git_unstage_all,
            git::git_discard,
            git::git_diff,
            git::git_splice,
            git::git_file_at,
            git::git_commit,
            git::git_undo_commit,
            git::git_last_message,
            git::git_identity,
            git::git_set_identity,
            git::git_switch,
            git::git_switch_detached,
            git::git_switch_tracking,
            git::git_branch_create,
            git::git_branch_rename,
            git::git_branch_delete,
            git::git_branch_unmerged,
            git::git_branch_set_upstream,
            git::git_branch_delete_remote,
            git::git_merge,
            git::git_rebase,
            git::git_rebase_plan,
            git::git_rebase_interactive,
            git::git_operation,
            git::git_fetch,
            git::git_pull,
            git::git_push,
            git::git_remote_add,
            git::git_remote_remove,
            git::git_remote_rename,
            git::git_remote_set_url,
            git::git_log_open,
            git::git_log_next,
            git::git_log_close,
            git::git_log,
            git::git_commit_detail,
            git::git_diff_files,
            git::git_merge_base,
            git::git_reflog,
            git::git_blame,
            git::git_stash_list,
            git::git_stash_push,
            git::git_stash_apply,
            git::git_stash_drop,
            git::git_stash_branch,
            git::git_stash_files,
            git::git_tag_create,
            git::git_tag_delete,
            git::git_tag_push,
            git::git_tag_delete_remote,
            git::git_cherry_pick,
            git::git_revert,
            git::git_reset,
            git::git_conflict,
            git::git_resolve_side,
            git::git_resolve_write,
            git::git_mark_resolved,
            git::git_resolve_delete,
            git::git_worktrees,
            git::git_worktree_add,
            git::git_worktree_remove,
            git::git_worktree_prune,
            git::git_submodules,
            git::git_submodule_update,
            git::git_submodule_sync,
            term::term_spawn,
            term::term_write,
            term::term_resize,
            term::term_set_paused,
            term::term_kill,
            lsp::lsp_servers,
            lsp::lsp_start,
            lsp::lsp_send,
            lsp::lsp_stop,
            forge::forge_accounts,
            forge::forge_login_start,
            forge::forge_login_poll,
            forge::forge_login_cancel,
            forge::forge_login_token,
            forge::forge_logout,
            forge::forge_context,
            forge::forge_repos,
            forge::forge_prs,
            forge::forge_pr,
            forge::forge_checks,
            forge::forge_default_branch,
            forge::forge_pr_create,
            forge::forge_pr_checkout,
        ])
        .events(collect_events![
            events::RepoChanged,
            events::RefsChanged,
            events::FsChanged,
            events::GitOutput,
            events::PromptRequest,
            events::SettingsChanged,
            events::AccountsChanged,
            events::Notice,
        ])
        .typ::<error::AppError>()
        .disable_serde_phases()
        .mocks()
}

/// Menulis `src/ipc/bindings.ts` dari definisi command dan event.
pub fn export_bindings(path: &str) -> Result<(), String> {
    specta_builder()
        .export(specta_typescript::Typescript::default().header("// @ts-nocheck\n// Berkas ini dibuat otomatis oleh tauri-specta. Jangan disunting."), path)
        .map_err(|e| e.to_string())
}

/// Folder yang diminta lewat baris perintah: argumen pertama yang merupakan direktori.
fn folder_argument(args: &[String], cwd: &str) -> Option<String> {
    args.iter().skip(1).filter(|a| !a.starts_with('-')).find_map(|arg| {
        let path = std::path::Path::new(arg);
        let full = if path.is_absolute() { path.to_path_buf() } else { std::path::Path::new(cwd).join(path) };
        full.is_dir().then(|| full.to_string_lossy().into_owned())
    })
}

pub fn run() {
    let builder = specta_builder();

    #[cfg(debug_assertions)]
    if let Err(e) = export_bindings("../src/ipc/bindings.ts") {
        eprintln!("oncode: failed to export TypeScript bindings: {e}");
    }

    let app = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, args, cwd| {
            // Instance kedua: buka jendela baru di proses yang sudah berjalan.
            let folder = folder_argument(&args, &cwd);
            let _ = app::open_window(app, folder.as_deref());
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .invoke_handler(builder.invoke_handler())
        .setup(move |app| {
            builder.mount_events(app);
            let config_dir = app.path().app_config_dir()?;
            let data_dir = app.path().app_data_dir()?;
            let tokens = Arc::new(tokens::FallbackStore::new(Arc::new(oncode_forge::KeychainStore::new())));
            let shared: Shared = Arc::new(AppState::new(app.handle().clone(), config_dir, data_dir, tokens));
            app.manage(shared);

            let args: Vec<String> = std::env::args().collect();
            let cwd = std::env::current_dir().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
            app::open_window(app.handle(), folder_argument(&args, &cwd).as_deref())?;
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::Destroyed = event {
                if let Some(state) = window.try_state::<Shared>() {
                    state.close_workspace(window.label());
                }
            }
        })
        .build(tauri::generate_context!())
        .expect("failed to start OnCode");

    app.run(|handle, event| {
        if let RunEvent::Exit = event {
            if let Some(state) = handle.try_state::<Shared>() {
                state.terminals.kill_all();
                let lsp = state.lsp.clone();
                tauri::async_runtime::block_on(async move { lsp.stop_all().await });
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `cargo test -p oncode export_bindings` memperbarui binding tanpa menjalankan aplikasi.
    #[test]
    fn export_bindings_to_frontend() {
        export_bindings("../src/ipc/bindings.ts").expect("ekspor binding");
        let text = std::fs::read_to_string("../src/ipc/bindings.ts").unwrap();
        assert!(text.contains("gitSnapshot") && text.contains("RepoSnapshot"));
    }

    #[test]
    fn picks_first_directory_argument() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("proyek");
        std::fs::create_dir(&sub).unwrap();
        let cwd = dir.path().to_string_lossy().into_owned();
        let args = vec!["oncode".to_string(), "--flag".to_string(), "tidak-ada".to_string(), "proyek".to_string()];
        assert_eq!(folder_argument(&args, &cwd), Some(sub.to_string_lossy().into_owned()));
        assert_eq!(folder_argument(&["oncode".to_string()], &cwd), None);
    }
}
