//! Schema Snapshot: fetch tables + columns + indexes + foreign keys for a
//! scope in one call. Feeds the ER diagram, schema-aware autocomplete and any
//! offline analysis without N round-trips from the frontend.
//!
//! Part of the v1 feature pack — frozen contract.

use std::time::Instant;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use mieosql_core::{ColumnDef, Engine, ForeignKeyDef, IndexDef, ObjectKind, Scope};

use crate::dto::ApiResult;
use crate::service::Api;

/// One table with everything attached.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TableSnapshot {
    pub name: String,
    pub kind: ObjectKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub comment: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    #[ts(type = "number | null")]
    pub rows_estimate: Option<u64>,
    pub columns: Vec<ColumnDef>,
    pub indexes: Vec<IndexDef>,
    pub foreign_keys: Vec<ForeignKeyDef>,
}

/// Everything the ER diagram / completion cache needs for one scope.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SchemaSnapshot {
    pub engine: Engine,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub database: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub schema: Option<String>,
    /// RFC 3339 timestamp of when the snapshot was taken (for cache headers).
    pub generated_at: String,
    #[ts(type = "number")]
    pub elapsed_ms: u64,
    /// True when `max_tables` cut the table list.
    pub truncated: bool,
    pub tables: Vec<TableSnapshot>,
}

/// Default table budget when the caller does not pick one.
const DEFAULT_MAX_TABLES: u32 = 400;
/// Hard cap to keep the payload sane.
const HARD_MAX_TABLES: u32 = 2_000;

impl Api {
    /// Take a full snapshot of `scope`.
    ///
    /// `max_tables` defaults to 400 (hard cap 2000) to keep the payload sane.
    /// Introspection is sequential by design: one session serializes on a
    /// single driver anyway.
    pub async fn schema_snapshot(
        &self,
        session_id: String,
        scope: Scope,
        include_views: bool,
        max_tables: Option<u32>,
    ) -> ApiResult<SchemaSnapshot> {
        let started = Instant::now();
        let info = self.session_info(session_id.clone()).await?;
        let limit = max_tables.unwrap_or(DEFAULT_MAX_TABLES).clamp(1, HARD_MAX_TABLES) as usize;

        let mut objects = self
            .list_objects(
                session_id.clone(),
                scope.clone(),
                Some(ObjectKind::Table),
                None,
            )
            .await?;
        if include_views {
            objects.extend(
                self.list_objects(session_id.clone(), scope.clone(), Some(ObjectKind::View), None)
                    .await?,
            );
        }

        // `truncated` must reflect the full catalog, not the kept slice.
        let truncated = objects.len() > limit;
        objects.truncate(limit);

        let mut tables: Vec<TableSnapshot> = Vec::with_capacity(objects.len());
        for object in objects {
            let mut columns = self
                .table_columns(session_id.clone(), scope.clone(), object.name.clone())
                .await?;
            columns.sort_by_key(|c| c.ordinal);

            let is_view = object.kind == ObjectKind::View;
            let (indexes, foreign_keys) = if is_view {
                // Some engines do not expose indexes/FKs for views; a missing
                // answer must not fail the whole snapshot.
                (
                    self.table_indexes(session_id.clone(), scope.clone(), object.name.clone())
                        .await
                        .unwrap_or_default(),
                    self.foreign_keys(session_id.clone(), scope.clone(), object.name.clone())
                        .await
                        .unwrap_or_default(),
                )
            } else {
                (
                    self.table_indexes(session_id.clone(), scope.clone(), object.name.clone())
                        .await?,
                    self.foreign_keys(session_id.clone(), scope.clone(), object.name.clone())
                        .await?,
                )
            };

            tables.push(TableSnapshot {
                name: object.name,
                kind: object.kind,
                comment: object.comment,
                rows_estimate: object.rows_estimate,
                columns,
                indexes,
                foreign_keys,
            });
        }

        Ok(SchemaSnapshot {
            engine: info.engine,
            database: scope.database.clone().or(info.database.clone()),
            schema: scope.schema.clone(),
            generated_at: chrono::Utc::now().to_rfc3339(),
            elapsed_ms: started.elapsed().as_millis() as u64,
            truncated,
            tables,
        })
    }
}
