//! PostgreSQL driver (sqlx).

use std::str::FromStr;
use std::time::Duration;

use chrono::{DateTime, NaiveDate, NaiveDateTime, NaiveTime, Utc};
use sqlx::postgres::types::PgInterval;
use sqlx::postgres::{
    PgArguments, PgConnectOptions, PgPool, PgPoolOptions, PgRow, PgSslMode, Postgres,
};
use sqlx::query::Query;
use sqlx::types::{BigDecimal, Json};
use sqlx::{ConnectOptions, Row, Transaction, TypeInfo, ValueRef};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use mieosql_core::{
    Capabilities, ColumnDef, ConnectionSpec, CreateDbUser, DataTypeKind, DatabaseRef,
    DbPrincipal, DbPrincipalDetail, DetailAttribute, DropDbPrincipal, Engine, ErrorCode,
    ForeignKeyDef, GrantRequest, GrantTarget, IndexDef, MieoError, MieoResult, ObjectKind,
    ObjectRef, PlaceholderStyle, PrincipalKind, PrivilegeEntry, QueryEvent, QueryOptions,
    QueryStats, RevokeRequest, RoleGrant, SchemaRef, Scope, ServerInfo, Value,
};

use crate::error_map::map_err;
use crate::stream::{Binder, Decoder, KindFn, stream_query};
use crate::util::*;

pub struct PostgresDriver {
    pool: PgPool,
    tx: Option<Transaction<'static, Postgres>>,
    spec: ConnectionSpec,
}

pub(crate) fn postgres_capabilities() -> Capabilities {
    Capabilities {
        engine_kind: mieosql_core::EngineKind::Sql,
        placeholder: PlaceholderStyle::DollarNumbered,
        list_databases: true,
        list_schemas: true,
        schema_is_database: false,
        transactions: true,
        simple_mode: true,
        ddl: true,
        multiple_databases: true,
        user_management: true,
    }
}

impl PostgresDriver {
    pub async fn connect(spec: &ConnectionSpec) -> MieoResult<Self> {
        if spec.ssh.is_some() {
            return Err(MieoError::unsupported(
                "SSH tunnels are not implemented yet (planned phase P1) — refusing to connect directly",
            ));
        }
        let host = spec.host.clone().unwrap_or_else(|| "127.0.0.1".into());
        let port = spec.port.unwrap_or(5432);

        let mut opts = PgConnectOptions::new()
            .host(&host)
            .port(port)
            .application_name("MieoSQL")
            .ssl_mode(match spec.ssl_mode {
                mieosql_core::SslMode::Disable => PgSslMode::Disable,
                mieosql_core::SslMode::Prefer => PgSslMode::Prefer,
                mieosql_core::SslMode::Require => PgSslMode::Require,
                mieosql_core::SslMode::VerifyCa => PgSslMode::VerifyCa,
                mieosql_core::SslMode::VerifyFull => PgSslMode::VerifyFull,
            });
        if let Some(u) = &spec.username {
            opts = opts.username(u);
        }
        if let Some(p) = &spec.password {
            opts = opts.password(p);
        }
        if let Some(db) = spec.database.as_deref().filter(|d| !d.is_empty()) {
            opts = opts.database(db);
        }
        opts = opts.disable_statement_logging();

        let pool = PgPoolOptions::new()
            .max_connections(4)
            .acquire_timeout(Duration::from_secs(spec.connect_timeout_secs))
            .idle_timeout(None)
            .max_lifetime(None)
            .connect_with(opts)
            .await
            .map_err(map_err)?;

        Ok(Self {
            pool,
            tx: None,
            spec: spec.clone(),
        })
    }

    pub fn spec(&self) -> &ConnectionSpec {
        &self.spec
    }

    pub fn in_transaction(&self) -> bool {
        self.tx.is_some()
    }

    pub async fn ping(&self) -> MieoResult<()> {
        sqlx::query::<Postgres>("SELECT 1")
            .fetch_one(&self.pool)
            .await
            .map_err(map_err)?;
        Ok(())
    }

    pub async fn server_info(&self) -> MieoResult<ServerInfo> {
        let row = sqlx::query::<Postgres>("SELECT version(), current_database(), current_user")
            .fetch_one(&self.pool)
            .await
            .map_err(map_err)?;
        Ok(ServerInfo {
            engine: Engine::Postgres,
            version: s(&row, 0),
            version_comment: None,
            current_database: os(&row, 1),
            current_user: os(&row, 2),
        })
    }

    pub async fn list_databases(&self) -> MieoResult<Vec<DatabaseRef>> {
        let rows = sqlx::query::<Postgres>(
            "SELECT datname, datistemplate FROM pg_database ORDER BY datname",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(map_err)?;
        Ok(rows
            .into_iter()
            .map(|row| {
                let name = s(&row, 0);
                let is_template = row.try_get::<bool, _>(1).unwrap_or(false);
                DatabaseRef {
                    is_system: is_template || name == "postgres",
                    name,
                    comment: None,
                    size_bytes: None,
                    charset: None,
                    collation: None,
                }
            })
            .collect())
    }

    pub async fn list_schemas(&self, _database: Option<&str>) -> MieoResult<Vec<SchemaRef>> {
        let rows = sqlx::query::<Postgres>("SELECT nspname FROM pg_namespace ORDER BY nspname")
            .fetch_all(&self.pool)
            .await
            .map_err(map_err)?;
        Ok(rows
            .into_iter()
            .map(|row| {
                let name = s(&row, 0);
                let is_system =
                    name == "pg_catalog" || name == "information_schema" || name.starts_with("pg_");
                SchemaRef { name, is_system }
            })
            .collect())
    }

    pub async fn list_objects(
        &self,
        scope: &Scope,
        kind: Option<ObjectKind>,
        pattern: Option<&str>,
    ) -> MieoResult<Vec<ObjectRef>> {
        // Stored routines + triggers live in pg_proc / pg_trigger, not pg_class.
        if matches!(kind, Some(ObjectKind::Function | ObjectKind::Procedure)) {
            let prokind = if kind == Some(ObjectKind::Function) { "f" } else { "p" };
            let mut rsql = String::from(
                "SELECT n.nspname, p.proname FROM pg_proc p                  JOIN pg_namespace n ON n.oid = p.pronamespace                  WHERE p.prokind = $1                    AND n.nspname NOT IN ('pg_catalog', 'information_schema')                    AND n.nspname NOT LIKE 'pg\\_%'",
            );
            let mut params: Vec<String> = vec![prokind.to_string()];
            if let Some(sc) = scope.schema.as_deref().filter(|s| !s.is_empty()) {
                params.push(sc.to_string());
                rsql.push_str(&format!(" AND n.nspname = ${}", params.len()));
            }
            if let Some(p) = pattern {
                params.push(format!("%{p}%"));
                rsql.push_str(&format!(" AND p.proname LIKE ${}", params.len()));
            }
            // Overloads share a name — the tree is keyed by name, so dedupe.
            rsql.push_str(" GROUP BY n.nspname, p.proname ORDER BY n.nspname, p.proname");
            let mut q = sqlx::query::<Postgres>(sqlx::AssertSqlSafe(rsql));
            for p in &params {
                q = q.bind(p.clone());
            }
            let rows = q.fetch_all(&self.pool).await.map_err(map_err)?;
            let rkind = if prokind == "f" { ObjectKind::Function } else { ObjectKind::Procedure };
            return Ok(rows
                .into_iter()
                .map(|row| ObjectRef {
                    name: s(&row, 1),
                    kind: rkind,
                    database: self.spec.database.clone(),
                    schema: Some(s(&row, 0)),
                    comment: None,
                    rows_estimate: None,
                    size_bytes: None,
                    updated_at: None,
                })
                .collect());
        }
        if kind == Some(ObjectKind::Trigger) {
            let mut tsql = String::from(
                "SELECT n.nspname, t.tgname, c.relname FROM pg_trigger t                  JOIN pg_class c ON c.oid = t.tgrelid                  JOIN pg_namespace n ON n.oid = c.relnamespace                  WHERE NOT t.tgisinternal                    AND n.nspname NOT IN ('pg_catalog', 'information_schema')                    AND n.nspname NOT LIKE 'pg\\_%'",
            );
            let mut params: Vec<String> = Vec::new();
            if let Some(sc) = scope.schema.as_deref().filter(|s| !s.is_empty()) {
                params.push(sc.to_string());
                tsql.push_str(&format!(" AND n.nspname = ${}", params.len()));
            }
            if let Some(p) = pattern {
                params.push(format!("%{p}%"));
                tsql.push_str(&format!(" AND t.tgname LIKE ${}", params.len()));
            }
            tsql.push_str(" ORDER BY n.nspname, t.tgname");
            let mut q = sqlx::query::<Postgres>(sqlx::AssertSqlSafe(tsql));
            for p in &params {
                q = q.bind(p.clone());
            }
            let rows = q.fetch_all(&self.pool).await.map_err(map_err)?;
            return Ok(rows
                .into_iter()
                .map(|row| ObjectRef {
                    name: s(&row, 1),
                    kind: ObjectKind::Trigger,
                    database: self.spec.database.clone(),
                    schema: Some(s(&row, 0)),
                    comment: Some(format!("ON {}", s(&row, 2))),
                    rows_estimate: None,
                    size_bytes: None,
                    updated_at: None,
                })
                .collect());
        }
        let kind_filter: Option<Vec<&str>> = match kind {
            Some(ObjectKind::Table) => Some(vec!["r", "p"]),
            Some(ObjectKind::View) => Some(vec!["v"]),
            Some(ObjectKind::MaterializedView) => Some(vec!["m"]),
            Some(ObjectKind::Sequence) => Some(vec!["S"]),
            Some(ObjectKind::Function | ObjectKind::Procedure | ObjectKind::Trigger) => {
                unreachable!("handled above")
            }
            None => None,
        };

        let mut sql = String::from(
            "SELECT n.nspname, c.relname, c.relkind::text, obj_description(c.oid, 'pg_class'), \
                    CASE WHEN c.relkind IN ('r','p') THEN GREATEST(c.reltuples, 0)::bigint ELSE NULL END, \
                    CASE WHEN c.relkind IN ('r','p') THEN pg_total_relation_size(c.oid) ELSE NULL END \
             FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
             WHERE c.relkind IN ('r','p','v','m','S') \
               AND n.nspname NOT IN ('pg_catalog', 'information_schema') \
               AND n.nspname NOT LIKE 'pg\\_%'",
        );
        let mut params: Vec<String> = Vec::new();
        if let Some(sc) = scope.schema.as_deref().filter(|s| !s.is_empty()) {
            params.push(sc.to_string());
            sql.push_str(&format!(" AND n.nspname = ${}", params.len()));
        }
        if let Some(kf) = kind_filter {
            params.push(kf.join(","));
            sql.push_str(&format!(
                " AND c.relkind::text = ANY(string_to_array(${}, ','))",
                params.len()
            ));
        }
        if let Some(p) = pattern {
            params.push(format!("%{p}%"));
            sql.push_str(&format!(" AND c.relname LIKE ${}", params.len()));
        }
        sql.push_str(" ORDER BY n.nspname, c.relname");

        let mut q = sqlx::query::<Postgres>(sqlx::AssertSqlSafe(sql));
        for p in &params {
            q = q.bind(p.clone());
        }
        let rows = q.fetch_all(&self.pool).await.map_err(map_err)?;

        Ok(rows
            .into_iter()
            .map(|row| {
                let relkind = s(&row, 2);
                let kind = match relkind.as_str() {
                    "v" => ObjectKind::View,
                    "m" => ObjectKind::MaterializedView,
                    "S" => ObjectKind::Sequence,
                    _ => ObjectKind::Table,
                };
                ObjectRef {
                    name: s(&row, 1),
                    kind,
                    database: self.spec.database.clone(),
                    schema: Some(s(&row, 0)),
                    comment: os(&row, 3).filter(|c| !c.is_empty()),
                    rows_estimate: i64o(&row, 4).filter(|v| *v >= 0).map(|v| v as u64),
                    size_bytes: i64o(&row, 5).filter(|v| *v > 0).map(|v| v as u64),
                    updated_at: None,
                }
            })
            .collect())
    }

    pub async fn table_columns(&self, scope: &Scope, table: &str) -> MieoResult<Vec<ColumnDef>> {
        let schema = self.resolve_schema(scope);

        // Primary key columns, resolved via `to_regclass` on a properly quoted name.
        let qualified = format!("{}.{}", quote_ansi_ident(&schema), quote_ansi_ident(table));
        let pk_columns: Vec<String> = sqlx::query::<Postgres>(
            "SELECT a.attname FROM pg_index i \
             JOIN pg_attribute a ON a.attrelid = i.indrelid AND a.attnum = ANY(i.indkey) \
             WHERE i.indrelid = to_regclass($1) AND i.indisprimary",
        )
        .bind(&qualified)
        .fetch_all(&self.pool)
        .await
        .map_err(map_err)?
        .into_iter()
        .map(|r| s(&r, 0))
        .collect();

        let rows = sqlx::query::<Postgres>(
            "SELECT a.attname, \
                    format_type(a.atttypid, a.atttypmod) AS native_type, \
                    a.attnotnull, \
                    pg_get_expr(ad.adbin, ad.adrelid) AS default_value, \
                    btrim(a.attidentity::text) AS identity, \
                    btrim(a.attgenerated::text) AS generated, \
                    col_description(a.attrelid, a.attnum) AS comment, \
                    t.typname, \
                    t.typcategory::text \
             FROM pg_attribute a \
             JOIN pg_class c ON c.oid = a.attrelid \
             JOIN pg_namespace n ON n.oid = c.relnamespace \
             JOIN pg_type t ON t.oid = a.atttypid \
             LEFT JOIN pg_attrdef ad ON ad.adrelid = a.attrelid AND ad.adnum = a.attnum \
             WHERE n.nspname = $1 AND c.relname = $2 AND a.attnum > 0 AND NOT a.attisdropped \
             ORDER BY a.attnum",
        )
        .bind(&schema)
        .bind(table)
        .fetch_all(&self.pool)
        .await
        .map_err(map_err)?;

        Ok(rows
            .into_iter()
            .enumerate()
            .map(|(i, row)| {
                let name = s(&row, 0);
                let native_type = s(&row, 1);
                let typname = s(&row, 7);
                let typcategory = s(&row, 8);
                let default_value = os(&row, 3).filter(|d| !d.is_empty());
                let identity = s(&row, 4);
                let generated = s(&row, 5);
                let data_type = if typcategory == "A" {
                    DataTypeKind::Array(Box::new(kind_pg(typname.trim_start_matches('_'))))
                } else {
                    kind_pg(&typname)
                };
                ColumnDef {
                    name: name.clone(),
                    ordinal: (i + 1) as u32,
                    data_type,
                    nullable: !row.try_get::<bool, _>(2).unwrap_or(false),
                    default_value: default_value.clone(),
                    is_primary_key: pk_columns.contains(&name),
                    is_auto_increment: identity == "a"
                        || identity == "d"
                        || default_value
                            .as_deref()
                            .is_some_and(|d| d.starts_with("nextval(")),
                    is_generated: !generated.is_empty(),
                    comment: os(&row, 6).filter(|c| !c.is_empty()),
                    char_max_length: None,
                    numeric_precision: None,
                    numeric_scale: None,
                    collation: None,
                    native_type,
                }
            })
            .collect())
    }

    pub async fn table_indexes(&self, scope: &Scope, table: &str) -> MieoResult<Vec<IndexDef>> {
        let schema = self.resolve_schema(scope);
        let rows = sqlx::query::<Postgres>(
            "SELECT i.relname, ix.indisunique, ix.indisprimary, am.amname, \
                    (SELECT array_agg(pg_get_indexdef(ix.indexrelid, k::int, true) ORDER BY k) \
                     FROM generate_series(1, ix.indnkeyatts) AS k) AS columns \
             FROM pg_index ix \
             JOIN pg_class i ON i.oid = ix.indexrelid \
             JOIN pg_class t ON t.oid = ix.indrelid \
             JOIN pg_namespace n ON n.oid = t.relnamespace \
             JOIN pg_am am ON am.oid = i.relam \
             WHERE n.nspname = $1 AND t.relname = $2 \
             ORDER BY i.relname",
        )
        .bind(&schema)
        .bind(table)
        .fetch_all(&self.pool)
        .await
        .map_err(map_err)?;

        Ok(rows
            .into_iter()
            .map(|row| {
                let columns: Vec<String> = row
                    .try_get::<Vec<Option<String>>, _>(4)
                    .unwrap_or_default()
                    .into_iter()
                    .flatten()
                    .collect();
                IndexDef {
                    name: s(&row, 0),
                    is_unique: row.try_get::<bool, _>(1).unwrap_or(false),
                    is_primary: row.try_get::<bool, _>(2).unwrap_or(false),
                    index_type: os(&row, 3),
                    columns,
                    comment: None,
                }
            })
            .collect())
    }

    pub async fn foreign_keys(&self, scope: &Scope, table: &str) -> MieoResult<Vec<ForeignKeyDef>> {
        let schema = self.resolve_schema(scope);
        let rows = sqlx::query::<Postgres>(
            "SELECT con.conname, \
                    (SELECT array_agg(a.attname ORDER BY k.ord) \
                     FROM unnest(con.conkey) WITH ORDINALITY AS k(attnum, ord) \
                     JOIN pg_attribute a ON a.attrelid = con.conrelid AND a.attnum = k.attnum) AS cols, \
                    rt.relname AS ref_table, \
                    rn.nspname AS ref_schema, \
                    (SELECT array_agg(a.attname ORDER BY k.ord) \
                     FROM unnest(con.confkey) WITH ORDINALITY AS k(attnum, ord) \
                     JOIN pg_attribute a ON a.attrelid = con.confrelid AND a.attnum = k.attnum) AS ref_cols, \
                    con.confupdtype::text, \
                    con.confdeltype::text \
             FROM pg_constraint con \
             JOIN pg_class t ON t.oid = con.conrelid \
             JOIN pg_namespace n ON n.oid = t.relnamespace \
             JOIN pg_class rt ON rt.oid = con.confrelid \
             JOIN pg_namespace rn ON rn.oid = rt.relnamespace \
             WHERE con.contype = 'f' AND n.nspname = $1 AND t.relname = $2 \
             ORDER BY con.conname",
        )
        .bind(&schema)
        .bind(table)
        .fetch_all(&self.pool)
        .await
        .map_err(map_err)?;

        Ok(rows
            .into_iter()
            .map(|row| {
                let arr = |idx: usize| -> Vec<String> {
                    row.try_get::<Vec<Option<String>>, _>(idx)
                        .unwrap_or_default()
                        .into_iter()
                        .flatten()
                        .collect()
                };
                ForeignKeyDef {
                    name: s(&row, 0),
                    columns: arr(1),
                    ref_table: s(&row, 2),
                    ref_schema: os(&row, 3),
                    ref_columns: arr(4),
                    on_update: map_fk_action(&s(&row, 5)),
                    on_delete: map_fk_action(&s(&row, 6)),
                }
            })
            .collect())
    }

    pub async fn object_ddl(
        &self,
        scope: &Scope,
        kind: ObjectKind,
        name: &str,
    ) -> MieoResult<Option<String>> {
        let schema = self.resolve_schema(scope);
        let qualified = format!("{}.{}", quote_ansi_ident(&schema), quote_ansi_ident(name));
        match kind {
            ObjectKind::Table => self.table_ddl(&schema, name, &qualified).await,
            ObjectKind::View => {
                let def: Option<String> = sqlx::query::<Postgres>(
                    "SELECT pg_get_viewdef(to_regclass($1), true)",
                )
                .bind(format!("{schema}.{name}"))
                .fetch_optional(&self.pool)
                .await
                .map_err(map_err)?
                .and_then(|row| row.try_get::<Option<String>, _>(0).ok().flatten());
                match def {
                    Some(d) if !d.trim().is_empty() => Ok(Some(format!(
                        "CREATE OR REPLACE VIEW {qualified} AS\n{d};"
                    ))),
                    _ => Ok(None),
                }
            }
            ObjectKind::MaterializedView => {
                let def: Option<String> = sqlx::query::<Postgres>(
                    "SELECT definition FROM pg_matviews WHERE schemaname = $1 AND matviewname = $2",
                )
                .bind(&schema)
                .bind(name)
                .fetch_optional(&self.pool)
                .await
                .map_err(map_err)?
                .and_then(|row| row.try_get::<Option<String>, _>(0).ok().flatten());
                match def {
                    Some(d) if !d.trim().is_empty() => Ok(Some(format!(
                        "CREATE MATERIALIZED VIEW {qualified} AS\n{d};"
                    ))),
                    _ => {
                        // Fallback to pg_get_viewdef which also works for matviews.
                        let fallback: Option<String> = sqlx::query::<Postgres>(
                            "SELECT pg_get_viewdef(to_regclass($1), true)",
                        )
                        .bind(format!("{schema}.{name}"))
                        .fetch_optional(&self.pool)
                        .await
                        .map_err(map_err)?
                        .and_then(|row| row.try_get::<Option<String>, _>(0).ok().flatten());
                        Ok(fallback
                            .filter(|d| !d.trim().is_empty())
                            .map(|d| format!("CREATE MATERIALIZED VIEW {qualified} AS\n{d};")))
                    }
                }
            }
            ObjectKind::Sequence => self.sequence_ddl(&schema, name, &qualified).await,
            ObjectKind::Function | ObjectKind::Procedure => {
                let rows = sqlx::query::<Postgres>(
                    "SELECT pg_get_functiondef(p.oid) FROM pg_proc p \
                     JOIN pg_namespace n ON n.oid = p.pronamespace \
                     WHERE n.nspname = $1 AND p.proname = $2 ORDER BY p.oid",
                )
                .bind(&schema)
                .bind(name)
                .fetch_all(&self.pool)
                .await
                .map_err(map_err)?;
                let defs: Vec<String> = rows
                    .into_iter()
                    .filter_map(|row| row.try_get::<Option<String>, _>(0).ok().flatten())
                    .filter(|d| !d.trim().is_empty())
                    .collect();
                if defs.is_empty() {
                    Ok(None)
                } else {
                    Ok(Some(defs.join("\n")))
                }
            }
            ObjectKind::Trigger => {
                let rows = sqlx::query::<Postgres>(
                    "SELECT pg_get_triggerdef(t.oid) FROM pg_trigger t \
                     JOIN pg_class c ON c.oid = t.tgrelid \
                     JOIN pg_namespace n ON n.oid = c.relnamespace \
                     WHERE NOT t.tgisinternal AND n.nspname = $1 AND c.relname = $2 \
                     ORDER BY t.tgname",
                )
                .bind(&schema)
                .bind(name)
                .fetch_all(&self.pool)
                .await
                .map_err(map_err)?;
                let defs: Vec<String> = rows
                    .into_iter()
                    .filter_map(|row| row.try_get::<Option<String>, _>(0).ok().flatten())
                    .filter(|d| !d.trim().is_empty())
                    .collect();
                if defs.is_empty() {
                    Ok(None)
                } else {
                    Ok(Some(
                        defs.into_iter().map(|d| format!("{d};")).collect::<Vec<_>>().join("\n"),
                    ))
                }
            }
        }
    }

    async fn table_ddl(
        &self,
        schema: &str,
        table: &str,
        qualified: &str,
    ) -> MieoResult<Option<String>> {
        let scope = Scope {
            database: None,
            schema: Some(schema.to_string()),
        };
        let columns = self.table_columns(&scope, table).await?;
        if columns.is_empty() {
            return Ok(None);
        }
        let mut lines: Vec<String> = Vec::new();
        for col in &columns {
            let mut def = format!("  {} {}", quote_ansi_ident(&col.name), col.native_type);
            if !col.nullable {
                def.push_str(" NOT NULL");
            }
            if let Some(d) = col.default_value.as_deref().filter(|d| !d.is_empty()) {
                def.push_str(&format!(" DEFAULT {d}"));
            }
            lines.push(def);
        }
        let pk: Vec<&str> = columns
            .iter()
            .filter(|c| c.is_primary_key)
            .map(|c| c.name.as_str())
            .collect();
        if !pk.is_empty() {
            let list = pk.iter().map(|n| quote_ansi_ident(n)).collect::<Vec<_>>().join(", ");
            lines.push(format!("  PRIMARY KEY ({list})"));
        }
        let mut ddl = format!("CREATE TABLE {qualified} (\n{}\n);", lines.join(",\n"));

        // Non-PK indexes as separate statements.
        if let Ok(indexes) = self.table_indexes(&scope, table).await {
            for idx in indexes.iter().filter(|i| !i.is_primary) {
                let cols = if idx.columns.is_empty() {
                    continue;
                } else {
                    idx.columns.join(", ")
                };
                let uniq = if idx.is_unique { "UNIQUE " } else { "" };
                ddl.push_str(&format!(
                    "\nCREATE {uniq}INDEX {} ON {qualified} ({cols});",
                    quote_ansi_ident(&idx.name)
                ));
            }
        }

        // Foreign keys as ALTER TABLE ... ADD CONSTRAINT.
        if let Ok(fks) = self.foreign_keys(&scope, table).await {
            for fk in &fks {
                if fk.columns.is_empty() || fk.ref_columns.is_empty() {
                    continue;
                }
                let cols = fk.columns.iter().map(|c| quote_ansi_ident(c)).collect::<Vec<_>>().join(", ");
                let ref_cols = fk.ref_columns.iter().map(|c| quote_ansi_ident(c)).collect::<Vec<_>>().join(", ");
                let ref_table = match fk.ref_schema.as_deref().filter(|s| !s.is_empty()) {
                    Some(rs) => format!("{}.{}", quote_ansi_ident(rs), quote_ansi_ident(&fk.ref_table)),
                    None => format!("{}.{}", quote_ansi_ident(schema), quote_ansi_ident(&fk.ref_table)),
                };
                let mut stmt = format!(
                    "\nALTER TABLE {qualified} ADD CONSTRAINT {} FOREIGN KEY ({cols}) REFERENCES {ref_table} ({ref_cols})",
                    quote_ansi_ident(&fk.name)
                );
                if let Some(rule) = fk.on_delete.as_deref() {
                    stmt.push_str(&format!(" ON DELETE {rule}"));
                }
                if let Some(rule) = fk.on_update.as_deref() {
                    stmt.push_str(&format!(" ON UPDATE {rule}"));
                }
                stmt.push(';');
                ddl.push_str(&stmt);
            }
        }

        // Table + column comments.
        let comment: Option<String> = sqlx::query::<Postgres>(
            "SELECT obj_description(to_regclass($1), 'pg_class')",
        )
        .bind(format!("{schema}.{table}"))
        .fetch_optional(&self.pool)
        .await
        .map_err(map_err)?
        .and_then(|row| row.try_get::<Option<String>, _>(0).ok().flatten());
        if let Some(c) = comment.filter(|c| !c.is_empty()) {
            ddl.push_str(&format!(
                "\nCOMMENT ON TABLE {qualified} IS {};",
                quote_literal(&c)
            ));
        }
        for col in &columns {
            if let Some(c) = col.comment.as_deref().filter(|c| !c.is_empty()) {
                ddl.push_str(&format!(
                    "\nCOMMENT ON COLUMN {qualified}.{} IS {};",
                    quote_ansi_ident(&col.name),
                    quote_literal(c)
                ));
            }
        }
        Ok(Some(ddl))
    }

    async fn sequence_ddl(
        &self,
        schema: &str,
        name: &str,
        qualified: &str,
    ) -> MieoResult<Option<String>> {
        let row = sqlx::query::<Postgres>(
            "SELECT start_value, minimum_value, maximum_value, increment_by, cycle_option, cache_size, data_type \
             FROM pg_sequences WHERE schemaname = $1 AND sequencename = $2",
        )
        .bind(schema)
        .bind(name)
        .fetch_optional(&self.pool)
        .await
        .map_err(map_err)?;
        let Some(row) = row else {
            return Ok(None);
        };
        let start: i64 = row.try_get::<i64, _>(0).unwrap_or(1);
        let min: i64 = row.try_get::<i64, _>(1).unwrap_or(1);
        let max: i64 = row.try_get::<i64, _>(2).unwrap_or(i64::MAX);
        let inc: i64 = row.try_get::<i64, _>(3).unwrap_or(1);
        let cycle: bool = row
            .try_get::<String, _>(4)
            .map(|v| v.eq_ignore_ascii_case("yes"))
            .unwrap_or(false);
        let cache: i64 = row.try_get::<i64, _>(5).unwrap_or(1);
        let data_type: String = row.try_get::<String, _>(6).unwrap_or_else(|_| "bigint".into());
        let mut ddl = format!("CREATE SEQUENCE {qualified} AS {data_type}");
        ddl.push_str(&format!("\n  INCREMENT BY {inc}\n  MINVALUE {min}\n  MAXVALUE {max}\n  START WITH {start}"));
        ddl.push_str(&format!("\n  CACHE {cache}"));
        ddl.push_str(if cycle { "\n  CYCLE;" } else { "\n  NO CYCLE;" });
        Ok(Some(ddl))
    }

    pub async fn use_database(&mut self, database: &str) -> MieoResult<()> {
        if self.tx.is_some() {
            return Err(MieoError::new(
                ErrorCode::Conflict,
                "cannot switch database inside an open transaction",
            ));
        }
        let mut spec = self.spec.clone();
        spec.database = Some(database.to_string());
        let new = Self::connect(&spec).await?;
        let old = std::mem::replace(self, new);
        old.pool.close().await;
        Ok(())
    }

    pub async fn begin(&mut self) -> MieoResult<()> {
        if self.tx.is_some() {
            return Err(MieoError::new(
                ErrorCode::Conflict,
                "a transaction is already open on this session",
            ));
        }
        self.tx = Some(self.pool.begin().await.map_err(map_err)?);
        Ok(())
    }

    pub async fn commit(&mut self) -> MieoResult<()> {
        let tx = self
            .tx
            .take()
            .ok_or_else(|| MieoError::new(ErrorCode::Conflict, "no open transaction to commit"))?;
        tx.commit().await.map_err(map_err)?;
        Ok(())
    }

    pub async fn rollback(&mut self) -> MieoResult<()> {
        let tx = self.tx.take().ok_or_else(|| {
            MieoError::new(ErrorCode::Conflict, "no open transaction to roll back")
        })?;
        tx.rollback().await.map_err(map_err)?;
        Ok(())
    }

    pub async fn run_query(
        &mut self,
        query_id: &str,
        sql: String,
        params: Vec<Value>,
        options: &QueryOptions,
        sink: &mpsc::Sender<QueryEvent>,
        cancel: &CancellationToken,
    ) -> MieoResult<QueryStats> {
        if let Some(tx) = self.tx.as_mut() {
            stream_query::<Postgres>(
                &mut **tx,
                query_id,
                sql,
                params,
                options,
                sink,
                cancel,
                bind_pg,
                decode_pg_row,
                kind_pg,
            )
            .await
        } else {
            let mut conn = self.pool.acquire().await.map_err(map_err)?;
            stream_query::<Postgres>(
                &mut conn,
                query_id,
                sql,
                params,
                options,
                sink,
                cancel,
                bind_pg,
                decode_pg_row,
                kind_pg,
            )
            .await
        }
    }

    // ------------------------------------------------------------------
    // Users / roles / grants (also drives the authorization UI).
    // In Postgres users and roles share `pg_roles`; `rolcanlogin` tells
    // logins apart from pure roles.
    // ------------------------------------------------------------------

    async fn exec(&self, sql: &str) -> MieoResult<()> {
        sqlx::query::<Postgres>(sqlx::AssertSqlSafe(sql.to_string()))
            .execute(&self.pool)
            .await
            .map_err(map_err)?;
        Ok(())
    }

    async fn role_rows(&self, logins: bool) -> MieoResult<Vec<DbPrincipal>> {
        let rows = sqlx::query::<Postgres>(
            "SELECT rolname, rolsuper, rolcreatedb, rolcreaterole, rolcanlogin              FROM pg_roles WHERE rolcanlogin = $1 ORDER BY rolname",
        )
        .bind(logins)
        .fetch_all(&self.pool)
        .await
        .map_err(map_err)?;
        Ok(rows
            .into_iter()
            .map(|row| {
                let superuser = row.try_get::<bool, _>(1).unwrap_or(false);
                let createdb = row.try_get::<bool, _>(2).unwrap_or(false);
                let createrole = row.try_get::<bool, _>(3).unwrap_or(false);
                let mut flags: Vec<&str> = Vec::new();
                if superuser {
                    flags.push("SUPERUSER");
                }
                if createdb {
                    flags.push("CREATEDB");
                }
                if createrole {
                    flags.push("CREATEROLE");
                }
                DbPrincipal {
                    name: s(&row, 0),
                    host: None,
                    kind: if logins {
                        PrincipalKind::User
                    } else {
                        PrincipalKind::Role
                    },
                    is_super: superuser,
                    can_login: row.try_get::<bool, _>(4).unwrap_or(logins),
                    comment: if flags.is_empty() {
                        None
                    } else {
                        Some(flags.join(" "))
                    },
                }
            })
            .collect())
    }

    pub async fn list_db_users(&mut self) -> MieoResult<Vec<DbPrincipal>> {
        self.role_rows(true).await
    }

    pub async fn list_db_roles(&mut self) -> MieoResult<Vec<DbPrincipal>> {
        self.role_rows(false).await
    }

    pub async fn db_role_members(&mut self) -> MieoResult<Vec<RoleGrant>> {
        let rows = sqlx::query::<Postgres>(
            "SELECT r.rolname, m.rolname, a.admin_option              FROM pg_auth_members a              JOIN pg_roles r ON r.oid = a.roleid              JOIN pg_roles m ON m.oid = a.member              ORDER BY 1, 2",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(map_err)?;
        Ok(rows
            .into_iter()
            .map(|row| RoleGrant {
                role: s(&row, 0),
                role_host: None,
                member: s(&row, 1),
                member_host: None,
                admin_option: row.try_get::<bool, _>(2).unwrap_or(false),
            })
            .collect())
    }

    pub async fn db_user_grants(
        &mut self,
        name: &str,
        _host: Option<&str>,
    ) -> MieoResult<Vec<String>> {
        // Direct object privileges are spread over many ACL columns; the
        // stable, useful surface is role membership plus owned objects.
        let rows = sqlx::query::<Postgres>(
            "SELECT r.rolname FROM pg_auth_members a              JOIN pg_roles r ON r.oid = a.roleid              JOIN pg_roles m ON m.oid = a.member              WHERE m.rolname = $1 ORDER BY 1",
        )
        .bind(name)
        .fetch_all(&self.pool)
        .await
        .map_err(map_err)?;
        Ok(rows
            .into_iter()
            .map(|row| format!("MEMBER OF {}", s(&row, 0)))
            .collect())
    }

    pub async fn db_principal_detail(
        &mut self,
        name: &str,
        _host: Option<&str>,
    ) -> MieoResult<DbPrincipalDetail> {
        let row = sqlx::query::<Postgres>(
            "SELECT rolcanlogin, rolsuper, rolcreatedb, rolcreaterole, rolinherit, \
                    rolreplication, rolconnlimit, rolvaliduntil, (rolpassword IS NOT NULL) \
             FROM pg_roles WHERE rolname = $1",
        )
        .bind(name)
        .fetch_optional(&self.pool)
        .await
        .map_err(map_err)?;
        let Some(row) = row else {
            return Err(MieoError::new(ErrorCode::NotFound, format!("role `{name}` not found")));
        };
        let bit = |i: usize| row.try_get::<bool, _>(i).unwrap_or(false);
        let can_login = bit(0);
        let is_super = bit(1);
        let principal = DbPrincipal {
            name: name.to_string(),
            host: None,
            kind: if can_login { PrincipalKind::User } else { PrincipalKind::Role },
            is_super,
            can_login,
            comment: None,
        };
        let mut attributes: Vec<DetailAttribute> = Vec::new();
        let kv = |k: &str, v: String| DetailAttribute { key: k.to_string(), value: v };
        let yn = |b: bool| if b { "yes".into() } else { "no".into() };
        attributes.push(kv("Can login", yn(can_login)));
        attributes.push(kv("Superuser", yn(is_super)));
        attributes.push(kv("Create DB", yn(bit(2))));
        attributes.push(kv("Create role", yn(bit(3))));
        attributes.push(kv("Inherit", yn(bit(4))));
        attributes.push(kv("Replication", yn(bit(5))));
        attributes.push(kv(
            "Connection limit",
            row.try_get::<i32, _>(6).map(|n| if n < 0 { "unlimited".into() } else { n.to_string() }).unwrap_or_else(|_| "unlimited".into()),
        ));
        if let Ok(Some(ts)) = row.try_get::<Option<chrono::DateTime<chrono::Utc>>, _>(7) {
            attributes.push(kv("Valid until", ts.to_rfc3339()));
        }
        attributes.push(kv("Password set", yn(bit(8))));

        // Object privileges from the standard view, grouped per table.
        let mut object_privileges: Vec<PrivilegeEntry> = Vec::new();
        if let Ok(rows) = sqlx::query::<Postgres>(
            "SELECT table_schema, table_name, privilege_type, is_grantable \
             FROM information_schema.table_privileges WHERE grantee = $1 \
             ORDER BY 1, 2",
        )
        .bind(name)
        .fetch_all(&self.pool)
        .await
        {
            let mut grouped: std::collections::BTreeMap<(String, String), (Vec<String>, bool)> =
                std::collections::BTreeMap::new();
            for row in rows {
                let key = (s(&row, 0), s(&row, 1));
                let e = grouped.entry(key).or_default();
                e.0.push(s(&row, 2));
                if s(&row, 3).eq_ignore_ascii_case("YES") {
                    e.1 = true;
                }
            }
            for ((schema, table), (mut privs, grantable)) in grouped {
                privs.sort();
                privs.dedup();
                object_privileges.push(PrivilegeEntry {
                    privileges: privs,
                    database: None,
                    schema: Some(schema),
                    table: Some(table),
                    grantable,
                });
            }
        }

        let all_members = self.db_role_members().await.unwrap_or_default();
        let member_of = all_members.iter().filter(|g| g.member == name).cloned().collect();
        let members = all_members.iter().filter(|g| g.role == name).cloned().collect();

        Ok(DbPrincipalDetail {
            principal,
            attributes,
            global_privileges: Vec::new(),
            object_privileges,
            member_of,
            members,
        })
    }

    pub async fn create_db_user(&mut self, req: &CreateDbUser) -> MieoResult<()> {
        let mut stmt = format!("CREATE ROLE {}", quote_ansi_ident(&req.name));
        if req.login {
            stmt.push_str(" WITH LOGIN");
        } else {
            stmt.push_str(" WITH NOLOGIN");
        }
        if req.superuser {
            stmt.push_str(" SUPERUSER");
        }
        if let Some(pw) = req.password.as_deref().filter(|p| !p.is_empty()) {
            stmt.push_str(&format!(" PASSWORD {}", quote_literal(pw)));
        }
        self.exec(&stmt).await
    }

    pub async fn drop_db_principal(&mut self, req: &DropDbPrincipal) -> MieoResult<()> {
        self.exec(&format!("DROP ROLE {}", quote_ansi_ident(&req.name)))
            .await
    }

    fn grant_object_pg(database: Option<&str>, schema: Option<&str>, table: Option<&str>) -> String {
        match (schema.filter(|s| !s.is_empty()), table.filter(|t| !t.is_empty())) {
            (Some(sc), Some(tbl)) => format!(
                "TABLE {}.{}",
                quote_ansi_ident(sc),
                quote_ansi_ident(tbl)
            ),
            _ => match database.filter(|d| !d.is_empty()) {
                Some(db) => format!("DATABASE {}", quote_ansi_ident(db)),
                None => "TABLE *".to_string(),
            },
        }
    }

    pub async fn grant(&mut self, req: &GrantRequest) -> MieoResult<()> {
        let grantee = quote_ansi_ident(&req.grantee);
        let stmt = match &req.target {
            GrantTarget::Privileges {
                privileges,
                database,
                schema,
                table,
            } => {
                let object = Self::grant_object_pg(
                    database.as_deref(),
                    schema.as_deref().or(Some("public")),
                    table.as_deref(),
                );
                if object == "TABLE *" {
                    return Err(MieoError::new(
                        ErrorCode::Data,
                        "choose a table or a database for this grant",
                    ));
                }
                let mut s = format!("GRANT {privileges} ON {object} TO {grantee}");
                if req.with_grant_option {
                    s.push_str(" WITH GRANT OPTION");
                }
                s
            }
            GrantTarget::Role { role, .. } => {
                format!("GRANT {} TO {grantee}", quote_ansi_ident(role))
            }
        };
        self.exec(&stmt).await
    }

    pub async fn revoke(&mut self, req: &RevokeRequest) -> MieoResult<()> {
        let grantee = quote_ansi_ident(&req.grantee);
        let stmt = match &req.target {
            GrantTarget::Privileges {
                privileges,
                database,
                schema,
                table,
            } => {
                let object = Self::grant_object_pg(
                    database.as_deref(),
                    schema.as_deref().or(Some("public")),
                    table.as_deref(),
                );
                if object == "TABLE *" {
                    return Err(MieoError::new(
                        ErrorCode::Data,
                        "choose a table or a database for this grant",
                    ));
                }
                format!("REVOKE {privileges} ON {object} FROM {grantee}")
            }
            GrantTarget::Role { role, .. } => {
                format!("REVOKE {} FROM {grantee}", quote_ansi_ident(role))
            }
        };
        self.exec(&stmt).await
    }

    pub async fn set_user_password(
        &mut self,
        name: &str,
        _host: Option<&str>,
        password: &str,
    ) -> MieoResult<()> {
        self.exec(&format!(
            "ALTER ROLE {} WITH PASSWORD {}",
            quote_ansi_ident(name),
            quote_literal(password)
        ))
        .await
    }

    fn resolve_schema(&self, scope: &Scope) -> String {
        scope
            .schema
            .clone()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "public".to_string())
    }
}

fn map_fk_action(code: &str) -> Option<String> {
    let label = match code {
        "a" => "NO ACTION",
        "r" => "RESTRICT",
        "c" => "CASCADE",
        "n" => "SET NULL",
        "d" => "SET DEFAULT",
        "" => return None,
        other => other,
    };
    Some(label.to_string())
}

// ---------------------------------------------------------------------------
// Row helpers for introspection queries
// ---------------------------------------------------------------------------

fn s(row: &PgRow, idx: usize) -> String {
    row.try_get::<String, _>(idx).unwrap_or_default()
}

fn os(row: &PgRow, idx: usize) -> Option<String> {
    row.try_get::<Option<String>, _>(idx).ok().flatten()
}

fn i64o(row: &PgRow, idx: usize) -> Option<i64> {
    row.try_get::<Option<i64>, _>(idx).ok().flatten()
}

// ---------------------------------------------------------------------------
// Value decoding
// ---------------------------------------------------------------------------

pub(crate) fn decode_pg_row(row: &PgRow) -> Vec<Value> {
    (0..row.len()).map(|i| decode_pg_cell(row, i)).collect()
}

fn decode_pg_cell(row: &PgRow, idx: usize) -> Value {
    let (native, is_null) = match row.try_get_raw(idx) {
        Ok(raw) => (raw.type_info().name().to_string(), raw.is_null()),
        Err(e) => {
            return Value::Unsupported {
                native_type: "unknown".into(),
                reason: e.to_string(),
            };
        }
    };
    if is_null {
        return Value::Null;
    }
    let kind = kind_pg(&native);

    if let DataTypeKind::Array(elem) = &kind {
        return decode_pg_array(row, idx, elem, &native);
    }

    macro_rules! unsupported {
        ($reason:expr) => {
            return Value::Unsupported {
                native_type: native.clone(),
                reason: $reason.to_string(),
            }
        };
    }
    macro_rules! try_decoded {
        ($($t:ty => $conv:expr),+ $(,)?) => {
            $(
                if let Ok(v) = row.try_get::<$t, _>(idx) {
                    let conv: fn($t) -> Value = $conv;
                    return conv(v);
                }
            )+
        };
    }

    match kind {
        DataTypeKind::Bool => {
            try_decoded!(bool => Value::Bool);
            unsupported!("bool decode failed")
        }
        DataTypeKind::Int | DataTypeKind::UInt => {
            try_decoded!(
                i64 => Value::Int,
                i32 => |v| Value::Int(v as i64),
                i16 => |v| Value::Int(v as i64),
                f64 => Value::Float,
            );
            unsupported!("integer decode failed")
        }
        DataTypeKind::Float => {
            try_decoded!(f64 => Value::Float, f32 => |v| Value::Float(v as f64));
            unsupported!("float decode failed")
        }
        DataTypeKind::Decimal => {
            if let Ok(v) = row.try_get::<BigDecimal, _>(idx) {
                return Value::Decimal(v.to_string());
            }
            try_decoded!(f64 => Value::Float);
            unsupported!("numeric decode failed")
        }
        DataTypeKind::Text | DataTypeKind::Enum | DataTypeKind::Unknown => {
            try_decoded!(String => Value::Text);
            if let Ok(v) = row.try_get::<Vec<u8>, _>(idx) {
                return Value::Bytes(b64_encode(&v));
            }
            unsupported!("text decode failed")
        }
        DataTypeKind::Bytes => {
            try_decoded!(Vec<u8> => |v| Value::Bytes(b64_encode(&v)));
            unsupported!("bytea decode failed")
        }
        DataTypeKind::Date => {
            try_decoded!(NaiveDate => |v| Value::Date(fmt_naive_date(v)));
            unsupported!("date decode failed")
        }
        DataTypeKind::Time => {
            try_decoded!(NaiveTime => |v| Value::Time(fmt_naive_time(v)));
            unsupported!("time decode failed")
        }
        DataTypeKind::DateTime => {
            try_decoded!(
                NaiveDateTime => |v| Value::DateTime(fmt_naive_dt(v)),
                DateTime<Utc> => |v| Value::DateTime(fmt_dt_utc(v)),
            );
            unsupported!("timestamp decode failed")
        }
        DataTypeKind::TimestampTz => {
            try_decoded!(
                DateTime<Utc> => |v| Value::DateTime(fmt_dt_utc(v)),
                NaiveDateTime => |v| Value::DateTime(fmt_naive_dt(v)),
            );
            unsupported!("timestamptz decode failed")
        }
        DataTypeKind::Json => {
            try_decoded!(Json<serde_json::Value> => |Json(v)| Value::Json(v));
            unsupported!("json decode failed")
        }
        DataTypeKind::Uuid => {
            try_decoded!(uuid::Uuid => |v| Value::Uuid(v.to_string()));
            unsupported!("uuid decode failed")
        }
        DataTypeKind::Interval => {
            if let Ok(iv) = row.try_get::<PgInterval, _>(idx) {
                return Value::Text(fmt_interval(iv.months, iv.days, iv.microseconds));
            }
            unsupported!("interval decode failed")
        }
        DataTypeKind::Bit => {
            // Postgres BIT/VARBIT requires the `bit-vec` feature for proper decoding.
            unsupported!("bit strings require the bit-vec feature")
        }
        DataTypeKind::Array(_) => unreachable!("handled above"),
        _ => {
            try_decoded!(String => Value::Text);
            if let Ok(v) = row.try_get::<Vec<u8>, _>(idx) {
                return Value::Bytes(b64_encode(&v));
            }
            unsupported!("no decode path for this type")
        }
    }
}

fn decode_pg_array(row: &PgRow, idx: usize, elem: &DataTypeKind, native: &str) -> Value {
    macro_rules! try_arr {
        ($t:ty, $conv:expr) => {{
            if let Ok(v) = row.try_get::<Vec<Option<$t>>, _>(idx) {
                let conv: fn($t) -> Value = $conv;
                return Value::Array(
                    v.into_iter()
                        .map(|o| o.map(conv).unwrap_or(Value::Null))
                        .collect(),
                );
            }
            if let Ok(v) = row.try_get::<Vec<$t>, _>(idx) {
                let conv: fn($t) -> Value = $conv;
                return Value::Array(v.into_iter().map(conv).collect());
            }
        }};
    }

    match elem {
        DataTypeKind::Int | DataTypeKind::UInt | DataTypeKind::Bit => {
            try_arr!(i16, |v| Value::Int(v as i64));
            try_arr!(i32, |v| Value::Int(v as i64));
            try_arr!(i64, |v| Value::Int(v));
            try_arr!(f64, |v| Value::Float(v));
        }
        DataTypeKind::Float => {
            try_arr!(f64, |v| Value::Float(v));
            try_arr!(f32, |v| Value::Float(v as f64));
        }
        DataTypeKind::Bool => {
            try_arr!(bool, |v| Value::Bool(v));
        }
        DataTypeKind::Text | DataTypeKind::Enum | DataTypeKind::Unknown => {
            try_arr!(String, |v| Value::Text(v));
        }
        DataTypeKind::Uuid => {
            try_arr!(uuid::Uuid, |v| Value::Uuid(v.to_string()));
        }
        DataTypeKind::Decimal => {
            try_arr!(BigDecimal, |v| Value::Decimal(v.to_string()));
        }
        DataTypeKind::Date => {
            try_arr!(NaiveDate, |v| Value::Date(fmt_naive_date(v)));
        }
        DataTypeKind::Time => {
            try_arr!(NaiveTime, |v| Value::Time(fmt_naive_time(v)));
        }
        DataTypeKind::DateTime => {
            try_arr!(NaiveDateTime, |v| Value::DateTime(fmt_naive_dt(v)));
        }
        DataTypeKind::TimestampTz => {
            try_arr!(DateTime<Utc>, |v| Value::DateTime(fmt_dt_utc(v)));
        }
        _ => {}
    }

    Value::Unsupported {
        native_type: native.to_string(),
        reason: "array element type not supported".into(),
    }
}

/// Normalized kind from a native Postgres type name (e.g. `INT4`, `NUMERIC`, `INT4[]`).
pub(crate) fn kind_pg(native: &str) -> DataTypeKind {
    let up = native.to_ascii_uppercase();
    if let Some(inner) = up.strip_suffix("[]") {
        return DataTypeKind::Array(Box::new(kind_pg(inner)));
    }
    match up.as_str() {
        "BOOL" => DataTypeKind::Bool,
        "INT2" | "INT4" | "INT8" => DataTypeKind::Int,
        "OID" => DataTypeKind::UInt,
        "FLOAT4" | "FLOAT8" => DataTypeKind::Float,
        "NUMERIC" | "MONEY" => DataTypeKind::Decimal,
        "TEXT" | "VARCHAR" | "BPCHAR" | "CHAR" | "NAME" | "CITEXT" => DataTypeKind::Text,
        "BYTEA" => DataTypeKind::Bytes,
        "DATE" => DataTypeKind::Date,
        "TIME" => DataTypeKind::Time,
        "TIMESTAMP" => DataTypeKind::DateTime,
        "TIMESTAMPTZ" => DataTypeKind::TimestampTz,
        "TIMETZ" => DataTypeKind::Time,
        "JSON" | "JSONB" => DataTypeKind::Json,
        "UUID" => DataTypeKind::Uuid,
        "BIT" | "VARBIT" => DataTypeKind::Bit,
        "INTERVAL" => DataTypeKind::Interval,
        _ => DataTypeKind::Unknown,
    }
}

// ---------------------------------------------------------------------------
// Parameter binding
// ---------------------------------------------------------------------------

fn parse_decimal(s: &str) -> MieoResult<BigDecimal> {
    BigDecimal::from_str(s)
        .map_err(|e| MieoError::new(ErrorCode::Data, format!("invalid decimal value: {e}")))
}

pub(crate) fn bind_pg<'q>(
    q: Query<'q, Postgres, PgArguments>,
    params: &[Value],
) -> MieoResult<Query<'q, Postgres, PgArguments>> {
    let mut q = q;
    for v in params {
        q = match v {
            Value::Null => q.bind(None::<String>),
            Value::Bool(b) => q.bind(*b),
            Value::Int(i) => q.bind(*i),
            Value::UInt(u) => {
                let as_i64 = i64::try_from(*u).map_err(|_| {
                    MieoError::new(
                        ErrorCode::Data,
                        "unsigned integer too large for PostgreSQL (max i64)",
                    )
                })?;
                q.bind(as_i64)
            }
            Value::Float(f) => q.bind(*f),
            Value::Decimal(s) => q.bind(parse_decimal(s)?),
            Value::Text(s) => q.bind(s.clone()),
            Value::Bytes(b) => q.bind(b64_decode(b)?),
            Value::Date(s) => match NaiveDate::parse_from_str(s, "%Y-%m-%d") {
                Ok(d) => q.bind(d),
                Err(_) => q.bind(s.clone()),
            },
            Value::Time(s) => match parse_naive_time(s) {
                Some(t) => q.bind(t),
                None => q.bind(s.clone()),
            },
            Value::DateTime(s) => match parse_datetime_utc(s) {
                Some(dt) => q.bind(dt),
                None => match NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S%.f") {
                    Ok(dt) => q.bind(dt),
                    Err(_) => q.bind(s.clone()),
                },
            },
            Value::Json(j) => q.bind(Json(j.clone())),
            Value::Uuid(s) => match uuid::Uuid::parse_str(s) {
                Ok(u) => q.bind(u),
                Err(_) => q.bind(s.clone()),
            },
            Value::Array(_) | Value::Unsupported { .. } => {
                return Err(MieoError::new(
                    ErrorCode::Data,
                    format!(
                        "cannot bind value of type `{}` as a parameter",
                        v.type_name()
                    ),
                ));
            }
        };
    }
    Ok(q)
}

fn parse_naive_time(s: &str) -> Option<NaiveTime> {
    NaiveTime::parse_from_str(s, "%H:%M:%S%.f")
        .ok()
        .or_else(|| NaiveTime::parse_from_str(s, "%H:%M").ok())
}

fn parse_datetime_utc(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

const _: Binder<Postgres> = bind_pg;
const _: Decoder<Postgres> = decode_pg_row;
const _: KindFn = kind_pg;
