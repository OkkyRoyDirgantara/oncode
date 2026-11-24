//! Titik masuk OnCode: menyiapkan runtime backend, state aplikasi, lalu membuka jendela.

// Di Windows, jangan buka jendela konsol pada build rilis.
#![cfg_attr(all(not(debug_assertions), target_os = "windows"), windows_subsystem = "windows")]

#[cfg(debug_assertions)]
mod dev;

use std::path::PathBuf;
use std::sync::Arc;

use oncode_core::{api, Core, Dirs, FallbackStore};
use oncode_ui::OnCode;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let cwd = std::env::current_dir().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    let folder = api::app::folder_argument(&args, &cwd).map(PathBuf::from);

    // Crate backend memakai tokio; GPUI punya eksekutornya sendiri (lihat `oncode_ui::app`).
    // Empat thread cukup: pekerjaan backend kebanyakan menunggu proses git, disk, atau jaringan.
    let runtime = Arc::new(tokio::runtime::Builder::new_multi_thread().worker_threads(4).enable_all().thread_name("oncode-backend").build().expect("runtime tokio gagal dibuat"));

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

    // Set ikon lengkap: aset bawaan hanya memuat ± 100 ikon Lucide, sedangkan fitur memakai lebih banyak.
    gpui_kit::application().with_assets(oncode_ui::file_icons::Assets).run(move |cx| {
        gpui_kit::init(cx);
        // Jendela terakhir ditutup: aplikasi selesai (macOS tetap hidup seperti aplikasi lain di sana).
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() && !cfg!(target_os = "macos") {
                cx.quit();
            }
        })
        .detach();
        // Terminal dan language server dihentikan sebelum proses berakhir.
        let (quit_core, quit_runtime) = (core.clone(), runtime.clone());
        cx.on_app_quit(move |_| {
            let (core, runtime) = (quit_core.clone(), quit_runtime.clone());
            async move {
                let _ = runtime.spawn(async move { core.shutdown().await }).await;
            }
        })
        .detach();
        oncode_ui::init(OnCode { core, runtime, events: Some(events_rx) }, cx);
        // Urutan menentukan urutan pendaftaran command; urutan ikon dan panel diatur lewat `order`.
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
        if let Err(error) = oncode_ui::try_open_window(folder, cx) {
            // Tanpa jendela, aplikasi tidak akan pernah menerima "jendela terakhir ditutup";
            // catat penyebabnya lalu keluar agar tidak tertinggal sebagai proses tak terlihat.
            write_log(&log_path, &format!("failed to open a window: {error:#}"));
            eprintln!("oncode: failed to open a window: {error:#}");
            cx.quit();
            return;
        }
        #[cfg(debug_assertions)]
        dev::run_script(cx);
    });
}

/// Menambahkan satu baris bertanggal ke `oncode.log` di folder data. Aplikasi GUI di Windows
/// tidak punya konsol, jadi galat saat mulai hanya terlihat lewat berkas ini.
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

/// Panic juga dicatat ke `oncode.log`, selain dicetak seperti biasa.
fn install_panic_log(path: PathBuf) {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        write_log(&path, &format!("panic: {info}\n{}", std::backtrace::Backtrace::force_capture()));
        default_hook(info);
    }));
}
