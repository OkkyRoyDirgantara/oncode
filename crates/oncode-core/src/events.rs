//! Event yang disiarkan `oncode-core` ke UI lewat [`EventSink`].

use std::sync::Arc;

use oncode_git::{GitLogEntry, RepoSnapshot};
use serde::Serialize;

use crate::settings::Settings;

/// Penerima event. Dipanggil dari thread mana pun (runtime tokio, pemantau berkas), jadi
/// harus cepat dan tidak boleh memblokir; UI biasanya hanya meneruskannya ke thread utama.
pub type EventSink = Arc<dyn Fn(CoreEvent) + Send + Sync>;

#[derive(Debug, Clone, PartialEq)]
pub enum CoreEvent {
    /// Keadaan sebuah repositori berubah; UI cukup menggambar ulang dari snapshot ini.
    RepoChanged { root: String, snapshot: RepoSnapshot },
    /// Branch, tag, atau HEAD berubah; daftar ref dan graph perlu dimuat ulang.
    RefsChanged { root: String },
    /// Berkas di folder proyek jendela `window` berubah dari luar aplikasi.
    FsChanged {
        /// Label jendela yang dituju.
        window: String,
        paths: Vec<String>,
        /// Terlalu banyak perubahan untuk dirinci; muat ulang semuanya.
        overflow: bool,
    },
    /// Satu perintah git yang dijalankan aplikasi, untuk panel Git Output.
    GitOutput { entry: GitLogEntry },
    /// Git membutuhkan jawaban pengguna; dijawab lewat `api::app::prompt_respond`.
    Prompt {
        /// Jendela yang folder proyeknya memuat repositori peminta; `None` = tampilkan di jendela mana saja.
        window: Option<String>,
        request: PromptRequest,
    },
    /// Pengaturan tingkat pengguna berubah. Pengaturan efektif per jendela (dengan penimpaan
    /// proyek) diambil lewat `api::app::settings_get`.
    SettingsChanged { settings: Settings },
    /// Daftar akun GitHub/GitLab berubah (login atau logout).
    AccountsChanged,
    /// Pemberitahuan dari backend yang ditampilkan UI sebagai toast.
    Notice { kind: NoticeKind, message: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PromptKind {
    /// Username atau teks biasa.
    Text,
    /// Password, token, atau passphrase: masukan disembunyikan.
    Secret,
    /// Pertanyaan ya/tidak (mis. sidik jari host SSH).
    Confirm,
    /// Pesan commit multi-baris.
    Message,
}

/// Isi [`CoreEvent::Prompt`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptRequest {
    pub id: String,
    pub kind: PromptKind,
    /// Teks pertanyaan dari git/ssh.
    pub prompt: String,
    /// Isi awal (untuk pesan commit).
    pub initial: String,
    pub repo: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum NoticeKind {
    Warning,
    Error,
}
