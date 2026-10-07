//! Data Generation: synthesize realistic test rows for a table.
//!
//! Part of the v1 feature pack — frozen contract. The generator is
//! deterministic for a given `seed`, engine-agnostic (values are rendered as
//! SQL literals), and can either preview the INSERT statements or execute
//! them against the session.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use mieosql_core::{MieoError, Scope};

use crate::dto::ApiResult;
use crate::service::Api;

/// Whether to preview or execute.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum GenMode {
    /// Only render the INSERT statements (capped preview), touch nothing.
    PreviewSql,
    /// Execute the generated INSERTs on the session.
    Execute,
}

/// Per-column generation strategy.
///
/// `Pattern` template tokens:
/// `{n}` digit 0-9 · `{N}` digit 1-9 · `{a}` lowercase letter ·
/// `{A}` uppercase letter · `{x}` hex digit · `{{`/`}}` literal braces.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[ts(export)]
pub enum GenStrategy {
    /// Pick by column type/name heuristics.
    Auto,
    Null,
    /// 1, 2, 3, ... (start + step).
    Sequential {
        start: i32,
        step: i32,
    },
    RandomInt {
        min: i32,
        max: i32,
    },
    RandomDecimal {
        min: f64,
        max: f64,
        scale: u32,
    },
    RandomString {
        min_len: u32,
        max_len: u32,
    },
    /// Random words from a built-in list.
    Lorem {
        words: u32,
    },
    Email,
    FullName,
    Phone,
    Company,
    City,
    Country,
    Address,
    Bool,
    /// Date within the last `days` days (YYYY-MM-DD).
    DateRecent {
        days: u32,
    },
    /// Datetime within the last `days` days (RFC 3339).
    DateTimeRecent {
        days: u32,
    },
    Uuid,
    /// Formatted string, see tokens above.
    Pattern {
        template: String,
    },
    /// Uniform pick from a fixed list.
    Pick {
        values: Vec<String>,
    },
    /// The same value for every row.
    Constant {
        value: String,
    },
    /// SQL expression passed through verbatim (advanced).
    Expression {
        sql: String,
    },
}

/// Rule overriding generation for one column.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct GenColumnRule {
    pub column: String,
    pub strategy: GenStrategy,
}

/// What to generate.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct GenRequest {
    pub scope: Scope,
    pub table: String,
    /// Number of rows (1..=200 000).
    pub rows: u32,
    /// Deterministic seed; `None` = time-based.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    #[ts(type = "number | null")]
    pub seed: Option<u64>,
    /// Per-column overrides; columns without a rule use `Auto`.
    #[serde(default)]
    pub rules: Vec<GenColumnRule>,
    /// Rows per INSERT statement (default 250, max 1000).
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub batch_size: Option<u32>,
    pub mode: GenMode,
}

/// Result of a generation run.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct GenResult {
    /// Preview SQL (always filled; capped in `Execute` mode).
    pub sql: String,
    /// Rows generated (or executed).
    pub rows: u32,
    pub executed: bool,
    #[ts(type = "number")]
    pub elapsed_ms: u64,
    /// True when the preview string was capped.
    pub truncated: bool,
    /// Columns skipped because they are auto-increment / generated.
    #[serde(default)]
    pub skipped_columns: Vec<String>,
}

impl Api {
    /// Generate test data for `request.table`.
    pub async fn generate_data(
        &self,
        session_id: String,
        request: GenRequest,
    ) -> ApiResult<GenResult> {
        let _ = (session_id, request);
        Err(MieoError::unsupported("data generation is not implemented yet").into())
    }
}
