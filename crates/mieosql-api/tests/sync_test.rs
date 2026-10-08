//! End-to-end sync over two SQLite database files: structure plan → apply →
//! re-plan → data plan → apply, using the public API surface only.

use std::path::PathBuf;

use mieosql_api::sync::{
    DataSyncRequest, DiffChange, DiffStatus, StructureSyncRequest, SyncEndpoint, SyncStatementKind,
};
use mieosql_api::{Api, ProfileInput};
use mieosql_core::{
    Engine as DbEngine, ObjectKind, QueryEvent, QueryOptions, Scope, SslMode, Value,
};
use mieosql_engine::{Engine, EngineConfig};

struct Env {
    api: Api,
    base: PathBuf,
    src_profile: String,
    tgt_profile: String,
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

async fn setup(tag: &str) -> Env {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let mut base = std::env::temp_dir();
    base.push(format!("mieosql-sync-{tag}-{}-{stamp}", std::process::id()));
    let data_dir = base.join("data");
    std::fs::create_dir_all(&data_dir).unwrap();

    let engine = Engine::open(EngineConfig {
        data_dir: data_dir.clone(),
    })
    .unwrap();
    let api = Api::new(engine);

    let mut profiles = Vec::new();
    for (name, file) in [("src", "src.db"), ("tgt", "tgt.db")] {
        let path = base.join(file);
        std::fs::write(&path, b"").unwrap();
        let profile = api
            .save_profile(ProfileInput {
                id: None,
                name: format!("sync-{name}"),
                engine: DbEngine::Sqlite,
                host: None,
                port: None,
                username: None,
                database: None,
                file: Some(path.to_string_lossy().into_owned()),
                params: Default::default(),
                ssl_mode: Some(SslMode::Disable),
                ssh: None,
                color: None,
                group: None,
                variant: None,
                is_prod: false,
                password: Some("test".into()),
                ssh_password: None,
            })
            .unwrap();
        profiles.push(profile.id.0.clone());
    }

    Env {
        api,
        base,
        src_profile: profiles[0].clone(),
        tgt_profile: profiles[1].clone(),
    }
}

/// Run a statement and panic on a query error.
async fn exec(api: &Api, session_id: &str, sql: &str) {
    let mut stream = api
        .start_query(
            session_id.to_string(),
            sql.to_string(),
            Vec::new(),
            Some(QueryOptions::default()),
        )
        .await
        .unwrap_or_else(|e| panic!("query start failed: {e}"));
    while let Some(event) = stream.events.recv().await {
        if let QueryEvent::Error { error, .. } = event {
            panic!("query failed: {error:?}");
        }
    }
}

async fn fetch(api: &Api, session_id: &str, sql: &str) -> Vec<Vec<Value>> {
    let mut stream = api
        .start_query(
            session_id.to_string(),
            sql.to_string(),
            Vec::new(),
            Some(QueryOptions::default()),
        )
        .await
        .unwrap();
    let mut rows = Vec::new();
    while let Some(event) = stream.events.recv().await {
        match event {
            QueryEvent::Batch { rows: batch, .. } => rows.extend(batch),
            QueryEvent::Error { error, .. } => panic!("query failed: {error:?}"),
            _ => {}
        }
    }
    rows
}

fn endpoint(session_id: &str, scope: &Scope) -> SyncEndpoint {
    SyncEndpoint {
        session_id: session_id.to_string(),
        scope: scope.clone(),
    }
}

fn data_request(src: &str, tgt: &str, scope: &Scope) -> DataSyncRequest {
    DataSyncRequest {
        source: endpoint(src, scope),
        target: endpoint(tgt, scope),
        table: "items".into(),
        key_columns: Vec::new(),
        compare_columns: Vec::new(),
        sample_limit: 1_000,
        max_statements: 100,
        full_scan: true,
    }
}

#[tokio::test]
async fn structure_and_data_sync_round_trip() {
    let env = setup("roundtrip").await;
    let api = &env.api;
    let src = api.open_session(env.src_profile.clone()).await.unwrap();
    let tgt = api.open_session(env.tgt_profile.clone()).await.unwrap();
    let scope = Scope {
        database: Some("main".into()),
        schema: None,
    };

    // --- source: schema + rows ------------------------------------------
    exec(
        api,
        &src.id,
        "CREATE TABLE items (id INTEGER PRIMARY KEY, name TEXT NOT NULL, price REAL)",
    )
    .await;
    exec(api, &src.id, "CREATE UNIQUE INDEX ux_items_name ON items (name)").await;
    exec(
        api,
        &src.id,
        "INSERT INTO items (id, name, price) VALUES (1, 'alpha', 1.5)",
    )
    .await;
    exec(
        api,
        &src.id,
        "INSERT INTO items (id, name, price) VALUES (2, 'beta', 2.5)",
    )
    .await;

    // --- target: schema missing `price`, rows differ ---------------------
    exec(
        api,
        &tgt.id,
        "CREATE TABLE items (id INTEGER PRIMARY KEY, name TEXT NOT NULL)",
    )
    .await;
    exec(
        api,
        &tgt.id,
        "INSERT INTO items (id, name) VALUES (2, 'beta-old')",
    )
    .await;
    exec(api, &tgt.id, "INSERT INTO items (id, name) VALUES (3, 'gamma')").await;

    // --- structure plan ---------------------------------------------------
    let plan = api
        .structure_sync_plan(StructureSyncRequest {
            source: endpoint(&src.id, &scope),
            target: endpoint(&tgt.id, &scope),
            tables: Vec::new(),
            include_indexes: true,
        })
        .await
        .unwrap();
    assert!(plan.same_engine);
    assert_eq!(plan.summary.tables_added, 0);
    assert_eq!(plan.summary.tables_removed, 0);
    assert_eq!(plan.summary.tables_changed, 1);
    assert_eq!(plan.summary.statements, plan.statements.len() as u64);

    let items = plan.diffs.iter().find(|d| d.table == "items").unwrap();
    assert_eq!(items.status, DiffStatus::Changed);
    assert!(items
        .columns
        .iter()
        .any(|c| c.name == "price" && c.change == DiffChange::Added));
    assert!(plan
        .statements
        .iter()
        .any(|s| s.kind == SyncStatementKind::AddColumn && s.sql.contains("price")));
    assert!(plan
        .statements
        .iter()
        .any(|s| s.kind == SyncStatementKind::AddIndex && s.sql.contains("ux_items_name")));

    // --- apply structure on the target -------------------------------------
    let sqls: Vec<String> = plan.statements.iter().map(|s| s.sql.clone()).collect();
    let applied = api
        .apply_sync_statements(tgt.id.clone(), sqls)
        .await
        .unwrap();
    assert_eq!(applied.failed, 0, "errors: {:?}", applied.errors);
    assert_eq!(applied.applied, plan.statements.len() as u64);

    let columns = api
        .table_columns(tgt.id.clone(), scope.clone(), "items".into())
        .await
        .unwrap();
    assert!(columns.iter().any(|c| c.name == "price"));
    let indexes = api
        .table_indexes(tgt.id.clone(), scope.clone(), "items".into())
        .await
        .unwrap();
    assert!(indexes.iter().any(|i| i.name == "ux_items_name"));

    // --- re-plan: nothing left to do ---------------------------------------
    let plan2 = api
        .structure_sync_plan(StructureSyncRequest {
            source: endpoint(&src.id, &scope),
            target: endpoint(&tgt.id, &scope),
            tables: Vec::new(),
            include_indexes: true,
        })
        .await
        .unwrap();
    assert_eq!(plan2.summary.statements, 0, "warnings: {:?}", plan2.warnings);
    assert!(plan2.diffs.iter().all(|d| d.status == DiffStatus::Same));

    // --- data plan ----------------------------------------------------------
    let dplan = api
        .data_sync_plan(data_request(&src.id, &tgt.id, &scope))
        .await
        .unwrap();
    assert_eq!(dplan.key_columns, vec!["id"]);
    assert_eq!(dplan.summary.source_rows, 2);
    assert_eq!(dplan.summary.target_rows, 2);
    assert_eq!(dplan.summary.to_insert, 1);
    assert_eq!(dplan.summary.to_update, 1);
    assert_eq!(dplan.summary.to_delete, 1);
    assert_eq!(dplan.summary.unchanged, 0);
    assert!(!dplan.summary.truncated, "warnings: {:?}", dplan.warnings);

    let kinds: Vec<SyncStatementKind> =
        dplan.statements.iter().map(|s| s.kind).collect();
    assert_eq!(
        kinds,
        vec![
            SyncStatementKind::Insert,
            SyncStatementKind::Update,
            SyncStatementKind::Delete
        ]
    );
    assert!(dplan.statements[1].sql.contains("SET \"name\" = 'beta'"));
    assert!(dplan.statements[1].sql.contains("\"price\" = 2.5"));

    // --- apply data sync ------------------------------------------------------
    let result = api
        .apply_data_sync(data_request(&src.id, &tgt.id, &scope))
        .await
        .unwrap();
    assert_eq!(result.inserted, 1);
    assert_eq!(result.updated, 1);
    assert_eq!(result.deleted, 1);
    assert_eq!(result.failed, 0, "errors: {:?}", result.errors);
    assert_eq!(result.truncated, 0);

    let rows = fetch(
        api,
        &tgt.id,
        "SELECT id, name, price FROM items ORDER BY id",
    )
    .await;
    assert_eq!(
        rows,
        vec![
            vec![
                Value::Int(1),
                Value::Text("alpha".into()),
                Value::Float(1.5)
            ],
            vec![
                Value::Int(2),
                Value::Text("beta".into()),
                Value::Float(2.5)
            ],
        ]
    );

    // --- apply semantics: skips + first-error rollback ------------------------
    let skipped = api
        .apply_sync_statements(
            tgt.id.clone(),
            vec![
                String::new(),
                "-- just a comment".into(),
                "SELECT 1".into(),
            ],
        )
        .await
        .unwrap();
    assert_eq!(skipped.applied, 1);
    assert_eq!(skipped.failed, 0);

    let failed = api
        .apply_sync_statements(
            tgt.id.clone(),
            vec!["INSERT INTO missing_table (id) VALUES (1)".into()],
        )
        .await
        .unwrap();
    assert_eq!(failed.applied, 0);
    assert_eq!(failed.failed, 1);
    assert_eq!(failed.errors.len(), 1);

    // The session is still usable after the rollback.
    let tables = api
        .list_objects(src.id.clone(), scope.clone(), Some(ObjectKind::Table), None)
        .await
        .unwrap();
    assert!(tables.iter().any(|t| t.name == "items"));
}

#[tokio::test]
async fn data_sync_caps_statements_and_reports_truncation() {
    let env = setup("caps").await;
    let api = &env.api;
    let src = api.open_session(env.src_profile.clone()).await.unwrap();
    let tgt = api.open_session(env.tgt_profile.clone()).await.unwrap();
    let scope = Scope {
        database: Some("main".into()),
        schema: None,
    };

    exec(
        api,
        &src.id,
        "CREATE TABLE items (id INTEGER PRIMARY KEY, name TEXT)",
    )
    .await;
    exec(
        api,
        &tgt.id,
        "CREATE TABLE items (id INTEGER PRIMARY KEY, name TEXT)",
    )
    .await;
    for id in 1..=5 {
        exec(
            api,
            &src.id,
            &format!("INSERT INTO items (id, name) VALUES ({id}, 'row{id}')"),
        )
        .await;
    }

    // Statement cap with full_scan: counts exact, preview capped.
    let mut request = data_request(&src.id, &tgt.id, &scope);
    request.max_statements = 2;
    let plan = api.data_sync_plan(request.clone()).await.unwrap();
    assert_eq!(plan.summary.to_insert, 5);
    assert_eq!(plan.summary.to_delete, 0);
    assert_eq!(plan.statements.len(), 2);
    assert!(plan.summary.truncated);
    assert!(plan
        .warnings
        .iter()
        .any(|w| w.contains("statement list capped")));

    // Applying the capped plan inserts only the previewed statements.
    let result = api.apply_data_sync(request).await.unwrap();
    assert_eq!(result.inserted, 2);
    assert_eq!(result.failed, 0);
    assert_eq!(result.truncated, 3);
    let rows = fetch(api, &tgt.id, "SELECT COUNT(*) FROM items").await;
    assert_eq!(rows[0][0], Value::Int(2));

    // Sample cap: only `sample_limit` rows per side are read. Rows 1-2 were
    // already inserted above, so the sampled pair matches (unchanged).
    let mut request = data_request(&src.id, &tgt.id, &scope);
    request.sample_limit = 2;
    request.max_statements = 100;
    let plan = api.data_sync_plan(request).await.unwrap();
    assert_eq!(plan.summary.source_rows, 2);
    assert_eq!(plan.summary.unchanged, 2);
    assert_eq!(plan.summary.to_insert, 0);
    assert!(plan.summary.truncated);
    assert!(plan.warnings.iter().any(|w| w.contains("sample limit")));
}
