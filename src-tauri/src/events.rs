//! Event yang disiarkan backend ke UI.

use oncode_git::{GitLogEntry, RepoSnapshot};
use serde::Serialize;
use specta::Type;
use tauri_specta::Event;

use crate::settings::Settings;

/// Keadaan sebuah repositori berubah; UI cukup menggambar ulang dari snapshot ini.
#[derive(Debug, Clone, Serialize, Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct RepoChanged {
    pub root: String,
    pub snapshot: RepoSnapshot,
}

/// Branch, tag, atau HEAD berubah; daftar ref dan graph perlu dimuat ulang.
#[derive(Debug, Clone, Serialize, Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct RefsChanged {
    pub root: String,
}

/// Berkas di folder proyek berubah dari luar aplikasi.
#[derive(Debug, Clone, Serialize, Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct FsChanged {
    pub paths: Vec<String>,
    /// Terlalu banyak perubahan untuk dirinci; muat ulang semuanya.
    pub overflow: bool,
}

/// Satu perintah git yang dijalankan aplikasi, untuk panel Git Output.
#[derive(Debug, Clone, Serialize, Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct GitOutput {
    pub entry: GitLogEntry,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
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

/// Git membutuhkan jawaban pengguna; dijawab lewat command `prompt_respond`.
#[derive(Debug, Clone, Serialize, Type, Event)]
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

#[derive(Debug, Clone, Serialize, Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct SettingsChanged {
    pub settings: Settings,
}

/// Daftar akun GitHub/GitLab berubah (login atau logout).
#[derive(Debug, Clone, Serialize, Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct AccountsChanged {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum NoticeKind {
    Warning,
    Error,
}

/// Pemberitahuan dari backend yang ditampilkan UI sebagai toast.
#[derive(Debug, Clone, Serialize, Type, Event)]
#[serde(rename_all = "camelCase")]
pub struct Notice {
    pub kind: NoticeKind,
    pub message: String,
}
