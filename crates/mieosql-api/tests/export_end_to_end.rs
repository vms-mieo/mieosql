//! End-to-end tests for result export: every format is written through the
//! real engine (SQLite) and its bytes are inspected.

use std::path::PathBuf;

use mieosql_api::{Api, ExportFormat, ProfileInput};
use mieosql_core::{QueryEvent, QueryOptions, SslMode, Value};
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
    base.push(format!("mieosql-export-{tag}-{}", std::process::id()));
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
            name: "sqlite-export".into(),
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
    let session_id = session.id.clone();

    let mut events = api
        .start_query(
            session_id.clone(),
            "CREATE TABLE items (\
                id INTEGER PRIMARY KEY, \
                name TEXT NOT NULL, \
                note TEXT, \
                price REAL, \
                blob_col BLOB, \
                meta TEXT\
             )"
            .into(),
            vec![],
            Some(QueryOptions::default()),
        )
        .await
        .unwrap()
        .events;
    while let Some(evt) = events.recv().await {
        if let QueryEvent::Error { error, .. } = evt {
            panic!("DDL failed: {error:?}");
        }
    }

    let mut events = api
        .start_query(
            session_id.clone(),
            "INSERT INTO items (id, name, note, price, blob_col, meta) VALUES \
             (1, 'Widget', NULL, 9.99, X'00FF', '{\"a\":1}'), \
             (2, 'O''Brien', 'line1' || char(10) || 'line2', NULL, NULL, NULL)"
                .into(),
            vec![],
            Some(QueryOptions::default()),
        )
        .await
        .unwrap()
        .events;
    while let Some(evt) = events.recv().await {
        if let QueryEvent::Error { error, .. } = evt {
            panic!("INSERT failed: {error:?}");
        }
    }

    TestEnv {
        api,
        session_id,
        dir: base,
    }
}

const SELECT: &str = "SELECT id, name, note, price, blob_col, meta FROM items ORDER BY id";

async fn export(env: &TestEnv, format: ExportFormat, file: &str) -> PathBuf {
    let path = env.dir.join(file);
    let stats = env
        .api
        .export_result(
            env.session_id.clone(),
            SELECT.into(),
            format,
            path.to_string_lossy().into_owned(),
            Some("items".into()),
        )
        .await
        .expect("export succeeds");
    assert_eq!(stats.rows, 2, "two rows exported");
    assert!(stats.bytes > 0, "file has content");
    path
}

#[tokio::test]
async fn export_csv_tsv_json_sql_xlsx() {
    let env = setup("all").await;

    // ---- CSV ---------------------------------------------------------------
    let csv = std::fs::read_to_string(export(&env, ExportFormat::Csv, "out.csv").await).unwrap();
    assert!(csv.starts_with('\u{FEFF}'), "csv carries a UTF-8 BOM");
    let lines: Vec<&str> = csv.trim_start_matches('\u{FEFF}').lines().collect();
    assert_eq!(lines[0], "id,name,note,price,blob_col,meta");
    assert!(
        lines[1].contains("1,Widget,,9.99,AP8="),
        "NULL=empty, blob=base64: {}",
        lines[1]
    );
    assert!(
        lines[1].contains("\"{\"\"a\"\":1}\""),
        "json text is csv-quoted: {}",
        lines[1]
    );
    // A quoted field may span physical lines — check the raw content.
    assert!(
        csv.contains("\"line1\nline2\""),
        "embedded newline is csv-quoted: {csv}"
    );

    // ---- TSV ---------------------------------------------------------------
    let tsv = std::fs::read_to_string(export(&env, ExportFormat::Tsv, "out.tsv").await).unwrap();
    assert!(tsv.lines().next().unwrap().contains('\t'));

    // ---- JSON Lines ----------------------------------------------------------
    let jsonl =
        std::fs::read_to_string(export(&env, ExportFormat::JsonLines, "out.jsonl").await).unwrap();
    let rows: Vec<serde_json::Value> = jsonl
        .lines()
        .map(|l| serde_json::from_str(l).expect("each line is valid json"))
        .collect();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["name"], "Widget");
    assert_eq!(rows[0]["note"], serde_json::Value::Null);
    assert_eq!(rows[0]["id"], 1);
    assert_eq!(rows[1]["name"], "O'Brien");

    // ---- JSON array ----------------------------------------------------------
    let json = std::fs::read_to_string(export(&env, ExportFormat::Json, "out.json").await).unwrap();
    let arr: serde_json::Value = serde_json::from_str(&json).expect("valid json array");
    assert_eq!(arr.as_array().map(|a| a.len()), Some(2));

    // ---- SQL inserts ----------------------------------------------------------
    let sql = std::fs::read_to_string(export(&env, ExportFormat::Sql, "out.sql").await).unwrap();
    assert!(
        sql.contains("INSERT INTO \"items\""),
        "quoted table name: {sql}"
    );
    assert!(sql.contains("'O''Brien'"), "quote escaping: {sql}");
    assert!(sql.contains("X'00FF'"), "sqlite hex blob literal: {sql}");
    assert!(sql.contains("NULL"), "NULL literal");

    // ---- XLSX ----------------------------------------------------------------
    let xlsx_path = export(&env, ExportFormat::Xlsx, "out.xlsx").await;
    let bytes = std::fs::read(&xlsx_path).unwrap();
    assert_eq!(&bytes[..2], b"PK", "xlsx is a zip container");
    assert!(bytes.len() > 1000, "xlsx has real content");
}

#[tokio::test]
async fn export_failure_removes_partial_file() {
    let env = setup("fail").await;
    let path = env.dir.join("bad.csv");
    let err = env
        .api
        .export_result(
            env.session_id.clone(),
            "SELECT * FROM table_missing".into(),
            ExportFormat::Csv,
            path.to_string_lossy().into_owned(),
            None,
        )
        .await;
    assert!(err.is_err(), "export of a broken query fails");
    assert!(!path.exists(), "partial file is removed on failure");
}

#[tokio::test]
async fn export_sql_literals_roundtrip_values() {
    // Guard the value encoders directly as well.
    use mieosql_api::ExportFormat as F;
    let env = setup("values").await;
    let dir = env.dir.join("values");
    std::fs::create_dir_all(&dir).unwrap();

    // Export row 2 (O'Brien) and assert the SQL we emit actually INSERTs back.
    let path = dir.join("one.sql");
    let stats = env
        .api
        .export_result(
            env.session_id.clone(),
            "SELECT id, name FROM items WHERE id = 2".into(),
            F::Sql,
            path.to_string_lossy().into_owned(),
            Some("items_copy".into()),
        )
        .await
        .unwrap();
    assert_eq!(stats.rows, 1);
    let sql = std::fs::read_to_string(&path).unwrap();

    // Replay the generated INSERTs into a second table.
    let mut events = env
        .api
        .start_query(
            env.session_id.clone(),
            "CREATE TABLE items_copy (id INTEGER, name TEXT)".into(),
            vec![],
            Some(QueryOptions::default()),
        )
        .await
        .unwrap()
        .events;
    while let Some(e) = events.recv().await {
        if let QueryEvent::Error { error, .. } = e {
            panic!("ddl: {error:?}");
        }
    }
    for statement in sql.lines().filter(|l| l.starts_with("INSERT")) {
        let mut events = env
            .api
            .start_query(
                env.session_id.clone(),
                statement.to_string(),
                vec![],
                Some(QueryOptions::default()),
            )
            .await
            .unwrap()
            .events;
        while let Some(e) = events.recv().await {
            if let QueryEvent::Error { error, .. } = e {
                panic!("replay failed for {statement}: {error:?}");
            }
        }
    }

    // Verify the round-tripped value.
    let mut events = env
        .api
        .start_query(
            env.session_id.clone(),
            "SELECT name FROM items_copy".into(),
            vec![],
            Some(QueryOptions::default()),
        )
        .await
        .unwrap()
        .events;
    let mut names: Vec<Value> = Vec::new();
    while let Some(e) = events.recv().await {
        if let QueryEvent::Batch { rows, .. } = e {
            for row in rows {
                names.extend(row);
            }
        }
    }
    assert_eq!(names, vec![Value::Text("O'Brien".into())]);
}
