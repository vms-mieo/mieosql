//! Data Generation: synthesize realistic test rows for a table.
//!
//! Part of the v1 feature pack — frozen contract. The generator is
//! deterministic for a given `seed`, engine-agnostic (values are rendered as
//! SQL literals), and can either preview the INSERT statements or execute
//! them against the session.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use mieosql_core::{ColumnDef, DataTypeKind, Engine, ErrorCode, MieoError, Scope, Value};

use crate::dto::{ApiError, ApiResult};
use crate::export::sql_literal;
use crate::service::{quote_ident, Api};
use crate::util::{Prng, CITIES, COMPANIES, COUNTRIES, FIRST_NAMES, LAST_NAMES, STREETS, WORDS};

/// Hard cap on the SQL preview returned to the UI.
const MAX_PREVIEW_ROWS: u32 = 1000;
/// Rows per INSERT statement unless the request overrides it.
const DEFAULT_BATCH_SIZE: u32 = 250;
/// Safety cap on generated rows.
const MAX_ROWS: u32 = 200_000;
/// Charset for random string values (lowercase letters + digits).
const STRING_CHARSET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";

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

/// One generated cell: either a typed value or a raw SQL expression.
#[derive(Debug, Clone)]
enum Cell {
    Value(Value),
    /// Raw SQL passed through verbatim (`GenStrategy::Expression`).
    Sql(String),
}

impl Cell {
    fn render(&self, engine: Engine) -> String {
        match self {
            Cell::Value(value) => sql_literal(engine, value),
            Cell::Sql(sql) => sql.clone(),
        }
    }
}

impl Api {
    /// Generate test data for `request.table`.
    pub async fn generate_data(
        &self,
        session_id: String,
        request: GenRequest,
    ) -> ApiResult<GenResult> {
        let started = std::time::Instant::now();

        let table = request.table.trim();
        if table.is_empty() {
            return Err(MieoError::new(ErrorCode::Data, "table name is required").into());
        }

        let info = self.session_info(session_id.clone()).await?;
        let engine = info.engine;

        let columns = self
            .table_columns(session_id.clone(), request.scope.clone(), table.to_string())
            .await?;

        // Auto-increment / generated columns are filled by the database.
        let mut skipped_columns: Vec<String> = Vec::new();
        let mut active: Vec<ColumnDef> = Vec::with_capacity(columns.len());
        for column in columns {
            if column.is_auto_increment || column.is_generated {
                skipped_columns.push(column.name);
            } else {
                active.push(column);
            }
        }
        if active.is_empty() {
            return Err(MieoError::new(
                ErrorCode::Data,
                format!("table `{table}` has no insertable columns"),
            )
            .with_hint("every column is auto-increment or generated")
            .into());
        }

        let rows = request.rows.clamp(1, MAX_ROWS);
        let batch_size = request.batch_size.unwrap_or(DEFAULT_BATCH_SIZE).clamp(1, 1000);
        let preview_rows = rows.min(MAX_PREVIEW_ROWS);
        let truncated = rows > preview_rows;

        // Per-column overrides are matched by name, case-insensitively.
        let mut rule_map: HashMap<String, GenStrategy> = HashMap::new();
        for rule in request.rules {
            rule_map.insert(rule.column.to_ascii_lowercase(), rule.strategy);
        }
        let strategies: Vec<GenStrategy> = active
            .iter()
            .map(|column| {
                rule_map
                    .get(&column.name.to_ascii_lowercase())
                    .cloned()
                    .unwrap_or(GenStrategy::Auto)
            })
            .collect();

        let qualified = qualified_table(engine, &request.scope, table);
        let column_names: Vec<String> = active.iter().map(|column| column.name.clone()).collect();

        // One PRNG drives every cell, so a seed fully determines the output.
        let mut rng = Prng::new(request.seed);
        let generate_rows = match request.mode {
            GenMode::PreviewSql => preview_rows,
            GenMode::Execute => rows,
        };

        // Wrap the INSERTs in a transaction when the session is idle. The
        // caller keeps ownership of any transaction already in progress.
        let manage_tx = request.mode == GenMode::Execute && !info.in_transaction;
        if manage_tx {
            self.begin_transaction(session_id.clone()).await?;
        }

        let mut preview_cells: Vec<Vec<Cell>> = Vec::with_capacity(preview_rows as usize);
        let mut processed: u32 = 0;
        let mut failure: Option<ApiError> = None;

        while processed < generate_rows {
            let batch_rows = batch_size.min(generate_rows - processed);
            let mut batch: Vec<Vec<Cell>> = Vec::with_capacity(batch_rows as usize);
            for offset in 0..batch_rows {
                let row_index = processed + offset;
                let row: Vec<Cell> = strategies
                    .iter()
                    .zip(active.iter())
                    .map(|(strategy, column)| generate_cell(&mut rng, strategy, column, row_index))
                    .collect();
                if (preview_cells.len() as u32) < preview_rows {
                    preview_cells.push(row.clone());
                }
                batch.push(row);
            }

            if request.mode == GenMode::Execute {
                let sql = render_insert(engine, &qualified, &column_names, &batch);
                if let Err(error) = self.execute(&session_id, sql).await {
                    failure = Some(error);
                    break;
                }
            }
            processed += batch_rows;
        }

        if let Some(error) = failure {
            if manage_tx {
                // Best effort: the original INSERT error is the one surfaced.
                let _ = self.rollback_transaction(session_id.clone()).await;
            }
            return Err(error);
        }
        if manage_tx {
            self.commit_transaction(session_id.clone()).await?;
        }

        // Re-render the accumulated preview rows as INSERT batches.
        let mut statements: Vec<String> = Vec::new();
        for chunk in preview_cells.chunks(batch_size as usize) {
            statements.push(render_insert(engine, &qualified, &column_names, chunk));
        }

        Ok(GenResult {
            sql: statements.join("\n"),
            rows,
            executed: request.mode == GenMode::Execute,
            elapsed_ms: started.elapsed().as_millis() as u64,
            truncated,
            skipped_columns,
        })
    }
}

// ---------------------------------------------------------------------------
// SQL rendering
// ---------------------------------------------------------------------------

/// Qualified table name: Postgres prefers the schema, other engines prefer the
/// database (same rule as `build_select_sql`); empty qualifiers are dropped.
fn qualified_table(engine: Engine, scope: &Scope, table: &str) -> String {
    let table = quote_ident(engine, table);
    match engine {
        Engine::Postgres => match scope.schema.as_deref().filter(|s| !s.is_empty()) {
            Some(schema) => format!("{}.{table}", quote_ident(engine, schema)),
            None => table,
        },
        _ => match scope.database.as_deref().filter(|s| !s.is_empty()) {
            Some(database) => format!("{}.{table}", quote_ident(engine, database)),
            None => table,
        },
    }
}

/// Build one multi-row `INSERT` statement from generated cells.
fn render_insert(engine: Engine, qualified: &str, columns: &[String], rows: &[Vec<Cell>]) -> String {
    let columns = columns
        .iter()
        .map(|name| quote_ident(engine, name))
        .collect::<Vec<_>>()
        .join(", ");
    let values = rows
        .iter()
        .map(|row| {
            format!(
                "({})",
                row.iter()
                    .map(|cell| cell.render(engine))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!("INSERT INTO {qualified} ({columns}) VALUES {values};")
}

// ---------------------------------------------------------------------------
// Strategy resolution (Auto heuristics)
// ---------------------------------------------------------------------------

/// Pick a concrete strategy for `GenStrategy::Auto` from the column's name
/// and normalized type.
fn resolve_auto(column: &ColumnDef) -> GenStrategy {
    let name = column.name.to_ascii_lowercase();

    // Semantic names win over the storage type.
    if name.contains("email") || name.contains("mail") {
        return GenStrategy::Email;
    }
    if name.contains("phone") || name.contains("mobile") || contains_word(&name, "tel") {
        return GenStrategy::Phone;
    }
    if name.contains("company") || name.contains("org") {
        return GenStrategy::Company;
    }
    if name.contains("city") {
        return GenStrategy::City;
    }
    if name.contains("country") {
        return GenStrategy::Country;
    }
    if name.contains("address") {
        return GenStrategy::Address;
    }
    if name.contains("name") {
        return GenStrategy::FullName;
    }

    // Date-ish names (`created_at`, `updated_at`, `start_date`, ...).
    if name.contains("created")
        || name.contains("updated")
        || name.contains("date")
        || name.ends_with("_at")
    {
        return match &column.data_type {
            DataTypeKind::Date => GenStrategy::DateRecent { days: 90 },
            _ => GenStrategy::DateTimeRecent { days: 90 },
        };
    }

    match &column.data_type {
        DataTypeKind::Bool | DataTypeKind::Bit => GenStrategy::Bool,
        DataTypeKind::Uuid => GenStrategy::Uuid,
        DataTypeKind::Int | DataTypeKind::UInt => GenStrategy::RandomInt {
            min: 1,
            max: 100_000,
        },
        DataTypeKind::Float | DataTypeKind::Decimal => GenStrategy::RandomDecimal {
            min: 1.0,
            max: 99_999.0,
            scale: column.numeric_scale.unwrap_or(2).min(12),
        },
        DataTypeKind::Date => GenStrategy::DateRecent { days: 90 },
        DataTypeKind::DateTime | DataTypeKind::TimestampTz => {
            GenStrategy::DateTimeRecent { days: 90 }
        }
        // Strings (and everything else): bounded random text.
        _ => {
            let max = column.char_max_length.map(|n| n.min(40)).unwrap_or(40).max(1) as u32;
            GenStrategy::RandomString {
                min_len: max.min(5),
                max_len: max,
            }
        }
    }
}

/// Word-boundary check used to avoid false positives like `hotel` matching
/// the `tel` heuristic.
fn contains_word(name: &str, word: &str) -> bool {
    name == word
        || name.starts_with(&format!("{word}_"))
        || name.ends_with(&format!("_{word}"))
        || name.contains(&format!("_{word}_"))
}

// ---------------------------------------------------------------------------
// Value generation
// ---------------------------------------------------------------------------

/// Generate one cell. `Auto` resolves through [`resolve_auto`] and may yield
/// NULL for nullable columns; an explicitly chosen strategy always applies.
fn generate_cell(
    rng: &mut Prng,
    strategy: &GenStrategy,
    column: &ColumnDef,
    row_index: u32,
) -> Cell {
    if matches!(strategy, GenStrategy::Auto) {
        if column.nullable && rng.chance(5) {
            return Cell::Value(Value::Null);
        }
        let resolved = resolve_auto(column);
        return generate_cell(rng, &resolved, column, row_index);
    }
    generate_value(rng, strategy, row_index)
}

/// Generate a value for a concrete (non-`Auto`) strategy.
fn generate_value(rng: &mut Prng, strategy: &GenStrategy, row_index: u32) -> Cell {
    let cell = match strategy {
        // `Auto` is resolved by the caller; keep a defensive fallback.
        GenStrategy::Auto => Cell::Value(Value::Text(rng.string(8, STRING_CHARSET))),
        GenStrategy::Null => Cell::Value(Value::Null),
        GenStrategy::Sequential { start, step } => Cell::Value(Value::Int(
            i64::from(*start) + i64::from(*step) * i64::from(row_index),
        )),
        GenStrategy::RandomInt { min, max } => {
            Cell::Value(Value::Int(rng.range_i64(i64::from(*min), i64::from(*max))))
        }
        GenStrategy::RandomDecimal { min, max, scale } => {
            let value = if min < max {
                rng.range_f64(*min, *max)
            } else {
                *min
            };
            Cell::Value(Value::Decimal(format!(
                "{:.*}",
                (*scale).min(12) as usize,
                value
            )))
        }
        GenStrategy::RandomString { min_len, max_len } => {
            let len = random_len(rng, *min_len, *max_len);
            Cell::Value(Value::Text(rng.string(len, STRING_CHARSET)))
        }
        GenStrategy::Lorem { words } => {
            let count = (*words).min(1000) as usize;
            let mut parts = Vec::with_capacity(count);
            for _ in 0..count {
                parts.push(*rng.pick(WORDS));
            }
            Cell::Value(Value::Text(parts.join(" ")))
        }
        GenStrategy::Email => {
            let first = rng.pick(FIRST_NAMES).to_lowercase();
            let last = rng.pick(LAST_NAMES).to_lowercase();
            Cell::Value(Value::Text(format!(
                "{first}.{last}{}@example.com",
                rng.digits(2)
            )))
        }
        GenStrategy::FullName => Cell::Value(Value::Text(format!(
            "{} {}",
            rng.pick(FIRST_NAMES),
            rng.pick(LAST_NAMES)
        ))),
        GenStrategy::Phone => Cell::Value(Value::Text(format!("+84{}", rng.digits(9)))),
        GenStrategy::Company => Cell::Value(Value::Text((*rng.pick(COMPANIES)).to_string())),
        GenStrategy::City => Cell::Value(Value::Text((*rng.pick(CITIES)).to_string())),
        GenStrategy::Country => Cell::Value(Value::Text((*rng.pick(COUNTRIES)).to_string())),
        GenStrategy::Address => Cell::Value(Value::Text(format!(
            "{} {}",
            rng.range_i64(1, 999),
            rng.pick(STREETS)
        ))),
        GenStrategy::Bool => Cell::Value(Value::Bool(rng.chance(50))),
        GenStrategy::DateRecent { days } => {
            let days = i64::from((*days).max(1));
            let when = chrono::Utc::now() - chrono::Duration::days(rng.range_i64(0, days));
            Cell::Value(Value::Date(when.format("%Y-%m-%d").to_string()))
        }
        GenStrategy::DateTimeRecent { days } => {
            let days = i64::from((*days).max(1));
            let when = chrono::Utc::now() - chrono::Duration::days(rng.range_i64(0, days));
            Cell::Value(Value::DateTime(when.to_rfc3339()))
        }
        GenStrategy::Uuid => Cell::Value(Value::Uuid(format!(
            "{}-{}-4{}-a{}-{}",
            rng.hex(8),
            rng.hex(4),
            rng.hex(3),
            rng.hex(3),
            rng.hex(12)
        ))),
        GenStrategy::Pattern { template } => {
            Cell::Value(Value::Text(render_pattern(rng, template)))
        }
        GenStrategy::Pick { values } => match values.first() {
            None => Cell::Value(Value::Null),
            Some(_) => Cell::Value(Value::Text(rng.pick(values).clone())),
        },
        GenStrategy::Constant { value } => Cell::Value(Value::Text(value.clone())),
        GenStrategy::Expression { sql } => Cell::Sql(sql.clone()),
    };
    cell
}

/// Random length in `[min, max]`; tolerates inverted bounds and caps the
/// length so a pathological request cannot allocate unbounded strings.
fn random_len(rng: &mut Prng, min: u32, max: u32) -> usize {
    const MAX_LEN: usize = 4096;
    let min = (min as usize).min(MAX_LEN);
    let max = (max as usize).min(MAX_LEN);
    if min >= max {
        return min;
    }
    rng.range_usize(min, max.saturating_add(1))
}

/// Render a `GenStrategy::Pattern` template.
///
/// Tokens: `{n}` digit 0-9, `{N}` digit 1-9, `{a}` lowercase letter,
/// `{A}` uppercase letter, `{x}` hex digit, `{{`/`}}` literal braces.
/// Unknown tokens are emitted verbatim.
fn render_pattern(rng: &mut Prng, template: &str) -> String {
    let chars: Vec<char> = template.chars().collect();
    let mut out = String::with_capacity(chars.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '{' {
            if i + 1 < chars.len() && chars[i + 1] == '{' {
                out.push('{');
                i += 2;
                continue;
            }
            if i + 2 < chars.len() && chars[i + 2] == '}' {
                let token = match chars[i + 1] {
                    'n' => Some(rng.string(1, b"0123456789")),
                    'N' => Some(rng.range_i64(1, 9).to_string()),
                    'a' => Some(rng.string(1, b"abcdefghijklmnopqrstuvwxyz")),
                    'A' => Some(rng.string(1, b"ABCDEFGHIJKLMNOPQRSTUVWXYZ")),
                    'x' => Some(rng.hex(1)),
                    _ => None,
                };
                if let Some(text) = token {
                    out.push_str(&text);
                    i += 3;
                    continue;
                }
            }
        } else if c == '}' && i + 1 < chars.len() && chars[i + 1] == '}' {
            out.push('}');
            i += 2;
            continue;
        }
        out.push(c);
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn column(name: &str, data_type: DataTypeKind) -> ColumnDef {
        ColumnDef {
            name: name.to_string(),
            ordinal: 1,
            native_type: String::new(),
            data_type,
            nullable: false,
            default_value: None,
            is_primary_key: false,
            is_auto_increment: false,
            is_generated: false,
            comment: None,
            char_max_length: None,
            numeric_precision: None,
            numeric_scale: None,
            collation: None,
        }
    }

    #[test]
    fn auto_strategy_prefers_semantic_names() {
        assert!(matches!(
            resolve_auto(&column("email", DataTypeKind::Text)),
            GenStrategy::Email
        ));
        assert!(matches!(
            resolve_auto(&column("Phone_Number", DataTypeKind::Text)),
            GenStrategy::Phone
        ));
        assert!(matches!(
            resolve_auto(&column("contact_tel", DataTypeKind::Text)),
            GenStrategy::Phone
        ));
        assert!(matches!(
            resolve_auto(&column("full_name", DataTypeKind::Text)),
            GenStrategy::FullName
        ));
        assert!(matches!(
            resolve_auto(&column("company_id", DataTypeKind::Int)),
            GenStrategy::Company
        ));
        assert!(matches!(
            resolve_auto(&column("city", DataTypeKind::Text)),
            GenStrategy::City
        ));
        assert!(matches!(
            resolve_auto(&column("country_code", DataTypeKind::Text)),
            GenStrategy::Country
        ));
        assert!(matches!(
            resolve_auto(&column("shipping_address", DataTypeKind::Text)),
            GenStrategy::Address
        ));
        // `tel` must not match inside another word.
        assert!(matches!(
            resolve_auto(&column("hotel_name", DataTypeKind::Text)),
            GenStrategy::FullName
        ));
    }

    #[test]
    fn auto_strategy_falls_back_to_types() {
        assert!(matches!(
            resolve_auto(&column("created_at", DataTypeKind::DateTime)),
            GenStrategy::DateTimeRecent { days: 90 }
        ));
        assert!(matches!(
            resolve_auto(&column("birth_date", DataTypeKind::Date)),
            GenStrategy::DateRecent { days: 90 }
        ));
        assert!(matches!(
            resolve_auto(&column("is_active", DataTypeKind::Bool)),
            GenStrategy::Bool
        ));
        assert!(matches!(
            resolve_auto(&column("uid", DataTypeKind::Uuid)),
            GenStrategy::Uuid
        ));
        assert!(matches!(
            resolve_auto(&column("age", DataTypeKind::Int)),
            GenStrategy::RandomInt { min: 1, max: 100_000 }
        ));
        assert!(matches!(
            resolve_auto(&column("score", DataTypeKind::Float)),
            GenStrategy::RandomDecimal { scale: 2, .. }
        ));

        let mut price = column("price", DataTypeKind::Decimal);
        price.numeric_scale = Some(4);
        assert!(matches!(
            resolve_auto(&price),
            GenStrategy::RandomDecimal { scale: 4, .. }
        ));

        let mut short = column("code", DataTypeKind::Text);
        short.char_max_length = Some(10);
        assert!(matches!(
            resolve_auto(&short),
            GenStrategy::RandomString {
                min_len: 5,
                max_len: 10
            }
        ));
    }

    #[test]
    fn pattern_tokens_render_expected_classes() {
        let mut rng = Prng::new(Some(7));
        let out = render_pattern(&mut rng, "AB{{x}}-{n}{N}{a}{A}{x}{{}}");
        assert!(out.starts_with("AB{x}-"), "unexpected output: {out}");
        assert!(out.ends_with('}'));
        let token: Vec<char> = out["AB{x}-".len().."AB{x}-".len() + 5].chars().collect();
        assert!(token[0].is_ascii_digit());
        assert!(('1'..='9').contains(&token[1]), "out={out:?}");
        assert!(token[2].is_ascii_lowercase());
        assert!(token[3].is_ascii_uppercase());
        assert!(token[4].is_ascii_hexdigit());
        assert!(out.ends_with("{}"));
        assert_eq!(out.len(), "AB{x}-".len() + 5 + 2);
    }

    #[test]
    fn pattern_leaves_unknown_tokens_verbatim() {
        let mut rng = Prng::new(Some(1));
        assert_eq!(render_pattern(&mut rng, "{z}"), "{z}");
        assert_eq!(render_pattern(&mut rng, "a{{b}}c"), "a{b}c");
        assert_eq!(render_pattern(&mut rng, "no tokens"), "no tokens");
    }

    #[test]
    fn generation_is_deterministic() {
        let column = column("email", DataTypeKind::Text);
        let render = |seed: u64| {
            let mut rng = Prng::new(Some(seed));
            (0..20)
                .map(|i| {
                    generate_cell(&mut rng, &GenStrategy::Auto, &column, i).render(Engine::MySql)
                })
                .collect::<Vec<_>>()
                .join(",")
        };
        assert_eq!(render(42), render(42));
        assert_ne!(render(42), render(43));
    }

    #[test]
    fn sequential_and_expression_cells_render() {
        let mut rng = Prng::new(Some(1));
        let sequential = GenStrategy::Sequential { start: 10, step: 2 };
        let first = generate_value(&mut rng, &sequential, 0);
        let third = generate_value(&mut rng, &sequential, 2);
        assert_eq!(first.render(Engine::Sqlite), "10");
        assert_eq!(third.render(Engine::Sqlite), "14");

        let expression = GenStrategy::Expression {
            sql: "NOW()".to_string(),
        };
        assert_eq!(
            generate_value(&mut rng, &expression, 0).render(Engine::Postgres),
            "NOW()"
        );
    }

    #[test]
    fn insert_rendering_quotes_identifiers_and_literals() {
        let rows = vec![vec![
            Cell::Value(Value::Text("O'Brien".to_string())),
            Cell::Value(Value::Null),
            Cell::Value(Value::Int(7)),
        ]];
        let sql = render_insert(
            Engine::Postgres,
            "\"public\".\"users\"",
            &["name".to_string(), "note".to_string(), "age".to_string()],
            &rows,
        );
        assert_eq!(
            sql,
            "INSERT INTO \"public\".\"users\" (\"name\", \"note\", \"age\") VALUES ('O''Brien', NULL, 7);"
        );

        let sql = render_insert(
            Engine::MySql,
            "`shop`.`users`",
            &["name".to_string()],
            &[vec![Cell::Value(Value::Text("x".to_string()))]],
        );
        assert_eq!(sql, "INSERT INTO `shop`.`users` (`name`) VALUES ('x');");
    }

    #[test]
    fn qualified_table_follows_engine_preference() {
        let scope = Scope {
            database: Some("db".to_string()),
            schema: Some("s".to_string()),
        };
        assert_eq!(qualified_table(Engine::Postgres, &scope, "t"), "\"s\".\"t\"");
        assert_eq!(qualified_table(Engine::MySql, &scope, "t"), "`db`.`t`");
        assert_eq!(qualified_table(Engine::Sqlite, &Scope::default(), "t"), "\"t\"");
    }
}
