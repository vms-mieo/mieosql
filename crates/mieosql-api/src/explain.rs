//! Visual Explain: run an engine-native EXPLAIN and return a structured plan
//! tree the frontend can render (costs, rows, nodes, raw text).
//!
//! Part of the v1 feature pack — frozen contract.

use std::collections::HashMap;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use ts_rs::TS;

use mieosql_core::{ColumnMeta, Engine, ErrorCode, MieoError, QueryMode, Value};

use crate::dto::ApiResult;
use crate::export::cell_text;
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
        let started = Instant::now();
        let sql = sql.trim().to_string();
        if sql.is_empty() {
            return Err(MieoError::new(ErrorCode::Data, "cannot explain an empty statement")
                .with_hint("type a SELECT statement first")
                .into());
        }

        let info = self.session_info(session_id.clone()).await?;
        let engine = info.engine;

        let mut plan = match engine {
            Engine::MySql | Engine::MariaDb => {
                self.explain_mysql(&session_id, &sql, engine).await?
            }
            Engine::Postgres => self.explain_postgres(&session_id, &sql, analyze).await?,
            Engine::Sqlite => self.explain_sqlite(&session_id, &sql).await?,
            other => {
                return Err(MieoError::unsupported(format!(
                    "visual explain is not supported for {}",
                    other.label()
                ))
                .into());
            }
        };

        plan.elapsed_ms = started.elapsed().as_millis() as u64;
        Ok(plan)
    }

    /// MySQL / MariaDB: `EXPLAIN FORMAT=JSON`, parsed best-effort.
    async fn explain_mysql(
        &self,
        session_id: &str,
        sql: &str,
        engine: Engine,
    ) -> ApiResult<ExplainPlan> {
        let statement = format!("EXPLAIN FORMAT=JSON {sql}");
        let out = self
            .collect_query(session_id, statement, Vec::new(), QueryMode::Simple, Some(1))
            .await?;
        let text = first_cell_text(&out.rows)?;
        let json = parse_json_plan(&text, engine)?;
        let root = mysql_root(&json);
        let (total_cost, max_cost) = tree_costs(&root);
        Ok(ExplainPlan {
            engine,
            format: ExplainFormat::Json,
            root,
            raw: Some(pretty_json(&json, &text)),
            analyzed: false,
            elapsed_ms: 0,
            total_cost,
            max_cost,
            sql: sql.to_string(),
        })
    }

    /// Postgres: `EXPLAIN (FORMAT JSON[, ANALYZE, BUFFERS])`.
    async fn explain_postgres(
        &self,
        session_id: &str,
        sql: &str,
        analyze: bool,
    ) -> ApiResult<ExplainPlan> {
        let plan_options = if analyze {
            "FORMAT JSON, ANALYZE, BUFFERS"
        } else {
            "FORMAT JSON"
        };
        let statement = format!("EXPLAIN ({plan_options}) {sql}");
        let out = self
            .collect_query(session_id, statement, Vec::new(), QueryMode::Simple, Some(1))
            .await?;
        let text = first_cell_text(&out.rows)?;
        let json = parse_json_plan(&text, Engine::Postgres)?;
        let root = pg_root(&json)?;
        let (total_cost, max_cost) = tree_costs(&root);
        Ok(ExplainPlan {
            engine: Engine::Postgres,
            format: ExplainFormat::Json,
            root,
            raw: Some(pretty_json(&json, &text)),
            analyzed: analyze,
            elapsed_ms: 0,
            total_cost,
            max_cost,
            sql: sql.to_string(),
        })
    }

    /// SQLite: `EXPLAIN QUERY PLAN`, mapped from `(id, parent, detail)` rows.
    async fn explain_sqlite(&self, session_id: &str, sql: &str) -> ApiResult<ExplainPlan> {
        let statement = format!("EXPLAIN QUERY PLAN {sql}");
        let out = self
            .collect_query(session_id, statement, Vec::new(), QueryMode::Simple, None)
            .await?;
        let rows = sqlite_rows(&out.columns, &out.rows);
        let root = sqlite_tree(&rows);
        let raw = rows
            .iter()
            .map(|row| row.detail.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let (total_cost, max_cost) = tree_costs(&root);
        Ok(ExplainPlan {
            engine: Engine::Sqlite,
            format: ExplainFormat::QueryPlan,
            root,
            raw: Some(raw),
            analyzed: false,
            elapsed_ms: 0,
            total_cost,
            max_cost,
            sql: sql.to_string(),
        })
    }
}

// ---------------------------------------------------------------------------
// Shared parsing helpers
// ---------------------------------------------------------------------------

/// First cell of the first row as text; engines return the plan as one cell.
fn first_cell_text(rows: &[Vec<Value>]) -> ApiResult<String> {
    let cell = rows
        .first()
        .and_then(|row| row.first())
        .ok_or_else(|| MieoError::new(ErrorCode::Data, "EXPLAIN returned no result"))?;
    let text = cell_text(cell);
    if text.trim().is_empty() {
        return Err(MieoError::new(ErrorCode::Data, "EXPLAIN returned an empty plan").into());
    }
    Ok(text)
}

/// Parse the plan document; a failure is reported as structured `Data` error.
fn parse_json_plan(text: &str, engine: Engine) -> ApiResult<JsonValue> {
    serde_json::from_str(text.trim()).map_err(|e| {
        MieoError::new(
            ErrorCode::Data,
            format!("cannot parse {} plan JSON: {e}", engine.label()),
        )
        .with_hint("the engine returned a plan this build could not read")
        .into()
    })
}

fn pretty_json(json: &JsonValue, fallback: &str) -> String {
    serde_json::to_string_pretty(json).unwrap_or_else(|_| fallback.trim().to_string())
}

/// Root cost plus the largest cost anywhere in the tree.
fn tree_costs(root: &ExplainNode) -> (Option<f64>, Option<f64>) {
    fn walk(node: &ExplainNode, max: &mut Option<f64>) {
        if let Some(cost) = node.cost.filter(|c| c.is_finite()) {
            *max = Some(max.map_or(cost, |m: f64| m.max(cost)));
        }
        for child in &node.children {
            walk(child, max);
        }
    }

    let mut max = None;
    walk(root, &mut max);
    (root.cost.filter(|c| c.is_finite()).or(max), max)
}

fn json_f64(value: Option<&JsonValue>) -> Option<f64> {
    match value? {
        JsonValue::Number(number) => number.as_f64().filter(|v| v.is_finite()),
        JsonValue::String(text) => parse_loose_f64(text),
        _ => None,
    }
}

fn json_u64(value: Option<&JsonValue>) -> Option<u64> {
    match value? {
        JsonValue::Number(number) => number.as_u64(),
        JsonValue::String(text) => text.trim().parse().ok(),
        _ => None,
    }
}

fn json_is_true(value: Option<&JsonValue>) -> bool {
    match value {
        Some(JsonValue::Bool(flag)) => *flag,
        Some(JsonValue::String(text)) => text.eq_ignore_ascii_case("true"),
        _ => false,
    }
}

fn json_string_list(value: &JsonValue) -> Vec<String> {
    match value {
        JsonValue::Array(items) => items
            .iter()
            .filter_map(JsonValue::as_str)
            .map(str::to_string)
            .collect(),
        JsonValue::String(text) if !text.is_empty() => vec![text.clone()],
        _ => Vec::new(),
    }
}

/// Numeric string from a JSON plan, tolerating `K`/`M`/`G` suffixes.
fn parse_loose_f64(text: &str) -> Option<f64> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    if let Ok(value) = text.parse::<f64>() {
        return value.is_finite().then_some(value);
    }
    let (digits, factor) = match text.chars().last()? {
        'k' | 'K' => (&text[..text.len() - 1], 1_000.0),
        'm' | 'M' => (&text[..text.len() - 1], 1_000_000.0),
        'g' | 'G' => (&text[..text.len() - 1], 1_000_000_000.0),
        _ => return None,
    };
    digits
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite())
        .map(|v| v * factor)
}

// ---------------------------------------------------------------------------
// MySQL / MariaDB
// ---------------------------------------------------------------------------

/// Build the renderable tree from a MySQL/MariaDB `FORMAT=JSON` document.
fn mysql_root(json: &JsonValue) -> ExplainNode {
    if let Some(query_block) = json.get("query_block") {
        let mut children = Vec::new();
        mysql_walk(query_block, &mut children);
        let label = match json_u64(query_block.get("select_id")) {
            Some(id) => format!("Query block #{id}"),
            None => "Query block".to_string(),
        };
        ExplainNode {
            label,
            kind: Some("query_block".to_string()),
            relation: None,
            cost: mysql_cost(query_block.get("cost_info")),
            rows_estimate: None,
            actual_rows: None,
            actual_ms: None,
            detail: None,
            children,
        }
    } else {
        let mut children = Vec::new();
        mysql_walk(json, &mut children);
        ExplainNode {
            label: "Query".to_string(),
            kind: None,
            relation: None,
            cost: None,
            rows_estimate: None,
            actual_rows: None,
            actual_ms: None,
            detail: None,
            children,
        }
    }
}

/// Best-effort recursive walk: objects with `table_name` become nodes, nested
/// structures (subqueries, unions, materialization) attach below them.
fn mysql_walk(value: &JsonValue, out: &mut Vec<ExplainNode>) {
    match value {
        JsonValue::Object(map) => {
            if let Some(name) = map.get("table_name").and_then(JsonValue::as_str) {
                let mut node = mysql_table_node(map, name);
                for (key, nested) in map {
                    if key == "table_name" {
                        continue;
                    }
                    let mut children = Vec::new();
                    mysql_walk(nested, &mut children);
                    node.children.extend(children);
                }
                out.push(node);
            } else {
                for nested in map.values() {
                    mysql_walk(nested, out);
                }
            }
        }
        JsonValue::Array(items) => {
            for nested in items {
                mysql_walk(nested, out);
            }
        }
        _ => {}
    }
}

fn mysql_table_node(table: &serde_json::Map<String, JsonValue>, name: &str) -> ExplainNode {
    let kind = table
        .get("access_type")
        .and_then(JsonValue::as_str)
        .map(str::to_string);
    let cost = mysql_cost(table.get("cost_info"))
        .or_else(|| json_f64(table.get("read_cost")))
        .or_else(|| json_f64(table.get("cost")));
    let rows_estimate = json_f64(table.get("rows_examined_per_scan"))
        .or_else(|| json_f64(table.get("rows_produced_per_join")))
        .or_else(|| json_f64(table.get("rows")));

    let mut details: Vec<String> = Vec::new();
    if let Some(key) = table
        .get("key")
        .and_then(JsonValue::as_str)
        .filter(|key| !key.is_empty())
    {
        details.push(format!("key: {key}"));
    }
    if let Some(possible) = table.get("possible_keys") {
        let list = json_string_list(possible);
        if !list.is_empty() {
            details.push(format!("possible keys: {}", list.join(", ")));
        }
    }
    if let Some(condition) = table
        .get("attached_condition")
        .and_then(JsonValue::as_str)
        .filter(|condition| !condition.trim().is_empty())
    {
        details.push(format!("condition: {condition}"));
    }
    for (field, label) in [
        ("using_filesort", "using filesort"),
        ("using_temporary_table", "using temporary table"),
        ("using_index", "using index"),
    ] {
        if json_is_true(table.get(field)) {
            details.push(label.to_string());
        }
    }
    if let Some(buffer) = table.get("using_join_buffer").and_then(JsonValue::as_str) {
        details.push(format!("using join buffer ({buffer})"));
    }

    ExplainNode {
        label: name.to_string(),
        kind,
        relation: Some(name.to_string()),
        cost,
        rows_estimate,
        actual_rows: None,
        actual_ms: None,
        detail: (!details.is_empty()).then(|| details.join("; ")),
        children: Vec::new(),
    }
}

fn mysql_cost(cost_info: Option<&JsonValue>) -> Option<f64> {
    let map = cost_info?.as_object()?;
    json_f64(map.get("query_cost"))
        .or_else(|| json_f64(map.get("read_cost")))
        .or_else(|| json_f64(map.get("prefix_cost")))
}

// ---------------------------------------------------------------------------
// Postgres
// ---------------------------------------------------------------------------

/// Strict parse of `[{ "Plan": {...} }]`.
fn pg_root(json: &JsonValue) -> ApiResult<ExplainNode> {
    let plan = match json {
        JsonValue::Array(items) => items.first(),
        _ => Some(json),
    }
    .and_then(|entry| entry.get("Plan"))
    .ok_or_else(|| {
        MieoError::new(ErrorCode::Data, "unexpected PostgreSQL plan shape")
            .with_hint("expected a `[{\"Plan\": ...}]` document")
    })?;
    pg_node(plan)
}

fn pg_node(plan: &JsonValue) -> ApiResult<ExplainNode> {
    let map = plan
        .as_object()
        .ok_or_else(|| MieoError::new(ErrorCode::Data, "PostgreSQL plan node is not an object"))?;
    let node_type = map
        .get("Node Type")
        .and_then(JsonValue::as_str)
        .ok_or_else(|| MieoError::new(ErrorCode::Data, "PostgreSQL plan node is missing `Node Type`"))?;
    let relation = map
        .get("Relation Name")
        .and_then(JsonValue::as_str)
        .filter(|name| !name.is_empty());
    let label = match relation {
        Some(name) => format!("{node_type} on {name}"),
        None => node_type.to_string(),
    };

    let mut details = Vec::new();
    for key in [
        "Join Filter",
        "Index Cond",
        "Hash Cond",
        "Merge Cond",
        "Recheck Cond",
        "Filter",
        "Sort Key",
        "Group Key",
    ] {
        if let Some(value) = map.get(key).and_then(pg_text) {
            details.push(format!("{key}: {value}"));
        }
    }

    let children = match map.get("Plans") {
        Some(JsonValue::Array(items)) => items.iter().map(pg_node).collect::<ApiResult<Vec<_>>>()?,
        _ => Vec::new(),
    };

    Ok(ExplainNode {
        label,
        kind: Some(node_type.to_string()),
        relation: relation.map(str::to_string),
        cost: json_f64(map.get("Total Cost")),
        rows_estimate: json_f64(map.get("Plan Rows")),
        actual_rows: json_f64(map.get("Actual Rows")),
        actual_ms: json_f64(map.get("Actual Total Time")),
        detail: (!details.is_empty()).then(|| details.join("; ")),
        children,
    })
}

fn pg_text(value: &JsonValue) -> Option<String> {
    match value {
        JsonValue::String(text) => Some(text.clone()),
        JsonValue::Array(items) => {
            let parts: Vec<String> = items.iter().filter_map(pg_text).collect();
            (!parts.is_empty()).then(|| parts.join(", "))
        }
        JsonValue::Number(number) => Some(number.to_string()),
        JsonValue::Bool(flag) => Some(flag.to_string()),
        JsonValue::Null | JsonValue::Object(_) => None,
    }
}

// ---------------------------------------------------------------------------
// SQLite
// ---------------------------------------------------------------------------

/// One `EXPLAIN QUERY PLAN` row (id, parent, detail).
#[derive(Debug, Clone)]
struct SqlitePlanRow {
    id: i64,
    parent: i64,
    detail: String,
}

fn sqlite_rows(columns: &[ColumnMeta], rows: &[Vec<Value>]) -> Vec<SqlitePlanRow> {
    let find = |name: &str| {
        columns
            .iter()
            .position(|column| column.name.eq_ignore_ascii_case(name))
    };
    let id_index = find("id").unwrap_or(0);
    let parent_index = find("parent").unwrap_or(1);
    let detail_index = find("detail").unwrap_or_else(|| {
        // Older SQLite omits the `notused` column and returns 3 columns.
        let width = rows.first().map(Vec::len).unwrap_or(columns.len());
        width.saturating_sub(1).max(2)
    });

    rows.iter()
        .map(|row| SqlitePlanRow {
            id: row.get(id_index).and_then(value_i64).unwrap_or(0),
            parent: row.get(parent_index).and_then(value_i64).unwrap_or(0),
            detail: row.get(detail_index).map(cell_text).unwrap_or_default(),
        })
        .collect()
}

fn value_i64(value: &Value) -> Option<i64> {
    match value {
        Value::Int(number) => Some(*number),
        Value::UInt(number) => i64::try_from(*number).ok(),
        Value::Float(number) if number.is_finite() => Some(*number as i64),
        Value::Text(text) | Value::Decimal(text) => text.trim().parse().ok(),
        _ => None,
    }
}

/// Map `(id, parent)` rows into a tree. SQLite emits parents before children,
/// so requiring `parent < index` both validates the link and guarantees
/// termination on malformed output.
fn sqlite_tree(rows: &[SqlitePlanRow]) -> ExplainNode {
    let mut nodes: Vec<ExplainNode> = rows
        .iter()
        .map(|row| {
            let detail = row.detail.trim().to_string();
            let kind = detail.split_whitespace().next().map(str::to_string);
            ExplainNode {
                label: if detail.is_empty() {
                    "(unknown)".to_string()
                } else {
                    detail
                },
                kind,
                relation: None,
                cost: None,
                rows_estimate: None,
                actual_rows: None,
                actual_ms: None,
                detail: None,
                children: Vec::new(),
            }
        })
        .collect();

    let id_to_index: HashMap<i64, usize> =
        rows.iter().enumerate().map(|(i, row)| (row.id, i)).collect();
    let mut children_of: Vec<Vec<usize>> = vec![Vec::new(); rows.len()];
    let mut roots: Vec<usize> = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let parent = (row.parent != 0)
            .then(|| id_to_index.get(&row.parent).copied())
            .flatten()
            .filter(|&parent| parent < index);
        match parent {
            Some(parent) => children_of[parent].push(index),
            None => roots.push(index),
        }
    }

    let mut top_level = Vec::with_capacity(roots.len());
    for index in roots {
        top_level.push(sqlite_take_node(index, &mut nodes, &children_of));
    }

    match top_level.len() {
        1 => top_level
            .into_iter()
            .next()
            .unwrap_or_else(|| sqlite_synthetic_root(Vec::new())),
        _ => sqlite_synthetic_root(top_level),
    }
}

fn sqlite_take_node(index: usize, nodes: &mut [ExplainNode], children_of: &[Vec<usize>]) -> ExplainNode {
    let placeholder = ExplainNode {
        label: String::new(),
        kind: None,
        relation: None,
        cost: None,
        rows_estimate: None,
        actual_rows: None,
        actual_ms: None,
        detail: None,
        children: Vec::new(),
    };
    let mut node = std::mem::replace(&mut nodes[index], placeholder);
    node.children = children_of[index]
        .iter()
        .map(|&child| sqlite_take_node(child, nodes, children_of))
        .collect();
    node
}

fn sqlite_synthetic_root(children: Vec<ExplainNode>) -> ExplainNode {
    ExplainNode {
        label: "Query plan".to_string(),
        kind: Some("QUERY PLAN".to_string()),
        relation: None,
        cost: None,
        rows_estimate: None,
        actual_rows: None,
        actual_ms: None,
        detail: None,
        children,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_mysql_plan() -> JsonValue {
        serde_json::json!({
            "query_block": {
                "select_id": 1,
                "cost_info": { "query_cost": "1.20" },
                "table": {
                    "table_name": "users",
                    "access_type": "ALL",
                    "rows_examined_per_scan": 100,
                    "cost_info": { "read_cost": "1.05" },
                    "attached_condition": "(`shop`.`users`.`id` > 10)",
                    "using_filesort": true
                }
            }
        })
    }

    fn sample_postgres_plan() -> JsonValue {
        serde_json::json!([
            {
                "Plan": {
                    "Node Type": "Nested Loop",
                    "Total Cost": 20.5,
                    "Plan Rows": 5,
                    "Plans": [
                        {
                            "Node Type": "Seq Scan",
                            "Relation Name": "users",
                            "Total Cost": 10.0,
                            "Plan Rows": 3,
                            "Filter": "(id > 1)",
                            "Actual Rows": 3,
                            "Actual Total Time": 0.123
                        },
                        {
                            "Node Type": "Index Scan",
                            "Relation Name": "orders",
                            "Total Cost": 8.5,
                            "Plan Rows": 2,
                            "Index Cond": "(user_id = users.id)"
                        }
                    ]
                },
                "Planning Time": 0.2,
                "Execution Time": 1.0
            }
        ])
    }

    #[test]
    fn mysql_json_builds_query_block_tree() {
        let root = mysql_root(&sample_mysql_plan());
        assert_eq!(root.label, "Query block #1");
        assert_eq!(root.kind.as_deref(), Some("query_block"));
        assert_eq!(root.cost, Some(1.20));
        assert_eq!(root.children.len(), 1);

        let table = &root.children[0];
        assert_eq!(table.label, "users");
        assert_eq!(table.kind.as_deref(), Some("ALL"));
        assert_eq!(table.relation.as_deref(), Some("users"));
        assert_eq!(table.cost, Some(1.05));
        assert_eq!(table.rows_estimate, Some(100.0));
        let detail = table.detail.as_deref().unwrap_or_default();
        assert!(detail.contains("condition: (`shop`.`users`.`id` > 10)"));
        assert!(detail.contains("using filesort"));

        let (total, max) = tree_costs(&root);
        assert_eq!(total, Some(1.20));
        assert_eq!(max, Some(1.20));
    }

    #[test]
    fn mysql_walk_nests_subquery_tables() {
        let json = serde_json::json!({
            "query_block": {
                "select_id": 1,
                "table": {
                    "table_name": "users",
                    "access_type": "ALL",
                    "attached_subqueries": [
                        {
                            "query_block": {
                                "select_id": 2,
                                "table": { "table_name": "orders", "access_type": "ref" }
                            }
                        }
                    ]
                }
            }
        });
        let root = mysql_root(&json);
        assert_eq!(root.children.len(), 1);
        let subquery_children = &root.children[0].children;
        assert_eq!(subquery_children.len(), 1);
        assert_eq!(subquery_children[0].label, "orders");
        assert_eq!(subquery_children[0].kind.as_deref(), Some("ref"));
    }

    #[test]
    fn mysql_falls_back_to_plain_object() {
        let json = serde_json::json!({
            "table": {
                "table_name": "t1",
                "access_type": "ALL",
                "rows": 7
            }
        });
        let root = mysql_root(&json);
        assert_eq!(root.label, "Query");
        assert_eq!(root.children.len(), 1);
        assert_eq!(root.children[0].rows_estimate, Some(7.0));
    }

    #[test]
    fn postgres_json_builds_strict_tree() {
        let root = pg_root(&sample_postgres_plan()).expect("valid plan");
        assert_eq!(root.label, "Nested Loop");
        assert_eq!(root.kind.as_deref(), Some("Nested Loop"));
        assert_eq!(root.cost, Some(20.5));
        assert_eq!(root.rows_estimate, Some(5.0));
        assert_eq!(root.children.len(), 2);

        let scan = &root.children[0];
        assert_eq!(scan.label, "Seq Scan on users");
        assert_eq!(scan.relation.as_deref(), Some("users"));
        assert_eq!(scan.actual_rows, Some(3.0));
        assert_eq!(scan.actual_ms, Some(0.123));
        assert!(scan.detail.as_deref().unwrap_or_default().contains("Filter: (id > 1)"));

        let index_scan = &root.children[1];
        assert_eq!(index_scan.label, "Index Scan on orders");
        assert!(
            index_scan
                .detail
                .as_deref()
                .unwrap_or_default()
                .contains("Index Cond: (user_id = users.id)")
        );

        let (total, max) = tree_costs(&root);
        assert_eq!(total, Some(20.5));
        assert_eq!(max, Some(20.5));
    }

    #[test]
    fn postgres_rejects_malformed_plans() {
        assert!(pg_root(&serde_json::json!([])).is_err());
        assert!(pg_root(&serde_json::json!([{ "NotAPlan": true }])).is_err());
        assert!(pg_root(&serde_json::json!([{ "Plan": { "Total Cost": 1.0 } }])).is_err());
    }

    fn row(id: i64, parent: i64, detail: &str) -> SqlitePlanRow {
        SqlitePlanRow {
            id,
            parent,
            detail: detail.to_string(),
        }
    }

    #[test]
    fn sqlite_rows_are_mapped_by_parent_id() {
        let rows = vec![
            row(5, 0, "LIST SUBQUERY 1"),
            row(9, 5, "SCAN emails"),
            row(6, 0, "SEARCH users USING INTEGER PRIMARY KEY (rowid=?)"),
        ];
        let root = sqlite_tree(&rows);
        assert_eq!(root.label, "Query plan");
        assert_eq!(root.kind.as_deref(), Some("QUERY PLAN"));
        assert_eq!(root.children.len(), 2);

        let subquery = &root.children[0];
        assert_eq!(subquery.label, "LIST SUBQUERY 1");
        assert_eq!(subquery.kind.as_deref(), Some("LIST"));
        assert_eq!(subquery.children.len(), 1);
        assert_eq!(subquery.children[0].label, "SCAN emails");
        assert_eq!(subquery.children[0].kind.as_deref(), Some("SCAN"));

        let search = &root.children[1];
        assert_eq!(search.kind.as_deref(), Some("SEARCH"));
        assert_eq!(search.label, "SEARCH users USING INTEGER PRIMARY KEY (rowid=?)");
    }

    #[test]
    fn sqlite_single_root_is_used_directly() {
        let rows = vec![row(2, 0, "SCAN CONSTANT ROW")];
        let root = sqlite_tree(&rows);
        assert_eq!(root.label, "SCAN CONSTANT ROW");
        assert_eq!(root.kind.as_deref(), Some("SCAN"));
        assert!(root.children.is_empty());
    }

    #[test]
    fn sqlite_unknown_parents_become_roots() {
        let rows = vec![row(3, 99, "SCAN t")];
        let root = sqlite_tree(&rows);
        assert_eq!(root.label, "SCAN t");
    }

    #[test]
    fn parses_loose_numbers() {
        assert_eq!(parse_loose_f64("1.20"), Some(1.20));
        assert_eq!(parse_loose_f64("15K"), Some(15_000.0));
        assert_eq!(parse_loose_f64("2M"), Some(2_000_000.0));
        assert_eq!(parse_loose_f64("not a number"), None);
        assert_eq!(json_f64(Some(&serde_json::json!("3.5"))), Some(3.5));
        assert_eq!(json_f64(Some(&serde_json::json!(7))), Some(7.0));
    }
}
