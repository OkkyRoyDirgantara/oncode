//! OnCode's entry point: sets up the backend runtime and the application state, then opens the window.

// On Windows, do not open a console window in release builds.
#![cfg_attr(all(not(debug_assertions), target_os = "windows"), windows_subsystem = "windows")]

#[cfg(debug_assertions)]
mod dev;
mod shell_env;
mod single_instance;

use std::path::PathBuf;
use std::sync::Arc;

use oncode_core::{api, Core, Dirs, FallbackStore};
use oncode_ui::OnCode;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let cwd = std::env::current_dir().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    // OnCode is already running (e.g. `oncode .` from a terminal): hand the folder to that process and
    // finish before loading anything.
    let endpoint = single_instance::enabled().then(single_instance::endpoint);
    if endpoint.as_deref().is_some_and(|endpoint| single_instance::forward(endpoint, &single_instance::Request { cwd: cwd.clone(), args: args.clone() })) {
        return;
    }
    // Before a runtime or any other thread is created (see `shell_env`).
    shell_env::import_login_path();
    let folder = api::app::folder_argument(&args, &cwd).map(PathBuf::from);

    // The backend crates use tokio; GPUI has its own executor (see `oncode_ui::app`).
    // Four threads are enough: backend work is mostly waiting on git processes, disk, or the network.
    let runtime = Arc::new(tokio::runtime::Builder::new_multi_thread().worker_threads(4).enable_all().thread_name("oncode-backend").build().expect("failed to create the tokio runtime"));
    let open_requests = endpoint.as_deref().and_then(|endpoint| single_instance::listen(endpoint, &runtime));

    let Some(dirs) = Dirs::default_for_app() else {
        eprintln!("oncode: could not determine the configuration directory");
        std::process::exit(1);
    };
    let log_path = dirs.data_dir.join("oncode.log");
    install_panic_log(log_path.clone());
    let (events_tx, events_rx) = tokio::sync::mpsc::unbounded_channel();
    let tokens = Arc::new(FallbackStore::new(Arc::new(oncode_forge::KeychainStore::new())));
    let core = Core::new(
        runtime.handle().clone(),
        dirs,
        tokens,
        Arc::new(move |event| {
            let _ = events_tx.send(event);
        }),
    );

    // The full icon set: the default assets only contain ± 100 Lucide icons, while the features use more.
    let application = gpui_kit::application().with_assets(oncode_ui::file_icons::Assets);
    // macOS: the Dock icon is clicked while there are no windows.
    application.on_reopen(oncode_ui::app_menu::reopen);
    application.run(move |cx| {
        gpui_kit::init(cx);
        // The last window is closed: the application finishes (macOS stays alive like other applications there).
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() && !cfg!(target_os = "macos") {
                cx.quit();
            }
        })
        .detach();
        // Terminals and language servers are stopped before the process ends.
        let (quit_core, quit_runtime) = (core.clone(), runtime.clone());
        cx.on_app_quit(move |_| {
            let (core, runtime) = (quit_core.clone(), quit_runtime.clone());
            async move {
                let _ = runtime.spawn(async move { core.shutdown().await }).await;
            }
        })
        .detach();
        oncode_ui::init(OnCode { core, runtime, events: Some(events_rx) }, cx);
        // The order determines the command registration order; the order of icons and panels is set via `order`.
        oncode_ui_explorer::init(cx);
        oncode_ui_search::init(cx);
        oncode_ui_diff::init(cx);
        oncode_ui_scm::init(cx);
        oncode_ui_graph::init(cx);
        oncode_ui_terminal::init(cx);
        oncode_ui_lsp::init(cx);
        oncode_ui_forge::init(cx);
        oncode_ui_settings::init(cx);
        oncode_ui_markdown::init(cx);
        cx.activate(true);
        // Without a folder on the command line, the last folder is opened again (and with it, hot exit's unsaved changes).
        let folder = folder.or_else(|| oncode_ui::startup_folder(cx));
        if let Err(error) = oncode_ui::try_open_window(folder, cx) {
            // Without a window, the application would never receive "last window closed";
            // log the cause and then exit so it does not linger as an invisible process.
            write_log(&log_path, &format!("failed to open a window: {error:#}"));
            eprintln!("oncode: failed to open a window: {error:#}");
            cx.quit();
            return;
        }
        if let Some(mut requests) = open_requests {
            cx.spawn(async move |cx| {
                while let Some(request) = requests.recv().await {
                    let folder = api::app::folder_argument(&request.args, &request.cwd).map(PathBuf::from);
                    let _ = cx.update(|cx| oncode_ui::open_from_command_line(folder, cx));
                }
            })
            .detach();
        }
        #[cfg(debug_assertions)]
        dev::run_script(cx);
    });
}

/// Appends a timestamped line to `oncode.log` in the data folder. A GUI application on Windows
/// has no console, so startup errors are only visible through this file.
fn write_log(path: &std::path::Path, message: &str) {
    use std::io::Write as _;
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        let _ = writeln!(file, "[{secs}] {message}");
    }
}

/// A panic is also recorded to `oncode.log`, in addition to being printed as usual.
fn install_panic_log(path: PathBuf) {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        write_log(&path, &format!("panic: {info}\n{}", std::backtrace::Backtrace::force_capture()));
        default_hook(info);
    }));
}
