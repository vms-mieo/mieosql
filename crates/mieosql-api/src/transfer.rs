//! Data Transfer: copy a table (or a filtered subset) from one connection to
//! another, cross-engine, streaming row batches.
//!
//! Part of the v1 feature pack — frozen contract.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use mieosql_core::MieoError;

use crate::dto::ApiResult;
use crate::service::Api;
use crate::sync::SyncEndpoint;

/// How the target table is prepared before inserting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum TransferMode {
    /// Create the target table when missing, then append.
    CreateAppend,
    /// Target must exist; append rows.
    Append,
    /// Target must exist; delete its rows, then append.
    TruncateAppend,
    /// Drop and recreate the target from source columns, then insert.
    DropCreate,
}

/// What to transfer.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TransferRequest {
    pub source: SyncEndpoint,
    pub target: SyncEndpoint,
    pub source_table: String,
    /// Target table name; defaults to `source_table`.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub target_table: Option<String>,
    pub mode: TransferMode,
    /// Columns to copy; empty = every source column (names must match).
    #[serde(default)]
    pub columns: Vec<String>,
    /// Optional `WHERE` fragment applied on the source query.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub where_clause: Option<String>,
    /// Rows per INSERT batch (default 250, max 1000).
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub batch_size: Option<u32>,
    /// Stop after this many rows.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    #[ts(type = "number | null")]
    pub limit: Option<u64>,
}

/// Outcome of a transfer.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TransferResult {
    #[ts(type = "number")]
    pub rows: u64,
    #[ts(type = "number")]
    pub elapsed_ms: u64,
    pub target_table: String,
    /// True when the target table was created by this transfer.
    pub created: bool,
    /// True when `limit` cut the transfer short.
    pub truncated: bool,
    #[serde(default)]
    pub warnings: Vec<String>,
}

impl Api {
    /// Stream rows from `request.source` into `request.target`.
    pub async fn transfer_table(&self, request: TransferRequest) -> ApiResult<TransferResult> {
        let _ = request;
        Err(MieoError::unsupported("data transfer is not implemented yet").into())
    }
}
