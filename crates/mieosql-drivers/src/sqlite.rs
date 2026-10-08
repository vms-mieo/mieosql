//! SQLite driver (sqlx, bundled SQLite — no system library required).

use std::time::Duration;

use sqlx::query::Query;
use sqlx::sqlite::{
    Sqlite, SqliteArguments, SqliteConnectOptions, SqlitePool, SqlitePoolOptions, SqliteRow,
};
use sqlx::types::Json;
use sqlx::{ConnectOptions, Row, Transaction, TypeInfo, ValueRef};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use mieosql_core::{
    Capabilities,
    ColumnDef,
    ConnectionSpec,
    CreateDbUser,
    DataTypeKind,
    DatabaseRef,
    DbPrincipal,
    DropDbPrincipal,
    Engine,
    ErrorCode,
    ForeignKeyDef,
    GrantRequest,
    IndexDef,
    MieoError,
    MieoResult,
    ObjectKind,
    ObjectRef,
    PlaceholderStyle,
    QueryEvent,
    QueryOptions,
    QueryStats,
    RevokeRequest,
    RoleGrant,
    SchemaRef,
    Scope,
    ServerInfo,
    Value,
    DbPrincipalDetail,
};

use crate::error_map::map_err;
use crate::ssh_tunnel::ensure_no_tunnel;
use crate::stream::{Binder, Decoder, KindFn, stream_query};
use crate::util::*;

pub struct SqliteDriver {
    pool: SqlitePool,
    tx: Option<Transaction<'static, Sqlite>>,
    spec: ConnectionSpec,
}

pub(crate) fn sqlite_capabilities() -> Capabilities {
    Capabilities {
        engine_kind: mieosql_core::EngineKind::Sql,
        placeholder: PlaceholderStyle::Question,
        list_databases: true,
        list_schemas: false,
        schema_is_database: false,
        transactions: true,
        simple_mode: true,
        ddl: true,
        multiple_databases: false,
        user_management: false,
    }
}

impl SqliteDriver {
    pub async fn connect(spec: &ConnectionSpec) -> MieoResult<Self> {
        ensure_no_tunnel(spec, "SQLite")?;
        let path = spec
            .file
            .clone()
            .or_else(|| spec.database.clone())
            .filter(|p| !p.is_empty())
            .ok_or_else(|| {
                MieoError::new(
                    ErrorCode::Connection,
                    "SQLite requires a database file path",
                )
            })?;
        let path = expand_tilde(&path);
        let in_memory = path == ":memory:" || path.is_empty();

        let mut opts = SqliteConnectOptions::new()
            .filename(&path)
            .busy_timeout(Duration::from_secs(5));
        if !in_memory {
            opts = opts.create_if_missing(false);
        }
        opts = opts.disable_statement_logging();

        // A shared in-memory database must stay on a single connection,
        // otherwise each pool member would see a different database.
        let max_connections = if in_memory { 1 } else { 4 };
        let pool = SqlitePoolOptions::new()
            .max_connections(max_connections)
            .acquire_timeout(Duration::from_secs(spec.connect_timeout_secs.max(5)))
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
        sqlx::query::<Sqlite>("SELECT 1")
            .fetch_one(&self.pool)
            .await
            .map_err(map_err)?;
        Ok(())
    }

    pub async fn server_info(&self) -> MieoResult<ServerInfo> {
        let row = sqlx::query::<Sqlite>("SELECT sqlite_version()")
            .fetch_one(&self.pool)
            .await
            .map_err(map_err)?;
        Ok(ServerInfo {
            engine: Engine::Sqlite,
            version: s(&row, 0),
            version_comment: Some("SQLite (bundled)".into()),
            current_database: self
                .spec
                .file
                .clone()
                .or_else(|| self.spec.database.clone()),
            current_user: None,
        })
    }

    pub async fn list_databases(&self) -> MieoResult<Vec<DatabaseRef>> {
        let rows = sqlx::query::<Sqlite>("PRAGMA database_list")
            .fetch_all(&self.pool)
            .await
            .map_err(map_err)?;
        Ok(rows
            .into_iter()
            .map(|row| {
                let name = s(&row, 1);
                let file = s(&row, 2);
                DatabaseRef {
                    is_system: name == "temp",
                    name,
                    comment: if file.is_empty() { None } else { Some(file) },
                    size_bytes: None,
                    charset: None,
                    collation: None,
                }
            })
            .collect())
    }

    pub async fn list_schemas(&self, _database: Option<&str>) -> MieoResult<Vec<SchemaRef>> {
        Err(MieoError::unsupported(
            "SQLite has no schema level; it has attached databases",
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
            .filter(|d| !d.is_empty())
            .unwrap_or_else(|| "main".to_string());
        let prefix = format!("{}.", quote_ansi_ident(&db));

        // Triggers live in sqlite_master with type = 'trigger'; tbl_name is
        // the table they are attached to.
        if kind == Some(ObjectKind::Trigger) {
            let mut sql = format!(
                "SELECT name, tbl_name FROM {prefix}sqlite_master \
                 WHERE type = 'trigger' AND name NOT LIKE 'sqlite_%'"
            );
            if pattern.is_some() {
                sql.push_str(" AND name LIKE ?");
            }
            sql.push_str(" ORDER BY name");

            let mut q = sqlx::query::<Sqlite>(sqlx::AssertSqlSafe(sql));
            if let Some(p) = pattern {
                q = q.bind(format!("%{p}%"));
            }
            let rows = q.fetch_all(&self.pool).await.map_err(map_err)?;

            return Ok(rows
                .into_iter()
                .map(|row| ObjectRef {
                    name: s(&row, 0),
                    kind: ObjectKind::Trigger,
                    database: Some(db.clone()),
                    schema: None,
                    comment: trigger_comment(os(&row, 1).as_deref()),
                    rows_estimate: None,
                    size_bytes: None,
                    updated_at: None,
                })
                .collect());
        }

        let type_filter = match kind {
            Some(ObjectKind::Table) => "('table')",
            Some(ObjectKind::View) => "('view')",
            Some(
                ObjectKind::Function
                | ObjectKind::Procedure
                | ObjectKind::Trigger
                | ObjectKind::Sequence
                | ObjectKind::MaterializedView,
            ) => return Ok(Vec::new()),
            None => "('table', 'view')",
        };

        let mut sql = format!(
            "SELECT name, type FROM {prefix}sqlite_master \
             WHERE type IN {type_filter} AND name NOT LIKE 'sqlite_%'"
        );
        if pattern.is_some() {
            sql.push_str(" AND name LIKE ?");
        }
        sql.push_str(" ORDER BY name");

        let mut q = sqlx::query::<Sqlite>(sqlx::AssertSqlSafe(sql));
        if let Some(p) = pattern {
            q = q.bind(format!("%{p}%"));
        }
        let rows = q.fetch_all(&self.pool).await.map_err(map_err)?;

        Ok(rows
            .into_iter()
            .map(|row| {
                let obj_type = s(&row, 1);
                ObjectRef {
                    name: s(&row, 0),
                    kind: if obj_type == "view" {
                        ObjectKind::View
                    } else {
                        ObjectKind::Table
                    },
                    database: Some(db.clone()),
                    schema: None,
                    comment: None,
                    rows_estimate: None,
                    size_bytes: None,
                    updated_at: None,
                }
            })
            .collect())
    }

    pub async fn table_columns(&self, scope: &Scope, table: &str) -> MieoResult<Vec<ColumnDef>> {
        let db = self.resolve_db(scope);
        let stmt = format!(
            "PRAGMA {}.table_xinfo({})",
            quote_ansi_ident(&db),
            quote_sqlite_string(table)
        );
        let rows = sqlx::query::<Sqlite>(sqlx::AssertSqlSafe(stmt))
            .fetch_all(&self.pool)
            .await
            .map_err(map_err)?;

        Ok(rows
            .into_iter()
            .map(|row| {
                // Columns: cid, name, type, notnull, dflt_value, pk, hidden
                let cid = i64o(&row, 0).unwrap_or(0);
                let declared = s(&row, 2);
                let notnull = i64o(&row, 3).unwrap_or(0);
                let pk = i64o(&row, 5).unwrap_or(0);
                let hidden = i64o(&row, 6).unwrap_or(0);
                ColumnDef {
                    name: s(&row, 1),
                    ordinal: (cid.max(0) as u32) + 1,
                    native_type: if declared.is_empty() {
                        "BLOB".to_string()
                    } else {
                        declared.clone()
                    },
                    data_type: kind_sqlite_decl(&declared),
                    nullable: notnull == 0,
                    default_value: os(&row, 4),
                    is_primary_key: pk > 0,
                    is_auto_increment: false,
                    is_generated: hidden >= 2,
                    comment: None,
                    char_max_length: None,
                    numeric_precision: None,
                    numeric_scale: None,
                    collation: None,
                }
            })
            .collect())
    }

    pub async fn table_indexes(&self, scope: &Scope, table: &str) -> MieoResult<Vec<IndexDef>> {
        let db = self.resolve_db(scope);
        let list_stmt = format!(
            "PRAGMA {}.index_list({})",
            quote_ansi_ident(&db),
            quote_sqlite_string(table)
        );
        let list = sqlx::query::<Sqlite>(sqlx::AssertSqlSafe(list_stmt))
            .fetch_all(&self.pool)
            .await
            .map_err(map_err)?;

        let mut out = Vec::with_capacity(list.len());
        for row in list {
            // Columns: seq, name, unique, origin, partial
            let name = s(&row, 1);
            let unique = i64o(&row, 2).unwrap_or(0) != 0;
            let origin = s(&row, 3);
            let info_stmt = format!(
                "PRAGMA {}.index_info({})",
                quote_ansi_ident(&db),
                quote_sqlite_string(&name)
            );
            let cols = sqlx::query::<Sqlite>(sqlx::AssertSqlSafe(info_stmt))
                .fetch_all(&self.pool)
                .await
                .map_err(map_err)?;
            let columns: Vec<String> = cols
                .into_iter()
                .map(|c| os(&c, 2).unwrap_or_else(|| "(expression)".to_string()))
                .collect();
            out.push(IndexDef {
                is_primary: origin == "pk",
                is_unique: unique || origin == "pk",
                index_type: Some(match origin.as_str() {
                    "pk" => "PRIMARY KEY".to_string(),
                    "u" => "UNIQUE".to_string(),
                    _ => "CREATE INDEX".to_string(),
                }),
                name,
                columns,
                comment: None,
            });
        }

        // `INTEGER PRIMARY KEY` (rowid alias) tables have no separate index
        // entry in `index_list`; synthesize one so the UI always shows the PK.
        if !out.iter().any(|i| i.is_primary) {
            let info_stmt = format!(
                "PRAGMA {}.table_info({})",
                quote_ansi_ident(&db),
                quote_sqlite_string(table)
            );
            let rows = sqlx::query::<Sqlite>(sqlx::AssertSqlSafe(info_stmt))
                .fetch_all(&self.pool)
                .await
                .map_err(map_err)?;
            let mut pk_columns: Vec<(i64, String)> = rows
                .iter()
                .filter_map(|r| {
                    let pk_order = i64o(r, 5).unwrap_or(0);
                    (pk_order > 0).then(|| (pk_order, s(r, 1)))
                })
                .collect();
            if !pk_columns.is_empty() {
                pk_columns.sort_by_key(|(order, _)| *order);
                out.push(IndexDef {
                    name: "PRIMARY".to_string(),
                    columns: pk_columns.into_iter().map(|(_, name)| name).collect(),
                    is_unique: true,
                    is_primary: true,
                    index_type: Some("PRIMARY KEY".to_string()),
                    comment: None,
                });
            }
        }

        Ok(out)
    }

    pub async fn foreign_keys(&self, scope: &Scope, table: &str) -> MieoResult<Vec<ForeignKeyDef>> {
        let db = self.resolve_db(scope);
        let stmt = format!(
            "PRAGMA {}.foreign_key_list({})",
            quote_ansi_ident(&db),
            quote_sqlite_string(table)
        );
        let rows = sqlx::query::<Sqlite>(sqlx::AssertSqlSafe(stmt))
            .fetch_all(&self.pool)
            .await
            .map_err(map_err)?;

        // Columns: id, seq, table, from, to, on_update, on_delete, match
        let mut out: Vec<ForeignKeyDef> = Vec::new();
        for row in rows {
            let id = i64o(&row, 0).unwrap_or(0);
            let ref_table = s(&row, 2);
            let from = os(&row, 3);
            let to = os(&row, 4);
            let name = format!("fk_{table}_{id}");
            match out.last_mut() {
                Some(fk) if fk.name == name => {
                    if let Some(c) = from {
                        fk.columns.push(c);
                    }
                    if let Some(c) = to {
                        fk.ref_columns.push(c);
                    }
                }
                _ => out.push(ForeignKeyDef {
                    name,
                    columns: from.into_iter().collect(),
                    ref_table: ref_table.clone(),
                    ref_schema: Some(db.clone()),
                    ref_columns: to.into_iter().collect(),
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
        let db = self.resolve_db(scope);
        let obj_type = match kind {
            ObjectKind::Table => "table",
            ObjectKind::View => "view",
            ObjectKind::Trigger => "trigger",
            _ => return Ok(None),
        };
        let stmt = format!(
            "SELECT sql FROM {}.sqlite_master WHERE name = ? AND type = ?",
            quote_ansi_ident(&db)
        );
        let row = sqlx::query::<Sqlite>(sqlx::AssertSqlSafe(stmt))
            .bind(name)
            .bind(obj_type)
            .fetch_optional(&self.pool)
            .await
            .map_err(map_err)?;
        Ok(row.and_then(|r| os(&r, 0)))
    }

    // SQLite is a single-user embedded database: no users, roles or grants.
    pub async fn list_db_users(&mut self) -> MieoResult<Vec<DbPrincipal>> {
        Ok(Vec::new())
    }

    pub async fn list_db_roles(&mut self) -> MieoResult<Vec<DbPrincipal>> {
        Ok(Vec::new())
    }

    pub async fn db_role_members(&mut self) -> MieoResult<Vec<RoleGrant>> {
        Ok(Vec::new())
    }

    pub async fn db_user_grants(
        &mut self,
        _name: &str,
        _host: Option<&str>,
    ) -> MieoResult<Vec<String>> {
        Ok(Vec::new())
    }

    fn users_unsupported<T>(what: &str) -> MieoResult<T> {
        Err(MieoError::unsupported(format!(
            "SQLite has no {what}: it is a single-user embedded database"
        )))
    }

    pub async fn db_principal_detail(
        &mut self,
        _name: &str,
        _host: Option<&str>,
    ) -> MieoResult<DbPrincipalDetail> {
        Err(MieoError::unsupported(
            "this engine has no user management",
        ))
    }

    pub async fn create_db_user(&mut self, _req: &CreateDbUser) -> MieoResult<()> {
        Self::users_unsupported("user management")
    }

    pub async fn drop_db_principal(&mut self, _req: &DropDbPrincipal) -> MieoResult<()> {
        Self::users_unsupported("user management")
    }

    pub async fn grant(&mut self, _req: &GrantRequest) -> MieoResult<()> {
        Self::users_unsupported("GRANT")
    }

    pub async fn revoke(&mut self, _req: &RevokeRequest) -> MieoResult<()> {
        Self::users_unsupported("REVOKE")
    }

    pub async fn set_user_password(
        &mut self,
        _name: &str,
        _host: Option<&str>,
        _password: &str,
    ) -> MieoResult<()> {
        Self::users_unsupported("user management")
    }

    pub async fn use_database(&mut self, database: &str) -> MieoResult<()> {
        if database == "main" || database.is_empty() {
            return Ok(());
        }
        Err(MieoError::unsupported(
            "switching attached SQLite databases is planned for a later phase",
        ))
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
            stream_query::<Sqlite>(
                &mut **tx,
                query_id,
                sql,
                params,
                options,
                sink,
                cancel,
                bind_sqlite,
                decode_sqlite_row,
                kind_sqlite_storage,
            )
            .await
        } else {
            let mut conn = self.pool.acquire().await.map_err(map_err)?;
            stream_query::<Sqlite>(
                &mut conn,
                query_id,
                sql,
                params,
                options,
                sink,
                cancel,
                bind_sqlite,
                decode_sqlite_row,
                kind_sqlite_storage,
            )
            .await
        }
    }

    fn resolve_db(&self, scope: &Scope) -> String {
        scope
            .database
            .clone()
            .filter(|d| !d.is_empty())
            .unwrap_or_else(|| "main".to_string())
    }
}

// ---------------------------------------------------------------------------
// Row helpers for introspection queries
// ---------------------------------------------------------------------------

/// Explorer comment for a trigger row: the table it is attached to.
fn trigger_comment(table: Option<&str>) -> Option<String> {
    table
        .filter(|t| !t.is_empty())
        .map(|t| format!("ON {t}"))
}

fn s(row: &SqliteRow, idx: usize) -> String {
    row.try_get::<String, _>(idx).unwrap_or_default()
}

fn os(row: &SqliteRow, idx: usize) -> Option<String> {
    row.try_get::<Option<String>, _>(idx).ok().flatten()
}

fn i64o(row: &SqliteRow, idx: usize) -> Option<i64> {
    row.try_get::<Option<i64>, _>(idx).ok().flatten()
}

// ---------------------------------------------------------------------------
// Value decoding
// ---------------------------------------------------------------------------

pub(crate) fn decode_sqlite_row(row: &SqliteRow) -> Vec<Value> {
    (0..row.len()).map(|i| decode_sqlite_cell(row, i)).collect()
}

fn decode_sqlite_cell(row: &SqliteRow, idx: usize) -> Value {
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

    macro_rules! unsupported {
        ($reason:expr) => {
            Value::Unsupported {
                native_type: native.clone(),
                reason: $reason.to_string(),
            }
        };
    }

    match kind_sqlite_storage(&native) {
        DataTypeKind::Int => {
            if let Ok(v) = row.try_get::<i64, _>(idx) {
                return Value::Int(v);
            }
            if let Ok(v) = row.try_get::<f64, _>(idx) {
                return Value::Float(v);
            }
            if let Ok(v) = row.try_get::<String, _>(idx) {
                return Value::Text(v);
            }
            unsupported!("integer decode failed")
        }
        DataTypeKind::Float => {
            if let Ok(v) = row.try_get::<f64, _>(idx) {
                return Value::Float(v);
            }
            if let Ok(v) = row.try_get::<i64, _>(idx) {
                return Value::Int(v);
            }
            unsupported!("real decode failed")
        }
        DataTypeKind::Text => {
            if let Ok(v) = row.try_get::<String, _>(idx) {
                return Value::Text(v);
            }
            if let Ok(v) = row.try_get::<Vec<u8>, _>(idx) {
                return Value::Bytes(b64_encode(&v));
            }
            unsupported!("text decode failed")
        }
        DataTypeKind::Bytes => {
            if let Ok(v) = row.try_get::<Vec<u8>, _>(idx) {
                return Value::Bytes(b64_encode(&v));
            }
            if let Ok(v) = row.try_get::<String, _>(idx) {
                return Value::Text(v);
            }
            unsupported!("blob decode failed")
        }
        _ => {
            if let Ok(v) = row.try_get::<String, _>(idx) {
                return Value::Text(v);
            }
            if let Ok(v) = row.try_get::<i64, _>(idx) {
                return Value::Int(v);
            }
            if let Ok(v) = row.try_get::<f64, _>(idx) {
                return Value::Float(v);
            }
            if let Ok(v) = row.try_get::<Vec<u8>, _>(idx) {
                return Value::Bytes(b64_encode(&v));
            }
            unsupported!("no decode path for this storage class")
        }
    }
}

/// Runtime storage class reported by the SQLite wire format.
pub(crate) fn kind_sqlite_storage(native: &str) -> DataTypeKind {
    match native.to_ascii_uppercase().as_str() {
        "INTEGER" | "INT" => DataTypeKind::Int,
        "REAL" | "FLOAT" | "DOUBLE" => DataTypeKind::Float,
        "TEXT" => DataTypeKind::Text,
        "BLOB" => DataTypeKind::Bytes,
        _ => DataTypeKind::Unknown,
    }
}

/// Declared type as written in the schema (SQLite type affinity rules).
fn kind_sqlite_decl(declared: &str) -> DataTypeKind {
    let base = declared
        .split('(')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_uppercase();
    match base.as_str() {
        "" => DataTypeKind::Unknown,
        _ => {
            if base.contains("INT") {
                DataTypeKind::Int
            } else if base.contains("CHAR") || base.contains("CLOB") || base.contains("TEXT") {
                DataTypeKind::Text
            } else if base.contains("BLOB") || base.is_empty() {
                DataTypeKind::Bytes
            } else if base.contains("REAL") || base.contains("FLOA") || base.contains("DOUB") {
                DataTypeKind::Float
            } else if base.contains("NUMERIC") || base.contains("DECIMAL") {
                DataTypeKind::Decimal
            } else if base.contains("BOOL") {
                DataTypeKind::Bool
            } else if base.contains("DATETIME") || base.contains("TIMESTAMP") {
                DataTypeKind::DateTime
            } else if base.contains("DATE") {
                DataTypeKind::Date
            } else if base.contains("TIME") {
                DataTypeKind::Time
            } else if base.contains("JSON") {
                DataTypeKind::Json
            } else if base.contains("UUID") {
                DataTypeKind::Uuid
            } else {
                DataTypeKind::Unknown
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Parameter binding
// ---------------------------------------------------------------------------

pub(crate) fn bind_sqlite<'q>(
    q: Query<'q, Sqlite, SqliteArguments>,
    params: &[Value],
) -> MieoResult<Query<'q, Sqlite, SqliteArguments>> {
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
                        "unsigned integer too large for SQLite (max i64)",
                    )
                })?;
                q.bind(as_i64)
            }
            Value::Float(f) => q.bind(*f),
            // SQLite has no decimal type; store as text, NUMERIC affinity converts.
            Value::Decimal(s) => q.bind(s.clone()),
            Value::Text(s) => q.bind(s.clone()),
            Value::Bytes(b) => q.bind(b64_decode(b)?),
            Value::Date(s) | Value::Time(s) | Value::DateTime(s) => q.bind(s.clone()),
            Value::Json(j) => q.bind(Json(j.clone())),
            Value::Uuid(s) => q.bind(s.clone()),
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

const _: Binder<Sqlite> = bind_sqlite;
const _: Decoder<Sqlite> = decode_sqlite_row;
const _: KindFn = kind_sqlite_storage;

// ---------------------------------------------------------------------------
// Unit tests (no server required)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trigger_comment_names_table() {
        assert_eq!(trigger_comment(Some("users")), Some("ON users".to_string()));
    }

    #[test]
    fn trigger_comment_empty_is_none() {
        assert_eq!(trigger_comment(None), None);
        assert_eq!(trigger_comment(Some("")), None);
    }
}
