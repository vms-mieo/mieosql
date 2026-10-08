//! Table maintenance: batch execution, CSV/JSON file import, staged grid
//! edits, SQL script splitting and `.sql` backup dumps.
//!
//! These commands back the data-maintenance UI:
//! - [`Api::execute_batch`] runs a statement list, optionally atomic;
//! - [`Api::import_file`] loads a CSV/JSON file into a table;
//! - [`Api::apply_grid_edits`] persists staged grid inserts/updates/deletes;
//! - [`Api::script_statements`] splits a `.sql` file into statements;
//! - [`Api::dump_backup`] writes a scope (DDL + data) to a `.sql` file.
//!
//! Shared notes:
//! - Parameterized statements go through the same prepared path as the rest
//!   of the API (`collect_query` with `QueryMode::Prepared`); placeholders
//!   follow the session's [`PlaceholderStyle`] — Postgres gets `$n`, every
//!   other SQL engine gets `?` (the SQL Server driver rewrites `?` to
//!   `@P1..` internally).
//! - Identifiers are quoted per engine (`"…"`, backticks, `[…]`).
//! - `ImportSummary` is exported to TypeScript as `ImportFileSummary` so it
//!   cannot collide with `workspace::ImportSummary` (the workspace bundle
//!   import result).

use std::collections::HashSet;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::time::Instant;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use mieosql_core::{
    Engine, ErrorCode, MieoError, ObjectKind, PlaceholderStyle, QueryEvent, QueryMode, QueryOptions,
    Scope, Value,
};

use crate::dto::{ApiError, ApiResult};
use crate::export::sql_literal;
use crate::service::{quote_ident, Api};

/// Cap on echoed error strings in every summary DTO.
const MAX_ERRORS: usize = 20;
/// Rows per INSERT batch when the caller does not pick a size.
const DEFAULT_IMPORT_BATCH: u32 = 500;
/// Rows per multi-row INSERT chunk in a backup file.
const DUMP_INSERT_CHUNK: usize = 500;
/// Cap on JSON keys imported from one file.
const MAX_JSON_KEYS: usize = 256;
/// Default CSV field delimiter.
const DEFAULT_CSV_DELIMITER: u8 = b',';

// ---------------------------------------------------------------------------
// DTOs
// ---------------------------------------------------------------------------

/// Outcome of a batch run.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct BatchResult {
    /// Statements that executed successfully before the run stopped.
    #[ts(type = "number")]
    pub applied: u64,
    /// 1 when the run stopped on an error, 0 otherwise.
    #[ts(type = "number")]
    pub failed: u64,
    #[serde(default)]
    pub errors: Vec<String>,
    #[ts(type = "number")]
    pub elapsed_ms: u64,
}

/// File format accepted by [`Api::import_file`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum ImportFormat {
    Csv,
    Json,
}

/// What happens to the table before the rows are inserted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum ImportMode {
    /// Keep the current rows.
    #[default]
    Append,
    /// Empty the table first.
    Truncate,
}

/// Knobs for [`Api::import_file`].
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ImportOptions {
    /// CSV field delimiter; must be one ASCII character. Default `,`.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub delimiter: Option<String>,
    /// First CSV row holds the column names. When false the columns are
    /// synthesized as `c1..cN` (the header names are never trusted blindly).
    #[serde(default)]
    pub has_header: bool,
    /// Field text that becomes SQL NULL. When this is the empty string,
    /// **every empty field becomes NULL** (empty strings cannot be imported).
    #[serde(default)]
    pub null_marker: String,
    #[serde(default)]
    pub mode: ImportMode,
    /// Rows per INSERT batch (default 500, auto-capped to keep the parameter
    /// count per statement within the engine's limit).
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub batch_size: Option<u32>,
}

/// Outcome of a file import.
///
/// Exported to TS as `ImportFileSummary` (the plain `ImportSummary` name is
/// taken by the workspace bundle importer).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[ts(rename = "ImportFileSummary")]
pub struct ImportSummary {
    /// Data rows read from the file.
    #[ts(type = "number")]
    pub rows_read: u64,
    /// Rows committed to the table.
    #[ts(type = "number")]
    pub rows_inserted: u64,
    /// Rows in the batch that failed (the run stops at the first bad batch).
    #[ts(type = "number")]
    pub rows_failed: u64,
    #[serde(default)]
    pub errors: Vec<String>,
    #[ts(type = "number")]
    pub elapsed_ms: u64,
}

/// One column/value pair staged by the data grid.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct GridCell {
    pub column: String,
    pub value: Value,
}

/// A staged update: key columns identify the row, `changes` are the new values.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RowChanges {
    pub keys: Vec<GridCell>,
    #[serde(default)]
    pub changes: Vec<GridCell>,
}

/// Staged edits from one data grid.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct GridEditRequest {
    pub session_id: String,
    pub scope: Scope,
    pub table: String,
    #[serde(default)]
    pub inserts: Vec<Vec<GridCell>>,
    #[serde(default)]
    pub updates: Vec<RowChanges>,
    /// Key values of the rows to delete.
    #[serde(default)]
    pub deletes: Vec<Vec<GridCell>>,
}

/// Outcome of applying staged grid edits.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct EditApplyResult {
    #[ts(type = "number")]
    pub inserted: u64,
    #[ts(type = "number")]
    pub updated: u64,
    #[ts(type = "number")]
    pub deleted: u64,
    /// 1 when a statement failed and the transaction was rolled back.
    #[ts(type = "number")]
    pub failed: u64,
    #[serde(default)]
    pub errors: Vec<String>,
    #[ts(type = "number")]
    pub elapsed_ms: u64,
}

/// A `.sql` script split into individual statements.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ScriptStatements {
    #[serde(default)]
    pub statements: Vec<String>,
    /// Unterminated strings/comments and other tokenizer oddities.
    #[serde(default)]
    pub warnings: Vec<String>,
}

/// Outcome of a `.sql` backup dump.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct BackupSummary {
    /// Tables whose DDL made it into the file.
    #[ts(type = "number")]
    pub tables: u64,
    /// Data rows written as INSERT statements.
    #[ts(type = "number")]
    pub rows: u64,
    #[ts(type = "number")]
    pub bytes: u64,
    /// Skipped tables / problems; the dump continues when possible.
    #[serde(default)]
    pub errors: Vec<String>,
    #[ts(type = "number")]
    pub elapsed_ms: u64,
}

// ---------------------------------------------------------------------------
// Batch execution
// ---------------------------------------------------------------------------

impl Api {
    /// Run `statements` in order on the session's simple execution path
    /// (the one DDL such as `CREATE DATABASE` needs).
    ///
    /// `atomic = true` wraps the run in a transaction we own (when the session
    /// is not already inside one): the first error rolls the partial run back.
    /// `atomic = false` leaves every statement committed as it goes — use it
    /// for statements the engine forbids inside a transaction.
    ///
    /// Blank and comment-only statements are skipped; the run always stops at
    /// the first error and reports progress.
    pub async fn execute_batch(
        &self,
        session_id: String,
        statements: Vec<String>,
        atomic: bool,
    ) -> ApiResult<BatchResult> {
        let started = Instant::now();
        let todo: Vec<(usize, String)> = statements
            .iter()
            .enumerate()
            .filter_map(|(index, raw)| {
                let sql = raw.trim().trim_end_matches(';').trim_end();
                if is_skippable_statement(sql) {
                    None
                } else {
                    Some((index, sql.to_string()))
                }
            })
            .collect();

        let mut applied = 0u64;
        let mut errors: Vec<String> = Vec::new();
        if todo.is_empty() {
            return Ok(BatchResult {
                applied: 0,
                failed: 0,
                errors,
                elapsed_ms: started.elapsed().as_millis() as u64,
            });
        }

        let mut began = false;
        if atomic {
            let info = self.session_info(session_id.clone()).await?;
            if !info.in_transaction {
                self.begin_transaction(session_id.clone()).await?;
                began = true;
            }
        }

        let mut failed = 0u64;
        for (index, sql) in &todo {
            match self.execute(&session_id, sql.clone()).await {
                Ok(_) => applied += 1,
                Err(e) => {
                    failed = 1;
                    if began {
                        let _ = self.rollback_transaction(session_id.clone()).await;
                    }
                    push_capped(
                        &mut errors,
                        format!(
                            "statement #{} failed: {e} (sql: {})",
                            index + 1,
                            short_sql(sql)
                        ),
                    );
                    break;
                }
            }
        }

        if failed == 0
            && began
            && let Err(e) = self.commit_transaction(session_id.clone()).await
        {
            let _ = self.rollback_transaction(session_id.clone()).await;
            return Err(MieoError::new(
                ErrorCode::Internal,
                format!("transaction commit failed: {e}"),
            )
            .into());
        }

        Ok(BatchResult {
            applied,
            failed,
            errors,
            elapsed_ms: started.elapsed().as_millis() as u64,
        })
    }

    // -----------------------------------------------------------------------
    // File import
    // -----------------------------------------------------------------------

    /// Import a CSV or JSON file into `table`.
    ///
    /// - **CSV**: when `has_header` the first row supplies the column names
    ///   (otherwise `c1..cN`); every field is imported as text and coerced by
    ///   the database. A field equal to `null_marker` becomes SQL NULL; when
    ///   `null_marker` is empty, *every* empty field becomes NULL.
    /// - **JSON**: a JSON array of objects or JSON lines. The union of keys is
    ///   the column list (capped at 256 keys); scalars map to typed values and
    ///   nested objects/arrays map to `Value::Json`.
    ///
    /// Each batch runs in its own transaction (when the session is idle) and
    /// the run stops at the first failed batch.
    pub async fn import_file(
        &self,
        session_id: String,
        scope: Scope,
        table: String,
        path: String,
        format: ImportFormat,
        options: ImportOptions,
    ) -> ApiResult<ImportSummary> {
        let started = Instant::now();
        let table = table.trim().to_string();
        if table.is_empty() {
            return Err(MieoError::new(ErrorCode::Data, "table name is required")
                .with_hint("pick a target table before importing")
                .into());
        }

        let info = self.session_info(session_id.clone()).await?;
        let engine = info.engine;
        if !engine.is_sql() {
            return Err(MieoError::unsupported(format!(
                "file import is not supported for {}",
                engine.label()
            ))
            .into());
        }
        let style = info.capabilities.placeholder;

        // Parse the whole file before touching the database.
        let raw = std::fs::read(&path).map_err(|e| {
            MieoError::new(ErrorCode::Data, format!("cannot read `{path}`: {e}"))
                .with_hint("check that the file exists and is readable")
        })?;
        let mut errors: Vec<String> = Vec::new();
        let (columns, rows) = match format {
            ImportFormat::Csv => {
                let delimiter = match options.delimiter.as_deref() {
                    None => DEFAULT_CSV_DELIMITER,
                    Some(text) => {
                        let bytes = text.as_bytes();
                        if bytes.len() != 1 || !bytes[0].is_ascii() {
                            return Err(MieoError::new(
                                ErrorCode::Data,
                                format!("delimiter `{text}` is invalid"),
                            )
                            .with_hint("pass a single ASCII character, e.g. \",\" or \";\"")
                            .into());
                        }
                        bytes[0]
                    }
                };
                parse_csv(raw.as_slice(), delimiter, options.has_header, &options.null_marker)?
            }
            ImportFormat::Json => parse_json(&raw, &mut errors)?,
        };

        let rows_read = rows.len() as u64;
        if rows.is_empty() {
            return Ok(ImportSummary {
                rows_read: 0,
                rows_inserted: 0,
                rows_failed: 0,
                errors,
                elapsed_ms: started.elapsed().as_millis() as u64,
            });
        }
        if columns.is_empty() {
            return Err(MieoError::new(ErrorCode::Data, "the file has no columns to import")
                .with_hint("check the CSV header or the JSON object keys")
                .into());
        }

        let qualified = qualified_table(engine, &scope, &table);
        if options.mode == ImportMode::Truncate {
            self.execute(&session_id, truncate_sql(engine, &qualified))
                .await?;
        }

        let quoted_columns: Vec<String> =
            columns.iter().map(|c| quote_ident(engine, c)).collect();
        let requested_batch = options.batch_size.unwrap_or(DEFAULT_IMPORT_BATCH).max(1) as usize;
        // Keep the placeholder count per statement under the engine's ceiling.
        let params_ceiling = match engine {
            Engine::SqlServer => 2_000,
            Engine::Sqlite => 900,
            Engine::Postgres => 30_000,
            _ => 10_000,
        };
        let batch_size = requested_batch.min((params_ceiling / columns.len().max(1)).max(1));

        let mut inserted = 0u64;
        let mut failed = 0u64;
        let mut done = 0u64;
        for (batch_index, chunk) in rows.chunks(batch_size).enumerate() {
            let sql = build_insert_sql(style, &qualified, &quoted_columns, chunk.len());
            let params: Vec<Value> = chunk.iter().flatten().cloned().collect();

            if !info.in_transaction {
                self.begin_transaction(session_id.clone()).await?;
            }
            match self
                .collect_query(&session_id, sql.clone(), params, QueryMode::Prepared, None)
                .await
            {
                Ok(_) => {
                    if !info.in_transaction
                        && let Err(e) = self.commit_transaction(session_id.clone()).await
                    {
                        let _ = self.rollback_transaction(session_id.clone()).await;
                        failed += chunk.len() as u64;
                        push_capped(
                            &mut errors,
                            format!("batch #{} commit failed: {e}", batch_index + 1),
                        );
                        break;
                    }
                    inserted += chunk.len() as u64;
                }
                Err(e) => {
                    if !info.in_transaction {
                        let _ = self.rollback_transaction(session_id.clone()).await;
                    }
                    failed += chunk.len() as u64;
                    push_capped(
                        &mut errors,
                        format!(
                            "batch #{} failed (rows {}-{}): {e}",
                            batch_index + 1,
                            done + 1,
                            done + chunk.len() as u64
                        ),
                    );
                    break;
                }
            }
            done += chunk.len() as u64;
        }

        Ok(ImportSummary {
            rows_read,
            rows_inserted: inserted,
            rows_failed: failed,
            errors,
            elapsed_ms: started.elapsed().as_millis() as u64,
        })
    }

    // -----------------------------------------------------------------------
    // Grid edits
    // -----------------------------------------------------------------------

    /// Apply staged data-grid edits in one transaction.
    ///
    /// Inserts, updates and deletes are parameterized; when the session is
    /// idle a transaction is opened and any failure rolls the whole run back
    /// (the failing statement is reported). Updates and deletes without key
    /// columns are rejected before anything runs.
    pub async fn apply_grid_edits(&self, request: GridEditRequest) -> ApiResult<EditApplyResult> {
        let started = Instant::now();
        let table = request.table.trim().to_string();
        if table.is_empty() {
            return Err(MieoError::new(ErrorCode::Data, "table name is required")
                .with_hint("the grid must know which table it edited")
                .into());
        }
        for (index, update) in request.updates.iter().enumerate() {
            if update.keys.is_empty() {
                return Err(MieoError::new(
                    ErrorCode::Data,
                    format!("update #{} has no key columns", index + 1),
                )
                .with_hint("send the primary-key values for every updated row")
                .into());
            }
        }
        for (index, delete) in request.deletes.iter().enumerate() {
            if delete.is_empty() {
                return Err(MieoError::new(
                    ErrorCode::Data,
                    format!("delete #{} has no key columns", index + 1),
                )
                .with_hint("send the primary-key values of every deleted row")
                .into());
            }
        }

        let info = self.session_info(request.session_id.clone()).await?;
        let engine = info.engine;
        let style = info.capabilities.placeholder;
        let qualified = qualified_table(engine, &request.scope, &table);

        let began = !info.in_transaction;
        if began {
            self.begin_transaction(request.session_id.clone()).await?;
        }

        let mut inserted = 0u64;
        let mut updated = 0u64;
        let mut deleted = 0u64;
        let mut errors: Vec<String> = Vec::new();

        let failure: Option<String> = 'apply: {
            for (index, row) in request.inserts.iter().enumerate() {
                if row.is_empty() {
                    continue;
                }
                let columns: Vec<String> = row
                    .iter()
                    .map(|cell| quote_ident(engine, &cell.column))
                    .collect();
                let sql = build_insert_sql(style, &qualified, &columns, 1);
                let params: Vec<Value> = row.iter().map(|cell| cell.value.clone()).collect();
                match self
                    .collect_query(
                        &request.session_id,
                        sql.clone(),
                        params,
                        QueryMode::Prepared,
                        None,
                    )
                    .await
                {
                    Ok(_) => inserted += 1,
                    Err(e) => {
                        break 'apply Some(format!(
                            "insert #{} failed: {e} (sql: {})",
                            index + 1,
                            short_sql(&sql)
                        ));
                    }
                }
            }

            for (index, update) in request.updates.iter().enumerate() {
                if update.changes.is_empty() {
                    continue;
                }
                let mut placeholder_index = 0usize;
                let sets: Vec<String> = update
                    .changes
                    .iter()
                    .map(|cell| {
                        placeholder_index += 1;
                        format!(
                            "{} = {}",
                            quote_ident(engine, &cell.column),
                            placeholder(style, placeholder_index)
                        )
                    })
                    .collect();
                let conditions: Vec<String> = update
                    .keys
                    .iter()
                    .map(|cell| {
                        placeholder_index += 1;
                        format!(
                            "{} = {}",
                            quote_ident(engine, &cell.column),
                            placeholder(style, placeholder_index)
                        )
                    })
                    .collect();
                let sql = format!(
                    "UPDATE {qualified} SET {} WHERE {}",
                    sets.join(", "),
                    conditions.join(" AND ")
                );
                let params: Vec<Value> = update
                    .changes
                    .iter()
                    .map(|cell| cell.value.clone())
                    .chain(update.keys.iter().map(|cell| cell.value.clone()))
                    .collect();
                match self
                    .collect_query(
                        &request.session_id,
                        sql.clone(),
                        params,
                        QueryMode::Prepared,
                        None,
                    )
                    .await
                {
                    Ok(_) => updated += 1,
                    Err(e) => {
                        break 'apply Some(format!(
                            "update #{} failed: {e} (sql: {})",
                            index + 1,
                            short_sql(&sql)
                        ));
                    }
                }
            }

            for (index, keys) in request.deletes.iter().enumerate() {
                let mut placeholder_index = 0usize;
                let conditions: Vec<String> = keys
                    .iter()
                    .map(|cell| {
                        placeholder_index += 1;
                        format!(
                            "{} = {}",
                            quote_ident(engine, &cell.column),
                            placeholder(style, placeholder_index)
                        )
                    })
                    .collect();
                let sql = format!(
                    "DELETE FROM {qualified} WHERE {}",
                    conditions.join(" AND ")
                );
                let params: Vec<Value> = keys.iter().map(|cell| cell.value.clone()).collect();
                match self
                    .collect_query(
                        &request.session_id,
                        sql.clone(),
                        params,
                        QueryMode::Prepared,
                        None,
                    )
                    .await
                {
                    Ok(_) => deleted += 1,
                    Err(e) => {
                        break 'apply Some(format!(
                            "delete #{} failed: {e} (sql: {})",
                            index + 1,
                            short_sql(&sql)
                        ));
                    }
                }
            }

            None
        };

        if let Some(message) = failure {
            if began {
                let _ = self.rollback_transaction(request.session_id.clone()).await;
            }
            push_capped(&mut errors, message);
            return Ok(EditApplyResult {
                inserted,
                updated,
                deleted,
                failed: 1,
                errors,
                elapsed_ms: started.elapsed().as_millis() as u64,
            });
        }

        if began
            && let Err(e) = self.commit_transaction(request.session_id.clone()).await
        {
            let _ = self.rollback_transaction(request.session_id.clone()).await;
            return Err(MieoError::new(
                ErrorCode::Internal,
                format!("transaction commit failed: {e}"),
            )
            .into());
        }

        Ok(EditApplyResult {
            inserted,
            updated,
            deleted,
            failed: 0,
            errors,
            elapsed_ms: started.elapsed().as_millis() as u64,
        })
    }

    // -----------------------------------------------------------------------
    // Script splitting
    // -----------------------------------------------------------------------

    /// Read `path` (UTF-8, lossy) and split it into individual SQL statements.
    ///
    /// The tokenizer understands single/double quotes, backticks, SQL Server
    /// `[…]` identifiers, `--` / `#` line comments, non-nested `/* */` block
    /// comments and Postgres dollar quoting (`$$…$$`, `$tag$…$tag$`).
    /// `DELIMITER <token>` client directives (mysql/Navicat style) switch the
    /// statement terminator and are consumed, never sent to the server; `;`
    /// inside `BEGIN..END` routine bodies does not split. Comments are
    /// stripped and statements are returned trimmed, without the terminator.
    pub fn script_statements(&self, path: String) -> ApiResult<ScriptStatements> {
        let bytes = std::fs::read(&path).map_err(|e| {
            MieoError::new(ErrorCode::Data, format!("cannot read script `{path}`: {e}"))
                .with_hint("check that the file exists and is readable")
        })?;
        let text = String::from_utf8_lossy(&bytes);
        let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
        let (statements, warnings) = split_sql_script(text);
        Ok(ScriptStatements {
            statements,
            warnings,
        })
    }

    /// Split editor text into statements with the same tokenizer the script
    /// runner uses. The query editor calls this before running: a single
    /// statement keeps the prepared streaming path, while multi-statement
    /// input (notably `DELIMITER`-wrapped routine definitions) runs
    /// statement-by-statement on the simple execution path, like Navicat.
    pub fn split_sql_text(&self, text: String) -> ScriptStatements {
        let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
        let (statements, warnings) = split_sql_script(text);
        ScriptStatements {
            statements,
            warnings,
        }
    }

    // -----------------------------------------------------------------------
    // Backup dump
    // -----------------------------------------------------------------------

    /// Stream the scope to `path` as a `.sql` backup: one section per table
    /// with the object DDL, plus (when `include_data`) the rows as multi-row
    /// INSERT statements, 500 rows per statement.
    ///
    /// `tables` restricts the dump when non-empty. Tables whose DDL is
    /// unavailable are skipped and reported in `errors`. On a fatal error the
    /// partial file is deleted, mirroring `export_result`.
    pub async fn dump_backup(
        &self,
        session_id: String,
        scope: Scope,
        path: String,
        include_data: bool,
        tables: Option<Vec<String>>,
    ) -> ApiResult<BackupSummary> {
        let started = Instant::now();
        let outcome = self
            .write_dump(&session_id, &scope, &path, include_data, tables)
            .await;
        match outcome {
            Ok((tables, rows, errors)) => Ok(BackupSummary {
                tables,
                rows,
                bytes: std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0),
                errors,
                elapsed_ms: started.elapsed().as_millis() as u64,
            }),
            Err(e) => {
                // Never leave a partial dump behind.
                let _ = std::fs::remove_file(&path);
                Err(e)
            }
        }
    }

    /// Write the dump file; `Err` is fatal (the caller deletes the file).
    async fn write_dump(
        &self,
        session_id: &str,
        scope: &Scope,
        path: &str,
        include_data: bool,
        tables: Option<Vec<String>>,
    ) -> ApiResult<(u64, u64, Vec<String>)> {
        let info = self.session_info(session_id.to_string()).await?;
        let engine = info.engine;
        if !engine.is_sql() {
            return Err(MieoError::unsupported(format!(
                "backup is not supported for {}",
                engine.label()
            ))
            .into());
        }

        let mut errors: Vec<String> = Vec::new();
        let objects = self
            .list_objects(
                session_id.to_string(),
                scope.clone(),
                Some(ObjectKind::Table),
                None,
            )
            .await?;

        let filter: Option<HashSet<String>> = tables
            .filter(|list| !list.is_empty())
            .map(|list| {
                list.into_iter()
                    .map(|name| name.trim().to_lowercase())
                    .filter(|name| !name.is_empty())
                    .collect()
            });
        if let Some(filter) = &filter {
            let mut wanted: Vec<&String> = filter.iter().collect();
            wanted.sort();
            for name in wanted {
                if !objects
                    .iter()
                    .any(|o| o.name.to_lowercase() == *name)
                {
                    push_capped(
                        &mut errors,
                        format!("table `{name}` was not found in this scope"),
                    );
                }
            }
        }
        let selected: Vec<String> = objects
            .iter()
            .filter(|o| {
                filter
                    .as_ref()
                    .is_none_or(|f| f.contains(&o.name.to_lowercase()))
            })
            .map(|o| o.name.clone())
            .collect();

        let file = File::create(path).map_err(|e| {
            MieoError::new(ErrorCode::Data, format!("cannot create file `{path}`: {e}"))
                .with_hint("check that the destination folder exists and is writable")
        })?;
        let mut out = BufWriter::new(file);

        writeln!(out, "-- MieoSQL backup").map_err(write_err)?;
        writeln!(out, "-- engine: {}", engine.label()).map_err(write_err)?;
        writeln!(out, "-- scope: {}", scope_label(scope)).map_err(write_err)?;
        writeln!(
            out,
            "-- generated: {}",
            chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
        )
        .map_err(write_err)?;
        writeln!(out, "-- tables: {}", selected.len()).map_err(write_err)?;
        if !include_data {
            writeln!(out, "-- data: omitted").map_err(write_err)?;
        }

        let mut tables_dumped = 0u64;
        let mut rows_written = 0u64;

        for table in &selected {
            writeln!(
                out,
                "\n-- ---------------------------------------------------------------"
            )
            .map_err(write_err)?;
            writeln!(out, "-- Table: {table}").map_err(write_err)?;

            let ddl = match self
                .object_ddl(
                    session_id.to_string(),
                    scope.clone(),
                    ObjectKind::Table,
                    table.clone(),
                )
                .await
            {
                Ok(Some(ddl)) if !ddl.trim().is_empty() => ddl,
                Ok(_) => {
                    push_capped(
                        &mut errors,
                        format!("DDL for `{table}` is unavailable; table skipped"),
                    );
                    continue;
                }
                Err(e) => {
                    push_capped(
                        &mut errors,
                        format!("DDL for `{table}` failed: {e}; table skipped"),
                    );
                    continue;
                }
            };
            let ddl = ddl.trim().trim_end_matches(';').trim_end();
            writeln!(out, "{ddl};").map_err(write_err)?;
            tables_dumped += 1;

            if !include_data {
                continue;
            }

            let qualified = qualified_table(engine, scope, table);
            let mut stream = self
                .start_query(
                    session_id.to_string(),
                    format!("SELECT * FROM {qualified}"),
                    Vec::new(),
                    Some(QueryOptions {
                        mode: QueryMode::Prepared,
                        fetch_size: 2048,
                        max_rows: None,
                        timeout_ms: None,
                    }),
                )
                .await?;

            let mut column_names: Vec<String> = Vec::new();
            while let Some(event) = stream.events.recv().await {
                match event {
                    QueryEvent::Columns { columns, .. } => {
                        column_names = columns.into_iter().map(|c| c.name).collect();
                    }
                    QueryEvent::Batch { rows, .. } => {
                        if column_names.is_empty() {
                            let width = rows.first().map(|row| row.len()).unwrap_or(0);
                            column_names =
                                (1..=width).map(|i| format!("column_{i}")).collect();
                        }
                        let quoted: Vec<String> = column_names
                            .iter()
                            .map(|name| quote_ident(engine, name))
                            .collect();
                        for chunk in rows.chunks(DUMP_INSERT_CHUNK) {
                            let statement = insert_literal_sql(engine, &qualified, &quoted, chunk);
                            writeln!(out, "{statement}").map_err(write_err)?;
                            rows_written += chunk.len() as u64;
                        }
                    }
                    QueryEvent::Error { error, .. } => {
                        // Fatal: the caller removes the partial file.
                        return Err(ApiError::from(error));
                    }
                    QueryEvent::Finished { .. } => break,
                    _ => {}
                }
            }
        }

        writeln!(out, "\n-- end of backup").map_err(write_err)?;
        out.flush().map_err(write_err)?;
        Ok((tables_dumped, rows_written, errors))
    }
}

// ---------------------------------------------------------------------------
// SQL builders and small helpers
// ---------------------------------------------------------------------------

/// `schema.table` (Postgres) / `database.table` (others), engine-quoted.
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

/// Human-readable scope for the backup header.
fn scope_label(scope: &Scope) -> String {
    let mut parts = Vec::new();
    if let Some(database) = scope.database.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        parts.push(format!("database `{database}`"));
    }
    if let Some(schema) = scope.schema.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        parts.push(format!("schema `{schema}`"));
    }
    if parts.is_empty() {
        "(default scope)".to_string()
    } else {
        parts.join(", ")
    }
}

/// Statement that empties a table before a truncating import.
fn truncate_sql(engine: Engine, qualified: &str) -> String {
    match engine {
        // SQLite has no TRUNCATE; SQL Server resets identity on TRUNCATE, so
        // a plain DELETE is the conservative "remove the rows" statement.
        Engine::Sqlite | Engine::SqlServer => format!("DELETE FROM {qualified}"),
        _ => format!("TRUNCATE TABLE {qualified}"),
    }
}

/// Parameter placeholder for the session's placeholder style, 1-based.
fn placeholder(style: PlaceholderStyle, index_1based: usize) -> String {
    match style {
        PlaceholderStyle::DollarNumbered => format!("${index_1based}"),
        PlaceholderStyle::Question => "?".to_string(),
    }
}

/// Prepared multi-row INSERT with the session's placeholders. `columns` must
/// already be quoted.
fn build_insert_sql(
    style: PlaceholderStyle,
    qualified: &str,
    columns: &[String],
    row_count: usize,
) -> String {
    let mut tuples = Vec::with_capacity(row_count);
    let mut index = 0usize;
    for _ in 0..row_count {
        let mut tuple = Vec::with_capacity(columns.len());
        for _ in 0..columns.len() {
            index += 1;
            tuple.push(placeholder(style, index));
        }
        tuples.push(format!("({})", tuple.join(", ")));
    }
    format!(
        "INSERT INTO {qualified} ({}) VALUES {}",
        columns.join(", "),
        tuples.join(", ")
    )
}

/// Literal (non-parameterized) multi-row INSERT for backup files.
fn insert_literal_sql(
    engine: Engine,
    qualified: &str,
    quoted_columns: &[String],
    rows: &[Vec<Value>],
) -> String {
    let tuples: Vec<String> = rows
        .iter()
        .map(|row| {
            let values: Vec<String> = row.iter().map(|v| sql_literal(engine, v)).collect();
            format!("({})", values.join(", "))
        })
        .collect();
    format!(
        "INSERT INTO {qualified} ({}) VALUES\n  {};",
        quoted_columns.join(", "),
        tuples.join(",\n  ")
    )
}

/// CSV text into `(column names, rows)`. Fields map to `Value::Text`; a field
/// equal to `null_marker` (or any empty field when `null_marker` is empty)
/// becomes `Value::Null`.
fn parse_csv(
    input: impl std::io::Read,
    delimiter: u8,
    has_header: bool,
    null_marker: &str,
) -> Result<(Vec<String>, Vec<Vec<Value>>), MieoError> {
    let mut reader = csv::ReaderBuilder::new()
        .delimiter(delimiter)
        .has_headers(false)
        .flexible(true)
        .from_reader(input);
    let mut records: Vec<csv::StringRecord> = Vec::new();
    for record in reader.records() {
        records.push(record.map_err(|e| {
            MieoError::new(ErrorCode::Data, format!("invalid CSV: {e}"))
                .with_hint("check the delimiter and quoting of the file")
        })?);
    }
    let width = records.iter().map(|record| record.len()).max().unwrap_or(0);
    let columns: Vec<String> = if has_header {
        if records.is_empty() {
            return Ok((Vec::new(), Vec::new()));
        }
        records[0]
            .iter()
            .enumerate()
            .map(|(index, name)| {
                // Excel-style files start with a UTF-8 BOM on the first header.
                let name = name.strip_prefix('\u{feff}').unwrap_or(name);
                if name.is_empty() {
                    format!("c{}", index + 1)
                } else {
                    name.to_string()
                }
            })
            .collect()
    } else {
        (1..=width).map(|i| format!("c{i}")).collect()
    };

    let data = if has_header { &records[1..] } else { &records[..] };
    let rows = data
        .iter()
        .map(|record| {
            columns
                .iter()
                .enumerate()
                .map(|(index, _)| match record.get(index) {
                    None => Value::Null,
                    Some(field) => {
                        let is_null = if null_marker.is_empty() {
                            field.is_empty()
                        } else {
                            field == null_marker
                        };
                        if is_null {
                            Value::Null
                        } else {
                            Value::Text(field.to_string())
                        }
                    }
                })
                .collect()
        })
        .collect();
    Ok((columns, rows))
}

/// JSON (array of objects or JSON lines) into `(union of keys, rows)`.
///
/// Keys are collected in first-seen order and capped at [`MAX_JSON_KEYS`]
/// (the overflow is reported through `errors`). Scalar values map to typed
/// values; nested objects/arrays map to `Value::Json`; missing keys are NULL.
fn parse_json(
    input: &[u8],
    errors: &mut Vec<String>,
) -> Result<(Vec<String>, Vec<Vec<Value>>), MieoError> {
    use serde_json::Value as JsonValue;

    let text = String::from_utf8_lossy(input);
    let trimmed = text.trim_start_matches('\u{feff}').trim();
    if trimmed.is_empty() {
        return Ok((Vec::new(), Vec::new()));
    }

    let mut objects: Vec<serde_json::Map<String, JsonValue>> = Vec::new();
    match serde_json::from_str::<JsonValue>(trimmed) {
        Ok(JsonValue::Array(items)) => {
            for (index, item) in items.into_iter().enumerate() {
                match item {
                    JsonValue::Object(object) => objects.push(object),
                    other => {
                        return Err(MieoError::new(
                            ErrorCode::Data,
                            format!(
                                "JSON array item #{} is {}; expected an object",
                                index + 1,
                                json_kind(&other)
                            ),
                        )
                        .with_hint("a JSON import expects an array of objects or one object per line"));
                    }
                }
            }
        }
        Ok(JsonValue::Object(object)) => objects.push(object),
        Ok(other) => {
            return Err(MieoError::new(
                ErrorCode::Data,
                format!("top-level JSON value is {}; expected an array of objects", json_kind(&other)),
            )
            .with_hint("a JSON import expects an array of objects or one object per line"));
        }
        Err(_) => {
            // Fall back to JSON lines.
            for (line_index, line) in trimmed.lines().enumerate() {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                match serde_json::from_str::<JsonValue>(line) {
                    Ok(JsonValue::Object(object)) => objects.push(object),
                    Ok(other) => {
                        return Err(MieoError::new(
                            ErrorCode::Data,
                            format!(
                                "JSON on line {} is {}; expected an object",
                                line_index + 1,
                                json_kind(&other)
                            ),
                        ));
                    }
                    Err(e) => {
                        return Err(MieoError::new(
                            ErrorCode::Data,
                            format!("invalid JSON on line {}: {e}", line_index + 1),
                        )
                        .with_hint("use a JSON array or one JSON object per line"));
                    }
                }
            }
        }
    }

    let mut keys: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut ignored = 0u64;
    for object in &objects {
        for key in object.keys() {
            if !seen.insert(key.clone()) {
                continue;
            }
            if keys.len() >= MAX_JSON_KEYS {
                ignored += 1;
                continue;
            }
            keys.push(key.clone());
        }
    }
    if ignored > 0 {
        push_capped(
            errors,
            format!(
                "only the first {MAX_JSON_KEYS} columns were imported; {ignored} key(s) were ignored"
            ),
        );
    }

    let rows = objects
        .iter()
        .map(|object| {
            keys.iter()
                .map(|key| object.get(key).map(json_to_value).unwrap_or(Value::Null))
                .collect()
        })
        .collect();
    Ok((keys, rows))
}

/// JSON scalar -> [`Value`]; nested containers stay JSON.
fn json_to_value(value: &serde_json::Value) -> Value {
    use serde_json::Value as JsonValue;
    match value {
        JsonValue::Null => Value::Null,
        JsonValue::Bool(b) => Value::Bool(*b),
        JsonValue::Number(number) => {
            if let Some(int) = number.as_i64() {
                Value::Int(int)
            } else if let Some(uint) = number.as_u64() {
                Value::UInt(uint)
            } else if let Some(float) = number.as_f64().filter(|f| f.is_finite()) {
                Value::Float(float)
            } else {
                Value::Text(number.to_string())
            }
        }
        JsonValue::String(text) => Value::Text(text.clone()),
        other => Value::Json(other.clone()),
    }
}

/// Short JSON kind name for error messages.
fn json_kind(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "a boolean",
        serde_json::Value::Number(_) => "a number",
        serde_json::Value::String(_) => "a string",
        serde_json::Value::Array(_) => "an array",
        serde_json::Value::Object(_) => "an object",
    }
}

/// Empty and comment-only statements never reach the engine.
fn is_skippable_statement(sql: &str) -> bool {
    let bytes = sql.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        let b = bytes[i];
        if b.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if b == b'-' && bytes.get(i + 1) == Some(&b'-') {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if b == b'#' && starts_hash_comment(bytes, i) {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if b == b'/' && bytes.get(i + 1) == Some(&b'*') {
            match sql[i + 2..].find("*/") {
                Some(end) => {
                    i = i + 2 + end + 2;
                    continue;
                }
                None => return true,
            }
        }
        return false;
    }
    true
}

/// First line of `sql`, capped for error messages.
fn short_sql(sql: &str) -> String {
    let first = sql.trim().lines().next().unwrap_or("");
    if first.chars().count() > 160 {
        let mut out: String = first.chars().take(160).collect();
        out.push('…');
        out
    } else {
        first.to_string()
    }
}

/// Append unless the summary's error list is already at its cap.
fn push_capped(errors: &mut Vec<String>, message: String) {
    if errors.len() < MAX_ERRORS {
        errors.push(message);
    }
}

/// `writeln!` I/O failure -> API error.
fn write_err(e: std::io::Error) -> ApiError {
    MieoError::from(e).into()
}

// ---------------------------------------------------------------------------
// SQL script splitter
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScanState {
    Normal,
    SingleQuote,
    DoubleQuote,
    Backtick,
    Bracket,
    LineComment,
    BlockComment,
    DollarQuote,
}

/// Split a SQL script into statements (see [`Api::script_statements`]).
fn split_sql_script(text: &str) -> (Vec<String>, Vec<String>) {
    let bytes = text.as_bytes();
    let mut statements: Vec<String> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut state = ScanState::Normal;
    let mut dollar_tag: Vec<u8> = Vec::new();
    // Active statement terminator. `DELIMITER <token>` directives (a client
    // command a la mysql/Navicat — never sent to the server) switch it, so
    // routine bodies delimited by e.g. `$$` survive as one statement.
    let mut delim: Vec<u8> = vec![b';'];
    // `BEGIN..END` nesting inside CREATE PROCEDURE/FUNCTION/TRIGGER/EVENT.
    // While > 0 the terminator does not split, which keeps routine bodies
    // whole even when the script uses no DELIMITER directive (SQLite
    // triggers, pasted procedures, T-SQL batches, ...).
    let mut depth: usize = 0;
    // First words of the pending statement (uppercased) + previous word, for
    // routine detection and `END CASE` handling.
    let mut head_words: Vec<String> = Vec::new();
    let mut prev_word = String::new();
    let mut i = 0usize;

    // Flush the pending statement and reset per-statement tracking. (The
    // BEGIN..END depth needs no reset: a split only ever happens at depth 0.)
    macro_rules! flush {
        () => {{
            push_statement(&mut statements, &mut current);
            head_words.clear();
            prev_word.clear();
        }};
    }

    while i < bytes.len() {
        let b = bytes[i];
        match state {
            ScanState::Normal => {
                // A DELIMITER directive is only recognised at statement start
                // (leading whitespace/comments only) — elsewhere `delimiter`
                // is an ordinary word (e.g. a column name).
                if (b == b'd' || b == b'D') && current.trim().is_empty() {
                    if let Some((token, next)) = parse_delimiter_directive(bytes, i) {
                        delim = token;
                        i = next;
                        continue;
                    }
                }
                // Statement terminator (outside routine bodies).
                if depth == 0 && bytes[i..].starts_with(&delim) {
                    flush!();
                    i += delim.len();
                    continue;
                }
                match b {
                    b'\'' => {
                        current.push('\'');
                        state = ScanState::SingleQuote;
                        i += 1;
                    }
                    b'"' => {
                        current.push('"');
                        state = ScanState::DoubleQuote;
                        i += 1;
                    }
                    b'`' => {
                        current.push('`');
                        state = ScanState::Backtick;
                        i += 1;
                    }
                    b'[' => {
                        current.push('[');
                        state = ScanState::Bracket;
                        i += 1;
                    }
                    b'-' if bytes.get(i + 1) == Some(&b'-') => {
                        state = ScanState::LineComment;
                        i += 2;
                    }
                    b'#' if starts_hash_comment(bytes, i) => {
                        state = ScanState::LineComment;
                        i += 1;
                    }
                    b'/' if bytes.get(i + 1) == Some(&b'*') => {
                        state = ScanState::BlockComment;
                        i += 2;
                    }
                    b'$' => match dollar_quote_open(bytes, i) {
                        Some((tag, end)) => {
                            current.push_str(&text[i..end]);
                            dollar_tag = tag;
                            state = ScanState::DollarQuote;
                            i = end;
                        }
                        None => {
                            current.push('$');
                            i += 1;
                        }
                    },
                    // Bare words: maximal munch keeps `end_date` / `my_begin`
                    // identifiers from tripping the BEGIN/END tracking, and
                    // quoted identifiers / comments never reach this arm.
                    b'_' | b'a'..=b'z' | b'A'..=b'Z' => {
                        let start = i;
                        while i < bytes.len()
                            && (bytes[i] == b'_'
                                || bytes[i] == b'$'
                                || bytes[i].is_ascii_alphanumeric())
                            // The active delimiter always ends a word, so a
                            // glued terminator (`END$$`, `END//`) still
                            // closes the block and then splits.
                            && !(delim.len() > 1 && bytes[i..].starts_with(&delim))
                        {
                            i += 1;
                        }
                        // ASCII-only scan: slicing is safe.
                        let word = &text[start..i];
                        current.push_str(word);
                        let upper = word.to_ascii_uppercase();
                        if head_words.len() < 6 {
                            head_words.push(upper.clone());
                        }
                        if head_is_routine(&head_words) {
                            match upper.as_str() {
                                "BEGIN" => depth += 1,
                                // `CASE..END [CASE]`: the END already closed
                                // it, so the CASE in `END CASE` must not
                                // re-open a level.
                                "CASE" => {
                                    if prev_word != "END" {
                                        depth += 1;
                                    }
                                }
                                // Covers END, END IF/LOOP/WHILE/REPEAT/CASE —
                                // each carries exactly one END token.
                                "END" => depth = depth.saturating_sub(1),
                                _ => {}
                            }
                        }
                        prev_word = upper;
                    }
                    _ => push_char(text, &mut current, &mut i),
                }
            }
            ScanState::SingleQuote => {
                if b == b'\'' {
                    if bytes.get(i + 1) == Some(&b'\'') {
                        current.push_str("''");
                        i += 2;
                    } else {
                        current.push('\'');
                        state = ScanState::Normal;
                        i += 1;
                    }
                } else {
                    push_char(text, &mut current, &mut i);
                }
            }
            ScanState::DoubleQuote => {
                if b == b'"' {
                    if bytes.get(i + 1) == Some(&b'"') {
                        current.push_str("\"\"");
                        i += 2;
                    } else {
                        current.push('"');
                        state = ScanState::Normal;
                        i += 1;
                    }
                } else {
                    push_char(text, &mut current, &mut i);
                }
            }
            ScanState::Backtick => {
                if b == b'`' {
                    if bytes.get(i + 1) == Some(&b'`') {
                        current.push_str("``");
                        i += 2;
                    } else {
                        current.push('`');
                        state = ScanState::Normal;
                        i += 1;
                    }
                } else {
                    push_char(text, &mut current, &mut i);
                }
            }
            ScanState::Bracket => {
                if b == b']' {
                    if bytes.get(i + 1) == Some(&b']') {
                        current.push_str("]]");
                        i += 2;
                    } else {
                        current.push(']');
                        state = ScanState::Normal;
                        i += 1;
                    }
                } else {
                    push_char(text, &mut current, &mut i);
                }
            }
            ScanState::LineComment => {
                if b == b'\n' {
                    current.push('\n');
                    state = ScanState::Normal;
                }
                i += 1;
            }
            ScanState::BlockComment => {
                if b == b'*' && bytes.get(i + 1) == Some(&b'/') {
                    // Replace the comment with a space so neighbouring tokens
                    // never glue together.
                    current.push(' ');
                    state = ScanState::Normal;
                    i += 2;
                } else {
                    i += 1;
                }
            }
            ScanState::DollarQuote => {
                if b == b'$' && dollar_quote_close_matches(bytes, i, &dollar_tag) {
                    let end = i + dollar_tag.len() + 2;
                    current.push_str(&text[i..end]);
                    dollar_tag.clear();
                    state = ScanState::Normal;
                    i = end;
                } else {
                    push_char(text, &mut current, &mut i);
                }
            }
        }
    }

    match state {
        ScanState::Normal | ScanState::LineComment => {}
        ScanState::SingleQuote => {
            warnings.push("unterminated string literal (missing closing ')".to_string());
        }
        ScanState::DoubleQuote => {
            warnings.push("unterminated quoted identifier (missing closing \")".to_string());
        }
        ScanState::Backtick => {
            warnings.push("unterminated backtick identifier (missing closing `)".to_string());
        }
        ScanState::Bracket => {
            warnings.push("unterminated bracket identifier (missing closing ])".to_string());
        }
        ScanState::BlockComment => {
            warnings.push("unterminated block comment (missing */)".to_string());
        }
        ScanState::DollarQuote => warnings.push(format!(
            "unterminated dollar-quoted string (${}$)",
            String::from_utf8_lossy(&dollar_tag)
        )),
    }
    if depth > 0 {
        warnings.push(format!(
            "unbalanced BEGIN..END block (depth {depth} at end of script); \
             the trailing statements may have been merged"
        ));
    }
    flush!();
    (statements, warnings)
}

/// Copy one whole UTF-8 char at byte offset `*i`, advancing `*i`.
fn push_char(text: &str, out: &mut String, i: &mut usize) {
    let ch = text[*i..].chars().next().unwrap_or('\u{fffd}');
    out.push(ch);
    *i += ch.len_utf8();
}

/// Does the pending statement open a stored-routine body (`CREATE ...
/// PROCEDURE|FUNCTION|TRIGGER|EVENT ...`)? Only inside such statements is
/// `BEGIN..END` nesting tracked, so plain transaction scripts
/// (`BEGIN; ... COMMIT;`) keep splitting at every `;` as before.
fn head_is_routine(head_words: &[String]) -> bool {
    let Some(first) = head_words.first() else {
        return false;
    };
    if first != "CREATE" {
        return false;
    }
    head_words[1..]
        .iter()
        .any(|w| matches!(w.as_str(), "PROCEDURE" | "FUNCTION" | "TRIGGER" | "EVENT"))
}

/// Parse a `DELIMITER <token>` client directive at byte offset `i`
/// (case-insensitive, same line). Returns the new delimiter and the offset
/// just past the end of the line. The directive itself is consumed — it is
/// never part of any statement and must never reach the server.
fn parse_delimiter_directive(bytes: &[u8], i: usize) -> Option<(Vec<u8>, usize)> {
    const KW: &[u8] = b"delimiter";
    if bytes.len() < i + KW.len() {
        return None;
    }
    if !bytes[i..i + KW.len()].eq_ignore_ascii_case(KW) {
        return None;
    }
    let mut j = i + KW.len();
    // Word boundary: `delimitersessioni` is not a directive.
    if j < bytes.len() && (bytes[j] == b'_' || bytes[j] == b'$' || bytes[j].is_ascii_alphanumeric())
    {
        return None;
    }
    // Spaces/tabs only — the token must stay on the same line.
    while j < bytes.len() && (bytes[j] == b' ' || bytes[j] == b'\t' || bytes[j] == b'\r') {
        j += 1;
    }
    if j >= bytes.len() || bytes[j] == b'\n' {
        return None;
    }
    let start = j;
    while j < bytes.len() && !bytes[j].is_ascii_whitespace() {
        j += 1;
    }
    let token = bytes[start..j].to_vec();
    if token.is_empty() {
        return None;
    }
    // Swallow the rest of the line (trailing comments included).
    while j < bytes.len() && bytes[j] != b'\n' {
        j += 1;
    }
    if j < bytes.len() {
        j += 1; // consume the newline itself
    }
    Some((token, j))
}

/// Push the buffered statement (trimmed) unless it is empty, then reset.
fn push_statement(statements: &mut Vec<String>, current: &mut String) {
    let trimmed = current.trim();
    if !trimmed.is_empty() {
        statements.push(trimmed.to_string());
    }
    current.clear();
}

/// `#` starts a MySQL comment when it is followed by whitespace or ends the
/// line, or when it is the first character of a line. Elsewhere it is kept —
/// SQL Server `#temp` tables must survive.
fn starts_hash_comment(bytes: &[u8], i: usize) -> bool {
    if bytes.get(i + 1).is_none_or(|b| b.is_ascii_whitespace()) {
        return true;
    }
    let mut j = i;
    while j > 0 {
        match bytes[j - 1] {
            b'\n' => return true,
            b' ' | b'\t' | b'\r' => j -= 1,
            _ => return false,
        }
    }
    true
}

/// `$tag$` / `$$` at `i`; returns the tag and the offset just past the opener.
fn dollar_quote_open(bytes: &[u8], i: usize) -> Option<(Vec<u8>, usize)> {
    debug_assert_eq!(bytes[i], b'$');
    let mut j = i + 1;
    if bytes.get(j) == Some(&b'$') {
        return Some((Vec::new(), j + 1));
    }
    let start = j;
    while let Some(&b) = bytes.get(j) {
        if b == b'$' {
            return Some((bytes[start..j].to_vec(), j + 1));
        }
        if !(b == b'_' || b.is_ascii_alphanumeric()) {
            return None;
        }
        j += 1;
    }
    None
}

/// Does `$tag$` start at `i`?
fn dollar_quote_close_matches(bytes: &[u8], i: usize, tag: &[u8]) -> bool {
    let start = i + 1;
    if bytes.len() < start + tag.len() + 1 {
        return false;
    }
    if &bytes[start..start + tag.len()] != tag {
        return false;
    }
    bytes.get(start + tag.len()) == Some(&b'$')
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn normalized(statement: &str) -> String {
        statement.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    #[test]
    fn splitter_splits_plain_statements() {
        let (statements, warnings) = split_sql_script("SELECT 1; SELECT 2;");
        assert_eq!(statements, vec!["SELECT 1", "SELECT 2"]);
        assert!(warnings.is_empty());

        let (statements, _) = split_sql_script("SELECT 1");
        assert_eq!(statements, vec!["SELECT 1"]);
    }

    #[test]
    fn splitter_ignores_semicolons_inside_quotes() {
        let script = "SELECT 'a;b'; SELECT \"c;d\"; SELECT `e;f`; SELECT [g;h];";
        let (statements, warnings) = split_sql_script(script);
        assert_eq!(
            statements,
            vec![
                "SELECT 'a;b'",
                "SELECT \"c;d\"",
                "SELECT `e;f`",
                "SELECT [g;h]",
            ]
        );
        assert!(warnings.is_empty());
    }

    #[test]
    fn splitter_handles_escaped_quotes() {
        let (statements, warnings) = split_sql_script("SELECT 'it''s; fine'; SELECT \"a\"\"; b\";");
        assert_eq!(statements.len(), 2);
        assert_eq!(statements[0], "SELECT 'it''s; fine'");
        assert_eq!(statements[1], "SELECT \"a\"\"; b\"");
        assert!(warnings.is_empty());
    }

    #[test]
    fn splitter_strips_comments() {
        let script = "-- leading; comment\nSELECT 1; # mysql; comment\nSELECT 2; /* a;b */ SELECT 3;";
        let (statements, warnings) = split_sql_script(script);
        assert_eq!(statements.len(), 3);
        assert_eq!(normalized(&statements[0]), "SELECT 1");
        assert_eq!(normalized(&statements[1]), "SELECT 2");
        assert_eq!(normalized(&statements[2]), "SELECT 3");
        assert!(warnings.is_empty());
    }

    #[test]
    fn splitter_does_not_treat_hash_tables_as_comments() {
        let (statements, warnings) = split_sql_script("CREATE TABLE #tmp (id INT); SELECT 1;");
        assert_eq!(statements.len(), 2);
        assert!(statements[0].contains("#tmp"));
        assert!(warnings.is_empty());

        // `#` comments are recognized at line start or when followed by
        // whitespace; a mid-line `#token` is kept for SQL Server `#temp`
        // tables (a deliberate trade-off).
        let (statements, _) = split_sql_script("SELECT 1;\n#trailing comment\nSELECT 2;");
        assert_eq!(statements, vec!["SELECT 1", "SELECT 2"]);

        let (statements, _) = split_sql_script("SELECT 1; # spaced comment\nSELECT 2;");
        assert_eq!(statements, vec!["SELECT 1", "SELECT 2"]);

        let (statements, _) = split_sql_script("SELECT 1; #glued\nSELECT 2;");
        assert_eq!(statements.len(), 2);
        assert!(statements[1].contains("#glued"));
    }

    #[test]
    fn splitter_handles_dollar_quoting() {
        let script = "CREATE FUNCTION f() RETURNS void AS $$ BEGIN; SELECT ';'; END $$ LANGUAGE plpgsql;\n\
                      CREATE FUNCTION g() RETURNS void AS $tag$ SELECT 1; $tag$ LANGUAGE sql;";
        let (statements, warnings) = split_sql_script(script);
        assert_eq!(statements.len(), 2);
        assert!(statements[0].contains("SELECT ';';"));
        assert!(statements[0].ends_with("LANGUAGE plpgsql"));
        assert!(statements[1].ends_with("LANGUAGE sql"));
        assert!(warnings.is_empty());
    }

    #[test]
    fn splitter_keeps_dollar_placeholders() {
        let (statements, warnings) = split_sql_script("SELECT $1; SELECT $2;");
        assert_eq!(statements, vec!["SELECT $1", "SELECT $2"]);
        assert!(warnings.is_empty());
    }

    #[test]
    fn splitter_skips_blank_and_comment_only_chunks() {
        let (statements, warnings) = split_sql_script(";\n-- just a comment;\n/* nothing */\n;\n");
        assert!(statements.is_empty());
        assert!(warnings.is_empty());
    }

    #[test]
    fn splitter_reports_unterminated_constructs() {
        let (statements, warnings) = split_sql_script("SELECT 'abc");
        assert_eq!(statements.len(), 1);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("unterminated string"));

        let (statements, warnings) = split_sql_script("SELECT 1 /* never closed");
        assert_eq!(statements.len(), 1);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("unterminated block comment"));

        let (_, warnings) = split_sql_script("CREATE FUNCTION f() AS $q$ SELECT 1");
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("dollar-quoted"));
    }

    #[test]
    fn splitter_handles_delimiter_wrapped_procedure() {
        // The Navicat-style script shape: DROP + DELIMITER $$ + CREATE
        // PROCEDURE with `;` inside the body + DELIMITER ; + CALL.
        let script = "DROP PROCEDURE IF EXISTS `p`;\n\n\
            DELIMITER $$\n\
            CREATE PROCEDURE `p`(IN a INT, OUT b LONGTEXT)\n\
            BEGIN\n\
            \tDECLARE v_created BIGINT DEFAULT DATE_FORMAT(NOW(), '%Y%m%d%H%i%s');\n\
            \tSELECT a;\n\
            END$$\n\n\
            DELIMITER ;\n\
            CALL p(1);";
        let (statements, warnings) = split_sql_script(script);
        assert!(warnings.is_empty(), "warnings: {warnings:?}");
        assert_eq!(statements.len(), 3);
        assert_eq!(statements[0], "DROP PROCEDURE IF EXISTS `p`");
        assert!(statements[1].starts_with("CREATE PROCEDURE `p`"));
        assert!(statements[1].contains("DECLARE v_created"));
        assert!(statements[1].ends_with("END"));
        assert_eq!(statements[2], "CALL p(1)");
        // The DELIMITER directives are client commands: nothing reaching the
        // server may mention them.
        assert!(
            !statements
                .iter()
                .any(|s| s.to_ascii_uppercase().contains("DELIMITER"))
        );
    }

    #[test]
    fn splitter_delimiter_is_case_insensitive_and_resets() {
        let (statements, warnings) =
            split_sql_script("delimiter //\nSELECT 1//\ndElImItEr ;\nSELECT 2;");
        assert!(warnings.is_empty(), "warnings: {warnings:?}");
        assert_eq!(statements, vec!["SELECT 1", "SELECT 2"]);
    }

    #[test]
    fn splitter_keeps_begin_end_body_whole_without_delimiter() {
        let (statements, warnings) = split_sql_script(
            "CREATE TRIGGER trg AFTER INSERT ON t BEGIN UPDATE t SET x = 1; UPDATE t SET y = 2; END;",
        );
        assert!(warnings.is_empty(), "warnings: {warnings:?}");
        assert_eq!(statements.len(), 1);
        assert!(statements[0].contains("UPDATE t SET x = 1; UPDATE t SET y = 2;"));
    }

    #[test]
    fn splitter_handles_case_end_case_inside_procedure() {
        let script = "CREATE PROCEDURE p() BEGIN \
            CASE x WHEN 1 THEN SELECT 1; ELSE SELECT 2; END CASE; \
            SELECT 3; END;";
        let (statements, warnings) = split_sql_script(script);
        assert!(warnings.is_empty(), "warnings: {warnings:?}");
        assert_eq!(statements.len(), 1);
    }

    #[test]
    fn splitter_still_splits_plain_transaction_blocks() {
        // `BEGIN;` outside a routine definition is a transaction opener, not
        // a body — every `;` still splits.
        let (statements, warnings) = split_sql_script("BEGIN; SELECT 1; COMMIT;");
        assert!(warnings.is_empty(), "warnings: {warnings:?}");
        assert_eq!(statements, vec!["BEGIN", "SELECT 1", "COMMIT"]);
    }

    #[test]
    fn splitter_treats_delimiter_word_as_ordinary_identifier() {
        let (statements, warnings) = split_sql_script("SELECT delimiter FROM t;");
        assert!(warnings.is_empty(), "warnings: {warnings:?}");
        assert_eq!(statements, vec!["SELECT delimiter FROM t"]);
    }

    #[test]
    fn splitter_warns_on_unbalanced_begin_end() {
        let (statements, warnings) = split_sql_script("CREATE PROCEDURE p() BEGIN SELECT 1;");
        assert_eq!(statements.len(), 1);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("unbalanced BEGIN..END"));
    }

    #[test]
    fn csv_with_header_maps_fields_and_nulls() {
        let input = b"id,name,note\n1,Alice,\n2,,hello\n";
        let (columns, rows) = parse_csv(&input[..], b',', true, "").unwrap();
        assert_eq!(columns, vec!["id", "name", "note"]);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0][0], Value::Text("1".into()));
        assert_eq!(rows[0][2], Value::Null); // empty field -> NULL
        assert_eq!(rows[1][1], Value::Null);
        assert_eq!(rows[1][2], Value::Text("hello".into()));
    }

    #[test]
    fn csv_without_header_synthesizes_columns() {
        let input = b"1,2\n3\n";
        let (columns, rows) = parse_csv(&input[..], b',', false, "").unwrap();
        assert_eq!(columns, vec!["c1", "c2"]);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1], vec![Value::Text("3".into()), Value::Null]);
    }

    #[test]
    fn csv_null_marker_only_matches_the_marker() {
        let input = b"a,b\nNULL,\n";
        let (_, rows) = parse_csv(&input[..], b',', true, "NULL").unwrap();
        assert_eq!(rows[0][0], Value::Null);
        // Empty field with a non-empty marker stays an empty string.
        assert_eq!(rows[0][1], Value::Text(String::new()));
    }

    #[test]
    fn csv_strips_utf8_bom_from_the_first_header() {
        let input = "\u{feff}id,name\n1,Alice\n".as_bytes();
        let (columns, _) = parse_csv(input, b',', true, "").unwrap();
        assert_eq!(columns, vec!["id", "name"]);
    }

    #[test]
    fn json_array_unions_keys_and_maps_scalars() {
        let input = br#"[{"id": 1, "ok": true, "meta": {"a": 1}}, {"id": 2, "note": "x"}]"#;
        let mut errors = Vec::new();
        let (columns, rows) = parse_json(input, &mut errors).unwrap();
        assert_eq!(columns, vec!["id", "ok", "meta", "note"]);
        assert_eq!(rows[0][0], Value::Int(1));
        assert_eq!(rows[0][1], Value::Bool(true));
        assert!(matches!(rows[0][2], Value::Json(_)));
        assert_eq!(rows[0][3], Value::Null);
        assert_eq!(rows[1][0], Value::Int(2));
        assert_eq!(rows[1][3], Value::Text("x".into()));
        assert!(errors.is_empty());
    }

    #[test]
    fn json_lines_are_accepted() {
        let input = b"{\"a\": 1}\n{\"a\": 2, \"b\": null}\n";
        let mut errors = Vec::new();
        let (columns, rows) = parse_json(input, &mut errors).unwrap();
        assert_eq!(columns, vec!["a", "b"]);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1][1], Value::Null);
    }

    #[test]
    fn json_rejects_scalars_with_a_clear_error() {
        let mut errors = Vec::new();
        let err = parse_json(b"[1, 2]", &mut errors).unwrap_err();
        assert_eq!(err.code(), ErrorCode::Data);
        assert!(err.info.message.contains("expected an object"));
    }

    #[test]
    fn placeholders_follow_the_session_style() {
        assert_eq!(placeholder(PlaceholderStyle::DollarNumbered, 1), "$1");
        assert_eq!(placeholder(PlaceholderStyle::DollarNumbered, 4), "$4");
        assert_eq!(placeholder(PlaceholderStyle::Question, 1), "?");
    }

    #[test]
    fn insert_sql_carries_all_bind_parameters() {
        let columns = vec!["\"id\"".to_string(), "\"name\"".to_string()];
        assert_eq!(
            build_insert_sql(PlaceholderStyle::Question, "\"items\"", &columns, 2),
            "INSERT INTO \"items\" (\"id\", \"name\") VALUES (?, ?), (?, ?)"
        );
        assert_eq!(
            build_insert_sql(PlaceholderStyle::DollarNumbered, "\"public\".\"items\"", &columns, 2),
            "INSERT INTO \"public\".\"items\" (\"id\", \"name\") VALUES ($1, $2), ($3, $4)"
        );
    }

    #[test]
    fn truncate_statement_is_engine_specific() {
        assert_eq!(truncate_sql(Engine::MySql, "`t`"), "TRUNCATE TABLE `t`");
        assert_eq!(truncate_sql(Engine::Postgres, "\"t\""), "TRUNCATE TABLE \"t\"");
        assert_eq!(truncate_sql(Engine::Sqlite, "\"t\""), "DELETE FROM \"t\"");
        assert_eq!(truncate_sql(Engine::SqlServer, "[t]"), "DELETE FROM [t]");
    }

    #[test]
    fn skippable_statements() {
        assert!(is_skippable_statement(""));
        assert!(is_skippable_statement("   "));
        assert!(is_skippable_statement("-- comment"));
        assert!(is_skippable_statement("# comment"));
        assert!(is_skippable_statement("/* comment */"));
        assert!(is_skippable_statement("-- c1\n/* c2 */"));
        assert!(!is_skippable_statement("-- note\nSELECT 1"));
        assert!(!is_skippable_statement("SELECT 1"));
    }

    #[test]
    fn dump_insert_literals_are_engine_aware() {
        let rows = vec![
            vec![Value::Int(1), Value::Text("O'Brien".into())],
            vec![Value::Null, Value::Text("x".into())],
        ];
        let sql = insert_literal_sql(
            Engine::Postgres,
            "\"public\".\"t\"",
            &["\"id\"".into(), "\"name\"".into()],
            &rows,
        );
        assert!(sql.starts_with("INSERT INTO \"public\".\"t\" (\"id\", \"name\") VALUES\n"));
        assert!(sql.contains("(1, 'O''Brien')"));
        assert!(sql.contains("(NULL, 'x')"));
        assert!(sql.ends_with(';'));
    }

    // -----------------------------------------------------------------------
    // End-to-end over SQLite: batch / import / grid edits / dump / split
    // -----------------------------------------------------------------------

    /// Fresh API + SQLite session in a throwaway directory.
    async fn sqlite_api(tag: &str) -> (Api, std::path::PathBuf, String) {
        use mieosql_core::{Engine as CoreEngine, SslMode};

        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let mut base = std::env::temp_dir();
        base.push(format!(
            "mieosql-maintenance-{tag}-{}-{nanos}",
            std::process::id()
        ));
        let data_dir = base.join("data");
        std::fs::create_dir_all(&data_dir).unwrap();
        let db_path = base.join("test.db");
        std::fs::write(&db_path, b"").unwrap();

        let engine = mieosql_engine::Engine::open(mieosql_engine::EngineConfig {
            data_dir: data_dir.clone(),
        })
        .unwrap();
        let api = Api::new(engine);
        let profile = api
            .save_profile(crate::dto::ProfileInput {
                id: None,
                name: "sqlite-maintenance".into(),
                engine: CoreEngine::Sqlite,
                host: None,
                port: None,
                username: None,
                database: None,
                file: Some(db_path.to_string_lossy().into_owned()),
                params: Default::default(),
                ssl_mode: Some(SslMode::Disable),
                ssh: None,
                color: None,
                group: None,
                variant: None,
                is_prod: false,
                password: None,
                ssh_password: None,
            })
            .unwrap();
        let session = api.open_session(profile.id.0.clone()).await.unwrap();
        (api, base, session.id)
    }

    async fn scalar(api: &Api, session_id: &str, sql: &str) -> String {
        let out = api
            .collect_query(
                session_id,
                sql.to_string(),
                Vec::new(),
                QueryMode::Prepared,
                None,
            )
            .await
            .unwrap();
        out.rows
            .first()
            .and_then(|row| row.first())
            .map(crate::export::cell_text)
            .unwrap_or_default()
    }

    fn cell(column: &str, value: Value) -> GridCell {
        GridCell {
            column: column.to_string(),
            value,
        }
    }

    fn csv_options(batch_size: Option<u32>) -> ImportOptions {
        ImportOptions {
            delimiter: None,
            has_header: true,
            null_marker: String::new(),
            mode: ImportMode::Append,
            batch_size,
        }
    }

    #[tokio::test]
    async fn split_text_then_batch_runs_routine_body_on_sqlite() {
        // The editor multi-statement path: split text, then run the parts
        // sequentially on the simple execution path. The trigger body keeps
        // its inner `;` and fires as one unit.
        let (api, _base, session_id) = sqlite_api("routine").await;
        let text = "CREATE TABLE t (id INTEGER PRIMARY KEY, x INT);\n\
            CREATE TABLE log (n INT);\n\
            CREATE TRIGGER trg AFTER INSERT ON t BEGIN INSERT INTO log VALUES (1); INSERT INTO log VALUES (2); END;";
        let split = api.split_sql_text(text.to_string());
        assert!(split.warnings.is_empty(), "warnings: {:?}", split.warnings);
        assert_eq!(split.statements.len(), 3);
        let result = api
            .execute_batch(session_id.clone(), split.statements, false)
            .await
            .unwrap();
        assert_eq!(result.failed, 0);
        assert_eq!(result.applied, 3);
        api.execute_batch(
            session_id.clone(),
            vec!["INSERT INTO t (id, x) VALUES (1, 0)".into()],
            false,
        )
        .await
        .unwrap();
        assert_eq!(scalar(&api, &session_id, "SELECT COUNT(*) FROM log").await, "2");
    }

    #[tokio::test]
    async fn maintenance_commands_end_to_end_on_sqlite() {
        let (api, base, session_id) = sqlite_api("e2e").await;
        let scope = Scope::default();

        // --- execute_batch: DDL + DML, blank/comment statements skipped ------
        let result = api
            .execute_batch(
                session_id.clone(),
                vec![
                    "CREATE TABLE items (id INTEGER PRIMARY KEY, name TEXT)".into(),
                    "".into(),
                    "-- comment".into(),
                    "INSERT INTO items (id, name) VALUES (1, 'a');".into(),
                    "INSERT INTO items (id, name) VALUES (2, 'b')".into(),
                ],
                false,
            )
            .await
            .unwrap();
        assert_eq!(result.applied, 3);
        assert_eq!(result.failed, 0);
        assert_eq!(
            scalar(&api, &session_id, "SELECT COUNT(*) FROM items").await,
            "2"
        );

        // --- execute_batch atomic: rollback on the first error ---------------
        let result = api
            .execute_batch(
                session_id.clone(),
                vec![
                    "INSERT INTO items (id, name) VALUES (3, 'c')".into(),
                    "INSERT INTO missing_table (id) VALUES (1)".into(),
                ],
                true,
            )
            .await
            .unwrap();
        assert_eq!(result.applied, 1);
        assert_eq!(result.failed, 1);
        assert!(result.errors[0].contains("statement #2"));
        assert_eq!(
            scalar(&api, &session_id, "SELECT COUNT(*) FROM items").await,
            "2"
        );

        // --- import_file: CSV, one row per batch, empty field -> NULL --------
        let csv_path = base.join("import.csv");
        std::fs::write(&csv_path, "id,name\n10,alpha\n11,\n12,gamma\n").unwrap();
        let summary = api
            .import_file(
                session_id.clone(),
                scope.clone(),
                "items".into(),
                csv_path.to_string_lossy().into_owned(),
                ImportFormat::Csv,
                csv_options(Some(1)),
            )
            .await
            .unwrap();
        assert_eq!(summary.rows_read, 3);
        assert_eq!(summary.rows_inserted, 3);
        assert_eq!(summary.rows_failed, 0);
        assert!(summary.errors.is_empty());
        assert_eq!(
            scalar(&api, &session_id, "SELECT COUNT(*) FROM items").await,
            "5"
        );
        assert_eq!(
            scalar(
                &api,
                &session_id,
                "SELECT COUNT(*) FROM items WHERE id = 11 AND name IS NULL"
            )
            .await,
            "1"
        );

        // --- import_file: JSON lines with truncate ---------------------------
        let json_path = base.join("import.jsonl");
        std::fs::write(
            &json_path,
            "{\"id\": 1, \"name\": \"one\"}\n{\"id\": 2, \"name\": \"two\"}\n",
        )
        .unwrap();
        let summary = api
            .import_file(
                session_id.clone(),
                scope.clone(),
                "items".into(),
                json_path.to_string_lossy().into_owned(),
                ImportFormat::Json,
                ImportOptions {
                    delimiter: None,
                    has_header: false,
                    null_marker: String::new(),
                    mode: ImportMode::Truncate,
                    batch_size: None,
                },
            )
            .await
            .unwrap();
        assert_eq!(summary.rows_read, 2);
        assert_eq!(summary.rows_inserted, 2);
        assert_eq!(
            scalar(&api, &session_id, "SELECT COUNT(*) FROM items").await,
            "2"
        );

        // --- apply_grid_edits: insert + update + delete in one run ----------
        let result = api
            .apply_grid_edits(GridEditRequest {
                session_id: session_id.clone(),
                scope: scope.clone(),
                table: "items".into(),
                inserts: vec![vec![
                    cell("id", Value::Int(30)),
                    cell("name", Value::Text("fresh".into())),
                ]],
                updates: vec![RowChanges {
                    keys: vec![cell("id", Value::Int(1))],
                    changes: vec![cell("name", Value::Text("updated".into()))],
                }],
                deletes: vec![vec![cell("id", Value::Int(2))]],
            })
            .await
            .unwrap();
        assert_eq!(
            (result.inserted, result.updated, result.deleted, result.failed),
            (1, 1, 1, 0)
        );
        assert_eq!(
            scalar(&api, &session_id, "SELECT name FROM items WHERE id = 1").await,
            "updated"
        );
        assert_eq!(
            scalar(&api, &session_id, "SELECT COUNT(*) FROM items").await,
            "2"
        );

        // Updates without keys are rejected before anything runs.
        let err = api
            .apply_grid_edits(GridEditRequest {
                session_id: session_id.clone(),
                scope: scope.clone(),
                table: "items".into(),
                inserts: Vec::new(),
                updates: vec![RowChanges {
                    keys: Vec::new(),
                    changes: vec![cell("name", Value::Text("x".into()))],
                }],
                deletes: Vec::new(),
            })
            .await
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::Data);

        // A failing statement rolls the whole grid edit back.
        let result = api
            .apply_grid_edits(GridEditRequest {
                session_id: session_id.clone(),
                scope: scope.clone(),
                table: "items".into(),
                inserts: vec![
                    vec![
                        cell("id", Value::Int(40)),
                        cell("name", Value::Text("ok".into())),
                    ],
                    // primary-key violation on the second insert
                    vec![
                        cell("id", Value::Int(1)),
                        cell("name", Value::Text("dup".into())),
                    ],
                ],
                updates: Vec::new(),
                deletes: Vec::new(),
            })
            .await
            .unwrap();
        assert_eq!(result.failed, 1);
        assert!(result.errors[0].contains("insert #2"));
        assert_eq!(
            scalar(
                &api,
                &session_id,
                "SELECT COUNT(*) FROM items WHERE id = 40"
            )
            .await,
            "0"
        );

        // --- dump_backup: DDL + data -----------------------------------------
        let dump_path = base.join("backup.sql");
        let summary = api
            .dump_backup(
                session_id.clone(),
                scope.clone(),
                dump_path.to_string_lossy().into_owned(),
                true,
                None,
            )
            .await
            .unwrap();
        assert!(summary.tables >= 1);
        assert!(summary.rows >= 2);
        assert!(summary.bytes > 0);
        assert!(summary.errors.is_empty());
        let dump = std::fs::read_to_string(&dump_path).unwrap();
        assert!(dump.contains("-- MieoSQL backup"));
        assert!(dump.contains("CREATE TABLE"));
        assert!(dump.contains("INSERT INTO"));
        assert!(dump.contains("updated"));

        // Table filter that matches nothing still dumps zero tables.
        let summary = api
            .dump_backup(
                session_id.clone(),
                scope.clone(),
                base.join("none.sql").to_string_lossy().into_owned(),
                false,
                Some(vec!["nope".into()]),
            )
            .await
            .unwrap();
        assert_eq!(summary.tables, 0);
        assert!(summary.errors[0].contains("nope"));

        // --- script_statements ------------------------------------------------
        let script_path = base.join("script.sql");
        std::fs::write(
            &script_path,
            "CREATE TABLE t (id INT);\nINSERT INTO t VALUES (1);\n-- tail comment\n",
        )
        .unwrap();
        let script = api
            .script_statements(script_path.to_string_lossy().into_owned())
            .unwrap();
        assert_eq!(script.statements.len(), 2);
        assert!(script.warnings.is_empty());

        let _ = std::fs::remove_dir_all(&base);
    }

    #[tokio::test]
    async fn import_stops_at_the_first_failed_batch() {
        let (api, base, session_id) = sqlite_api("import-fail").await;
        api.execute_batch(
            session_id.clone(),
            vec!["CREATE TABLE items (id INTEGER PRIMARY KEY, name TEXT)".into()],
            false,
        )
        .await
        .unwrap();

        // Second row violates the primary key.
        let csv_path = base.join("bad.csv");
        std::fs::write(&csv_path, "id,name\n1,ok\n1,dup\n2,later\n").unwrap();
        let summary = api
            .import_file(
                session_id.clone(),
                Scope::default(),
                "items".into(),
                csv_path.to_string_lossy().into_owned(),
                ImportFormat::Csv,
                csv_options(Some(2)),
            )
            .await
            .unwrap();
        assert_eq!(summary.rows_read, 3);
        assert_eq!(summary.rows_inserted, 0);
        assert_eq!(summary.rows_failed, 2);
        assert_eq!(summary.errors.len(), 1);
        assert_eq!(
            scalar(&api, &session_id, "SELECT COUNT(*) FROM items").await,
            "0"
        );

        let _ = std::fs::remove_dir_all(&base);
    }
}
