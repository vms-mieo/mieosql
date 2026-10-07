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
    Capabilities, ColumnDef, ConnectionSpec, DataTypeKind, DatabaseRef, ErrorCode, ForeignKeyDef,
    IndexDef, MieoError, MieoResult, ObjectKind, ObjectRef, PlaceholderStyle, QueryEvent,
    QueryOptions, QueryStats, SchemaRef, Scope, ServerInfo, Value,
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
        match kind {
            Some(ObjectKind::Table) => sql.push_str(" AND TABLE_TYPE = 'BASE TABLE'"),
            Some(ObjectKind::View) => sql.push_str(" AND TABLE_TYPE = 'VIEW'"),
            Some(
                ObjectKind::Function
                | ObjectKind::Procedure
                | ObjectKind::Trigger
                | ObjectKind::Sequence
                | ObjectKind::MaterializedView,
            ) => return Ok(Vec::new()),
            None => sql.push_str(" AND TABLE_TYPE IN ('BASE TABLE', 'VIEW')"),
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
        let stmt = match kind {
            ObjectKind::Table => format!("SHOW CREATE TABLE {qualified}"),
            ObjectKind::View => format!("SHOW CREATE VIEW {qualified}"),
            _ => return Ok(None),
        };
        let rows = sqlx::raw_sql(sqlx::AssertSqlSafe(stmt))
            .fetch_all(&self.pool)
            .await
            .map_err(map_err)?;
        if let Some(row) = rows.first() {
            let ddl = row.try_get::<String, _>(1).unwrap_or_default();
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
