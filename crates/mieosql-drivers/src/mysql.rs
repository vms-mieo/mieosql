//! MySQL / MariaDB driver (sqlx).

use std::str::FromStr;
use std::time::Duration;

use chrono::{DateTime, NaiveDate, NaiveDateTime, NaiveTime, Utc};
use sqlx::mysql::{
    MySql, MySqlArguments, MySqlConnectOptions, MySqlPool, MySqlPoolOptions, MySqlRow, MySqlSslMode,
};
use sqlx::query::Query;
use sqlx::types::{BigDecimal, Json};
use sqlx::{ConnectOptions, Row, Transaction, TypeInfo, ValueRef};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use mieosql_core::{
    Capabilities, ColumnDef, ConnectionSpec, CreateDbUser, DataTypeKind, DatabaseRef,
    DbPrincipal, DbPrincipalDetail, DetailAttribute, DropDbPrincipal, ErrorCode, ForeignKeyDef,
    GrantRequest, GrantTarget, IndexDef, MieoError, MieoResult, ObjectKind, ObjectRef,
    PlaceholderStyle, PrincipalKind, PrivilegeEntry, QueryEvent, QueryOptions, QueryStats,
    RevokeRequest, RoleGrant, SchemaRef, Scope, ServerInfo, Value,
};

use crate::error_map::map_err;
use crate::stream::{Binder, Decoder, KindFn, stream_query};
use crate::util::*;

pub struct MySqlDriver {
    pool: MySqlPool,
    tx: Option<Transaction<'static, MySql>>,
    spec: ConnectionSpec,
}

pub(crate) fn mysql_capabilities() -> Capabilities {
    Capabilities {
        engine_kind: mieosql_core::EngineKind::Sql,
        placeholder: PlaceholderStyle::Question,
        list_databases: true,
        list_schemas: false,
        schema_is_database: true,
        transactions: true,
        simple_mode: true,
        ddl: true,
        multiple_databases: true,
        user_management: true,
    }
}

impl MySqlDriver {
    pub async fn connect(spec: &ConnectionSpec) -> MieoResult<Self> {
        if spec.ssh.is_some() {
            return Err(MieoError::unsupported(
                "SSH tunnels are not implemented yet (planned phase P1) — refusing to connect directly",
            ));
        }
        let host = spec.host.clone().unwrap_or_else(|| "127.0.0.1".into());
        let port = spec.port.unwrap_or(3306);

        let mut opts = MySqlConnectOptions::new()
            .host(&host)
            .port(port)
            .charset("utf8mb4")
            .ssl_mode(match spec.ssl_mode {
                mieosql_core::SslMode::Disable => MySqlSslMode::Disabled,
                mieosql_core::SslMode::Prefer => MySqlSslMode::Preferred,
                mieosql_core::SslMode::Require => MySqlSslMode::Required,
                mieosql_core::SslMode::VerifyCa => MySqlSslMode::VerifyCa,
                mieosql_core::SslMode::VerifyFull => MySqlSslMode::VerifyIdentity,
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

        let pool = MySqlPoolOptions::new()
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
        sqlx::query::<MySql>("SELECT 1")
            .fetch_one(&self.pool)
            .await
            .map_err(map_err)?;
        Ok(())
    }

    pub async fn server_info(&self) -> MieoResult<ServerInfo> {
        let row =
            sqlx::query::<MySql>("SELECT VERSION(), @@version_comment, DATABASE(), CURRENT_USER()")
                .fetch_one(&self.pool)
                .await
                .map_err(map_err)?;
        Ok(ServerInfo {
            engine: self.spec.engine,
            version: s(&row, 0),
            version_comment: os(&row, 1).filter(|v| !v.is_empty()),
            current_database: os(&row, 2).filter(|v| !v.is_empty()),
            current_user: os(&row, 3),
        })
    }

    pub async fn list_databases(&self) -> MieoResult<Vec<DatabaseRef>> {
        let rows = sqlx::query::<MySql>(
            "SELECT SCHEMA_NAME, DEFAULT_CHARACTER_SET_NAME, DEFAULT_COLLATION_NAME \
             FROM information_schema.SCHEMATA ORDER BY SCHEMA_NAME",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(map_err)?;
        Ok(rows
            .into_iter()
            .map(|row| {
                let name = s(&row, 0);
                DatabaseRef {
                    is_system: is_mysql_system_db(&name),
                    charset: os(&row, 1),
                    collation: os(&row, 2),
                    comment: None,
                    size_bytes: None,
                    name,
                }
            })
            .collect())
    }

    pub async fn list_schemas(&self, _database: Option<&str>) -> MieoResult<Vec<SchemaRef>> {
        Err(MieoError::unsupported(
            "MySQL has no separate schema level; use list_databases",
        ))
    }

    pub async fn list_objects(
        &self,
        scope: &Scope,
        kind: Option<ObjectKind>,
        pattern: Option<&str>,
    ) -> MieoResult<Vec<ObjectRef>> {
        let db = scope
            .database
            .clone()
            .or_else(|| self.spec.database.clone())
            .ok_or_else(|| MieoError::new(ErrorCode::Data, "no database selected"))?;

        let mut sql = String::from(
            "SELECT TABLE_NAME, TABLE_TYPE, ENGINE, TABLE_ROWS, DATA_LENGTH, INDEX_LENGTH, \
                    TABLE_COMMENT, CREATE_TIME, UPDATE_TIME \
             FROM information_schema.TABLES WHERE TABLE_SCHEMA = ?",
        );
        if matches!(
            kind,
            Some(ObjectKind::Function | ObjectKind::Procedure)
        ) {
            let want = if kind == Some(ObjectKind::Function) {
                "FUNCTION"
            } else {
                "PROCEDURE"
            };
            let mut rsql = String::from(
                "SELECT ROUTINE_NAME, ROUTINE_COMMENT FROM information_schema.ROUTINES                  WHERE ROUTINE_SCHEMA = ? AND ROUTINE_TYPE = ?",
            );
            if pattern.is_some() {
                rsql.push_str(" AND ROUTINE_NAME LIKE ?");
            }
            rsql.push_str(" ORDER BY ROUTINE_NAME");
            let mut q = sqlx::query::<MySql>(sqlx::AssertSqlSafe(rsql)).bind(&db).bind(want);
            if let Some(p) = pattern {
                q = q.bind(format!("%{p}%"));
            }
            let rows = q.fetch_all(&self.pool).await.map_err(map_err)?;
            let rkind = if want == "FUNCTION" {
                ObjectKind::Function
            } else {
                ObjectKind::Procedure
            };
            return Ok(rows
                .into_iter()
                .map(|row| ObjectRef {
                    name: s(&row, 0),
                    kind: rkind.clone(),
                    database: Some(db.clone()),
                    schema: None,
                    comment: os(&row, 1).filter(|c| !c.is_empty()),
                    rows_estimate: None,
                    size_bytes: None,
                    updated_at: None,
                })
                .collect());
        }
        // Triggers come from information_schema.TRIGGERS. MySQL Events ride
        // along under this kind too — core ObjectKind has no Event variant
        // (contract frozen) — marked with an "EVENT · ..." comment. The
        // EVENTS catalog needs the EVENT privilege; on failure just return
        // the triggers.
        if kind == Some(ObjectKind::Trigger) {
            let mut tsql = String::from(
                "SELECT TRIGGER_NAME, EVENT_OBJECT_TABLE, ACTION_TIMING, EVENT_MANIPULATION \
                 FROM information_schema.TRIGGERS WHERE TRIGGER_SCHEMA = ?",

            );
            if pattern.is_some() {
                tsql.push_str(" AND TRIGGER_NAME LIKE ?");
            }
            tsql.push_str(" ORDER BY TRIGGER_NAME");
            let mut q = sqlx::query::<MySql>(sqlx::AssertSqlSafe(tsql)).bind(&db);
            if let Some(p) = pattern {
                q = q.bind(format!("%{p}%"));
            }
            let rows = q.fetch_all(&self.pool).await.map_err(map_err)?;
            let mut out: Vec<ObjectRef> = rows
                .into_iter()
                .map(|row| {
                    let timing = s(&row, 2);
                    let event = s(&row, 3);
                    let table = s(&row, 1);
                    let comment = if table.is_empty() {
                        None
                    } else {
                        Some(format!("ON {table} · {timing} {event}"))
                    };
                    ObjectRef {
                        name: s(&row, 0),
                        kind: ObjectKind::Trigger,
                        database: Some(db.clone()),
                        schema: None,
                        comment,
                        rows_estimate: None,
                        size_bytes: None,
                        updated_at: None,
                    }
                })
                .collect();

            let mut esql = String::from(
                "SELECT EVENT_NAME, EVENT_TYPE, EXECUTE_AT, INTERVAL_VALUE, \
                        INTERVAL_FIELD, STARTS, ENDS, STATUS \
                 FROM information_schema.EVENTS WHERE EVENT_SCHEMA = ?",
            );
            if pattern.is_some() {
                esql.push_str(" AND EVENT_NAME LIKE ?");
            }
            esql.push_str(" ORDER BY EVENT_NAME");
            let mut eq = sqlx::query::<MySql>(sqlx::AssertSqlSafe(esql)).bind(&db);
            if let Some(p) = pattern {
                eq = eq.bind(format!("%{p}%"));
            }
            if let Ok(rows) = eq.fetch_all(&self.pool).await {
                out.extend(rows.into_iter().map(|row| {
                    let schedule = format_event_schedule(
                        &s(&row, 1),
                        dt_o(&row, 2).map(fmt_naive_dt).as_deref(),
                        os(&row, 3).as_deref(),
                        os(&row, 4).as_deref(),
                        dt_o(&row, 5).map(fmt_naive_dt).as_deref(),
                        dt_o(&row, 6).map(fmt_naive_dt).as_deref(),

                        os(&row, 7).as_deref(),
                    );
                    ObjectRef {
                        name: s(&row, 0),
                        kind: ObjectKind::Trigger,
                        database: Some(db.clone()),
                        schema: None,
                        comment: Some(format!("EVENT · {schedule}")),
                        rows_estimate: None,
                        size_bytes: None,
                        updated_at: None,
                    }
                }));
            }
            return Ok(out);
        }
        match kind {
            Some(ObjectKind::Table) => sql.push_str(" AND TABLE_TYPE = 'BASE TABLE'"),
            Some(ObjectKind::View) => sql.push_str(" AND TABLE_TYPE = 'VIEW'"),
            Some(
                ObjectKind::Sequence
                | ObjectKind::MaterializedView,
            ) => return Ok(Vec::new()),
            None => sql.push_str(" AND TABLE_TYPE IN ('BASE TABLE', 'VIEW')"),
            Some(ObjectKind::Function | ObjectKind::Procedure | ObjectKind::Trigger) => {
                unreachable!("handled above")
            }
        }
        if pattern.is_some() {
            sql.push_str(" AND TABLE_NAME LIKE ?");
        }
        sql.push_str(" ORDER BY TABLE_NAME");

        let mut q = sqlx::query::<MySql>(sqlx::AssertSqlSafe(sql)).bind(&db);
        if let Some(p) = pattern {
            q = q.bind(format!("%{p}%"));
        }
        let rows = q.fetch_all(&self.pool).await.map_err(map_err)?;

        Ok(rows
            .into_iter()
            .map(|row| {
                let table_type = s(&row, 1);
                let kind = if table_type == "VIEW" {
                    ObjectKind::View
                } else {
                    ObjectKind::Table
                };
                let comment = os(&row, 6).filter(|c| !c.is_empty());
                let engine = os(&row, 2);
                let mut comment_parts: Vec<String> = Vec::new();
                if let Some(e) = engine.filter(|e| !e.is_empty()) {
                    comment_parts.push(e);
                }
                if let Some(c) = &comment {
                    comment_parts.push(c.clone());
                }
                ObjectRef {
                    name: s(&row, 0),
                    kind,
                    database: Some(db.clone()),
                    schema: None,
                    comment: if comment_parts.is_empty() {
                        None
                    } else {
                        Some(comment_parts.join(" · "))
                    },
                    rows_estimate: i64o(&row, 3).filter(|v| *v >= 0).map(|v| v as u64),
                    size_bytes: match (i64o(&row, 4), i64o(&row, 5)) {
                        (None, None) => None,
                        (a, b) => Some((a.unwrap_or(0).max(0) + b.unwrap_or(0).max(0)) as u64),
                    },
                    updated_at: dt_o(&row, 8).map(fmt_naive_dt),
                }
            })
            .collect())
    }

    pub async fn table_columns(&self, scope: &Scope, table: &str) -> MieoResult<Vec<ColumnDef>> {
        let db = self.resolve_db(scope)?;
        let rows = sqlx::query::<MySql>(
            "SELECT COLUMN_NAME, ORDINAL_POSITION, COLUMN_TYPE, DATA_TYPE, IS_NULLABLE, \
                    COLUMN_DEFAULT, COLUMN_KEY, EXTRA, COLUMN_COMMENT, CHARACTER_MAXIMUM_LENGTH, \
                    NUMERIC_PRECISION, NUMERIC_SCALE, COLLATION_NAME \
             FROM information_schema.COLUMNS \
             WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ? ORDER BY ORDINAL_POSITION",
        )
        .bind(&db)
        .bind(table)
        .fetch_all(&self.pool)
        .await
        .map_err(map_err)?;

        Ok(rows
            .into_iter()
            .map(|row| {
                let data_type = s(&row, 3);
                let extra = s(&row, 7).to_ascii_lowercase();
                let column_key = s(&row, 6);
                ColumnDef {
                    name: s(&row, 0),
                    ordinal: i64o(&row, 1).unwrap_or(0).max(0) as u32,
                    native_type: s(&row, 2),
                    data_type: kind_mysql(&data_type),
                    nullable: s(&row, 4).eq_ignore_ascii_case("YES"),
                    default_value: os(&row, 5),
                    is_primary_key: column_key == "PRI",
                    is_auto_increment: extra.contains("auto_increment"),
                    is_generated: extra.contains("generated"),
                    comment: os(&row, 8).filter(|c| !c.is_empty()),
                    char_max_length: i64o(&row, 9).map(|v| v.max(0) as u64),
                    numeric_precision: i64o(&row, 10).map(|v| v.max(0) as u32),
                    numeric_scale: i64o(&row, 11).map(|v| v.max(0) as u32),
                    collation: os(&row, 12),
                }
            })
            .collect())
    }

    pub async fn table_indexes(&self, scope: &Scope, table: &str) -> MieoResult<Vec<IndexDef>> {
        let db = self.resolve_db(scope)?;
        let rows = sqlx::query::<MySql>(
            "SELECT INDEX_NAME, NON_UNIQUE, COLUMN_NAME, SEQ_IN_INDEX, INDEX_TYPE, INDEX_COMMENT \
             FROM information_schema.STATISTICS \
             WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ? ORDER BY INDEX_NAME, SEQ_IN_INDEX",
        )
        .bind(&db)
        .bind(table)
        .fetch_all(&self.pool)
        .await
        .map_err(map_err)?;

        let mut out: Vec<IndexDef> = Vec::new();
        for row in rows {
            let name = s(&row, 0);
            let column = os(&row, 2);
            let unique = i64o(&row, 1).unwrap_or(1) == 0;
            let index_type = os(&row, 4);
            let comment = os(&row, 5).filter(|c| !c.is_empty());
            let is_primary = name == "PRIMARY";
            match out.last_mut() {
                Some(idx) if idx.name == name => {
                    if let Some(c) = column {
                        idx.columns.push(c);
                    }
                }
                _ => out.push(IndexDef {
                    is_primary,
                    name,
                    columns: column.into_iter().collect(),
                    is_unique: unique || is_primary,
                    index_type,
                    comment,
                }),
            }
        }
        Ok(out)
    }

    pub async fn foreign_keys(&self, scope: &Scope, table: &str) -> MieoResult<Vec<ForeignKeyDef>> {
        let db = self.resolve_db(scope)?;
        let rows = sqlx::query::<MySql>(
            "SELECT k.CONSTRAINT_NAME, k.COLUMN_NAME, k.REFERENCED_TABLE_SCHEMA, \
                    k.REFERENCED_TABLE_NAME, k.REFERENCED_COLUMN_NAME, r.UPDATE_RULE, r.DELETE_RULE \
             FROM information_schema.KEY_COLUMN_USAGE k \
             JOIN information_schema.REFERENTIAL_CONSTRAINTS r \
               ON r.CONSTRAINT_SCHEMA = k.CONSTRAINT_SCHEMA \
              AND r.CONSTRAINT_NAME = k.CONSTRAINT_NAME \
              AND r.TABLE_NAME = k.TABLE_NAME \
             WHERE k.TABLE_SCHEMA = ? AND k.TABLE_NAME = ? AND k.REFERENCED_TABLE_NAME IS NOT NULL \
             ORDER BY k.CONSTRAINT_NAME, k.ORDINAL_POSITION",
        )
        .bind(&db)
        .bind(table)
        .fetch_all(&self.pool)
        .await
        .map_err(map_err)?;

        let mut out: Vec<ForeignKeyDef> = Vec::new();
        for row in rows {
            let name = s(&row, 0);
            match out.last_mut() {
                Some(fk) if fk.name == name => {
                    if let Some(c) = os(&row, 1) {
                        fk.columns.push(c);
                    }
                    if let Some(c) = os(&row, 4) {
                        fk.ref_columns.push(c);
                    }
                }
                _ => out.push(ForeignKeyDef {
                    name,
                    columns: os(&row, 1).into_iter().collect(),
                    ref_table: s(&row, 3),
                    ref_schema: os(&row, 2),
                    ref_columns: os(&row, 4).into_iter().collect(),
                    on_update: os(&row, 5),
                    on_delete: os(&row, 6),
                }),
            }
        }
        Ok(out)
    }

    pub async fn object_ddl(
        &self,
        scope: &Scope,
        kind: ObjectKind,
        name: &str,
    ) -> MieoResult<Option<String>> {
        let db = self.resolve_db(scope)?;
        let qualified = format!("{}.{}", quote_mysql_ident(&db), quote_mysql_ident(name));
        // `Trigger` also covers MySQL Events (no `ObjectKind::Event` in the
        // frozen core contract): try SHOW CREATE TRIGGER first, then
        // SHOW CREATE EVENT. Trigger DDL sits in column 2
        // (Trigger, sql_mode, SQL Original Statement, ...); event DDL in
        // column 3 (Event, sql_mode, time_zone, Create Event, ...).

        if kind == ObjectKind::Trigger {
            let trigger_stmt = format!("SHOW CREATE TRIGGER {qualified}");
            if let Ok(rows) = sqlx::raw_sql(sqlx::AssertSqlSafe(trigger_stmt))
                .fetch_all(&self.pool)
                .await
            {
                if let Some(row) = rows.first() {
                    let ddl = row.try_get::<String, _>(2).unwrap_or_default();
                    if !ddl.is_empty() {
                        return Ok(Some(ddl));
                    }
                }
            }
            let event_stmt = format!("SHOW CREATE EVENT {qualified}");
            let rows = sqlx::raw_sql(sqlx::AssertSqlSafe(event_stmt))
                .fetch_all(&self.pool)
                .await
                .map_err(map_err)?;
            if let Some(row) = rows.first() {
                let ddl = row.try_get::<String, _>(3).unwrap_or_default();
                if !ddl.is_empty() {
                    return Ok(Some(ddl));
                }
            }
            return Ok(None);
        }
        // SHOW CREATE FUNCTION/PROCEDURE return the body in column 2
        // (Function, sql_mode, Create Function).
        let (stmt, col) = match kind {
            ObjectKind::Table => (format!("SHOW CREATE TABLE {qualified}"), 1),
            ObjectKind::View => (format!("SHOW CREATE VIEW {qualified}"), 1),
            ObjectKind::Function => (format!("SHOW CREATE FUNCTION {qualified}"), 2),
            ObjectKind::Procedure => (format!("SHOW CREATE PROCEDURE {qualified}"), 2),

            _ => return Ok(None),
        };
        let rows = sqlx::raw_sql(sqlx::AssertSqlSafe(stmt))
            .fetch_all(&self.pool)
            .await
            .map_err(map_err)?;
        if let Some(row) = rows.first() {
            let ddl = row.try_get::<String, _>(col).unwrap_or_default();
            if !ddl.is_empty() {
                return Ok(Some(ddl));
            }
        }
        Ok(None)
    }

    pub async fn use_database(&mut self, database: &str) -> MieoResult<()> {
        if self.tx.is_some() {
            return Err(MieoError::new(
                ErrorCode::Conflict,
                "cannot switch database inside an open transaction",
            ));
        }
        let stmt = format!("USE {}", quote_mysql_ident(database));
        sqlx::raw_sql(sqlx::AssertSqlSafe(stmt))
            .execute(&self.pool)
            .await
            .map_err(map_err)?;
        self.spec.database = Some(database.to_string());
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
            stream_query::<MySql>(
                &mut **tx,
                query_id,
                sql,
                params,
                options,
                sink,
                cancel,
                bind_mysql,
                decode_mysql_row,
                kind_mysql,
            )
            .await
        } else {
            let mut conn = self.pool.acquire().await.map_err(map_err)?;
            stream_query::<MySql>(
                &mut conn,
                query_id,
                sql,
                params,
                options,
                sink,
                cancel,
                bind_mysql,
                decode_mysql_row,
                kind_mysql,
            )
            .await
        }
    }

    // ------------------------------------------------------------------
    // Users / roles / grants (also drives the authorization UI).
    // ------------------------------------------------------------------

    fn user_host(user: &str, host: Option<&str>) -> String {
        let host = host.filter(|h| !h.is_empty()).unwrap_or("%");
        format!(
            "{}@{}",
            quote_mysql_ident(user),
            quote_mysql_ident(host)
        )
    }

    async fn exec(&self, sql: &str) -> MieoResult<()> {
        sqlx::raw_sql(sqlx::AssertSqlSafe(sql.to_string()))
            .execute(&self.pool)
            .await
            .map_err(map_err)?;
        Ok(())
    }

    pub async fn list_db_users(&mut self) -> MieoResult<Vec<DbPrincipal>> {
        let rows = sqlx::query::<MySql>(
            "SELECT user, host, Super_priv FROM mysql.user ORDER BY user, host",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(map_err)?;
        Ok(rows
            .into_iter()
            .map(|row| DbPrincipal {
                name: s(&row, 0),
                host: Some(s(&row, 1)),
                kind: PrincipalKind::User,
                is_super: row
                    .try_get::<String, _>(2)
                    .map(|v| v.eq_ignore_ascii_case("Y"))
                    .unwrap_or(false),
                can_login: true,
                comment: None,
            })
            .collect())
    }

    pub async fn list_db_roles(&mut self) -> MieoResult<Vec<DbPrincipal>> {
        // MySQL 8+: roles surface through `mysql.role_edges`. MariaDB 10.5+
        // exposes `is_role` on `mysql.user` instead.
        if let Ok(rows) = sqlx::query::<MySql>(
            "SELECT DISTINCT FROM_USER, FROM_HOST FROM mysql.role_edges ORDER BY 1, 2",
        )
        .fetch_all(&self.pool)
        .await
        {
            return Ok(rows
                .into_iter()
                .map(|row| DbPrincipal {
                    name: s(&row, 0),
                    host: Some(s(&row, 1)),
                    kind: PrincipalKind::Role,
                    is_super: false,
                    can_login: false,
                    comment: None,
                })
                .collect());
        }
        let rows = sqlx::query::<MySql>(
            "SELECT user, host FROM mysql.user WHERE is_role = 'Y' ORDER BY user, host",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(map_err)?;
        Ok(rows
            .into_iter()
            .map(|row| DbPrincipal {
                name: s(&row, 0),
                host: Some(s(&row, 1)),
                kind: PrincipalKind::Role,
                is_super: false,
                can_login: false,
                comment: None,
            })
            .collect())
    }

    pub async fn db_role_members(&mut self) -> MieoResult<Vec<RoleGrant>> {
        let Ok(rows) = sqlx::query::<MySql>(
            "SELECT FROM_USER, FROM_HOST, TO_USER, TO_HOST, WITH_ADMIN_OPTION              FROM mysql.role_edges ORDER BY 1, 2, 3, 4",
        )
        .fetch_all(&self.pool)
        .await
        else {
            return Ok(Vec::new());
        };
        Ok(rows
            .into_iter()
            .map(|row| RoleGrant {
                role: s(&row, 0),
                role_host: Some(s(&row, 1)),
                member: s(&row, 2),
                member_host: Some(s(&row, 3)),
                admin_option: row
                    .try_get::<String, _>(4)
                    .map(|v| v.eq_ignore_ascii_case("Y"))
                    .unwrap_or(false),
            })
            .collect())
    }

    pub async fn db_user_grants(&mut self, name: &str, host: Option<&str>) -> MieoResult<Vec<String>> {
        let stmt = format!("SHOW GRANTS FOR {}", Self::user_host(name, host));
        let rows = sqlx::raw_sql(sqlx::AssertSqlSafe(stmt))
            .fetch_all(&self.pool)
            .await
            .map_err(map_err)?;
        Ok(rows.into_iter().map(|row| s(&row, 0)).collect())
    }

    fn grantee_of(user: &str, host: Option<&str>) -> String {
        let lit = |v: &str| format!("\'{}\'", v.replace('\'', "''"));
        format!("{}@{}", lit(user), lit(host.filter(|h| !h.is_empty()).unwrap_or("%")))
    }

    pub async fn db_principal_detail(
        &mut self,
        name: &str,
        host: Option<&str>,
    ) -> MieoResult<DbPrincipalDetail> {
        let grantee = Self::grantee_of(name, host);
        let host_owned = host.filter(|h| !h.is_empty()).unwrap_or("%").to_string();

        // Base principal: reuse the users/roles listings.
        let mut principals = self.list_db_users().await?;
        principals.extend(self.list_db_roles().await?);
        let principal = principals
            .into_iter()
            .find(|p| p.name == name && p.host.as_deref().unwrap_or("%") == host_owned)
            .unwrap_or(DbPrincipal {
                name: name.to_string(),
                host: Some(host_owned.clone()),
                kind: PrincipalKind::User,
                is_super: false,
                can_login: true,
                comment: None,
            });

        // Account attributes (never the password hash).
        let mut attributes: Vec<DetailAttribute> = Vec::new();
        let attr_rows = sqlx::query::<MySql>(
            "SELECT plugin, account_locked, password_expired, password_last_changed, \
                    max_questions, max_updates, max_connections, max_user_connections \
             FROM mysql.user WHERE user = ? AND host = ?",
        )
        .bind(name)
        .bind(&host_owned)
        .fetch_optional(&self.pool)
        .await
        .map_err(map_err)?;
        if let Some(row) = attr_rows {
            let kv = |k: &str, v: String| DetailAttribute { key: k.to_string(), value: v };
            if let Some(v) = os(&row, 0).filter(|v| !v.is_empty()) {
                attributes.push(kv("Auth plugin", v));
            }
            attributes.push(kv(
                "Account",
                if s(&row, 1) == "Y" { "locked".into() } else { "unlocked".into() },
            ));
            attributes.push(kv(
                "Password expired",
                if s(&row, 2) == "Y" { "yes".into() } else { "no".into() },
            ));
            if let Some(v) = os(&row, 3).filter(|v| !v.is_empty()) {
                attributes.push(kv("Password changed", v));
            }
            let limits = [
                ("max_questions", i64o(&row, 4)),
                ("max_updates", i64o(&row, 5)),
                ("max_connections", i64o(&row, 6)),
                ("max_user_connections", i64o(&row, 7)),
            ];
            let set: Vec<String> = limits
                .into_iter()
                .filter_map(|(k, v)| match v {
                    Some(n) if n > 0 => Some(format!("{k}={n}")),
                    _ => None,
                })
                .collect();
            attributes.push(kv(
                "Resource limits",
                if set.is_empty() { "none".into() } else { set.join("  ") },
            ));
        }

        // Global privileges via USER_PRIVILEGES (stable across versions).
        let mut global_privileges: Vec<String> = Vec::new();
        let mut grantable_global = false;
        if let Ok(rows) = sqlx::query::<MySql>(
            "SELECT PRIVILEGE_TYPE, IS_GRANTABLE FROM INFORMATION_SCHEMA.USER_PRIVILEGES \
             WHERE GRANTEE = ? ORDER BY PRIVILEGE_TYPE",
        )
        .bind(&grantee)
        .fetch_all(&self.pool)
        .await
        {
            for row in rows {
                global_privileges.push(s(&row, 0));
                if s(&row, 1).eq_ignore_ascii_case("YES") {
                    grantable_global = true;
                }
            }
        }
        if grantable_global {
            attributes.push(DetailAttribute {
                key: "Grant option".into(),
                value: "global".into(),
            });
        }

        // Object privileges, grouped by (schema, table).
        let mut object_privileges: Vec<PrivilegeEntry> = Vec::new();
        let mut grouped: std::collections::BTreeMap<(String, Option<String>), Vec<String>> =
            std::collections::BTreeMap::new();
        let mut grantable = std::collections::BTreeSet::new();
        if let Ok(rows) = sqlx::query::<MySql>(
            "SELECT TABLE_SCHEMA, NULL, PRIVILEGE_TYPE, IS_GRANTABLE \
             FROM INFORMATION_SCHEMA.SCHEMA_PRIVILEGES WHERE GRANTEE = ? \
             UNION ALL \
             SELECT TABLE_SCHEMA, TABLE_NAME, PRIVILEGE_TYPE, IS_GRANTABLE \
             FROM INFORMATION_SCHEMA.TABLE_PRIVILEGES WHERE GRANTEE = ? \
             ORDER BY 1, 2",
        )
        .bind(&grantee)
        .bind(&grantee)
        .fetch_all(&self.pool)
        .await
        {
            for row in rows {
                let key = (s(&row, 0), os(&row, 1).filter(|v| !v.is_empty() && v != "NULL"));
                grouped.entry(key.clone()).or_default().push(s(&row, 2));
                if s(&row, 3).eq_ignore_ascii_case("YES") {
                    grantable.insert(key);
                }
            }
        }
        // information_schema returns the literal string "NULL" for missing
        // table names in some versions — normalize it away.
        for ((db, table), mut privs) in grouped {
            let table = table.filter(|t| t != "NULL");
            let gkey = (db.clone(), table.clone());
            privs.sort();
            privs.dedup();
            object_privileges.push(PrivilegeEntry {
                privileges: privs,
                database: Some(db),
                schema: None,
                table,
                grantable: grantable.contains(&gkey),
            });
        }

        // Memberships both directions.
        let all_members = self.db_role_members().await.unwrap_or_default();
        let member_of = all_members
            .iter()
            .filter(|g| {
                g.member == name && g.member_host.as_deref().unwrap_or("%") == host_owned
            })
            .cloned()
            .collect();
        let members = all_members
            .iter()
            .filter(|g| {
                g.role == name && g.role_host.as_deref().unwrap_or("%") == host_owned
            })
            .cloned()
            .collect();

        Ok(DbPrincipalDetail {
            principal,
            attributes,
            global_privileges,
            object_privileges,
            member_of,
            members,
        })
    }

    pub async fn create_db_user(&mut self, req: &CreateDbUser) -> MieoResult<()> {
        let account = Self::user_host(&req.name, req.host.as_deref());
        let mut stmt = format!("CREATE USER {account}");
        if let Some(pw) = req.password.as_deref().filter(|p| !p.is_empty()) {
            stmt.push_str(&format!(" IDENTIFIED BY {}", quote_literal(pw)));
        }
        self.exec(&stmt).await?;
        if req.superuser {
            let grant = format!(
                "GRANT ALL PRIVILEGES ON *.* TO {account} WITH GRANT OPTION"
            );
            self.exec(&grant).await?;
        }
        Ok(())
    }

    pub async fn drop_db_principal(&mut self, req: &DropDbPrincipal) -> MieoResult<()> {
        let account = Self::user_host(&req.name, req.host.as_deref());
        let stmt = match req.kind {
            PrincipalKind::User => format!("DROP USER {account}"),
            PrincipalKind::Role => format!("DROP ROLE {account}"),
        };
        self.exec(&stmt).await
    }

    fn grant_object(req_host: Option<&str>, database: Option<&str>, table: Option<&str>) -> String {
        let _ = req_host;
        match (database.filter(|d| !d.is_empty()), table.filter(|t| !t.is_empty())) {
            (Some(db), Some(tbl)) => format!(
                "{}.{}",
                quote_mysql_ident(db),
                quote_mysql_ident(tbl)
            ),
            (Some(db), None) => format!("{}.*", quote_mysql_ident(db)),
            (None, Some(tbl)) => format!("*.{}", quote_mysql_ident(tbl)),
            (None, None) => "*.*".to_string(),
        }
    }

    pub async fn grant(&mut self, req: &GrantRequest) -> MieoResult<()> {
        let account = Self::user_host(&req.grantee, req.grantee_host.as_deref());
        let stmt = match &req.target {
            GrantTarget::Privileges {
                privileges,
                database,
                schema: _,
                table,
            } => {
                let object = Self::grant_object(req.grantee_host.as_deref(), database.as_deref(), table.as_deref());
                let mut s = format!("GRANT {privileges} ON {object} TO {account}");
                if req.with_grant_option {
                    s.push_str(" WITH GRANT OPTION");
                }
                s
            }
            GrantTarget::Role { role, host } => {
                format!("GRANT {} TO {account}", Self::user_host(role, host.as_deref()))
            }
        };
        self.exec(&stmt).await
    }

    pub async fn revoke(&mut self, req: &RevokeRequest) -> MieoResult<()> {
        let account = Self::user_host(&req.grantee, req.grantee_host.as_deref());
        let stmt = match &req.target {
            GrantTarget::Privileges {
                privileges,
                database,
                schema: _,
                table,
            } => {
                let object = Self::grant_object(req.grantee_host.as_deref(), database.as_deref(), table.as_deref());
                format!("REVOKE {privileges} ON {object} FROM {account}")
            }
            GrantTarget::Role { role, host } => {
                format!("REVOKE {} FROM {account}", Self::user_host(role, host.as_deref()))
            }
        };
        self.exec(&stmt).await
    }

    pub async fn set_user_password(
        &mut self,
        name: &str,
        host: Option<&str>,
        password: &str,
    ) -> MieoResult<()> {
        let account = Self::user_host(name, host);
        self.exec(&format!(
            "ALTER USER {account} IDENTIFIED BY {}",
            quote_literal(password)
        ))
        .await
    }

    fn resolve_db(&self, scope: &Scope) -> MieoResult<String> {
        scope
            .database
            .clone()
            .or_else(|| self.spec.database.clone())
            .ok_or_else(|| MieoError::new(ErrorCode::Data, "no database selected"))
    }
}

fn is_mysql_system_db(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "information_schema" | "mysql" | "performance_schema" | "sys"
    )
}

// ---------------------------------------------------------------------------
// Row helpers for introspection queries
// ---------------------------------------------------------------------------

fn s(row: &MySqlRow, idx: usize) -> String {
    row.try_get::<String, _>(idx).unwrap_or_default()
}

fn os(row: &MySqlRow, idx: usize) -> Option<String> {
    row.try_get::<Option<String>, _>(idx).ok().flatten()
}

fn i64o(row: &MySqlRow, idx: usize) -> Option<i64> {
    row.try_get::<Option<i64>, _>(idx).ok().flatten()
}

fn dt_o(row: &MySqlRow, idx: usize) -> Option<NaiveDateTime> {
    row.try_get::<Option<NaiveDateTime>, _>(idx).ok().flatten()
}

/// Human-readable schedule for a MySQL Event Scheduler row, shown after the
/// "EVENT · " prefix in the explorer comment. All inputs are the already
/// stringified `information_schema.EVENTS` columns; each is optional because
/// one-time and recurring events populate different subsets.
fn format_event_schedule(
    event_type: &str,
    execute_at: Option<&str>,
    interval_value: Option<&str>,
    interval_field: Option<&str>,
    starts: Option<&str>,
    ends: Option<&str>,
    status: Option<&str>,
) -> String {
    let mut out = String::new();
    if event_type.eq_ignore_ascii_case("ONE TIME") {
        out.push_str("ONE TIME");
        if let Some(at) = execute_at.filter(|s| !s.is_empty()) {
            out.push_str(" AT ");
            out.push_str(at);
        }
    } else {
        out.push_str("EVERY ");
        match (interval_value, interval_field) {
            (Some(v), Some(f)) if !v.is_empty() && !f.is_empty() => {
                out.push_str(v);
                out.push(' ');
                out.push_str(f);
            }
            _ => out.push_str("?"),
        }
        if let Some(s) = starts.filter(|s| !s.is_empty()) {
            out.push_str(" STARTS ");
            out.push_str(s);
        }
        if let Some(e) = ends.filter(|s| !s.is_empty()) {
            out.push_str(" ENDS ");
            out.push_str(e);
        }
    }
    if let Some(st) = status.filter(|s| !s.is_empty() && !s.eq_ignore_ascii_case("ENABLED")) {
        out.push_str(" (");
        out.push_str(st);
        out.push(')');
    }
    out
}

// ---------------------------------------------------------------------------
// Value decoding
// ---------------------------------------------------------------------------

pub(crate) fn decode_mysql_row(row: &MySqlRow) -> Vec<Value> {
    (0..row.len()).map(|i| decode_mysql_cell(row, i)).collect()
}

fn decode_mysql_cell(row: &MySqlRow, idx: usize) -> Value {
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
    let kind = kind_mysql(&native);
    let unsupported = |reason: &str| Value::Unsupported {
        native_type: native.clone(),
        reason: reason.to_string(),
    };

    match kind {
        DataTypeKind::Bool => row
            .try_get::<bool, _>(idx)
            .map(Value::Bool)
            .or_else(|_| row.try_get::<i64, _>(idx).map(|v| Value::Bool(v != 0)))
            .unwrap_or_else(|e| unsupported(&e.to_string())),
        DataTypeKind::Int => {
            if let Ok(v) = row.try_get::<i64, _>(idx) {
                return Value::Int(v);
            }
            if let Ok(v) = row.try_get::<u64, _>(idx) {
                return Value::UInt(v);
            }
            if let Ok(v) = row.try_get::<f64, _>(idx) {
                return Value::Float(v);
            }
            unsupported("integer decode failed")
        }
        DataTypeKind::UInt => {
            if let Ok(v) = row.try_get::<u64, _>(idx) {
                return Value::UInt(v);
            }
            if let Ok(v) = row.try_get::<i64, _>(idx) {
                return Value::Int(v);
            }
            if let Ok(v) = row.try_get::<f64, _>(idx) {
                return Value::Float(v);
            }
            unsupported("unsigned integer decode failed")
        }
        DataTypeKind::Float => {
            if let Ok(v) = row.try_get::<f64, _>(idx) {
                return Value::Float(v);
            }
            if let Ok(v) = row.try_get::<f32, _>(idx) {
                return Value::Float(v as f64);
            }
            unsupported("float decode failed")
        }
        DataTypeKind::Decimal => {
            if let Ok(v) = row.try_get::<BigDecimal, _>(idx) {
                return Value::Decimal(v.to_string());
            }
            if let Ok(v) = row.try_get::<f64, _>(idx) {
                return Value::Float(v);
            }
            unsupported("decimal decode failed")
        }
        DataTypeKind::Text | DataTypeKind::Enum | DataTypeKind::Set => {
            if let Ok(v) = row.try_get::<String, _>(idx) {
                return Value::Text(v);
            }
            if let Ok(v) = row.try_get::<Vec<u8>, _>(idx) {
                return Value::Bytes(b64_encode(&v));
            }
            unsupported("text decode failed")
        }
        DataTypeKind::Bytes | DataTypeKind::Geometry => {
            if let Ok(v) = row.try_get::<Vec<u8>, _>(idx) {
                return Value::Bytes(b64_encode(&v));
            }
            if let Ok(v) = row.try_get::<String, _>(idx) {
                return Value::Bytes(b64_encode(v.as_bytes()));
            }
            unsupported("binary decode failed")
        }
        DataTypeKind::Date => {
            if let Ok(v) = row.try_get::<NaiveDate, _>(idx) {
                return Value::Date(fmt_naive_date(v));
            }
            if let Ok(v) = row.try_get::<String, _>(idx) {
                return Value::Text(v);
            }
            unsupported("date decode failed")
        }
        DataTypeKind::Time => {
            if let Ok(v) = row.try_get::<NaiveTime, _>(idx) {
                return Value::Time(fmt_naive_time(v));
            }
            if let Ok(v) = row.try_get::<String, _>(idx) {
                return Value::Text(v);
            }
            unsupported("time decode failed")
        }
        DataTypeKind::DateTime => {
            if let Ok(v) = row.try_get::<NaiveDateTime, _>(idx) {
                return Value::DateTime(fmt_naive_dt(v));
            }
            if let Ok(v) = row.try_get::<DateTime<Utc>, _>(idx) {
                return Value::DateTime(fmt_dt_utc(v));
            }
            if let Ok(v) = row.try_get::<String, _>(idx) {
                return Value::Text(v);
            }
            unsupported("datetime decode failed")
        }
        DataTypeKind::TimestampTz => {
            if let Ok(v) = row.try_get::<DateTime<Utc>, _>(idx) {
                return Value::DateTime(fmt_dt_utc(v));
            }
            if let Ok(v) = row.try_get::<NaiveDateTime, _>(idx) {
                return Value::DateTime(fmt_naive_dt(v));
            }
            if let Ok(v) = row.try_get::<String, _>(idx) {
                return Value::Text(v);
            }
            unsupported("timestamp decode failed")
        }
        DataTypeKind::Json => {
            if let Ok(Json(v)) = row.try_get::<Json<serde_json::Value>, _>(idx) {
                return Value::Json(v);
            }
            if let Ok(v) = row.try_get::<String, _>(idx) {
                return match serde_json::from_str(&v) {
                    Ok(json) => Value::Json(json),
                    Err(_) => Value::Text(v),
                };
            }
            unsupported("json decode failed")
        }
        DataTypeKind::Uuid => {
            if let Ok(v) = row.try_get::<uuid::Uuid, _>(idx) {
                return Value::Uuid(v.to_string());
            }
            if let Ok(v) = row.try_get::<String, _>(idx) {
                return Value::Text(v);
            }
            unsupported("uuid decode failed")
        }
        DataTypeKind::Bit => {
            if let Ok(v) = row.try_get::<Vec<u8>, _>(idx) {
                return Value::Bytes(b64_encode(&v));
            }
            if let Ok(v) = row.try_get::<u64, _>(idx) {
                return Value::UInt(v);
            }
            if let Ok(v) = row.try_get::<i64, _>(idx) {
                return Value::Int(v);
            }
            unsupported("bit decode failed")
        }
        _ => {
            if let Ok(v) = row.try_get::<String, _>(idx) {
                return Value::Text(v);
            }
            if let Ok(v) = row.try_get::<Vec<u8>, _>(idx) {
                return Value::Bytes(b64_encode(&v));
            }
            if let Ok(v) = row.try_get::<i64, _>(idx) {
                return Value::Int(v);
            }
            if let Ok(v) = row.try_get::<u64, _>(idx) {
                return Value::UInt(v);
            }
            if let Ok(v) = row.try_get::<f64, _>(idx) {
                return Value::Float(v);
            }
            unsupported("no decode path for this type")
        }
    }
}

/// Normalized kind from a native MySQL type name (e.g. `VARCHAR`, `BIGINT UNSIGNED`).
pub(crate) fn kind_mysql(native: &str) -> DataTypeKind {
    let up = native.to_ascii_uppercase();
    let unsigned = up.contains("UNSIGNED");
    let base = up
        .replace("_UNSIGNED", "")
        .replace(" UNSIGNED", "")
        .trim()
        .to_string();
    match base.as_str() {
        "TINYINT" | "SMALLINT" | "MEDIUMINT" | "INT" | "INTEGER" | "BIGINT" | "YEAR" => {
            if unsigned {
                DataTypeKind::UInt
            } else {
                DataTypeKind::Int
            }
        }
        "BOOL" | "BOOLEAN" => DataTypeKind::Bool,
        "FLOAT" | "DOUBLE" | "REAL" => DataTypeKind::Float,
        "DECIMAL" | "NUMERIC" => DataTypeKind::Decimal,
        "CHAR" | "VARCHAR" | "TINYTEXT" | "TEXT" | "MEDIUMTEXT" | "LONGTEXT" => DataTypeKind::Text,
        "ENUM" => DataTypeKind::Enum,
        "SET" => DataTypeKind::Set,
        "BINARY" | "VARBINARY" | "TINYBLOB" | "BLOB" | "MEDIUMBLOB" | "LONGBLOB" => {
            DataTypeKind::Bytes
        }
        "GEOMETRY" => DataTypeKind::Geometry,
        "DATE" => DataTypeKind::Date,
        "TIME" => DataTypeKind::Time,
        "DATETIME" => DataTypeKind::DateTime,
        "TIMESTAMP" => DataTypeKind::TimestampTz,
        "JSON" => DataTypeKind::Json,
        "BIT" => DataTypeKind::Bit,
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

pub(crate) fn bind_mysql<'q>(
    q: Query<'q, MySql, MySqlArguments>,
    params: &[Value],
) -> MieoResult<Query<'q, MySql, MySqlArguments>> {
    let mut q = q;
    for v in params {
        q = match v {
            Value::Null => q.bind(None::<String>),
            Value::Bool(b) => q.bind(*b),
            Value::Int(i) => q.bind(*i),
            Value::UInt(u) => q.bind(*u),
            Value::Float(f) => q.bind(*f),
            Value::Decimal(s) => q.bind(parse_decimal(s)?),
            Value::Text(s) => q.bind(s.clone()),
            Value::Bytes(b) => q.bind(b64_decode(b)?),
            Value::Date(s) | Value::Time(s) | Value::DateTime(s) => q.bind(s.clone()),
            Value::Json(j) => q.bind(Json(j.clone())),
            Value::Uuid(s) => q.bind(
                uuid::Uuid::parse_str(s)
                    .map_err(|e| MieoError::new(ErrorCode::Data, format!("invalid uuid: {e}")))?,
            ),
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

// Ensure the fn-pointer types are actually compatible (compile-time check).
const _: Binder<MySql> = bind_mysql;
const _: Decoder<MySql> = decode_mysql_row;
const _: KindFn = kind_mysql;

// ---------------------------------------------------------------------------
// Unit tests (no server required)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_schedule_recurring() {
        assert_eq!(
            format_event_schedule(
                "RECURRING",
                None,
                Some("1"),
                Some("HOUR"),
                Some("2026-01-01 00:00:00"),
                None,
                Some("ENABLED"),
            ),
            "EVERY 1 HOUR STARTS 2026-01-01 00:00:00"
        );
    }

    #[test]
    fn event_schedule_one_time() {
        assert_eq!(
            format_event_schedule(
                "ONE TIME",
                Some("2026-05-01 12:00:00"),
                None,
                None,
                None,
                None,
                Some("ENABLED"),
            ),
            "ONE TIME AT 2026-05-01 12:00:00"
        );
    }

    #[test]
    fn event_schedule_disabled_flagged() {
        assert_eq!(
            format_event_schedule(
                "RECURRING",
                None,
                Some("1"),
                Some("DAY"),
                None,
                None,
                Some("DISABLED"),
            ),
            "EVERY 1 DAY (DISABLED)"
        );
    }

    #[test]
    fn event_schedule_missing_parts() {
        assert_eq!(
            format_event_schedule("RECURRING", None, None, None, None, None, None),
            "EVERY ?"
        );
        assert_eq!(
            format_event_schedule("ONE TIME", None, None, None, None, None, None),
            "ONE TIME"
        );
    }
}
