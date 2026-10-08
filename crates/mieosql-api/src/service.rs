//! The service facade: every operation the frontend can perform.

use chrono::Utc;

use mieosql_core::{
    Capabilities, ColumnDef, ConnectionProfile, ConnectionSpec, DatabaseRef, Engine, ErrorCode,
    ForeignKeyDef, IndexDef, MieoError, ObjectKind, ObjectRef, ProfileId, QueryEvent, QueryMode,
    QueryOptions, SchemaRef, Scope, ServerInfo, Value,
};
use mieosql_engine::{
    Engine as CoreEngine, HistoryEntry, ProfileSecrets, QueryStream, SessionInfo,
};

use crate::dto::{ApiError, ApiResult, ProfileInput};
use crate::export::{ExportFormat, ExportStats, ResultExporter};
use crate::fetch::CollectedRows;
use crate::maintenance::{KafkaConsumeFrom, KafkaProduceResult, KafkaTopicPartition};

/// Frontend-facing facade over the engine.
///
/// Deliberately transport-agnostic: Tauri commands, the CLI and (later) an
/// HTTP/WebSocket bridge all go through this type.
#[derive(Clone)]
pub struct Api {
    engine: CoreEngine,
}

impl Api {
    pub fn new(engine: CoreEngine) -> Self {
        Self { engine }
    }

    pub fn engine(&self) -> &CoreEngine {
        &self.engine
    }

    // ------------------------------------------------------------------
    // Profiles
    // ------------------------------------------------------------------

    pub fn list_profiles(&self) -> Vec<ConnectionProfile> {
        self.engine.list_profiles()
    }

    pub fn get_profile(&self, id: String) -> ApiResult<ConnectionProfile> {
        self.engine.get_profile(&ProfileId(id)).map_err(Into::into)
    }

    pub fn save_profile(&self, input: ProfileInput) -> ApiResult<ConnectionProfile> {
        let (profile, secrets) = build_profile(input)?;
        self.engine
            .save_profile(&profile, &secrets)
            .map_err(Into::into)
    }

    pub fn delete_profile(&self, id: String) -> ApiResult<()> {
        self.engine
            .delete_profile(&ProfileId(id))
            .map_err(Into::into)
    }

    pub fn has_password(&self, id: String) -> bool {
        self.engine.has_password(&ProfileId(id))
    }

    // ------------------------------------------------------------------
    // Connection testing
    // ------------------------------------------------------------------

    /// Test a saved profile (optionally overriding the stored password).
    pub async fn test_profile(
        &self,
        id: String,
        password: Option<String>,
    ) -> ApiResult<ServerInfo> {
        self.engine
            .test_profile(&ProfileId(id), password)
            .await
            .map_err(Into::into)
    }

    /// Test connection parameters that may not be saved yet.
    pub async fn test_connection(&self, input: ProfileInput) -> ApiResult<ServerInfo> {
        let (profile, secrets) = build_profile(input)?;
        let spec = ConnectionSpec::from_profile(&profile, secrets.password);
        self.engine.test_connection(&spec).await.map_err(Into::into)
    }

    // ------------------------------------------------------------------
    // Sessions
    // ------------------------------------------------------------------

    pub async fn open_session(
        &self,
        profile_id: String,
        password: Option<String>,
    ) -> ApiResult<SessionInfo> {
        self.engine
            .open_session_for_profile_with_password(&ProfileId(profile_id), password)
            .await
            .map_err(Into::into)
    }

    pub async fn close_session(&self, session_id: String) -> ApiResult<()> {
        self.engine
            .close_session(&session_id)
            .await
            .map_err(Into::into)
    }

    pub async fn list_sessions(&self) -> Vec<SessionInfo> {
        self.engine.list_sessions().await
    }

    pub async fn session_info(&self, session_id: String) -> ApiResult<SessionInfo> {
        self.engine
            .session_info(&session_id)
            .await
            .map_err(Into::into)
    }

    pub async fn capabilities(&self, session_id: String) -> ApiResult<Capabilities> {
        self.engine
            .capabilities(&session_id)
            .await
            .map_err(Into::into)
    }

    pub async fn server_info(&self, session_id: String) -> ApiResult<ServerInfo> {
        self.engine
            .server_info(&session_id)
            .await
            .map_err(Into::into)
    }

    // ------------------------------------------------------------------
    // Introspection
    // ------------------------------------------------------------------

    pub async fn list_databases(&self, session_id: String) -> ApiResult<Vec<DatabaseRef>> {
        self.engine
            .list_databases(&session_id)
            .await
            .map_err(Into::into)
    }

    pub async fn list_schemas(
        &self,
        session_id: String,
        database: Option<String>,
    ) -> ApiResult<Vec<SchemaRef>> {
        self.engine
            .list_schemas(&session_id, database.as_deref())
            .await
            .map_err(Into::into)
    }

    pub async fn list_objects(
        &self,
        session_id: String,
        scope: Scope,
        kind: Option<ObjectKind>,
        pattern: Option<String>,
    ) -> ApiResult<Vec<ObjectRef>> {
        self.engine
            .list_objects(&session_id, &scope, kind, pattern.as_deref())
            .await
            .map_err(Into::into)
    }

    pub async fn table_columns(
        &self,
        session_id: String,
        scope: Scope,
        table: String,
    ) -> ApiResult<Vec<ColumnDef>> {
        self.engine
            .table_columns(&session_id, &scope, &table)
            .await
            .map_err(Into::into)
    }

    pub async fn table_indexes(
        &self,
        session_id: String,
        scope: Scope,
        table: String,
    ) -> ApiResult<Vec<IndexDef>> {
        self.engine
            .table_indexes(&session_id, &scope, &table)
            .await
            .map_err(Into::into)
    }

    pub async fn foreign_keys(
        &self,
        session_id: String,
        scope: Scope,
        table: String,
    ) -> ApiResult<Vec<ForeignKeyDef>> {
        self.engine
            .foreign_keys(&session_id, &scope, &table)
            .await
            .map_err(Into::into)
    }

    pub async fn object_ddl(
        &self,
        session_id: String,
        scope: Scope,
        kind: ObjectKind,
        name: String,
    ) -> ApiResult<Option<String>> {
        self.engine
            .object_ddl(&session_id, &scope, kind, &name)
            .await
            .map_err(Into::into)
    }

    pub async fn use_database(&self, session_id: String, database: String) -> ApiResult<()> {
        self.engine
            .use_database(&session_id, &database)
            .await
            .map_err(Into::into)
    }

    // ------------------------------------------------------------------
    // Kafka topic administration
    // ------------------------------------------------------------------

    /// Kafka topic names allow ASCII letters/digits plus `.`, `_`, `-`
    /// (mirrors the broker's own validation).
    fn validate_kafka_topic(topic: &str) -> ApiResult<()> {
        if topic.is_empty() || topic.len() > 249 {
            return Err(
                MieoError::new(ErrorCode::Data, "Kafka topic name must be 1-249 characters")
                    .into(),
            );
        }
        if !topic
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
        {
            return Err(MieoError::new(
                ErrorCode::Data,
                format!(
                    "invalid Kafka topic name `{topic}` (allowed: letters, digits, `.`, `_`, `-`)"
                ),
            )
            .into());
        }
        Ok(())
    }

    pub async fn kafka_create_topic(
        &self,
        session_id: String,
        topic: String,
        partitions: i32,
        replication_factor: i16,
    ) -> ApiResult<()> {
        Self::validate_kafka_topic(&topic)?;
        if partitions < 1 || partitions > 100_000 {
            return Err(
                MieoError::new(ErrorCode::Data, "partitions must be between 1 and 100000").into(),
            );
        }
        if replication_factor < 1 {
            return Err(
                MieoError::new(ErrorCode::Data, "replication factor must be >= 1").into(),
            );
        }
        self.engine
            .kafka_create_topic(&session_id, &topic, partitions, replication_factor)
            .await
            .map_err(Into::into)
    }

    pub async fn kafka_delete_topic(&self, session_id: String, topic: String) -> ApiResult<()> {
        Self::validate_kafka_topic(&topic)?;
        self.engine
            .kafka_delete_topic(&session_id, &topic)
            .await
            .map_err(Into::into)
    }

    pub async fn kafka_topic_partitions(
        &self,
        session_id: String,
        topic: String,
    ) -> ApiResult<Vec<KafkaTopicPartition>> {
        Self::validate_kafka_topic(&topic)?;
        let marks = self
            .engine
            .kafka_topic_partitions(&session_id, &topic)
            .await
            .map_err(ApiError::from)?;
        Ok(marks
            .into_iter()
            .map(|(partition, earliest, latest)| KafkaTopicPartition {
                partition,
                earliest,
                latest,
            })
            .collect())
    }

    pub async fn kafka_produce(
        &self,
        session_id: String,
        topic: String,
        partition: Option<i32>,
        key: Option<String>,
        value: String,
    ) -> ApiResult<KafkaProduceResult> {
        Self::validate_kafka_topic(&topic)?;
        if let Some(p) = partition
            && p < 0
        {
            return Err(MieoError::new(ErrorCode::Data, "`partition` must be >= 0").into());
        }
        let (partition, offset) = self
            .engine
            .kafka_produce(
                &session_id,
                &topic,
                partition,
                key.filter(|k| !k.is_empty()),
                value,
            )
            .await
            .map_err(ApiError::from)?;
        Ok(KafkaProduceResult { partition, offset })
    }

    /// Fetch up to `limit` records from one topic partition (bounded,
    /// kafka-ui style message browsing). An omitted partition resolves to the
    /// topic's first partition.
    pub async fn kafka_consume(
        &self,
        session_id: String,
        topic: String,
        partition: Option<i32>,
        from: KafkaConsumeFrom,
        limit: u32,
    ) -> ApiResult<CollectedRows> {
        Self::validate_kafka_topic(&topic)?;
        let partition = match partition {
            Some(p) => {
                if p < 0 {
                    return Err(
                        MieoError::new(ErrorCode::Data, "`partition` must be >= 0").into(),
                    );
                }
                p
            }
            None => {
                let marks = self
                    .engine
                    .kafka_topic_partitions(&session_id, &topic)
                    .await
                    .map_err(ApiError::from)?;
                marks.first().map(|(p, _, _)| *p).ok_or_else(|| {
                    ApiError::from(MieoError::new(
                        ErrorCode::NotFound,
                        format!("Kafka topic `{topic}` has no partitions"),
                    ))
                })?
            }
        };
        let from_part = match from {
            KafkaConsumeFrom::Earliest => "\"earliest\"".to_string(),
            KafkaConsumeFrom::Latest => "\"latest\"".to_string(),
            KafkaConsumeFrom::Offset(o) => o.to_string(),
        };
        let limit = u64::from(limit.clamp(1, 2000));
        // Topic names are validated above, so JSON embedding is safe.
        let command = format!(
            "{{ \"topic\": \"{topic}\", \"partition\": {partition}, \"from\": {from_part}, \"limit\": {limit} }}"
        );
        let out = self
            .collect_query(&session_id, command, Vec::new(), QueryMode::Simple, Some(limit))
            .await?;
        Ok(CollectedRows {
            columns: out.columns,
            rows: out.rows,
            stats: out.stats,
        })
    }

    // ------------------------------------------------------------------
    // Transactions
    // ------------------------------------------------------------------

    pub async fn begin_transaction(&self, session_id: String) -> ApiResult<()> {
        self.engine
            .begin_transaction(&session_id)
            .await
            .map_err(Into::into)
    }

    pub async fn commit_transaction(&self, session_id: String) -> ApiResult<()> {
        self.engine
            .commit_transaction(&session_id)
            .await
            .map_err(Into::into)
    }

    pub async fn rollback_transaction(&self, session_id: String) -> ApiResult<()> {
        self.engine
            .rollback_transaction(&session_id)
            .await
            .map_err(Into::into)
    }

    // ------------------------------------------------------------------
    // Queries
    // ------------------------------------------------------------------

    /// Start a query; events stream through `QueryStream::events`.
    pub async fn start_query(
        &self,
        session_id: String,
        sql: String,
        params: Vec<Value>,
        options: Option<QueryOptions>,
    ) -> ApiResult<QueryStream> {
        self.engine
            .run_query(&session_id, sql, params, options.unwrap_or_default())
            .await
            .map_err(Into::into)
    }

    /// Start a `SELECT * FROM <table> LIMIT n` grid preview with correctly
    /// quoted identifiers for the session's engine. Returns the stream and the
    /// generated SQL (so it can be re-run verbatim for exports).
    pub async fn preview_table(
        &self,
        session_id: String,
        scope: Scope,
        table: String,
        limit: Option<u64>,
    ) -> ApiResult<(QueryStream, String)> {
        let info = self.engine.session_info(&session_id).await?;
        let limit = limit.unwrap_or(200).clamp(1, 1_000_000);
        let sql = if info.engine.is_sql() {
            build_select_sql(info.engine, &scope, &table, limit)
        } else {
            self.engine
                .preview_statement(&session_id, &scope, &table, limit)
                .await?
        };
        let stream = self
            .engine
            .run_query(
                &session_id,
                sql.clone(),
                Vec::new(),
                QueryOptions {
                    mode: QueryMode::Prepared,
                    fetch_size: 512,
                    max_rows: Some(limit),
                    timeout_ms: None,
                },
            )
            .await
            .map_err(ApiError::from)?;
        Ok((stream, sql))
    }

    /// Re-run `sql` and stream the result straight into a file.
    /// The query is executed server-side; row data never crosses the IPC bridge.
    pub async fn export_result(
        &self,
        session_id: String,
        sql: String,
        format: ExportFormat,
        path: String,
        table: Option<String>,
    ) -> ApiResult<ExportStats> {
        let info = self.engine.session_info(&session_id).await?;
        let started = std::time::Instant::now();
        let mut stream = self
            .engine
            .run_query(
                &session_id,
                sql,
                Vec::new(),
                QueryOptions {
                    mode: QueryMode::Prepared,
                    fetch_size: 2048,
                    max_rows: None,
                    timeout_ms: None,
                },
            )
            .await?;

        let table_name = table.unwrap_or_else(|| "result".to_string());
        let mut exporter = ResultExporter::create(format, info.engine, table_name, path.into())?;

        while let Some(event) = stream.events.recv().await {
            match event {
                QueryEvent::Columns { columns, .. } => exporter.set_columns(columns)?,
                QueryEvent::Batch { rows, .. } => exporter.write_rows(&rows)?,
                QueryEvent::Error { error, .. } => {
                    exporter.abort();
                    return Err(ApiError::from(error));
                }
                QueryEvent::Finished { .. } => break,
                _ => {}
            }
        }

        let mut stats = exporter.finish()?;
        stats.elapsed_ms = started.elapsed().as_millis() as u64;
        Ok(stats)
    }

    pub fn cancel_query(&self, query_id: String) -> bool {
        self.engine.cancel_query(&query_id)
    }

    // ------------------------------------------------------------------
    // History
    // ------------------------------------------------------------------

    pub fn list_history(&self, limit: Option<usize>) -> Vec<HistoryEntry> {
        self.engine.list_history(limit.unwrap_or(200))
    }

    pub fn clear_history(&self) {
        self.engine.clear_history();
    }

    pub fn history_retention_days(&self) -> u32 {
        self.engine.history_retention_days()
    }

    pub fn set_history_retention_days(&self, days: u32) -> ApiResult<u32> {
        self.engine
            .set_history_retention_days(days)
            .map_err(Into::into)
    }

    /// Cancel every in-flight query (called on app shutdown).
    pub fn shutdown(&self) {
        self.engine.cancel_all_queries();
    }
}

/// Convert dialog input into a stored profile + its secrets.
fn build_profile(input: ProfileInput) -> ApiResult<(ConnectionProfile, ProfileSecrets)> {
    let mut profile = ConnectionProfile::new(input.name.clone(), input.engine, Utc::now());
    if let Some(id) = input.id.clone() {
        profile.id = ProfileId(id);
    }
    profile.host = input.host.clone();
    profile.port = input.port.or_else(|| input.engine.default_port());
    profile.username = input.username.clone();
    profile.database = input.database.clone();
    profile.file = input.file.clone();
    profile.params = input.params.clone();
    profile.ssl_mode = input.ssl_mode.unwrap_or_default();
    profile.ssh = input.ssh.clone();
    profile.color = input.color.clone();
    profile.group = input.group.clone();
    profile.variant = input.variant.clone();

    let secrets = ProfileSecrets {
        password: input.password.clone(),
        ssh_password: input.ssh_password.clone(),
    };
    Ok((profile, secrets))
}

// ---------------------------------------------------------------------------
// SQL generation helpers
// ---------------------------------------------------------------------------

/// Quote an identifier according to the engine's rules.
pub fn quote_ident(engine: Engine, name: &str) -> String {
    match engine {
        Engine::MySql | Engine::MariaDb => format!("`{}`", name.replace('`', "``")),
        Engine::SqlServer => format!("[{}]", name.replace(']', "]]")),
        _ => format!("\"{}\"", name.replace('"', "\"\"")),
    }
}

/// Build the grid-preview statement for a table with safe identifier quoting.
///
/// - PostgreSQL: identifiers are schema-qualified; a connection is pinned to
///   one database, so the database part is only used to switch beforehand.
/// - SQL Server: identifiers are schema-qualified the same way.
/// - MySQL/MariaDB/SQLite: identifiers are database-qualified.
pub fn build_select_sql(engine: Engine, scope: &Scope, table: &str, limit: u64) -> String {
    let table = quote_ident(engine, table);
    let qualified = match engine {
        Engine::Postgres | Engine::SqlServer => {
            match scope.schema.as_deref().filter(|s| !s.is_empty()) {
                Some(schema) => format!("{}.{table}", quote_ident(engine, schema)),
                None => table,
            }
        }
        _ => match scope.database.as_deref().filter(|s| !s.is_empty()) {
            Some(database) => format!("{}.{table}", quote_ident(engine, database)),
            None => table,
        },
    };
    format!("SELECT * FROM {qualified} LIMIT {limit}")
}

/// Render a `SELECT` with an engine-correct row cap.
///
/// `select_list` is the projection (`*` or `a, b`), `from_where` the rest of
/// the statement (`FROM t WHERE ...`). SQL Server needs `TOP (n)` right after
/// `SELECT`; every other engine gets a trailing `LIMIT n`.
pub fn select_limited(engine: Engine, select_list: &str, from_where: &str, n: u64) -> String {
    match engine {
        Engine::SqlServer => format!("SELECT TOP ({n}) {select_list} {from_where}"),
        _ => format!("SELECT {select_list} {from_where} LIMIT {n}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope(db: Option<&str>, schema: Option<&str>) -> Scope {
        Scope {
            database: db.map(str::to_string),
            schema: schema.map(str::to_string),
        }
    }

    #[test]
    fn quotes_mysql_identifiers() {
        let sql = build_select_sql(Engine::MySql, &scope(Some("shop"), None), "order", 100);
        assert_eq!(sql, "SELECT * FROM `shop`.`order` LIMIT 100");
    }

    #[test]
    fn quotes_postgres_schema() {
        let sql = build_select_sql(
            Engine::Postgres,
            &scope(Some("main_db"), Some("Public")),
            "users",
            50,
        );
        assert_eq!(sql, "SELECT * FROM \"Public\".\"users\" LIMIT 50");
    }

    #[test]
    fn escapes_embedded_quotes() {
        assert_eq!(quote_ident(Engine::MySql, "we`ird"), "`we``ird`");
        assert_eq!(quote_ident(Engine::Postgres, "we\"ird"), "\"we\"\"ird\"");
    }

    #[test]
    fn sqlite_uses_database_qualifier() {
        let sql = build_select_sql(Engine::Sqlite, &scope(Some("main"), None), "items", 10);
        assert_eq!(sql, "SELECT * FROM \"main\".\"items\" LIMIT 10");
    }
}
