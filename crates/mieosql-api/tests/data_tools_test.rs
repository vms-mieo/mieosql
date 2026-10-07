//! End-to-end tests for feature pack B3 (data generation + masked export)
//! over a real SQLite engine, plus deterministic previews.

use std::path::PathBuf;

use mieosql_api::datagen::{GenColumnRule, GenMode, GenRequest, GenStrategy};
use mieosql_api::masking::{MaskColumnRule, MaskExportRequest, MaskStrategy};
use mieosql_api::{Api, ExportFormat, ProfileInput};
use mieosql_core::{ColumnMeta, QueryEvent, QueryMode, QueryOptions, Scope, SslMode, Value};
use mieosql_engine::{Engine, EngineConfig};

struct TestEnv {
    api: Api,
    session_id: String,
    dir: PathBuf,
}

impl Drop for TestEnv {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

async fn setup(tag: &str) -> TestEnv {
    let mut base = std::env::temp_dir();
    base.push(format!("mieosql-data-tools-{tag}-{}", std::process::id()));
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
            name: "sqlite-data-tools".into(),
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
            password: None,
            ssh_password: None,
        })
        .unwrap();
    let session = api.open_session(profile.id.0.clone()).await.unwrap();

    TestEnv {
        api,
        session_id: session.id,
        dir: base,
    }
}

/// Run a statement and fail the test on any engine error.
async fn exec(api: &Api, session_id: &str, sql: &str) {
    let mut stream = api
        .start_query(
            session_id.to_string(),
            sql.to_string(),
            Vec::new(),
            Some(QueryOptions::default()),
        )
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

async fn query(api: &Api, session_id: &str, sql: &str) -> (Vec<ColumnMeta>, Vec<Vec<Value>>) {
    let mut stream = api
        .start_query(
            session_id.to_string(),
            sql.to_string(),
            Vec::new(),
            Some(QueryOptions {
                mode: QueryMode::Prepared,
                fetch_size: 256,
                max_rows: None,
                timeout_ms: None,
            }),
        )
        .await
        .expect("query starts");
    let mut columns = Vec::new();
    let mut rows = Vec::new();
    while let Some(event) = stream.events.recv().await {
        match event {
            QueryEvent::Columns { columns: cols, .. } => columns = cols,
            QueryEvent::Batch { rows: batch, .. } => rows.extend(batch),
            QueryEvent::Error { error, .. } => panic!("query failed: {error:?}"),
            QueryEvent::Finished { .. } => break,
            _ => {}
        }
    }
    (columns, rows)
}

#[tokio::test]
async fn generate_then_execute_inserts() {
    let env = setup("gen-exec").await;
    let api = &env.api;
    let sid = env.session_id.clone();

    exec(
        api,
        &sid,
        "CREATE TABLE people (\
             id INTEGER PRIMARY KEY AUTOINCREMENT, \
             name TEXT NOT NULL, \
             email TEXT, \
             age INTEGER, \
             note TEXT, \
             created_at TEXT, \
             name_upper TEXT GENERATED ALWAYS AS (upper(name)) VIRTUAL\
         )",
    )
    .await;

    let result = api
        .generate_data(
            sid.clone(),
            GenRequest {
                scope: Scope::default(),
                table: "people".into(),
                rows: 25,
                seed: Some(42),
                rules: vec![
                    GenColumnRule {
                        column: "id".into(),
                        strategy: GenStrategy::Sequential { start: 1, step: 1 },
                    },
                    GenColumnRule {
                        column: "name".into(),
                        strategy: GenStrategy::Constant {
                            value: "Alice".into(),
                        },
                    },
                    GenColumnRule {
                        column: "email".into(),
                        strategy: GenStrategy::Email,
                    },
                    GenColumnRule {
                        column: "age".into(),
                        strategy: GenStrategy::RandomInt { min: 18, max: 65 },
                    },
                    GenColumnRule {
                        column: "created_at".into(),
                        strategy: GenStrategy::DateTimeRecent { days: 90 },
                    },
                ],
                batch_size: Some(7),
                mode: GenMode::Execute,
            },
        )
        .await
        .expect("generation executes");

    assert!(result.executed);
    assert_eq!(result.rows, 25);
    assert!(!result.truncated);
    assert_eq!(result.skipped_columns, vec!["name_upper".to_string()]);
    assert!(
        result.sql.contains("INSERT INTO \"people\""),
        "preview should carry the generated INSERTs: {}",
        result.sql
    );

    let (_, rows) = query(
        api,
        &sid,
        "SELECT id, name, email, age, name_upper FROM people ORDER BY id",
    )
    .await;
    assert_eq!(rows.len(), 25);
    for (index, row) in rows.iter().enumerate() {
        assert_eq!(row[0].to_display_string(), (index + 1).to_string());
        assert_eq!(row[1], Value::Text("Alice".into()));
        assert!(row[2].to_display_string().contains('@'));
        let age: i64 = row[3].to_display_string().parse().expect("age is an int");
        assert!((18..=65).contains(&age));
        assert_eq!(row[4], Value::Text("ALICE".into()));
    }
}

#[tokio::test]
async fn preview_is_capped_deterministic_and_side_effect_free() {
    let env = setup("gen-preview").await;
    let api = &env.api;
    let sid = env.session_id.clone();

    exec(api, &sid, "CREATE TABLE items (label TEXT, amount INTEGER)").await;

    let request = GenRequest {
        scope: Scope::default(),
        table: "items".into(),
        rows: 5_000,
        seed: Some(7),
        rules: vec![],
        batch_size: Some(250),
        mode: GenMode::PreviewSql,
    };
    let first = api
        .generate_data(sid.clone(), request.clone())
        .await
        .expect("preview");
    let second = api.generate_data(sid.clone(), request).await.expect("preview");

    assert_eq!(first.sql, second.sql, "same seed must produce the same SQL");
    assert_eq!(first.rows, 5_000);
    assert!(first.truncated);
    assert!(!first.executed);
    // 1000 preview rows / 250 per batch = 4 statements.
    assert_eq!(first.sql.matches("INSERT INTO").count(), 4);

    let (_, rows) = query(api, &sid, "SELECT COUNT(*) FROM items").await;
    assert_eq!(rows[0][0], Value::Int(0), "preview must not insert");
}

#[tokio::test]
async fn execute_inside_existing_transaction_does_not_commit() {
    let env = setup("gen-tx").await;
    let api = &env.api;
    let sid = env.session_id.clone();

    exec(api, &sid, "CREATE TABLE tx_items (label TEXT)").await;
    api.begin_transaction(sid.clone()).await.unwrap();

    let result = api
        .generate_data(
            sid.clone(),
            GenRequest {
                scope: Scope::default(),
                table: "tx_items".into(),
                rows: 3,
                seed: Some(1),
                rules: vec![],
                batch_size: None,
                mode: GenMode::Execute,
            },
        )
        .await
        .expect("generation executes inside the caller transaction");
    assert!(result.executed);

    // Visible inside the transaction, gone after rollback: the generator must
    // not have committed on its own.
    let (_, rows) = query(api, &sid, "SELECT COUNT(*) FROM tx_items").await;
    assert_eq!(rows[0][0], Value::Int(3));
    api.rollback_transaction(sid.clone()).await.unwrap();
    let (_, rows) = query(api, &sid, "SELECT COUNT(*) FROM tx_items").await;
    assert_eq!(rows[0][0], Value::Int(0));
}

#[tokio::test]
async fn masked_export_hides_values_and_reports_rules() {
    let env = setup("mask").await;
    let api = &env.api;
    let sid = env.session_id.clone();

    exec(
        api,
        &sid,
        "CREATE TABLE contacts (email TEXT, phone TEXT, note TEXT, clearance INTEGER)",
    )
    .await;
    exec(
        api,
        &sid,
        "INSERT INTO contacts (email, phone, note, clearance) VALUES \
         ('alice@corp.com', '0901234567', 'secret one', 9), \
         ('bob@corp.com', '0907654321', 'secret two', 3), \
         ('carol@corp.com', '0911111111', NULL, 7)",
    )
    .await;

    let request = MaskExportRequest {
        sql: "SELECT email, phone, note, clearance FROM contacts ORDER BY email".into(),
        format: ExportFormat::Csv,
        path: env.dir.join("masked.csv").to_string_lossy().into_owned(),
        table: None,
        rules: vec![
            MaskColumnRule {
                column: "EMAIL".into(),
                strategy: MaskStrategy::Email,
            },
            MaskColumnRule {
                column: "phone".into(),
                strategy: MaskStrategy::Hash {
                    salt: Some("pepper".into()),
                },
            },
            MaskColumnRule {
                column: "clearance".into(),
                strategy: MaskStrategy::RandomInt { min: 1, max: 5 },
            },
            MaskColumnRule {
                column: "no_such_column".into(),
                strategy: MaskStrategy::Null,
            },
        ],
        seed: Some(11),
    };

    let result = api
        .export_result_masked(sid.clone(), request.clone())
        .await
        .expect("masked export");
    assert_eq!(result.masked_columns, 3);
    assert_eq!(result.unmatched_rules, vec!["no_such_column".to_string()]);
    assert_eq!(result.stats.rows, 3);
    assert!(result.stats.bytes > 0);
    assert!(result.stats.path.ends_with("masked.csv"));

    let text = std::fs::read_to_string(&result.stats.path).unwrap();
    assert!(!text.contains("alice@corp.com"), "raw values must not leak");
    assert!(!text.contains("0901234567"), "raw values must not leak");
    assert!(text.contains("@example.com"));

    // Header is prefixed by the CSV BOM.
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 4);
    assert!(lines[0].ends_with("email,phone,note,clearance"));
    for line in &lines[1..] {
        let fields: Vec<&str> = line.split(',').collect();
        assert!(fields[0].starts_with("user"));
        assert_eq!(fields[1].len(), 16, "hash mask is 16 hex chars");
        assert!(fields[1].chars().all(|c| c.is_ascii_hexdigit()));
        assert!(fields[2].starts_with("secret") || fields[2].is_empty());
        let clearance: i64 = fields[3].parse().expect("random int mask");
        assert!((1..=5).contains(&clearance));
    }

    // Re-run with the same seed: fake values are stable, so the bytes match.
    let mut second = request;
    second.path = env.dir.join("masked-2.csv").to_string_lossy().into_owned();
    let again = api.export_result_masked(sid.clone(), second).await.unwrap();
    let text2 = std::fs::read_to_string(&again.stats.path).unwrap();
    assert_eq!(text, text2);
}
