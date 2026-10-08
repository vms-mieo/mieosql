//! SQLite golden tests (QA-01): value-model round trip, DDL round trip,
//! structure-sync plan and cross-session transfer. No server needed.

use std::path::{Path, PathBuf};

use mieosql_api::sync::{DiffChange, DiffStatus, StructureSyncRequest, SyncEndpoint};
use mieosql_api::transfer::{TransferMode, TransferRequest};
use mieosql_api::{Api, ProfileInput};
use mieosql_core::{
    Engine as DbEngine, ErrorCode, ObjectKind, QueryEvent, QueryOptions, Scope, SslMode, Value,
};
use mieosql_engine::{Engine, EngineConfig};

struct Env {
    api: Api,
    base: PathBuf,
}

impl Drop for Env {
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
    base.push(format!("mieosql-golden-{tag}-{}-{nanos}", std::process::id()));
    base
}

async fn open_db(api: &Api, name: &str, file: &Path) -> (String, String) {
    std::fs::write(file, b"").unwrap(); // empty file = valid empty SQLite db
    let profile = api
        .save_profile(ProfileInput {
            id: None,
            name: name.into(),
            engine: DbEngine::Sqlite,
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
            variant: None,
            is_prod: false,
            password: None,
            ssh_password: None,
        })
        .unwrap();
    let pid = profile.id.0.clone();
    let sid = api.open_session(pid.clone()).await.unwrap().id;
    (pid, sid)
}

async fn setup_one(tag: &str) -> (Env, String, String) {
    let base = unique_base(tag);
    std::fs::create_dir_all(base.join("data")).unwrap();
    let engine = Engine::open(EngineConfig {
        data_dir: base.join("data"),
    })
    .unwrap();
    let api = Api::new(engine);
    let (pid, sid) = open_db(&api, "golden", &base.join("test.db")).await;
    (Env { api, base }, pid, sid)
}

async fn setup_two(tag: &str) -> (Env, String, String) {
    let base = unique_base(tag);
    std::fs::create_dir_all(base.join("data")).unwrap();
    let engine = Engine::open(EngineConfig {
        data_dir: base.join("data"),
    })
    .unwrap();
    let api = Api::new(engine);
    let (_, src) = open_db(&api, "src", &base.join("src.db")).await;
    let (_, tgt) = open_db(&api, "tgt", &base.join("tgt.db")).await;
    (Env { api, base }, src, tgt)
}

async fn exec(api: &Api, sid: &str, sql: &str) {
    exec_params(api, sid, sql, Vec::new()).await;
}

async fn exec_params(api: &Api, sid: &str, sql: &str, params: Vec<Value>) {
    let mut stream = api
        .start_query(sid.to_string(), sql.to_string(), params, Some(QueryOptions::default()))
        .await
        .unwrap_or_else(|e| panic!("query start failed for `{sql}`: {e}"));
    while let Some(event) = stream.events.recv().await {
        if let QueryEvent::Error { error, .. } = event {
            panic!("statement failed `{sql}`: {error:?}");
        }
    }
}

async fn fetch(api: &Api, sid: &str, sql: &str) -> Vec<Vec<Value>> {
    api.fetch_rows(sid.to_string(), sql.to_string(), Some(100))
        .await
        .unwrap()
        .rows
}

fn endpoint(sid: &str) -> SyncEndpoint {
    SyncEndpoint {
        session_id: sid.to_string(),
        scope: Scope::default(),
    }
}

#[tokio::test]
async fn value_model_lossless_round_trip() {
    let (env, _pid, sid) = setup_one("values").await;
    let api = &env.api;
    exec(
        api,
        &sid,
        "CREATE TABLE kv (id INTEGER PRIMARY KEY, big INTEGER, huge INTEGER, \
         amount NUMERIC, amount_txt TEXT, payload BLOB, note TEXT)",
    )
    .await;

    // 2^53+1 is the first integer a JSON double cannot represent exactly.
    let big = 9_007_199_254_740_993i64;
    let dec = "123456789012345678901234567890.123456789";
    let blob_b64 = "aGVsbG8="; // base64("hello")
    exec_params(
        api,
        &sid,
        "INSERT INTO kv (id, big, huge, amount, amount_txt, payload, note) \
         VALUES (?, ?, ?, ?, ?, ?, ?)",
        vec![
            Value::Int(1),
            Value::Int(big),
            Value::Int(i64::MAX),
            Value::Decimal(dec.into()),
            Value::Decimal(dec.into()),
            Value::Bytes(blob_b64.into()),
            Value::Null,
        ],
    )
    .await;

    let rows = fetch(api, &sid, "SELECT big, huge, amount, amount_txt, payload, note FROM kv").await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][0], Value::Int(big), "bigint beyond 2^53 must survive");
    assert_eq!(rows[0][1], Value::Int(i64::MAX));
    // SQLite NUMERIC affinity converts decimal text to REAL: f64 precision is
    // not preserved beyond ~16 significant digits (verified behavior).
    assert!(matches!(&rows[0][2], Value::Float(_)), "got {:?}", rows[0][2]);
    assert_ne!(rows[0][2].to_display_string(), dec);
    // TEXT affinity keeps the decimal string byte-exact: the lossless path.
    assert_eq!(rows[0][3], Value::Text(dec.into()));
    assert_eq!(rows[0][4], Value::Bytes(blob_b64.into()), "blob base64 must match");
    assert_eq!(rows[0][5], Value::Null);

    // The JSON wire format keeps 64-bit ints as strings (no float loss).
    let wire = serde_json::to_string(&Value::Int(i64::MAX)).unwrap();
    assert!(wire.contains("9223372036854775807"), "wire: {wire}");
    let back: Value = serde_json::from_str(&wire).unwrap();
    assert_eq!(back, Value::Int(i64::MAX));
}

#[tokio::test]
async fn ddl_round_trip_composite_pk_fk_unique_default() {
    let (env, _pid, sid) = setup_one("ddl").await;
    let api = &env.api;
    let scope = Scope::default();
    exec(api, &sid, "CREATE TABLE authors (id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT NOT NULL)")
        .await;
    exec(
        api,
        &sid,
        "CREATE TABLE books (author_id INTEGER NOT NULL, seq INTEGER NOT NULL, \
         title TEXT NOT NULL DEFAULT 'untitled', isbn TEXT, qty INTEGER DEFAULT 7, \
         PRIMARY KEY (author_id, seq), \
         FOREIGN KEY (author_id) REFERENCES authors(id))",
    )
    .await;
    exec(api, &sid, "CREATE UNIQUE INDEX ux_books_isbn ON books (isbn)").await;

    // Column defaults apply on insert.
    exec(api, &sid, "INSERT INTO authors (id, name) VALUES (1, 'a')").await;
    exec(api, &sid, "INSERT INTO books (author_id, seq, isbn) VALUES (1, 1, 'x')").await;
    let rows = fetch(api, &sid, "SELECT title, qty FROM books").await;
    assert_eq!(rows[0][0], Value::Text("untitled".into()));
    assert_eq!(rows[0][1].to_display_string(), "7");

    let cols = api.table_columns(sid.clone(), scope.clone(), "books".into()).await.unwrap();
    for pk in ["author_id", "seq"] {
        assert!(cols.iter().find(|c| c.name == pk).unwrap().is_primary_key, "{pk} is PK");
    }

    let indexes = api.table_indexes(sid.clone(), scope.clone(), "books".into()).await.unwrap();
    assert!(indexes.iter().any(|i| i.name == "ux_books_isbn" && i.is_unique));

    let fks = api.foreign_keys(sid.clone(), scope.clone(), "books".into()).await.unwrap();
    assert_eq!(fks.len(), 1);
    assert_eq!(fks[0].ref_table, "authors");
    assert_eq!(fks[0].columns, vec!["author_id".to_string()]);

    let ddl = api
        .object_ddl(sid.clone(), scope.clone(), ObjectKind::Table, "books".into())
        .await
        .unwrap()
        .expect("ddl available");
    let upper = ddl.to_uppercase();
    assert!(upper.contains("CREATE TABLE"), "ddl: {ddl}");
    assert!(upper.contains("PRIMARY KEY"), "ddl: {ddl}");
    assert!(upper.contains("REFERENCES"), "ddl: {ddl}");
}

#[tokio::test]
async fn structure_plan_detects_added_and_modified() {
    let (env, src, tgt) = setup_two("sync").await;
    let api = &env.api;
    exec(api, &src, "CREATE TABLE items (id INTEGER PRIMARY KEY, name TEXT, price REAL)").await;
    exec(api, &src, "CREATE TABLE audit_log (id INTEGER PRIMARY KEY, action TEXT)").await;
    exec(api, &tgt, "CREATE TABLE items (id INTEGER PRIMARY KEY, name VARCHAR(32))").await;

    let plan = api
        .structure_sync_plan(StructureSyncRequest {
            source: endpoint(&src),
            target: endpoint(&tgt),
            tables: Vec::new(),
            include_indexes: false,
        })
        .await
        .unwrap();
    assert!(plan.same_engine);
    assert_eq!(plan.summary.tables_added, 1);
    let added = plan.diffs.iter().find(|d| d.table == "audit_log").unwrap();
    assert_eq!(added.status, DiffStatus::Added);

    let items = plan.diffs.iter().find(|d| d.table == "items").unwrap();
    assert_eq!(items.status, DiffStatus::Changed);
    assert!(items.columns.iter().any(|c| c.name == "price" && c.change == DiffChange::Added));
    assert!(items.columns.iter().any(|c| c.name == "name" && c.change == DiffChange::Modified));
}

#[tokio::test]
async fn transfer_between_two_sessions_of_same_file() {
    // Same SQLite file, two sessions: satisfies the different-session rule.
    let (env, pid, src) = setup_one("transfer").await;
    let api = &env.api;
    let dst = api.open_session(pid).await.unwrap().id;
    exec(api, &src, "CREATE TABLE items (id INTEGER PRIMARY KEY, name TEXT)").await;
    exec(api, &src, "INSERT INTO items (id, name) VALUES (1, 'a'), (2, 'b'), (3, 'c')").await;

    let result = api
        .transfer_table(TransferRequest {
            source: endpoint(&src),
            target: endpoint(&dst),
            source_table: "items".into(),
            target_table: Some("items_copy".into()),
            mode: TransferMode::CreateAppend,
            columns: vec![],
            where_clause: None,
            batch_size: Some(100),
            limit: None,
        })
        .await
        .unwrap();
    assert_eq!(result.rows, 3);
    assert!(result.created);
    let rows = fetch(api, &dst, "SELECT COUNT(*) FROM items_copy").await;
    assert_eq!(rows[0][0], Value::Int(3));

    // The same session on both sides would deadlock the driver: rejected.
    let err = api
        .transfer_table(TransferRequest {
            source: endpoint(&src),
            target: endpoint(&src),
            source_table: "items".into(),
            target_table: Some("other".into()),
            mode: TransferMode::CreateAppend,
            columns: vec![],
            where_clause: None,
            batch_size: None,
            limit: None,
        })
        .await
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::Unsupported);
}
