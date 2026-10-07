//! Team workspace bundles (Collaboration v1): export/import connection
//! metadata as a portable JSON file. Secrets are **never** included — the
//! receiving teammate re-enters passwords, so bundles can be shared anywhere.
//!
//! Part of the v1 feature pack — frozen contract.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use mieosql_core::{ConnectionProfile, MieoError};

use crate::dto::ApiResult;
use crate::service::Api;

/// A shareable bundle of connection metadata.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct WorkspaceBundle {
    pub version: u32,
    pub app: String,
    /// RFC 3339 timestamp.
    pub exported_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub note: Option<String>,
    /// Profiles without secrets.
    pub profiles: Vec<ConnectionProfile>,
}

/// Outcome of importing a bundle.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ImportSummary {
    #[ts(type = "number")]
    pub imported: u64,
    #[ts(type = "number")]
    pub skipped: u64,
    /// Names that collided and were skipped (or replaced with `overwrite`).
    #[serde(default)]
    pub names: Vec<String>,
}

impl Api {
    /// Write a workspace bundle (connection metadata, no secrets) to `path`.
    pub fn export_workspace(&self, path: String) -> ApiResult<WorkspaceBundle> {
        let _ = path;
        Err(MieoError::unsupported("workspace export is not implemented yet").into())
    }

    /// Import profiles from a bundle. With `overwrite`, same-id profiles are
    /// replaced; otherwise collisions are skipped.
    pub fn import_workspace(&self, path: String, overwrite: bool) -> ApiResult<ImportSummary> {
        let _ = (path, overwrite);
        Err(MieoError::unsupported("workspace import is not implemented yet").into())
    }
}
