//! Integration test for the database-wide search (`Api::search_database`)
//! over a real SQLite file: names, columns, definitions, limit/truncation
//! and structured regex errors.

use std::path::PathBuf;

use mieosql_api::search::{SearchHitField, SearchMatchMode, SearchRequest};
use mieosql_api::{Api, ProfileInput};
use mieosql_core::{ErrorCode, SslMode};
use mieosql_engine::{Engine, EngineConfig};

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
    base.push(format!("mieosql-search-{tag}-{}", std::process::id()));
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
            name: "sqlite-search-test".into(),
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

async fn exec(api: &Api, session_id: &str, sql: &str) {
    let mut stream = api
        .start_query(session_id.to_string(), sql.to_string(), Vec::new(), None)
        .await
        .unwrap_or_else(|e| panic!("query start failed for `{sql}`: {e}"));
    while let Some(event) = stream.events.recv().await {
        if let mieosql_core::QueryEvent::Error { error, .. } = event {
            panic!("statement failed: `{sql}`: {error:?}");
        }
    }
}

fn request(pattern: &str, mode: SearchMatchMode) -> SearchRequest {
    SearchRequest {
        pattern: pattern.to_string(),
        case_sensitive: false,
        mode,
        kinds: Vec::new(),
        databases: Vec::new(),
        schemas: Vec::new(),
        include_columns: true,
        include_definitions: false,
        limit: 500,
    }
}

#[tokio::test]
async fn sqlite_search_finds_names_columns_definitions_and_limits() {
    let env = setup("main").await;
    let session = env.api.open_session(env.profile_id.clone()).await.unwrap();
    let sid = session.id.clone();

    exec(
        &env.api,
        &sid,
        "CREATE TABLE users (id INTEGER PRIMARY KEY, email TEXT, full_name TEXT)",
    )
    .await;
    exec(
        &env.api,
        &sid,
        "CREATE TABLE audit_log (id INTEGER PRIMARY KEY, user_id INTEGER, action TEXT)",
    )
    .await;
    exec(
        &env.api,
        &sid,
        "CREATE VIEW active_users AS SELECT id, email FROM users",
    )
    .await;

    // --- names + columns, case-insensitive contains -----------------------
    let res = env
        .api
        .search_database(sid.clone(), request("USER", SearchMatchMode::Contains))
        .await
        .unwrap();

    assert_eq!(res.scanned_objects, 3, "two tables + one view");
    assert!(!res.truncated);

    let name_hits: Vec<&str> = res
        .hits
        .iter()
        .filter(|h| h.matched_in == SearchHitField::Name)
        .map(|h| h.object.as_str())
        .collect();
    assert!(name_hits.contains(&"users"));
    assert!(name_hits.contains(&"active_users"));
    assert!(!name_hits.contains(&"audit_log"));

    // Column hits are produced even when the table name does not match.
    let col = res
        .hits
        .iter()
        .find(|h| h.matched_in == SearchHitField::Column && h.object == "audit_log")
        .expect("user_id column hit for audit_log");
    assert_eq!(col.column.as_deref(), Some("user_id"));
    assert_eq!(col.column_type.as_deref(), Some("INTEGER"));
    assert_eq!(col.database.as_deref(), Some("main"));

    // --- definition search (whole word, excerpt) --------------------------
    let mut req = request("action", SearchMatchMode::WholeWord);
    req.include_definitions = true;
    let res = env.api.search_database(sid.clone(), req).await.unwrap();

    let def = res
        .hits
        .iter()
        .find(|h| h.matched_in == SearchHitField::Definition)
        .expect("definition hit for audit_log");
    assert_eq!(def.object, "audit_log");
    let excerpt = def.excerpt.as_deref().unwrap_or_default();
    assert!(excerpt.contains("action"), "unexpected excerpt: {excerpt}");
    assert!(res
        .hits
        .iter()
        .any(|h| h.matched_in == SearchHitField::Column
            && h.column.as_deref() == Some("action")));

    // --- limit + truncated ------------------------------------------------
    let mut req = request("user", SearchMatchMode::Contains);
    req.limit = 1;
    let res = env.api.search_database(sid.clone(), req).await.unwrap();
    assert_eq!(res.hits.len(), 1);
    assert!(res.truncated, "more matching objects exist than the limit");

    // A pattern with no hits must not report truncation.
    let req = request("no_such_object_xyz", SearchMatchMode::Contains);
    let res = env.api.search_database(sid.clone(), req).await.unwrap();
    assert!(res.hits.is_empty());
    assert!(!res.truncated);

    // --- regex mode -------------------------------------------------------
    let req = request("^a.*_log$", SearchMatchMode::Regex);
    let res = env.api.search_database(sid.clone(), req).await.unwrap();
    assert!(res
        .hits
        .iter()
        .any(|h| h.object == "audit_log" && h.matched_in == SearchHitField::Name));

    // --- invalid regex → structured Syntax error --------------------------
    let err = env
        .api
        .search_database(sid.clone(), request("(", SearchMatchMode::Regex))
        .await
        .expect_err("invalid regex must fail");
    assert_eq!(err.code, ErrorCode::Syntax);

    // --- empty pattern → structured Data error ----------------------------
    let err = env
        .api
        .search_database(sid.clone(), request("", SearchMatchMode::Contains))
        .await
        .expect_err("empty pattern must fail");
    assert_eq!(err.code, ErrorCode::Data);
}
