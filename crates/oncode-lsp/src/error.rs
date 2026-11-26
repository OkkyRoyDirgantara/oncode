use serde::Serialize;

/// Error code the UI uses to choose a message and follow-up action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum LspErrorCode {
    /// There is no server with that id (it was stopped or its process has exited).
    NotFound,
    /// The server command was not found on PATH; the UI shows installation instructions.
    ServerNotInstalled,
    /// The command was found but its process failed to start.
    SpawnFailed,
    /// Failed to write to the server's stdin.
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
