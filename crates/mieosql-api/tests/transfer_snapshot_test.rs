//! Integration tests for data transfer + schema snapshot over SQLite.
//!
//! Covers B5: `Api::transfer_table` cross-session copy (create/append,
//! truncate, limit, column subset, drop/create) and `Api::schema_snapshot`
//! (tables, views, FKs, truncation).

use std::path::PathBuf;

use mieosql_api::sync::SyncEndpoint;
use mieosql_api::transfer::{TransferMode, TransferRequest};
use mieosql_api::{Api, ProfileInput};
use mieosql_core::{Engine, ErrorCode, ObjectKind, QueryEvent, QueryOptions, Scope, SslMode, Value};
use mieosql_engine::{Engine as CoreEngine, EngineConfig};

struct TestEnv {
    api: Api,
    base: PathBuf,
}

impl Drop for TestEnv {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

fn unique_base(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let mut base = std::env::temp_dir();
    base.push(format!("mieosql-b5-{tag}-{}-{nanos}", std::process::id()));
    base
}

async fn add_sqlite_profile(api: &Api, name: &str, file: &std::path::Path) -> String {
    std::fs::write(file, b"").unwrap(); // empty file = valid empty SQLite db
    let profile = api
        .save_profile(ProfileInput {
            id: None,
            name: name.into(),
            engine: Engine::Sqlite,
            host: None,
            port: None,
            username: None,
            database: None,
            file: Some(file.to_string_lossy().into_owned()),
            params: Default::default(),
            ssl_mode: Some(SslMode::Disable),
            ssh: None,
            color: None,
            group: None,
            password: None,
            ssh_password: None,
        })
        .unwrap();
    api.open_session(profile.id.0.clone())
        .await
        .unwrap()
        .id
}

/// Two sessions over two separate SQLite files.
async fn setup(tag: &str) -> (TestEnv, String, String) {
    let base = unique_base(tag);
    std::fs::create_dir_all(&base).unwrap();
    let engine = CoreEngine::open(EngineConfig {
        data_dir: base.join("data"),
    })
    .unwrap();
    let api = Api::new(engine);
    let source = add_sqlite_profile(&api, "src", &base.join("src.db")).await;
    let target = add_sqlite_profile(&api, "dst", &base.join("dst.db")).await;
    (TestEnv { api, base }, source, target)
}

async fn exec(api: &Api, session_id: &str, sql: &str) {
    let mut stream = api
        .start_query(
            session_id.to_string(),
            sql.to_string(),
            vec![],
            Some(QueryOptions::default()),
        )
        .await
        .expect("query starts");
    let mut error = None;
    while let Some(event) = stream.events.recv().await {
        if let QueryEvent::Error { error: e, .. } = event {
            error = Some(e);
        }
    }
    assert!(error.is_none(), "statement failed: {error:?}\nsql: {sql}");
}

async fn scalar(api: &Api, session_id: &str, sql: &str) -> Value {
    let out = api
        .fetch_rows(session_id.to_string(), sql.to_string(), Some(10))
        .await
        .unwrap();
    out.rows
        .first()
        .and_then(|row| row.first().cloned())
        .unwrap_or(Value::Null)
}

fn request(source: &str, target: &str, mode: TransferMode) -> TransferRequest {
    TransferRequest {
        source: SyncEndpoint {
            session_id: source.to_string(),
            scope: Scope::default(),
        },
        target: SyncEndpoint {
            session_id: target.to_string(),
            scope: Scope::default(),
        },
        source_table: "items".into(),
        target_table: None,
        mode,
        columns: vec![],
        where_clause: None,
        batch_size: Some(2),
        limit: None,
    }
}

#[tokio::test]
async fn transfer_sqlite_create_append_limit_and_subset() {
    let (env, src, dst) = setup("transfer").await;
    let api = &env.api;

    exec(
        api,
        &src,
        "CREATE TABLE items (\
             id INTEGER PRIMARY KEY AUTOINCREMENT, \
             name TEXT NOT NULL, \
             qty INTEGER, \
             price REAL\
         )",
    )
    .await;
    exec(
        api,
        &src,
        "INSERT INTO items (name, qty, price) VALUES \
             ('a', 1, 1.5), ('b', 2, 2.5), ('c', 3, 3.5), ('d', 4, 4.5), ('e', 5, 5.5)",
    )
    .await;

    // --- CreateAppend -----------------------------------------------------
    let result = api
        .transfer_table(request(&src, &dst, TransferMode::CreateAppend))
        .await
        .unwrap();
    assert_eq!(result.rows, 5);
    assert!(result.created);
    assert!(!result.truncated);
    assert_eq!(result.target_table, "items");
    assert!(result.warnings.is_empty(), "unexpected: {:?}", result.warnings);
    assert_eq!(scalar(api, &dst, "SELECT COUNT(*) FROM items").await, Value::Int(5));
    assert_eq!(
        scalar(api, &dst, "SELECT name FROM items WHERE qty = 3").await,
        Value::Text("c".into())
    );

    // --- TruncateAppend + limit ------------------------------------------
    let mut req = request(&src, &dst, TransferMode::TruncateAppend);
    req.limit = Some(3);
    let result = api.transfer_table(req).await.unwrap();
    assert_eq!(result.rows, 3);
    assert!(!result.created);
    assert!(result.truncated, "source has 5 rows, limit 3 must truncate");
    assert_eq!(scalar(api, &dst, "SELECT COUNT(*) FROM items").await, Value::Int(3));

    // --- column subset (case-insensitive) + WHERE + Append ----------------
    let mut req = request(&src, &dst, TransferMode::Append);
    req.columns = vec!["NAME".into()];
    req.where_clause = Some("qty >= 4".into());
    let result = api.transfer_table(req).await.unwrap();
    assert_eq!(result.rows, 2, "rows d and e match qty >= 4");
    assert!(!result.truncated);
    assert_eq!(scalar(api, &dst, "SELECT COUNT(*) FROM items").await, Value::Int(5));
    assert_eq!(
        scalar(
            api,
            &dst,
            "SELECT COUNT(*) FROM items WHERE name IN ('d', 'e') AND qty IS NULL",
        )
        .await,
        Value::Int(2),
        "the subset insert must leave unselected columns NULL"
    );

    // --- DropCreate under a new target name (same-engine DDL retarget) ----
    let mut req = request(&src, &dst, TransferMode::DropCreate);
    req.target_table = Some("items_copy".into());
    let result = api.transfer_table(req).await.unwrap();
    assert!(result.created);
    assert_eq!(result.target_table, "items_copy");
    assert_eq!(result.rows, 5);
    assert_eq!(
        scalar(api, &dst, "SELECT COUNT(*) FROM items_copy").await,
        Value::Int(5)
    );
}

#[tokio::test]
async fn transfer_sqlite_errors_are_structured() {
    let (env, src, dst) = setup("transfer-errors").await;
    let api = &env.api;

    exec(api, &src, "CREATE TABLE items (id INTEGER PRIMARY KEY, name TEXT)").await;
    exec(api, &src, "INSERT INTO items (id, name) VALUES (1, 'a')").await;

    // Unknown source column -> NotFound with hint.
    let mut req = request(&src, &dst, TransferMode::CreateAppend);
    req.columns = vec!["nope".into()];
    let err = api.transfer_table(req).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);
    assert!(err.hint.is_some());

    // Append to a missing target -> NotFound.
    let err = api
        .transfer_table(request(&src, &dst, TransferMode::Append))
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);

    // Unknown source table -> NotFound.
    let mut req = request(&src, &dst, TransferMode::CreateAppend);
    req.source_table = "ghost".into();
    let err = api.transfer_table(req).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::NotFound);

    // Same session for both sides would deadlock -> unsupported.
    let mut req = request(&src, &dst, TransferMode::CreateAppend);
    req.target = req.source.clone();
    let err = api.transfer_table(req).await.unwrap_err();
    assert_eq!(err.code, ErrorCode::Unsupported);
}

#[tokio::test]
async fn snapshot_sqlite_tables_fks_and_views() {
    let (env, src, _dst) = setup("snapshot").await;
    let api = &env.api;

    exec(
        api,
        &src,
        "CREATE TABLE authors (id INTEGER PRIMARY KEY, name TEXT NOT NULL)",
    )
    .await;
    exec(
        api,
        &src,
        "CREATE TABLE books (\
             id INTEGER PRIMARY KEY, \
             author_id INTEGER REFERENCES authors(id), \
             title TEXT\
         )",
    )
    .await;
    exec(
        api,
        &src,
        "CREATE VIEW book_titles AS SELECT title FROM books",
    )
    .await;

    // --- full snapshot incl. views ---------------------------------------
    let snapshot = api
        .schema_snapshot(src.clone(), Scope::default(), true, None)
        .await
        .unwrap();
    assert_eq!(snapshot.engine, Engine::Sqlite);
    assert!(!snapshot.truncated);
    assert!(!snapshot.generated_at.is_empty());

    let authors = snapshot
        .tables
        .iter()
        .find(|t| t.name == "authors")
        .expect("authors table");
    assert_eq!(authors.kind, ObjectKind::Table);
    assert!(authors
        .columns
        .iter()
        .any(|c| c.name == "id" && c.is_primary_key));
    assert!(authors.indexes.iter().any(|i| i.is_primary));

    let books = snapshot
        .tables
        .iter()
        .find(|t| t.name == "books")
        .expect("books table");
    assert_eq!(books.foreign_keys.len(), 1);
    assert_eq!(books.foreign_keys[0].ref_table, "authors");
    assert_eq!(books.foreign_keys[0].columns, vec!["author_id".to_string()]);

    let view = snapshot
        .tables
        .iter()
        .find(|t| t.name == "book_titles")
        .expect("view");
    assert_eq!(view.kind, ObjectKind::View);
    assert!(view.columns.iter().any(|c| c.name == "title"));

    // --- views excluded ---------------------------------------------------
    let snapshot = api
        .schema_snapshot(src.clone(), Scope::default(), false, None)
        .await
        .unwrap();
    assert!(snapshot.tables.iter().all(|t| t.kind == ObjectKind::Table));
    assert!(snapshot.tables.iter().all(|t| t.name != "book_titles"));

    // --- max_tables clamps + truncation flag ------------------------------
    let snapshot = api
        .schema_snapshot(src.clone(), Scope::default(), true, Some(1))
        .await
        .unwrap();
    assert_eq!(snapshot.tables.len(), 1);
    assert!(snapshot.truncated);

    let snapshot = api
        .schema_snapshot(src.clone(), Scope::default(), true, Some(0))
        .await
        .unwrap();
    assert_eq!(snapshot.tables.len(), 1, "max_tables clamps to at least 1");
    assert!(snapshot.truncated);
}
