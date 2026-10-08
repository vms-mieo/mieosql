//! SQL Server connector integration test.
//!
//! Ignored by default (no server is required to keep CI green). Run against a
//! real instance with:
//!
//! ```bash
//! MIEOSQL_TEST_MSSQL_URL='sqlserver://sa:YourStrong%40Passw0rd@localhost:1433/master?ssl=require' \
//!   cargo test -p mieosql-api --test connector_sqlserver_test -- --ignored --nocapture
//! ```
//!
//! URL shape: `sqlserver://user:password@host:port/database?ssl=<mode>`.
//! The password must be percent-encoded (`@` -> `%40`, `:` -> `%3A`, ...).
//! `ssl` accepts `disable`, `prefer`, `require` (default), `verify_ca`,
//! `verify_full`. When the variable is unset the test prints a skip notice and
//! returns without failing.

use mieosql_api::{Api, ProfileInput};
use mieosql_core::{
    DataTypeKind, Engine as CoreEngine, ErrorCode, ObjectKind, QueryEvent, QueryMode, QueryOptions,
    QueryStats, Scope, SslMode, Value,
};
use mieosql_engine::{Engine, EngineConfig};

struct MssqlUrl {
    host: String,
    port: u16,
    username: String,
    password: String,
    database: String,
    ssl_mode: SslMode,
}

fn parse_url(raw: &str) -> MssqlUrl {
    let rest = raw
        .strip_prefix("sqlserver://")
        .or_else(|| raw.strip_prefix("mssql://"))
        .unwrap_or_else(|| {
            panic!("MIEOSQL_TEST_MSSQL_URL must start with `sqlserver://` or `mssql://`")
        });

    let (main, query) = match rest.split_once('?') {
        Some((main, query)) => (main, query),
        None => (rest, ""),
    };
    let (hostport, dbpath) = match main.split_once('/') {
        Some((hostport, dbpath)) => (hostport, dbpath),
        None => (main, ""),
    };
    let (userinfo, hostport) = match hostport.rsplit_once('@') {
        Some((userinfo, hostport)) => (Some(userinfo), hostport),
        None => (None, hostport),
    };
    let (username, password) = match userinfo {
        Some(userinfo) => {
            let (user, password) = userinfo.split_once(':').unwrap_or((userinfo, ""));
            (percent_decode(user), percent_decode(password))
        }
        None => (String::new(), String::new()),
    };
    let (host, port) = match hostport.split_once(':') {
        Some((host, port)) => (
            percent_decode(host),
            port.parse::<u16>().expect("invalid port in URL"),
        ),
        None => (percent_decode(hostport), 1433),
    };
    let database = if dbpath.is_empty() {
        "master".to_string()
    } else {
        percent_decode(dbpath)
    };

    let mut ssl_mode = SslMode::Require;
    for pair in query.split('&').filter(|p| !p.is_empty()) {
        if let Some((key, value)) = pair.split_once('=')
            && (key.eq_ignore_ascii_case("ssl") || key.eq_ignore_ascii_case("encrypt"))
        {
            ssl_mode = match value.to_ascii_lowercase().as_str() {
                "disable" | "disabled" | "false" | "no" | "notsupported" => SslMode::Disable,
                "prefer" | "preferred" => SslMode::Prefer,
                "require" | "required" | "true" | "yes" => SslMode::Require,
                "verify_ca" | "verify-ca" => SslMode::VerifyCa,
                "verify_full" | "verify-full" | "strict" => SslMode::VerifyFull,
                other => panic!("unsupported ssl mode `{other}`"),
            };
        }
    }

    MssqlUrl {
        host,
        port,
        username,
        password,
        database,
        ssl_mode,
    }
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let (Some(high), Some(low)) = (hex(bytes[index + 1]), hex(bytes[index + 2])) {
                out.push((high << 4) | low);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
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
        .filter_map(|event| match event {
            QueryEvent::Batch { rows, .. } => Some(rows.clone()),
            _ => None,
        })
        .flatten()
        .collect()
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

fn error_of(events: &[QueryEvent]) -> mieosql_core::ErrorInfo {
    events
        .iter()
        .find_map(|event| match event {
            QueryEvent::Error { error, .. } => Some(error.clone()),
            _ => None,
        })
        .expect("error event")
}

fn has_error(events: &[QueryEvent]) -> bool {
    events
        .iter()
        .any(|event| matches!(event, QueryEvent::Error { .. }))
}

#[tokio::test]
#[ignore = "requires MIEOSQL_TEST_MSSQL_URL (a reachable SQL Server instance)"]
async fn sqlserver_full_flow() {
    let Ok(raw_url) = std::env::var("MIEOSQL_TEST_MSSQL_URL") else {
        eprintln!("MIEOSQL_TEST_MSSQL_URL is not set — skipping");
        return;
    };
    let url = parse_url(&raw_url);

    // Isolated engine state (profiles + vault) for this run.
    let mut base = std::env::temp_dir();
    base.push(format!("mieosql-mssql-test-{}", std::process::id()));
    let data_dir = base.join("data");
    std::fs::create_dir_all(&data_dir).unwrap();
    let engine = Engine::open(EngineConfig {
        data_dir: data_dir.clone(),
    })
    .unwrap();
    let api = Api::new(engine);

    let profile = api
        .save_profile(ProfileInput {
            id: None,
            name: "mssql-test".into(),
            engine: CoreEngine::SqlServer,
            host: Some(url.host.clone()),
            port: Some(url.port),
            username: Some(url.username.clone()),
            database: Some(url.database.clone()),
            file: None,
            params: Default::default(),
            ssl_mode: Some(url.ssl_mode),
            ssh: None,
            color: None,
            group: None,
            variant: None,
            is_prod: false,
            password: Some(url.password.clone()),
            ssh_password: None,
        })
        .unwrap();

    let session = api.open_session(profile.id.0.clone()).await.unwrap();
    assert_eq!(session.engine, CoreEngine::SqlServer);
    assert!(session.capabilities.transactions);
    let sid = session.id.clone();

    // --- server info / databases -----------------------------------------
    let info = api.server_info(sid.clone()).await.unwrap();
    assert_eq!(info.engine, CoreEngine::SqlServer);
    assert!(!info.version.is_empty(), "product version must be reported");

    let databases = api.list_databases(sid.clone()).await.unwrap();
    assert!(
        databases.iter().any(|db| db.name == url.database),
        "connected database must be listed: {databases:?}"
    );

    // --- ping + simple / prepared SELECT ---------------------------------
    let events = run_query(&api, &sid, "SELECT 1 AS one", vec![], QueryOptions::default()).await;
    assert_eq!(collect_rows(&events)[0][0], Value::Int(1));

    let events = run_query(
        &api,
        &sid,
        "SELECT 1 AS one",
        vec![],
        QueryOptions {
            mode: QueryMode::Simple,
            ..Default::default()
        },
    )
    .await;
    assert_eq!(collect_rows(&events)[0][0], Value::Int(1));

    // --- DDL + parameterized DML -----------------------------------------
    let table = format!("mieosql_connector_{}", std::process::id());
    let ddl = format!(
        "CREATE TABLE dbo.[{table}] (\
             id INT IDENTITY(1,1) PRIMARY KEY, \
             name NVARCHAR(100) NOT NULL, \
             amount DECIMAL(10,2) NULL, \
             flag BIT NULL, \
             created DATETIME2 NULL, \
             tags NVARCHAR(MAX) NULL\
         )"
    );
    let events = run_query(
        &api,
        &sid,
        &ddl,
        vec![],
        QueryOptions {
            mode: QueryMode::Simple,
            ..Default::default()
        },
    )
    .await;
    assert!(!has_error(&events), "DDL failed: {events:?}");

    let events = run_query(
        &api,
        &sid,
        &format!(
            "INSERT INTO dbo.[{table}] (name, amount, flag, created, tags) VALUES (?, ?, ?, ?, ?)"
        ),
        vec![
            Value::Text("Alice".into()),
            Value::Decimal("12.34".into()),
            Value::Bool(true),
            Value::DateTime("2024-01-02 03:04:05.678".into()),
            Value::Null,
        ],
        QueryOptions::default(),
    )
    .await;
    assert_eq!(stats(&events).rows_affected, 1, "insert failed: {events:?}");

    let events = run_query(
        &api,
        &sid,
        &format!("SELECT id, name, amount, flag, tags FROM dbo.[{table}] WHERE name = ?"),
        vec![Value::Text("Alice".into())],
        QueryOptions::default(),
    )
    .await;
    let rows = collect_rows(&events);
    assert_eq!(rows.len(), 1, "rows: {rows:?}");
    assert!(matches!(rows[0][0], Value::Int(_)), "identity id: {rows:?}");
    assert_eq!(rows[0][1], Value::Text("Alice".into()));
    assert_eq!(rows[0][2], Value::Decimal("12.34".into()));
    assert_eq!(rows[0][3], Value::Bool(true));
    assert_eq!(rows[0][4], Value::Null);

    // UPDATE without parameters still reports affected rows.
    let events = run_query(
        &api,
        &sid,
        &format!("UPDATE dbo.[{table}] SET amount = 99.99 WHERE name = 'Alice'"),
        vec![],
        QueryOptions::default(),
    )
    .await;
    assert_eq!(stats(&events).rows_affected, 1, "update: {events:?}");

    // --- transactions ------------------------------------------------------
    api.begin_transaction(sid.clone()).await.unwrap();
    run_query(
        &api,
        &sid,
        &format!("INSERT INTO dbo.[{table}] (name) VALUES ('Temp')"),
        vec![],
        QueryOptions::default(),
    )
    .await;
    api.rollback_transaction(sid.clone()).await.unwrap();

    let events = run_query(
        &api,
        &sid,
        &format!("SELECT COUNT_BIG(*) FROM dbo.[{table}]"),
        vec![],
        QueryOptions::default(),
    )
    .await;
    assert_eq!(
        collect_rows(&events)[0][0],
        Value::Int(1),
        "rollback must undo the insert"
    );

    api.begin_transaction(sid.clone()).await.unwrap();
    run_query(
        &api,
        &sid,
        &format!("INSERT INTO dbo.[{table}] (name) VALUES ('Temp2')"),
        vec![],
        QueryOptions::default(),
    )
    .await;
    api.commit_transaction(sid.clone()).await.unwrap();

    let events = run_query(
        &api,
        &sid,
        &format!("SELECT COUNT_BIG(*) FROM dbo.[{table}]"),
        vec![],
        QueryOptions::default(),
    )
    .await;
    assert_eq!(
        collect_rows(&events)[0][0],
        Value::Int(2),
        "commit must persist the insert"
    );

    // --- introspection ------------------------------------------------------
    let scope = Scope {
        database: Some(url.database.clone()),
        schema: Some("dbo".into()),
    };

    let objects = api
        .list_objects(
            sid.clone(),
            scope.clone(),
            Some(ObjectKind::Table),
            Some(table.clone()),
        )
        .await
        .unwrap();
    assert!(
        objects.iter().any(|object| object.name == table),
        "objects: {objects:?}"
    );

    let columns = api
        .table_columns(sid.clone(), scope.clone(), table.clone())
        .await
        .unwrap();
    let id_column = columns
        .iter()
        .find(|column| column.name == "id")
        .expect("id column");
    assert!(id_column.is_primary_key, "id must be the PK: {id_column:?}");
    assert!(id_column.is_auto_increment, "id must be identity");
    let name_column = columns
        .iter()
        .find(|column| column.name == "name")
        .expect("name column");
    assert!(!name_column.nullable);
    assert_eq!(name_column.data_type, DataTypeKind::Text);
    assert_eq!(name_column.native_type, "nvarchar(100)");

    let indexes = api
        .table_indexes(sid.clone(), scope.clone(), table.clone())
        .await
        .unwrap();
    assert!(
        indexes.iter().any(|index| index.is_primary),
        "primary key index must be reported: {indexes:?}"
    );

    let foreign_keys = api
        .foreign_keys(sid.clone(), scope.clone(), table.clone())
        .await
        .unwrap();
    assert!(foreign_keys.is_empty(), "no FKs expected: {foreign_keys:?}");

    let ddl = api
        .object_ddl(
            sid.clone(),
            scope.clone(),
            ObjectKind::Table,
            table.clone(),
        )
        .await
        .unwrap()
        .expect("table DDL available");
    assert!(ddl.to_uppercase().contains("CREATE TABLE"));
    assert!(ddl.contains("IDENTITY(1,1)"), "ddl: {ddl}");

    // --- error mapping -------------------------------------------------------
    let events = run_query(
        &api,
        &sid,
        "SELECT * FROM dbo.mieosql_missing_table_xyz",
        vec![],
        QueryOptions::default(),
    )
    .await;
    let error = error_of(&events);
    assert!(
        matches!(error.code, ErrorCode::NotFound | ErrorCode::Syntax),
        "unexpected error code: {error:?}"
    );

    // --- cleanup -------------------------------------------------------------
    let events = run_query(
        &api,
        &sid,
        &format!("DROP TABLE dbo.[{table}]"),
        vec![],
        QueryOptions {
            mode: QueryMode::Simple,
            ..Default::default()
        },
    )
    .await;
    assert!(!has_error(&events), "drop failed: {events:?}");

    let _ = api.close_session(sid).await;
    let _ = std::fs::remove_dir_all(&base);
}
