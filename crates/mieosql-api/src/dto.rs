//! Frontend-facing DTOs.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use mieosql_core::{Engine, ErrorCode, ErrorInfo, SshSpec, SslMode};

/// Structured error payload crossing the IPC boundary.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ApiError {
    pub code: ErrorCode,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub hint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub position: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub db_code: Option<String>,
}

impl From<ErrorInfo> for ApiError {
    fn from(info: ErrorInfo) -> Self {
        Self {
            code: info.code,
            message: info.message,
            detail: info.detail,
            hint: info.hint,
            position: info.position,
            db_code: info.db_code,
        }
    }
}

impl From<mieosql_core::MieoError> for ApiError {
    fn from(err: mieosql_core::MieoError) -> Self {
        Self::from(err.info)
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)?;
        if let Some(hint) = &self.hint {
            write!(f, " ({hint})")?;
        }
        Ok(())
    }
}

impl std::error::Error for ApiError {}

pub type ApiResult<T> = Result<T, ApiError>;

/// Response of a table-preview start: the query id plus the exact SQL that was
/// generated (kept by the frontend so the result can be re-exported verbatim).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PreviewStart {
    pub query_id: String,
    pub sql: String,
}

/// Everything the connection dialog needs to create or update a profile.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ProfileInput {
    /// `None` creates a new profile; `Some` updates in place.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub id: Option<String>,
    pub name: String,
    pub engine: Engine,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub host: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub port: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub username: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub database: Option<String>,
    /// SQLite file path.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub file: Option<String>,
    #[serde(default)]
    pub params: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub ssl_mode: Option<SslMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub ssh: Option<SshSpec>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub color: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub group: Option<String>,
    /// Connector preset / cloud flavor (`aurora`, `documentdb`, `redshift`,
    /// `azure`, `elasticache`, `memorystore`, ...). Descriptive only.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub variant: Option<String>,
    /// Password policy: `None` keeps the stored secret, `Some("")` clears it.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub password: Option<String>,
    /// SSH password / key passphrase policy, same as `password`.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub ssh_password: Option<String>,
}
