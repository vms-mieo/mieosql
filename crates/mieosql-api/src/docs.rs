//! Data Dictionary: export schema documentation (Markdown / HTML) from live
//! catalog metadata — tables, columns, indexes, foreign keys and DDL.
//!
//! Part of the v1 feature pack — frozen contract.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use mieosql_core::{MieoError, Scope};

use crate::dto::ApiResult;
use crate::service::Api;

/// Output document format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum DocFormat {
    Markdown,
    Html,
}

/// What to include in the generated documentation.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DocOptions {
    pub scope: Scope,
    pub format: DocFormat,
    #[serde(default = "default_true")]
    pub include_columns: bool,
    #[serde(default = "default_true")]
    pub include_indexes: bool,
    #[serde(default = "default_true")]
    pub include_foreign_keys: bool,
    /// Include views alongside tables.
    #[serde(default)]
    pub include_views: bool,
    /// Include the CREATE DDL of every object (slower).
    #[serde(default)]
    pub include_ddl: bool,
    /// Document title; defaults to the scope name.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub title: Option<String>,
}

fn default_true() -> bool {
    true
}

/// What was written.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DocSummary {
    #[ts(type = "number")]
    pub tables: u64,
    #[ts(type = "number")]
    pub columns: u64,
    #[ts(type = "number")]
    pub indexes: u64,
    #[ts(type = "number")]
    pub foreign_keys: u64,
    #[ts(type = "number")]
    pub bytes: u64,
    #[ts(type = "number")]
    pub elapsed_ms: u64,
    pub path: String,
}

impl Api {
    /// Generate schema documentation for `options.scope` and write it to `path`.
    pub async fn export_documentation(
        &self,
        session_id: String,
        options: DocOptions,
        path: String,
    ) -> ApiResult<DocSummary> {
        let _ = (session_id, options, path);
        Err(MieoError::unsupported("data dictionary export is not implemented yet").into())
    }
}
