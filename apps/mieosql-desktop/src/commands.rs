//! Tauri commands — thin adapters over [`mieosql_api::Api`].
//!
//! Query results stream to the frontend through the `mieosql://query` event:
//! every payload is `{ queryId, event }` where `event` is a `QueryEvent`.

use tauri::{AppHandle, Emitter, State};

use mieosql_api::{ApiError, ExportFormat, ExportStats, PreviewStart, ProfileInput};
use mieosql_core::{
    Capabilities, ColumnDef, ConnectionProfile, DatabaseRef, ForeignKeyDef, IndexDef, ObjectKind,
    ObjectRef, QueryOptions, SchemaRef, Scope, ServerInfo, Value,
};
use mieosql_engine::{HistoryEntry, SessionInfo};

use crate::AppState;

pub const QUERY_EVENT: &str = "mieosql://query";

#[tauri::command]
pub fn api_version() -> String {
    mieosql_api::API_VERSION.to_string()
}

// ---------------------------------------------------------------------------
// Profiles
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn list_profiles(state: State<'_, AppState>) -> Vec<ConnectionProfile> {
    state.api.list_profiles()
}

#[tauri::command]
pub fn get_profile(state: State<'_, AppState>, id: String) -> Result<ConnectionProfile, ApiError> {
    state.api.get_profile(id)
}

#[tauri::command]
pub fn save_profile(
    state: State<'_, AppState>,
    input: ProfileInput,
) -> Result<ConnectionProfile, ApiError> {
    state.api.save_profile(input)
}

#[tauri::command]
pub fn delete_profile(state: State<'_, AppState>, id: String) -> Result<(), ApiError> {
    state.api.delete_profile(id)
}

#[tauri::command]
pub async fn test_profile(
    state: State<'_, AppState>,
    id: String,
    password: Option<String>,
) -> Result<ServerInfo, ApiError> {
    state.api.test_profile(id, password).await
}

#[tauri::command]
pub async fn test_connection(
    state: State<'_, AppState>,
    input: ProfileInput,
) -> Result<ServerInfo, ApiError> {
    state.api.test_connection(input).await
}

// ---------------------------------------------------------------------------
// Sessions
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn open_session(
    state: State<'_, AppState>,
    profile_id: String,
) -> Result<SessionInfo, ApiError> {
    state.api.open_session(profile_id).await
}

#[tauri::command]
pub async fn close_session(state: State<'_, AppState>, session_id: String) -> Result<(), ApiError> {
    state.api.close_session(session_id).await
}

#[tauri::command]
pub async fn list_sessions(state: State<'_, AppState>) -> Result<Vec<SessionInfo>, ApiError> {
    Ok(state.api.list_sessions().await)
}

#[tauri::command]
pub async fn session_info(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<SessionInfo, ApiError> {
    state.api.session_info(session_id).await
}

#[tauri::command]
pub async fn capabilities(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<Capabilities, ApiError> {
    state.api.capabilities(session_id).await
}

#[tauri::command]
pub async fn server_info(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<ServerInfo, ApiError> {
    state.api.server_info(session_id).await
}

// ---------------------------------------------------------------------------
// Introspection
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn list_databases(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<Vec<DatabaseRef>, ApiError> {
    state.api.list_databases(session_id).await
}

#[tauri::command]
pub async fn list_schemas(
    state: State<'_, AppState>,
    session_id: String,
    database: Option<String>,
) -> Result<Vec<SchemaRef>, ApiError> {
    state.api.list_schemas(session_id, database).await
}

#[tauri::command]
pub async fn list_objects(
    state: State<'_, AppState>,
    session_id: String,
    scope: Scope,
    kind: Option<ObjectKind>,
    pattern: Option<String>,
) -> Result<Vec<ObjectRef>, ApiError> {
    state
        .api
        .list_objects(session_id, scope, kind, pattern)
        .await
}

#[tauri::command]
pub async fn table_columns(
    state: State<'_, AppState>,
    session_id: String,
    scope: Scope,
    table: String,
) -> Result<Vec<ColumnDef>, ApiError> {
    state.api.table_columns(session_id, scope, table).await
}

#[tauri::command]
pub async fn table_indexes(
    state: State<'_, AppState>,
    session_id: String,
    scope: Scope,
    table: String,
) -> Result<Vec<IndexDef>, ApiError> {
    state.api.table_indexes(session_id, scope, table).await
}

#[tauri::command]
pub async fn foreign_keys(
    state: State<'_, AppState>,
    session_id: String,
    scope: Scope,
    table: String,
) -> Result<Vec<ForeignKeyDef>, ApiError> {
    state.api.foreign_keys(session_id, scope, table).await
}

#[tauri::command]
pub async fn object_ddl(
    state: State<'_, AppState>,
    session_id: String,
    scope: Scope,
    kind: ObjectKind,
    name: String,
) -> Result<Option<String>, ApiError> {
    state.api.object_ddl(session_id, scope, kind, name).await
}

#[tauri::command]
pub async fn use_database(
    state: State<'_, AppState>,
    session_id: String,
    database: String,
) -> Result<(), ApiError> {
    state.api.use_database(session_id, database).await
}

// ---------------------------------------------------------------------------
// Transactions
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn begin_transaction(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<(), ApiError> {
    state.api.begin_transaction(session_id).await
}

#[tauri::command]
pub async fn commit_transaction(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<(), ApiError> {
    state.api.commit_transaction(session_id).await
}

#[tauri::command]
pub async fn rollback_transaction(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<(), ApiError> {
    state.api.rollback_transaction(session_id).await
}

// ---------------------------------------------------------------------------
// Queries
// ---------------------------------------------------------------------------

/// Spawn the task that forwards `QueryStream` events to the frontend on
/// `mieosql://query` as `{ queryId, event }`. Returns the query id.
fn spawn_query_pump(app: AppHandle, mut stream: mieosql_engine::QueryStream) -> String {
    let query_id = stream.query_id.clone();
    let emit_id = query_id.clone();
    tauri::async_runtime::spawn(async move {
        while let Some(event) = stream.events.recv().await {
            let payload = serde_json::json!({
                "queryId": emit_id,
                "event": event,
            });
            if app.emit(QUERY_EVENT, payload).is_err() {
                // Window closed — stop pumping.
                break;
            }
        }
    });
    query_id
}

/// Start a query; returns its id immediately.
#[tauri::command]
pub async fn start_query(
    app: AppHandle,
    state: State<'_, AppState>,
    session_id: String,
    sql: String,
    params: Vec<Value>,
    options: Option<QueryOptions>,
) -> Result<String, ApiError> {
    let stream = state
        .api
        .start_query(session_id, sql, params, options)
        .await?;
    Ok(spawn_query_pump(app, stream))
}

/// Open a table as a data grid: generates `SELECT * FROM <table> LIMIT n`
/// with engine-correct identifier quoting, then streams the result.
#[tauri::command]
pub async fn preview_table(
    app: AppHandle,
    state: State<'_, AppState>,
    session_id: String,
    scope: Scope,
    table: String,
    limit: Option<u64>,
) -> Result<PreviewStart, ApiError> {
    let (stream, sql) = state
        .api
        .preview_table(session_id, scope, table, limit)
        .await?;
    let query_id = spawn_query_pump(app, stream);
    Ok(PreviewStart { query_id, sql })
}

/// Re-run a result's SQL server-side and write it straight to `path`
/// (CSV / TSV / JSON / JSONL / SQL inserts / XLSX). The row data never crosses
/// the IPC bridge, so exports are as large as the query allows.
#[tauri::command]
pub async fn export_result(
    state: State<'_, AppState>,
    session_id: String,
    sql: String,
    format: ExportFormat,
    path: String,
    table: Option<String>,
) -> Result<ExportStats, ApiError> {
    state
        .api
        .export_result(session_id, sql, format, path, table)
        .await
}

#[tauri::command]
pub fn cancel_query(state: State<'_, AppState>, query_id: String) -> bool {
    state.api.cancel_query(query_id)
}

// ---------------------------------------------------------------------------
// History
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn list_history(state: State<'_, AppState>, limit: Option<usize>) -> Vec<HistoryEntry> {
    state.api.list_history(limit)
}

#[tauri::command]
pub fn clear_history(state: State<'_, AppState>) {
    state.api.clear_history()
}
