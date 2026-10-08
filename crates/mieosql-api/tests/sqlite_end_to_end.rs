//! End-to-end integration test over SQLite: profiles → vault → session →
//! DDL/DML → streaming → transactions → introspection → history.

use std::path::PathBuf;

use mieosql_api::{Api, ProfileInput};
use mieosql_core::{
    ErrorCode, ObjectKind, ProfileId, QueryEvent, QueryMode, QueryOptions, Scope, SslMode, Value,
};
use mieosql_engine::{Engine, EngineConfig, HistoryStatus};

struct TestEnv {
    api: Api,
    data_dir: PathBuf,
    db_path: PathBuf,
    profile_id: String,
}

impl Drop for TestEnv {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.data_dir);
        let _ = std::fs::remove_file(&self.db_path);
    }
}

async fn setup(tag: &str) -> TestEnv {
    let mut base = std::env::temp_dir();
    base.push(format!("mieosql-test-{tag}-{}", std::process::id()));
    let data_dir = base.join("data");
    std::fs::create_dir_all(&data_dir).unwrap();
    let db_path = base.join("test.db");
    std::fs::write(&db_path, b"").unwrap(); // empty file = valid empty SQLite db

    let engine = Engine::open(EngineConfig {
        data_dir: data_dir.clone(),
    })
    .unwrap();
    let api = Api::new(engine);

    let profile = api
        .save_profile(ProfileInput {
            id: None,
            name: "sqlite-test".into(),
            engine: mieosql_core::Engine::Sqlite,
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
            password: Some("vault-roundtrip".into()),
            ssh_password: None,
        })
        .unwrap();

    TestEnv {
        api,
        data_dir,
        db_path,
        profile_id: profile.id.0.clone(),
    }
}

async fn run_query(
    api: &Api,
    session_id: &str,
    sql: &str,
    params: Vec<Value>,
    options: QueryOptions,
) -> Vec<QueryEvent> {
    let mut stream = api
        .start_query(
            session_id.to_string(),
            sql.to_string(),
            params,
            Some(options),
        )
        .await
        .expect("query starts");
    let mut events = Vec::new();
    while let Some(event) = stream.events.recv().await {
        events.push(event);
    }
    events
}

fn collect_rows(events: &[QueryEvent]) -> Vec<Vec<Value>> {
    events
        .iter()
        .filter_map(|e| match e {
            QueryEvent::Batch { rows, .. } => Some(rows.clone()),
            _ => None,
        })
        .flatten()
        .collect()
}

fn stats(events: &[QueryEvent]) -> mieosql_core::QueryStats {
    events
        .iter()
        .find_map(|e| match e {
            QueryEvent::Finished { stats, .. } => Some(stats.clone()),
            _ => None,
        })
        .expect("finished event")
}

fn error_of(events: &[QueryEvent]) -> mieosql_core::ErrorInfo {
    events
        .iter()
        .find_map(|e| match e {
            QueryEvent::Error { error, .. } => Some(error.clone()),
            _ => None,
        })
        .expect("error event")
}

#[tokio::test]
async fn sqlite_full_flow() {
    let env = setup("full").await;
    let api = &env.api;

    // --- profile + vault round trip -------------------------------------
    let engine = api.engine();
    let spec = engine
        .resolve_spec(&ProfileId(env.profile_id.clone()))
        .unwrap();
    assert_eq!(spec.password.as_deref(), Some("vault-roundtrip"));

    // --- session ---------------------------------------------------------
    let session = api.open_session(env.profile_id.clone()).await.unwrap();
    assert_eq!(session.engine, mieosql_core::Engine::Sqlite);
    assert!(session.capabilities.transactions);
    let sid = session.id.clone();

    // --- DDL -------------------------------------------------------------
    let events = run_query(
        api,
        &sid,
        "CREATE TABLE users (\
             id INTEGER PRIMARY KEY AUTOINCREMENT, \
             name TEXT NOT NULL, \
             email TEXT, \
             score REAL, \
             active INTEGER DEFAULT 1\
         )",
        vec![],
        QueryOptions::default(),
    )
    .await;
    assert!(
        !events.iter().any(|e| matches!(e, QueryEvent::Error { .. })),
        "DDL failed: {events:?}"
    );

    // --- INSERT with bound parameters ------------------------------------
    for (name, email, score) in [
        ("Alice", Some("alice@example.com"), 9.5f64),
        ("Bob", None, 7.25),
        ("Carol", Some("carol@example.com"), 8.0),
    ] {
        let events = run_query(
            api,
            &sid,
            "INSERT INTO users (name, email, score) VALUES (?, ?, ?)",
            vec![
                Value::Text(name.into()),
                email.map(|e| Value::Text(e.into())).unwrap_or(Value::Null),
                Value::Float(score),
            ],
            QueryOptions::default(),
        )
        .await;
        let st = stats(&events);
        assert_eq!(st.rows_affected, 1);
        assert!(st.last_insert_id.is_some());
    }

    // --- SELECT streaming (batch size 2 over 3 rows) ----------------------
    let events = run_query(
        api,
        &sid,
        "SELECT id, name, email, score FROM users ORDER BY id",
        vec![],
        QueryOptions {
            mode: QueryMode::Prepared,
            fetch_size: 2,
            max_rows: None,
            timeout_ms: None,
        },
    )
    .await;

    let columns = events
        .iter()
        .find_map(|e| match e {
            QueryEvent::Columns { columns, .. } => Some(columns.clone()),
            _ => None,
        })
        .expect("columns event");
    assert_eq!(columns.len(), 4);
    assert_eq!(columns[1].name, "name");

    let batches = events
        .iter()
        .filter(|e| matches!(e, QueryEvent::Batch { .. }))
        .count();
    assert_eq!(batches, 2, "expected 2 streamed batches for fetch_size=2");

    let rows = collect_rows(&events);
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0][1], Value::Text("Alice".into()));
    assert_eq!(rows[1][2], Value::Null);
    assert_eq!(rows[2][3], Value::Float(8.0));
    assert_eq!(stats(&events).rows, 3);

    // --- max_rows truncation ---------------------------------------------
    let events = run_query(
        api,
        &sid,
        "SELECT id FROM users ORDER BY id",
        vec![],
        QueryOptions {
            max_rows: Some(2),
            ..Default::default()
        },
    )
    .await;
    let st = stats(&events);
    assert_eq!(st.rows, 2);
    assert!(st.truncated);

    // --- transactions ------------------------------------------------------
    api.begin_transaction(sid.clone()).await.unwrap();
    run_query(
        api,
        &sid,
        "INSERT INTO users (name) VALUES ('Temp')",
        vec![],
        QueryOptions::default(),
    )
    .await;
    api.rollback_transaction(sid.clone()).await.unwrap();

    let events = run_query(
        api,
        &sid,
        "SELECT COUNT(*) FROM users",
        vec![],
        QueryOptions::default(),
    )
    .await;
    assert_eq!(
        collect_rows(&events)[0][0],
        Value::Int(3),
        "rollback must undo the insert"
    );

    api.begin_transaction(sid.clone()).await.unwrap();
    run_query(
        api,
        &sid,
        "INSERT INTO users (name) VALUES ('Dana')",
        vec![],
        QueryOptions::default(),
    )
    .await;
    api.commit_transaction(sid.clone()).await.unwrap();

    let events = run_query(
        api,
        &sid,
        "SELECT COUNT(*) FROM users",
        vec![],
        QueryOptions::default(),
    )
    .await;
    assert_eq!(
        collect_rows(&events)[0][0],
        Value::Int(4),
        "commit must persist the insert"
    );

    // --- introspection ------------------------------------------------------
    let scope = Scope {
        database: Some("main".into()),
        schema: None,
    };
    let tables = api
        .list_objects(sid.clone(), scope.clone(), Some(ObjectKind::Table), None)
        .await
        .unwrap();
    assert!(tables.iter().any(|t| t.name == "users"));

    let columns = api
        .table_columns(sid.clone(), scope.clone(), "users".into())
        .await
        .unwrap();
    let id_col = columns.iter().find(|c| c.name == "id").unwrap();
    assert!(id_col.is_primary_key);
    let name_col = columns.iter().find(|c| c.name == "name").unwrap();
    assert!(!name_col.nullable);

    let indexes = api
        .table_indexes(sid.clone(), scope.clone(), "users".into())
        .await
        .unwrap();
    assert!(
        indexes.iter().any(|i| i.is_primary),
        "primary key index must be reported: {indexes:?}"
    );

    let ddl = api
        .object_ddl(
            sid.clone(),
            scope.clone(),
            ObjectKind::Table,
            "users".into(),
        )
        .await
        .unwrap()
        .expect("ddl available");
    assert!(ddl.to_uppercase().contains("CREATE TABLE"));

    // --- error mapping ------------------------------------------------------
    let events = run_query(
        api,
        &sid,
        "SELECT * FROM table_that_does_not_exist",
        vec![],
        QueryOptions::default(),
    )
    .await;
    let err = error_of(&events);
    assert!(
        matches!(err.code, ErrorCode::NotFound | ErrorCode::Syntax),
        "unexpected error code: {err:?}"
    );

    // --- history ------------------------------------------------------------
    let history = api.list_history(Some(100));
    assert!(history.len() >= 5);
    assert_eq!(history[0].status, HistoryStatus::Error);

    // --- persistence: engine re-open keeps profiles + secrets ---------------
    let _ = api.close_session(sid.clone()).await;
    drop(api.clone());
    let engine2 = Engine::open(EngineConfig {
        data_dir: env.data_dir.clone(),
    })
    .unwrap();
    let profiles = engine2.list_profiles();
    assert_eq!(profiles.len(), 1);
    let spec = engine2
        .resolve_spec(&ProfileId(env.profile_id.clone()))
        .unwrap();
    assert_eq!(spec.password.as_deref(), Some("vault-roundtrip"));
    assert!(env.db_path.exists());
}
