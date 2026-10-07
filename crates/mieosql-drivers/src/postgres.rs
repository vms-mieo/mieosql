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
    Capabilities, ColumnDef, ConnectionSpec, DataTypeKind, DatabaseRef, Engine, ErrorCode,
    ForeignKeyDef, IndexDef, MieoError, MieoResult, ObjectKind, ObjectRef, PlaceholderStyle,
    QueryEvent, QueryOptions, QueryStats, SchemaRef, Scope, ServerInfo, Value,
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
        placeholder: PlaceholderStyle::DollarNumbered,
        list_databases: true,
        list_schemas: true,
        schema_is_database: false,
        transactions: true,
        simple_mode: true,
        // Full Postgres DDL generation (pg_dump style) is a P1 item.
        ddl: false,
        multiple_databases: true,
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
        let kind_filter: Option<Vec<&str>> = match kind {
            Some(ObjectKind::Table) => Some(vec!["r", "p"]),
            Some(ObjectKind::View) => Some(vec!["v"]),
            Some(ObjectKind::MaterializedView) => Some(vec!["m"]),
            Some(ObjectKind::Sequence) => Some(vec!["S"]),
            Some(ObjectKind::Function | ObjectKind::Procedure | ObjectKind::Trigger) => {
                return Ok(Vec::new());
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
        _scope: &Scope,
        _kind: ObjectKind,
        _name: &str,
    ) -> MieoResult<Option<String>> {
        // Full Postgres DDL reconstruction (pg_dump style) is planned for P1.
        Ok(None)
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
