//! Structure & Data Synchronization: diff two live scopes and generate (or
//! apply) the SQL needed to make the target match the source.
//!
//! Part of the v1 feature pack — frozen contract.
//!
//! Scope of v1:
//! - Structure: tables added/removed/changed (columns + indexes), FK diff is
//!   reported for information only. Same-engine ALTERs are exact; cross-engine
//!   generation is best-effort and flagged in `warnings`.
//! - Data: key-based row diff (INSERT/UPDATE/DELETE) with capped sampling and
//!   statement preview; apply recomputes and executes in batches.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use mieosql_core::{IndexDef, MieoError, Scope};

use crate::dto::ApiResult;
use crate::service::Api;

/// One side of a synchronization.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SyncEndpoint {
    pub session_id: String,
    pub scope: Scope,
}

/// Kind of change for one object/column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum DiffChange {
    Added,
    Removed,
    Modified,
}

/// Status of one table in the structure diff.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum DiffStatus {
    /// Exists in source, missing in target.
    Added,
    /// Exists in target, missing in source.
    Removed,
    /// Present on both sides with different columns/indexes.
    Changed,
    /// Identical.
    Same,
}

/// One differing column.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ColumnDiff {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub source_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub target_type: Option<String>,
    pub change: DiffChange,
}

/// Structure diff for one table.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TableDiff {
    pub table: String,
    pub status: DiffStatus,
    #[serde(default)]
    pub columns: Vec<ColumnDiff>,
    #[serde(default)]
    pub indexes_added: Vec<IndexDef>,
    #[serde(default)]
    pub indexes_removed: Vec<IndexDef>,
    /// FK differences, human-readable (report-only in v1).
    #[serde(default)]
    pub fk_report: Vec<String>,
}

/// The exact statements a sync would run.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SyncStatement {
    pub table: String,
    pub sql: String,
    pub kind: SyncStatementKind,
    /// True when the statement drops/overwrites data or objects.
    pub destructive: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum SyncStatementKind {
    CreateTable,
    DropTable,
    AddColumn,
    DropColumn,
    AlterColumn,
    AddIndex,
    DropIndex,
    Insert,
    Update,
    Delete,
}

/// Counts for the plan header.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SyncSummary {
    #[ts(type = "number")]
    pub tables_added: u64,
    #[ts(type = "number")]
    pub tables_removed: u64,
    #[ts(type = "number")]
    pub tables_changed: u64,
    #[ts(type = "number")]
    pub statements: u64,
}

/// Request for a structure sync plan.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct StructureSyncRequest {
    pub source: SyncEndpoint,
    pub target: SyncEndpoint,
    /// Restrict to these tables; empty = all tables.
    #[serde(default)]
    pub tables: Vec<String>,
    #[serde(default = "default_true")]
    pub include_indexes: bool,
}

fn default_true() -> bool {
    true
}

/// Structure diff + generated migration script.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct StructureSyncPlan {
    pub diffs: Vec<TableDiff>,
    pub statements: Vec<SyncStatement>,
    pub summary: SyncSummary,
    /// False when source and target engines differ (script is best-effort).
    pub same_engine: bool,
    #[serde(default)]
    pub warnings: Vec<String>,
}

/// Outcome of applying a statement list.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SyncApplyResult {
    #[ts(type = "number")]
    pub applied: u64,
    #[ts(type = "number")]
    pub failed: u64,
    #[serde(default)]
    pub errors: Vec<String>,
    #[ts(type = "number")]
    pub elapsed_ms: u64,
}

/// Request for a data diff.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DataSyncRequest {
    pub source: SyncEndpoint,
    pub target: SyncEndpoint,
    pub table: String,
    /// Key columns (usually the primary key) used to pair rows.
    pub key_columns: Vec<String>,
    /// Columns compared for UPDATE detection; empty = all non-key columns.
    #[serde(default)]
    pub compare_columns: Vec<String>,
    /// Max rows read per side while diffing (safety valve).
    #[serde(default = "default_sample_limit")]
    #[ts(type = "number")]
    pub sample_limit: u64,
    /// Cap on generated statements in the preview.
    #[serde(default = "default_max_statements")]
    pub max_statements: u32,
    /// Keep scanning past `max_statements` to get accurate counts.
    #[serde(default)]
    pub full_scan: bool,
}

fn default_sample_limit() -> u64 {
    100_000
}

fn default_max_statements() -> u32 {
    2_000
}

/// Row-level diff counts.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DataDiffSummary {
    #[ts(type = "number")]
    pub source_rows: u64,
    #[ts(type = "number")]
    pub target_rows: u64,
    #[ts(type = "number")]
    pub to_insert: u64,
    #[ts(type = "number")]
    pub to_update: u64,
    #[ts(type = "number")]
    pub to_delete: u64,
    #[ts(type = "number")]
    pub unchanged: u64,
    /// True when `sample_limit` or `max_statements` cut the diff short.
    pub truncated: bool,
}

/// Data diff + generated statements.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DataSyncPlan {
    pub summary: DataDiffSummary,
    pub statements: Vec<SyncStatement>,
    pub key_columns: Vec<String>,
    #[serde(default)]
    pub warnings: Vec<String>,
}

/// Outcome of applying a data sync.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DataSyncApplyResult {
    #[ts(type = "number")]
    pub inserted: u64,
    #[ts(type = "number")]
    pub updated: u64,
    #[ts(type = "number")]
    pub deleted: u64,
    #[ts(type = "number")]
    pub failed: u64,
    #[serde(default)]
    pub errors: Vec<String>,
    #[ts(type = "number")]
    pub elapsed_ms: u64,
    #[ts(type = "number")]
    pub truncated: bool,
}

impl Api {
    /// Introspect both endpoints and build the structure sync plan.
    pub async fn structure_sync_plan(
        &self,
        request: StructureSyncRequest,
    ) -> ApiResult<StructureSyncPlan> {
        let _ = request;
        Err(MieoError::unsupported("structure synchronization is not implemented yet").into())
    }

    /// Apply a list of statements (from `structure_sync_plan`) on a session.
    pub async fn apply_sync_statements(
        &self,
        session_id: String,
        statements: Vec<String>,
    ) -> ApiResult<SyncApplyResult> {
        let _ = (session_id, statements);
        Err(MieoError::unsupported("apply sync statements is not implemented yet").into())
    }

    /// Diff table rows by key and generate the sync statements preview.
    pub async fn data_sync_plan(&self, request: DataSyncRequest) -> ApiResult<DataSyncPlan> {
        let _ = request;
        Err(MieoError::unsupported("data synchronization is not implemented yet").into())
    }

    /// Recompute the data diff and apply it on the target.
    pub async fn apply_data_sync(
        &self,
        request: DataSyncRequest,
    ) -> ApiResult<DataSyncApplyResult> {
        let _ = request;
        Err(MieoError::unsupported("apply data sync is not implemented yet").into())
    }
}
