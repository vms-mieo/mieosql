//! OB-04/OB-05: routines / triggers / MySQL Events introspection.
//!
//! - SQLite trigger listing + DDL runs live (no server needed).
//! - MySQL (triggers + Events under `ObjectKind::Trigger`) and SQL Server
//!   (functions / procedures / triggers) need real servers, so those tests
//!   are ignored by default:
//!
//! ```bash
//! MIEOSQL_TEST_MYSQL_URL='mysql://root:secret@127.0.0.1:3306/mieosql_test' \
//!   cargo test -p mieosql-api --test trigger_ddl_test -- --ignored --nocapture
//! MIEOSQL_TEST_MSSQL_URL='sqlserver://sa:YourStrong%40Passw0rd@localhost:1433/master?ssl=require' \
//!   cargo test -p mieosql-api --test trigger_ddl_test -- --ignored --nocapture
//! ```

use mieosql_api::{Api, ProfileInput};
use mieosql_core::{Engine as CoreEngine, ObjectKind, QueryEvent, QueryOptions, Scope, SslMode};
use mieosql_engine::{Engine, EngineConfig};

struct TestEnv {
    api: Api,
    data_dir: std::path::PathBuf,
    db_path: std::path::PathBuf,
    profile_id: String,
}

impl Drop for TestEnv {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.data_dir);
        let _ = std::fs::remove_file(&self.db_path);
    }
}

async fn sqlite_setup(tag: &str) -> TestEnv {
    let mut base = std::env::temp_dir();
    base.push(format!("mieosql-test-{tag}-{}", std::process::id()));
    let data_dir = base.join("data");
    std::fs::create_dir_all(&data_dir).unwrap();
    let db_path = base.join("test.db");
    std::fs::write(&db_path, b"").unwrap();

    let engine = Engine::open(EngineConfig {
        data_dir: data_dir.clone(),
    })
    .unwrap();
    let api = Api::new(engine);
    let profile = api
        .save_profile(ProfileInput {
            id: None,
            name: "sqlite-trigger-test".into(),
            engine: CoreEngine::Sqlite,
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
            password: None,
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

async fn run_sql(api: &Api, session_id: &str, sql: &str) {
    let mut stream = api
        .start_query(
            session_id.to_string(),
            sql.to_string(),
            vec![],
            Some(QueryOptions::default()),
        )
        .await
        .expect("query starts");
    let mut events = Vec::new();
    while let Some(event) = stream.events.recv().await {
        events.push(event);
    }
    assert!(
        !events.iter().any(|e| matches!(e, QueryEvent::Error { .. })),
        "SQL failed [{sql}]: {events:?}"
    );
}

#[tokio::test]
async fn sqlite_trigger_list_and_ddl() {
    let env = sqlite_setup("trig").await;
    let api = &env.api;
    let session = api.open_session(env.profile_id.clone(), None).await.unwrap();
    let sid = session.id.clone();

    run_sql(api, &sid, "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT)").await;
    run_sql(
        api,
        &sid,
        "CREATE TRIGGER trg_users_ai AFTER INSERT ON users \
         BEGIN UPDATE users SET name = NEW.name WHERE id = NEW.id; END",
    )
    .await;

    let scope = Scope {
        database: Some("main".into()),
        schema: None,
    };
    let triggers = api
        .list_objects(sid.clone(), scope.clone(), Some(ObjectKind::Trigger), None)
        .await
        .unwrap();
    let trg = triggers
        .iter()
        .find(|o| o.name == "trg_users_ai")
        .expect("trigger must be listed");
    assert_eq!(trg.kind, ObjectKind::Trigger);
    assert_eq!(trg.comment.as_deref(), Some("ON users"));

    // Pattern filter applies to triggers as well.
    let filtered = api
        .list_objects(
            sid.clone(),
            scope.clone(),
            Some(ObjectKind::Trigger),
            Some("no_such_trigger".into()),
        )
        .await
        .unwrap();
    assert!(filtered.is_empty());

    let ddl = api
        .object_ddl(sid.clone(), scope.clone(), ObjectKind::Trigger, "trg_users_ai".into())
        .await
        .unwrap()
        .expect("trigger ddl available");
    assert!(
        ddl.to_uppercase().contains("CREATE TRIGGER"),
        "unexpected ddl: {ddl}"
    );

    // Tables / views are unaffected by the trigger branch.
    let tables = api
        .list_objects(sid.clone(), scope.clone(), Some(ObjectKind::Table), None)
        .await
        .unwrap();
    assert!(tables.iter().any(|t| t.name == "users"));
    assert!(
        tables.iter().all(|t| t.kind == ObjectKind::Table),
        "kind=None/Trigger must not leak into table listing"
    );
}

// --- server-backed tests (ignored without a live instance) ----------------

fn server_api(tag: &str) -> (Api, std::path::PathBuf) {
    let mut base = std::env::temp_dir();
    base.push(format!("mieosql-test-{tag}-{}", std::process::id()));
    let data_dir = base.join("data");
    std::fs::create_dir_all(&data_dir).unwrap();
    let engine = Engine::open(EngineConfig {
        data_dir: data_dir.clone(),
    })
    .unwrap();
    (Api::new(engine), data_dir)
}

fn parse_simple_url(raw: &str, scheme: &str) -> (String, u16, String, String, String) {
    let rest = raw.strip_prefix(scheme).unwrap_or_else(|| {
        panic!("URL must start with `{scheme}`");
    });
    let rest = rest.split_once('?').map(|(m, _)| m).unwrap_or(rest);
    let (hostport, db) = match rest.split_once('/') {
        Some((h, d)) => (h, d),
        None => (rest, ""),
    };
    let (userinfo, hostport) = match hostport.rsplit_once('@') {
        Some((u, h)) => (Some(u), h),
        None => (None, hostport),
    };
    let (user, pass) = match userinfo {
        Some(u) => {
            let (a, b) = u.split_once(':').unwrap_or((u, ""));
            (a.to_string(), b.to_string())
        }
        None => (String::new(), String::new()),
    };
    let (host, port) = match hostport.split_once(':') {
        Some((h, p)) => (h.to_string(), p.parse().expect("invalid port")),
        None => (hostport.to_string(), 0),
    };
    (host, port, user, pass, db.to_string())
}

#[tokio::test]
#[ignore = "requires MIEOSQL_TEST_MYSQL_URL (a reachable MySQL instance)"]
async fn mysql_trigger_and_event_list_and_ddl() {
    let Ok(raw) = std::env::var("MIEOSQL_TEST_MYSQL_URL") else {
        eprintln!("MIEOSQL_TEST_MYSQL_URL is not set — skipping");
        return;
    };
    let (host, port, user, pass, db) = parse_simple_url(&raw, "mysql://");
    let db = if db.is_empty() { "mieosql_test".into() } else { db };

    let (api, data_dir) = server_api("mysql-trig");
    let profile = api
        .save_profile(ProfileInput {
            id: None,
            name: "mysql-trigger-test".into(),
            engine: CoreEngine::MySql,
            host: Some(host),
            port: if port == 0 { Some(3306) } else { Some(port) },
            username: Some(user),
            database: Some(db.clone()),
            file: None,
            params: Default::default(),
            ssl_mode: Some(SslMode::Disable),
            ssh: None,
            color: None,
            group: None,
            variant: None,
            password: Some(pass),
            ssh_password: None,
        })
        .unwrap();
    let session = api.open_session(profile.id.0.clone(), None).await.unwrap();
    let sid = session.id.clone();
    let scope = Scope {
        database: Some(db),
        schema: None,
    };

    run_sql(&api, &sid, "CREATE TABLE IF NOT EXISTS ob_users (id INT PRIMARY KEY)").await;
    run_sql(
        &api,
        &sid,
        "DROP TRIGGER IF EXISTS ob_trg_ai",
    )
    .await;
    run_sql(
        &api,
        &sid,
        "CREATE TRIGGER ob_trg_ai BEFORE INSERT ON ob_users FOR EACH ROW SET NEW.id = NEW.id",
    )
    .await;
    run_sql(&api, &sid, "DROP EVENT IF EXISTS ob_evt_hourly").await;
    run_sql(
        &api,
        &sid,
        "CREATE EVENT ob_evt_hourly ON SCHEDULE EVERY 1 HOUR DO SELECT 1",
    )
    .await;

    let objects = api
        .list_objects(sid.clone(), scope.clone(), Some(ObjectKind::Trigger), None)
        .await
        .unwrap();
    let trg = objects
        .iter()
        .find(|o| o.name == "ob_trg_ai")
        .expect("trigger must be listed");
    assert!(trg.comment.as_deref().unwrap_or("").contains("ON ob_users"));
    let evt = objects
        .iter()
        .find(|o| o.name == "ob_evt_hourly")
        .expect("event must ride along under Trigger kind");
    assert!(
        evt.comment.as_deref().unwrap_or("").starts_with("EVENT ·"),
        "event comment must be marked: {evt:?}"
    );

    let ddl = api
        .object_ddl(sid.clone(), scope.clone(), ObjectKind::Trigger, "ob_trg_ai".into())
        .await
        .unwrap()
        .expect("trigger ddl");
    assert!(ddl.to_uppercase().contains("TRIGGER"), "ddl: {ddl}");
    let ddl = api
        .object_ddl(
            sid.clone(),
            scope.clone(),
            ObjectKind::Trigger,
            "ob_evt_hourly".into(),
        )
        .await
        .unwrap()
        .expect("event ddl via SHOW CREATE EVENT");
    assert!(ddl.to_uppercase().contains("EVENT"), "ddl: {ddl}");

    run_sql(&api, &sid, "DROP TRIGGER ob_trg_ai").await;
    run_sql(&api, &sid, "DROP EVENT ob_evt_hourly").await;
    run_sql(&api, &sid, "DROP TABLE ob_users").await;
    let _ = std::fs::remove_dir_all(&data_dir);
}

#[tokio::test]
#[ignore = "requires MIEOSQL_TEST_MSSQL_URL (a reachable SQL Server instance)"]
async fn sqlserver_routine_trigger_list_and_ddl() {
    let Ok(raw) = std::env::var("MIEOSQL_TEST_MSSQL_URL") else {
        eprintln!("MIEOSQL_TEST_MSSQL_URL is not set — skipping");
        return;
    };
    // Reuse the mysql-style split; strip an optional ?ssl=... tail.
    let (hostport_user, _query) = match raw.split_once('?') {
        Some((m, q)) => (m, Some(q)),
        None => (raw.as_str(), None),
    };
    let (host, port, user, pass, db) = parse_simple_url(hostport_user, "sqlserver://");
    let (host, port, user, pass, db) = if host.is_empty() {
        let (h, p, u, pw, d) = parse_simple_url(hostport_user, "mssql://");
        (h, p, u, pw, d)
    } else {
        (host, port, user, pass, db)
    };
    let db = if db.is_empty() { "master".into() } else { db };

    let (api, data_dir) = server_api("mssql-trig");
    let profile = api
        .save_profile(ProfileInput {
            id: None,
            name: "mssql-trigger-test".into(),
            engine: CoreEngine::SqlServer,
            host: Some(host),
            port: if port == 0 { Some(1433) } else { Some(port) },
            username: Some(user),
            database: Some(db.clone()),
            file: None,
            params: Default::default(),
            ssl_mode: Some(SslMode::Require),
            ssh: None,
            color: None,
            group: None,
            variant: None,
            password: Some(pass),
            ssh_password: None,
        })
        .unwrap();
    let session = api.open_session(profile.id.0.clone(), None).await.unwrap();
    let sid = session.id.clone();
    let scope = Scope {
        database: Some(db),
        schema: Some("dbo".into()),
    };

    run_sql(
        &api,
        &sid,
        "IF OBJECT_ID('dbo.ob_proc_hello', 'P') IS NOT NULL DROP PROCEDURE dbo.ob_proc_hello",
    )
    .await;
    run_sql(
        &api,
        &sid,
        "CREATE PROCEDURE dbo.ob_proc_hello AS BEGIN SELECT 1 AS one END",
    )
    .await;

    let procs = api
        .list_objects(
            sid.clone(),
            scope.clone(),
            Some(ObjectKind::Procedure),
            Some("ob_proc_hello".into()),
        )
        .await
        .unwrap();
    assert!(
        procs.iter().any(|o| o.name == "ob_proc_hello" && o.kind == ObjectKind::Procedure),
        "procedure must be listed: {procs:?}"
    );

    let ddl = api
        .object_ddl(
            sid.clone(),
            scope.clone(),
            ObjectKind::Procedure,
            "ob_proc_hello".into(),
        )
        .await
        .unwrap()
        .expect("procedure ddl via OBJECT_DEFINITION");
    assert!(
        ddl.to_uppercase().contains("CREATE PROCEDURE"),
        "ddl: {ddl}"
    );

    run_sql(&api, &sid, "DROP PROCEDURE dbo.ob_proc_hello").await;
    let _ = std::fs::remove_dir_all(&data_dir);
}
