//! Pembantu uji tampilan (fitur `test-support`): aplikasi uji dengan `oncode-core` sungguhan
//! di direktori sementara, dan jendela tanpa layar berisi `Shell`.
//!
//! ```ignore
//! #[gpui_kit::test]
//! async fn contoh(cx: &mut TestAppContext) {
//!     let app = TestApp::new(cx);
//!     let project = app.project(&[("a.txt", "isi")]);
//!     let window = app.open(Some(project.path()), cx).await;
//!     ...
//! }
//! ```

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::{px, size, AnyWindowHandle, AppContext as _, Bounds, Entity, TestAppContext, WindowBounds, WindowOptions};
use oncode_core::{Core, Dirs, FallbackStore, Shared};
use tempfile::TempDir;

use crate::{OnCode, Shell, Workbench};

/// Penyimpanan token di memori; uji tidak menyentuh keychain.
fn memory_tokens() -> Arc<FallbackStore> {
    Arc::new(FallbackStore::new(Arc::new(oncode_forge::MemoryStore::default())))
}

static GIT_ENV: std::sync::Once = std::sync::Once::new();

/// Mengisolasi git dari konfigurasi global/sistem dan menetapkan identitas uji (sekali per proses).
pub fn isolate_git() {
    GIT_ENV.call_once(|| {
        let vars = [
            ("GIT_CONFIG_GLOBAL", if cfg!(windows) { "NUL" } else { "/dev/null" }),
            ("GIT_CONFIG_NOSYSTEM", "1"),
            ("GIT_AUTHOR_NAME", "Penguji"),
            ("GIT_AUTHOR_EMAIL", "penguji@contoh.id"),
            ("GIT_COMMITTER_NAME", "Penguji"),
            ("GIT_COMMITTER_EMAIL", "penguji@contoh.id"),
            ("GIT_CONFIG_COUNT", "2"),
            ("GIT_CONFIG_KEY_0", "protocol.file.allow"),
            ("GIT_CONFIG_VALUE_0", "always"),
            ("GIT_CONFIG_KEY_1", "commit.gpgsign"),
            ("GIT_CONFIG_VALUE_1", "false"),
        ];
        for (key, value) in vars {
            std::env::set_var(key, value);
        }
    });
}

/// Menjalankan git langsung (di luar lapisan yang diuji) dan mengembalikan stdout; panik bila gagal.
pub fn git(cwd: &Path, args: &[&str]) -> String {
    isolate_git();
    let out = std::process::Command::new("git").current_dir(cwd).args(args).output().expect("git tersedia");
    assert!(out.status.success(), "git {args:?} gagal: {}{}", String::from_utf8_lossy(&out.stderr), String::from_utf8_lossy(&out.stdout));
    String::from_utf8_lossy(&out.stdout).into_owned()
}

pub struct TestApp {
    pub core: Shared,
    pub runtime: Arc<tokio::runtime::Runtime>,
    /// Direktori konfigurasi dan data sementara; dihapus saat uji selesai.
    pub dirs: TempDir,
    projects: std::cell::RefCell<Vec<TempDir>>,
}

pub struct TestWindow {
    pub handle: AnyWindowHandle,
    pub shell: Entity<Shell>,
}

impl TestApp {
    /// Menyiapkan GPUI, komponen, `oncode-core`, dan fitur dasar. Fitur lain didaftarkan
    /// pemanggil lewat `cx.update(|cx| crate_fitur::init(cx))` sebelum membuka jendela.
    pub fn new(cx: &mut TestAppContext) -> Self {
        isolate_git();
        // `oncode-helper` (kredensial dan interactive rebase) ada di `target/<profil>/`, dicari dari
        // folder binary uji ke atas: binary uji ada di `deps/` atau, dengan layout build-dir baru
        // Cargo, di `build/<crate>/<hash>/out/`.
        if std::env::var_os("ONCODE_HELPER").is_none() {
            let name = if cfg!(windows) { "oncode-helper.exe" } else { "oncode-helper" };
            let helper = std::env::current_exe().ok().and_then(|exe| exe.ancestors().skip(1).map(|dir| dir.join(name)).find(|path| path.is_file()));
            if let Some(helper) = helper {
                std::env::set_var("ONCODE_HELPER", helper);
            }
        }
        let runtime = Arc::new(tokio::runtime::Builder::new_multi_thread().enable_all().worker_threads(2).build().expect("runtime tokio"));
        let dirs = tempfile::tempdir().expect("direktori sementara");
        let (events_tx, events_rx) = tokio::sync::mpsc::unbounded_channel();
        let core = Core::new(
            runtime.handle().clone(),
            Dirs::under(dirs.path()),
            memory_tokens(),
            Arc::new(move |event| {
                let _ = events_tx.send(event);
            }),
        );
        // Pekerjaan backend berjalan di thread tokio sungguhan, jadi eksekutor uji harus mau menunggu.
        cx.executor().allow_parking();
        cx.update(|cx| {
            gpui_kit::init(cx);
            crate::init(OnCode { core: core.clone(), runtime: runtime.clone(), events: Some(events_rx) }, cx);
        });
        Self { core, runtime, dirs, projects: Default::default() }
    }

    /// Folder proyek sementara berisi berkas-berkas ini (path relatif, isi).
    pub fn project(&self, files: &[(&str, &str)]) -> PathBuf {
        let dir = tempfile::tempdir().expect("direktori proyek");
        for (rel, content) in files {
            let full = dir.path().join(rel);
            if let Some(parent) = full.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(full, content).unwrap();
        }
        let path = canonical(dir.path());
        self.projects.borrow_mut().push(dir);
        path
    }

    /// Folder proyek berupa repositori git (branch `main`) dengan satu commit berisi berkas-berkas
    /// ini, sudah dipercaya. Git diisolasi dari konfigurasi pengguna.
    pub fn git_project(&self, files: &[(&str, &str)]) -> PathBuf {
        let path = self.project(files);
        git(&path, &["init", "-q", "-b", "main"]);
        // Identitas di konfigurasi repositori, seperti pengguna yang sudah mengatur git.
        git(&path, &["config", "user.name", "Penguji"]);
        git(&path, &["config", "user.email", "penguji@contoh.id"]);
        git(&path, &["add", "-A"]);
        git(&path, &["commit", "-q", "--allow-empty", "-m", "Commit pertama"]);
        self.trust(&path);
        path
    }

    /// Mencabut kepercayaan folder (mode terbatas).
    pub fn trust_revoke(&self, path: &Path) {
        self.core.store.set_trusted(&path.to_string_lossy(), false);
    }

    /// Menandai folder sebagai dipercaya (seolah pengguna sudah menyetujuinya).
    pub fn trust(&self, path: &Path) {
        self.core.store.set_trusted(&path.to_string_lossy(), true);
    }

    /// Membuka jendela tanpa layar; bila `folder` diberikan, menunggu sampai folder terbuka.
    pub async fn open(&self, folder: Option<&Path>, cx: &mut TestAppContext) -> TestWindow {
        let folder_buf = folder.map(Path::to_path_buf);
        let (handle, shell) = cx.update(|cx| {
            gpui_kit::open_window(
                WindowOptions { window_bounds: Some(WindowBounds::Windowed(Bounds::new(Default::default(), size(px(1200.), px(800.))))), ..Default::default() },
                cx,
                |window, cx| cx.new(|cx| Shell::new(folder_buf, window, cx)),
            )
            .expect("jendela uji")
        });
        let window = TestWindow { handle, shell };
        // Jendela aktif seperti di aplikasi sungguhan: GPUI hanya memanggil pendengar fokus-lepas
        // (`on_blur`) dan beberapa perilaku lain bila jendelanya aktif.
        window.update(cx, |window, _| window.activate_window());
        cx.run_until_parked();
        if folder.is_some() {
            window.wait_until(cx, "folder terbuka", |shell, cx| shell.read(cx).workspace().is_some()).await;
        }
        window
    }
}

impl TestWindow {
    pub fn workbench(&self, cx: &mut TestAppContext) -> Entity<Workbench> {
        cx.update(|cx| self.shell.read(cx).workbench().clone())
    }

    /// Menunggu sampai `condition` benar (paling lama 10 detik), sambil menjalankan tugas yang tertunda.
    pub async fn wait_until(&self, cx: &mut TestAppContext, what: &str, mut condition: impl FnMut(&Entity<Shell>, &gpui_kit::App) -> bool) {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            cx.run_until_parked();
            if cx.update(|cx| condition(&self.shell, cx)) {
                return;
            }
            assert!(std::time::Instant::now() < deadline, "menunggu terlalu lama: {what}");
            // Beri waktu thread backend (tokio) menyelesaikan pekerjaannya.
            cx.executor().timer(Duration::from_millis(20)).await;
        }
    }

    /// Menjalankan `f` dengan jendela ini (untuk `find`, `click`, `input`, `press`).
    pub fn update<R>(&self, cx: &mut TestAppContext, f: impl FnOnce(&mut gpui_kit::Window, &mut gpui_kit::App) -> R) -> R {
        cx.update_window(self.handle, |_, window, cx| f(window, cx)).expect("jendela masih ada")
    }

    /// Menjalankan command seperti dari command palette.
    pub fn run_command(&self, cx: &mut TestAppContext, id: &str) -> bool {
        self.update(cx, |window, cx| crate::execute_now(id, &serde_json::Value::Null, window, cx))
    }
}

/// Path kanonik seperti yang dipakai `workspace_open`: di Windows tanpa awalan `\\?\`.
fn canonical(path: &std::path::Path) -> PathBuf {
    let path = std::fs::canonicalize(path).unwrap();
    let text = path.to_string_lossy();
    match text.strip_prefix(r"\\?\") {
        Some(rest) if !rest.starts_with("UNC") => PathBuf::from(rest),
        _ => path,
    }
}
