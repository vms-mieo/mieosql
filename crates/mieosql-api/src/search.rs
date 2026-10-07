//! Database-wide search: find objects, columns and (optionally) definitions
//! matching a pattern across a whole connection.
//!
//! Part of the v1 feature pack. The DTOs and method signatures below are a
//! frozen contract: implementations fill the bodies, but callers (Tauri
//! commands, CLI, tests) rely on these exact shapes.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use mieosql_core::{MieoError, ObjectKind};

use crate::dto::ApiResult;
use crate::service::Api;

/// How the pattern is matched against names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum SearchMatchMode {
    /// `pattern` appears anywhere in the name.
    Contains,
    /// Name starts with `pattern`.
    Prefix,
    /// `pattern` matches a whole word (identifier boundaries).
    WholeWord,
    /// `pattern` is a regular expression (Rust regex syntax).
    Regex,
}

/// Which part of an object matched.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum SearchHitField {
    /// The object name itself.
    Name,
    /// A column name.
    Column,
    /// The object definition (view SQL / DDL) when requested.
    Definition,
}

/// What to search for, and where.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SearchRequest {
    pub pattern: String,
    /// Case-sensitive matching (default: case-insensitive).
    #[serde(default)]
    pub case_sensitive: bool,
    pub mode: SearchMatchMode,
    /// Object kinds to scan; empty = tables + views.
    #[serde(default)]
    pub kinds: Vec<ObjectKind>,
    /// Databases to search; empty = every database visible to the session.
    #[serde(default)]
    pub databases: Vec<String>,
    /// Schemas to search; empty = every schema of the selected databases.
    #[serde(default)]
    pub schemas: Vec<String>,
    /// Also match column names.
    #[serde(default = "default_true")]
    pub include_columns: bool,
    /// Also match view/table definitions (engine-dependent, best effort).
    #[serde(default)]
    pub include_definitions: bool,
    /// Cap on returned hits (1..=5000).
    #[serde(default = "default_limit")]
    pub limit: u32,
}

fn default_true() -> bool {
    true
}

fn default_limit() -> u32 {
    500
}

/// One matched object / column.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SearchHit {
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub database: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub schema: Option<String>,
    /// Table / view / function name.
    pub object: String,
    pub kind: ObjectKind,
    /// Column name when the hit came from a column.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub column: Option<String>,
    /// Native column type, when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub column_type: Option<String>,
    /// A short excerpt around the match for definition hits.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub excerpt: Option<String>,
    pub matched_in: SearchHitField,
}

/// Result set of a database search.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SearchResponse {
    pub hits: Vec<SearchHit>,
    /// Objects (tables/views) inspected.
    #[ts(type = "number")]
    pub scanned_objects: u64,
    /// True when `limit` cut the hit list short.
    pub truncated: bool,
}

impl Api {
    /// Search the whole connection for objects/columns matching `request`.
    pub async fn search_database(
        &self,
        session_id: String,
        request: SearchRequest,
    ) -> ApiResult<SearchResponse> {
        let _ = (session_id, request);
        Err(MieoError::unsupported("database search is not implemented yet").into())
    }
}
