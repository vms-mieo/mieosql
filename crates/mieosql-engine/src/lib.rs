//! MieoSQL engine: the stateful service layer between drivers and the API.
//!
//! Responsibilities:
//! - connection profiles (metadata) + encrypted vault (secrets)
//! - sessions (open connections) and query cancellation
//! - query execution with streaming events and history

mod history;
mod session;
mod settings;
mod store;
mod vault;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::Utc;
use mieosql_core::{
    Capabilities,
    ColumnDef,
    ConnectionProfile,
    ConnectionSpec,
    CreateDbUser,
    DatabaseRef,
    DbPrincipal,
    DropDbPrincipal,
    ErrorCode,
    ForeignKeyDef,
    GrantRequest,
    IndexDef,
    MieoError,
    MieoResult,
    ObjectKind,
    ObjectRef,
    ProfileId,
    QueryEvent,
    QueryMode,
    QueryOptions,
    RevokeRequest,
    RoleGrant,
    SchemaRef,
    Scope,
    ServerInfo,
    Value,
    DbPrincipalDetail,
};
use mieosql_drivers::AnyDriver;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

pub use history::{HistoryEntry, HistoryStatus, HistoryStore, DEFAULT_RETENTION_DAYS};
pub use session::{QueryStream, Session, SessionInfo, SessionManager};
pub use store::ProfileStore;
pub use vault::Vault;

/// Runtime configuration for the engine.
#[derive(Debug, Clone)]
pub struct EngineConfig {
    /// Directory holding profiles.json, secrets.json, settings.json, history.jsonl.
    pub data_dir: PathBuf,
}

impl EngineConfig {
    /// OS-appropriate application data directory.
    pub fn default_dir() -> PathBuf {
        dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("MieoSQL")
    }
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            data_dir: Self::default_dir(),
        }
    }
}

/// Secrets associated with a profile. `None` keeps the existing secret,
/// `Some("")` clears it.
#[derive(Debug, Clone, Default)]
pub struct ProfileSecrets {
    pub password: Option<String>,
    pub ssh_password: Option<String>,
}

struct EngineInner {
    data_dir: PathBuf,
    profiles: ProfileStore,
    vault: Vault,
    sessions: SessionManager,
    history: HistoryStore,
}

/// The engine facade. Cheap to clone (shares one inner state).
#[derive(Clone)]
pub struct Engine {
    inner: Arc<EngineInner>,
}

impl Engine {
    /// Open (or create) the engine state in `config.data_dir`.
    pub fn open(config: EngineConfig) -> MieoResult<Self> {
        std::fs::create_dir_all(&config.data_dir)?;
        let profiles = ProfileStore::load(config.data_dir.join("profiles.json"))?;
        let vault = Vault::open(&config.data_dir)?;
        let retention = settings::load_history_retention(&config.data_dir);
        let history = HistoryStore::load(config.data_dir.join("history.jsonl"), retention);
        Ok(Self {
            inner: Arc::new(EngineInner {
                data_dir: config.data_dir,
                profiles,
                vault,
                sessions: SessionManager::new(),
                history,
            }),
        })
    }

    pub fn data_dir(&self) -> &Path {
        &self.inner.data_dir
    }

    // ------------------------------------------------------------------
    // Profiles & secrets
    // ------------------------------------------------------------------

    pub fn list_profiles(&self) -> Vec<ConnectionProfile> {
        self.inner.profiles.list()
    }

    pub fn get_profile(&self, id: &ProfileId) -> MieoResult<ConnectionProfile> {
        self.inner.profiles.get(id)
    }

    pub fn save_profile(
        &self,
        profile: &ConnectionProfile,
        secrets: &ProfileSecrets,
    ) -> MieoResult<ConnectionProfile> {
        let stored = self.inner.profiles.upsert(profile.clone())?;
        if let Some(password) = &secrets.password {
            self.inner.vault.set(&stored.id.0, "password", password)?;
        }
        if let Some(ssh_password) = &secrets.ssh_password {
            self.inner
                .vault
                .set(&stored.id.0, "ssh_password", ssh_password)?;
        }
        Ok(stored)
    }

    pub fn delete_profile(&self, id: &ProfileId) -> MieoResult<()> {
        self.inner.profiles.remove(id)?;
        self.inner.vault.remove_profile(&id.0)?;
        Ok(())
    }

    /// Build a fully-resolved connection spec (with secrets) for a profile.
    pub fn resolve_spec(&self, id: &ProfileId) -> MieoResult<ConnectionSpec> {
        let profile = self.inner.profiles.get(id)?;
        let mut spec =
            ConnectionSpec::from_profile(&profile, self.inner.vault.get(&id.0, "password"));
        spec.ssh_password = self.inner.vault.get(&id.0, "ssh_password");
        Ok(spec)
    }

    /// Whether the vault holds a saved password for this profile.
    /// The secret itself never leaves the vault — Navicat-style "Save password" check.
    pub fn has_password(&self, id: &ProfileId) -> bool {
        self.inner.vault.has(&id.0, "password")
    }

    // ------------------------------------------------------------------
    // Connection testing
    // ------------------------------------------------------------------

    pub async fn test_connection(&self, spec: &ConnectionSpec) -> MieoResult<ServerInfo> {
        let mut driver = AnyDriver::connect(spec).await?;
        driver.server_info().await
    }

    pub async fn test_profile(
        &self,
        id: &ProfileId,
        password_override: Option<String>,
    ) -> MieoResult<ServerInfo> {
        let mut spec = self.resolve_spec(id)?;
        if let Some(password) = password_override.filter(|p| !p.is_empty()) {
            spec.password = Some(password);
        }
        self.test_connection(&spec).await
    }

    // ------------------------------------------------------------------
    // Sessions
    // ------------------------------------------------------------------

    pub async fn open_session(&self, spec: &ConnectionSpec) -> MieoResult<session::SessionInfo> {
        let driver = AnyDriver::connect(spec).await?;
        let session = Arc::new(Session {
            id: format!("s_{}", uuid::Uuid::now_v7().simple()),
            profile_id: spec.profile_id.clone(),
            name: spec.name.clone(),
            engine: spec.engine,
            capabilities: driver.capabilities(),
            opened_at: Utc::now(),
            database: std::sync::RwLock::new(spec.database.clone()),
            driver: tokio::sync::Mutex::new(driver),
            spec: spec.clone(),
            meta: tokio::sync::Mutex::new(None),
            backend_pid: tokio::sync::Mutex::new(None),
        });
        let info = session.info().await;
        self.inner.sessions.insert(session);
        Ok(info)
    }

    pub async fn open_session_for_profile(
        &self,
        id: &ProfileId,
    ) -> MieoResult<session::SessionInfo> {
        let spec = self.resolve_spec(id)?;
        self.open_session(&spec).await
    }

    /// Open a session, optionally with a transient password that is never
    /// persisted (used when "Save password" is unchecked).
    pub async fn open_session_for_profile_with_password(
        &self,
        id: &ProfileId,
        password_override: Option<String>,
    ) -> MieoResult<session::SessionInfo> {
        let mut spec = self.resolve_spec(id)?;
        if let Some(password) = password_override.filter(|p| !p.is_empty()) {
            spec.password = Some(password);
        }
        self.open_session(&spec).await
    }

    pub async fn close_session(&self, session_id: &str) -> MieoResult<()> {
        self.inner.sessions.remove(session_id).ok_or_else(|| {
            MieoError::new(
                ErrorCode::NotFound,
                format!("session `{session_id}` not found"),
            )
        })?;
        Ok(())
    }

    pub async fn list_sessions(&self) -> Vec<session::SessionInfo> {
        let sessions = self.inner.sessions.list();
        let mut out = Vec::with_capacity(sessions.len());
        for s in sessions {
            out.push(s.info().await);
        }
        out
    }

    pub async fn session_info(&self, session_id: &str) -> MieoResult<session::SessionInfo> {
        let session = self.inner.sessions.get(session_id)?;
        Ok(session.info().await)
    }

    // ------------------------------------------------------------------
    // Introspection
    // ------------------------------------------------------------------

    pub async fn server_info(&self, session_id: &str) -> MieoResult<ServerInfo> {
        let session = self.inner.sessions.get(session_id)?;
        let mut conn = introspection_conn(&session, &self.inner.sessions).await?;
        conn.driver().server_info().await
    }

    pub async fn list_databases(&self, session_id: &str) -> MieoResult<Vec<DatabaseRef>> {
        let session = self.inner.sessions.get(session_id)?;
        let mut conn = introspection_conn(&session, &self.inner.sessions).await?;
        conn.driver().list_databases().await
    }

    /// List schemas; engines where schema == database fall back to databases.
    pub async fn list_schemas(
        &self,
        session_id: &str,
        database: Option<&str>,
    ) -> MieoResult<Vec<SchemaRef>> {
        let session = self.inner.sessions.get(session_id)?;
        let mut conn = introspection_conn(&session, &self.inner.sessions).await?;
        let driver = conn.driver();
        if driver.capabilities().schema_is_database {
            let dbs = driver.list_databases().await?;
            let only = database.map(|d| d.to_string());
            return Ok(dbs
                .into_iter()
                .filter(|db| only.as_deref().is_none_or(|o| o == db.name))
                .map(|db| SchemaRef {
                    name: db.name,
                    is_system: db.is_system,
                })
                .collect());
        }
        driver.list_schemas(database).await
    }

    pub async fn list_objects(
        &self,
        session_id: &str,
        scope: &Scope,
        kind: Option<ObjectKind>,
        pattern: Option<&str>,
    ) -> MieoResult<Vec<ObjectRef>> {
        let session = self.inner.sessions.get(session_id)?;
        let mut conn = introspection_conn(&session, &self.inner.sessions).await?;
        conn.driver().list_objects(scope, kind, pattern).await
    }

    pub async fn table_columns(
        &self,
        session_id: &str,
        scope: &Scope,
        table: &str,
    ) -> MieoResult<Vec<ColumnDef>> {
        let session = self.inner.sessions.get(session_id)?;
        let mut conn = introspection_conn(&session, &self.inner.sessions).await?;
        conn.driver().table_columns(scope, table).await
    }

    pub async fn table_indexes(
        &self,
        session_id: &str,
        scope: &Scope,
        table: &str,
    ) -> MieoResult<Vec<IndexDef>> {
        let session = self.inner.sessions.get(session_id)?;
        let mut conn = introspection_conn(&session, &self.inner.sessions).await?;
        conn.driver().table_indexes(scope, table).await
    }

    // ------------------------------------------------------------------
    // Kafka topic administration (produce + watermarks included)
    // ------------------------------------------------------------------

    pub async fn kafka_create_topic(
        &self,
        session_id: &str,
        topic: &str,
        partitions: i32,
        replication_factor: i16,
    ) -> MieoResult<()> {
        let session = self.inner.sessions.get(session_id)?;
        let mut conn = introspection_conn(&session, &self.inner.sessions).await?;
        match conn.driver() {
            AnyDriver::Kafka(driver) => {
                driver
                    .create_topic(topic, partitions, replication_factor)
                    .await
            }
            _ => Err(not_kafka_session()),
        }
    }

    pub async fn kafka_delete_topic(&self, session_id: &str, topic: &str) -> MieoResult<()> {
        let session = self.inner.sessions.get(session_id)?;
        let mut conn = introspection_conn(&session, &self.inner.sessions).await?;
        match conn.driver() {
            AnyDriver::Kafka(driver) => driver.delete_topic(topic).await,
            _ => Err(not_kafka_session()),
        }
    }

    /// `(partition, earliest offset, latest offset)` per partition of `topic`.
    pub async fn kafka_topic_partitions(
        &self,
        session_id: &str,
        topic: &str,
    ) -> MieoResult<Vec<(i32, i64, i64)>> {
        let session = self.inner.sessions.get(session_id)?;
        let mut conn = introspection_conn(&session, &self.inner.sessions).await?;
        match conn.driver() {
            AnyDriver::Kafka(driver) => driver.topic_watermarks(topic).await,
            _ => Err(not_kafka_session()),
        }
    }

    /// Produce one record; returns the (partition, broker offset) it landed on.
    pub async fn kafka_produce(
        &self,
        session_id: &str,
        topic: &str,
        partition: Option<i32>,
        key: Option<String>,
        value: String,
    ) -> MieoResult<(i32, i64)> {
        let session = self.inner.sessions.get(session_id)?;
        let mut conn = introspection_conn(&session, &self.inner.sessions).await?;
        match conn.driver() {
            AnyDriver::Kafka(driver) => {
                driver.produce_one(topic, partition, key, value).await
            }
            _ => Err(not_kafka_session()),
        }
    }

    pub async fn foreign_keys(
        &self,
        session_id: &str,
        scope: &Scope,
        table: &str,
    ) -> MieoResult<Vec<ForeignKeyDef>> {
        let session = self.inner.sessions.get(session_id)?;
        let mut conn = introspection_conn(&session, &self.inner.sessions).await?;
        conn.driver().foreign_keys(scope, table).await
    }

    pub async fn object_ddl(
        &self,
        session_id: &str,
        scope: &Scope,
        kind: ObjectKind,
        name: &str,
    ) -> MieoResult<Option<String>> {
        let session = self.inner.sessions.get(session_id)?;
        let mut conn = introspection_conn(&session, &self.inner.sessions).await?;
        conn.driver().object_ddl(scope, kind, name).await
    }

    // ------------------------------------------------------------------
    // Users / roles / grants (also drives the authorization UI).
    // ------------------------------------------------------------------

    pub async fn list_db_users(&self, session_id: &str) -> MieoResult<Vec<DbPrincipal>> {
        let session = self.inner.sessions.get(session_id)?;
        let mut guard = session.driver.lock().await;
        guard.list_db_users().await
    }

    pub async fn list_db_roles(&self, session_id: &str) -> MieoResult<Vec<DbPrincipal>> {
        let session = self.inner.sessions.get(session_id)?;
        let mut guard = session.driver.lock().await;
        guard.list_db_roles().await
    }

    pub async fn db_role_members(&self, session_id: &str) -> MieoResult<Vec<RoleGrant>> {
        let session = self.inner.sessions.get(session_id)?;
        let mut guard = session.driver.lock().await;
        guard.db_role_members().await
    }

    pub async fn db_user_grants(
        &self,
        session_id: &str,
        name: &str,
        host: Option<&str>,
    ) -> MieoResult<Vec<String>> {
        let session = self.inner.sessions.get(session_id)?;
        let mut guard = session.driver.lock().await;
        guard.db_user_grants(name, host).await
    }

    pub async fn create_db_user(
        &self,
        session_id: &str,
        req: &CreateDbUser,
    ) -> MieoResult<()> {
        let session = self.inner.sessions.get(session_id)?;
        let mut guard = session.driver.lock().await;
        guard.create_db_user(req).await
    }

    pub async fn drop_db_principal(
        &self,
        session_id: &str,
        req: &DropDbPrincipal,
    ) -> MieoResult<()> {
        let session = self.inner.sessions.get(session_id)?;
        let mut guard = session.driver.lock().await;
        guard.drop_db_principal(req).await
    }

    pub async fn grant(&self, session_id: &str, req: &GrantRequest) -> MieoResult<()> {
        let session = self.inner.sessions.get(session_id)?;
        let mut guard = session.driver.lock().await;
        guard.grant(req).await
    }

    pub async fn revoke(&self, session_id: &str, req: &RevokeRequest) -> MieoResult<()> {
        let session = self.inner.sessions.get(session_id)?;
        let mut guard = session.driver.lock().await;
        guard.revoke(req).await
    }

    pub async fn set_user_password(
        &self,
        session_id: &str,
        name: &str,
        host: Option<&str>,
        password: &str,
    ) -> MieoResult<()> {
        let session = self.inner.sessions.get(session_id)?;
        let mut guard = session.driver.lock().await;
        guard.set_user_password(name, host, password).await
    }

    pub async fn db_principal_detail(
        &self,
        session_id: &str,
        name: &str,
        host: Option<&str>,
    ) -> MieoResult<DbPrincipalDetail> {
        let session = self.inner.sessions.get(session_id)?;
        let mut guard = session.driver.lock().await;
        guard.db_principal_detail(name, host).await
    }

    pub async fn use_database(&self, session_id: &str, database: &str) -> MieoResult<()> {
        let session = self.inner.sessions.get(session_id)?;
        {
            let mut guard = session.driver.lock().await;
            guard.use_database(database).await?;
        }
        session.set_database(Some(database.to_string()));
        // Keep a lazily opened meta connection on the same catalog. If it
        // cannot follow the switch, drop it so the next introspection call
        // re-opens it lazily with the current database.
        let mut meta = session.meta.lock().await;
        if let Some(driver) = meta.as_mut() {
            if driver.use_database(database).await.is_err() {
                *meta = None;
            }
        }
        Ok(())
    }

    /// Engine-appropriate preview statement for the data grid (SQL engines are
    /// handled by the API layer; NoSQL engines provide their native syntax).
    pub async fn preview_statement(
        &self,
        session_id: &str,
        scope: &Scope,
        table: &str,
        limit: u64,
    ) -> MieoResult<String> {
        let session = self.inner.sessions.get(session_id)?;
        let mut conn = introspection_conn(&session, &self.inner.sessions).await?;
        conn.driver().preview_statement(scope, table, limit)
    }

    // ------------------------------------------------------------------
    // Transactions
    // ------------------------------------------------------------------

    pub async fn begin_transaction(&self, session_id: &str) -> MieoResult<()> {
        let session = self.inner.sessions.get(session_id)?;
        let mut guard = session.driver.lock().await;
        guard.begin().await
    }

    pub async fn commit_transaction(&self, session_id: &str) -> MieoResult<()> {
        let session = self.inner.sessions.get(session_id)?;
        let mut guard = session.driver.lock().await;
        guard.commit().await
    }

    pub async fn rollback_transaction(&self, session_id: &str) -> MieoResult<()> {
        let session = self.inner.sessions.get(session_id)?;
        let mut guard = session.driver.lock().await;
        guard.rollback().await
    }

    // ------------------------------------------------------------------
    // Query execution
    // ------------------------------------------------------------------

    /// Start a query and return its event stream immediately.
    /// Events are produced by a background task; cancel with `cancel_query`.
    pub async fn run_query(
        &self,
        session_id: &str,
        sql: String,
        params: Vec<Value>,
        options: QueryOptions,
    ) -> MieoResult<QueryStream> {
        let session = self.inner.sessions.get(session_id)?;
        let query_id = format!("q_{}", uuid::Uuid::now_v7().simple());
        let (tx, rx) = mpsc::channel::<QueryEvent>(64);
        let cancel = CancellationToken::new();
        self.inner
            .sessions
            .register_query(&query_id, session_id, cancel.clone());

        let inner = self.inner.clone();
        let qid = query_id.clone();
        let session_id_owned = session_id.to_string();
        let session_for_task = session.clone();
        let sql_for_history = sql.clone();

        tokio::spawn(async move {
            let started_at = Utc::now();
            let _ = tx
                .send(QueryEvent::Started {
                    query_id: qid.clone(),
                    at: started_at.to_rfc3339(),
                })
                .await;

            let result = {
                let mut guard = session_for_task.driver.lock().await;
                // The primary is idle here (we hold its lock): record which
                // server backend it is, so `cancel_query` can kill it for
                // real. Best-effort; never fails the user's query.
                refresh_backend_pid(&session_for_task, &mut guard).await;
                guard
                    .run_query(&qid, sql, params, &options, &tx, &cancel)
                    .await
            };
            let elapsed_ms = (Utc::now() - started_at).num_milliseconds().max(0) as u64;

            let database = session_for_task
                .database
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .clone();

            let (status, stats, error) = match result {
                Ok(stats) => {
                    let _ = tx
                        .send(QueryEvent::Finished {
                            query_id: qid.clone(),
                            stats: stats.clone(),
                        })
                        .await;
                    (HistoryStatus::Ok, stats, None)
                }
                Err(e) => {
                    let e = if cancel.is_cancelled() && e.code() != ErrorCode::Cancelled {
                        MieoError::cancelled()
                    } else {
                        e
                    };
                    let status = if e.code() == ErrorCode::Cancelled {
                        HistoryStatus::Cancelled
                    } else {
                        HistoryStatus::Error
                    };
                    let _ = tx
                        .send(QueryEvent::Error {
                            query_id: qid.clone(),
                            error: e.info.clone(),
                        })
                        .await;
                    (status, Default::default(), Some(e.info))
                }
            };

            inner.history.record(HistoryEntry {
                id: qid.clone(),
                session_id: session_id_owned,
                profile_id: session_for_task.profile_id.clone().map(|p| p.0),
                engine: session_for_task.engine,
                database,
                sql: sql_for_history,
                started_at,
                elapsed_ms,
                rows: stats.rows,
                rows_affected: stats.rows_affected,
                truncated: stats.truncated,
                status,
                error,
            });
            inner.sessions.finish_query(&qid);
        });

        Ok(QueryStream {
            query_id,
            events: rx,
        })
    }

    /// Cancel a running query by id. Returns `true` if it was found.
    ///
    /// Always delivers the soft (client-side) cancel first, then best-effort
    /// kills the server backend through the session's meta connection in the
    /// background — without the kill the server would keep working even
    /// though we stopped listening. Stays synchronous for the API layer; all
    /// kill failures are swallowed.
    pub fn cancel_query(&self, query_id: &str) -> bool {
        if !self.inner.sessions.cancel_query(query_id) {
            return false;
        }
        if let Some(session_id) = self.inner.sessions.session_for_query(query_id) {
            if let Ok(session) = self.inner.sessions.get(&session_id) {
                if tokio::runtime::Handle::try_current().is_ok() {
                    tokio::spawn(async move {
                        kill_session_backend(&session).await;
                    });
                }
            }
        }
        true
    }

    pub fn cancel_all_queries(&self) {
        self.inner.sessions.cancel_all();
    }

    // ------------------------------------------------------------------
    // History
    // ------------------------------------------------------------------

    pub fn list_history(&self, limit: usize) -> Vec<HistoryEntry> {
        self.inner.history.list(limit)
    }

    pub fn clear_history(&self) {
        self.inner.history.clear();
    }

    pub fn history_retention_days(&self) -> u32 {
        self.inner.history.retention_days()
    }

    pub fn set_history_retention_days(&self, days: u32) -> MieoResult<u32> {
        let days = settings::save_history_retention(&self.inner.data_dir, days)?;
        Ok(self.inner.history.set_retention_days(days))
    }

    /// Capabilities for a session's engine.
    pub async fn capabilities(&self, session_id: &str) -> MieoResult<Capabilities> {
        let session = self.inner.sessions.get(session_id)?;
        Ok(session.capabilities.clone())
    }
}

// ------------------------------------------------------------------
// Query cancellation + introspection helpers (QE-03/QE-04)
//
// A session owns two physical connections: the primary (`Session::driver`,
// serializes one statement at a time like one query tab) and a lazily opened
// side connection (`Session::meta`). Introspection (schema tree,
// autocomplete, server info) prefers the primary when it is idle — keeping
// exact tab semantics, including manual transactions — and routes around it
// through the side connection otherwise, so a long-running query never
// blocks the schema tree. Cancellation delivers the soft token first and
// then kills the server backend through the side connection.
// ------------------------------------------------------------------

/// Which physical connection serves one introspection call.
enum IntrospectionConn<'a> {
    /// Primary connection: idle and outside a manual transaction.
    Query(tokio::sync::MutexGuard<'a, AnyDriver>),
    /// Side connection: the primary is busy (or in a manual transaction).
    Meta(tokio::sync::MutexGuard<'a, Option<AnyDriver>>),
}

impl IntrospectionConn<'_> {
    fn driver(&mut self) -> &mut AnyDriver {
        match self {
            IntrospectionConn::Query(guard) => guard,
            IntrospectionConn::Meta(guard) => {
                guard.as_mut().expect("meta connection ensured before use")
            }
        }
    }
}

/// Error for Kafka-only operations issued against another engine's session.
fn not_kafka_session() -> MieoError {
    MieoError::new(
        ErrorCode::Unsupported,
        "this operation is only available for Kafka sessions".to_string(),
    )
}

/// Pick the connection for one introspection call (see `IntrospectionConn`).
///
/// Never waits on a running query: when the primary connection is busy, or a
/// manual transaction is open on it, the lazily opened meta connection serves
/// the call instead. (Introspection inside a manual transaction deliberately
/// bypasses the transaction so catalog reads neither see uncommitted DDL nor
/// get folded into the user's transaction.)
async fn introspection_conn<'a>(
    session: &'a Arc<Session>,
    sessions: &SessionManager,
) -> MieoResult<IntrospectionConn<'a>> {
    // Fast, non-blocking path: the primary is idle and outside a manual
    // transaction. `try_lock` (not `lock`) is what keeps a long query from
    // blocking the schema tree: on contention we fall through to the meta
    // connection below. A tiny race remains (a query may start between the
    // `has_running_query` check and `try_lock`), but then `try_lock` simply
    // fails and we take the meta path anyway.
    if !sessions.has_running_query(&session.id) {
        if let Ok(guard) = session.driver.try_lock() {
            if !guard.in_transaction() {
                return Ok(IntrospectionConn::Query(guard));
            }
        }
        // `try_lock` failed (a query just started) or a manual transaction is
        // open: fall through to the meta connection. Any acquired guard is
        // dropped here — the primary is never held across the awaits below.
    }
    // In-memory SQLite cannot use a side connection: every connection owns a
    // private database, so a second one would report an empty catalog. Wait
    // on the primary instead (correctness over latency).
    if is_memory_sqlite(session) {
        let guard = session.driver.lock().await;
        return Ok(IntrospectionConn::Query(guard));
    }
    ensure_meta_conn(session).await?;
    Ok(IntrospectionConn::Meta(session.meta.lock().await))
}

/// Lazily open (or reuse) the session's second physical connection.
///
/// The meta connection is created with the *currently selected* database, so
/// schema calls observe the same catalog as the primary connection.
async fn ensure_meta_conn(session: &Arc<Session>) -> MieoResult<()> {
    if session.meta.lock().await.is_some() {
        return Ok(());
    }
    let mut spec = session.spec.clone();
    spec.database = session
        .database
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    let driver = AnyDriver::connect(&spec).await?;
    let mut guard = session.meta.lock().await;
    if guard.is_none() {
        *guard = Some(driver);
    }
    // Else a concurrent call already opened one; drop the spare.
    Ok(())
}

/// Whether this session's catalog lives on a private in-memory database
/// (where a second connection would see an empty, different database).
fn is_memory_sqlite(session: &Session) -> bool {
    if session.engine != mieosql_core::Engine::Sqlite {
        return false;
    }
    let current = session
        .database
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    let path = current
        .as_deref()
        .or(session.spec.file.as_deref())
        .or(session.spec.database.as_deref())
        .unwrap_or("");
    path == ":memory:" || path.is_empty()
}

/// Scalar "who am I on the server" statement per engine, for backend-pid
/// capture before each query runs.
fn backend_id_sql(engine: mieosql_core::Engine) -> Option<&'static str> {
    match engine {
        mieosql_core::Engine::Postgres => Some("SELECT pg_backend_pid()"),
        mieosql_core::Engine::MySql | mieosql_core::Engine::MariaDb => {
            Some("SELECT CONNECTION_ID()")
        }
        mieosql_core::Engine::SqlServer => Some("SELECT @@SPID"),
        // SQLite exposes no backend id, and NoSQL engines need no
        // server-side kill: the client-side token is sufficient there.
        _ => None,
    }
}

/// Server-side kill statement for a previously captured backend id.
/// The id is validated numeric first — never interpolate unchecked text into
/// a kill statement.
fn kill_sql(engine: mieosql_core::Engine, backend_pid: &str) -> Option<String> {
    if backend_pid.trim().parse::<i64>().is_err() {
        return None;
    }
    let pid = backend_pid.trim();
    match engine {
        mieosql_core::Engine::Postgres => Some(format!("SELECT pg_cancel_backend({pid})")),
        mieosql_core::Engine::MySql | mieosql_core::Engine::MariaDb => {
            Some(format!("KILL QUERY {pid}"))
        }
        mieosql_core::Engine::SqlServer => Some(format!("KILL {pid}")),
        // SQLite: no kill primitive is available through sqlx (the native
        // `sqlite3_interrupt` handle is not exposed), so cancellation stays
        // client-side only. NoSQL engines likewise rely on the token.
        _ => None,
    }
}

/// Run `sql` (a single-row, single-column statement) on `driver` and return
/// the first cell as text. Used for backend-pid capture and for kill
/// statements, on either the primary or the meta connection.
///
/// Implemented on top of `AnyDriver::run_query` in `Simple` mode so no
/// driver changes are needed: events are drained from a throwaway channel.
/// The small channel never blocks the driver: at most a handful of rows
/// (`max_rows`) plus metadata events are produced.
async fn exec_scalar(driver: &mut AnyDriver, sql: &str) -> MieoResult<Option<String>> {
    let (tx, mut rx) = mpsc::channel::<QueryEvent>(64);
    let cancel = CancellationToken::new();
    let options = QueryOptions {
        mode: QueryMode::Simple,
        fetch_size: 8,
        max_rows: Some(8),
        timeout_ms: Some(10_000),
    };
    driver
        .run_query(
            "meta_scalar",
            sql.to_string(),
            Vec::new(),
            &options,
            &tx,
            &cancel,
        )
        .await?;
    drop(tx);
    let mut first: Option<String> = None;
    while let Some(event) = rx.recv().await {
        match event {
            QueryEvent::Batch { rows, .. } => {
                if first.is_none() {
                    first = rows
                        .first()
                        .and_then(|row| row.first())
                        .map(|value| match value {
                            Value::Int(i) => i.to_string(),
                            Value::UInt(u) => u.to_string(),
                            Value::Text(s)
                            | Value::Decimal(s)
                            | Value::Date(s)
                            | Value::Time(s)
                            | Value::DateTime(s)
                            | Value::Uuid(s) => s.clone(),
                            other => other.to_display_string(),
                        });
                }
            }
            QueryEvent::Error { error, .. } => {
                return Err(MieoError::new(error.code, error.message));
            }
            _ => {}
        }
    }
    Ok(first)
}

/// Capture the primary connection's backend id while it is idle. Called with
/// the primary lock held, right before the user's statement runs, so the id
/// is as fresh as possible. Best-effort: failures leave the previous value
/// and never fail the user's query.
///
/// Pooling caveat: sqlx-backed drivers acquire a pooled connection per
/// statement outside transactions, so this id may name a different physical
/// connection than the one that ends up running the query. In practice the
/// pool is idle (one statement at a time per session) and hands back the
/// same connection; inside a manual transaction the id is exact. A wrong id
/// only makes the server-side kill a harmless no-op on an idle backend in
/// the common case.
async fn refresh_backend_pid(session: &Session, driver: &mut AnyDriver) {
    let Some(sql) = backend_id_sql(session.engine) else {
        return;
    };
    if let Ok(Some(pid)) = exec_scalar(driver, sql).await {
        *session.backend_pid.lock().await = Some(pid.trim().to_string());
    }
}

/// Best-effort server-side kill for a cancelled query's backend, executed on
/// the session's meta connection (never the busy primary). Fire-and-forget
/// from `cancel_query`; all failures are swallowed — the soft token cancel
/// has already been delivered, and a kill that cannot run must not surface
/// as a second error.
async fn kill_session_backend(session: &Arc<Session>) {
    let pid = session.backend_pid.lock().await.clone();
    let Some(pid) = pid else { return };
    let Some(sql) = kill_sql(session.engine, &pid) else {
        return;
    };
    if is_memory_sqlite(session) {
        return;
    }
    if ensure_meta_conn(session).await.is_err() {
        return;
    }
    let mut guard = session.meta.lock().await;
    if let Some(driver) = guard.as_mut() {
        let _ = exec_scalar(driver, &sql).await;
    }
    // TODO: idle-close the meta connection (drop it after N seconds without
    // use) instead of holding a second physical connection per session.
}
