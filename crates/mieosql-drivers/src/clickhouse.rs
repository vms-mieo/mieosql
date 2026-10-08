//! ClickHouse driver (official `clickhouse` crate — HTTP protocol, 8123/8443).
//!
//! Query results stream through ClickHouse's
//! `JSONCompactEachRowWithNamesAndTypes` format: the first two lines are the
//! column names and native types, every following line is one row array. That
//! keeps the driver fully dynamic (no compile-time row structs) and lossless
//! enough for the grid: 64-bit integers, decimals and timestamps are preserved
//! as strings exactly like the other drivers.

use std::time::{Duration, Instant};

use bytes::Bytes;
use clickhouse::{Client, Compression};
use serde_json::Value as Json;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use mieosql_core::{
    Capabilities,
    ColumnDef,
    ColumnMeta,
    ConnectionSpec,
    CreateDbUser,
    DataTypeKind,
    DatabaseRef,
    DbPrincipal,
    DbPrincipalDetail,
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
    SslMode,
    Value,
};

use crate::ssh_tunnel::{SshTunnel, prepare_endpoint};
use crate::util::quote_ansi_ident;

/// Rows per streamed `Batch` event.
const DEFAULT_FETCH_SIZE: usize = 512;
/// Wire format used for both metadata and user queries.
const FORMAT: &str = "JSONCompactEachRowWithNamesAndTypes";

pub struct ClickHouseDriver {
    client: Client,
    spec: ConnectionSpec,
    /// Database targeted when a scope does not name one (`use_database` updates it).
    current_database: String,
    /// Keeps the SSH tunnel (if any) alive for the connection lifetime.
    _ssh_tunnel: Option<SshTunnel>,
}

pub(crate) fn clickhouse_capabilities() -> Capabilities {
    Capabilities {
        engine_kind: mieosql_core::EngineKind::Sql,
        placeholder: PlaceholderStyle::Question,
        list_databases: true,
        list_schemas: false,
        schema_is_database: true,
        transactions: false,
        simple_mode: true,
        ddl: true,
        multiple_databases: true,
        // User/role management is intentionally disabled in this build.
        user_management: false,
    }
}

const USER_ADMIN_HINT: &str = "ClickHouse user management is not supported yet; use `clickhouse-client` grants";

impl ClickHouseDriver {
    pub async fn connect(spec: &ConnectionSpec) -> MieoResult<Self> {
        // ClickHouse's HTTP interface: 8123 = plain, 8443 = TLS. `Prefer`
        // therefore stays on HTTP unless the port is a well-known TLS port —
        // the default profile (port 8123, Prefer) must connect to plain
        // servers, while the ClickHouse Cloud preset (8443 + Require) uses
        // HTTPS.
        let secure = match spec.ssl_mode {
            SslMode::Disable => false,
            SslMode::Prefer => spec.port.is_some_and(is_tls_port),
            _ => true,
        };
        let scheme = if secure { "https" } else { "http" };
        let (host, port, _ssh_tunnel) =
            prepare_endpoint(spec, "localhost", if secure { 8443 } else { 8123 }).await?;
        let url = format!("{scheme}://{host}:{port}");

        let database = spec
            .database
            .as_deref()
            .map(str::trim)
            .filter(|d| !d.is_empty())
            .unwrap_or("default")
            .to_string();

        let mut client = Client::default()
            .with_url(url)
            .with_database(database.clone())
            .with_compression(Compression::Lz4)
            .with_product_info("MieoSQL", env!("CARGO_PKG_VERSION"));
        if let Some(user) = spec.username.as_deref().filter(|u| !u.is_empty()) {
            client = client.with_user(user);
        }
        if let Some(password) = spec.password.as_deref().filter(|p| !p.is_empty()) {
            client = client.with_password(password);
        }

        let mut driver = ClickHouseDriver {
            client,
            spec: spec.clone(),
            current_database: database,
            _ssh_tunnel,
        };
        // The HTTP client is lazy: probe once so a wrong host/port/credentials
        // fails here (bounded by `connect_timeout_secs`) instead of reporting a
        // healthy session that cannot serve a single query.
        driver.ping().await.map_err(|e| {
            e.with_hint("check host/port, SSL mode (8123 = HTTP, 8443 = HTTPS), user and password")
        })?;
        Ok(driver)
    }

    pub fn spec(&self) -> &ConnectionSpec {
        &self.spec
    }

    pub fn in_transaction(&self) -> bool {
        false
    }

    // ------------------------------------------------------------------
    // Connection / server info
    // ------------------------------------------------------------------

    pub async fn ping(&mut self) -> MieoResult<()> {
        let (_, _, rows) = self.fetch_json("SELECT 1", &[]).await?;
        if rows.is_empty() {
            return Err(MieoError::new(
                ErrorCode::Connection,
                "ClickHouse ping returned no rows",
            ));
        }
        Ok(())
    }

    pub async fn server_info(&mut self) -> MieoResult<ServerInfo> {
        let (_, _, rows) = self
            .fetch_json("SELECT version(), currentUser(), currentDatabase()", &[])
            .await?;
        let row = rows.first().ok_or_else(|| {
            MieoError::new(ErrorCode::Connection, "ClickHouse server info returned no rows")
        })?;
        Ok(ServerInfo {
            engine: Engine::ClickHouse,
            version: json_string(row.first()).unwrap_or_else(|| "unknown".into()),
            version_comment: None,
            current_database: json_string(row.get(2)),
            current_user: json_string(row.get(1)),
        })
    }

    // ------------------------------------------------------------------
    // Catalog browsing
    // ------------------------------------------------------------------

    pub async fn list_databases(&mut self) -> MieoResult<Vec<DatabaseRef>> {
        let (_, _, rows) = self
            .fetch_json("SELECT name FROM system.databases ORDER BY name", &[])
            .await?;
        Ok(rows
            .iter()
            .filter_map(|row| json_string(row.first()))
            .map(|name| {
                let lower = name.to_ascii_lowercase();
                let is_system = lower == "system"
                    || lower == "information_schema"
                    || lower.starts_with("_");
                DatabaseRef {
                    name,
                    comment: None,
                    size_bytes: None,
                    charset: None,
                    collation: None,
                    is_system,
                }
            })
            .collect())
    }

    pub async fn list_schemas(&mut self, _database: Option<&str>) -> MieoResult<Vec<SchemaRef>> {
        // ClickHouse has no schema level; the UI uses databases directly.
        Ok(Vec::new())
    }

    pub async fn list_objects(
        &mut self,
        scope: &Scope,
        kind: Option<ObjectKind>,
        pattern: Option<&str>,
    ) -> MieoResult<Vec<ObjectRef>> {
        // ClickHouse has no functions/procedures/triggers/sequences objects.
        if matches!(
            kind,
            Some(ObjectKind::Function | ObjectKind::Procedure | ObjectKind::Trigger | ObjectKind::Sequence)
        ) {
            return Ok(Vec::new());
        }

        let db = self.database_name(scope);
        let mut sql = String::from(
            "SELECT name, engine, total_rows, comment, metadata_modification_time \
             FROM system.tables WHERE database = ? AND is_temporary = 0",
        );
        match kind {
            Some(ObjectKind::View) => sql.push_str(" AND engine = 'View'"),
            Some(ObjectKind::MaterializedView) => sql.push_str(" AND engine = 'MaterializedView'"),
            Some(ObjectKind::Table) => {
                sql.push_str(" AND engine NOT IN ('View', 'MaterializedView')")
            }
            _ => sql.push_str(" AND engine NOT IN ('View', 'MaterializedView')"),
        }
        let mut params = vec![Json::String(db.clone())];
        if let Some(p) = pattern.filter(|p| !p.trim().is_empty()) {
            sql.push_str(" AND positionCaseInsensitive(name, ?) > 0");
            params.push(Json::String(p.trim().to_string()));
        }
        sql.push_str(" ORDER BY name");

        let (_, _, rows) = self.fetch_json(&sql, &params).await?;
        Ok(rows
            .iter()
            .filter_map(|row| {
                let name = json_string(row.first())?;
                let engine = json_string(row.get(1)).unwrap_or_default();
                let kind = match engine.as_str() {
                    "View" => ObjectKind::View,
                    "MaterializedView" => ObjectKind::MaterializedView,
                    _ => ObjectKind::Table,
                };
                Some(ObjectRef {
                    name,
                    kind,
                    database: Some(db.clone()),
                    schema: None,
                    comment: json_string(row.get(3)).filter(|c| !c.is_empty()),
                    rows_estimate: json_u64(row.get(2)),
                    size_bytes: None,
                    updated_at: json_string(row.get(4)),
                })
            })
            .collect())
    }

    pub async fn table_columns(&mut self, scope: &Scope, table: &str) -> MieoResult<Vec<ColumnDef>> {
        let db = self.database_name(scope);
        let (_, _, rows) = self
            .fetch_json(
                "SELECT name, type, position, default_kind, default_expression, \
                        is_in_primary_key, comment \
                 FROM system.columns WHERE database = ? AND table = ? ORDER BY position",
                &[Json::String(db), Json::String(table.to_string())],
            )
            .await?;

        Ok(rows
            .iter()
            .filter_map(|row| {
                let name = json_string(row.first())?;
                let native_type = json_string(row.get(1)).unwrap_or_default();
                let (precision, scale) = decimal_shape(&native_type);
                Some(ColumnDef {
                    name,
                    ordinal: json_u64(row.get(2)).unwrap_or(0) as u32,
                    data_type: kind_of(&native_type),
                    nullable: native_type.trim_start().starts_with("Nullable("),
                    default_value: json_string(row.get(4)).filter(|v| !v.is_empty()),
                    is_primary_key: json_bool(row.get(5)),
                    is_auto_increment: false,
                    is_generated: matches!(
                        json_string(row.get(3)).as_deref(),
                        Some("MATERIALIZED") | Some("ALIAS")
                    ),
                    comment: json_string(row.get(6)).filter(|c| !c.is_empty()),
                    char_max_length: fixed_string_len(&native_type),
                    numeric_precision: precision,
                    numeric_scale: scale,
                    collation: None,
                    native_type: native_type.clone(),
                })
            })
            .collect())
    }

    pub async fn table_indexes(&mut self, scope: &Scope, table: &str) -> MieoResult<Vec<IndexDef>> {
        let db = self.database_name(scope);
        let mut indexes = Vec::new();

        let (_, _, rows) = self
            .fetch_json(
                "SELECT name, expr, type FROM system.data_skipping_indices \
                 WHERE database = ? AND table = ? ORDER BY name",
                &[Json::String(db.clone()), Json::String(table.to_string())],
            )
            .await?;
        for row in &rows {
            let Some(name) = json_string(row.first()) else {
                continue;
            };
            let expr = json_string(row.get(1)).unwrap_or_default();
            indexes.push(IndexDef {
                name,
                columns: vec![expr],
                is_unique: false,
                is_primary: false,
                index_type: json_string(row.get(2)),
                comment: None,
            });
        }

        // Sort key doubles as the primary index in ClickHouse.
        let (_, _, pk_rows) = self
            .fetch_json(
                "SELECT primary_key FROM system.tables WHERE database = ? AND name = ?",
                &[Json::String(db), Json::String(table.to_string())],
            )
            .await?;
        if let Some(pk) = pk_rows
            .first()
            .and_then(|row| json_string(row.first()))
            .filter(|p| !p.trim().is_empty())
        {
            indexes.insert(
                0,
                IndexDef {
                    name: "PRIMARY".into(),
                    columns: vec![pk],
                    is_unique: false,
                    is_primary: true,
                    index_type: Some("primary key".into()),
                    comment: None,
                },
            );
        }
        Ok(indexes)
    }

    pub async fn foreign_keys(
        &mut self,
        _scope: &Scope,
        _table: &str,
    ) -> MieoResult<Vec<ForeignKeyDef>> {
        // ClickHouse does not enforce foreign keys.
        Ok(Vec::new())
    }

    pub async fn object_ddl(
        &mut self,
        scope: &Scope,
        _kind: ObjectKind,
        name: &str,
    ) -> MieoResult<Option<String>> {
        let db = self.database_name(scope);
        let (_, _, rows) = self
            .fetch_json(
                "SELECT create_table_query FROM system.tables \
                 WHERE database = ? AND name = ?",
                &[Json::String(db), Json::String(name.to_string())],
            )
            .await?;
        Ok(rows
            .first()
            .and_then(|row| json_string(row.first()))
            .filter(|ddl| !ddl.trim().is_empty()))
    }

    pub async fn use_database(&mut self, database: &str) -> MieoResult<()> {
        let database = database.trim();
        if database.is_empty() {
            return Err(MieoError::new(ErrorCode::Data, "database name is empty"));
        }
        self.current_database = database.to_string();
        self.client = self.client.clone().with_database(database);
        Ok(())
    }

    // ------------------------------------------------------------------
    // Users / roles / grants (not supported in this build)
    // ------------------------------------------------------------------

    pub async fn list_db_users(&mut self) -> MieoResult<Vec<DbPrincipal>> {
        Err(MieoError::unsupported(USER_ADMIN_HINT))
    }

    pub async fn list_db_roles(&mut self) -> MieoResult<Vec<DbPrincipal>> {
        Err(MieoError::unsupported(USER_ADMIN_HINT))
    }

    pub async fn db_role_members(&mut self) -> MieoResult<Vec<RoleGrant>> {
        Err(MieoError::unsupported(USER_ADMIN_HINT))
    }

    pub async fn db_user_grants(
        &mut self,
        _name: &str,
        _host: Option<&str>,
    ) -> MieoResult<Vec<String>> {
        Err(MieoError::unsupported(USER_ADMIN_HINT))
    }

    pub async fn create_db_user(&mut self, _req: &CreateDbUser) -> MieoResult<()> {
        Err(MieoError::unsupported(USER_ADMIN_HINT))
    }

    pub async fn drop_db_principal(&mut self, _req: &DropDbPrincipal) -> MieoResult<()> {
        Err(MieoError::unsupported(USER_ADMIN_HINT))
    }

    pub async fn grant(&mut self, _req: &GrantRequest) -> MieoResult<()> {
        Err(MieoError::unsupported(USER_ADMIN_HINT))
    }

    pub async fn revoke(&mut self, _req: &RevokeRequest) -> MieoResult<()> {
        Err(MieoError::unsupported(USER_ADMIN_HINT))
    }

    pub async fn db_principal_detail(
        &mut self,
        _name: &str,
        _host: Option<&str>,
    ) -> MieoResult<DbPrincipalDetail> {
        Err(MieoError::unsupported(USER_ADMIN_HINT))
    }

    pub async fn set_user_password(
        &mut self,
        _name: &str,
        _host: Option<&str>,
        _password: &str,
    ) -> MieoResult<()> {
        Err(MieoError::unsupported(USER_ADMIN_HINT))
    }

    // ------------------------------------------------------------------
    // Transactions (ClickHouse HTTP is stateless)
    // ------------------------------------------------------------------

    pub async fn begin(&mut self) -> MieoResult<()> {
        Err(MieoError::unsupported(
            "ClickHouse HTTP connections do not support interactive transactions",
        ))
    }

    pub async fn commit(&mut self) -> MieoResult<()> {
        Err(MieoError::unsupported(
            "ClickHouse HTTP connections do not support interactive transactions",
        ))
    }

    pub async fn rollback(&mut self) -> MieoResult<()> {
        Err(MieoError::unsupported(
            "ClickHouse HTTP connections do not support interactive transactions",
        ))
    }

    // ------------------------------------------------------------------
    // Query execution
    // ------------------------------------------------------------------

    pub async fn run_query(
        &mut self,
        query_id: &str,
        sql: String,
        params: Vec<Value>,
        options: &QueryOptions,
        sink: &mpsc::Sender<QueryEvent>,
        cancel: &CancellationToken,
    ) -> MieoResult<QueryStats> {
        let start = Instant::now();
        let deadline = options
            .timeout_ms
            .map(|ms| tokio::time::Instant::now() + Duration::from_millis(ms));
        let sql = sql.trim().to_string();
        if sql.is_empty() {
            return Err(MieoError::new(ErrorCode::Syntax, "empty statement"));
        }

        let mut stats = QueryStats::default();

        if !statement_returns_rows(&sql) {
            // DDL / INSERT / OPTIMIZE / ... — no result set.
            let mut query = self.client.query(&sql);
            for p in &params {
                query = query.bind(value_to_json(p));
            }
            if let Some(ms) = options.timeout_ms {
                query = query.with_setting("max_execution_time", max_execution_secs(ms));
            }
            run_with_cancel_and_timeout(query.execute(), cancel, deadline).await?;
            stats.elapsed_ms = start.elapsed().as_millis() as u64;
            return Ok(stats);
        }

        if has_explicit_format(&sql) {
            return Err(MieoError::unsupported(
                "explicit FORMAT clauses are handled by the result grid; remove FORMAT from the statement",
            ));
        }

        let mut query = self.client.query(&sql);
        for p in &params {
            query = query.bind(value_to_json(p));
        }
        if let Some(ms) = options.timeout_ms {
            query = query.with_setting("max_execution_time", max_execution_secs(ms));
        }
        let mut cursor = query.fetch_bytes(FORMAT).map_err(map_ch_err)?;

        let fetch_size = options
            .fetch_size
            .max(1)
            .try_into()
            .unwrap_or(DEFAULT_FETCH_SIZE)
            .max(1)
            .min(8192);
        let cap = options.max_rows;

        let mut lines = LineBuffer::default();
        let mut names: Vec<String> = Vec::new();
        let mut types: Vec<String> = Vec::new();
        let mut columns_sent = false;
        let mut batch: Vec<Vec<Value>> = Vec::with_capacity(fetch_size);
        let mut batch_index: u64 = 0;
        let mut truncated = false;

        loop {
            let chunk = next_chunk(&mut cursor, cancel, deadline).await?;
            let done = chunk.is_none();
            let complete = match chunk {
                Some(bytes) => lines.push(&bytes),
                None => lines.flush(),
            };

            for line in complete {
                let row: Vec<Json> = serde_json::from_str(&line).map_err(|e| {
                    MieoError::new(
                        ErrorCode::Data,
                        format!("invalid ClickHouse JSON row: {e}"),
                    )
                })?;

                if !columns_sent {
                    if names.is_empty() {
                        names = row.iter().map(json_to_string).collect();
                        continue;
                    }
                    // Second header line: native types.
                    types = row.iter().map(json_to_string).collect();
                    let columns: Vec<ColumnMeta> = names
                        .iter()
                        .enumerate()
                        .map(|(i, name)| {
                            let native = types.get(i).cloned().unwrap_or_default();
                            ColumnMeta {
                                name: name.clone(),
                                ordinal: i as u32,
                                native_type: native.clone(),
                                kind: kind_of(&native),
                                nullable: Some(native.trim_start().starts_with("Nullable(")),
                            }
                        })
                        .collect();
                    send_event(
                        sink,
                        QueryEvent::Columns {
                            query_id: query_id.to_string(),
                            columns,
                        },
                    )
                    .await?;
                    columns_sent = true;
                    continue;
                }

                if cap.is_some_and(|m| stats.rows >= m) {
                    // Keep draining so the transport finishes cleanly.
                    truncated = true;
                    continue;
                }

                stats.rows += 1;
                batch.push(
                    row.iter()
                        .enumerate()
                        .map(|(i, v)| value_from_json(types.get(i).map(String::as_str), v))
                        .collect(),
                );
                if batch.len() >= fetch_size {
                    batch_index += 1;
                    send_event(
                        sink,
                        QueryEvent::Batch {
                            query_id: query_id.to_string(),
                            rows: std::mem::take(&mut batch),
                            batch_index,
                        },
                    )
                    .await?;
                    batch.reserve(fetch_size);
                }
            }

            if done {
                break;
            }
        }

        // Header-only responses (zero rows) still need column metadata.
        if !columns_sent && !names.is_empty() {
            let columns: Vec<ColumnMeta> = names
                .iter()
                .enumerate()
                .map(|(i, name)| {
                    let native = types.get(i).cloned().unwrap_or_default();
                    ColumnMeta {
                        name: name.clone(),
                        ordinal: i as u32,
                        native_type: native.clone(),
                        kind: kind_of(&native),
                        nullable: None,
                    }
                })
                .collect();
            send_event(
                sink,
                QueryEvent::Columns {
                    query_id: query_id.to_string(),
                    columns,
                },
            )
            .await?;
        }

        if !batch.is_empty() {
            batch_index += 1;
            send_event(
                sink,
                QueryEvent::Batch {
                    query_id: query_id.to_string(),
                    rows: batch,
                    batch_index,
                },
            )
            .await?;
        }

        stats.truncated = truncated;
        stats.elapsed_ms = start.elapsed().as_millis() as u64;
        Ok(stats)
    }

    // ------------------------------------------------------------------
    // Internals
    // ------------------------------------------------------------------

    /// Database named by `scope`, falling back to the driver default.
    fn database_name(&self, scope: &Scope) -> String {
        scope
            .database
            .as_deref()
            .map(str::trim)
            .filter(|d| !d.is_empty())
            .unwrap_or(&self.current_database)
            .to_string()
    }

    /// Run a metadata query and return `(names, types, rows)`.
    async fn fetch_json(
        &self,
        sql: &str,
        params: &[Json],
    ) -> MieoResult<(Vec<String>, Vec<String>, Vec<Vec<Json>>)> {
        let connect_timeout = Duration::from_secs(self.spec.connect_timeout_secs.max(1));
        let mut query = self.client.query(sql);
        for p in params {
            query = query.bind(p.clone());
        }
        let mut cursor = query.fetch_bytes(FORMAT).map_err(map_ch_err)?;

        let buffered = tokio::time::timeout(connect_timeout, cursor.collect())
            .await
            .map_err(|_| MieoError::new(ErrorCode::Timeout, "ClickHouse request timed out"))?
            .map_err(map_ch_err)?;

        let mut names: Vec<String> = Vec::new();
        let mut types: Vec<String> = Vec::new();
        let mut rows: Vec<Vec<Json>> = Vec::new();
        let mut lines = LineBuffer::default();
        for line in lines.push(&buffered) {
            let row: Vec<Json> = serde_json::from_str(&line).map_err(|e| {
                MieoError::new(ErrorCode::Data, format!("invalid ClickHouse JSON row: {e}"))
            })?;
            if names.is_empty() {
                names = row.iter().map(json_to_string).collect();
            } else if types.is_empty() {
                types = row.iter().map(json_to_string).collect();
            } else {
                rows.push(row);
            }
        }
        for line in lines.flush() {
            if let Ok(row) = serde_json::from_str::<Vec<Json>>(&line) {
                rows.push(row);
            }
        }
        Ok((names, types, rows))
    }
}

// ---------------------------------------------------------------------------
// Streaming helpers
// ---------------------------------------------------------------------------

/// Splits raw ClickHouse output into lines across chunk boundaries.
#[derive(Default)]
struct LineBuffer {
    buf: Vec<u8>,
}

impl LineBuffer {
    fn push(&mut self, chunk: &[u8]) -> Vec<String> {
        self.buf.extend_from_slice(chunk);
        self.drain_lines(false)
    }

    fn flush(&mut self) -> Vec<String> {
        self.drain_lines(true)
    }

    fn drain_lines(&mut self, flush: bool) -> Vec<String> {
        let mut lines = Vec::new();
        let mut start = 0usize;
        while let Some(pos) = self.buf[start..].iter().position(|b| *b == b'\n') {
            let end = start + pos;
            push_line(&mut lines, &self.buf[start..end]);
            start = end + 1;
        }
        self.buf.drain(..start);
        if flush && !self.buf.is_empty() {
            let tail = std::mem::take(&mut self.buf);
            push_line(&mut lines, &tail);
        }
        lines
    }
}

fn push_line(lines: &mut Vec<String>, raw: &[u8]) {
    let text = String::from_utf8_lossy(raw);
    let trimmed = text.trim_end_matches('\r');
    if !trimmed.trim().is_empty() {
        lines.push(trimmed.to_string());
    }
}

async fn send_event(sink: &mpsc::Sender<QueryEvent>, event: QueryEvent) -> MieoResult<()> {
    sink.send(event).await.map_err(|_| MieoError::cancelled())
}

async fn next_chunk(
    cursor: &mut clickhouse::query::BytesCursor,
    cancel: &CancellationToken,
    deadline: Option<tokio::time::Instant>,
) -> MieoResult<Option<Bytes>> {
    match deadline {
        Some(dl) => tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(MieoError::cancelled()),
            result = tokio::time::timeout_at(dl, cursor.next()) => match result {
                Err(_) => Err(MieoError::new(ErrorCode::Timeout, "query timed out")),
                Ok(Err(e)) => Err(map_ch_err(e)),
                Ok(Ok(chunk)) => Ok(chunk),
            },
        },
        None => tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(MieoError::cancelled()),
            result = cursor.next() => result.map_err(map_ch_err),
        },
    }
}

async fn run_with_cancel_and_timeout<F, T>(
    future: F,
    cancel: &CancellationToken,
    deadline: Option<tokio::time::Instant>,
) -> MieoResult<T>
where
    F: std::future::Future<Output = Result<T, clickhouse::error::Error>>,
{
    match deadline {
        Some(dl) => tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(MieoError::cancelled()),
            result = tokio::time::timeout_at(dl, future) => match result {
                Err(_) => Err(MieoError::new(ErrorCode::Timeout, "query timed out")),
                Ok(Err(e)) => Err(map_ch_err(e)),
                Ok(Ok(value)) => Ok(value),
            },
        },
        None => tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(MieoError::cancelled()),
            result = future => result.map_err(map_ch_err),
        },
    }
}

// ---------------------------------------------------------------------------
// Statement classification
// ---------------------------------------------------------------------------

fn statement_returns_rows(sql: &str) -> bool {
    let first = sql
        .trim_start_matches(|c: char| c.is_whitespace() || c == '(' || c == ';')
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    matches!(
        first.as_str(),
        "select" | "with" | "show" | "describe" | "desc" | "explain" | "exists"
    )
}

fn has_explicit_format(sql: &str) -> bool {
    sql.to_ascii_lowercase()
        .split_whitespace()
        .any(|token| token == "format")
}

fn max_execution_secs(ms: u64) -> String {
    ((ms + 999) / 1000).max(1).to_string()
}

/// Well-known ClickHouse TLS ports (https interface / native TLS / 443).
fn is_tls_port(port: u16) -> bool {
    matches!(port, 443 | 8443 | 9440)
}

// ---------------------------------------------------------------------------
// Type / value mapping
// ---------------------------------------------------------------------------

/// Strips `Nullable(...)` / `LowCardinality(...)` wrappers.
fn base_type(ty: &str) -> &str {
    let mut current = ty.trim();
    loop {
        if let Some(inner) = current
            .strip_prefix("Nullable(")
            .or_else(|| current.strip_prefix("LowCardinality("))
            .and_then(|rest| rest.strip_suffix(')'))
        {
            current = inner.trim();
        } else {
            return current;
        }
    }
}

fn kind_of(native: &str) -> DataTypeKind {
    let base = base_type(native);
    if base.starts_with("Array(") {
        let inner = base
            .strip_prefix("Array(")
            .and_then(|rest| rest.strip_suffix(')'))
            .unwrap_or("String");
        return DataTypeKind::Array(Box::new(kind_of(inner)));
    }
    if base.starts_with("Decimal") {
        return DataTypeKind::Decimal;
    }
    let head = base
        .split(['(', ' '])
        .next()
        .unwrap_or(base)
        .to_ascii_lowercase();
    match head.as_str() {
        "bool" | "boolean" => DataTypeKind::Bool,
        "int8" | "int16" | "int32" | "int64" | "int128" | "int256" => DataTypeKind::Int,
        "uint8" | "uint16" | "uint32" | "uint64" | "uint128" | "uint256" => DataTypeKind::UInt,
        "float32" | "float64" | "bfloat16" => DataTypeKind::Float,
        "string" | "fixedstring" | "ipv4" | "ipv6" | "interval" => DataTypeKind::Text,
        "date" | "date32" => DataTypeKind::Date,
        "datetime" | "datetime32" => DataTypeKind::DateTime,
        "datetime64" => DataTypeKind::TimestampTz,
        "uuid" => DataTypeKind::Uuid,
        "enum8" | "enum16" => DataTypeKind::Enum,
        "map" | "tuple" | "nested" | "json" | "object" | "variant" | "dynamic" => DataTypeKind::Json,
        "aggregatefunction" | "simpleaggregatefunction" | "nothing" => DataTypeKind::Unknown,
        _ => DataTypeKind::Unknown,
    }
}

fn value_from_json(native: Option<&str>, value: &Json) -> Value {
    if value.is_null() {
        return Value::Null;
    }
    let native = native.unwrap_or("");
    let base = base_type(native);

    if base.starts_with("Array(") {
        if let Some(items) = value.as_array() {
            let inner = base
                .strip_prefix("Array(")
                .and_then(|rest| rest.strip_suffix(')'));
            return Value::Array(
                items
                    .iter()
                    .map(|item| value_from_json(inner, item))
                    .collect(),
            );
        }
        return Value::Json(value.clone());
    }

    let head = base
        .split(['(', ' '])
        .next()
        .unwrap_or(base)
        .to_ascii_lowercase();
    match head.as_str() {
        "bool" | "boolean" => Value::Bool(json_bool(Some(value))),
        "int8" | "int16" | "int32" | "int64" => {
            if let Some(n) = value.as_i64() {
                Value::Int(n)
            } else if let Some(s) = value.as_str() {
                match s.parse::<i64>() {
                    Ok(n) => Value::Int(n),
                    Err(_) => Value::Decimal(s.to_string()),
                }
            } else {
                Value::Decimal(json_to_string(value))
            }
        }
        "int128" | "int256" => Value::Decimal(json_to_string(value)),
        "uint8" | "uint16" | "uint32" | "uint64" => {
            if let Some(n) = value.as_u64() {
                Value::UInt(n)
            } else if let Some(s) = value.as_str() {
                match s.parse::<u64>() {
                    Ok(n) => Value::UInt(n),
                    Err(_) => Value::Decimal(s.to_string()),
                }
            } else {
                Value::Decimal(json_to_string(value))
            }
        }
        "uint128" | "uint256" => Value::Decimal(json_to_string(value)),
        "float32" | "float64" | "bfloat16" => match value.as_f64() {
            Some(f) => Value::Float(f),
            None => {
                let text = json_to_string(value).to_ascii_lowercase();
                match text.as_str() {
                    "nan" => Value::Float(f64::NAN),
                    "inf" | "+inf" | "infinity" => Value::Float(f64::INFINITY),
                    "-inf" | "-infinity" => Value::Float(f64::NEG_INFINITY),
                    _ => Value::Text(json_to_string(value)),
                }
            }
        },
        "decimal" | "decimal32" | "decimal64" | "decimal128" | "decimal256" => {
            Value::Decimal(json_to_string(value))
        }
        "date" | "date32" => Value::Date(json_to_string(value)),
        "datetime" | "datetime32" | "datetime64" => Value::DateTime(json_to_string(value)),
        "uuid" => Value::Uuid(json_to_string(value)),
        "enum8" | "enum16" => Value::Text(json_to_string(value)),
        "string" | "fixedstring" | "ipv4" | "ipv6" | "interval" => match value {
            Json::String(s) => Value::Text(s.clone()),
            other => Value::Text(json_to_string(other)),
        },
        _ => match value {
            Json::Object(_) | Json::Array(_) => Value::Json(value.clone()),
            other => Value::Text(json_to_string(other)),
        },
    }
}

fn json_to_string(value: &Json) -> String {
    match value {
        Json::String(s) => s.clone(),
        Json::Null => String::new(),
        Json::Bool(b) => b.to_string(),
        Json::Number(n) => n.to_string(),
        other => other.to_string(),
    }
}

fn json_string(value: Option<&Json>) -> Option<String> {
    value.and_then(|v| match v {
        Json::String(s) if !s.is_empty() => Some(s.clone()),
        Json::Null => None,
        other => Some(json_to_string(other)),
    })
}

fn json_u64(value: Option<&Json>) -> Option<u64> {
    let v = value?;
    v.as_u64()
        .or_else(|| v.as_str().and_then(|s| s.parse::<u64>().ok()))
}

fn json_bool(value: Option<&Json>) -> bool {
    match value {
        Some(Json::Bool(b)) => *b,
        Some(Json::Number(n)) => n.as_u64().unwrap_or(0) != 0,
        Some(Json::String(s)) => matches!(s.as_str(), "1" | "true" | "True"),
        _ => false,
    }
}

/// `Decimal(p, s)` → `(Some(p), Some(s))`.
fn decimal_shape(native: &str) -> (Option<u32>, Option<u32>) {
    let base = base_type(native);
    let Some(rest) = base.strip_prefix("Decimal(") else {
        return (None, None);
    };
    let Some(inner) = rest.strip_suffix(')') else {
        return (None, None);
    };
    let mut parts = inner.split(',').map(|p| p.trim().parse::<u32>());
    match (parts.next(), parts.next()) {
        (Some(Ok(p)), Some(Ok(s))) => (Some(p), Some(s)),
        _ => (None, None),
    }
}

/// `FixedString(N)` → `Some(N)`.
fn fixed_string_len(native: &str) -> Option<u64> {
    let base = base_type(native);
    base.strip_prefix("FixedString(")
        .and_then(|rest| rest.strip_suffix(')'))
        .and_then(|inner| inner.trim().parse::<u64>().ok())
}

/// Best-effort conversion of a bound `Value` into a JSON scalar/array. Unlike
/// the IPC form, numbers stay numbers so the server-side `?` binding infers
/// the right ClickHouse type.
fn value_to_json(value: &Value) -> Json {
    match value {
        Value::Null => Json::Null,
        Value::Bool(b) => Json::Bool(*b),
        Value::Int(i) => Json::Number((*i).into()),
        Value::UInt(u) => Json::Number((*u).into()),
        Value::Float(f) => serde_json::Number::from_f64(*f)
            .map(Json::Number)
            .unwrap_or_else(|| Json::String(f.to_string())),
        Value::Decimal(s) => s
            .parse::<f64>()
            .ok()
            .and_then(serde_json::Number::from_f64)
            .map(Json::Number)
            .unwrap_or_else(|| Json::String(s.clone())),
        Value::Text(s) | Value::Date(s) | Value::Time(s) | Value::DateTime(s) | Value::Uuid(s) => {
            Json::String(s.clone())
        }
        Value::Bytes(s) => Json::String(s.clone()),
        Value::Json(j) => j.clone(),
        Value::Array(items) => Json::Array(items.iter().map(value_to_json).collect()),
        Value::Unsupported { .. } => Json::Null,
    }
}

/// Keep the identifier helper referenced for future DDL generation helpers.
#[allow(dead_code)]
pub(crate) fn quote_clickhouse_ident(name: &str) -> String {
    quote_ansi_ident(name)
}

// ---------------------------------------------------------------------------
// Error mapping
// ---------------------------------------------------------------------------

fn map_ch_err(e: clickhouse::error::Error) -> MieoError {
    use clickhouse::error::Error as ChError;

    let (code, message) = match &e {
        ChError::Network(_) => (ErrorCode::Network, format!("ClickHouse network error: {e}")),
        ChError::TimedOut => (ErrorCode::Timeout, "ClickHouse request timed out".to_string()),
        ChError::InvalidUtf8Encoding(_) | ChError::NotEnoughData | ChError::DataFormat(_) => (
            ErrorCode::Data,
            format!("ClickHouse decode error: {e}"),
        ),
        ChError::BadResponse(message) => classify_ch_message(message),
        ChError::Unsupported(message) => (ErrorCode::Unsupported, message.clone()),
        _ => (ErrorCode::Internal, format!("ClickHouse error: {e}")),
    };

    MieoError::new(code, message).with_source(e)
}

/// Classify a ClickHouse server error from its `Code: NNN` prefix.
fn classify_ch_message(message: &str) -> (ErrorCode, String) {
    let code = message
        .split_whitespace()
        .skip_while(|t| *t != "Code:")
        .nth(1)
        .and_then(|n| n.trim_end_matches(',').parse::<u32>().ok());
    let lower = message.to_ascii_lowercase();
    let kind = match code {
        Some(516) | Some(192) => ErrorCode::Auth,
        Some(60) | Some(81) | Some(49) | Some(47) => ErrorCode::NotFound,
        Some(62) | Some(115) | Some(44) | Some(43) => ErrorCode::Syntax,
        Some(241) | Some(173) => ErrorCode::Connection,
        Some(159) | Some(160) | Some(209) => ErrorCode::Timeout,
        Some(164) | Some(158) | Some(212) => ErrorCode::Constraint,
        Some(394) | Some(252) => ErrorCode::Unsupported,
        _ => {
            if lower.contains("authentication failed") {
                ErrorCode::Auth
            } else if lower.contains("syntax error") {
                ErrorCode::Syntax
            } else if lower.contains("unknown table") || lower.contains("doesn't exist") {
                ErrorCode::NotFound
            } else {
                ErrorCode::Internal
            }
        }
    };
    (kind, message.to_string())
}
