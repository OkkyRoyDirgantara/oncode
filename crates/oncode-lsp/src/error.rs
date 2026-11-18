use serde::Serialize;
use specta::Type;

/// Kode galat yang dipakai UI untuk memilih pesan dan aksi lanjutan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum LspErrorCode {
    /// Tidak ada server dengan id tersebut (sudah dihentikan atau prosesnya sudah keluar).
    NotFound,
    /// Perintah server tidak ditemukan di PATH; UI menampilkan petunjuk pemasangan.
    ServerNotInstalled,
    /// Perintah ditemukan tetapi prosesnya gagal dijalankan.
    SpawnFailed,
    /// Gagal menulis ke stdin server.
    Io,
}

#[derive(Debug, Clone, thiserror::Error)]
#[error("{message}")]
pub struct LspError {
    pub code: LspErrorCode,
    pub message: String,
}

pub type Result<T> = std::result::Result<T, LspError>;

impl LspError {
    pub fn new(code: LspErrorCode, message: impl Into<String>) -> Self {
        Self { code, message: message.into() }
    }

    pub fn not_found(id: u32) -> Self {
        Self::new(LspErrorCode::NotFound, format!("Language server {id} not found or already stopped"))
    }

    pub fn spawn_failed(message: impl Into<String>) -> Self {
        Self::new(LspErrorCode::SpawnFailed, message)
    }

    pub fn io(context: &str, err: impl std::fmt::Display) -> Self {
        Self::new(LspErrorCode::Io, format!("{context}: {err}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_serialize_as_camel_case() {
        let json = |code: LspErrorCode| serde_json::to_string(&code).unwrap_or_default();
        assert_eq!(json(LspErrorCode::NotFound), "\"notFound\"");
        assert_eq!(json(LspErrorCode::ServerNotInstalled), "\"serverNotInstalled\"");
        assert_eq!(json(LspErrorCode::SpawnFailed), "\"spawnFailed\"");
        assert_eq!(json(LspErrorCode::Io), "\"io\"");
    }
}
