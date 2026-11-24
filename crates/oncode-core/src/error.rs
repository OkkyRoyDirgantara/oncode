//! Galat tunggal yang dikembalikan ke UI oleh semua fungsi API.

use serde::Serialize;

/// `code` dipakai UI untuk memilih pesan dan aksi lanjutan (mis. `pushRejected`, `stale`, `untrusted`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, thiserror::Error)]
#[serde(rename_all = "camelCase")]
#[error("{message}")]
pub struct AppError {
    pub code: String,
    pub message: String,
    /// Mis. stderr git apa adanya.
    pub detail: Option<String>,
}

pub type AppResult<T> = Result<T, AppError>;

impl AppError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self { code: code.to_string(), message: message.into(), detail: None }
    }

    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new("invalidInput", message)
    }

    pub fn untrusted() -> Self {
        Self::new("untrusted", "This folder is not trusted; this feature is disabled in restricted mode")
    }
}

/// Nama varian enum kode galat dalam camelCase (mis. `pushRejected`), sama seperti bentuk serde-nya.
fn code_name<T: Serialize>(code: &T) -> String {
    serde_json::to_value(code).ok().and_then(|v| v.as_str().map(String::from)).unwrap_or_else(|| "unknown".into())
}

impl From<oncode_git::GitError> for AppError {
    fn from(e: oncode_git::GitError) -> Self {
        Self { code: code_name(&e.code), message: e.message, detail: e.detail }
    }
}

impl From<oncode_workspace::WsError> for AppError {
    fn from(e: oncode_workspace::WsError) -> Self {
        Self { code: code_name(&e.code), message: e.message, detail: None }
    }
}

impl From<oncode_terminal::TermError> for AppError {
    fn from(e: oncode_terminal::TermError) -> Self {
        Self { code: code_name(&e.code), message: e.message, detail: None }
    }
}

impl From<oncode_lsp::LspError> for AppError {
    fn from(e: oncode_lsp::LspError) -> Self {
        Self { code: code_name(&e.code), message: e.message, detail: None }
    }
}

impl From<oncode_forge::ForgeError> for AppError {
    fn from(e: oncode_forge::ForgeError) -> Self {
        Self { code: code_name(&e.code), message: e.message, detail: None }
    }
}

impl From<oncode_ai::AiError> for AppError {
    fn from(e: oncode_ai::AiError) -> Self {
        Self { code: code_name(&e.code), message: e.message, detail: None }
    }
}

impl From<std::io::Error> for AppError {
    fn from(e: std::io::Error) -> Self {
        Self::new("io", e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_error_codes_keep_their_camel_case_names() {
        let git = oncode_git::GitError { code: oncode_git::GitErrorCode::NotARepo, message: "bukan repo".into(), detail: Some("fatal".into()) };
        let app = AppError::from(git);
        assert_eq!((app.code.as_str(), app.detail.as_deref()), ("notARepo", Some("fatal")));
        assert_eq!(AppError::untrusted().code, "untrusted");
        assert_eq!(AppError::invalid("x").code, "invalidInput");
        assert_eq!(AppError::from(std::io::Error::other("rusak")).code, "io");
        assert_eq!(AppError::new("io", "pesan").to_string(), "pesan");
    }
}
