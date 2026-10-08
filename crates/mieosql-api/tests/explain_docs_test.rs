//! Integration coverage for feature pack B2: visual explain + data dictionary
//! export, exercised end-to-end on a temporary SQLite database.
//!
//! Pure plan/doc parsing is unit-tested inside `src/explain.rs` / `src/docs.rs`;
//! this file checks the wiring: query execution, tree mapping and file output.

use std::path::PathBuf;

use mieosql_api::docs::{DocFormat, DocOptions};
use mieosql_api::explain::ExplainFormat;
use mieosql_api::{Api, ProfileInput};
use mieosql_core::{Engine, ErrorCode, QueryEvent, Scope, SslMode};

struct TestEnv {
    api: Api,
    session_id: String,
    base: PathBuf,
}

impl Drop for TestEnv {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

async fn setup(tag: &str) -> TestEnv {
    let mut base = std::env::temp_dir();
    base.push(format!("mieosql-b2-{tag}-{}", std::process::id()));
    let data_dir = base.join("data");
    std::fs::create_dir_all(&data_dir).unwrap();
    let db_path = base.join("test.db");
    std::fs::write(&db_path, b"").unwrap(); // empty file = valid empty SQLite db

    let engine = mieosql_engine::Engine::open(mieosql_engine::EngineConfig { data_dir }).unwrap();
    let api = Api::new(engine);
    let profile = api
        .save_profile(ProfileInput {
            id: None,
            name: format!("sqlite-{tag}"),
            engine: Engine::Sqlite,
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
            password: Some("test".into()),
            ssh_password: None,
        })
        .unwrap();
    let session = api.open_session(profile.id.0.clone(), None).await.unwrap();

    TestEnv {
        api,
        session_id: session.id,
        base,
    }
}

/// Run a statement and drain its event stream, panicking on engine errors.
async fn exec(api: &Api, session_id: &str, sql: &str) {
    let mut stream = api
        .start_query(session_id.to_string(), sql.to_string(), Vec::new(), None)
        .await
        .expect("query starts");
    while let Some(event) = stream.events.recv().await {
        match event {
            QueryEvent::Error { error, .. } => panic!("query failed: {error:?}"),
            QueryEvent::Finished { .. } => break,
            _ => {}
        }
    }
}

fn sqlite_scope() -> Scope {
    Scope {
        database: Some("main".to_string()),
        schema: None,
    }
}

#[tokio::test]
async fn sqlite_explain_query_plan() {
    let env = setup("explain").await;
    exec(
        &env.api,
        &env.session_id,
        "CREATE TABLE users (\
             id INTEGER PRIMARY KEY AUTOINCREMENT, \
             name TEXT NOT NULL, \
             email TEXT\
         )",
    )
    .await;

    let plan = env
        .api
        .explain_query(
            env.session_id.clone(),
            "SELECT id, name FROM users WHERE id = 1".to_string(),
            false,
        )
        .await
        .expect("explain succeeds");

    assert_eq!(plan.engine, Engine::Sqlite);
    assert_eq!(plan.format, ExplainFormat::QueryPlan);
    assert!(!plan.analyzed);
    assert!(plan.total_cost.is_none());
    assert_eq!(plan.sql, "SELECT id, name FROM users WHERE id = 1");

    let raw = plan.raw.as_deref().unwrap_or_default();
    assert!(
        raw.contains("SEARCH") || raw.contains("SCAN"),
        "unexpected raw plan: {raw}"
    );
    assert!(
        plan.root.label.contains("SEARCH") || plan.root.label.contains("SCAN"),
        "unexpected root label: {}",
        plan.root.label
    );

    // Empty statements are rejected before touching the engine.
    let error = env
        .api
        .explain_query(env.session_id.clone(), "   ".to_string(), false)
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::Data);

    // Bad SQL surfaces as a structured engine error, never as a panic.
    let error = env
        .api
        .explain_query(
            env.session_id.clone(),
            "SELECT * FROM no_such_table".to_string(),
            false,
        )
        .await
        .unwrap_err();
    assert!(matches!(
        error.code,
        ErrorCode::NotFound | ErrorCode::Syntax | ErrorCode::Data
    ));
}

#[tokio::test]
async fn sqlite_documentation_export_markdown_and_html() {
    let env = setup("docs").await;
    exec(
        &env.api,
        &env.session_id,
        "CREATE TABLE users (\
             id INTEGER PRIMARY KEY AUTOINCREMENT, \
             name TEXT NOT NULL, \
             email TEXT\
         )",
    )
    .await;
    exec(
        &env.api,
        &env.session_id,
        "CREATE TABLE orders (\
             id INTEGER PRIMARY KEY, \
             user_id INTEGER NOT NULL REFERENCES users(id), \
             total REAL\
         )",
    )
    .await;

    let options = DocOptions {
        scope: sqlite_scope(),
        format: DocFormat::Markdown,
        include_columns: true,
        include_indexes: true,
        include_foreign_keys: true,
        include_views: false,
        include_ddl: true,
        title: None,
    };

    let md_path = env.base.join("schema.md");
    let summary = env
        .api
        .export_documentation(
            env.session_id.clone(),
            options.clone(),
            md_path.to_string_lossy().into_owned(),
        )
        .await
        .expect("markdown export succeeds");
    assert_eq!(summary.tables, 2);
    assert!(summary.columns >= 6, "unexpected column count: {summary:?}");
    assert!(summary.indexes >= 2, "unexpected index count: {summary:?}");
    assert!(summary.foreign_keys >= 1);
    assert!(summary.bytes > 0);
    assert_eq!(summary.path, md_path.to_string_lossy());

    let md = std::fs::read_to_string(&md_path).expect("markdown file exists");
    assert!(md.starts_with("# main — Data Dictionary"));
    assert!(md.contains("| Column | Type | Nullable | Key | Default | Comment |"));
    assert!(md.contains("## users"));
    assert!(md.contains("## orders"));
    assert!(md.contains("### Foreign keys"));
    assert!(md.contains("```sql"));

    let html_path = env.base.join("schema.html");
    let mut html_options = options.clone();
    html_options.format = DocFormat::Html;
    let html_summary = env
        .api
        .export_documentation(
            env.session_id.clone(),
            html_options,
            html_path.to_string_lossy().into_owned(),
        )
        .await
        .expect("html export succeeds");
    assert_eq!(html_summary.tables, 2);

    // Self-contained HTML: inline style only, no external assets, no CDN.
    let html = std::fs::read_to_string(&html_path).expect("html file exists");
    assert!(html.starts_with("<!DOCTYPE html>"));
    assert!(html.contains("<style>"));
    assert!(html.contains("users"));
    assert!(html.contains("orders"));
    assert!(!html.contains("http://") && !html.contains("https://"));

    // A missing output directory maps to a structured Data error.
    let missing = env.base.join("no-such-dir").join("schema.md");
    let error = env
        .api
        .export_documentation(
            env.session_id.clone(),
            options,
            missing.to_string_lossy().into_owned(),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::Data);
}
