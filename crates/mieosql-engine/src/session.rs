//! Sessions: long-lived connections addressed by id, plus query cancellation.

use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex, RwLock};

use chrono::{DateTime, Utc};
use mieosql_core::{
    Capabilities, ConnectionSpec, Engine, ErrorCode, MieoError, MieoResult, ProfileId, QueryEvent,
};
use mieosql_drivers::AnyDriver;
use tokio::sync::{Mutex, mpsc};
use tokio_util::sync::CancellationToken;
use ts_rs::TS;

/// A single open connection.
pub struct Session {
    pub id: String,
    pub profile_id: Option<ProfileId>,
    pub name: String,
    pub engine: Engine,
    pub capabilities: Capabilities,
    pub opened_at: DateTime<Utc>,
    /// Database currently selected on this session (kept in sync with the driver).
    pub database: RwLock<Option<String>>,
    /// The driver is behind a mutex: statements on one session are serialized,
    /// exactly like one query tab on one connection in a classic client.
    pub driver: Mutex<AnyDriver>,
    /// Resolved connection parameters (including secrets), kept so a second
    /// physical connection can be opened lazily for introspection and for
    /// server-side query cancellation. Never leaves the backend.
    pub spec: ConnectionSpec,
    /// Lazily opened second connection. Serves schema introspection while the
    /// primary connection is busy running a query (or inside a manual
    /// transaction), and carries the `KILL` / `pg_cancel_backend` statement on
    /// cancel. `None` until first needed, then kept for the session lifetime.
    pub meta: Mutex<Option<AnyDriver>>,
    /// Backend process id of the primary connection, as last observed
    /// (`pg_backend_pid()` / `CONNECTION_ID()` / `@@SPID`, stored as text).
    /// Refreshed on the idle primary right before each query runs; used for
    /// the server-side kill in `cancel_query`. Best-effort only (see
    /// `refresh_backend_pid` in `lib.rs` for the pooling caveat).
    pub backend_pid: Mutex<Option<String>>,
}

/// Serializable session description handed to the frontend.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, TS)]
#[ts(export)]
pub struct SessionInfo {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
    pub name: String,
    pub engine: Engine,
    pub capabilities: Capabilities,
    pub opened_at: DateTime<Utc>,
    pub in_transaction: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub database: Option<String>,
}

impl Session {
    pub async fn info(&self) -> SessionInfo {
        // `try_lock` so that listing sessions never blocks behind a running
        // query: when busy we report "not in transaction" (transiently wrong
        // at worst) instead of hanging the sidebar.
        let in_transaction = self
            .driver
            .try_lock()
            .map(|guard| guard.in_transaction())
            .unwrap_or(false);
        SessionInfo {
            id: self.id.clone(),
            profile_id: self.profile_id.clone().map(|p| p.0),
            name: self.name.clone(),
            engine: self.engine,
            capabilities: self.capabilities.clone(),
            opened_at: self.opened_at,
            in_transaction,
            database: self
                .database
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .clone(),
        }
    }

    pub fn set_database(&self, database: Option<String>) {
        *self.database.write().unwrap_or_else(|e| e.into_inner()) = database;
    }
}

/// The stream returned to callers of `Engine::run_query`.
pub struct QueryStream {
    pub query_id: String,
    pub events: mpsc::Receiver<QueryEvent>,
}

/// Registry of sessions and in-flight queries.
#[derive(Default)]
pub struct SessionManager {
    sessions: RwLock<HashMap<String, Arc<Session>>>,
    queries: StdMutex<HashMap<String, CancellationToken>>,
    /// Which session each in-flight query belongs to. Maintained alongside
    /// `queries`: needed to route introspection around a busy primary
    /// connection, and to find the backend pid on cancel.
    query_session: StdMutex<HashMap<String, String>>,
}

impl SessionManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&self, session: Arc<Session>) {
        self.sessions
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(session.id.clone(), session);
    }

    pub fn get(&self, id: &str) -> MieoResult<Arc<Session>> {
        self.sessions
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(id)
            .cloned()
            .ok_or_else(|| MieoError::new(ErrorCode::NotFound, format!("session `{id}` not found")))
    }

    pub fn remove(&self, id: &str) -> Option<Arc<Session>> {
        self.sessions
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .remove(id)
    }

    pub fn list(&self) -> Vec<Arc<Session>> {
        self.sessions
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .cloned()
            .collect()
    }

    pub fn register_query(&self, query_id: &str, session_id: &str, token: CancellationToken) {
        self.queries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(query_id.to_string(), token);
        self.query_session
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(query_id.to_string(), session_id.to_string());
    }

    pub fn finish_query(&self, query_id: &str) {
        self.queries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(query_id);
        self.query_session
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(query_id);
    }

    /// Session an in-flight query belongs to, if it is still running.
    pub fn session_for_query(&self, query_id: &str) -> Option<String> {
        self.query_session
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(query_id)
            .cloned()
    }

    /// Whether `session_id` currently has at least one running query.
    /// Non-blocking: used to route introspection without waiting on the
    /// primary connection.
    pub fn has_running_query(&self, session_id: &str) -> bool {
        self.query_session
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .any(|s| s == session_id)
    }

    /// Cancel a running query. Returns `true` when the query was found.
    pub fn cancel_query(&self, query_id: &str) -> bool {
        let queries = self.queries.lock().unwrap_or_else(|e| e.into_inner());
        match queries.get(query_id) {
            Some(token) => {
                token.cancel();
                true
            }
            None => false,
        }
    }

    /// Cancel every in-flight query (used when closing the app).
    pub fn cancel_all(&self) {
        let queries = self.queries.lock().unwrap_or_else(|e| e.into_inner());
        for token in queries.values() {
            token.cancel();
        }
    }
}
