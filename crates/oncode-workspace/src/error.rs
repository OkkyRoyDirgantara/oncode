use std::io;
use std::path::Path;

use serde::Serialize;

/// Kode galat yang dipakai UI untuk memilih pesan dan aksi lanjutan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum WsErrorCode {
    NotFound,
    AlreadyExists,
    PermissionDenied,
    InvalidPath,
    InvalidPattern,
    Cancelled,
    Io,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, thiserror::Error)]
#[serde(rename_all = "camelCase")]
#[error("{message}")]
pub struct WsError {
    pub code: WsErrorCode,
    /// Ringkas, untuk ditampilkan ke pengguna.
    pub message: String,
}

pub type Result<T> = std::result::Result<T, WsError>;

impl WsError {
    pub fn new(code: WsErrorCode, message: impl Into<String>) -> Self {
        Self { code, message: message.into() }
    }

    pub fn not_found(path: &Path) -> Self {
        Self::new(WsErrorCode::NotFound, format!("File or folder not found: {}", path.display()))
    }

    pub fn already_exists(path: &Path) -> Self {
        Self::new(WsErrorCode::AlreadyExists, format!("A file or folder with that name already exists: {}", path.display()))
    }

    pub fn invalid_path(message: impl Into<String>) -> Self {
        Self::new(WsErrorCode::InvalidPath, message)
    }

    pub fn invalid_pattern(message: impl Into<String>) -> Self {
        Self::new(WsErrorCode::InvalidPattern, message)
    }

    pub fn other(message: impl Into<String>) -> Self {
        Self::new(WsErrorCode::Io, message)
    }

    /// Galat I/O yang terjadi pada `path`; kodenya diturunkan dari `ErrorKind`.
    pub fn io(path: &Path, err: io::Error) -> Self {
        let code = code_of(err.kind());
        let shown = path.display();
        let message = match code {
            WsErrorCode::NotFound => format!("File or folder not found: {shown}"),
            WsErrorCode::AlreadyExists => format!("A file or folder with that name already exists: {shown}"),
            WsErrorCode::PermissionDenied => format!("Permission denied: {shown}"),
            WsErrorCode::InvalidPath => format!("Invalid path: {shown}"),
            _ => format!("I/O error on {shown}: {err}"),
        };
        Self { code, message }
    }
}

fn code_of(kind: io::ErrorKind) -> WsErrorCode {
    match kind {
        io::ErrorKind::NotFound => WsErrorCode::NotFound,
        io::ErrorKind::AlreadyExists => WsErrorCode::AlreadyExists,
        io::ErrorKind::PermissionDenied => WsErrorCode::PermissionDenied,
        io::ErrorKind::InvalidInput
        | io::ErrorKind::NotADirectory
        | io::ErrorKind::IsADirectory => WsErrorCode::InvalidPath,
        _ => WsErrorCode::Io,
    }
}

impl From<io::Error> for WsError {
    fn from(err: io::Error) -> Self {
        let code = code_of(err.kind());
        let message = match code {
            WsErrorCode::NotFound => "File or folder not found".to_string(),
            WsErrorCode::AlreadyExists => "A file or folder with that name already exists".to_string(),
            WsErrorCode::PermissionDenied => "Permission denied".to_string(),
            WsErrorCode::InvalidPath => format!("Invalid path: {err}"),
            _ => format!("I/O error: {err}"),
        };
        Self { code, message }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_io_error_kinds() {
        let e: WsError = io::Error::from(io::ErrorKind::NotFound).into();
        assert_eq!(e.code, WsErrorCode::NotFound);
        let e: WsError = io::Error::from(io::ErrorKind::AlreadyExists).into();
        assert_eq!(e.code, WsErrorCode::AlreadyExists);
        let e: WsError = io::Error::from(io::ErrorKind::PermissionDenied).into();
        assert_eq!(e.code, WsErrorCode::PermissionDenied);
        let e: WsError = io::Error::other("x").into();
        assert_eq!(e.code, WsErrorCode::Io);
    }

    #[test]
    fn display_is_message_and_code_is_camel_case() {
        let e = WsError::new(WsErrorCode::InvalidPattern, "pola salah");
        assert_eq!(e.to_string(), "pola salah");
        let json = serde_json::to_string(&e).unwrap();
        assert_eq!(json, r#"{"code":"invalidPattern","message":"pola salah"}"#);
    }
}
