//! Data Transfer: copy a table (or a filtered subset) from one connection to
//! another, cross-engine, streaming row batches.
//!
//! Part of the v1 feature pack — frozen contract.
//!
//! Design notes:
//! - The source `SELECT` streams through `start_query`; rows are inserted batch
//!   by batch, so the whole table never has to fit in memory.
//! - Source and target must be **different sessions**: one session serializes
//!   statements behind a driver mutex, so streaming from a session while
//!   inserting into the same one would deadlock. Opening a second connection
//!   (even to the same database) is the supported workflow.
//! - Cross-engine target creation is best-effort: column types are mapped from
//!   `DataTypeKind` and any lossy decision is flagged in `warnings`.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use mieosql_core::{
    ColumnDef, DataTypeKind, Engine, ErrorCode, MieoError, ObjectKind, QueryEvent, QueryMode,
    QueryOptions, Scope, Value,
};

use crate::dto::{ApiError, ApiResult};
use crate::export::cell_text;
use crate::service::{quote_ident, select_limited, Api};
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

/// Default rows per INSERT batch.
const DEFAULT_BATCH_SIZE: u32 = 250;
/// Hard cap on rows per INSERT batch.
const MAX_BATCH_SIZE: u32 = 1_000;

impl Api {
    /// Stream rows from `request.source` into `request.target`.
    pub async fn transfer_table(&self, request: TransferRequest) -> ApiResult<TransferResult> {
        let started = std::time::Instant::now();

        let source_table = request.source_table.trim().to_string();
        if source_table.is_empty() {
            return Err(MieoError::new(ErrorCode::Data, "source_table is required")
                .with_hint("pick a table to transfer")
                .into());
        }
        let target_table = request
            .target_table
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .unwrap_or(source_table.as_str())
            .to_string();
        let batch_size = request
            .batch_size
            .unwrap_or(DEFAULT_BATCH_SIZE)
            .clamp(1, MAX_BATCH_SIZE) as usize;

        if request.source.session_id == request.target.session_id {
            return Err(MieoError::unsupported(
                "source and target sessions must differ",
            )
            .with_hint(
                "a single session serializes statements; open a second connection (even to the same database)",
            )
            .into());
        }

        let source_info = self.session_info(request.source.session_id.clone()).await?;
        let target_info = self.session_info(request.target.session_id.clone()).await?;
        let source_engine = source_info.engine;
        let target_engine = target_info.engine;

        // --- source columns ------------------------------------------------
        let mut all_columns = self
            .table_columns(
                request.source.session_id.clone(),
                request.source.scope.clone(),
                source_table.clone(),
            )
            .await?;
        if all_columns.is_empty() {
            return Err(MieoError::new(
                ErrorCode::NotFound,
                format!("source table `{source_table}` not found"),
            )
            .with_hint("check the table name and its database/schema scope")
            .into());
        }
        all_columns.sort_by_key(|c| c.ordinal);

        let mut warnings: Vec<String> = Vec::new();
        let selected = select_columns(&all_columns, &request.columns, &mut warnings)?;
        if selected.is_empty() {
            return Err(MieoError::new(
                ErrorCode::Data,
                format!("no transferable columns on `{source_table}`"),
            )
            .with_hint("generated columns are computed by the database and cannot be copied")
            .into());
        }
        let column_names: Vec<String> = selected.iter().map(|c| c.name.clone()).collect();

        // --- target preparation --------------------------------------------
        let target_objects = self
            .list_objects(
                request.target.session_id.clone(),
                request.target.scope.clone(),
                Some(ObjectKind::Table),
                None,
            )
            .await?;
        let target_exists = target_objects
            .iter()
            .any(|o| o.name.eq_ignore_ascii_case(&target_table));

        let mut created = false;
        match request.mode {
            TransferMode::CreateAppend => {
                if !target_exists {
                    self.create_transfer_target(
                        &request.target,
                        target_engine,
                        &target_table,
                        source_engine,
                        &request.source,
                        &source_table,
                        &selected,
                        &mut warnings,
                    )
                    .await?;
                    created = true;
                }
            }
            TransferMode::Append | TransferMode::TruncateAppend => {
                if !target_exists {
                    return Err(target_missing(&target_table, request.mode));
                }
                if request.mode == TransferMode::TruncateAppend {
                    let qualified =
                        qualified_table(target_engine, &request.target.scope, &target_table);
                    self.execute(&request.target.session_id, format!("DELETE FROM {qualified}"))
                        .await?;
                }
            }
            TransferMode::DropCreate => {
                let qualified = qualified_table(target_engine, &request.target.scope, &target_table);
                self.execute(
                    &request.target.session_id,
                    format!("DROP TABLE IF EXISTS {qualified}"),
                )
                .await?;
                self.create_transfer_target(
                    &request.target,
                    target_engine,
                    &target_table,
                    source_engine,
                    &request.source,
                    &source_table,
                    &selected,
                    &mut warnings,
                )
                .await?;
                created = true;
            }
        }

        // --- stream source rows and insert in batches ----------------------
        let where_clause = request
            .where_clause
            .as_deref()
            .map(str::trim)
            .map(|w| w.trim_end_matches(';').trim())
            .filter(|w| !w.is_empty());
        let select_sql = build_transfer_select(
            source_engine,
            &request.source.scope,
            &source_table,
            &column_names,
            where_clause,
            request.limit,
        );
        let mut stream = self
            .start_query(
                request.source.session_id.clone(),
                select_sql,
                Vec::new(),
                Some(QueryOptions {
                    mode: QueryMode::Prepared,
                    fetch_size: batch_size as u32,
                    // The SELECT already fetches `limit + 1` rows; this is a
                    // belt-and-braces bound for drivers that ignore LIMIT.
                    max_rows: request.limit.map(|n| n.saturating_add(1)),
                    timeout_ms: None,
                }),
            )
            .await?;

        let qualified_target =
            qualified_table(target_engine, &request.target.scope, &target_table);
        let mut rows_transferred: u64 = 0;
        let mut truncated = false;
        let mut conversions = ConvertStats::default();

        while let Some(event) = stream.events.recv().await {
            match event {
                QueryEvent::Batch { rows, .. } => {
                    let rows = clamp_to_limit(rows, request.limit, rows_transferred, &mut truncated);
                    if rows.is_empty() {
                        continue;
                    }
                    let converted: Vec<Vec<Value>> = rows
                        .into_iter()
                        .map(|row| convert_row(target_engine, row, &mut conversions))
                        .collect();
                    for chunk in chunk_rows(converted, batch_size) {
                        let count = chunk.len() as u64;
                        let sql = build_insert_sql(
                            target_engine,
                            &qualified_target,
                            &column_names,
                            chunk.len(),
                        );
                        let params: Vec<Value> = chunk.into_iter().flatten().collect();
                        self.collect_query(
                            &request.target.session_id,
                            sql,
                            params,
                            QueryMode::Prepared,
                            None,
                        )
                        .await?;
                        rows_transferred += count;
                    }
                }
                QueryEvent::Error { error, .. } => {
                    return Err(ApiError::from(error));
                }
                QueryEvent::Finished { stats, .. } => {
                    truncated |= stats.truncated;
                    break;
                }
                _ => {}
            }
        }

        if conversions.unsupported > 0 {
            warnings.push(format!(
                "{} unsupported value(s) were written as NULL",
                conversions.unsupported
            ));
        }
        if conversions.coerced > 0 {
            warnings.push(format!(
                "{} array/json value(s) were converted to text for {}",
                conversions.coerced,
                target_engine.label()
            ));
        }

        Ok(TransferResult {
            rows: rows_transferred,
            elapsed_ms: started.elapsed().as_millis() as u64,
            target_table,
            created,
            truncated,
            warnings,
        })
    }

    /// Create the target table: same-engine reuses the source DDL (renamed when
    /// needed); cross-engine or missing DDL generates a best-effort CREATE.
    #[allow(clippy::too_many_arguments)]
    async fn create_transfer_target(
        &self,
        target: &SyncEndpoint,
        target_engine: Engine,
        target_table: &str,
        source_engine: Engine,
        source: &SyncEndpoint,
        source_table: &str,
        columns: &[ColumnDef],
        warnings: &mut Vec<String>,
    ) -> ApiResult<()> {
        let ddl = if target_engine == source_engine {
            let raw = self
                .object_ddl(
                    source.session_id.clone(),
                    source.scope.clone(),
                    ObjectKind::Table,
                    source_table.to_string(),
                )
                .await?;
            match raw {
                Some(raw) => match retarget_ddl(target_engine, &raw, source_table, target_table) {
                    Some(ddl) => Some(ddl),
                    None => {
                        warnings.push(
                            "could not rewrite the source DDL for the new table name; generated CREATE TABLE from column metadata"
                                .to_string(),
                        );
                        None
                    }
                },
                None => {
                    warnings.push(
                        "source DDL was unavailable; generated CREATE TABLE from column metadata"
                            .to_string(),
                    );
                    None
                }
            }
        } else {
            warnings.push(format!(
                "cross-engine create ({} -> {}): generated DDL is best-effort",
                source_engine.label(),
                target_engine.label()
            ));
            None
        };

        let ddl = ddl.unwrap_or_else(|| {
            generate_create_table(target_engine, &target.scope, target_table, columns)
        });
        self.execute(&target.session_id, ddl).await?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Column selection
// ---------------------------------------------------------------------------

/// Resolve the columns to copy. Empty `requested` = every column except
/// generated ones (their values are computed by the source database and must
/// not be written).
fn select_columns(
    all: &[ColumnDef],
    requested: &[String],
    warnings: &mut Vec<String>,
) -> ApiResult<Vec<ColumnDef>> {
    if requested.is_empty() {
        let generated = all.iter().filter(|c| c.is_generated).count();
        if generated > 0 {
            warnings.push(format!(
                "skipped {generated} generated column(s): their values are computed by the database"
            ));
        }
        return Ok(all.iter().filter(|c| !c.is_generated).cloned().collect());
    }

    let mut out = Vec::with_capacity(requested.len());
    for wanted in requested {
        let found = all
            .iter()
            .find(|c| c.name.eq_ignore_ascii_case(wanted.trim()))
            .ok_or_else(|| {
                MieoError::new(
                    ErrorCode::NotFound,
                    format!("column `{wanted}` not found on the source table"),
                )
                .with_hint("check the column list against the source table")
            })?;
        if found.is_generated {
            warnings.push(format!(
                "skipped generated column `{}`: its value is computed by the database",
                found.name
            ));
            continue;
        }
        out.push(found.clone());
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// SQL builders
// ---------------------------------------------------------------------------

/// Quote a table and qualify it for the engine's scope rules (Postgres:
/// schema, others: database) — same policy as `build_select_sql`.
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

/// `SELECT {cols} FROM {qualified} [WHERE ..] [LIMIT ..]`.
///
/// When `limit` is set the query fetches one extra row so the caller can report
/// `truncated` exactly: `used = min(rows, limit)`, `truncated = rows > limit`.
fn build_transfer_select(
    engine: Engine,
    scope: &Scope,
    table: &str,
    columns: &[String],
    where_clause: Option<&str>,
    limit: Option<u64>,
) -> String {
    let cols = columns
        .iter()
        .map(|c| quote_ident(engine, c))
        .collect::<Vec<_>>()
        .join(", ");
    let mut from_where = format!("FROM {}", qualified_table(engine, scope, table));
    if let Some(where_clause) = where_clause.map(str::trim).filter(|w| !w.is_empty()) {
        from_where.push_str(" WHERE ");
        from_where.push_str(where_clause.trim_end_matches(';').trim_end());
    }
    match limit {
        // One extra row so the caller can report `truncated` exactly.
        Some(limit) => select_limited(engine, &cols, &from_where, limit.saturating_add(1)),
        None => format!("SELECT {cols} {from_where}"),
    }
}

/// Parameter placeholder for the target engine (`?` / `$n`), 1-based index.
fn placeholder(engine: Engine, index_1based: usize) -> String {
    match engine {
        Engine::Postgres => format!("${index_1based}"),
        _ => "?".to_string(),
    }
}

/// Prepared multi-row INSERT with engine-specific placeholders.
fn build_insert_sql(
    engine: Engine,
    qualified: &str,
    columns: &[String],
    row_count: usize,
) -> String {
    let cols = columns
        .iter()
        .map(|c| quote_ident(engine, c))
        .collect::<Vec<_>>()
        .join(", ");
    let mut tuples = Vec::with_capacity(row_count);
    let mut index = 0usize;
    for _ in 0..row_count {
        let mut tuple = Vec::with_capacity(columns.len());
        for _ in 0..columns.len() {
            index += 1;
            tuple.push(placeholder(engine, index));
        }
        tuples.push(format!("({})", tuple.join(", ")));
    }
    format!("INSERT INTO {qualified} ({cols}) VALUES {}", tuples.join(", "))
}

/// Split `rows` into consecutive chunks of at most `size` (never empty ones).
fn chunk_rows<T>(rows: Vec<T>, size: usize) -> Vec<Vec<T>> {
    let size = size.max(1);
    let mut out = Vec::new();
    let mut it = rows.into_iter();
    loop {
        let chunk: Vec<T> = it.by_ref().take(size).collect();
        if chunk.is_empty() {
            break;
        }
        out.push(chunk);
    }
    out
}

/// Keep at most `limit - already` rows, flagging truncation when more arrived.
fn clamp_to_limit(
    mut rows: Vec<Vec<Value>>,
    limit: Option<u64>,
    already: u64,
    truncated: &mut bool,
) -> Vec<Vec<Value>> {
    let Some(limit) = limit else { return rows };
    let remaining = limit.saturating_sub(already);
    if rows.len() as u64 > remaining {
        rows.truncate(remaining as usize);
        *truncated = true;
    }
    rows
}

// ---------------------------------------------------------------------------
// Value conversion
// ---------------------------------------------------------------------------

/// Counters for values adapted on the fly.
#[derive(Default)]
struct ConvertStats {
    unsupported: u64,
    coerced: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ValueConversion {
    Kept,
    Coerced,
    Unsupported,
}

/// Adapt one source value for the target engine.
///
/// - `Unsupported` decodes nowhere: transfer as `NULL` (and warn).
/// - Arrays / JSON are not first-class on MySQL/SQLite: render as text there;
///   Postgres keeps them untouched.
fn convert_value(target: Engine, value: Value) -> (Value, ValueConversion) {
    if matches!(value, Value::Unsupported { .. }) {
        return (Value::Null, ValueConversion::Unsupported);
    }
    if target != Engine::Postgres && matches!(value, Value::Array(_) | Value::Json(_)) {
        return (Value::Text(cell_text(&value)), ValueConversion::Coerced);
    }
    (value, ValueConversion::Kept)
}

fn convert_row(target: Engine, row: Vec<Value>, stats: &mut ConvertStats) -> Vec<Value> {
    row.into_iter()
        .map(|value| {
            let (value, kind) = convert_value(target, value);
            match kind {
                ValueConversion::Kept => {}
                ValueConversion::Coerced => stats.coerced += 1,
                ValueConversion::Unsupported => stats.unsupported += 1,
            }
            value
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Cross-engine DDL
// ---------------------------------------------------------------------------

/// Rewrite the table name inside a same-engine `CREATE TABLE` DDL so the copy
/// can use a different name. Returns `None` when the name cannot be located.
fn retarget_ddl(engine: Engine, ddl: &str, from: &str, to: &str) -> Option<String> {
    if from == to {
        return Some(ddl.to_string());
    }
    let from_quoted = quote_ident(engine, from);
    if ddl.contains(&from_quoted) {
        return Some(ddl.replace(&from_quoted, &quote_ident(engine, to)));
    }
    // Some engines store the statement as typed (usually unquoted): fall back to
    // a whole-word, case-insensitive replacement.
    let pattern = format!(r"(?i)\b{}\b", regex::escape(from));
    let re = regex::Regex::new(&pattern).ok()?;
    if re.is_match(ddl) {
        let to_quoted = quote_ident(engine, to);
        return Some(re.replace_all(ddl, to_quoted.as_str()).into_owned());
    }
    None
}

/// Best-effort `CREATE TABLE` for a target engine, generated from source column
/// metadata. Defaults are dropped, generated columns become plain columns.
fn generate_create_table(engine: Engine, scope: &Scope, table: &str, columns: &[ColumnDef]) -> String {
    let qualified = qualified_table(engine, scope, table);
    let pk: Vec<String> = columns
        .iter()
        .filter(|c| c.is_primary_key)
        .map(|c| quote_ident(engine, &c.name))
        .collect();
    let sqlite_inline_pk = engine == Engine::Sqlite
        && pk.len() == 1
        && columns.iter().any(|c| {
            c.is_primary_key
                && c.is_auto_increment
                && matches!(c.data_type, DataTypeKind::Int | DataTypeKind::UInt)
        });

    let mut defs: Vec<String> = Vec::with_capacity(columns.len() + 1);
    for col in columns {
        let mut parts = vec![quote_ident(engine, &col.name), target_type_for(engine, col)];
        let inline_pk = sqlite_inline_pk && col.is_primary_key;
        if inline_pk {
            parts.push("PRIMARY KEY AUTOINCREMENT".to_string());
        } else {
            if !col.nullable && !col.is_generated && !col.is_auto_increment {
                parts.push("NOT NULL".to_string());
            }
            if col.is_auto_increment {
                match engine {
                    Engine::MySql | Engine::MariaDb => parts.push("AUTO_INCREMENT".to_string()),
                    Engine::Postgres if col.data_type == DataTypeKind::Int => {
                        parts.push("GENERATED BY DEFAULT AS IDENTITY".to_string())
                    }
                    _ => {}
                }
            }
        }
        defs.push(parts.join(" "));
    }
    if !pk.is_empty() && !sqlite_inline_pk {
        defs.push(format!("PRIMARY KEY ({})", pk.join(", ")));
    }
    let mut sql = format!("CREATE TABLE {qualified} (\n  {}\n)", defs.join(",\n  "));
    if engine == Engine::ClickHouse {
        // MergeTree needs an explicit sort key; use the primary key when there
        // is one, otherwise an empty tuple.
        if pk.is_empty() {
            sql.push_str(" ENGINE = MergeTree ORDER BY tuple()");
        } else {
            sql.push_str(&format!(" ENGINE = MergeTree ORDER BY ({})", pk.join(", ")));
        }
    }
    sql
}

/// Map a source column to the target engine's native type.
fn target_type_for(engine: Engine, col: &ColumnDef) -> String {
    match engine {
        Engine::Postgres => pg_type(&col.data_type, col),
        Engine::MySql | Engine::MariaDb => match &col.data_type {
            DataTypeKind::Bool => "TINYINT(1)".to_string(),
            DataTypeKind::Int => "BIGINT".to_string(),
            DataTypeKind::UInt => "BIGINT UNSIGNED".to_string(),
            DataTypeKind::Float => "DOUBLE".to_string(),
            DataTypeKind::Decimal => match (col.numeric_precision, col.numeric_scale) {
                (Some(p), Some(s)) => format!("DECIMAL({p},{s})"),
                (Some(p), None) => format!("DECIMAL({p})"),
                _ => "DECIMAL(38,10)".to_string(),
            },
            DataTypeKind::Text => "LONGTEXT".to_string(),
            DataTypeKind::Bytes => "LONGBLOB".to_string(),
            DataTypeKind::Date => "DATE".to_string(),
            DataTypeKind::Time => "TIME".to_string(),
            DataTypeKind::DateTime | DataTypeKind::TimestampTz => "DATETIME(6)".to_string(),
            DataTypeKind::Json => "JSON".to_string(),
            DataTypeKind::Uuid => "CHAR(36)".to_string(),
            DataTypeKind::Bit => "BIT(1)".to_string(),
            DataTypeKind::Interval => "VARCHAR(64)".to_string(),
            DataTypeKind::Enum | DataTypeKind::Set => "TEXT".to_string(),
            DataTypeKind::Geometry => "TEXT".to_string(),
            // Arrays travel as rendered text for MySQL/MariaDB targets.
            DataTypeKind::Array(_) => "LONGTEXT".to_string(),
            DataTypeKind::Unknown => "TEXT".to_string(),
        },
        Engine::Sqlite => match &col.data_type {
            DataTypeKind::Bool | DataTypeKind::Int | DataTypeKind::UInt => "INTEGER".to_string(),
            DataTypeKind::Float | DataTypeKind::Decimal => "REAL".to_string(),
            DataTypeKind::Bytes => "BLOB".to_string(),
            _ => "TEXT".to_string(),
        },
        Engine::SqlServer => match &col.data_type {
            DataTypeKind::Bool => "BIT".to_string(),
            DataTypeKind::Int => "BIGINT".to_string(),
            // SQL Server has no unsigned integers; DECIMAL(20,0) holds any u64.
            DataTypeKind::UInt => "DECIMAL(20,0)".to_string(),
            DataTypeKind::Float => "FLOAT".to_string(),
            DataTypeKind::Decimal => match (col.numeric_precision, col.numeric_scale) {
                (Some(p), Some(s)) => format!("DECIMAL({p},{s})"),
                (Some(p), None) => format!("DECIMAL({p})"),
                _ => "DECIMAL(38,10)".to_string(),
            },
            DataTypeKind::Text => "NVARCHAR(MAX)".to_string(),
            DataTypeKind::Bytes => "VARBINARY(MAX)".to_string(),
            DataTypeKind::Date => "DATE".to_string(),
            DataTypeKind::Time => "TIME".to_string(),
            DataTypeKind::DateTime => "DATETIME2".to_string(),
            DataTypeKind::TimestampTz => "DATETIMEOFFSET".to_string(),
            DataTypeKind::Json => "NVARCHAR(MAX)".to_string(),
            DataTypeKind::Uuid => "UNIQUEIDENTIFIER".to_string(),
            DataTypeKind::Bit => "VARBINARY(MAX)".to_string(),
            DataTypeKind::Interval => "NVARCHAR(64)".to_string(),
            DataTypeKind::Enum | DataTypeKind::Set => "NVARCHAR(MAX)".to_string(),
            DataTypeKind::Geometry => "VARBINARY(MAX)".to_string(),
            DataTypeKind::Array(_) => "NVARCHAR(MAX)".to_string(),
            DataTypeKind::Unknown => "NVARCHAR(MAX)".to_string(),
        },
        Engine::ClickHouse => {
            let base = match &col.data_type {
                DataTypeKind::Bool | DataTypeKind::Bit => "UInt8".to_string(),
                DataTypeKind::Int => "Int64".to_string(),
                DataTypeKind::UInt => "UInt64".to_string(),
                DataTypeKind::Float => "Float64".to_string(),
                DataTypeKind::Decimal => match (col.numeric_precision, col.numeric_scale) {
                    (Some(p), Some(s)) => format!("Decimal({p},{s})"),
                    (Some(p), None) => format!("Decimal({p})"),
                    _ => "Decimal(38,10)".to_string(),
                },
                DataTypeKind::Text
                | DataTypeKind::Time
                | DataTypeKind::Json
                | DataTypeKind::Interval
                | DataTypeKind::Enum
                | DataTypeKind::Set
                | DataTypeKind::Geometry
                | DataTypeKind::Unknown => "String".to_string(),
                DataTypeKind::Bytes => "String".to_string(),
                DataTypeKind::Date => "Date".to_string(),
                DataTypeKind::DateTime | DataTypeKind::TimestampTz => {
                    "DateTime64(3)".to_string()
                }
                DataTypeKind::Uuid => "UUID".to_string(),
                DataTypeKind::Array(_) => "Array(String)".to_string(),
            };
            // ClickHouse types are non-nullable by default; Array cannot be
            // wrapped in Nullable.
            if col.nullable && !base.starts_with("Array(") {
                format!("Nullable({base})")
            } else {
                base
            }
        }
        _ => "TEXT".to_string(),
    }
}

fn pg_type(kind: &DataTypeKind, col: &ColumnDef) -> String {
    match kind {
        DataTypeKind::Bool => "BOOLEAN".to_string(),
        DataTypeKind::Int => "BIGINT".to_string(),
        DataTypeKind::UInt => "NUMERIC(20)".to_string(),
        DataTypeKind::Float => "DOUBLE PRECISION".to_string(),
        DataTypeKind::Decimal => match (col.numeric_precision, col.numeric_scale) {
            (Some(p), Some(s)) => format!("NUMERIC({p},{s})"),
            (Some(p), None) => format!("NUMERIC({p})"),
            _ => "NUMERIC".to_string(),
        },
        DataTypeKind::Text => "TEXT".to_string(),
        DataTypeKind::Bytes => "BYTEA".to_string(),
        DataTypeKind::Date => "DATE".to_string(),
        DataTypeKind::Time => "TIME".to_string(),
        DataTypeKind::DateTime => "TIMESTAMP".to_string(),
        DataTypeKind::TimestampTz => "TIMESTAMPTZ".to_string(),
        DataTypeKind::Json => "JSONB".to_string(),
        DataTypeKind::Uuid => "UUID".to_string(),
        DataTypeKind::Bit => "BYTEA".to_string(),
        DataTypeKind::Interval => "INTERVAL".to_string(),
        DataTypeKind::Enum | DataTypeKind::Set => "TEXT".to_string(),
        DataTypeKind::Geometry => "TEXT".to_string(),
        DataTypeKind::Array(inner) => format!("{}[]", pg_type(inner, col)),
        DataTypeKind::Unknown => "TEXT".to_string(),
    }
}

fn target_missing(target_table: &str, mode: TransferMode) -> ApiError {
    let hint = match mode {
        TransferMode::Append | TransferMode::TruncateAppend => {
            "use the `create_append` mode to create it automatically"
        }
        _ => "check the target table name and its database/schema scope",
    };
    MieoError::new(
        ErrorCode::NotFound,
        format!("target table `{target_table}` not found"),
    )
    .with_hint(hint)
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn col(name: &str, data_type: DataTypeKind, nullable: bool, pk: bool) -> ColumnDef {
        ColumnDef {
            name: name.into(),
            ordinal: 0,
            native_type: String::new(),
            data_type,
            nullable,
            default_value: None,
            is_primary_key: pk,
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
    fn placeholders_follow_engine() {
        assert_eq!(placeholder(Engine::MySql, 1), "?");
        assert_eq!(placeholder(Engine::Sqlite, 3), "?");
        assert_eq!(placeholder(Engine::Postgres, 1), "$1");
        assert_eq!(placeholder(Engine::Postgres, 4), "$4");
    }

    #[test]
    fn insert_sql_is_built_per_engine() {
        let cols = vec!["id".to_string(), "name".to_string()];
        assert_eq!(
            build_insert_sql(Engine::Sqlite, "\"items\"", &cols, 2),
            "INSERT INTO \"items\" (\"id\", \"name\") VALUES (?, ?), (?, ?)"
        );
        assert_eq!(
            build_insert_sql(Engine::Postgres, "\"public\".\"items\"", &cols, 2),
            "INSERT INTO \"public\".\"items\" (\"id\", \"name\") VALUES ($1, $2), ($3, $4)"
        );
    }

    #[test]
    fn chunk_rows_splits_and_ignores_empty_input() {
        let rows: Vec<i32> = (1..=5).collect();
        let chunks = chunk_rows(rows, 2);
        assert_eq!(chunks, vec![vec![1, 2], vec![3, 4], vec![5]]);
        assert!(chunk_rows(Vec::<i32>::new(), 2).is_empty());
        assert_eq!(chunk_rows(vec![7], 0), vec![vec![7]]);
    }

    #[test]
    fn values_are_converted_for_the_target_engine() {
        let array = Value::Array(vec![Value::Int(1), Value::Int(2)]);
        let (value, kind) = convert_value(Engine::MySql, array.clone());
        assert!(matches!(value, Value::Text(_)));
        assert_eq!(kind, ValueConversion::Coerced);

        let (value, kind) = convert_value(Engine::Sqlite, Value::Json(serde_json::json!({"a": 1})));
        assert_eq!(value, Value::Text("{\"a\":1}".into()));
        assert_eq!(kind, ValueConversion::Coerced);

        let (value, kind) = convert_value(Engine::Postgres, array);
        assert!(matches!(value, Value::Array(_)));
        assert_eq!(kind, ValueConversion::Kept);

        let (value, kind) = convert_value(
            Engine::Sqlite,
            Value::Unsupported {
                native_type: "geometry".into(),
                reason: "binary".into(),
            },
        );
        assert_eq!(value, Value::Null);
        assert_eq!(kind, ValueConversion::Unsupported);

        let (value, kind) = convert_value(Engine::Postgres, Value::Text("x".into()));
        assert_eq!(value, Value::Text("x".into()));
        assert_eq!(kind, ValueConversion::Kept);
    }

    #[test]
    fn select_sql_quotes_and_fetches_one_extra_row() {
        let cols = vec!["id".to_string(), "we\"ird".to_string()];
        let scope = Scope {
            database: Some("shop".into()),
            schema: None,
        };
        let sql = build_transfer_select(
            Engine::MySql,
            &scope,
            "orders",
            &cols,
            Some("id > 10"),
            Some(100),
        );
        assert_eq!(
            sql,
            "SELECT `id`, `we\"ird` FROM `shop`.`orders` WHERE id > 10 LIMIT 101"
        );

        let sql = build_transfer_select(
            Engine::Postgres,
            &Scope::default(),
            "t",
            &["a".to_string()],
            None,
            None,
        );
        assert_eq!(sql, "SELECT \"a\" FROM \"t\"");
    }

    #[test]
    fn limit_clamp_reports_truncation() {
        let rows: Vec<Vec<Value>> = (0..4).map(|i| vec![Value::Int(i)]).collect();
        let mut truncated = false;
        let kept = clamp_to_limit(rows.clone(), Some(3), 0, &mut truncated);
        assert_eq!(kept.len(), 3);
        assert!(truncated);

        let mut truncated = false;
        let kept = clamp_to_limit(rows, Some(3), 3, &mut truncated);
        assert!(kept.is_empty());
        assert!(truncated);

        let mut truncated = false;
        let kept = clamp_to_limit(vec![vec![Value::Int(1)]], None, 0, &mut truncated);
        assert_eq!(kept.len(), 1);
        assert!(!truncated);
    }

    #[test]
    fn generated_columns_are_skipped() {
        let mut all = vec![
            col("id", DataTypeKind::Int, false, true),
            col("computed", DataTypeKind::Text, true, false),
        ];
        all[1].is_generated = true;

        let mut warnings = Vec::new();
        let selected = select_columns(&all, &[], &mut warnings).unwrap();
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].name, "id");
        assert_eq!(warnings.len(), 1);

        let selected = select_columns(&all, &["COMPUTED".to_string()], &mut warnings).unwrap();
        assert!(selected.is_empty());
        assert_eq!(warnings.len(), 2);

        let err = select_columns(&all, &["missing".to_string()], &mut Vec::new()).unwrap_err();
        assert_eq!(err.code, ErrorCode::NotFound);
    }

    #[test]
    fn cross_engine_create_is_best_effort() {
        let cols = vec![
            col("id", DataTypeKind::Int, false, true),
            col("name", DataTypeKind::Text, true, false),
        ];
        let ddl = generate_create_table(Engine::MySql, &Scope::db("shop"), "users", &cols);
        assert!(ddl.starts_with("CREATE TABLE `shop`.`users`"));
        assert!(ddl.contains("`id` BIGINT"));
        assert!(ddl.contains("PRIMARY KEY (`id`)"));
        assert!(ddl.contains("`name` LONGTEXT"));

        let ddl = generate_create_table(Engine::Postgres, &Scope::default(), "users", &cols);
        assert!(ddl.contains("\"id\" BIGINT"));
        assert!(ddl.contains("PRIMARY KEY (\"id\")"));
        assert!(ddl.contains("\"name\" TEXT"));
    }

    #[test]
    fn sqlite_auto_increment_pk_is_inlined() {
        let mut id = col("id", DataTypeKind::Int, false, true);
        id.is_auto_increment = true;
        let ddl = generate_create_table(Engine::Sqlite, &Scope::default(), "t", &[id]);
        assert!(ddl.contains("\"id\" INTEGER PRIMARY KEY AUTOINCREMENT"));
        assert!(!ddl.contains("PRIMARY KEY ("));
    }

    #[test]
    fn ddl_is_retargeted_when_renaming() {
        let quoted = "CREATE TABLE \"users\" (\"id\" bigint)";
        let out = retarget_ddl(Engine::Postgres, quoted, "users", "users_copy").unwrap();
        assert!(out.contains("\"users_copy\""));
        assert!(!out.contains("\"users\""));

        let unquoted = "CREATE TABLE users (id INTEGER)";
        let out = retarget_ddl(Engine::Sqlite, unquoted, "users", "copy").unwrap();
        assert!(out.contains("CREATE TABLE \"copy\""));
        // A different name leaves the DDL untouched.
        assert_eq!(
            retarget_ddl(Engine::Sqlite, unquoted, "users", "users").unwrap(),
            unquoted
        );
    }
}
