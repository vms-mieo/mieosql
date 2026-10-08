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
//!
//! Implementation notes:
//! - Sessions serialize statements (one driver behind a mutex), so every plan
//!   is built sequentially.
//! - Row identity is the textual key (`cell_text` joined with `\u{1f}`), which
//!   keeps cross-engine comparisons dialect independent. A value containing the
//!   separator character itself is a theoretical collision (same as NULL vs
//!   empty string) — acceptable for v1.
//! - Only executable SQL becomes a statement; anything the target engine
//!   cannot do (e.g. SQLite column type changes) is skipped and reported in
//!   `warnings` so the plan stays honest.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::time::Instant;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use mieosql_core::{
    ColumnDef, DataTypeKind, Engine, ErrorCode, ForeignKeyDef, IndexDef, MieoError, ObjectKind,
    QueryMode, Scope, Value,
};

use crate::dto::ApiResult;
use crate::export::{cell_text, sql_literal};
use crate::service::{quote_ident, select_limited, Api};

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
    /// Number of changes not applied because the statement list was capped
    /// (1+ when the sample itself was truncated).
    #[ts(type = "number")]
    pub truncated: u64,
}

impl Api {
    /// Introspect both endpoints and build the structure sync plan.
    pub async fn structure_sync_plan(
        &self,
        request: StructureSyncRequest,
    ) -> ApiResult<StructureSyncPlan> {
        let source_id = request.source.session_id.clone();
        let target_id = request.target.session_id.clone();
        let src_info = self.session_info(source_id.clone()).await?;
        let dst_info = self.session_info(target_id.clone()).await?;
        let src_engine = src_info.engine;
        let dst_engine = dst_info.engine;
        let same_engine = src_engine == dst_engine;

        let mut warnings: Vec<String> = Vec::new();
        if !same_engine {
            warnings.push(format!(
                "source is {} and target is {}; the generated script is best-effort",
                src_engine.label(),
                dst_engine.label()
            ));
        }

        // `tables` filter is case-insensitive; empty = every table.
        let filter: Option<HashSet<String>> = if request.tables.is_empty() {
            None
        } else {
            Some(
                request
                    .tables
                    .iter()
                    .map(|t| t.trim().to_lowercase())
                    .filter(|t| !t.is_empty())
                    .collect(),
            )
        };
        let keep = |name: &str| {
            filter
                .as_ref()
                .map_or(true, |f| f.contains(&name.to_lowercase()))
        };

        let src_tables = self
            .list_objects(
                source_id.clone(),
                request.source.scope.clone(),
                Some(ObjectKind::Table),
                None,
            )
            .await?;
        let dst_tables = self
            .list_objects(
                target_id.clone(),
                request.target.scope.clone(),
                Some(ObjectKind::Table),
                None,
            )
            .await?;

        // lower-case name -> name as reported by the engine.
        let src_map: BTreeMap<String, String> = src_tables
            .iter()
            .filter(|t| keep(&t.name))
            .map(|t| (t.name.to_lowercase(), t.name.clone()))
            .collect();
        let dst_map: BTreeMap<String, String> = dst_tables
            .iter()
            .filter(|t| keep(&t.name))
            .map(|t| (t.name.to_lowercase(), t.name.clone()))
            .collect();

        let mut names: BTreeSet<String> = src_map.keys().cloned().collect();
        names.extend(dst_map.keys().cloned());

        let mut diffs: Vec<TableDiff> = Vec::new();
        let mut buckets = StatementBuckets::default();

        for key in names {
            match (src_map.get(&key), dst_map.get(&key)) {
                (Some(src_name), None) => {
                    let columns = self
                        .table_columns(
                            source_id.clone(),
                            request.source.scope.clone(),
                            src_name.clone(),
                        )
                        .await?;
                    let column_diffs: Vec<ColumnDiff> = columns
                        .iter()
                        .map(|c| ColumnDiff {
                            name: c.name.clone(),
                            source_type: Some(c.native_type.clone()),
                            target_type: None,
                            change: DiffChange::Added,
                        })
                        .collect();

                    let ddl = self
                        .object_ddl(
                            source_id.clone(),
                            request.source.scope.clone(),
                            ObjectKind::Table,
                            src_name.clone(),
                        )
                        .await?;
                    let sql = match ddl.filter(|d| !d.trim().is_empty()) {
                        Some(ddl) => qualify_create_table(
                            &trim_statement(ddl),
                            dst_engine,
                            &request.target.scope,
                        ),
                        None => {
                            if same_engine {
                                warnings.push(format!(
                                    "DDL for `{src_name}` is unavailable; generated from column \
                                     metadata (indexes and foreign keys are not included)"
                                ));
                            }
                            if columns.iter().any(|c| c.is_generated) {
                                warnings.push(format!(
                                    "table `{src_name}` has generated columns; they are created \
                                     as plain columns in the generated DDL"
                                ));
                            }
                            generated_create_table_sql(
                                dst_engine,
                                &request.target.scope,
                                src_name,
                                &columns,
                                same_engine,
                            )
                        }
                    };
                    buckets.create_tables.push(SyncStatement {
                        table: src_name.clone(),
                        sql,
                        kind: SyncStatementKind::CreateTable,
                        destructive: false,
                    });
                    diffs.push(TableDiff {
                        table: src_name.clone(),
                        status: DiffStatus::Added,
                        columns: column_diffs,
                        indexes_added: Vec::new(),
                        indexes_removed: Vec::new(),
                        fk_report: Vec::new(),
                    });
                }
                (None, Some(tgt_name)) => {
                    let columns = self
                        .table_columns(
                            target_id.clone(),
                            request.target.scope.clone(),
                            tgt_name.clone(),
                        )
                        .await?;
                    let column_diffs: Vec<ColumnDiff> = columns
                        .iter()
                        .map(|c| ColumnDiff {
                            name: c.name.clone(),
                            source_type: None,
                            target_type: Some(c.native_type.clone()),
                            change: DiffChange::Removed,
                        })
                        .collect();
                    buckets.drop_tables.push(SyncStatement {
                        table: tgt_name.clone(),
                        sql: format!(
                            "DROP TABLE {}",
                            qualified_table(dst_engine, &request.target.scope, tgt_name)
                        ),
                        kind: SyncStatementKind::DropTable,
                        destructive: true,
                    });
                    diffs.push(TableDiff {
                        table: tgt_name.clone(),
                        status: DiffStatus::Removed,
                        columns: column_diffs,
                        indexes_added: Vec::new(),
                        indexes_removed: Vec::new(),
                        fk_report: Vec::new(),
                    });
                }
                (Some(src_name), Some(tgt_name)) => {
                    let src_cols = self
                        .table_columns(
                            source_id.clone(),
                            request.source.scope.clone(),
                            src_name.clone(),
                        )
                        .await?;
                    let tgt_cols = self
                        .table_columns(
                            target_id.clone(),
                            request.target.scope.clone(),
                            tgt_name.clone(),
                        )
                        .await?;

                    let column_diffs = diff_columns(&src_cols, &tgt_cols);
                    for diff in &column_diffs {
                        match diff.change {
                            DiffChange::Added => {
                                let Some(col) =
                                    src_cols.iter().find(|c| c.name == diff.name)
                                else {
                                    continue;
                                };
                                if !col.nullable {
                                    warnings.push(format!(
                                        "column `{tgt_name}.{}` is NOT NULL; adding it to a \
                                         non-empty table fails without a default",
                                        col.name
                                    ));
                                }
                                buckets.add_columns.push(SyncStatement {
                                    table: tgt_name.clone(),
                                    sql: add_column_sql(
                                        dst_engine,
                                        &request.target.scope,
                                        tgt_name,
                                        col,
                                        same_engine,
                                    ),
                                    kind: SyncStatementKind::AddColumn,
                                    destructive: false,
                                });
                            }
                            DiffChange::Removed => {
                                buckets.drop_columns.push(SyncStatement {
                                    table: tgt_name.clone(),
                                    sql: format!(
                                        "ALTER TABLE {} DROP COLUMN {}",
                                        qualified_table(
                                            dst_engine,
                                            &request.target.scope,
                                            tgt_name
                                        ),
                                        quote_ident(dst_engine, &diff.name)
                                    ),
                                    kind: SyncStatementKind::DropColumn,
                                    destructive: true,
                                });
                            }
                            DiffChange::Modified => {
                                let Some(col) =
                                    src_cols.iter().find(|c| c.name == diff.name)
                                else {
                                    continue;
                                };
                                match alter_column_sql(
                                    dst_engine,
                                    &request.target.scope,
                                    tgt_name,
                                    col,
                                    same_engine,
                                ) {
                                    Some(sql) => buckets.alter_columns.push(SyncStatement {
                                        table: tgt_name.clone(),
                                        sql,
                                        kind: SyncStatementKind::AlterColumn,
                                        destructive: false,
                                    }),
                                    None => warnings.push(format!(
                                        "column `{tgt_name}.{}` differs but {} cannot alter \
                                         column types; skipped",
                                        col.name,
                                        dst_engine.label()
                                    )),
                                }
                            }
                        }
                    }

                    let mut indexes_added: Vec<IndexDef> = Vec::new();
                    let mut indexes_removed: Vec<IndexDef> = Vec::new();
                    if request.include_indexes {
                        let src_idx = self
                            .table_indexes(
                                source_id.clone(),
                                request.source.scope.clone(),
                                src_name.clone(),
                            )
                            .await?;
                        let tgt_idx = self
                            .table_indexes(
                                target_id.clone(),
                                request.target.scope.clone(),
                                tgt_name.clone(),
                            )
                            .await?;

                        // PostgreSQL constraint-backed indexes cannot be dropped directly.
                        let non_droppable: HashSet<String> =
                            if dst_engine == Engine::Postgres {
                                match self
                                    .pg_constraint_index_names(
                                        &target_id,
                                        &request.target.scope,
                                        tgt_name,
                                    )
                                    .await
                                {
                                    Ok(names) => names.into_iter().collect(),
                                    Err(e) => {
                                        warnings.push(format!(
                                            "could not inspect PostgreSQL constraints on \
                                             `{tgt_name}`: {e}; constraint-backed indexes may be \
                                             skipped conservatively"
                                        ));
                                        HashSet::new()
                                    }
                                }
                            } else {
                                HashSet::new()
                            };

                        let index_diff = diff_indexes(
                            tgt_name,
                            &src_idx,
                            &tgt_idx,
                            dst_engine,
                            &request.target.scope,
                            &non_droppable,
                        );
                        indexes_added = index_diff.added;
                        indexes_removed = index_diff.removed;
                        buckets.add_indexes.extend(index_diff.add_statements);
                        buckets.drop_indexes.extend(index_diff.drop_statements);
                        warnings.extend(index_diff.warnings);
                    }

                    let src_fks = self
                        .foreign_keys(
                            source_id.clone(),
                            request.source.scope.clone(),
                            src_name.clone(),
                        )
                        .await?;
                    let tgt_fks = self
                        .foreign_keys(
                            target_id.clone(),
                            request.target.scope.clone(),
                            tgt_name.clone(),
                        )
                        .await?;
                    let fk_report = fk_diff_lines(&src_fks, &tgt_fks);

                    let status = if column_diffs.is_empty()
                        && indexes_added.is_empty()
                        && indexes_removed.is_empty()
                    {
                        DiffStatus::Same
                    } else {
                        DiffStatus::Changed
                    };
                    diffs.push(TableDiff {
                        table: src_name.clone(),
                        status,
                        columns: column_diffs,
                        indexes_added,
                        indexes_removed,
                        fk_report,
                    });
                }
                (None, None) => {}
            }
        }

        // A CREATE TABLE that embeds a foreign key must not run before its
        // parent table exists (MySQL 1824, PostgreSQL 42P01, …). Order the
        // created tables parents-first and warn about parents that are neither
        // synced nor already present on the target.
        {
            let mut deps: HashMap<String, Vec<String>> = HashMap::new();
            for stmt in &buckets.create_tables {
                let refs = self
                    .foreign_keys(
                        source_id.clone(),
                        request.source.scope.clone(),
                        stmt.table.clone(),
                    )
                    .await
                    .unwrap_or_default();
                deps.insert(
                    stmt.table.to_lowercase(),
                    refs.iter().map(|f| f.ref_table.to_lowercase()).collect(),
                );
            }
            sort_create_tables_by_deps(&mut buckets.create_tables, &deps);
            for stmt in &buckets.create_tables {
                let Some(refs) = deps.get(&stmt.table.to_lowercase()) else {
                    continue;
                };
                for dep in refs {
                    let created_here = buckets
                        .create_tables
                        .iter()
                        .any(|s| s.table.to_lowercase() == *dep);
                    let on_target = dst_map.contains_key(dep);
                    if !created_here && !on_target {
                        warnings.push(format!(
                            "`{}` has a foreign key to `{}`, but that table is neither \
                             included in this sync nor present on the target — sync it first, \
                             otherwise the CREATE TABLE will fail",
                            stmt.table, dep
                        ));
                    }
                }
            }
        }

        // Dropping runs children-first for the same FK reason (a parent that
        // is still referenced by a table outside this plan will fail at apply
        // time and surface the driver error).
        if !buckets.drop_tables.is_empty() {
            let mut drop_deps: HashMap<String, Vec<String>> = HashMap::new();
            for stmt in &buckets.drop_tables {
                let refs = self
                    .foreign_keys(
                        target_id.clone(),
                        request.target.scope.clone(),
                        stmt.table.clone(),
                    )
                    .await
                    .unwrap_or_default();
                drop_deps.insert(
                    stmt.table.to_lowercase(),
                    refs.iter().map(|f| f.ref_table.to_lowercase()).collect(),
                );
            }
            // Sort parents-first, then reverse → children are dropped first.
            sort_create_tables_by_deps(&mut buckets.drop_tables, &drop_deps);
            buckets.drop_tables.reverse();
        }

        let statements = buckets.ordered();
        let summary = SyncSummary {
            tables_added: diffs
                .iter()
                .filter(|d| d.status == DiffStatus::Added)
                .count() as u64,
            tables_removed: diffs
                .iter()
                .filter(|d| d.status == DiffStatus::Removed)
                .count() as u64,
            tables_changed: diffs
                .iter()
                .filter(|d| d.status == DiffStatus::Changed)
                .count() as u64,
            statements: statements.len() as u64,
        };

        Ok(StructureSyncPlan {
            diffs,
            statements,
            summary,
            same_engine,
            warnings: dedupe_warnings(warnings),
        })
    }

    /// Apply a list of statements (from `structure_sync_plan`) on a session.
    pub async fn apply_sync_statements(
        &self,
        session_id: String,
        statements: Vec<String>,
    ) -> ApiResult<SyncApplyResult> {
        if statements.iter().all(|s| is_skippable_statement(s)) {
            return Ok(SyncApplyResult {
                applied: 0,
                failed: 0,
                errors: Vec::new(),
                elapsed_ms: 0,
            });
        }
        self.execute_sync_statements(&session_id, &statements).await
    }

    /// Diff table rows by key and generate the sync statements preview.
    pub async fn data_sync_plan(&self, request: DataSyncRequest) -> ApiResult<DataSyncPlan> {
        self.build_data_sync_plan(&request).await
    }

    /// Recompute the data diff and apply it on the target.
    pub async fn apply_data_sync(
        &self,
        request: DataSyncRequest,
    ) -> ApiResult<DataSyncApplyResult> {
        let plan = self.build_data_sync_plan(&request).await?;
        if plan.statements.is_empty() {
            // Nothing to do — do not open a transaction on the target.
            return Ok(DataSyncApplyResult {
                inserted: 0,
                updated: 0,
                deleted: 0,
                failed: 0,
                errors: Vec::new(),
                elapsed_ms: 0,
                truncated: u64::from(plan.summary.truncated),
            });
        }
        let sqls: Vec<String> = plan.statements.iter().map(|s| s.sql.clone()).collect();
        let run = self
            .execute_sync_statements(&request.target.session_id, &sqls)
            .await?;

        // Statements are ordered insert -> update -> delete and none is
        // skippable, so the applied prefix maps 1:1 by kind.
        let mut inserted = 0u64;
        let mut updated = 0u64;
        let mut deleted = 0u64;
        for stmt in plan.statements.iter().take(run.applied as usize) {
            match stmt.kind {
                SyncStatementKind::Insert => inserted += 1,
                SyncStatementKind::Update => updated += 1,
                SyncStatementKind::Delete => deleted += 1,
                _ => {}
            }
        }

        let changed =
            plan.summary.to_insert + plan.summary.to_update + plan.summary.to_delete;
        let omitted = changed.saturating_sub(plan.statements.len() as u64);
        let truncated = if plan.summary.truncated && omitted == 0 {
            1
        } else {
            omitted
        };

        Ok(DataSyncApplyResult {
            inserted,
            updated,
            deleted,
            failed: run.failed,
            errors: run.errors,
            elapsed_ms: run.elapsed_ms,
            truncated,
        })
    }

    /// Core data diff, shared by `data_sync_plan` and `apply_data_sync`.
    async fn build_data_sync_plan(
        &self,
        request: &DataSyncRequest,
    ) -> ApiResult<DataSyncPlan> {
        let table = request.table.trim().to_string();
        if table.is_empty() {
            return Err(MieoError::new(ErrorCode::Data, "table name is required").into());
        }

        let source_id = request.source.session_id.clone();
        let target_id = request.target.session_id.clone();
        let src_info = self.session_info(source_id.clone()).await?;
        let dst_info = self.session_info(target_id.clone()).await?;
        let src_engine = src_info.engine;
        let dst_engine = dst_info.engine;

        let src_cols = self
            .table_columns(source_id.clone(), request.source.scope.clone(), table.clone())
            .await?;
        if src_cols.is_empty() {
            return Err(MieoError::new(
                ErrorCode::NotFound,
                format!("table `{table}` not found on the source"),
            )
            .with_hint("check the endpoint scope and the table name")
            .into());
        }
        let tgt_cols = self
            .table_columns(target_id.clone(), request.target.scope.clone(), table.clone())
            .await?;
        if tgt_cols.is_empty() {
            return Err(MieoError::new(
                ErrorCode::NotFound,
                format!("table `{table}` not found on the target"),
            )
            .with_hint("check the endpoint scope and the table name")
            .into());
        }

        let mut warnings: Vec<String> = Vec::new();
        let columns = resolve_data_columns(
            &src_cols,
            &tgt_cols,
            &request.key_columns,
            &request.compare_columns,
            &mut warnings,
        )?;

        let sample_limit = request.sample_limit.clamp(1, MAX_SAMPLE_LIMIT);
        if sample_limit != request.sample_limit {
            warnings.push(format!(
                "sample_limit {} was clamped to {sample_limit}",
                request.sample_limit
            ));
        }
        let max_statements = (request.max_statements.max(1) as usize).min(MAX_STATEMENTS_CAP);
        if max_statements != request.max_statements as usize {
            warnings.push(format!(
                "max_statements {} was clamped to {max_statements}",
                request.max_statements
            ));
        }

        // Read one extra row per side to detect truncation.
        let query_limit = sample_limit.saturating_add(1);
        let src_fetched = self
            .fetch_sync_rows(
                &source_id,
                src_engine,
                &request.source.scope,
                &table,
                &columns.insert,
                &columns.keys,
                query_limit,
            )
            .await?;
        let target_select = unique_names(&columns.keys_target, &columns.compare_target);
        let tgt_fetched = self
            .fetch_sync_rows(
                &target_id,
                dst_engine,
                &request.target.scope,
                &table,
                &target_select,
                &columns.keys_target,
                query_limit,
            )
            .await?;

        let (src_used, src_truncated) =
            take_sample_len(src_fetched.rows.len(), sample_limit);
        let (tgt_used, tgt_truncated) =
            take_sample_len(tgt_fetched.rows.len(), sample_limit);
        let sample_truncated = src_truncated || tgt_truncated;

        let key_positions = positions_in(&columns.insert, &columns.keys)?;
        let compare_positions = positions_in(&columns.insert, &columns.compare)?;
        let target_key_positions = positions_in(&target_select, &columns.keys_target)?;
        let target_compare_positions = positions_in(&target_select, &columns.compare_target)?;

        let (src_map, src_dups) = build_source_states(
            &src_fetched,
            &key_positions,
            &compare_positions,
            src_used,
        )?;
        let (tgt_map, tgt_dups) = build_target_states(
            &tgt_fetched,
            &target_key_positions,
            &target_compare_positions,
            tgt_used,
        )?;
        if src_dups > 0 {
            warnings.push(format!(
                "source rows contain {src_dups} duplicate key value(s); the last row wins"
            ));
        }
        if tgt_dups > 0 {
            warnings.push(format!(
                "target rows contain {tgt_dups} duplicate key value(s); the last row wins"
            ));
        }

        let diff = diff_rows(src_map, tgt_map, max_statements, request.full_scan);

        if sample_truncated {
            warnings.push(format!(
                "sample limit reached ({sample_limit} rows per side); the diff is partial"
            ));
        }
        if diff.scan_truncated {
            warnings.push(format!(
                "scan stopped at max_statements = {max_statements}; counts are a lower bound \
                 (set full_scan to count every sampled row)"
            ));
        } else if diff.statements_truncated {
            warnings.push(format!(
                "statement list capped at max_statements = {max_statements}; remaining changes \
                 were counted but not generated"
            ));
        }

        let mut unsupported = 0u64;
        let mut statements: Vec<SyncStatement> = Vec::new();
        for row in &diff.inserts {
            unsupported += count_unsupported(&row.full_values);
            statements.push(SyncStatement {
                table: table.clone(),
                sql: insert_row_sql(
                    dst_engine,
                    &request.target.scope,
                    &table,
                    &columns.insert_target,
                    &row.full_values,
                ),
                kind: SyncStatementKind::Insert,
                destructive: false,
            });
        }
        for row in &diff.updates {
            unsupported += count_unsupported(&row.key_values);
            unsupported += count_unsupported(&row.compare_values);
            statements.push(SyncStatement {
                table: table.clone(),
                sql: update_row_sql(
                    dst_engine,
                    &request.target.scope,
                    &table,
                    &columns.compare_target,
                    &row.compare_values,
                    &columns.keys_target,
                    &row.key_values,
                ),
                kind: SyncStatementKind::Update,
                destructive: true,
            });
        }
        for row in &diff.deletes {
            unsupported += count_unsupported(&row.key_values);
            statements.push(SyncStatement {
                table: table.clone(),
                sql: delete_row_sql(
                    dst_engine,
                    &request.target.scope,
                    &table,
                    &columns.keys_target,
                    &row.key_values,
                ),
                kind: SyncStatementKind::Delete,
                destructive: true,
            });
        }
        if unsupported > 0 {
            warnings.push(format!(
                "{unsupported} value(s) could not be decoded and were written as NULL"
            ));
        }

        let summary = DataDiffSummary {
            source_rows: src_used as u64,
            target_rows: tgt_used as u64,
            to_insert: diff.insert_count,
            to_update: diff.update_count,
            to_delete: diff.delete_count,
            unchanged: diff.unchanged,
            truncated: sample_truncated || diff.statements_truncated,
        };

        Ok(DataSyncPlan {
            summary,
            statements,
            key_columns: columns.keys,
            warnings: dedupe_warnings(warnings),
        })
    }

    /// Fetch `select` from `table` ordered by `order_by` (limited).
    ///
    /// Uses prepared mode with engine-quoted identifiers; the LIMIT is a
    /// literal integer (always safe, since it is clamped by the caller).
    async fn fetch_sync_rows(
        &self,
        session_id: &str,
        engine: Engine,
        scope: &Scope,
        table: &str,
        select: &[String],
        order_by: &[String],
        limit: u64,
    ) -> ApiResult<FetchedRows> {
        if select.is_empty() {
            return Err(MieoError::new(
                ErrorCode::Data,
                format!("no columns available to read from `{table}`"),
            )
            .into());
        }
        if order_by.is_empty() {
            return Err(MieoError::new(
                ErrorCode::Data,
                "no key columns to order the rows by",
            )
            .into());
        }
        let cols = select
            .iter()
            .map(|c| quote_ident(engine, c))
            .collect::<Vec<_>>()
            .join(", ");
        let order = order_by
            .iter()
            .map(|c| quote_ident(engine, c))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = select_limited(
            engine,
            &cols,
            &format!(
                "FROM {} ORDER BY {order}",
                qualified_table(engine, scope, table)
            ),
            limit,
        );
        let out = self
            .collect_query(session_id, sql, Vec::new(), QueryMode::Prepared, Some(limit))
            .await?;
        Ok(FetchedRows { rows: out.rows })
    }

    /// Execute statements in order, wrapping them in a transaction we own.
    ///
    /// Stops on the first error. When the session was already inside a
    /// transaction the caller keeps ownership (no commit/rollback here);
    /// otherwise a rollback undoes a partial run.
    async fn execute_sync_statements(
        &self,
        session_id: &str,
        statements: &[String],
    ) -> ApiResult<SyncApplyResult> {
        let started = Instant::now();
        let info = self.session_info(session_id.to_string()).await?;
        let began = !info.in_transaction;
        if began {
            self.begin_transaction(session_id.to_string()).await?;
        }

        let mut applied = 0u64;
        for (i, raw) in statements.iter().enumerate() {
            if is_skippable_statement(raw) {
                continue;
            }
            let sql = raw.trim();
            match self.execute(session_id, sql.to_string()).await {
                Ok(_) => applied += 1,
                Err(e) => {
                    if began {
                        let _ = self.rollback_transaction(session_id.to_string()).await;
                    }
                    return Ok(SyncApplyResult {
                        applied,
                        failed: 1,
                        errors: vec![format!(
                            "statement #{} failed: {e} (sql: {})",
                            i + 1,
                            short_sql(sql)
                        )],
                        elapsed_ms: started.elapsed().as_millis() as u64,
                    });
                }
            }
        }

        if began {
            if let Err(e) = self.commit_transaction(session_id.to_string()).await {
                let _ = self.rollback_transaction(session_id.to_string()).await;
                return Err(MieoError::new(
                    ErrorCode::Internal,
                    format!("transaction commit failed: {e}"),
                )
                .into());
            }
        }

        Ok(SyncApplyResult {
            applied,
            failed: 0,
            errors: Vec::new(),
            elapsed_ms: started.elapsed().as_millis() as u64,
        })
    }

    /// Index names on `table` that are backed by a constraint (PG only).
    async fn pg_constraint_index_names(
        &self,
        session_id: &str,
        scope: &Scope,
        table: &str,
    ) -> ApiResult<Vec<String>> {
        let schema = scope
            .schema
            .clone()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "public".to_string());
        let sql = "SELECT i.relname FROM pg_constraint con \
                   JOIN pg_class i ON i.oid = con.conindid \
                   JOIN pg_class t ON t.oid = con.conrelid \
                   JOIN pg_namespace n ON n.oid = t.relnamespace \
                   WHERE n.nspname = $1 AND t.relname = $2"
            .to_string();
        let out = self
            .collect_query(
                session_id,
                sql,
                vec![Value::Text(schema), Value::Text(table.to_string())],
                QueryMode::Prepared,
                None,
            )
            .await?;
        Ok(out
            .rows
            .iter()
            .filter_map(|row| row.first().map(cell_text))
            .filter(|name| !name.is_empty())
            .collect())
    }
}

// ---------------------------------------------------------------------------
// Constants and small shared helpers
// ---------------------------------------------------------------------------

/// Separator used to join cells into a key / row signature.
const UNIT_SEP: char = '\u{1f}';
/// Hard safety cap for `sample_limit` (rows read per side).
const MAX_SAMPLE_LIMIT: u64 = 5_000_000;
/// Hard safety cap for `max_statements`.
const MAX_STATEMENTS_CAP: usize = 100_000;

/// Join rendered cell values into one deterministic string.
fn join_cells(values: &[Value]) -> String {
    values
        .iter()
        .map(cell_text)
        .collect::<Vec<_>>()
        .join(&UNIT_SEP.to_string())
}

/// `schema.table` (Postgres) / `database.table` (others), engine-quoted.
/// Same-engine `SHOW CREATE TABLE` text is unqualified. A MySQL target session
/// without a default database fails with 1046 "No database selected", so
/// rewrite `CREATE TABLE \`t\`` to the fully qualified target name.
fn qualify_create_table(ddl: &str, engine: Engine, scope: &Scope) -> String {
    if !matches!(engine, Engine::MySql | Engine::MariaDb) {
        return ddl.to_string();
    }
    if scope.database.as_deref().filter(|s| !s.is_empty()).is_none() {
        return ddl.to_string();
    }
    let Ok(re) = regex::Regex::new(
        r"(?is)^(\s*CREATE\s+(?:TEMPORARY\s+)?TABLE\s+(?:IF\s+NOT\s+EXISTS\s+)?)([^\s(]+)",
    ) else {
        return ddl.to_string();
    };
    let Some(caps) = re.captures(ddl) else {
        return ddl.to_string();
    };
    let token = caps[2].trim();
    if identifier_is_qualified(token) {
        return ddl.to_string(); // already `db`.`t` or db.t
    }
    let name = unquote_identifier(token);
    let replacement = qualified_table(engine, scope, name);
    format!(
        "{}{}{}",
        &caps[1],
        replacement,
        &ddl[caps[0].len()..]
    )
}

/// True when the token contains a `.` outside quoted segments — i.e. it is
/// already `db.table` / `schema.table`.
fn identifier_is_qualified(token: &str) -> bool {
    let mut quote: Option<char> = None;
    for ch in token.chars() {
        match quote {
            Some(closing) => {
                if ch == closing {
                    quote = None;
                }
            }
            None => match ch {
                '.' => return true,
                '`' | '"' => quote = Some(ch),
                '[' => quote = Some(']'),
                _ => {}
            },
        }
    }
    false
}

/// Strips the outer quoting of a single identifier (`` `t` ``, `"t"`, `[t]`).
fn unquote_identifier(token: &str) -> &str {
    let t = token.trim();
    let first = t.chars().next();
    let last = t.chars().last();
    match (first, last) {
        (Some('`'), Some('`')) | (Some('"'), Some('"')) | (Some('['), Some(']'))
            if t.len() >= 2 =>
        {
            &t[1..t.len() - 1]
        }
        _ => t,
    }
}

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

/// Remove a single trailing `;` and surrounding whitespace from a DDL text.
fn trim_statement(sql: impl AsRef<str>) -> String {
    sql.as_ref().trim().trim_end_matches(';').trim_end().to_string()
}

/// Empty and comment-only statements are skipped by apply.
fn is_skippable_statement(sql: &str) -> bool {
    let trimmed = sql.trim();
    trimmed.is_empty() || trimmed.starts_with("--")
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

/// Deduplicate warnings while preserving their order.
fn dedupe_warnings(warnings: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::with_capacity(warnings.len());
    for warning in warnings {
        if seen.insert(warning.clone()) {
            out.push(warning);
        }
    }
    out
}

/// Case-insensitive lookup of `needle` in `names`.
fn position_ci(names: &[String], needle: &str) -> Option<usize> {
    names.iter().position(|n| n.eq_ignore_ascii_case(needle))
}

/// Map `wanted` to their positions inside `names` (case-insensitive).
fn positions_in(names: &[String], wanted: &[String]) -> ApiResult<Vec<usize>> {
    wanted
        .iter()
        .map(|w| {
            position_ci(names, w).ok_or_else(|| {
                MieoError::new(
                    ErrorCode::Internal,
                    format!("column `{w}` missing from the introspected list"),
                )
                .into()
            })
        })
        .collect()
}

/// `names_a` plus every `names_b` entry not already present (case-insensitive).
fn unique_names(names_a: &[String], names_b: &[String]) -> Vec<String> {
    let mut out: Vec<String> = names_a.to_vec();
    for name in names_b {
        if !out.iter().any(|n| n.eq_ignore_ascii_case(name)) {
            out.push(name.clone());
        }
    }
    out
}

/// Number of rows used and whether the query returned more than `limit`.
fn take_sample_len(rows: usize, sample_limit: u64) -> (usize, bool) {
    let limit = sample_limit.max(1);
    if rows as u64 > limit {
        (limit as usize, true)
    } else {
        (rows, false)
    }
}

/// Count values the driver could not decode (they become NULL literals).
fn count_unsupported(values: &[Value]) -> u64 {
    values
        .iter()
        .filter(|v| matches!(v, Value::Unsupported { .. }))
        .count() as u64
}

// ---------------------------------------------------------------------------
// Structure diff — pure helpers
// ---------------------------------------------------------------------------

/// Statements grouped by operation; `ordered` applies the contract order:
/// create -> add column -> alter column -> add index -> drop index ->
/// drop column -> drop table.
#[derive(Default)]
struct StatementBuckets {
    create_tables: Vec<SyncStatement>,
    add_columns: Vec<SyncStatement>,
    alter_columns: Vec<SyncStatement>,
    add_indexes: Vec<SyncStatement>,
    drop_indexes: Vec<SyncStatement>,
    drop_columns: Vec<SyncStatement>,
    drop_tables: Vec<SyncStatement>,
}

impl StatementBuckets {
    fn ordered(self) -> Vec<SyncStatement> {
        let mut out = Vec::new();
        out.extend(self.create_tables);
        out.extend(self.add_columns);
        out.extend(self.alter_columns);
        out.extend(self.add_indexes);
        out.extend(self.drop_indexes);
        out.extend(self.drop_columns);
        out.extend(self.drop_tables);
        out
    }
}

/// Normalize a native type for comparison.
fn normalized_type(native_type: &str) -> String {
    native_type.trim().to_lowercase()
}

/// Column-level diff by exact name (`Same` columns are omitted).
fn diff_columns(source: &[ColumnDef], target: &[ColumnDef]) -> Vec<ColumnDiff> {
    let target_by_name: HashMap<&str, &ColumnDef> =
        target.iter().map(|c| (c.name.as_str(), c)).collect();
    let source_names: HashSet<&str> = source.iter().map(|c| c.name.as_str()).collect();

    let mut out = Vec::new();
    for col in source {
        match target_by_name.get(col.name.as_str()) {
            None => out.push(ColumnDiff {
                name: col.name.clone(),
                source_type: Some(col.native_type.clone()),
                target_type: None,
                change: DiffChange::Added,
            }),
            Some(target_col) => {
                let type_differs = normalized_type(&col.native_type)
                    != normalized_type(&target_col.native_type);
                if type_differs || col.nullable != target_col.nullable {
                    out.push(ColumnDiff {
                        name: col.name.clone(),
                        source_type: Some(col.native_type.clone()),
                        target_type: Some(target_col.native_type.clone()),
                        change: DiffChange::Modified,
                    });
                }
            }
        }
    }
    for col in target {
        if !source_names.contains(col.name.as_str()) {
            out.push(ColumnDiff {
                name: col.name.clone(),
                source_type: None,
                target_type: Some(col.native_type.clone()),
                change: DiffChange::Removed,
            });
        }
    }
    out
}

/// PKs and engine-internal indexes never participate in the index diff.
fn is_skippable_index(index: &IndexDef, engine: Engine) -> bool {
    index.is_primary
        || (engine == Engine::Sqlite
            && index.name.to_lowercase().starts_with("sqlite_autoindex"))
}

/// Same index name and columns (case-insensitive) plus unique flag.
fn index_equivalent(a: &IndexDef, b: &IndexDef) -> bool {
    a.is_unique == b.is_unique
        && a.columns.len() == b.columns.len()
        && a.columns
            .iter()
            .zip(b.columns.iter())
            .all(|(x, y)| x.eq_ignore_ascii_case(y))
}

struct IndexDiff {
    added: Vec<IndexDef>,
    removed: Vec<IndexDef>,
    add_statements: Vec<SyncStatement>,
    drop_statements: Vec<SyncStatement>,
    warnings: Vec<String>,
}

/// Diff indexes by name. Same-name/different-shape indexes are reported but
/// not rewritten (the add-before-drop order would collide), and target indexes
/// that cannot be dropped directly (PG constraint-backed) are skipped.
fn diff_indexes(
    table: &str,
    source: &[IndexDef],
    target: &[IndexDef],
    engine: Engine,
    scope: &Scope,
    non_droppable: &HashSet<String>,
) -> IndexDiff {
    let mut out = IndexDiff {
        added: Vec::new(),
        removed: Vec::new(),
        add_statements: Vec::new(),
        drop_statements: Vec::new(),
        warnings: Vec::new(),
    };

    let target_by_name: HashMap<&str, &IndexDef> = target
        .iter()
        .filter(|i| !is_skippable_index(i, engine))
        .map(|i| (i.name.as_str(), i))
        .collect();
    let source_names: HashSet<&str> = source
        .iter()
        .filter(|i| !is_skippable_index(i, engine))
        .map(|i| i.name.as_str())
        .collect();

    for index in source.iter().filter(|i| !is_skippable_index(i, engine)) {
        if index.columns.is_empty() {
            out.warnings.push(format!(
                "index `{table}.{}` has no plain columns and cannot be recreated; skipped",
                index.name
            ));
            continue;
        }
        match target_by_name.get(index.name.as_str()) {
            Some(target_index) if index_equivalent(index, target_index) => {
                // identical
            }
            Some(_) => out.warnings.push(format!(
                "index `{table}.{}` differs between source and target; skipped (cannot drop \
                 and re-create in a single pass)",
                index.name
            )),
            None => {
                out.added.push(index.clone());
                out.add_statements.push(SyncStatement {
                    table: table.to_string(),
                    sql: create_index_sql(engine, scope, table, index),
                    kind: SyncStatementKind::AddIndex,
                    destructive: false,
                });
            }
        }
    }

    for index in target.iter().filter(|i| !is_skippable_index(i, engine)) {
        if source_names.contains(index.name.as_str()) {
            continue;
        }
        out.removed.push(index.clone());
        if non_droppable.contains(&index.name) {
            out.warnings.push(format!(
                "index `{table}.{}` exists only on the target but is constraint-backed and \
                 cannot be dropped; skipped",
                index.name
            ));
            continue;
        }
        out.drop_statements.push(SyncStatement {
            table: table.to_string(),
            sql: drop_index_sql(engine, scope, table, &index.name),
            kind: SyncStatementKind::DropIndex,
            destructive: true,
        });
    }

    out
}

fn fk_equivalent(a: &ForeignKeyDef, b: &ForeignKeyDef) -> bool {
    a.columns.len() == b.columns.len()
        && a.ref_columns.len() == b.ref_columns.len()
        && a.columns
            .iter()
            .zip(b.columns.iter())
            .all(|(x, y)| x.eq_ignore_ascii_case(y))
        && a.ref_columns
            .iter()
            .zip(b.ref_columns.iter())
            .all(|(x, y)| x.eq_ignore_ascii_case(y))
        && a.ref_table.eq_ignore_ascii_case(&b.ref_table)
}

fn fk_line(sign: char, fk: &ForeignKeyDef) -> String {
    let reference = match fk.ref_schema.as_deref().filter(|s| !s.is_empty()) {
        Some(schema) => format!("{schema}.{}", fk.ref_table),
        None => fk.ref_table.clone(),
    };
    format!(
        "{sign} {} ({}) -> {reference}({})",
        fk.name,
        fk.columns.join(", "),
        fk.ref_columns.join(", ")
    )
}

/// FK differences as `+` (source only), `-` (target only), `~` (changed).
/// Stable topological sort: parents before children, original order kept
/// wherever dependencies allow. Table names are lower-case keys in `deps`;
/// a self-reference never blocks (the table is created in the same statement).
fn sort_create_tables_by_deps(
    statements: &mut Vec<SyncStatement>,
    deps: &HashMap<String, Vec<String>>,
) {
    let pending: HashSet<String> = statements.iter().map(|s| s.table.to_lowercase()).collect();
    let mut emitted: HashSet<String> = HashSet::new();
    let mut out: Vec<SyncStatement> = Vec::with_capacity(statements.len());
    let mut remaining: Vec<SyncStatement> = std::mem::take(statements);

    while !remaining.is_empty() {
        let mut bumped: Vec<SyncStatement> = Vec::with_capacity(remaining.len());
        let mut progressed = false;
        for stmt in remaining {
            let self_name = stmt.table.to_lowercase();
            let ready = deps
                .get(&self_name)
                .map(|refs| {
                    refs.iter().all(|r| {
                        *r == self_name || !pending.contains(r) || emitted.contains(r)
                    })
                })
                .unwrap_or(true);
            if ready {
                emitted.insert(self_name);
                out.push(stmt);
                progressed = true;
            } else {
                bumped.push(stmt);
            }
        }
        remaining = bumped;
        if !progressed {
            // Dependency cycle: emit the rest as-is rather than losing them.
            out.extend(remaining.drain(..));
            break;
        }
    }
    *statements = out;
}

fn fk_diff_lines(source: &[ForeignKeyDef], target: &[ForeignKeyDef]) -> Vec<String> {
    let source_names: HashSet<&str> = source.iter().map(|f| f.name.as_str()).collect();
    let target_by_name: HashMap<&str, &ForeignKeyDef> =
        target.iter().map(|f| (f.name.as_str(), f)).collect();

    let mut out = Vec::new();
    for fk in source {
        match target_by_name.get(fk.name.as_str()) {
            None => out.push(fk_line('+', fk)),
            Some(t) if !fk_equivalent(fk, t) => out.push(fk_line('~', fk)),
            Some(_) => {}
        }
    }
    for fk in target {
        if !source_names.contains(fk.name.as_str()) {
            out.push(fk_line('-', fk));
        }
    }
    out
}

/// Type for a source column on the target engine.
///
/// Same-engine plans keep the source `native_type` verbatim; cross-engine
/// plans map the normalized [`DataTypeKind`] (best-effort).
fn column_type_sql(target: Engine, col: &ColumnDef, same_engine: bool) -> String {
    if same_engine && !col.native_type.trim().is_empty() {
        return col.native_type.trim().to_string();
    }
    map_type(target, col)
}

fn map_type(engine: Engine, col: &ColumnDef) -> String {
    map_kind(engine, &col.data_type, col)
}

fn map_kind(engine: Engine, kind: &DataTypeKind, col: &ColumnDef) -> String {
    match engine {
        Engine::Postgres => match kind {
            DataTypeKind::Bool => "boolean".into(),
            DataTypeKind::Int | DataTypeKind::UInt => "bigint".into(),
            DataTypeKind::Float => "double precision".into(),
            DataTypeKind::Decimal => match col.numeric_precision {
                Some(p) => match col.numeric_scale {
                    Some(s) => format!("numeric({p},{s})"),
                    None => format!("numeric({p})"),
                },
                None => "numeric".into(),
            },
            DataTypeKind::Text => "text".into(),
            DataTypeKind::Bytes => "bytea".into(),
            DataTypeKind::Date => "date".into(),
            DataTypeKind::Time => "time".into(),
            DataTypeKind::DateTime => "timestamp".into(),
            DataTypeKind::TimestampTz => "timestamptz".into(),
            DataTypeKind::Json => "jsonb".into(),
            DataTypeKind::Uuid => "uuid".into(),
            DataTypeKind::Bit => "bit".into(),
            DataTypeKind::Interval => "interval".into(),
            DataTypeKind::Enum | DataTypeKind::Set => "text".into(),
            DataTypeKind::Geometry => "bytea".into(),
            DataTypeKind::Array(inner) => {
                format!("{}[]", map_kind(engine, inner, col))
            }
            DataTypeKind::Unknown => "text".into(),
        },
        Engine::MySql | Engine::MariaDb => match kind {
            DataTypeKind::Bool => "CHAR(1)".into(),
            DataTypeKind::Int => "BIGINT".into(),
            DataTypeKind::UInt => "BIGINT UNSIGNED".into(),
            DataTypeKind::Float => "DOUBLE".into(),
            DataTypeKind::Decimal => match col.numeric_precision {
                Some(p) => match col.numeric_scale {
                    Some(s) => format!("DECIMAL({p},{s})"),
                    None => format!("DECIMAL({p})"),
                },
                None => "DECIMAL".into(),
            },
            DataTypeKind::Text => "TEXT".into(),
            DataTypeKind::Bytes => "LONGBLOB".into(),
            DataTypeKind::Date => "DATE".into(),
            DataTypeKind::Time => "TIME".into(),
            DataTypeKind::DateTime | DataTypeKind::TimestampTz => "DATETIME".into(),
            DataTypeKind::Json => "JSON".into(),
            DataTypeKind::Uuid => "CHAR(36)".into(),
            DataTypeKind::Bit => "BIT(1)".into(),
            DataTypeKind::Interval => "BIGINT".into(),
            DataTypeKind::Enum | DataTypeKind::Set => "TEXT".into(),
            DataTypeKind::Geometry => "LONGBLOB".into(),
            DataTypeKind::Array(_) => "JSON".into(),
            DataTypeKind::Unknown => "TEXT".into(),
        },
        Engine::Sqlite => match kind {
            DataTypeKind::Bool
            | DataTypeKind::Int
            | DataTypeKind::UInt
            | DataTypeKind::Bit => "INTEGER".into(),
            DataTypeKind::Float | DataTypeKind::Decimal => "REAL".into(),
            DataTypeKind::Bytes => "BLOB".into(),
            _ => "TEXT".into(),
        },
        Engine::SqlServer => match kind {
            DataTypeKind::Bool => "BIT".into(),
            DataTypeKind::Int => "BIGINT".into(),
            // SQL Server has no unsigned integers; DECIMAL(20,0) holds any u64.
            DataTypeKind::UInt => "DECIMAL(20,0)".into(),
            DataTypeKind::Float => "FLOAT".into(),
            DataTypeKind::Decimal => match col.numeric_precision {
                Some(p) => match col.numeric_scale {
                    Some(s) => format!("DECIMAL({p},{s})"),
                    None => format!("DECIMAL({p})"),
                },
                None => "DECIMAL(38,10)".into(),
            },
            DataTypeKind::Text => "NVARCHAR(MAX)".into(),
            DataTypeKind::Bytes => "VARBINARY(MAX)".into(),
            DataTypeKind::Date => "DATE".into(),
            DataTypeKind::Time => "TIME".into(),
            DataTypeKind::DateTime => "DATETIME2".into(),
            DataTypeKind::TimestampTz => "DATETIMEOFFSET".into(),
            DataTypeKind::Json => "NVARCHAR(MAX)".into(),
            DataTypeKind::Uuid => "UNIQUEIDENTIFIER".into(),
            DataTypeKind::Bit => "VARBINARY(MAX)".into(),
            DataTypeKind::Interval => "NVARCHAR(64)".into(),
            DataTypeKind::Enum | DataTypeKind::Set => "NVARCHAR(MAX)".into(),
            DataTypeKind::Geometry => "VARBINARY(MAX)".into(),
            DataTypeKind::Array(_) => "NVARCHAR(MAX)".into(),
            DataTypeKind::Unknown => "NVARCHAR(MAX)".into(),
        },
        // Engines beyond the implemented SQL drivers keep the native type.
        Engine::ClickHouse => {
            let base = match kind {
                DataTypeKind::Bool | DataTypeKind::Bit => "UInt8".to_string(),
                DataTypeKind::Int => "Int64".to_string(),
                DataTypeKind::UInt => "UInt64".to_string(),
                DataTypeKind::Float => "Float64".to_string(),
                DataTypeKind::Decimal => match col.numeric_precision {
                    Some(p) => match col.numeric_scale {
                        Some(s) => format!("Decimal({p},{s})"),
                        None => format!("Decimal({p})"),
                    },
                    None => "Decimal(38,10)".to_string(),
                },
                DataTypeKind::Date => "Date".to_string(),
                DataTypeKind::DateTime | DataTypeKind::TimestampTz => {
                    "DateTime64(3)".to_string()
                }
                DataTypeKind::Uuid => "UUID".to_string(),
                DataTypeKind::Array(_) => "Array(String)".to_string(),
                _ => "String".to_string(),
            };
            // ClickHouse types are non-nullable by default; Array cannot be
            // wrapped in Nullable.
            if col.nullable && !base.starts_with("Array(") {
                format!("Nullable({base})")
            } else {
                base
            }
        }
        _ => {
            if col.native_type.trim().is_empty() {
                "TEXT".into()
            } else {
                col.native_type.trim().to_string()
            }
        }
    }
}

/// `CREATE TABLE` derived from column metadata (cross-engine or missing DDL).
fn generated_create_table_sql(
    engine: Engine,
    scope: &Scope,
    table: &str,
    columns: &[ColumnDef],
    same_engine: bool,
) -> String {
    let mut defs: Vec<String> = Vec::with_capacity(columns.len() + 1);
    let mut pk: Vec<String> = Vec::new();
    for col in columns {
        let mut def = format!(
            "{} {}",
            quote_ident(engine, &col.name),
            column_type_sql(engine, col, same_engine)
        );
        if !col.nullable {
            def.push_str(" NOT NULL");
        }
        if col.is_primary_key && col.is_auto_increment
            && matches!(engine, Engine::MySql | Engine::MariaDb)
        {
            def.push_str(" AUTO_INCREMENT");
        }
        defs.push(def);
        if col.is_primary_key {
            pk.push(quote_ident(engine, &col.name));
        }
    }
    if !pk.is_empty() {
        defs.push(format!("PRIMARY KEY ({})", pk.join(", ")));
    }
    let mut sql = format!(
        "CREATE TABLE {} ({})",
        qualified_table(engine, scope, table),
        defs.join(", ")
    );
    if engine == Engine::ClickHouse {
        // MergeTree needs an explicit sort key.
        if pk.is_empty() {
            sql.push_str(" ENGINE = MergeTree ORDER BY tuple()");
        } else {
            sql.push_str(&format!(" ENGINE = MergeTree ORDER BY ({})", pk.join(", ")));
        }
    }
    sql
}

fn add_column_sql(
    engine: Engine,
    scope: &Scope,
    table: &str,
    col: &ColumnDef,
    same_engine: bool,
) -> String {
    let mut def = format!(
        "{} {}",
        quote_ident(engine, &col.name),
        column_type_sql(engine, col, same_engine)
    );
    if !col.nullable {
        def.push_str(" NOT NULL");
    }
    format!(
        "ALTER TABLE {} ADD COLUMN {def}",
        qualified_table(engine, scope, table)
    )
}

/// `None` = the engine cannot alter column types (SQLite).
fn alter_column_sql(
    engine: Engine,
    scope: &Scope,
    table: &str,
    col: &ColumnDef,
    same_engine: bool,
) -> Option<String> {
    let qualified = qualified_table(engine, scope, table);
    let name = quote_ident(engine, &col.name);
    let type_sql = column_type_sql(engine, col, same_engine);
    match engine {
        Engine::MySql | Engine::MariaDb => {
            let mut def = format!("{name} {type_sql}");
            if !col.nullable {
                def.push_str(" NOT NULL");
            }
            Some(format!("ALTER TABLE {qualified} MODIFY COLUMN {def}"))
        }
        Engine::Postgres => Some(format!(
            "ALTER TABLE {qualified} ALTER COLUMN {name} TYPE {type_sql}"
        )),
        Engine::ClickHouse => {
            // `type_sql` already carries Nullable(...) when the column allows
            // NULL, so no separate NOT NULL clause is needed.
            Some(format!(
                "ALTER TABLE {qualified} MODIFY COLUMN {name} {type_sql}"
            ))
        }
        _ => None,
    }
}

fn create_index_sql(engine: Engine, scope: &Scope, table: &str, index: &IndexDef) -> String {
    let columns = index
        .columns
        .iter()
        .map(|c| quote_ident(engine, c))
        .collect::<Vec<_>>()
        .join(", ");
    let unique = if index.is_unique { "UNIQUE " } else { "" };
    if engine == Engine::Sqlite {
        // SQLite rejects a schema-qualified table here; the index name carries
        // the database instead (`CREATE INDEX main.ix ON t (...)`).
        let index_name = match scope.database.as_deref().filter(|s| !s.is_empty()) {
            Some(database) => format!(
                "{}.{}",
                quote_ident(engine, database),
                quote_ident(engine, &index.name)
            ),
            None => quote_ident(engine, &index.name),
        };
        return format!(
            "CREATE {unique}INDEX {index_name} ON {} ({columns})",
            quote_ident(engine, table)
        );
    }
    format!(
        "CREATE {unique}INDEX {} ON {} ({columns})",
        quote_ident(engine, &index.name),
        qualified_table(engine, scope, table)
    )
}

fn drop_index_sql(engine: Engine, scope: &Scope, table: &str, name: &str) -> String {
    match engine {
        Engine::MySql | Engine::MariaDb => format!(
            "DROP INDEX {} ON {}",
            quote_ident(engine, name),
            qualified_table(engine, scope, table)
        ),
        Engine::Postgres => {
            let index = match scope.schema.as_deref().filter(|s| !s.is_empty()) {
                Some(schema) => format!(
                    "{}.{}",
                    quote_ident(engine, schema),
                    quote_ident(engine, name)
                ),
                None => quote_ident(engine, name),
            };
            format!("DROP INDEX {index}")
        }
        _ => {
            let index = match scope.database.as_deref().filter(|s| !s.is_empty()) {
                Some(database) => format!(
                    "{}.{}",
                    quote_ident(engine, database),
                    quote_ident(engine, name)
                ),
                None => quote_ident(engine, name),
            };
            format!("DROP INDEX {index}")
        }
    }
}

// ---------------------------------------------------------------------------
// Data diff — pure helpers
// ---------------------------------------------------------------------------

/// One row participating in the diff.
#[derive(Debug, Clone)]
struct RowState {
    /// Rendered key (used for matching).
    key: String,
    key_values: Vec<Value>,
    /// Values compared for UPDATE detection.
    compare_values: Vec<Value>,
    /// Rendered compare values (textual hash).
    signature: String,
    /// Full source row (source side only; empty on the target side).
    full_values: Vec<Value>,
}

impl RowState {
    fn new(
        key_values: Vec<Value>,
        compare_values: Vec<Value>,
        full_values: Vec<Value>,
    ) -> Self {
        Self {
            key: join_cells(&key_values),
            signature: join_cells(&compare_values),
            key_values,
            compare_values,
            full_values,
        }
    }
}

/// Rows read from one endpoint.
struct FetchedRows {
    rows: Vec<Vec<Value>>,
}

/// Column lists resolved for a data sync.
#[derive(Debug)]
struct DataColumns {
    /// Key columns (source names).
    keys: Vec<String>,
    /// Key columns (target names).
    keys_target: Vec<String>,
    /// Columns written by INSERT (source names, common to both sides).
    insert: Vec<String>,
    /// Columns written by INSERT (target names).
    insert_target: Vec<String>,
    /// Columns written by UPDATE (source names).
    compare: Vec<String>,
    /// Columns written by UPDATE (target names).
    compare_target: Vec<String>,
}

/// Case-insensitive column lookup by (trimmed) name.
fn find_column<'a>(cols: &'a [ColumnDef], name: &str) -> Option<&'a ColumnDef> {
    cols.iter()
        .find(|c| c.name.eq_ignore_ascii_case(name.trim()))
}

/// Resolve key/compare/insert columns and validate them on both sides.
fn resolve_data_columns(
    source: &[ColumnDef],
    target: &[ColumnDef],
    requested_keys: &[String],
    requested_compare: &[String],
    warnings: &mut Vec<String>,
) -> ApiResult<DataColumns> {
    // --- keys: request, else source primary key.
    let mut keys: Vec<String> = Vec::new();
    for requested in requested_keys {
        let requested = requested.trim();
        if requested.is_empty() {
            continue;
        }
        let col = find_column(source, requested).ok_or_else(|| {
            MieoError::new(
                ErrorCode::NotFound,
                format!("key column `{requested}` not found in the source table"),
            )
            .with_hint("check key_columns")
        })?;
        if !keys.iter().any(|k| k.eq_ignore_ascii_case(&col.name)) {
            keys.push(col.name.clone());
        }
    }
    if keys.is_empty() {
        let mut pk: Vec<&ColumnDef> = source.iter().filter(|c| c.is_primary_key).collect();
        pk.sort_by_key(|c| c.ordinal);
        keys = pk.into_iter().map(|c| c.name.clone()).collect();
    }
    if keys.is_empty() {
        return Err(MieoError::new(
            ErrorCode::Data,
            "no key columns available for the data diff",
        )
        .with_hint("specify key_columns, or use a table with a primary key")
        .into());
    }

    let keys_target: Vec<String> = keys
        .iter()
        .map(|key| {
            find_column(target, key)
                .map(|c| c.name.clone())
                .ok_or_else(|| {
                    MieoError::new(
                        ErrorCode::NotFound,
                        format!("key column `{key}` not found in the target table"),
                    )
                    .with_hint("source and target keys must exist on both sides")
                })
        })
        .collect::<Result<_, _>>()?;

    // --- insert columns: source columns that also exist on the target.
    let mut insert = Vec::new();
    let mut insert_target = Vec::new();
    let mut missing = Vec::new();
    for col in source {
        match find_column(target, &col.name) {
            Some(target_col) => {
                let is_key = keys.iter().any(|k| k.eq_ignore_ascii_case(&col.name));
                if target_col.is_generated && !is_key {
                    warnings.push(format!(
                        "column `{}` is generated on the target and is skipped",
                        target_col.name
                    ));
                    continue;
                }
                insert.push(col.name.clone());
                insert_target.push(target_col.name.clone());
            }
            None => missing.push(col.name.clone()),
        }
    }
    if !missing.is_empty() {
        warnings.push(format!(
            "column(s) not present on the target are skipped: {}",
            missing.join(", ")
        ));
    }
    if insert.is_empty() {
        return Err(MieoError::new(
            ErrorCode::Data,
            "the source and target tables share no columns",
        )
        .into());
    }

    // --- compare columns: request (ci, canonical source names), else every
    // non-key insert column.
    let requested: Vec<String> = if requested_compare.is_empty() {
        insert
            .iter()
            .filter(|name| !keys.iter().any(|k| k.eq_ignore_ascii_case(name)))
            .cloned()
            .collect()
    } else {
        let mut resolved: Vec<String> = Vec::new();
        for requested in requested_compare {
            let requested = requested.trim();
            if requested.is_empty() {
                continue;
            }
            let col = find_column(source, requested).ok_or_else(|| {
                MieoError::new(
                    ErrorCode::NotFound,
                    format!("compare column `{requested}` not found in the source table"),
                )
                .with_hint("check compare_columns")
            })?;
            if !resolved.iter().any(|c| c.eq_ignore_ascii_case(&col.name)) {
                resolved.push(col.name.clone());
            }
        }
        resolved
    };

    let mut compare = Vec::new();
    let mut compare_target = Vec::new();
    for name in requested {
        if keys.iter().any(|k| k.eq_ignore_ascii_case(&name)) {
            continue;
        }
        match insert
            .iter()
            .position(|n| n.eq_ignore_ascii_case(&name))
        {
            Some(pos) => {
                compare.push(insert[pos].clone());
                compare_target.push(insert_target[pos].clone());
            }
            None => warnings.push(format!(
                "compare column `{name}` is not present on the target and is skipped"
            )),
        }
    }

    Ok(DataColumns {
        keys,
        keys_target,
        insert,
        insert_target,
        compare,
        compare_target,
    })
}

/// Pluck values at `positions` from a result row.
fn pluck(row: &[Value], positions: &[usize]) -> ApiResult<Vec<Value>> {
    positions
        .iter()
        .map(|&i| {
            row.get(i).cloned().ok_or_else(|| {
                MieoError::new(
                    ErrorCode::Data,
                    "query returned fewer columns than requested",
                )
                .into()
            })
        })
        .collect()
}

fn build_source_states(
    fetched: &FetchedRows,
    key_positions: &[usize],
    compare_positions: &[usize],
    used: usize,
) -> ApiResult<(BTreeMap<String, RowState>, u64)> {
    let mut map = BTreeMap::new();
    let mut duplicates = 0u64;
    for row in fetched.rows.iter().take(used) {
        let state = RowState::new(
            pluck(row, key_positions)?,
            pluck(row, compare_positions)?,
            row.clone(),
        );
        if map.insert(state.key.clone(), state).is_some() {
            duplicates += 1;
        }
    }
    Ok((map, duplicates))
}

fn build_target_states(
    fetched: &FetchedRows,
    key_positions: &[usize],
    compare_positions: &[usize],
    used: usize,
) -> ApiResult<(BTreeMap<String, RowState>, u64)> {
    let mut map = BTreeMap::new();
    let mut duplicates = 0u64;
    for row in fetched.rows.iter().take(used) {
        let state = RowState::new(
            pluck(row, key_positions)?,
            pluck(row, compare_positions)?,
            Vec::new(),
        );
        if map.insert(state.key.clone(), state).is_some() {
            duplicates += 1;
        }
    }
    Ok((map, duplicates))
}

/// Row diff result: inserts/updates carry source values, deletes key values.
struct RowDiff {
    /// Statement payloads, capped at `max_statements`.
    inserts: Vec<RowState>,
    updates: Vec<RowState>,
    deletes: Vec<RowState>,
    /// Accurate counts when the scan completed (lower bounds on early stop).
    insert_count: u64,
    update_count: u64,
    delete_count: u64,
    unchanged: u64,
    /// Scanning stopped early (`!full_scan` + statement cap reached).
    scan_truncated: bool,
    /// The statement list had to be capped (includes `scan_truncated`).
    statements_truncated: bool,
}

/// Pair rows by key and classify them.
///
/// Matching target entries are removed as they are seen, so whatever remains
/// is target-only (DELETE). When `full_scan` is false the scan stops as soon
/// as `max_statements` changes were found, making the counts a lower bound.
/// With `full_scan` the counts are exact but the statement payloads are still
/// capped so a preview cannot explode.
fn diff_rows(
    source: BTreeMap<String, RowState>,
    mut target: BTreeMap<String, RowState>,
    max_statements: usize,
    full_scan: bool,
) -> RowDiff {
    let cap = max_statements.max(1);
    let mut inserts = Vec::new();
    let mut updates = Vec::new();
    let mut unchanged = 0u64;
    let mut scan_truncated = false;

    for (key, source_row) in source {
        match target.remove(&key) {
            None => inserts.push(source_row),
            Some(target_row) => {
                if target_row.signature == source_row.signature {
                    unchanged += 1;
                } else {
                    updates.push(source_row);
                }
            }
        }
        if !full_scan && inserts.len() + updates.len() >= cap {
            scan_truncated = true;
            break;
        }
    }

    let mut deletes = if scan_truncated {
        Vec::new()
    } else {
        target.into_values().collect()
    };
    let insert_count = inserts.len() as u64;
    let update_count = updates.len() as u64;
    let delete_count = deletes.len() as u64;

    let mut statements_truncated = scan_truncated;
    if insert_count + update_count + delete_count > cap as u64 {
        statements_truncated = true;
        // Keep inserts first, then updates, then deletes.
        inserts.truncate(cap);
        updates.truncate(cap.saturating_sub(inserts.len()));
        deletes.truncate(cap.saturating_sub(inserts.len() + updates.len()));
    }

    RowDiff {
        inserts,
        updates,
        deletes,
        insert_count,
        update_count,
        delete_count,
        unchanged,
        scan_truncated,
        statements_truncated,
    }
}

fn insert_row_sql(
    engine: Engine,
    scope: &Scope,
    table: &str,
    columns: &[String],
    values: &[Value],
) -> String {
    let cols = columns
        .iter()
        .map(|c| quote_ident(engine, c))
        .collect::<Vec<_>>()
        .join(", ");
    let vals = values
        .iter()
        .map(|v| sql_literal(engine, v))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "INSERT INTO {} ({cols}) VALUES ({vals});",
        qualified_table(engine, scope, table)
    )
}

fn update_row_sql(
    engine: Engine,
    scope: &Scope,
    table: &str,
    set_columns: &[String],
    set_values: &[Value],
    key_columns: &[String],
    key_values: &[Value],
) -> String {
    let sets = set_columns
        .iter()
        .zip(set_values.iter())
        .map(|(c, v)| format!("{} = {}", quote_ident(engine, c), sql_literal(engine, v)))
        .collect::<Vec<_>>()
        .join(", ");
    let wheres = key_columns
        .iter()
        .zip(key_values.iter())
        .map(|(c, v)| format!("{} = {}", quote_ident(engine, c), sql_literal(engine, v)))
        .collect::<Vec<_>>()
        .join(" AND ");
    format!(
        "UPDATE {} SET {sets} WHERE {wheres};",
        qualified_table(engine, scope, table)
    )
}

fn delete_row_sql(
    engine: Engine,
    scope: &Scope,
    table: &str,
    key_columns: &[String],
    key_values: &[Value],
) -> String {
    let wheres = key_columns
        .iter()
        .zip(key_values.iter())
        .map(|(c, v)| format!("{} = {}", quote_ident(engine, c), sql_literal(engine, v)))
        .collect::<Vec<_>>()
        .join(" AND ");
    format!(
        "DELETE FROM {} WHERE {wheres};",
        qualified_table(engine, scope, table)
    )
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- fixtures ---------------------------------------------------------

    fn col(name: &str, native: &str, kind: DataTypeKind, nullable: bool) -> ColumnDef {
        ColumnDef {
            name: name.to_string(),
            ordinal: 1,
            native_type: native.to_string(),
            data_type: kind,
            nullable,
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

    fn table_scope(db: &str) -> Scope {
        Scope {
            database: Some(db.to_string()),
            schema: None,
        }
    }

    fn pg_scope(schema: &str) -> Scope {
        Scope {
            database: None,
            schema: Some(schema.to_string()),
        }
    }

    #[test]
    fn qualify_create_table_for_mysql() {
        // Unqualified DDL copied from SHOW CREATE TABLE gets the target db.
        assert_eq!(
            qualify_create_table(
                "CREATE TABLE `users` (\n  `id` int\n) ENGINE=InnoDB",
                Engine::MySql,
                &table_scope("tgt"),
            ),
            "CREATE TABLE `tgt`.`users` (\n  `id` int\n) ENGINE=InnoDB"
        );
        // Already qualified: untouched.
        assert_eq!(
            qualify_create_table(
                "CREATE TABLE `src`.`users` (`id` int)",
                Engine::MySql,
                &table_scope("tgt"),
            ),
            "CREATE TABLE `src`.`users` (`id` int)"
        );
        // Quoted name containing a dot stays a single identifier.
        assert_eq!(
            qualify_create_table(
                "CREATE TABLE `we.ird` (`id` int)",
                Engine::MySql,
                &table_scope("tgt"),
            ),
            "CREATE TABLE `tgt`.`we.ird` (`id` int)"
        );
        // IF NOT EXISTS keeps its clause.
        assert_eq!(
            qualify_create_table(
                "CREATE TABLE IF NOT EXISTS users (id int)",
                Engine::MariaDb,
                &table_scope("tgt"),
            ),
            "CREATE TABLE IF NOT EXISTS `tgt`.`users` (id int)"
        );
        // Other engines are left alone (SQLite tests expect unqualified DDL).
        assert_eq!(
            qualify_create_table(
                "CREATE TABLE items (id INTEGER PRIMARY KEY)",
                Engine::Sqlite,
                &table_scope("main"),
            ),
            "CREATE TABLE items (id INTEGER PRIMARY KEY)"
        );
    }

    fn idx(name: &str, columns: &[&str], unique: bool, primary: bool) -> IndexDef {
        IndexDef {
            name: name.to_string(),
            columns: columns.iter().map(|c| c.to_string()).collect(),
            is_unique: unique,
            is_primary: primary,
            index_type: None,
            comment: None,
        }
    }

    fn fk(name: &str, columns: &[&str], ref_table: &str, ref_columns: &[&str]) -> ForeignKeyDef {
        ForeignKeyDef {
            name: name.to_string(),
            columns: columns.iter().map(|c| c.to_string()).collect(),
            ref_table: ref_table.to_string(),
            ref_schema: None,
            ref_columns: ref_columns.iter().map(|c| c.to_string()).collect(),
            on_update: None,
            on_delete: None,
        }
    }

    fn src_rows(items: &[(i64, &str)]) -> BTreeMap<String, RowState> {
        items
            .iter()
            .map(|(id, name)| {
                let key_values = vec![Value::Int(*id)];
                let compare_values = vec![Value::Text((*name).to_string())];
                let full_values = vec![Value::Int(*id), Value::Text((*name).to_string())];
                let state = RowState::new(key_values, compare_values, full_values);
                (state.key.clone(), state)
            })
            .collect()
    }

    fn tgt_rows(items: &[(i64, &str)]) -> BTreeMap<String, RowState> {
        items
            .iter()
            .map(|(id, name)| {
                let state = RowState::new(
                    vec![Value::Int(*id)],
                    vec![Value::Text((*name).to_string())],
                    Vec::new(),
                );
                (state.key.clone(), state)
            })
            .collect()
    }

    // --- structure: column diff ------------------------------------------

    #[test]
    fn diff_columns_classifies_all_changes() {
        let source = vec![
            col("id", "INTEGER", DataTypeKind::Int, false),
            col("name", "VARCHAR(50)", DataTypeKind::Text, true),
            col("score", "REAL", DataTypeKind::Float, true),
            col("created", "TIMESTAMP", DataTypeKind::DateTime, true),
        ];
        let target = vec![
            col("id", "INTEGER", DataTypeKind::Int, false),
            col("name", "VARCHAR(255)", DataTypeKind::Text, true),
            col("old", "TEXT", DataTypeKind::Text, true),
        ];

        let diffs = diff_columns(&source, &target);
        assert_eq!(diffs.len(), 4);
        assert_eq!(diffs[0].name, "name");
        assert_eq!(diffs[0].change, DiffChange::Modified);
        assert_eq!(diffs[1].name, "score");
        assert_eq!(diffs[1].change, DiffChange::Added);
        assert_eq!(diffs[2].name, "created");
        assert_eq!(diffs[2].change, DiffChange::Added);
        assert_eq!(diffs[3].name, "old");
        assert_eq!(diffs[3].change, DiffChange::Removed);
    }

    #[test]
    fn diff_columns_ignores_case_and_whitespace() {
        let source = vec![col("name", " VARCHAR(50) ", DataTypeKind::Text, true)];
        let target = vec![col("name", "varchar(50)", DataTypeKind::Text, true)];
        assert!(diff_columns(&source, &target).is_empty());
    }

    #[test]
    fn diff_columns_detects_nullability_change() {
        let source = vec![col("name", "TEXT", DataTypeKind::Text, false)];
        let target = vec![col("name", "TEXT", DataTypeKind::Text, true)];
        let diffs = diff_columns(&source, &target);
        assert_eq!(diffs.len(), 1);
        assert_eq!(diffs[0].change, DiffChange::Modified);
    }

    // --- structure: DDL generation ----------------------------------------

    #[test]
    fn create_table_cross_engine_maps_types_for_postgres() {
        let mut id = col("id", "INTEGER", DataTypeKind::Int, false);
        id.is_primary_key = true;
        id.is_auto_increment = true;
        let mut amount = col("amount", "DECIMAL(10,2)", DataTypeKind::Decimal, true);
        amount.numeric_precision = Some(10);
        amount.numeric_scale = Some(2);
        let cols = vec![
            id,
            col("name", "TEXT", DataTypeKind::Text, true),
            col("ok", "INTEGER", DataTypeKind::Bool, true),
            amount,
            col("payload", "BLOB", DataTypeKind::Bytes, true),
        ];

        let sql = generated_create_table_sql(
            Engine::Postgres,
            &pg_scope("public"),
            "items",
            &cols,
            false,
        );
        assert_eq!(
            sql,
            "CREATE TABLE \"public\".\"items\" (\"id\" bigint NOT NULL, \"name\" text, \
             \"ok\" boolean, \"amount\" numeric(10,2), \"payload\" bytea, PRIMARY KEY (\"id\"))"
        );
    }

    #[test]
    fn create_table_mysql_uses_contract_types_and_auto_increment() {
        let mut id = col("id", "INTEGER", DataTypeKind::Int, false);
        id.is_primary_key = true;
        id.is_auto_increment = true;
        let cols = vec![
            id,
            col("flag", "INTEGER", DataTypeKind::Bool, true),
            col("label", "TEXT", DataTypeKind::Text, true),
        ];
        let sql =
            generated_create_table_sql(Engine::MySql, &table_scope("shop"), "t", &cols, false);
        assert_eq!(
            sql,
            "CREATE TABLE `shop`.`t` (`id` BIGINT NOT NULL AUTO_INCREMENT, `flag` CHAR(1), \
             `label` TEXT, PRIMARY KEY (`id`))"
        );
    }

    #[test]
    fn create_table_same_engine_keeps_native_types() {
        let cols = vec![
            col("name", "VARCHAR(120)", DataTypeKind::Text, true),
            col("score", "REAL", DataTypeKind::Float, true),
        ];
        let sql =
            generated_create_table_sql(Engine::Sqlite, &table_scope("main"), "t", &cols, true);
        assert_eq!(
            sql,
            "CREATE TABLE \"main\".\"t\" (\"name\" VARCHAR(120), \"score\" REAL)"
        );
    }

    #[test]
    fn add_column_keeps_not_null_and_native_type() {
        let c = col("age", "INTEGER", DataTypeKind::Int, false);
        let sql = add_column_sql(Engine::Sqlite, &table_scope("main"), "users", &c, true);
        assert_eq!(
            sql,
            "ALTER TABLE \"main\".\"users\" ADD COLUMN \"age\" INTEGER NOT NULL"
        );
    }

    #[test]
    fn alter_column_per_engine() {
        let c = col("age", "BIGINT", DataTypeKind::Int, false);
        assert_eq!(
            alter_column_sql(Engine::MySql, &table_scope("shop"), "users", &c, false).unwrap(),
            "ALTER TABLE `shop`.`users` MODIFY COLUMN `age` BIGINT NOT NULL"
        );
        assert_eq!(
            alter_column_sql(Engine::Postgres, &pg_scope("public"), "users", &c, false).unwrap(),
            "ALTER TABLE \"public\".\"users\" ALTER COLUMN \"age\" TYPE bigint"
        );
        assert!(alter_column_sql(Engine::Sqlite, &table_scope("main"), "users", &c, true).is_none());
    }

    // --- structure: indexes / FKs ------------------------------------------

    #[test]
    fn index_statements_quote_per_engine() {
        let index = idx("ux_email", &["email"], true, false);
        assert_eq!(
            create_index_sql(Engine::MySql, &table_scope("shop"), "users", &index),
            "CREATE UNIQUE INDEX `ux_email` ON `shop`.`users` (`email`)"
        );
        assert_eq!(
            create_index_sql(Engine::Sqlite, &table_scope("main"), "users", &index),
            "CREATE UNIQUE INDEX \"main\".\"ux_email\" ON \"users\" (\"email\")"
        );
        assert_eq!(
            drop_index_sql(Engine::Postgres, &pg_scope("public"), "users", "ux_email"),
            "DROP INDEX \"public\".\"ux_email\""
        );
        assert_eq!(
            drop_index_sql(Engine::MySql, &table_scope("shop"), "users", "ux_email"),
            "DROP INDEX `ux_email` ON `shop`.`users`"
        );
        assert_eq!(
            drop_index_sql(Engine::Sqlite, &table_scope("main"), "users", "ux_email"),
            "DROP INDEX \"main\".\"ux_email\""
        );
    }

    #[test]
    fn index_diff_reports_add_and_drop() {
        let source = vec![idx("ix_a", &["a"], false, false)];
        let target = vec![idx("ix_b", &["b"], false, false)];
        let diff = diff_indexes(
            "t",
            &source,
            &target,
            Engine::MySql,
            &table_scope("shop"),
            &HashSet::new(),
        );
        assert_eq!(diff.added.len(), 1);
        assert_eq!(diff.removed.len(), 1);
        assert_eq!(diff.add_statements.len(), 1);
        assert_eq!(diff.drop_statements.len(), 1);
        assert!(diff.warnings.is_empty());
    }

    #[test]
    fn index_diff_skips_internal_and_constraint_backed_indexes() {
        let source = vec![idx("PRIMARY", &["id"], true, true)];
        let target = vec![idx("sqlite_autoindex_t_1", &["id"], true, false)];
        let diff = diff_indexes(
            "t",
            &source,
            &target,
            Engine::Sqlite,
            &table_scope("main"),
            &HashSet::new(),
        );
        assert!(diff.added.is_empty() && diff.removed.is_empty());
        assert!(diff.add_statements.is_empty() && diff.drop_statements.is_empty());

        let source = vec![];
        let target = vec![idx("users_email_key", &["email"], true, false)];
        let non_droppable: HashSet<String> = ["users_email_key".to_string()].into_iter().collect();
        let diff = diff_indexes(
            "users",
            &source,
            &target,
            Engine::Postgres,
            &pg_scope("public"),
            &non_droppable,
        );
        assert_eq!(diff.removed.len(), 1);
        assert!(diff.drop_statements.is_empty());
        assert_eq!(diff.warnings.len(), 1);
    }

    #[test]
    fn index_diff_warns_on_same_name_different_shape() {
        let source = vec![idx("ix", &["a"], false, false)];
        let target = vec![idx("ix", &["b"], false, false)];
        let diff = diff_indexes(
            "t",
            &source,
            &target,
            Engine::MySql,
            &table_scope("shop"),
            &HashSet::new(),
        );
        assert!(diff.added.is_empty() && diff.removed.is_empty());
        assert_eq!(diff.warnings.len(), 1);
    }

    #[test]
    fn fk_diff_lines_format() {
        let source = vec![
            fk("fk_a", &["a_id"], "a", &["id"]),
            fk("fk_both", &["b_id"], "b", &["id"]),
        ];
        let target = vec![
            fk("fk_both", &["b_id"], "b", &["id"]),
            fk("fk_c", &["c_id"], "c", &["id"]),
        ];
        let lines = fk_diff_lines(&source, &target);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], "+ fk_a (a_id) -> a(id)");
        assert_eq!(lines[1], "- fk_c (c_id) -> c(id)");
    }

    #[test]
    fn statement_buckets_follow_contract_order() {
        let mut buckets = StatementBuckets::default();
        let stmt = |kind| SyncStatement {
            table: "t".into(),
            sql: String::new(),
            kind,
            destructive: false,
        };
        buckets.drop_tables.push(stmt(SyncStatementKind::DropTable));
        buckets.add_columns.push(stmt(SyncStatementKind::AddColumn));
        buckets.create_tables.push(stmt(SyncStatementKind::CreateTable));
        buckets.drop_indexes.push(stmt(SyncStatementKind::DropIndex));
        buckets.alter_columns.push(stmt(SyncStatementKind::AlterColumn));
        buckets.add_indexes.push(stmt(SyncStatementKind::AddIndex));
        buckets.drop_columns.push(stmt(SyncStatementKind::DropColumn));

        let kinds: Vec<SyncStatementKind> = buckets.ordered().into_iter().map(|s| s.kind).collect();
        assert_eq!(
            kinds,
            vec![
                SyncStatementKind::CreateTable,
                SyncStatementKind::AddColumn,
                SyncStatementKind::AlterColumn,
                SyncStatementKind::AddIndex,
                SyncStatementKind::DropIndex,
                SyncStatementKind::DropColumn,
                SyncStatementKind::DropTable,
            ]
        );
    }

    // --- data: row diff -----------------------------------------------------

    #[test]
    fn diff_rows_finds_insert_update_delete() {
        let diff = diff_rows(
            src_rows(&[(1, "a"), (2, "b")]),
            tgt_rows(&[(2, "old"), (3, "c")]),
            100,
            true,
        );
        assert_eq!(diff.inserts.len(), 1);
        assert_eq!(diff.updates.len(), 1);
        assert_eq!(diff.deletes.len(), 1);
        assert_eq!(diff.unchanged, 0);
        assert!(!diff.statements_truncated && !diff.scan_truncated);
        // Insert carries the full source row; delete carries the target key.
        assert_eq!(
            diff.inserts[0].full_values,
            vec![Value::Int(1), Value::Text("a".into())]
        );
        assert_eq!(diff.deletes[0].key_values, vec![Value::Int(3)]);
    }

    #[test]
    fn diff_rows_counts_unchanged() {
        let diff = diff_rows(
            src_rows(&[(1, "a"), (2, "b")]),
            tgt_rows(&[(1, "a"), (2, "b")]),
            100,
            true,
        );
        assert_eq!(diff.unchanged, 2);
        assert!(diff.inserts.is_empty() && diff.updates.is_empty() && diff.deletes.is_empty());
    }

    #[test]
    fn diff_rows_stops_early_without_full_scan() {
        let diff = diff_rows(
            src_rows(&[(1, "a"), (2, "b"), (3, "c")]),
            tgt_rows(&[(9, "z")]),
            1,
            false,
        );
        assert_eq!(diff.inserts.len(), 1);
        assert!(diff.updates.is_empty());
        // The scan stopped before target-only keys could be determined.
        assert!(diff.deletes.is_empty());
        assert!(diff.scan_truncated);
        assert!(diff.statements_truncated);
    }

    #[test]
    fn diff_rows_full_scan_counts_everything_but_caps_statements() {
        let diff = diff_rows(
            src_rows(&[(1, "a"), (2, "b")]),
            tgt_rows(&[(9, "z")]),
            1,
            true,
        );
        // Counts are accurate...
        assert_eq!(diff.insert_count, 2);
        assert_eq!(diff.delete_count, 1);
        // ...but the capped list kept the first insert only.
        assert_eq!(diff.inserts.len(), 1);
        assert!(diff.deletes.is_empty());
        assert!(diff.statements_truncated);
        assert!(!diff.scan_truncated);

        let diff = diff_rows(src_rows(&[(1, "a"), (2, "b")]), tgt_rows(&[]), 1, true);
        assert_eq!(diff.insert_count, 2);
        assert_eq!(diff.inserts.len(), 1);
        assert!(diff.statements_truncated);
    }

    #[test]
    fn row_state_uses_unit_separator_and_text() {
        let state = RowState::new(
            vec![Value::Int(1), Value::Text("x".into())],
            vec![Value::Null, Value::Text("".into())],
            vec![],
        );
        assert_eq!(state.key, format!("1{UNIT_SEP}x"));
        assert_eq!(state.signature, format!("{UNIT_SEP}"));
    }

    // --- data: column resolution -------------------------------------------

    #[test]
    fn resolve_columns_falls_back_to_primary_key() {
        let mut id = col("id", "INTEGER", DataTypeKind::Int, false);
        id.is_primary_key = true;
        let source = vec![id, col("name", "TEXT", DataTypeKind::Text, true)];
        let target = source.clone();
        let mut warnings = Vec::new();
        let resolved =
            resolve_data_columns(&source, &target, &[], &[], &mut warnings).unwrap();
        assert_eq!(resolved.keys, vec!["id"]);
        assert_eq!(resolved.keys_target, vec!["id"]);
        assert_eq!(resolved.compare, vec!["name"]);
        assert_eq!(resolved.insert, vec!["id", "name"]);
        assert!(warnings.is_empty());
    }

    #[test]
    fn resolve_columns_requires_a_key_when_no_pk() {
        let source = vec![col("name", "TEXT", DataTypeKind::Text, true)];
        let target = source.clone();
        let err = resolve_data_columns(&source, &target, &[], &[], &mut Vec::new()).unwrap_err();
        assert_eq!(err.code, ErrorCode::Data);
    }

    #[test]
    fn resolve_columns_rejects_unknown_key() {
        let source = vec![col("id", "INTEGER", DataTypeKind::Int, true)];
        let target = source.clone();
        let err = resolve_data_columns(
            &source,
            &target,
            &["missing".to_string()],
            &[],
            &mut Vec::new(),
        )
        .unwrap_err();
        assert_eq!(err.code, ErrorCode::NotFound);
    }

    #[test]
    fn resolve_columns_skips_columns_missing_on_target() {
        let source = vec![
            col("id", "INTEGER", DataTypeKind::Int, true),
            col("extra", "TEXT", DataTypeKind::Text, true),
            col("name", "TEXT", DataTypeKind::Text, true),
        ];
        let target = vec![
            col("id", "INTEGER", DataTypeKind::Int, true),
            col("name", "TEXT", DataTypeKind::Text, true),
        ];
        let mut warnings = Vec::new();
        let resolved = resolve_data_columns(
            &source,
            &target,
            &["id".to_string()],
            &["NAME".to_string()],
            &mut warnings,
        )
        .unwrap();
        assert_eq!(resolved.keys, vec!["id"]);
        assert_eq!(resolved.insert, vec!["id", "name"]);
        assert_eq!(resolved.compare, vec!["name"]);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("extra"));
    }

    // --- data: statement builders -------------------------------------------

    #[test]
    fn insert_update_delete_sql_use_target_engine_literals() {
        let scope = table_scope("shop");
        let insert = insert_row_sql(
            Engine::MySql,
            &scope,
            "t",
            &["id".to_string(), "name".to_string()],
            &[Value::Int(1), Value::Text("O'Brien".into())],
        );
        assert_eq!(
            insert,
            "INSERT INTO `shop`.`t` (`id`, `name`) VALUES (1, 'O''Brien');"
        );

        let update = update_row_sql(
            Engine::Postgres,
            &pg_scope("public"),
            "t",
            &["name".to_string()],
            &[Value::Text("new".into())],
            &["id".to_string()],
            &[Value::Int(1)],
        );
        assert_eq!(
            update,
            "UPDATE \"public\".\"t\" SET \"name\" = 'new' WHERE \"id\" = 1;"
        );

        let delete = delete_row_sql(
            Engine::Sqlite,
            &table_scope("main"),
            "t",
            &["id".to_string()],
            &[Value::Int(7)],
        );
        assert_eq!(delete, "DELETE FROM \"main\".\"t\" WHERE \"id\" = 7;");
    }

    #[test]
    fn composite_keys_generate_conjunctive_where() {
        let sql = delete_row_sql(
            Engine::Sqlite,
            &table_scope("main"),
            "t",
            &["a".to_string(), "b".to_string()],
            &[Value::Int(1), Value::Text("x".into())],
        );
        assert_eq!(
            sql,
            "DELETE FROM \"main\".\"t\" WHERE \"a\" = 1 AND \"b\" = 'x';"
        );
    }

    // --- misc ----------------------------------------------------------------

    #[test]
    fn skippable_statements() {
        assert!(is_skippable_statement(""));
        assert!(is_skippable_statement("   "));
        assert!(is_skippable_statement("-- a comment"));
        assert!(is_skippable_statement("  -- comment\nSELECT 1"));
        assert!(!is_skippable_statement("SELECT 1"));
    }

    #[test]
    fn sample_len_truncation_semantics() {
        assert_eq!(take_sample_len(10, 5), (5, true));
        assert_eq!(take_sample_len(5, 5), (5, false));
        assert_eq!(take_sample_len(0, 5), (0, false));
    }

    #[test]
    fn trim_statement_removes_trailing_semicolon() {
        assert_eq!(trim_statement("CREATE TABLE t (a INT);"), "CREATE TABLE t (a INT)");
        assert_eq!(trim_statement("  CREATE TABLE t (a INT)  "), "CREATE TABLE t (a INT)");
    }

    #[test]
    fn unique_names_deduplicates_case_insensitively() {
        let a = vec!["ID".to_string(), "name".to_string()];
        let b = vec!["id".to_string(), "extra".to_string()];
        assert_eq!(unique_names(&a, &b), vec!["ID", "name", "extra"]);
    }
}
