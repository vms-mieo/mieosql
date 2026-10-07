//! Schema Snapshot: fetch tables + columns + indexes + foreign keys for a
//! scope in one call. Feeds the ER diagram, schema-aware autocomplete and any
//! offline analysis without N round-trips from the frontend.
//!
//! Part of the v1 feature pack — frozen contract.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use mieosql_core::{ColumnDef, Engine, ForeignKeyDef, IndexDef, MieoError, ObjectKind, Scope};

use crate::dto::ApiResult;
use crate::service::Api;

/// One table with everything attached.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TableSnapshot {
    pub name: String,
    pub kind: ObjectKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub comment: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    #[ts(type = "number | null")]
    pub rows_estimate: Option<u64>,
    pub columns: Vec<ColumnDef>,
    pub indexes: Vec<IndexDef>,
    pub foreign_keys: Vec<ForeignKeyDef>,
}

/// Everything the ER diagram / completion cache needs for one scope.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SchemaSnapshot {
    pub engine: Engine,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub database: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub schema: Option<String>,
    /// RFC 3339 timestamp of when the snapshot was taken (for cache headers).
    pub generated_at: String,
    #[ts(type = "number")]
    pub elapsed_ms: u64,
    /// True when `max_tables` cut the table list.
    pub truncated: bool,
    pub tables: Vec<TableSnapshot>,
}

impl Api {
    /// Take a full snapshot of `scope`.
    ///
    /// `max_tables` defaults to 400 (hard cap 2000) to keep the payload sane.
    pub async fn schema_snapshot(
        &self,
        session_id: String,
        scope: Scope,
        include_views: bool,
        max_tables: Option<u32>,
    ) -> ApiResult<SchemaSnapshot> {
        let _ = (session_id, scope, include_views, max_tables);
        Err(MieoError::unsupported("schema snapshot is not implemented yet").into())
    }
}
