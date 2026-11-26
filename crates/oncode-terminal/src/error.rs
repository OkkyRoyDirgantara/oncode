use serde::Serialize;

/// Error code the UI uses to choose the message and follow-up action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TermErrorCode {
    /// There is no terminal with that id (it was closed or its process has already exited).
    NotFound,
    /// The PTY could not be created or the shell failed to start.
    SpawnFailed,
    /// Failed to write to the PTY or resize it.
    Io,
}

#[derive(Debug, Clone, thiserror::Error)]
#[error("{message}")]
pub struct TermError {
    pub code: TermErrorCode,
    pub message: String,
}

pub type Result<T> = std::result::Result<T, TermError>;

impl TermError {
    pub fn new(code: TermErrorCode, message: impl Into<String>) -> Self {
        Self { code, message: message.into() }
    }

    pub fn not_found(id: u32) -> Self {
        Self::new(TermErrorCode::NotFound, format!("Terminal {id} not found or already closed"))
    }

    pub fn spawn_failed(message: impl Into<String>) -> Self {
        Self::new(TermErrorCode::SpawnFailed, message)
    }

    pub fn io(context: &str, err: impl std::fmt::Display) -> Self {
        Self::new(TermErrorCode::Io, format!("{context}: {err}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_serialize_as_camel_case() {
        let json = |code: TermErrorCode| serde_json::to_string(&code).unwrap_or_default();
        assert_eq!(json(TermErrorCode::NotFound), "\"notFound\"");
        assert_eq!(json(TermErrorCode::SpawnFailed), "\"spawnFailed\"");
        assert_eq!(json(TermErrorCode::Io), "\"io\"");
    }

    #[test]
    fn display_is_the_message() {
        let err = TermError::not_found(7);
        assert_eq!(err.code, TermErrorCode::NotFound);
        assert_eq!(err.to_string(), "Terminal 7 not found or already closed");
    }
}
