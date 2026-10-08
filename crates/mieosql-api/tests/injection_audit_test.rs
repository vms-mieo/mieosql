//! Injection audit (QA-08) through the public API on SQLite: every SQL
//! fragment built from user input must quote identifiers instead of
//! interpolating them. No server needed.

use std::path::PathBuf;

use mieosql_api::service::{build_select_sql, quote_ident};
use mieosql_api::sync::SyncEndpoint;
use mieosql_api::transfer::{TransferMode, TransferRequest};
use mieosql_api::{Api, ProfileInput};
use mieosql_core::{
    Engine as DbEngine, ErrorCode, QueryEvent, QueryOptions, Scope, SslMode, Value,
};
use mieosql_engine::{Engine, EngineConfig};

struct Env {
    api: Api,
    base: PathBuf,
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

async fn setup(tag: &str) -> (Env, String, String) {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let mut base = std::env::temp_dir();
    base.push(format!("mieosql-inject-{tag}-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(base.join("data")).unwrap();
    let db_path = base.join("test.db");
    std::fs::write(&db_path, b"").unwrap();
    let engine = Engine::open(EngineConfig {
        data_dir: base.join("data"),
    })
    .unwrap();
    let api = Api::new(engine);
    let profile = api
        .save_profile(ProfileInput {
            id: None,
            name: "inject".into(),
            engine: DbEngine::Sqlite,
            host: None,
            port: None,
            username: None,
            database: None,
            file: Some(db_path.to_string_lossy().into_owned()),
            params: Default::default(),
            ssl_mode: Some(SslMode::Disable),
            ssh: None,
            color: None,
            group: None,
            variant: None,
            is_prod: false,
            password: None,
            ssh_password: None,
        })
        .unwrap();
    let pid = profile.id.0.clone();
    let sid = api.open_session(pid.clone()).await.unwrap().id;
    (Env { api, base }, pid, sid)
}

async fn exec(api: &Api, sid: &str, sql: &str) {
    let mut stream = api
        .start_query(sid.to_string(), sql.to_string(), Vec::new(), Some(QueryOptions::default()))
        .await
        .unwrap_or_else(|e| panic!("query start failed for `{sql}`: {e}"));
    while let Some(event) = stream.events.recv().await {
        if let QueryEvent::Error { error, .. } = event {
            panic!("statement failed `{sql}`: {error:?}");
        }
    }
}

async fn count(api: &Api, sid: &str, table_quoted: &str) -> Value {
    api.fetch_rows(sid.to_string(), format!("SELECT COUNT(*) FROM {table_quoted}"), Some(10))
        .await
        .unwrap()
        .rows[0][0]
        .clone()
}

#[test]
fn quote_ident_escapes_hostile_names() {
    // SQLite/Postgres style: wrap in double quotes, double embedded quotes.
    assert_eq!(quote_ident(DbEngine::Sqlite, "a\"b"), "\"a\"\"b\"");
    assert_eq!(quote_ident(DbEngine::Postgres, "a\"b"), "\"a\"\"b\"");
    // Payloads with brackets, backticks or comment markers stay one identifier.
    assert_eq!(quote_ident(DbEngine::Sqlite, "x]; DROP TABLE t;--"), "\"x]; DROP TABLE t;--\"");
    assert_eq!(quote_ident(DbEngine::Sqlite, "a`b"), "\"a`b\"");
    assert_eq!(quote_ident(DbEngine::Sqlite, "with space"), "\"with space\"");
    // Other engines escape their own quote character.
    assert_eq!(quote_ident(DbEngine::SqlServer, "a]b"), "[a]]b]");
    assert_eq!(quote_ident(DbEngine::MySql, "a`b"), "`a``b`");

    let scope = Scope::default();
    let sql = build_select_sql(DbEngine::Sqlite, &scope, "users; DROP TABLE users;--", 10);
    assert_eq!(sql, "SELECT * FROM \"users; DROP TABLE users;--\" LIMIT 10");
}

#[tokio::test]
async fn preview_table_quotes_weird_table_names() {
    let (env, _pid, sid) = setup("weird").await;
    let api = &env.api;
    exec(api, &sid, "CREATE TABLE \"we\"\"ird;name\" (id INTEGER PRIMARY KEY, v TEXT)").await;
    exec(api, &sid, "INSERT INTO \"we\"\"ird;name\" (id, v) VALUES (1, 'x')").await;

    let (mut stream, sql) = api
        .preview_table(sid.clone(), Scope::default(), "we\"ird;name".into(), Some(10))
        .await
        .unwrap();
    assert_eq!(sql, "SELECT * FROM \"we\"\"ird;name\" LIMIT 10");
    let mut rows = Vec::new();
    while let Some(event) = stream.events.recv().await {
        match event {
            QueryEvent::Batch { rows: b, .. } => rows.extend(b),
            QueryEvent::Error { error, .. } => panic!("preview failed: {error:?}"),
            QueryEvent::Finished { .. } => break,
            _ => {}
        }
    }
    assert_eq!(rows, vec![vec![Value::Int(1), Value::Text("x".into())]]);
}

#[tokio::test]
async fn injection_in_preview_table_name_fails_without_executing() {
    let (env, _pid, sid) = setup("preview-inject").await;
    let api = &env.api;
    exec(api, &sid, "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT)").await;
    exec(api, &sid, "INSERT INTO users (id, name) VALUES (1, 'alice')").await;

    let hostile = "users; DROP TABLE users;--";
    match api.preview_table(sid.clone(), Scope::default(), hostile.into(), Some(10)).await {
        Err(e) => assert!(matches!(e.code, ErrorCode::NotFound | ErrorCode::Syntax)),
        Ok((mut stream, sql)) => {
            // The payload must stay inside one quoted identifier.
            assert!(sql.contains("\"users; DROP TABLE users;--\""), "sql: {sql}");
            let mut saw_error = false;
            while let Some(event) = stream.events.recv().await {
                match event {
                    QueryEvent::Error { .. } => { saw_error = true; break; }
                    QueryEvent::Finished { .. } => break,
                    _ => {}
                }
            }
            assert!(saw_error, "quoted hostile name must not resolve to a table");
        }
    }
    // The DROP never ran: the table and its row survive.
    assert_eq!(count(api, &sid, "\"users\"").await, Value::Int(1));
}

#[tokio::test]
async fn transfer_rejects_hostile_column_without_executing() {
    let (env, pid, src) = setup("transfer-inject").await;
    let api = &env.api;
    let dst = api.open_session(pid).await.unwrap().id;
    exec(api, &src, "CREATE TABLE items (id INTEGER PRIMARY KEY, name TEXT)").await;
    exec(api, &src, "INSERT INTO items (id, name) VALUES (1, 'a')").await;
    exec(api, &dst, "CREATE TABLE items_copy (id INTEGER PRIMARY KEY, name TEXT)").await;

    let err = api
        .transfer_table(TransferRequest {
            source: SyncEndpoint { session_id: src.clone(), scope: Scope::default() },
            target: SyncEndpoint { session_id: dst.clone(), scope: Scope::default() },
            source_table: "items".into(),
            target_table: Some("items_copy".into()),
            mode: TransferMode::Append,
            columns: vec!["id\"; DROP TABLE items;--".into()],
            where_clause: None,
            batch_size: None,
            limit: None,
        })
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
    // Nothing was copied and the source is intact.
    assert_eq!(count(api, &src, "\"items\"").await, Value::Int(1));
    assert_eq!(count(api, &dst, "\"items_copy\"").await, Value::Int(0));
}

#[tokio::test]
async fn hostile_table_name_in_introspection_returns_error() {
    let (env, _pid, sid) = setup("introspect").await;
    let api = &env.api;
    exec(api, &sid, "CREATE TABLE items (id INTEGER PRIMARY KEY)").await;

    // A hostile name is treated as one (nonexistent) identifier: the lookup
    // is parameterized, so it matches nothing and executes nothing.
    let cols = api
        .table_columns(sid.clone(), Scope::default(), "items\"; DROP TABLE items;--".into())
        .await
        .unwrap();
    assert!(cols.is_empty(), "hostile name must match no table");
    // The table survives the hostile lookup.
    assert_eq!(count(api, &sid, "\"items\"").await, Value::Int(0));
}
