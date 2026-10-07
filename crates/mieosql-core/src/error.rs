//! Structured, UI-friendly errors.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Coarse error classification the frontend can branch on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ErrorCode {
    /// Cannot reach the server / open the file.
    Connection,
    /// Authentication or authorization with the server failed.
    Auth,
    /// Network level failure (DNS, TCP reset, TLS).
    Network,
    Timeout,
    /// SQL syntax error.
    Syntax,
    Permission,
    NotFound,
    /// Unique/FK/check constraint violation.
    Constraint,
    /// Conflict (e.g. duplicate object name).
    Conflict,
    /// The engine/driver does not support the requested operation.
    Unsupported,
    /// Query was cancelled by the user.
    Cancelled,
    /// Bad data (decode error, invalid parameter...).
    Data,
    Internal,
}

/// Serializable error payload. This is what crosses the IPC boundary.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ErrorInfo {
    pub code: ErrorCode,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub hint: Option<String>,
    /// Character position in the SQL text, when the engine reports one.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub position: Option<u32>,
    /// Engine-native error code (e.g. `ER_NO_SUCH_TABLE`, SQLSTATE).
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub db_code: Option<String>,
}

impl ErrorInfo {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            detail: None,
            hint: None,
            position: None,
            db_code: None,
        }
    }
}

/// Error type used across all MieoSQL crates.
#[derive(Debug, thiserror::Error)]
#[error("{}", .info.message)]
pub struct MieoError {
    pub info: ErrorInfo,
    #[source]
    pub source: Option<Box<dyn std::error::Error + Send + Sync>>,
}

pub type MieoResult<T> = Result<T, MieoError>;

impl MieoError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            info: ErrorInfo::new(code, message),
            source: None,
        }
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Internal, message)
    }

    pub fn unsupported(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Unsupported, message)
    }

    pub fn cancelled() -> Self {
        Self::new(ErrorCode::Cancelled, "Query cancelled")
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.info.detail = Some(detail.into());
        self
    }

    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.info.hint = Some(hint.into());
        self
    }

    pub fn with_db_code(mut self, code: impl Into<String>) -> Self {
        self.info.db_code = Some(code.into());
        self
    }

    pub fn with_position(mut self, position: u32) -> Self {
        self.info.position = Some(position);
        self
    }

    pub fn with_source(mut self, source: impl std::error::Error + Send + Sync + 'static) -> Self {
        self.source = Some(Box::new(source));
        self
    }

    pub fn code(&self) -> ErrorCode {
        self.info.code
    }

    pub fn into_info(self) -> ErrorInfo {
        self.info
    }
}

impl From<std::io::Error> for MieoError {
    fn from(e: std::io::Error) -> Self {
        MieoError::new(ErrorCode::Internal, format!("I/O error: {e}")).with_source(e)
    }
}

impl From<serde_json::Error> for MieoError {
    fn from(e: serde_json::Error) -> Self {
        MieoError::new(ErrorCode::Data, format!("JSON error: {e}")).with_source(e)
    }
}
