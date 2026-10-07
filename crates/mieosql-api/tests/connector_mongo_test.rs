//! MongoDB / AWS DocumentDB connector integration test.
//!
//! Ignored by default; requires a live server:
//!
//! ```bash
//! MIEOSQL_TEST_MONGO_URL='mongodb://user:pass@localhost:27017/testdb' \
//!     cargo test -p mieosql-api --test connector_mongo_test -- --ignored
//! ```
//!
//! `mongodb+srv://` URLs are not supported (the driver builds plain
//! `mongodb://` connection strings); use a direct host list instead. Query
//! parameters of the URL (`authSource`, `replicaSet`, `tls`, `tlsInsecure`,
//! `directConnection`, `appName`, `tlsCAFile`) are forwarded to the driver.

use std::collections::BTreeMap;
use std::path::PathBuf;

use mieosql_core::{
    ConnectionSpec, Engine as EngineName, EngineKind, ErrorCode, QueryEvent, QueryOptions, Scope,
    SslMode, Value,
};
use mieosql_engine::{Engine, EngineConfig};

struct TestEnv {
    engine: Engine,
    data_dir: PathBuf,
}

impl Drop for TestEnv {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.data_dir);
    }
}

/// Parse a `mongodb://` URL into the driver's `ConnectionSpec`.
fn spec_from_url(url: &str) -> ConnectionSpec {
    let rest = url.strip_prefix("mongodb://").unwrap_or_else(|| {
        panic!(
            "MIEOSQL_TEST_MONGO_URL must start with `mongodb://` (got `{url}`); \
             mongodb+srv is not supported by this driver"
        )
    });

    let (before_query, query) = match rest.split_once('?') {
        Some((before, query)) => (before, query),
        None => (rest, ""),
    };
    let (authority, database) = match before_query.split_once('/') {
        Some((authority, database)) => (authority, database),
        None => (before_query, ""),
    };
    let (userinfo, hosts) = match authority.rsplit_once('@') {
        Some((userinfo, hosts)) => (Some(userinfo), hosts),
        None => (None, authority),
    };
    let (username, password) = match userinfo {
        Some(info) => match info.split_once(':') {
            Some((user, password)) => (Some(user.to_string()), Some(password.to_string())),
            None => (Some(info.to_string()), None),
        },
        None => (None, None),
    };

    let mut params = BTreeMap::new();
    let mut ssl_mode = SslMode::Disable; // a plain mongodb:// URL means no TLS
    for pair in query.split('&').filter(|pair| !pair.is_empty()) {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        match key.to_ascii_lowercase().as_str() {
            "authsource" => {
                params.insert("auth_source".to_string(), value.to_string());
            }
            "replicaset" => {
                params.insert("replica_set".to_string(), value.to_string());
            }
            "tlsinsecure" => {
                params.insert("tls_insecure".to_string(), value.to_string());
            }
            "direct" | "directconnection" => {
                params.insert("direct".to_string(), value.to_string());
            }
            "appname" => {
                params.insert("app_name".to_string(), value.to_string());
            }
            "tlscafile" => {
                params.insert("tls_ca_file".to_string(), value.to_string());
            }
            "tls" | "ssl" => {
                if value.eq_ignore_ascii_case("true") {
                    ssl_mode = SslMode::VerifyFull;
                }
            }
            _ => {
                params.insert(key.to_string(), value.to_string());
            }
        }
    }

    ConnectionSpec {
        profile_id: None,
        name: "mongo-integration".to_string(),
        engine: EngineName::MongoDb,
        host: Some(hosts.to_string()),
        port: None,
        username,
        password,
        database: (!database.is_empty()).then(|| database.to_string()),
        file: None,
        params,
        ssl_mode,
        variant: None,
        ssh: None,
        ssh_password: None,
        connect_timeout_secs: 10,
    }
}

async fn run_query(
    engine: &Engine,
    session_id: &str,
    query: &str,
    options: QueryOptions,
) -> Vec<QueryEvent> {
    let mut stream = engine
        .run_query(session_id, query.to_string(), Vec::new(), options)
        .await
        .unwrap_or_else(|e| panic!("query `{query}` did not start: {e}"));
    let mut events = Vec::new();
    while let Some(event) = stream.events.recv().await {
        events.push(event);
    }
    events
}

fn collect_rows(events: &[QueryEvent]) -> Vec<Vec<Value>> {
    events
        .iter()
        .filter_map(|event| match event {
            QueryEvent::Batch { rows, .. } => Some(rows.clone()),
            _ => None,
        })
        .flatten()
        .collect()
}

fn columns_event(events: &[QueryEvent]) -> Vec<mieosql_core::ColumnMeta> {
    events
        .iter()
        .find_map(|event| match event {
            QueryEvent::Columns { columns, .. } => Some(columns.clone()),
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

fn stats(events: &[QueryEvent]) -> mieosql_core::QueryStats {
    events
        .iter()
        .find_map(|event| match event {
            QueryEvent::Finished { stats, .. } => Some(stats.clone()),
            _ => None,
        })
        .expect("finished event")
}

#[tokio::test]
#[ignore = "requires a live MongoDB server via MIEOSQL_TEST_MONGO_URL"]
async fn mongo_connector_end_to_end() {
    let url = std::env::var("MIEOSQL_TEST_MONGO_URL").expect(
        "set MIEOSQL_TEST_MONGO_URL=mongodb://user:pass@host:port/db (this test is #[ignore])",
    );
    let spec = spec_from_url(&url);

    let mut data_dir = std::env::temp_dir();
    data_dir.push(format!("mieosql-mongo-it-{}", std::process::id()));
    std::fs::create_dir_all(&data_dir).expect("create data dir");
    let env = TestEnv {
        engine: Engine::open(EngineConfig {
            data_dir: data_dir.clone(),
        })
        .expect("open engine"),
        data_dir,
    };
    let engine = &env.engine;

    // --- connect -----------------------------------------------------------
    let session = engine
        .open_session(&spec)
        .await
        .expect("connect to MongoDB");
    assert_eq!(session.engine, EngineName::MongoDb);
    assert_eq!(session.capabilities.engine_kind, EngineKind::Document);
    assert!(!session.capabilities.transactions);

    let database = spec.database.clone().unwrap_or_else(|| "admin".to_string());

    // --- server info -------------------------------------------------------
    let info = engine.server_info(&session.id).await.expect("server info");
    assert!(!info.version.is_empty(), "buildInfo version must be present");
    assert_eq!(info.current_database.as_deref(), Some(database.as_str()));

    // --- read-only queries on a random (missing) collection -----------------
    let collection = format!("mieosql_it_{}", std::process::id());

    let events = run_query(
        engine,
        &session.id,
        &format!("db.{collection}.countDocuments({{}})"),
        QueryOptions::default(),
    )
    .await;
    assert!(error_of(&events).is_none(), "count failed: {events:?}");
    let rows = collect_rows(&events);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][0], Value::Int(0));

    let events = run_query(
        engine,
        &session.id,
        &format!("db.{collection}.find({{}}).limit(5)"),
        QueryOptions::default(),
    )
    .await;
    assert!(error_of(&events).is_none(), "find failed: {events:?}");
    assert_eq!(stats(&events).rows, 0);

    let events = run_query(
        engine,
        &session.id,
        &format!("db.{collection}.aggregate([{{\"$match\": {{}}}}, {{\"$limit\": 3}}])"),
        QueryOptions::default(),
    )
    .await;
    assert!(error_of(&events).is_none(), "aggregate failed: {events:?}");

    // --- JSON query form ----------------------------------------------------
    let json_form = format!("{{\"collection\": \"{collection}\", \"find\": {{}}, \"limit\": 2}}");
    let events = run_query(engine, &session.id, &json_form, QueryOptions::default()).await;
    assert!(error_of(&events).is_none(), "JSON form failed: {events:?}");

    // --- structured syntax errors -------------------------------------------
    let events = run_query(
        engine,
        &session.id,
        "db.users.find({\"a\": 1",
        QueryOptions::default(),
    )
    .await;
    let error = error_of(&events).expect("syntax error event");
    assert_eq!(error.code, ErrorCode::Syntax);
    assert!(error.hint.is_some());

    // --- databases ----------------------------------------------------------
    let databases = engine
        .list_databases(&session.id)
        .await
        .expect("list databases");
    assert!(!databases.is_empty(), "expected at least one database");

    // --- collections (and views) --------------------------------------------
    let scope = Scope::db(database.clone());
    let objects = engine
        .list_objects(&session.id, &scope, None, None)
        .await
        .expect("list collections");

    // When the database has data, exercise inference / indexes / preview.
    if let Some(first) = objects.first() {
        let columns = engine
            .table_columns(&session.id, &scope, &first.name)
            .await
            .expect("table columns");
        assert!(
            !columns.is_empty(),
            "field inference found no columns in `{}`",
            first.name
        );

        let indexes = engine
            .table_indexes(&session.id, &scope, &first.name)
            .await
            .expect("table indexes");
        assert!(
            indexes.iter().any(|index| index.is_primary),
            "expected the `_id_` index: {indexes:?}"
        );

        let preview = engine
            .preview_statement(&session.id, &scope, &first.name, 10)
            .await
            .expect("preview statement");
        assert!(preview.contains(".find({})"), "unexpected preview: {preview}");

        let events = run_query(engine, &session.id, &preview, QueryOptions::default()).await;
        assert!(error_of(&events).is_none(), "preview failed: {events:?}");
        let columns = columns_event(&events);
        for row in collect_rows(&events) {
            assert_eq!(row.len(), columns.len(), "row/column alignment");
        }
    }

    // --- use_database switches the default ----------------------------------
    engine
        .use_database(&session.id, "admin")
        .await
        .expect("use_database");
    let info = engine
        .server_info(&session.id)
        .await
        .expect("server info after switch");
    assert_eq!(info.current_database.as_deref(), Some("admin"));

    let _ = engine.close_session(&session.id).await;
}

