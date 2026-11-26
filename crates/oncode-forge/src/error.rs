use serde::Serialize;

/// Error code the UI uses to choose a message and follow-up action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ForgeErrorCode {
    /// The token is missing, wrong, or expired (HTTP 401).
    Unauthorized,
    /// The token is valid but not permitted to access this resource (HTTP 403).
    Forbidden,
    /// The resource does not exist, or is not visible to this account (HTTP 404).
    NotFound,
    /// The API rate limit was exceeded (HTTP 429, or 403 with a rate-limit header).
    RateLimited,
    /// Failed to connect, timed out, or the server is having problems (HTTP 5xx).
    Network,
    /// The server's response is not shaped as expected.
    InvalidResponse,
    /// The operation is not supported by this provider or instance.
    Unsupported,
    /// The OS keychain is unavailable or denied access.
    Keychain,
    /// Input was rejected, either by this library or by the server (HTTP 400/409/422).
    InvalidInput,
}

/// Error from the forge layer. `message` is safe to show to the user and never
/// contains a token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, thiserror::Error)]
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
        let err = ForgeError::new(ForgeErrorCode::RateLimited, "API rate limit exceeded");
        let json = serde_json::to_value(&err).expect("serialize error");
        assert_eq!(json, serde_json::json!({ "code": "rateLimited", "message": "API rate limit exceeded" }));
        assert_eq!(err.to_string(), "API rate limit exceeded");
    }
}
