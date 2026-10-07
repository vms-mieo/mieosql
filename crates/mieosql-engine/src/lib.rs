//! MieoSQL engine: the stateful service layer between drivers and the API.
//!
//! Responsibilities:
//! - connection profiles (metadata) + encrypted vault (secrets)
//! - sessions (open connections) and query cancellation
//! - query execution with streaming events and history

mod history;
mod session;
mod store;
mod vault;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::Utc;
use mieosql_core::{
    Capabilities, ColumnDef, ConnectionProfile, ConnectionSpec, DatabaseRef, ErrorCode,
    ForeignKeyDef, IndexDef, MieoError, MieoResult, ObjectKind, ObjectRef, ProfileId, QueryEvent,
    QueryOptions, SchemaRef, Scope, ServerInfo, Value,
};
use mieosql_drivers::AnyDriver;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

pub use history::{HistoryEntry, HistoryStatus, HistoryStore};
pub use session::{QueryStream, Session, SessionInfo, SessionManager};
pub use store::ProfileStore;
pub use vault::Vault;

/// Runtime configuration for the engine.
#[derive(Debug, Clone)]
pub struct EngineConfig {
    /// Directory holding profiles.json, secrets.json, master.key, history.jsonl.
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
        let history = HistoryStore::load(config.data_dir.join("history.jsonl"));
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
        let mut guard = session.driver.lock().await;
        guard.server_info().await
    }

    pub async fn list_databases(&self, session_id: &str) -> MieoResult<Vec<DatabaseRef>> {
        let session = self.inner.sessions.get(session_id)?;
        let mut guard = session.driver.lock().await;
        guard.list_databases().await
    }

    /// List schemas; engines where schema == database fall back to databases.
    pub async fn list_schemas(
        &self,
        session_id: &str,
        database: Option<&str>,
    ) -> MieoResult<Vec<SchemaRef>> {
        let session = self.inner.sessions.get(session_id)?;
        let mut guard = session.driver.lock().await;
        if guard.capabilities().schema_is_database {
            let dbs = guard.list_databases().await?;
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
        guard.list_schemas(database).await
    }

    pub async fn list_objects(
        &self,
        session_id: &str,
        scope: &Scope,
        kind: Option<ObjectKind>,
        pattern: Option<&str>,
    ) -> MieoResult<Vec<ObjectRef>> {
        let session = self.inner.sessions.get(session_id)?;
        let mut guard = session.driver.lock().await;
        guard.list_objects(scope, kind, pattern).await
    }

    pub async fn table_columns(
        &self,
        session_id: &str,
        scope: &Scope,
        table: &str,
    ) -> MieoResult<Vec<ColumnDef>> {
        let session = self.inner.sessions.get(session_id)?;
        let mut guard = session.driver.lock().await;
        guard.table_columns(scope, table).await
    }

    pub async fn table_indexes(
        &self,
        session_id: &str,
        scope: &Scope,
        table: &str,
    ) -> MieoResult<Vec<IndexDef>> {
        let session = self.inner.sessions.get(session_id)?;
        let mut guard = session.driver.lock().await;
        guard.table_indexes(scope, table).await
    }

    pub async fn foreign_keys(
        &self,
        session_id: &str,
        scope: &Scope,
        table: &str,
    ) -> MieoResult<Vec<ForeignKeyDef>> {
        let session = self.inner.sessions.get(session_id)?;
        let mut guard = session.driver.lock().await;
        guard.foreign_keys(scope, table).await
    }

    pub async fn object_ddl(
        &self,
        session_id: &str,
        scope: &Scope,
        kind: ObjectKind,
        name: &str,
    ) -> MieoResult<Option<String>> {
        let session = self.inner.sessions.get(session_id)?;
        let mut guard = session.driver.lock().await;
        guard.object_ddl(scope, kind, name).await
    }

    pub async fn use_database(&self, session_id: &str, database: &str) -> MieoResult<()> {
        let session = self.inner.sessions.get(session_id)?;
        {
            let mut guard = session.driver.lock().await;
            guard.use_database(database).await?;
        }
        session.set_database(Some(database.to_string()));
        Ok(())
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
            .register_query(&query_id, cancel.clone());

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
    pub fn cancel_query(&self, query_id: &str) -> bool {
        self.inner.sessions.cancel_query(query_id)
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

    /// Capabilities for a session's engine.
    pub async fn capabilities(&self, session_id: &str) -> MieoResult<Capabilities> {
        let session = self.inner.sessions.get(session_id)?;
        Ok(session.capabilities.clone())
    }
}
