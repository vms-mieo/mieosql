//! Query execution protocol: options, events (streaming), stats.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::error::ErrorInfo;
use crate::model::ColumnMeta;
use crate::value::Value;

/// Default number of rows per `Batch` event.
pub const DEFAULT_FETCH_SIZE: u32 = 256;

/// How the statement is sent to the server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum QueryMode {
    /// Server-side prepared statement with bound parameters. Default.
    #[default]
    Prepared,
    /// Unprepared text execution — required for `USE`, some DDL, `SET`, ...
    /// Bound parameters are rejected in this mode.
    Simple,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct QueryOptions {
    #[serde(default)]
    pub mode: QueryMode,
    /// Rows per streamed batch.
    #[serde(default = "default_fetch_size")]
    pub fetch_size: u32,
    /// Hard cap on returned rows; `None` = engine default.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    #[ts(type = "number | null")]
    pub max_rows: Option<u64>,
    /// Server-side timeout in milliseconds, if supported.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    #[ts(type = "number | null")]
    pub timeout_ms: Option<u64>,
}

fn default_fetch_size() -> u32 {
    DEFAULT_FETCH_SIZE
}

impl Default for QueryOptions {
    fn default() -> Self {
        Self {
            mode: QueryMode::default(),
            fetch_size: DEFAULT_FETCH_SIZE,
            max_rows: None,
            timeout_ms: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum MessageLevel {
    Info,
    Warning,
}

/// Streamed events of one query execution.
///
/// Internally tagged (`type`) so the frontend can exhaustively switch on it.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
#[ts(export)]
pub enum QueryEvent {
    /// Emitted once when execution actually starts on the server.
    Started { query_id: String, at: String },
    /// Result-set columns (sent before the first `Batch`).
    Columns {
        query_id: String,
        columns: Vec<ColumnMeta>,
    },
    /// A chunk of rows.
    Batch {
        query_id: String,
        rows: Vec<Vec<Value>>,
        /// Sequence number of this batch, starting at 0.
        #[ts(type = "number")]
        batch_index: u64,
    },
    /// Emitted for non-SELECT statements.
    Affected {
        query_id: String,
        #[ts(type = "number")]
        rows_affected: u64,
        #[ts(type = "number | null")]
        last_insert_id: Option<u64>,
    },
    /// Informational message from the server/driver (e.g. warnings).
    Message {
        query_id: String,
        level: MessageLevel,
        text: String,
    },
    /// Execution failed; `Finished` will not follow.
    Error { query_id: String, error: ErrorInfo },
    /// Terminal event on success.
    Finished { query_id: String, stats: QueryStats },
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct QueryStats {
    /// Rows streamed to the client.
    #[ts(type = "number")]
    pub rows: u64,
    /// Rows affected on the server (UPDATE/INSERT/DELETE).
    #[ts(type = "number")]
    pub rows_affected: u64,
    #[ts(type = "number")]
    pub elapsed_ms: u64,
    /// True when `max_rows` cut the result short.
    #[serde(default)]
    pub truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    #[ts(type = "number | null")]
    pub last_insert_id: Option<u64>,
}
