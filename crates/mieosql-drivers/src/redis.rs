//! Redis driver (`redis` crate, tokio + rustls).
//!
//! Query language (v1): Redis command syntax, e.g.
//! - `GET user:1`
//! - `HGETALL user:1`
//! - `SCAN 0 MATCH user:* COUNT 200`
//! - `PREVIEW <key>` (pseudo-command: TYPE + value, used by the data grid)
//!
//! Reply → result-set heuristic (unit-tested):
//! - `Nil` → one row with a `NULL` value.
//! - Integer / simple string / bulk string / double / boolean → one row,
//!   column `value`.
//! - Arrays of scalars → one row per element, column `value`.
//! - Field/value pairs (`HGETALL`, `ZRANGE ... WITHSCORES`, `CONFIG GET`) →
//!   two columns `field, value`.
//! - `SCAN`-shaped `[cursor, [..]]` replies → one row per key, column `value`.
//! - Nested arrays (e.g. `XRANGE`) → flattened into `c1..cN`.
//! - Maps (RESP3) → two columns `field, value`.
//!
//! Write commands additionally report `rows_affected` (counted from the reply
//! when the command returns a removed/added/moved-element count, e.g. `DEL`
//! or `HSET`, and 1 otherwise).
//!
//! Binary safety: command arguments are tokenized into raw bytes; replies that
//! are not valid UTF-8 are returned as base64 `Bytes` values.
//!
//! Transactions map onto `MULTI` / `EXEC` / `DISCARD`. While a transaction is
//! open each statement is acknowledged by the server with `QUEUED`; the
//! per-command results produced by `EXEC` are streamed as a single reply only
//! if the user types `EXEC` manually, while `commit()` discards the payload
//! (the UI executes statements one at a time and has no place to route
//! individual results).
//!
//! TLS: `ssl_mode != Disable` switches the URL scheme to `rediss://`. With the
//! compiled feature set certificates are always verified against webpki roots,
//! so `Require` behaves like `VerifyCa`/`VerifyFull` (the `tls-rustls-insecure`
//! feature is not enabled).

use std::time::{Duration, Instant};

use redis::aio::MultiplexedConnection;
use redis::{
    AsyncConnectionConfig, Client, ErrorKind as RedisErrorKind, ServerErrorKind,
    Value as RedisValue,
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use mieosql_core::{
    Capabilities, ColumnDef, ColumnMeta, ConnectionSpec, DataTypeKind, DatabaseRef, Engine,
    ErrorCode, ForeignKeyDef, IndexDef, MieoError, MieoResult, ObjectKind, ObjectRef,
    PlaceholderStyle, QueryEvent, QueryOptions, QueryStats, SchemaRef, Scope, ServerInfo, SslMode,
    Value,
};

/// Maximum number of keys `list_objects` returns from `SCAN`.
const SCAN_KEY_CAP: usize = 1000;
/// Maximum number of hash fields `table_columns` reports.
const HASH_FIELD_CAP: usize = 100;
/// Inclusive end index of `LRANGE` / `ZRANGE` previews (100 items).
const PREVIEW_END: i64 = 99;
/// `COUNT` used by `XRANGE` previews.
const PREVIEW_STREAM_COUNT: i64 = 100;
/// Native type name shown for query-result columns.
const NATIVE_TYPE: &str = "redis";

pub struct RedisDriver {
    spec: ConnectionSpec,
    conn: MultiplexedConnection,
    in_transaction: bool,
    /// Currently selected logical database (`SELECT n`).
    db: i64,
}

pub(crate) fn redis_capabilities() -> Capabilities {
    Capabilities {
        engine_kind: mieosql_core::EngineKind::KeyValue,
        placeholder: PlaceholderStyle::Question,
        list_databases: true,
        list_schemas: false,
        schema_is_database: false,
        // MULTI / EXEC / DISCARD map onto begin / commit / rollback.
        transactions: true,
        simple_mode: true,
        ddl: false,
        multiple_databases: true,
    }
}

impl RedisDriver {
    pub async fn connect(spec: &ConnectionSpec) -> MieoResult<Self> {
        let database = spec
            .database
            .as_deref()
            .map(str::trim)
            .filter(|d| !d.is_empty());
        let db = match database {
            Some(raw) => raw.parse::<i64>().map_err(|_| {
                MieoError::new(
                    ErrorCode::Data,
                    format!("Redis database must be a number, got `{raw}`"),
                )
                .with_hint("use the numeric Redis DB index (usually 0-15)")
            })?,
            None => 0,
        };

        let url = build_url(spec);
        let client = Client::open(url).map_err(map_redis_err)?;

        let config = AsyncConnectionConfig::new()
            .set_connection_timeout(Some(Duration::from_secs(spec.connect_timeout_secs.max(1))))
            // No connection-level response timeout: per-query deadlines are
            // enforced by `exec_query` (the crate default is only 500 ms).
            .set_response_timeout(None);
        let conn = client
            .get_multiplexed_async_connection_with_config(&config)
            .await
            .map_err(map_redis_err)?;

        Ok(Self {
            spec: spec.clone(),
            conn,
            in_transaction: false,
            db,
        })
    }

    pub fn spec(&self) -> &ConnectionSpec {
        &self.spec
    }

    pub fn in_transaction(&self) -> bool {
        self.in_transaction
    }

    pub async fn ping(&mut self) -> MieoResult<()> {
        self.exec_raw(&redis::cmd("PING")).await.map(|_| ())
    }

    pub async fn server_info(&mut self) -> MieoResult<ServerInfo> {
        let reply = self.exec_raw(&redis::cmd("INFO")).await?;
        let text = reply_as_text(&reply).unwrap_or_default();
        let info = redis::InfoDict::new(&text);

        let version: Option<String> = info.get("redis_version");
        let mode: Option<String> = info.get("redis_mode");
        let clients: Option<String> = info.get("connected_clients");
        let mut comment_parts = Vec::new();
        if let Some(mode) = &mode {
            comment_parts.push(format!("mode={mode}"));
        }
        if let Some(clients) = &clients {
            comment_parts.push(format!("connected_clients={clients}"));
        }

        Ok(ServerInfo {
            engine: Engine::Redis,
            version: version.unwrap_or_else(|| "unknown".to_string()),
            version_comment: (!comment_parts.is_empty()).then(|| comment_parts.join(", ")),
            current_database: Some(self.db.to_string()),
            current_user: self.spec.username.clone(),
        })
    }

    pub async fn list_databases(&mut self) -> MieoResult<Vec<DatabaseRef>> {
        self.ensure_idle()?;
        // `CONFIG GET databases` is disabled on some managed servers; fall back
        // to the classic Redis default of 16 logical databases.
        let count = match self
            .exec_raw(&redis::cmd("CONFIG").arg("GET").arg("databases"))
            .await
        {
            Ok(reply) => find_pair_field(&reply, "databases")
                .and_then(|v| v.parse::<i64>().ok())
                .filter(|n| *n > 0)
                .unwrap_or(16),
            Err(err) => {
                tracing::debug!("Redis CONFIG GET databases failed, assuming 16: {err}");
                16
            }
        };

        Ok((0..count)
            .map(|i| DatabaseRef {
                name: i.to_string(),
                comment: None,
                size_bytes: None,
                charset: None,
                collation: None,
                is_system: i == 0,
            })
            .collect())
    }

    pub async fn list_schemas(&mut self, database: Option<&str>) -> MieoResult<Vec<SchemaRef>> {
        let _ = database;
        Ok(Vec::new())
    }

    pub async fn list_objects(
        &mut self,
        scope: &Scope,
        kind: Option<ObjectKind>,
        pattern: Option<&str>,
    ) -> MieoResult<Vec<ObjectRef>> {
        self.ensure_idle()?;
        // Redis only has one object kind: keys.
        if kind.is_some_and(|k| k != ObjectKind::Table) {
            return Ok(Vec::new());
        }
        let db = self.scope_db(scope).await?;
        let pattern = pattern.map(str::trim).filter(|p| !p.is_empty()).unwrap_or("*");

        let mut cursor: u64 = 0;
        let mut keys: Vec<String> = Vec::new();
        loop {
            let mut cmd = redis::cmd("SCAN");
            cmd.arg(cursor)
                .arg("MATCH")
                .arg(pattern)
                .arg("COUNT")
                .arg(1000i64);
            let reply = self.exec_raw(&cmd).await?;
            let (next, batch) = parse_scan_reply(&reply)?;
            keys.extend(batch);
            if next == 0 || keys.len() >= SCAN_KEY_CAP {
                break;
            }
            cursor = next;
        }
        keys.truncate(SCAN_KEY_CAP);

        Ok(keys
            .into_iter()
            .map(|name| ObjectRef {
                name,
                kind: ObjectKind::Table,
                database: Some(db.to_string()),
                schema: None,
                comment: None,
                rows_estimate: None,
                size_bytes: None,
                updated_at: None,
            })
            .collect())
    }

    pub async fn table_columns(
        &mut self,
        scope: &Scope,
        table: &str,
    ) -> MieoResult<Vec<ColumnDef>> {
        self.ensure_idle()?;
        self.scope_db(scope).await?;

        let type_reply = self.exec_raw(&redis::cmd("TYPE").arg(table)).await?;
        if !matches!(reply_as_text(&type_reply).as_deref(), Some("hash")) {
            // Only hashes expose named fields; other types have no schema.
            return Ok(Vec::new());
        }

        let reply = self.exec_raw(&redis::cmd("HKEYS").arg(table)).await?;
        let fields: Vec<String> = reply
            .as_sequence()
            .unwrap_or(&[])
            .iter()
            .filter_map(reply_as_text)
            .take(HASH_FIELD_CAP)
            .collect();

        Ok(fields
            .into_iter()
            .enumerate()
            .map(|(i, name)| ColumnDef {
                name,
                ordinal: i as u32,
                native_type: "string".to_string(),
                data_type: DataTypeKind::Text,
                nullable: true,
                default_value: None,
                is_primary_key: false,
                is_auto_increment: false,
                is_generated: false,
                comment: Some("Redis hash field".to_string()),
                char_max_length: None,
                numeric_precision: None,
                numeric_scale: None,
                collation: None,
            })
            .collect())
    }

    pub async fn table_indexes(&mut self, scope: &Scope, table: &str) -> MieoResult<Vec<IndexDef>> {
        let _ = (scope, table);
        Ok(Vec::new())
    }

    pub async fn foreign_keys(
        &mut self,
        scope: &Scope,
        table: &str,
    ) -> MieoResult<Vec<ForeignKeyDef>> {
        let _ = (scope, table);
        Ok(Vec::new())
    }

    pub async fn object_ddl(
        &mut self,
        scope: &Scope,
        kind: ObjectKind,
        name: &str,
    ) -> MieoResult<Option<String>> {
        let _ = (scope, kind, name);
        Ok(None)
    }

    pub async fn use_database(&mut self, database: &str) -> MieoResult<()> {
        self.ensure_idle()?;
        self.select_db(database).await.map(|_| ())
    }

    pub async fn begin(&mut self) -> MieoResult<()> {
        if self.in_transaction {
            return Err(transaction_in_progress());
        }
        self.exec_raw(&redis::cmd("MULTI")).await?;
        self.in_transaction = true;
        Ok(())
    }

    pub async fn commit(&mut self) -> MieoResult<()> {
        if !self.in_transaction {
            return Err(no_transaction());
        }
        // EXEC returns one reply per queued command. The UI runs statements one
        // at a time and already received each `QUEUED` acknowledgement, so the
        // aggregate payload is intentionally discarded here.
        let result = self.exec_raw(&redis::cmd("EXEC")).await.map(|_| ());
        self.in_transaction = false;
        result
    }

    pub async fn rollback(&mut self) -> MieoResult<()> {
        if !self.in_transaction {
            return Err(no_transaction());
        }
        let result = self.exec_raw(&redis::cmd("DISCARD")).await.map(|_| ());
        self.in_transaction = false;
        result
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
        let start = Instant::now();

        let mut tokens = tokenize_command(&sql)?;
        if tokens.is_empty() {
            return Err(MieoError::new(ErrorCode::Syntax, "empty Redis command")
                .with_hint("for example: GET key, HGETALL key, SCAN 0 MATCH *"));
        }
        let name = String::from_utf8_lossy(&tokens.remove(0)).into_owned();
        let upper = name.to_ascii_uppercase();
        let args = tokens;

        // Pseudo-command used by the data grid.
        if upper == "PREVIEW" {
            if self.in_transaction {
                return Err(MieoError::new(
                    ErrorCode::Conflict,
                    "PREVIEW is not available inside a Redis transaction",
                ));
            }
            let Some(key) = args.first().map(|k| String::from_utf8_lossy(k).into_owned()) else {
                return Err(
                    MieoError::new(ErrorCode::Syntax, "PREVIEW requires a key")
                        .with_hint("PREVIEW <key>"),
                );
            };
            let result = self.preview_key(&key, options, cancel).await?;
            return emit_result(query_id, result, options, sink, start, 0).await;
        }

        // Bind `?` placeholders in order; extra parameters are appended.
        let mut bound: Vec<Vec<u8>> = Vec::with_capacity(args.len() + params.len());
        let mut param_iter = params.iter();
        for arg in &args {
            if arg.as_slice() == b"?".as_slice() {
                match param_iter.next() {
                    Some(value) => bound.push(value_to_arg(value)?),
                    None => {
                        return Err(MieoError::new(
                            ErrorCode::Data,
                            "not enough parameters for the `?` placeholders",
                        ));
                    }
                }
            } else {
                bound.push(arg.clone());
            }
        }
        for value in param_iter {
            bound.push(value_to_arg(value)?);
        }

        let mut cmd = redis::cmd(&name);
        for arg in &bound {
            cmd.arg(arg);
        }

        let reply = self.exec_query(&cmd, options, cancel).await?;

        // Track transaction state even when the user types the commands by hand.
        match upper.as_str() {
            "MULTI" => self.in_transaction = true,
            "EXEC" | "DISCARD" => self.in_transaction = false,
            _ => {}
        }

        let rows_affected = rows_affected_for(&upper, &reply);
        let args_text: Vec<String> = args
            .iter()
            .map(|a| String::from_utf8_lossy(a).into_owned())
            .collect();
        let result = reply_to_result(&upper, &args_text, &reply);
        emit_result(query_id, result, options, sink, start, rows_affected).await
    }

    /// `PREVIEW <key>` pseudo-command (TYPE + value fetch).
    pub fn preview_statement(&self, scope: &Scope, table: &str, limit: u64) -> MieoResult<String> {
        let _ = (scope, limit);
        Ok(format!("PREVIEW {table}"))
    }

    pub fn engine(&self) -> Engine {
        Engine::Redis
    }

    // ------------------------------------------------------------------
    // Internals
    // ------------------------------------------------------------------

    /// Run a command without a per-query deadline (introspection, transactions).
    async fn exec_raw(&mut self, cmd: &redis::Cmd) -> MieoResult<RedisValue> {
        cmd.query_async::<RedisValue>(&mut self.conn)
            .await
            .map_err(map_redis_err)
    }

    /// Run a user command, honoring the cancellation token and `timeout_ms`.
    async fn exec_query(
        &mut self,
        cmd: &redis::Cmd,
        options: &QueryOptions,
        cancel: &CancellationToken,
    ) -> MieoResult<RedisValue> {
        let deadline = options
            .timeout_ms
            .map(|ms| tokio::time::Instant::now() + Duration::from_millis(ms));
        tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(MieoError::cancelled()),
            _ = wait_deadline(deadline) => {
                Err(MieoError::new(ErrorCode::Timeout, "Redis command timed out"))
            }
            reply = cmd.query_async::<RedisValue>(&mut self.conn) => reply.map_err(map_redis_err),
        }
    }

    /// `SELECT n` and remember the index.
    async fn select_db(&mut self, database: &str) -> MieoResult<i64> {
        let trimmed = database.trim();
        let n = trimmed.parse::<i64>().map_err(|_| {
            MieoError::new(
                ErrorCode::Data,
                format!("Redis database must be a number, got `{trimmed}`"),
            )
            .with_hint("use the numeric Redis DB index (usually 0-15)")
        })?;
        self.exec_raw(&redis::cmd("SELECT").arg(n)).await?;
        self.db = n;
        Ok(n)
    }

    /// Resolve the logical database for a scope, switching when needed.
    async fn scope_db(&mut self, scope: &Scope) -> MieoResult<i64> {
        match scope
            .database
            .as_deref()
            .map(str::trim)
            .filter(|d| !d.is_empty())
        {
            Some(db) => self.select_db(db).await,
            None => Ok(self.db),
        }
    }

    fn ensure_idle(&self) -> MieoResult<()> {
        if self.in_transaction {
            return Err(MieoError::new(
                ErrorCode::Conflict,
                "not available inside a Redis transaction",
            ));
        }
        Ok(())
    }

    /// Fetch a key according to its `TYPE` and shape the rows for the grid.
    async fn preview_key(
        &mut self,
        key: &str,
        options: &QueryOptions,
        cancel: &CancellationToken,
    ) -> MieoResult<ResultSet> {
        let type_reply = self
            .exec_query(redis::cmd("TYPE").arg(key), options, cancel)
            .await?;
        let type_name = reply_as_text(&type_reply).unwrap_or_else(|| "none".to_string());

        match preview_plan(key, &type_name) {
            PreviewPlan::Missing => Ok(single_value(Value::Null, DataTypeKind::Unknown)),
            PreviewPlan::Unsupported(kind) => Err(MieoError::unsupported(format!(
                "PREVIEW does not support the Redis type `{kind}`"
            ))),
            PreviewPlan::Commands(commands) => {
                let mut result = None;
                for plan in &commands {
                    let mut cmd = redis::cmd(&plan.name);
                    for arg in &plan.args {
                        cmd.arg(arg.as_str());
                    }
                    let reply = self.exec_query(&cmd, options, cancel).await?;
                    result = Some(map_preview_reply(plan.shape, &reply));
                }
                result.ok_or_else(|| MieoError::internal("empty PREVIEW plan"))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Connection URL
// ---------------------------------------------------------------------------

/// `redis://[user[:pass]@]host:port[/db]`, `rediss://` when TLS is requested.
fn build_url(spec: &ConnectionSpec) -> String {
    if spec.ssl_mode == SslMode::Require {
        // The compiled rustls feature set always verifies certificates.
        tracing::debug!(
            "Redis ssl_mode=require: certificates are still verified \
             (tls-rustls-insecure feature not enabled)"
        );
    }
    let scheme = if spec.ssl_mode == SslMode::Disable {
        "redis"
    } else {
        "rediss"
    };

    let host = spec
        .host
        .as_deref()
        .map(str::trim)
        .filter(|h| !h.is_empty())
        .unwrap_or("127.0.0.1");
    let host = if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]")
    } else {
        host.to_string()
    };
    let port = spec.port.unwrap_or(6379);

    let mut url = format!("{scheme}://");
    let username = spec.username.as_deref().filter(|u| !u.is_empty());
    match (username, spec.password.as_deref()) {
        (Some(user), Some(pass)) => {
            url.push_str(&percent_encode_userinfo(user));
            url.push(':');
            url.push_str(&percent_encode_userinfo(pass));
            url.push('@');
        }
        (Some(user), None) => {
            url.push_str(&percent_encode_userinfo(user));
            url.push('@');
        }
        // Password-only form: `redis://:pass@host`.
        (None, Some(pass)) => {
            url.push(':');
            url.push_str(&percent_encode_userinfo(pass));
            url.push('@');
        }
        (None, None) => {}
    }
    url.push_str(&host);
    url.push(':');
    url.push_str(&port.to_string());
    if let Some(db) = spec
        .database
        .as_deref()
        .map(str::trim)
        .filter(|d| !d.is_empty())
    {
        url.push('/');
        url.push_str(db);
    }
    url
}

/// Percent-encode userinfo (RFC 3986 unreserved characters stay literal).
fn percent_encode_userinfo(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for byte in raw.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(byte as char);
        } else {
            out.push('%');
            const HEX: &[u8; 16] = b"0123456789ABCDEF";
            out.push(HEX[(byte >> 4) as usize] as char);
            out.push(HEX[(byte & 0x0f) as usize] as char);
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

fn map_redis_err(err: redis::RedisError) -> MieoError {
    let message = err.to_string();
    let db_code = err.code().map(str::to_string);

    let code = if err.is_timeout() {
        ErrorCode::Timeout
    } else if err.is_connection_refusal() || err.is_connection_dropped() {
        ErrorCode::Connection
    } else {
        match err.kind() {
            RedisErrorKind::AuthenticationFailed => ErrorCode::Auth,
            RedisErrorKind::Io => ErrorCode::Network,
            RedisErrorKind::Client
            | RedisErrorKind::InvalidClientConfig
            | RedisErrorKind::Parse => ErrorCode::Connection,
            RedisErrorKind::Server(kind) => match kind {
                ServerErrorKind::NoPerm | ServerErrorKind::ReadOnly => ErrorCode::Permission,
                ServerErrorKind::ExecAbort
                | ServerErrorKind::BusyLoading
                | ServerErrorKind::TryAgain
                | ServerErrorKind::ClusterDown
                | ServerErrorKind::MasterDown => ErrorCode::Conflict,
                ServerErrorKind::ResponseError => {
                    let lower = message.to_ascii_lowercase();
                    if lower.contains("unknown command")
                        || lower.contains("syntax error")
                        || lower.contains("wrong number of arguments")
                    {
                        ErrorCode::Syntax
                    } else {
                        ErrorCode::Data
                    }
                }
                _ => ErrorCode::Data,
            },
            _ => ErrorCode::Data,
        }
    };

    let mut mapped = MieoError::new(code, format!("Redis error: {message}"));
    if let Some(db_code) = db_code {
        mapped = mapped.with_db_code(db_code);
    }
    mapped.with_source(err)
}

fn transaction_in_progress() -> MieoError {
    MieoError::new(
        ErrorCode::Conflict,
        "a Redis transaction is already in progress",
    )
}

fn no_transaction() -> MieoError {
    MieoError::new(ErrorCode::Conflict, "no Redis transaction is in progress")
}

// ---------------------------------------------------------------------------
// Command tokenizer
// ---------------------------------------------------------------------------

/// Tokenize a Redis command line into raw argument bytes.
///
/// Whitespace separates arguments; single (`'`) and double (`"`) quotes group
/// them. Escapes: `\n`, `\r`, `\t`, `\b`, `\a`, `\xHH`, `\\`, `\"`, `\'` —
/// inside single quotes only `\'` and `\\` are special (redis-cli semantics).
fn tokenize_command(input: &str) -> MieoResult<Vec<Vec<u8>>> {
    let mut tokens: Vec<Vec<u8>> = Vec::new();
    let mut current: Vec<u8> = Vec::new();
    let mut in_token = false;
    let mut quote: Option<char> = None;
    let mut chars = input.chars().peekable();

    while let Some(c) = chars.next() {
        match quote {
            Some(q) => {
                if c == q {
                    quote = None;
                } else if c == '\\' {
                    let next = chars
                        .next()
                        .ok_or_else(|| syntax_error("trailing backslash"))?;
                    if q == '\'' {
                        match next {
                            '\'' => current.push(b'\''),
                            '\\' => current.push(b'\\'),
                            other => {
                                current.push(b'\\');
                                push_char(&mut current, other);
                            }
                        }
                    } else {
                        push_escaped(&mut current, next, &mut chars)?;
                    }
                } else {
                    push_char(&mut current, c);
                }
            }
            None => {
                if c.is_whitespace() {
                    if in_token {
                        tokens.push(std::mem::take(&mut current));
                        in_token = false;
                    }
                } else if c == '\'' || c == '"' {
                    quote = Some(c);
                    in_token = true;
                } else if c == '\\' {
                    let next = chars
                        .next()
                        .ok_or_else(|| syntax_error("trailing backslash"))?;
                    push_escaped(&mut current, next, &mut chars)?;
                    in_token = true;
                } else {
                    push_char(&mut current, c);
                    in_token = true;
                }
            }
        }
    }

    if let Some(q) = quote {
        return Err(syntax_error(&format!("unterminated {q} quote")));
    }
    if in_token {
        tokens.push(current);
    }
    Ok(tokens)
}

fn push_char(buf: &mut Vec<u8>, c: char) {
    let mut tmp = [0u8; 4];
    buf.extend_from_slice(c.encode_utf8(&mut tmp).as_bytes());
}

fn push_escaped(
    buf: &mut Vec<u8>,
    first: char,
    chars: &mut std::iter::Peekable<std::str::Chars<'_>>,
) -> MieoResult<()> {
    match first {
        'n' => buf.push(b'\n'),
        'r' => buf.push(b'\r'),
        't' => buf.push(b'\t'),
        'b' => buf.push(0x08),
        'a' => buf.push(0x07),
        'x' => {
            let high = chars
                .next()
                .and_then(|c| c.to_digit(16))
                .ok_or_else(|| syntax_error("invalid \\x escape"))?;
            let low = chars
                .next()
                .and_then(|c| c.to_digit(16))
                .ok_or_else(|| syntax_error("invalid \\x escape"))?;
            buf.push(((high << 4) | low) as u8);
        }
        other => push_char(buf, other),
    }
    Ok(())
}

fn syntax_error(message: &str) -> MieoError {
    MieoError::new(
        ErrorCode::Syntax,
        format!("Redis command parse error: {message}"),
    )
}

/// Convert a bound [`Value`] parameter into raw Redis argument bytes.
fn value_to_arg(value: &Value) -> MieoResult<Vec<u8>> {
    Ok(match value {
        Value::Null => {
            return Err(MieoError::new(
                ErrorCode::Data,
                "NULL is not a valid Redis argument",
            ));
        }
        Value::Bool(b) => {
            if *b {
                b"1".to_vec()
            } else {
                b"0".to_vec()
            }
        }
        Value::Int(i) => i.to_string().into_bytes(),
        Value::UInt(u) => u.to_string().into_bytes(),
        Value::Float(f) => f.to_string().into_bytes(),
        Value::Decimal(s)
        | Value::Text(s)
        | Value::Date(s)
        | Value::Time(s)
        | Value::DateTime(s)
        | Value::Uuid(s) => s.clone().into_bytes(),
        Value::Bytes(b64) => crate::util::b64_decode(b64)?,
        Value::Json(json) => json.to_string().into_bytes(),
        Value::Array(_) => {
            return Err(MieoError::new(
                ErrorCode::Data,
                "array parameters are not supported by the Redis driver",
            ));
        }
        Value::Unsupported { native_type, .. } => {
            return Err(MieoError::new(
                ErrorCode::Data,
                format!("cannot bind an unsupported value of type `{native_type}`"),
            ));
        }
    })
}

// ---------------------------------------------------------------------------
// Reply mapping
// ---------------------------------------------------------------------------

/// Materialized result set produced by one Redis reply.
#[derive(Debug)]
struct ResultSet {
    columns: Vec<ColumnMeta>,
    rows: Vec<Vec<Value>>,
}

fn column(name: impl Into<String>, ordinal: u32, kind: DataTypeKind) -> ColumnMeta {
    ColumnMeta {
        name: name.into(),
        ordinal,
        native_type: NATIVE_TYPE.to_string(),
        kind,
        nullable: Some(true),
    }
}

/// Textual view of a scalar reply, when it has one.
fn reply_as_text(reply: &RedisValue) -> Option<String> {
    match reply {
        RedisValue::SimpleString(s) => Some(s.clone()),
        RedisValue::Okay => Some("OK".to_string()),
        RedisValue::BulkString(bytes) => Some(String::from_utf8_lossy(bytes).into_owned()),
        RedisValue::VerbatimString { text, .. } => Some(text.clone()),
        _ => None,
    }
}

/// UTF-8 text when possible, base64 `Bytes` otherwise.
fn bytes_to_value(bytes: &[u8]) -> Value {
    match std::str::from_utf8(bytes) {
        Ok(text) => Value::Text(text.to_string()),
        Err(_) => Value::Bytes(crate::util::b64_encode(bytes)),
    }
}

fn is_scalar(reply: &RedisValue) -> bool {
    !matches!(
        reply,
        RedisValue::Array(_) | RedisValue::Set(_) | RedisValue::Map(_)
    )
}

fn scalar_kind(reply: &RedisValue) -> DataTypeKind {
    match reply {
        RedisValue::Int(_) => DataTypeKind::Int,
        RedisValue::Double(_) => DataTypeKind::Float,
        RedisValue::Boolean(_) => DataTypeKind::Bool,
        RedisValue::Nil => DataTypeKind::Unknown,
        _ => DataTypeKind::Text,
    }
}

fn redis_scalar_to_value(reply: &RedisValue) -> Value {
    match reply {
        RedisValue::Nil => Value::Null,
        RedisValue::Int(i) => Value::Int(*i),
        RedisValue::Double(f) => Value::Float(*f),
        RedisValue::Boolean(b) => Value::Bool(*b),
        RedisValue::BulkString(bytes) => bytes_to_value(bytes),
        RedisValue::SimpleString(s) => Value::Text(s.clone()),
        RedisValue::VerbatimString { text, .. } => Value::Text(text.clone()),
        RedisValue::Okay => Value::Text("OK".to_string()),
        RedisValue::BigNumber(bytes) => Value::Text(String::from_utf8_lossy(bytes).into_owned()),
        RedisValue::ServerError(err) => Value::Text(format!("ERR {err}")),
        other => Value::Unsupported {
            native_type: NATIVE_TYPE.to_string(),
            reason: format!("{other:?}"),
        },
    }
}

fn single_value(value: Value, kind: DataTypeKind) -> ResultSet {
    ResultSet {
        columns: vec![column("value", 0, kind)],
        rows: vec![vec![value]],
    }
}

fn uniform_kind(items: &[RedisValue]) -> DataTypeKind {
    let mut kind: Option<DataTypeKind> = None;
    for item in items {
        if !is_scalar(item) {
            return DataTypeKind::Unknown;
        }
        let item_kind = scalar_kind(item);
        if item_kind == DataTypeKind::Unknown {
            return DataTypeKind::Unknown;
        }
        match &kind {
            None => kind = Some(item_kind),
            Some(previous) if *previous == item_kind => {}
            _ => return DataTypeKind::Unknown,
        }
    }
    kind.unwrap_or(DataTypeKind::Unknown)
}

fn scalars_result(items: &[RedisValue]) -> ResultSet {
    ResultSet {
        columns: vec![column("value", 0, uniform_kind(items))],
        rows: items
            .iter()
            .map(|item| vec![redis_scalar_to_value(item)])
            .collect(),
    }
}

fn pairs_result(items: &[(RedisValue, RedisValue)]) -> ResultSet {
    ResultSet {
        columns: vec![
            column("field", 0, DataTypeKind::Text),
            column("value", 1, DataTypeKind::Text),
        ],
        rows: items
            .iter()
            .map(|(field, value)| vec![redis_scalar_to_value(field), redis_scalar_to_value(value)])
            .collect(),
    }
}

fn pairs_from_slice(items: &[RedisValue]) -> ResultSet {
    ResultSet {
        columns: vec![
            column("field", 0, DataTypeKind::Text),
            column("value", 1, DataTypeKind::Text),
        ],
        rows: items
            .chunks(2)
            .filter(|pair| pair.len() == 2)
            .map(|pair| {
                vec![
                    redis_scalar_to_value(&pair[0]),
                    redis_scalar_to_value(&pair[1]),
                ]
            })
            .collect(),
    }
}

fn empty_pairs() -> ResultSet {
    ResultSet {
        columns: vec![
            column("field", 0, DataTypeKind::Text),
            column("value", 1, DataTypeKind::Text),
        ],
        rows: Vec::new(),
    }
}

/// Recursively flatten a (possibly nested) reply into scalar values.
fn flatten_value(reply: &RedisValue, out: &mut Vec<Value>) {
    match reply {
        RedisValue::Array(items) | RedisValue::Set(items) => {
            for item in items {
                flatten_value(item, out);
            }
        }
        RedisValue::Map(items) => {
            for (key, value) in items {
                flatten_value(key, out);
                flatten_value(value, out);
            }
        }
        other => out.push(redis_scalar_to_value(other)),
    }
}

/// Nested arrays become rows with `c1..cN` columns (padding with NULL).
fn nested_result(items: &[RedisValue]) -> ResultSet {
    let mut flat_rows: Vec<Vec<Value>> = Vec::with_capacity(items.len());
    let mut width = 0usize;
    for item in items {
        let mut row = Vec::new();
        flatten_value(item, &mut row);
        width = width.max(row.len());
        flat_rows.push(row);
    }
    let width = width.max(1);
    let columns = (0..width)
        .map(|i| column(format!("c{}", i + 1), i as u32, DataTypeKind::Unknown))
        .collect();
    let rows = flat_rows
        .into_iter()
        .map(|mut row| {
            while row.len() < width {
                row.push(Value::Null);
            }
            row
        })
        .collect();
    ResultSet { columns, rows }
}

/// Commands whose reply is a flat field/value sequence.
fn expects_pairs(cmd: &str, args: &[String]) -> bool {
    match cmd {
        "HGETALL" => true,
        "HRANDFIELD" => args
            .iter()
            .any(|a| a.eq_ignore_ascii_case("WITHVALUES")),
        "ZRANGE" | "ZREVRANGE" | "ZRANGEBYSCORE" | "ZREVRANGEBYSCORE" | "ZRANGEBYLEX"
        | "ZREVRANGEBYLEX" | "ZDIFF" | "ZINTER" | "ZUNION" => args
            .iter()
            .any(|a| a.eq_ignore_ascii_case("WITHSCORES")),
        "CONFIG" => args.first().is_some_and(|a| a.eq_ignore_ascii_case("GET")),
        _ => false,
    }
}

fn array_result(cmd: &str, args: &[String], items: &[RedisValue]) -> ResultSet {
    if items.is_empty() {
        // An empty list/set still renders as an (empty) `value` column.
        return ResultSet {
            columns: vec![column("value", 0, DataTypeKind::Unknown)],
            rows: Vec::new(),
        };
    }
    if expects_pairs(cmd, args) && items.len() % 2 == 0 {
        return pairs_from_slice(items);
    }
    if items.iter().all(is_scalar) {
        return scalars_result(items);
    }
    // SCAN/HSCAN/SSCAN/ZSCAN shape: [cursor, [payload...]].
    if items.len() == 2
        && is_scalar(&items[0])
        && let RedisValue::Array(inner) = &items[1]
    {
        if cmd == "HSCAN" && inner.len() % 2 == 0 {
            return pairs_from_slice(inner);
        }
        return scalars_result(inner);
    }
    if items.iter().all(|item| {
        matches!(
            item,
            RedisValue::Array(_) | RedisValue::Set(_) | RedisValue::Map(_)
        )
    }) {
        return nested_result(items);
    }
    // Mixed fallback: one flattened row.
    let mut row = Vec::new();
    flatten_value(&RedisValue::Array(items.to_vec()), &mut row);
    let columns = (0..row.len().max(1))
        .map(|i| column(format!("c{}", i + 1), i as u32, DataTypeKind::Unknown))
        .collect();
    ResultSet {
        columns,
        rows: vec![row],
    }
}

/// Reply → rows heuristic (see the module docs for the full table).
fn reply_to_result(cmd: &str, args: &[String], reply: &RedisValue) -> ResultSet {
    match reply {
        RedisValue::Nil => single_value(Value::Null, DataTypeKind::Unknown),
        RedisValue::Int(i) => single_value(Value::Int(*i), DataTypeKind::Int),
        RedisValue::Double(f) => single_value(Value::Float(*f), DataTypeKind::Float),
        RedisValue::Boolean(b) => single_value(Value::Bool(*b), DataTypeKind::Bool),
        RedisValue::BulkString(bytes) => single_value(bytes_to_value(bytes), DataTypeKind::Text),
        RedisValue::SimpleString(s) => single_value(Value::Text(s.clone()), DataTypeKind::Text),
        RedisValue::Okay => single_value(Value::Text("OK".to_string()), DataTypeKind::Text),
        RedisValue::VerbatimString { text, .. } => {
            single_value(Value::Text(text.clone()), DataTypeKind::Text)
        }
        RedisValue::BigNumber(bytes) => single_value(
            Value::Text(String::from_utf8_lossy(bytes).into_owned()),
            DataTypeKind::Text,
        ),
        RedisValue::Set(items) => scalars_result(items),
        RedisValue::Array(items) => array_result(cmd, args, items),
        RedisValue::Map(items) => pairs_result(items),
        RedisValue::Push { data, .. } => {
            if data.len() == 1 {
                reply_to_result(cmd, args, &data[0])
            } else {
                array_result(cmd, args, data)
            }
        }
        RedisValue::Attribute { data, .. } => reply_to_result(cmd, args, data),
        RedisValue::ServerError(err) => {
            single_value(Value::Text(format!("ERR {err}")), DataTypeKind::Text)
        }
        other => single_value(
            Value::Unsupported {
                native_type: NATIVE_TYPE.to_string(),
                reason: format!("{other:?}"),
            },
            DataTypeKind::Unknown,
        ),
    }
}

/// `SCAN`-family reply parsing: `[cursor, [key, ...]]`.
fn parse_scan_reply(reply: &RedisValue) -> MieoResult<(u64, Vec<String>)> {
    if let RedisValue::Array(items) = reply
        && items.len() == 2
    {
        let cursor = match &items[0] {
            RedisValue::Int(i) => u64::try_from(*i).ok(),
            other => reply_as_text(other).and_then(|s| s.parse::<u64>().ok()),
        };
        if let (Some(cursor), Some(sequence)) = (cursor, items[1].as_sequence()) {
            let keys = sequence.iter().filter_map(reply_as_text).collect();
            return Ok((cursor, keys));
        }
    }
    Err(MieoError::new(ErrorCode::Data, "unexpected SCAN reply")
        .with_detail(format!("{reply:?}")))
}

// ---------------------------------------------------------------------------
// PREVIEW
// ---------------------------------------------------------------------------

/// Shape of the reply produced by a `PREVIEW` command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PreviewShape {
    /// A single scalar (`GET`, `SMEMBERS`).
    Scalar,
    /// A flat field/value sequence (`HGETALL`, `ZRANGE ... WITHSCORES`).
    Pairs,
    /// A list whose elements get an explicit index column (`LRANGE`).
    IndexValue,
    /// Entries whose nested fields are flattened (`XRANGE`).
    Nested,
}

#[derive(Debug, PartialEq, Eq)]
struct PreviewCommand {
    name: String,
    args: Vec<String>,
    shape: PreviewShape,
}

#[derive(Debug, PartialEq, Eq)]
enum PreviewPlan {
    /// The key does not exist.
    Missing,
    /// Type without preview support (module types, ...).
    Unsupported(String),
    /// Commands to run, in order.
    Commands(Vec<PreviewCommand>),
}

/// Command selection for `PREVIEW <key>` by `TYPE` result (pure, unit-tested).
fn preview_plan(key: &str, type_name: &str) -> PreviewPlan {
    let command = |name: &str, args: Vec<String>, shape| PreviewCommand {
        name: name.to_string(),
        args,
        shape,
    };
    match type_name {
        "none" => PreviewPlan::Missing,
        "string" => PreviewPlan::Commands(vec![command("GET", vec![key.to_string()], PreviewShape::Scalar)]),
        "hash" => PreviewPlan::Commands(vec![command(
            "HGETALL",
            vec![key.to_string()],
            PreviewShape::Pairs,
        )]),
        "list" => PreviewPlan::Commands(vec![command(
            "LRANGE",
            vec![key.to_string(), "0".to_string(), PREVIEW_END.to_string()],
            PreviewShape::IndexValue,
        )]),
        "set" => PreviewPlan::Commands(vec![command(
            "SMEMBERS",
            vec![key.to_string()],
            PreviewShape::Scalar,
        )]),
        "zset" => PreviewPlan::Commands(vec![command(
            "ZRANGE",
            vec![
                key.to_string(),
                "0".to_string(),
                PREVIEW_END.to_string(),
                "WITHSCORES".to_string(),
            ],
            PreviewShape::Pairs,
        )]),
        "stream" => PreviewPlan::Commands(vec![command(
            "XRANGE",
            vec![
                key.to_string(),
                "-".to_string(),
                "+".to_string(),
                "COUNT".to_string(),
                PREVIEW_STREAM_COUNT.to_string(),
            ],
            PreviewShape::Nested,
        )]),
        other => PreviewPlan::Unsupported(other.to_string()),
    }
}

fn map_preview_reply(shape: PreviewShape, reply: &RedisValue) -> ResultSet {
    match shape {
        PreviewShape::Scalar => reply_to_result("", &[], reply),
        PreviewShape::Pairs => match reply {
            RedisValue::Array(items) => pairs_from_slice(items),
            RedisValue::Map(items) => pairs_result(items),
            RedisValue::Nil => empty_pairs(),
            other => reply_to_result("", &[], other),
        },
        PreviewShape::IndexValue => index_value_result(reply),
        PreviewShape::Nested => match reply {
            RedisValue::Array(items) => stream_result(items),
            RedisValue::Nil => ResultSet {
                columns: vec![column("value", 0, DataTypeKind::Unknown)],
                rows: Vec::new(),
            },
            other => reply_to_result("", &[], other),
        },
    }
}

/// `LRANGE` preview: rows of `index, value`.
fn index_value_result(reply: &RedisValue) -> ResultSet {
    let items: &[RedisValue] = match reply {
        RedisValue::Array(items) | RedisValue::Set(items) => items,
        RedisValue::Nil => &[],
        other => std::slice::from_ref(other),
    };
    ResultSet {
        columns: vec![
            column("index", 0, DataTypeKind::Int),
            column("value", 1, DataTypeKind::Text),
        ],
        rows: items
            .iter()
            .enumerate()
            .map(|(i, item)| vec![Value::Int(i as i64), redis_scalar_to_value(item)])
            .collect(),
    }
}

/// `XRANGE` preview: entries `[id, [field, value, ...]]`.
fn stream_result(items: &[RedisValue]) -> ResultSet {
    let simple = items.iter().all(|entry| {
        matches!(
            entry,
            RedisValue::Array(parts)
                if parts.len() == 2
                    && matches!(&parts[1], RedisValue::Array(inner) if inner.len() == 2)
        )
    });
    if !simple {
        return nested_result(items);
    }
    ResultSet {
        columns: vec![
            column("id", 0, DataTypeKind::Text),
            column("field", 1, DataTypeKind::Text),
            column("value", 2, DataTypeKind::Text),
        ],
        rows: items
            .iter()
            .map(|entry| {
                if let RedisValue::Array(parts) = entry
                    && let RedisValue::Array(inner) = &parts[1]
                {
                    return vec![
                        redis_scalar_to_value(&parts[0]),
                        redis_scalar_to_value(&inner[0]),
                        redis_scalar_to_value(&inner[1]),
                    ];
                }
                vec![redis_scalar_to_value(entry)]
            })
            .collect(),
    }
}

// ---------------------------------------------------------------------------
// Write-command bookkeeping
// ---------------------------------------------------------------------------

/// Best-effort classification of Redis write commands (drives `rows_affected`).
fn is_write_command(cmd: &str) -> bool {
    matches!(
        cmd,
        "SET" | "SETNX"
            | "SETEX"
            | "PSETEX"
            | "MSET"
            | "MSETNX"
            | "APPEND"
            | "SETRANGE"
            | "GETSET"
            | "GETDEL"
            | "GETEX"
            | "INCR"
            | "DECR"
            | "INCRBY"
            | "DECRBY"
            | "INCRBYFLOAT"
            | "DEL"
            | "UNLINK"
            | "EXPIRE"
            | "PEXPIRE"
            | "EXPIREAT"
            | "PEXPIREAT"
            | "PERSIST"
            | "RENAME"
            | "RENAMENX"
            | "MOVE"
            | "COPY"
            | "RESTORE"
            | "LPUSH"
            | "RPUSH"
            | "LPUSHX"
            | "RPUSHX"
            | "LPOP"
            | "RPOP"
            | "LSET"
            | "LINSERT"
            | "LREM"
            | "LTRIM"
            | "LMOVE"
            | "RPOPLPUSH"
            | "HSET"
            | "HSETNX"
            | "HMSET"
            | "HDEL"
            | "HINCRBY"
            | "HINCRBYFLOAT"
            | "SADD"
            | "SREM"
            | "SPOP"
            | "SMOVE"
            | "SINTERSTORE"
            | "SUNIONSTORE"
            | "SDIFFSTORE"
            | "ZADD"
            | "ZREM"
            | "ZINCRBY"
            | "ZPOPMIN"
            | "ZPOPMAX"
            | "ZREMRANGEBYRANK"
            | "ZREMRANGEBYSCORE"
            | "ZREMRANGEBYLEX"
            | "ZDIFFSTORE"
            | "ZINTERSTORE"
            | "ZUNIONSTORE"
            | "GEOADD"
            | "PFADD"
            | "PFMERGE"
            | "XADD"
            | "XDEL"
            | "XTRIM"
            | "XACK"
            | "XCLAIM"
            | "XAUTOCLAIM"
            | "XGROUP"
            | "XSETID"
            | "FLUSHDB"
            | "FLUSHALL"
    )
}

/// Commands whose integer reply is a removed/added/moved-element count.
fn counts_affected(cmd: &str) -> bool {
    matches!(
        cmd,
        "DEL" | "UNLINK"
            | "HDEL"
            | "HSET"
            | "HSETNX"
            | "SREM"
            | "SADD"
            | "ZREM"
            | "ZADD"
            | "GEOADD"
            | "PFADD"
            | "LPOP"
            | "RPOP"
            | "SPOP"
            | "ZPOPMIN"
            | "ZPOPMAX"
            | "XDEL"
            | "XTRIM"
            | "XACK"
            | "EXPIRE"
            | "PEXPIRE"
            | "EXPIREAT"
            | "PEXPIREAT"
            | "PERSIST"
            | "RENAMENX"
            | "MOVE"
            | "COPY"
            | "SMOVE"
            | "LREM"
            | "SINTERSTORE"
            | "SUNIONSTORE"
            | "SDIFFSTORE"
            | "ZDIFFSTORE"
            | "ZINTERSTORE"
            | "ZUNIONSTORE"
    )
}

fn rows_affected_for(cmd: &str, reply: &RedisValue) -> u64 {
    if !is_write_command(cmd) {
        return 0;
    }
    if counts_affected(cmd)
        && let RedisValue::Int(n) = reply
    {
        return (*n).max(0) as u64;
    }
    1
}

// ---------------------------------------------------------------------------
// Streaming helpers
// ---------------------------------------------------------------------------

async fn send_evt(sink: &mpsc::Sender<QueryEvent>, event: QueryEvent) -> MieoResult<()> {
    sink.send(event).await.map_err(|_| MieoError::cancelled())
}

async fn wait_deadline(deadline: Option<tokio::time::Instant>) {
    match deadline {
        Some(at) => tokio::time::sleep_until(at).await,
        None => std::future::pending::<()>().await,
    }
}

/// Stream a materialized result set as `Columns` / `Batch` / `Affected`.
async fn emit_result(
    query_id: &str,
    result: ResultSet,
    options: &QueryOptions,
    sink: &mpsc::Sender<QueryEvent>,
    start: Instant,
    rows_affected: u64,
) -> MieoResult<QueryStats> {
    let fetch_size = options.fetch_size.max(1) as usize;
    let mut stats = QueryStats::default();

    if !result.columns.is_empty() {
        send_evt(
            sink,
            QueryEvent::Columns {
                query_id: query_id.to_string(),
                columns: result.columns,
            },
        )
        .await?;
    }

    let mut batch: Vec<Vec<Value>> = Vec::with_capacity(fetch_size.min(4096));
    let mut batch_index: u64 = 0;
    let mut truncated = false;
    for row in result.rows {
        if options.max_rows.is_some_and(|max| stats.rows >= max) {
            truncated = true;
            break;
        }
        stats.rows += 1;
        batch.push(row);
        if batch.len() >= fetch_size {
            batch_index += 1;
            send_evt(
                sink,
                QueryEvent::Batch {
                    query_id: query_id.to_string(),
                    rows: std::mem::take(&mut batch),
                    batch_index,
                },
            )
            .await?;
            batch.reserve(fetch_size.min(4096));
        }
    }
    if !batch.is_empty() {
        batch_index += 1;
        send_evt(
            sink,
            QueryEvent::Batch {
                query_id: query_id.to_string(),
                rows: batch,
                batch_index,
            },
        )
        .await?;
    }

    stats.rows_affected = rows_affected;
    if rows_affected > 0 {
        send_evt(
            sink,
            QueryEvent::Affected {
                query_id: query_id.to_string(),
                rows_affected,
                last_insert_id: None,
            },
        )
        .await?;
    }

    stats.truncated = truncated;
    stats.elapsed_ms = start.elapsed().as_millis() as u64;
    Ok(stats)
}

/// Find `field` in a `CONFIG GET`-style pair reply.
fn find_pair_field(reply: &RedisValue, field: &str) -> Option<String> {
    for (key, value) in reply.as_map_iter()? {
        if reply_as_text(key).as_deref() == Some(field) {
            return reply_as_text(value);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn bulk(text: &str) -> RedisValue {
        RedisValue::BulkString(text.as_bytes().to_vec())
    }

    fn tokenize(input: &str) -> Vec<String> {
        tokenize_command(input)
            .unwrap()
            .into_iter()
            .map(|token| String::from_utf8(token).unwrap())
            .collect()
    }

    fn spec(
        host: &str,
        port: u16,
        username: Option<&str>,
        password: Option<&str>,
        database: Option<&str>,
        ssl_mode: SslMode,
    ) -> ConnectionSpec {
        ConnectionSpec {
            profile_id: None,
            name: "test".to_string(),
            engine: Engine::Redis,
            host: Some(host.to_string()),
            port: Some(port),
            username: username.map(str::to_string),
            password: password.map(str::to_string),
            database: database.map(str::to_string),
            file: None,
            params: Default::default(),
            ssl_mode,
            variant: None,
            ssh: None,
            ssh_password: None,
            connect_timeout_secs: 15,
        }
    }

    // --- tokenizer -----------------------------------------------------

    #[test]
    fn tokenizer_splits_on_whitespace() {
        assert_eq!(tokenize("GET user:1"), vec!["GET", "user:1"]);
        assert_eq!(
            tokenize("  SCAN   0   MATCH user:*  COUNT 200 "),
            vec!["SCAN", "0", "MATCH", "user:*", "COUNT", "200"]
        );
    }

    #[test]
    fn tokenizer_honors_quotes_and_escapes() {
        assert_eq!(
            tokenize(r#"SET greeting "hello world""#),
            vec!["SET", "greeting", "hello world"]
        );
        assert_eq!(
            tokenize(r#"SET greeting 'single quoted'"#),
            vec!["SET", "greeting", "single quoted"]
        );
        assert_eq!(
            tokenize(r#"SET key "line1\nline2""#),
            vec!["SET", "key", "line1\nline2"]
        );
        assert_eq!(tokenize(r#"SET key 'it\'s'"#), vec!["SET", "key", "it's"]);
        assert_eq!(
            tokenize(r#"SET key "say \"hi\"""#),
            vec!["SET", "key", "say \"hi\""]
        );
        // Empty quoted argument survives.
        assert_eq!(tokenize(r#"SET key """#), vec!["SET", "key", ""]);
    }

    #[test]
    fn tokenizer_decodes_hex_escapes_to_raw_bytes() {
        let tokens = tokenize_command(r"SET key \x00\xff").unwrap();
        assert_eq!(&tokens[0], b"SET");
        assert_eq!(tokens[2], vec![0x00, 0xff]);
    }

    #[test]
    fn tokenizer_rejects_invalid_input() {
        assert!(tokenize_command(r#"GET "unterminated"#).is_err());
        assert!(tokenize_command("GET trailing\\").is_err());
        assert!(tokenize_command(r#"GET "\xzz""#).is_err());
    }

    // --- reply mapping --------------------------------------------------

    #[test]
    fn reply_nil_maps_to_single_null_row() {
        let result = reply_to_result("GET", &[], &RedisValue::Nil);
        assert_eq!(result.columns.len(), 1);
        assert_eq!(result.rows, vec![vec![Value::Null]]);
    }

    #[test]
    fn reply_scalars_map_to_value_column() {
        let result = reply_to_result("GET", &[], &bulk("hello"));
        assert_eq!(result.columns[0].name, "value");
        assert_eq!(result.rows, vec![vec![Value::Text("hello".into())]]);

        let result = reply_to_result("INCR", &[], &RedisValue::Int(41));
        assert_eq!(result.rows, vec![vec![Value::Int(41)]]);
        assert_eq!(result.columns[0].kind, DataTypeKind::Int);

        let result = reply_to_result("SET", &[], &RedisValue::Okay);
        assert_eq!(result.rows, vec![vec![Value::Text("OK".into())]]);
    }

    #[test]
    fn reply_non_utf8_bulk_maps_to_base64_bytes() {
        let result = reply_to_result("GET", &[], &RedisValue::BulkString(vec![0x00, 0xff]));
        assert_eq!(
            result.rows[0][0],
            Value::Bytes(crate::util::b64_encode(&[0x00, 0xff]))
        );
    }

    #[test]
    fn reply_array_of_scalars_maps_to_rows() {
        let reply = RedisValue::Array(vec![bulk("a"), bulk("b")]);
        let result = reply_to_result("LRANGE", &[], &reply);
        assert_eq!(result.columns.len(), 1);
        assert_eq!(
            result.rows,
            vec![
                vec![Value::Text("a".into())],
                vec![Value::Text("b".into())]
            ]
        );
    }

    #[test]
    fn reply_hgetall_maps_to_field_value_pairs() {
        let reply = RedisValue::Array(vec![bulk("f1"), bulk("v1"), bulk("f2"), bulk("v2")]);
        let result = reply_to_result("HGETALL", &[], &reply);
        assert_eq!(
            result
                .columns
                .iter()
                .map(|c| c.name.as_str())
                .collect::<Vec<_>>(),
            vec!["field", "value"]
        );
        assert_eq!(result.rows.len(), 2);
        assert_eq!(result.rows[0][0], Value::Text("f1".into()));
        assert_eq!(result.rows[1][1], Value::Text("v2".into()));
    }

    #[test]
    fn reply_zrange_pairs_only_with_withscores() {
        let reply = RedisValue::Array(vec![bulk("m1"), bulk("1.5")]);
        let with = reply_to_result(
            "ZRANGE",
            &[
                "k".into(),
                "0".into(),
                "-1".into(),
                "WITHSCORES".into(),
            ],
            &reply,
        );
        assert_eq!(with.columns.len(), 2);
        assert_eq!(with.rows.len(), 1);

        let without = reply_to_result("ZRANGE", &["k".into(), "0".into(), "-1".into()], &reply);
        assert_eq!(without.columns.len(), 1);
        assert_eq!(without.rows.len(), 2);
    }

    #[test]
    fn reply_scan_cursor_unwraps_key_list() {
        let reply = RedisValue::Array(vec![
            bulk("17"),
            RedisValue::Array(vec![bulk("k1"), bulk("k2")]),
        ]);
        let result = reply_to_result("SCAN", &[], &reply);
        assert_eq!(
            result.rows,
            vec![
                vec![Value::Text("k1".into())],
                vec![Value::Text("k2".into())]
            ]
        );
    }

    #[test]
    fn reply_hscan_cursor_unwraps_field_value_pairs() {
        let reply = RedisValue::Array(vec![
            bulk("0"),
            RedisValue::Array(vec![bulk("f1"), bulk("v1")]),
        ]);
        let result = reply_to_result("HSCAN", &[], &reply);
        assert_eq!(result.columns.len(), 2);
        assert_eq!(result.rows.len(), 1);
        assert_eq!(result.rows[0][0], Value::Text("f1".into()));
    }

    #[test]
    fn reply_nested_arrays_flatten_to_c_columns() {
        let reply = RedisValue::Array(vec![RedisValue::Array(vec![
            bulk("1-1"),
            RedisValue::Array(vec![bulk("f"), bulk("v")]),
        ])]);
        let result = reply_to_result("XRANGE", &[], &reply);
        assert_eq!(
            result
                .columns
                .iter()
                .map(|c| c.name.as_str())
                .collect::<Vec<_>>(),
            vec!["c1", "c2", "c3"]
        );
        assert_eq!(
            result.rows,
            vec![vec![
                Value::Text("1-1".into()),
                Value::Text("f".into()),
                Value::Text("v".into())
            ]]
        );
    }

    #[test]
    fn reply_map_maps_to_pairs() {
        let reply = RedisValue::Map(vec![(bulk("databases"), bulk("16"))]);
        let result = reply_to_result("CONFIG", &["GET".into(), "databases".into()], &reply);
        assert_eq!(result.columns.len(), 2);
        assert_eq!(
            result.rows,
            vec![vec![
                Value::Text("databases".into()),
                Value::Text("16".into())
            ]]
        );
    }

    #[test]
    fn reply_empty_array_keeps_value_column() {
        let result = reply_to_result("KEYS", &[], &RedisValue::Array(vec![]));
        assert_eq!(result.columns.len(), 1);
        assert_eq!(result.columns[0].name, "value");
        assert!(result.rows.is_empty());
    }

    #[test]
    fn parse_scan_reply_reads_cursor_and_keys() {
        let reply = RedisValue::Array(vec![
            RedisValue::Int(17),
            RedisValue::Array(vec![bulk("a"), bulk("b")]),
        ]);
        let (cursor, keys) = parse_scan_reply(&reply).unwrap();
        assert_eq!(cursor, 17);
        assert_eq!(keys, vec!["a", "b"]);
        assert!(parse_scan_reply(&bulk("nope")).is_err());
    }

    // --- PREVIEW --------------------------------------------------------

    #[test]
    fn preview_plan_selects_command_by_type() {
        assert_eq!(
            preview_plan("user:1", "string"),
            PreviewPlan::Commands(vec![PreviewCommand {
                name: "GET".into(),
                args: vec!["user:1".into()],
                shape: PreviewShape::Scalar,
            }])
        );
        assert_eq!(
            preview_plan("user:1", "hash"),
            PreviewPlan::Commands(vec![PreviewCommand {
                name: "HGETALL".into(),
                args: vec!["user:1".into()],
                shape: PreviewShape::Pairs,
            }])
        );
        assert_eq!(
            preview_plan("user:1", "list"),
            PreviewPlan::Commands(vec![PreviewCommand {
                name: "LRANGE".into(),
                args: vec!["user:1".into(), "0".into(), "99".into()],
                shape: PreviewShape::IndexValue,
            }])
        );
        assert_eq!(
            preview_plan("user:1", "set"),
            PreviewPlan::Commands(vec![PreviewCommand {
                name: "SMEMBERS".into(),
                args: vec!["user:1".into()],
                shape: PreviewShape::Scalar,
            }])
        );
        assert_eq!(
            preview_plan("user:1", "zset"),
            PreviewPlan::Commands(vec![PreviewCommand {
                name: "ZRANGE".into(),
                args: vec![
                    "user:1".into(),
                    "0".into(),
                    "99".into(),
                    "WITHSCORES".into()
                ],
                shape: PreviewShape::Pairs,
            }])
        );
        assert_eq!(
            preview_plan("events", "stream"),
            PreviewPlan::Commands(vec![PreviewCommand {
                name: "XRANGE".into(),
                args: vec![
                    "events".into(),
                    "-".into(),
                    "+".into(),
                    "COUNT".into(),
                    "100".into()
                ],
                shape: PreviewShape::Nested,
            }])
        );
        assert_eq!(preview_plan("missing", "none"), PreviewPlan::Missing);
        assert_eq!(
            preview_plan("module", "ReJSON-RL"),
            PreviewPlan::Unsupported("ReJSON-RL".into())
        );
    }

    #[test]
    fn preview_list_rows_carry_index_column() {
        let reply = RedisValue::Array(vec![bulk("a"), bulk("b")]);
        let result = map_preview_reply(PreviewShape::IndexValue, &reply);
        assert_eq!(
            result
                .columns
                .iter()
                .map(|c| c.name.as_str())
                .collect::<Vec<_>>(),
            vec!["index", "value"]
        );
        assert_eq!(result.rows[0][0], Value::Int(0));
        assert_eq!(result.rows[1][0], Value::Int(1));
        assert_eq!(result.rows[1][1], Value::Text("b".into()));
    }

    #[test]
    fn preview_stream_uses_id_field_value_columns() {
        let reply = RedisValue::Array(vec![
            RedisValue::Array(vec![bulk("1-1"), RedisValue::Array(vec![bulk("f1"), bulk("v1")])]),
            RedisValue::Array(vec![bulk("1-2"), RedisValue::Array(vec![bulk("f2"), bulk("v2")])]),
        ]);
        let result = map_preview_reply(PreviewShape::Nested, &reply);
        assert_eq!(
            result
                .columns
                .iter()
                .map(|c| c.name.as_str())
                .collect::<Vec<_>>(),
            vec!["id", "field", "value"]
        );
        assert_eq!(result.rows.len(), 2);
        assert_eq!(result.rows[0][0], Value::Text("1-1".into()));
        assert_eq!(result.rows[1][2], Value::Text("v2".into()));
    }

    // --- helpers --------------------------------------------------------

    #[test]
    fn write_commands_report_rows_affected() {
        assert_eq!(rows_affected_for("SET", &RedisValue::Okay), 1);
        assert_eq!(rows_affected_for("DEL", &RedisValue::Int(3)), 3);
        assert_eq!(rows_affected_for("DEL", &RedisValue::Int(0)), 0);
        assert_eq!(rows_affected_for("HSET", &RedisValue::Int(2)), 2);
        assert_eq!(rows_affected_for("GET", &bulk("x")), 0);
        assert!(!is_write_command("GET"));
        assert!(is_write_command("HSET"));
    }

    #[test]
    fn value_binding_converts_params() {
        assert_eq!(value_to_arg(&Value::Text("a b".into())).unwrap(), b"a b");
        assert_eq!(value_to_arg(&Value::Int(-7)).unwrap(), b"-7");
        assert_eq!(value_to_arg(&Value::Bool(true)).unwrap(), b"1");
        assert_eq!(
            value_to_arg(&Value::Bytes(crate::util::b64_encode(&[1, 2]))).unwrap(),
            vec![1, 2]
        );
        assert!(value_to_arg(&Value::Null).is_err());
    }

    #[test]
    fn build_url_encodes_credentials_and_tls() {
        assert_eq!(
            build_url(&spec("localhost", 6379, None, None, None, SslMode::Disable)),
            "redis://localhost:6379"
        );
        assert_eq!(
            build_url(&spec(
                "cache.example.com",
                6380,
                Some("default"),
                Some("p@ss:word"),
                Some("3"),
                SslMode::VerifyFull,
            )),
            "rediss://default:p%40ss%3Aword@cache.example.com:6380/3"
        );
        assert_eq!(
            build_url(&spec("::1", 6379, None, None, None, SslMode::Disable)),
            "redis://[::1]:6379"
        );
        assert_eq!(
            build_url(&spec(
                "h",
                6379,
                None,
                Some("pw"),
                None,
                SslMode::Disable
            )),
            "redis://:pw@h:6379"
        );
    }
}
