use serde::Serialize;
use specta::Type;

/// Kode galat yang dipakai UI untuk memilih pesan dan aksi lanjutan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub enum ForgeErrorCode {
    /// Token tidak ada, salah, atau sudah kedaluwarsa (HTTP 401).
    Unauthorized,
    /// Token sah tetapi tidak berhak atas sumber daya ini (HTTP 403).
    Forbidden,
    /// Sumber daya tidak ada, atau tidak terlihat oleh akun ini (HTTP 404).
    NotFound,
    /// Batas laju API terlampaui (HTTP 429, atau 403 dengan header batas laju).
    RateLimited,
    /// Gagal terhubung, waktu habis, atau server sedang bermasalah (HTTP 5xx).
    Network,
    /// Jawaban server tidak berbentuk seperti yang diharapkan.
    InvalidResponse,
    /// Operasi tidak didukung oleh penyedia atau instance ini.
    Unsupported,
    /// Keychain OS tidak tersedia atau menolak akses.
    Keychain,
    /// Masukan ditolak, baik oleh pustaka ini maupun oleh server (HTTP 400/409/422).
    InvalidInput,
}

/// Galat dari lapisan forge. `message` aman ditampilkan ke pengguna dan tidak pernah
/// memuat token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type, thiserror::Error)]
#[serde(rename_all = "camelCase")]
#[error("{message}")]
pub struct ForgeError {
    pub code: ForgeErrorCode,
    pub message: String,
}

pub type Result<T> = std::result::Result<T, ForgeError>;

impl ForgeError {
    pub fn new(code: ForgeErrorCode, message: impl Into<String>) -> Self {
        Self { code, message: message.into() }
    }

    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(ForgeErrorCode::InvalidInput, message)
    }

    pub fn unsupported(message: impl Into<String>) -> Self {
        Self::new(ForgeErrorCode::Unsupported, message)
    }

    pub fn invalid_response(message: impl Into<String>) -> Self {
        Self::new(ForgeErrorCode::InvalidResponse, message)
    }

    pub fn keychain(message: impl Into<String>) -> Self {
        Self::new(ForgeErrorCode::Keychain, message)
    }

    pub fn network(message: impl Into<String>) -> Self {
        Self::new(ForgeErrorCode::Network, message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_with_camel_case_code() {
        let err = ForgeError::new(ForgeErrorCode::RateLimited, "Batas laju API terlampaui");
        let json = serde_json::to_value(&err).expect("serialisasi galat");
        assert_eq!(json, serde_json::json!({ "code": "rateLimited", "message": "Batas laju API terlampaui" }));
        assert_eq!(err.to_string(), "Batas laju API terlampaui");
    }
}
