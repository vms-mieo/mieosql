//! Visual Explain: run an engine-native EXPLAIN and return a structured plan
//! tree the frontend can render (costs, rows, nodes, raw text).
//!
//! Part of the v1 feature pack — frozen contract.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use mieosql_core::{Engine, MieoError};

use crate::dto::ApiResult;
use crate::service::Api;

/// Which native shape the plan was read from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ExplainFormat {
    /// `EXPLAIN FORMAT=JSON` (MySQL) or `EXPLAIN (FORMAT JSON)` (Postgres).
    Json,
    /// Plain-text plan.
    Text,
    /// `EXPLAIN QUERY PLAN` rows (SQLite).
    QueryPlan,
}

/// One node of the execution plan tree.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ExplainNode {
    /// Short display label, e.g. `Seq Scan on users`.
    pub label: String,
    /// Engine node type, e.g. `Seq Scan`, `Nested Loop`, `SCAN`.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub kind: Option<String>,
    /// Relation/table touched, when reported.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub relation: Option<String>,
    /// Estimated total cost.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub cost: Option<f64>,
    /// Planner row estimate.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub rows_estimate: Option<f64>,
    /// Actual rows (EXPLAIN ANALYZE only).
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub actual_rows: Option<f64>,
    /// Actual total time in ms (EXPLAIN ANALYZE only).
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub actual_ms: Option<f64>,
    /// Extra human-readable detail (filters, using filesort, ...).
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub detail: Option<String>,
    pub children: Vec<ExplainNode>,
}

/// A complete, renderable plan.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ExplainPlan {
    pub engine: Engine,
    pub format: ExplainFormat,
    pub root: ExplainNode,
    /// Pretty raw output for a "Raw" view.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub raw: Option<String>,
    /// True when the plan came from EXPLAIN ANALYZE (real execution).
    pub analyzed: bool,
    #[ts(type = "number")]
    pub elapsed_ms: u64,
    /// Estimated total cost of the root node, when available.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub total_cost: Option<f64>,
    /// Largest cost in the tree — lets the UI scale cost bars consistently.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub max_cost: Option<f64>,
    /// The statement that was explained (normalized).
    pub sql: String,
}

impl Api {
    /// Explain `sql` on the session's engine.
    ///
    /// `analyze` requests real execution stats where the engine supports it
    /// (Postgres). Engines without ANALYZE support ignore the flag.
    pub async fn explain_query(
        &self,
        session_id: String,
        sql: String,
        analyze: bool,
    ) -> ApiResult<ExplainPlan> {
        let _ = (session_id, sql, analyze);
        Err(MieoError::unsupported("visual explain is not implemented yet").into())
    }
}
