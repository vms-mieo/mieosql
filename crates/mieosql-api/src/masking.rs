//! Data Masking: export query results with sensitive columns masked, so
//! production data can safely reach DEV/UAT.
//!
//! Part of the v1 feature pack — frozen contract. Masking is applied to the
//! streamed `Value`s right before they are written, so files never contain
//! unmasked rows even transiently in the IPC layer.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use mieosql_core::MieoError;

use crate::dto::ApiResult;
use crate::export::{ExportFormat, ExportStats};
use crate::service::Api;

/// Masking strategy for one column.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[ts(export)]
pub enum MaskStrategy {
    Null,
    /// Keep `keep_start` leading and `keep_end` trailing chars, mask the rest.
    Redact {
        keep_start: u32,
        keep_end: u32,
    },
    /// Replace with a constant value.
    Fixed {
        value: String,
    },
    /// Fake e-mail address (same fake value per distinct input, per run).
    Email,
    Phone,
    Name,
    Company,
    City,
    Address,
    RandomString {
        min_len: u32,
        max_len: u32,
    },
    /// Deterministic pseudonym: hash of the original + optional salt.
    /// Non-cryptographic (FNV-1a/SipHash) — good for de-identification, not
    /// for security-critical irreversibility.
    Hash {
        salt: Option<String>,
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
    DateRecent {
        days: u32,
    },
}

/// Masking rule for one result column (matched case-insensitively).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct MaskColumnRule {
    pub column: String,
    pub strategy: MaskStrategy,
}

/// Re-run this SQL and export the result with masking applied.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct MaskExportRequest {
    /// The SQL whose result will be exported (usually a result's recorded SQL).
    pub sql: String,
    pub format: ExportFormat,
    pub path: String,
    /// Table name used for SQL-INSERT exports.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub table: Option<String>,
    #[serde(default)]
    pub rules: Vec<MaskColumnRule>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    #[ts(type = "number | null")]
    pub seed: Option<u64>,
}

/// Export result plus masking report.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct MaskExportResult {
    pub stats: ExportStats,
    /// Rules that matched a result column and were applied.
    pub masked_columns: u32,
    /// Rules whose column did not exist in the result (typos, stale config).
    #[serde(default)]
    pub unmatched_rules: Vec<String>,
}

impl Api {
    /// Export the result of `request.sql` with the given masking rules.
    pub async fn export_result_masked(
        &self,
        session_id: String,
        request: MaskExportRequest,
    ) -> ApiResult<MaskExportResult> {
        let _ = (session_id, request);
        Err(MieoError::unsupported("masked export is not implemented yet").into())
    }
}
