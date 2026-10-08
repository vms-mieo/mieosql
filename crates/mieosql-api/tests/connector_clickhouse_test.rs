//! ClickHouse connector integration test.
//!
//! Disabled by default: it needs a real ClickHouse server and is marked
//! `#[ignore]`, so the regular `cargo test` suite never requires ClickHouse.
//! Run it with:
//!
//! ```text
//! MIEOSQL_TEST_CLICKHOUSE_URL=localhost:8123 \
//!   cargo test -p mieosql-api --test connector_clickhouse_test -- --ignored --nocapture
//! ```
//!
//! `MIEOSQL_TEST_CLICKHOUSE_URL` accepts `host`, `host:port` or a full
//! `http(s)://host:port` URL. Optional credentials:
//! `MIEOSQL_TEST_CLICKHOUSE_USER` (default `default`) and
//! `MIEOSQL_TEST_CLICKHOUSE_PASSWORD` (default empty). TLS is only exercised
//! when the URL uses `https://` (SslMode::Require).

use std::path::PathBuf;

use mieosql_api::{Api, ProfileInput};
use mieosql_core::{
    Engine as EngineId, ErrorCode, ObjectKind, QueryEvent, QueryMode, QueryOptions, QueryStats,
    Scope, SslMode, Value,
};
use mieosql_engine::{Engine, EngineConfig};

struct TestEnv {
    api: Api,
    data_dir: PathBuf,
}

impl Drop for TestEnv {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.data_dir);
    }
}

struct Endpoint {
    host: String,
    port: Option<u16>,
    ssl_mode: SslMode,
}

fn endpoint_from_env() -> Option<Endpoint> {
    let raw = std::env::var("MIEOSQL_TEST_CLICKHOUSE_URL").ok()?;
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }

    let (rest, ssl_mode) = if let Some(rest) = raw.strip_prefix("https://") {
        (rest, SslMode::Require)
    } else if let Some(rest) = raw.strip_prefix("http://") {
        (rest, SslMode::Disable)
    } else {
        (raw, SslMode::Disable)
    };
    let rest = rest.trim_end_matches('/');
    let (host, port) = match rest.rsplit_once(':') {
        Some((host, port)) if !host.is_empty() => {
            (host.to_string(), port.parse::<u16>().ok())
        }
        _ => (rest.to_string(), None),
    };
    Some(Endpoint {
        host,
        port,
        ssl_mode,
    })
}

async fn run(api: &Api, session_id: &str, sql: &str, params: Vec<Value>, max_rows: Option<u64>) -> Vec<QueryEvent> {
    let mut stream = api
        .start_query(
            session_id.to_string(),
            sql.to_string(),
            params,
            Some(QueryOptions {
                mode: QueryMode::Simple,
                fetch_size: 64,
                max_rows,
                timeout_ms: Some(20_000),
            }),
        )
        .await
        .expect("query starts");
    let mut events = Vec::new();
    while let Some(event) = stream.events.recv().await {
        events.push(event);
    }
    events
}

fn rows(events: &[QueryEvent]) -> Vec<Vec<Value>> {
    events
        .iter()
        .filter_map(|event| match event {
            QueryEvent::Batch { rows, .. } => Some(rows.clone()),
            _ => None,
        })
        .flatten()
        .collect()
}

fn columns(events: &[QueryEvent]) -> Vec<String> {
    events
        .iter()
        .find_map(|event| match event {
            QueryEvent::Columns { columns, .. } => {
                Some(columns.iter().map(|c| c.name.clone()).collect())
            }
            _ => None,
        })
        .unwrap_or_default()
}

fn error_of(events: &[QueryEvent]) -> Option<mieosql_core::ErrorInfo> {
    events.iter().find_map(|event| match event {
        QueryEvent::Error { error, .. } => Some(error.clone()),
        _ => None,
    })
}

fn stats(events: &[QueryEvent]) -> QueryStats {
    events
        .iter()
        .find_map(|event| match event {
            QueryEvent::Finished { stats, .. } => Some(stats.clone()),
            _ => None,
        })
        .expect("finished event")
}

#[tokio::test]
#[ignore = "requires MIEOSQL_TEST_CLICKHOUSE_URL (host[:port] or http(s)://host:port)"]
async fn clickhouse_browse_query_roundtrip() {
    let Some(endpoint) = endpoint_from_env() else {
        eprintln!("MIEOSQL_TEST_CLICKHOUSE_URL not set; skipping ClickHouse integration test");
        return;
    };

    let mut base = std::env::temp_dir();
    base.push(format!("mieosql-test-clickhouse-{}", std::process::id()));
    let data_dir = base.join("data");
    std::fs::create_dir_all(&data_dir).expect("create test data dir");
    let engine = Engine::open(EngineConfig {
        data_dir: data_dir.clone(),
    })
    .expect("open engine");
    let api = Api::new(engine);
    let env = TestEnv { api, data_dir };

    let username = std::env::var("MIEOSQL_TEST_CLICKHOUSE_USER")
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .or_else(|| Some("default".to_string()));
    let password = std::env::var("MIEOSQL_TEST_CLICKHOUSE_PASSWORD")
        .ok()
        .filter(|v| !v.is_empty());

    let profile = env
        .api
        .save_profile(ProfileInput {
            id: None,
            name: "clickhouse-integration".to_string(),
            engine: EngineId::ClickHouse,
            host: Some(endpoint.host),
            port: endpoint.port,
            username,
            database: Some("default".to_string()),
            file: None,
            params: Default::default(),
            ssl_mode: Some(endpoint.ssl_mode),
            ssh: None,
            color: None,
            group: None,
            variant: None,
            password,
            ssh_password: None,
        })
        .expect("save profile");

    let session = env
        .api
        .open_session(profile.id.0.clone(), None)
        .await
        .expect("open ClickHouse session");

    // ---- server identity / capabilities ----------------------------------
    let info = env
        .api
        .server_info(session.id.clone())
        .await
        .expect("server info");
    assert_eq!(info.engine, EngineId::ClickHouse);
    assert!(
        !info.version.is_empty(),
        "ClickHouse version should be reported"
    );

    let capabilities = env
        .api
        .capabilities(session.id.clone())
        .await
        .expect("capabilities");
    assert!(capabilities.list_databases);
    assert!(capabilities.schema_is_database);
    assert!(!capabilities.transactions);
    assert!(!capabilities.user_management);

    // ---- databases --------------------------------------------------------
    let databases = env
        .api
        .list_databases(session.id.clone())
        .await
        .expect("list databases");
    assert!(
        databases.iter().any(|db| db.name == "default"),
        "the `default` database should exist"
    );
    assert!(
        databases.iter().any(|db| db.name == "system" && db.is_system),
        "the `system` database should be flagged as system"
    );

    // ---- fixture table ----------------------------------------------------
    let table = format!("mieosql_it_{}", std::process::id());
    let drop_sql = format!("DROP TABLE IF EXISTS default.{table}");
    assert!(error_of(&run(&env.api, &session.id, &drop_sql, vec![], None).await).is_none());
    let create_sql = format!(
        "CREATE TABLE default.{table} (\
           id Int64, \
           name String, \
           score Decimal(10,2), \
           tags Array(String), \
           note Nullable(String), \
           at DateTime64(3)\
         ) ENGINE = MergeTree ORDER BY id"
    );
    assert!(
        error_of(&run(&env.api, &session.id, &create_sql, vec![], None).await).is_none(),
        "CREATE TABLE should succeed"
    );
    let insert_sql = format!(
        "INSERT INTO default.{table} (id, name, score, tags, note, at) VALUES \
         (1, 'alpha', 12.50, ['a','b'], 'first', '2024-01-02 03:04:05.000'), \
         (2, 'beta', 99.99, [], NULL, '2024-02-03 04:05:06.000'), \
         (3, 'gamma', -1.25, ['x'], NULL, '2024-03-04 05:06:07.000')"
    );
    assert!(error_of(&run(&env.api, &session.id, &insert_sql, vec![], None).await).is_none());

    // ---- catalog browsing -------------------------------------------------
    let objects = env
        .api
        .list_objects(
            session.id.clone(),
            Scope::db("default"),
            Some(ObjectKind::Table),
            Some(table.clone()),
        )
        .await
        .expect("list objects");
    assert_eq!(objects.len(), 1, "fixture table should be listed");
    assert_eq!(objects[0].name, table);
    assert_eq!(objects[0].kind, ObjectKind::Table);

    let cols = env
        .api
        .table_columns(session.id.clone(), Scope::db("default"), table.clone())
        .await
        .expect("table columns");
    let by_name = |name: &str| cols.iter().find(|c| c.name == name).expect(name);
    assert_eq!(by_name("id").native_type, "Int64");
    assert!(by_name("note").nullable);
    assert!(!by_name("name").nullable);

    let ddl = env
        .api
        .object_ddl(
            session.id.clone(),
            Scope::db("default"),
            ObjectKind::Table,
            table.clone(),
        )
        .await
        .expect("object ddl");
    assert!(
        ddl.as_deref()
            .is_some_and(|ddl| ddl.contains("CREATE TABLE")),
        "DDL should contain CREATE TABLE: {ddl:?}"
    );

    // ---- query rows -------------------------------------------------------
    let select_sql = format!(
        "SELECT id, name, score, tags, note, at FROM default.{table} ORDER BY id"
    );
    let events = run(&env.api, &session.id, &select_sql, vec![], None).await;
    assert!(
        error_of(&events).is_none(),
        "select failed: {:?}",
        error_of(&events)
    );
    assert_eq!(
        columns(&events),
        vec!["id", "name", "score", "tags", "note", "at"]
    );
    let got = rows(&events);
    assert_eq!(got.len(), 3);
    assert!(matches!(&got[0][0], Value::Int(1)));
    assert!(matches!(&got[0][1], Value::Text(name) if name == "alpha"));
    assert!(matches!(&got[0][2], Value::Decimal(score) if score == "12.5"));
    assert!(matches!(&got[0][3], Value::Array(items) if items.len() == 2));
    assert!(matches!(&got[1][4], Value::Null));

    // ---- bound parameters -------------------------------------------------
    let param_sql = format!("SELECT name FROM default.{table} WHERE id = ?");
    let events = run(&env.api, &session.id, &param_sql, vec![Value::Int(2)], None).await;
    assert!(
        error_of(&events).is_none(),
        "parameterized query failed: {:?}",
        error_of(&events)
    );
    let got = rows(&events);
    assert_eq!(got.len(), 1);
    assert!(matches!(&got[0][0], Value::Text(name) if name == "beta"));

    // ---- max_rows truncation ---------------------------------------------
    let events = run(&env.api, &session.id, &select_sql, vec![], Some(2)).await;
    assert!(error_of(&events).is_none());
    assert_eq!(rows(&events).len(), 2);
    assert!(stats(&events).truncated, "stats should report truncation");

    // ---- error mapping ----------------------------------------------------
    let events = run(
        &env.api,
        &session.id,
        "SELECT * FROM default.table_that_does_not_exist_xyz",
        vec![],
        None,
    )
    .await;
    let err = error_of(&events).expect("unknown table should error");
    assert_eq!(err.code, ErrorCode::NotFound, "got {err:?}");

    // ---- cleanup ----------------------------------------------------------
    assert!(error_of(&run(&env.api, &session.id, &drop_sql, vec![], None).await).is_none());

    // ---- unreachable server fails at connect time -------------------------
    let bad = env
        .api
        .save_profile(ProfileInput {
            id: None,
            name: "clickhouse-unreachable".to_string(),
            engine: EngineId::ClickHouse,
            host: Some("127.0.0.1".to_string()),
            port: Some(1),
            username: Some("default".to_string()),
            database: Some("default".to_string()),
            file: None,
            params: Default::default(),
            ssl_mode: Some(SslMode::Disable),
            ssh: None,
            color: None,
            group: None,
            variant: None,
            password: None,
            ssh_password: None,
        })
        .expect("save unreachable profile");
    let result = env.api.open_session(bad.id.0.clone(), None).await;
    assert!(
        result.is_err(),
        "connecting to a closed port must fail instead of reporting a healthy session"
    );
}
