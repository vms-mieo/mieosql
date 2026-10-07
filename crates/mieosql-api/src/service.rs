//! The service facade: every operation the frontend can perform.

use chrono::Utc;

use mieosql_core::{
    Capabilities, ColumnDef, ConnectionProfile, ConnectionSpec, DatabaseRef, Engine, ForeignKeyDef,
    IndexDef, ObjectKind, ObjectRef, ProfileId, QueryEvent, QueryMode, QueryOptions, SchemaRef,
    Scope, ServerInfo, Value,
};
use mieosql_engine::{
    Engine as CoreEngine, HistoryEntry, ProfileSecrets, QueryStream, SessionInfo,
};

use crate::dto::{ApiError, ApiResult, ProfileInput};
use crate::export::{ExportFormat, ExportStats, ResultExporter};

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

    pub async fn open_session(&self, profile_id: String) -> ApiResult<SessionInfo> {
        self.engine
            .open_session_for_profile(&ProfileId(profile_id))
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
        let sql = build_select_sql(info.engine, &scope, &table, limit);
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
        _ => format!("\"{}\"", name.replace('"', "\"\"")),
    }
}

/// Build the grid-preview statement for a table with safe identifier quoting.
///
/// - PostgreSQL: identifiers are schema-qualified; a connection is pinned to
///   one database, so the database part is only used to switch beforehand.
/// - MySQL/MariaDB/SQLite: identifiers are database-qualified.
pub fn build_select_sql(engine: Engine, scope: &Scope, table: &str, limit: u64) -> String {
    let table = quote_ident(engine, table);
    let qualified = match engine {
        Engine::Postgres => match scope.schema.as_deref().filter(|s| !s.is_empty()) {
            Some(schema) => format!("{}.{table}", quote_ident(engine, schema)),
            None => table,
        },
        _ => match scope.database.as_deref().filter(|s| !s.is_empty()) {
            Some(database) => format!("{}.{table}", quote_ident(engine, database)),
            None => table,
        },
    };
    format!("SELECT * FROM {qualified} LIMIT {limit}")
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
