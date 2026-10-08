//! MySQL → MySQL structure sync integration test.
//!
//! Covers the foreign-key pitfalls: statements are ordered parents-first and
//! a missing FK parent is reported as a plan warning instead of a failed apply.
//!
//! Ignored by default; needs a MySQL server with two databases:
//!
//! ```text
//! MIEOSQL_TEST_MYSQL_URL='mysql://root:mieosql-test@127.0.0.1:13306/src' \
//! MIEOSQL_TEST_MYSQL_TARGET='mysql://root:mieosql-test@127.0.0.1:13306/tgt' \
//!   cargo test -p mieosql-api --test debug_mysql_sync -- --ignored --nocapture
//! ```

use std::collections::BTreeMap;
use std::path::PathBuf;

use mieosql_api::sync::{StructureSyncRequest, SyncEndpoint};
use mieosql_api::{Api, ProfileInput};
use mieosql_core::{Engine as DbEngine, QueryEvent, QueryOptions, Scope, SslMode};
use mieosql_engine::{Engine, EngineConfig};

struct Env {
    api: Api,
    data_dir: PathBuf,
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.data_dir);
    }
}

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
        if let QueryEvent::Error { error, .. } = event {
            panic!("query failed: {error:?}\nSQL: {sql}");
        }
    }
}

fn parse_mysql_url(url: &str) -> (String, u16, String, String, String) {
    // mysql://user:pass@host:port/db
    let rest = url.strip_prefix("mysql://").expect("mysql:// URL");
    let (creds, addr) = rest.split_once('@').expect("user:pass@");
    let (user, pass) = creds.split_once(':').expect("user:pass");
    let (addr, db) = addr.split_once('/').expect("host:port/db");
    let (host, port) = addr.split_once(':').expect("host:port");
    (
        host.to_string(),
        port.parse().expect("port"),
        user.to_string(),
        pass.to_string(),
        db.to_string(),
    )
}

fn profile(name: &str, url: &str) -> ProfileInput {
    let (host, port, username, password, database) = parse_mysql_url(url);
    ProfileInput {
        id: None,
        name: name.to_string(),
        engine: DbEngine::MySql,
        host: Some(host),
        port: Some(port),
        username: Some(username),
        database: Some(database),
        file: None,
        params: BTreeMap::new(),
        ssl_mode: Some(SslMode::Disable),
        ssh: None,
        color: None,
        group: None,
        variant: None,
        password: Some(password),
        ssh_password: None,
    }
}

#[tokio::test]
#[ignore = "requires MIEOSQL_TEST_MYSQL_URL + MIEOSQL_TEST_MYSQL_TARGET (mysql:// URLs)"]
async fn mysql_structure_sync_orders_and_fks() {
    let Some(src_url) = std::env::var("MIEOSQL_TEST_MYSQL_URL").ok() else {
        eprintln!("MIEOSQL_TEST_MYSQL_URL not set; skipping");
        return;
    };
    let Some(tgt_url) = std::env::var("MIEOSQL_TEST_MYSQL_TARGET").ok() else {
        eprintln!("MIEOSQL_TEST_MYSQL_TARGET not set; skipping");
        return;
    };

    let mut base = std::env::temp_dir();
    base.push(format!("mieosql-debug-mysql-sync-{}", std::process::id()));
    let data_dir = base.join("data");
    std::fs::create_dir_all(&data_dir).unwrap();
    let engine = Engine::open(EngineConfig {
        data_dir: data_dir.clone(),
    })
    .unwrap();
    let api = Api::new(engine);
    let env = Env { api, data_dir };

    let src_profile = env.api.save_profile(profile("src", &src_url)).unwrap();
    let tgt_profile = env.api.save_profile(profile("tgt", &tgt_url)).unwrap();
    let src = env
        .api
        .open_session(src_profile.id.0.clone(), None)
        .await
        .unwrap();
    let tgt = env
        .api
        .open_session(tgt_profile.id.0.clone(), None)
        .await
        .unwrap();

    // Rich source table: unsigned PK + auto inc, varchar, text, decimal,
    // enum, json, timestamps, unique + plain index.
    exec(&env.api, &src.id, "DROP TABLE IF EXISTS orders").await;
    exec(&env.api, &src.id, "DROP TABLE IF EXISTS users").await;
    exec(&env.api, &tgt.id, "DROP TABLE IF EXISTS orders").await;
    exec(&env.api, &tgt.id, "DROP TABLE IF EXISTS users").await;
    exec(
        &env.api,
        &src.id,
        "CREATE TABLE users (\
           id BIGINT UNSIGNED NOT NULL AUTO_INCREMENT, \
           email VARCHAR(255) NOT NULL, \
           name VARCHAR(120) DEFAULT NULL, \
           bio TEXT, \
           score DECIMAL(10,2) NOT NULL DEFAULT 0.00, \
           status ENUM('active','banned') NOT NULL DEFAULT 'active', \
           meta JSON, \
           created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP, \
           PRIMARY KEY (id), \
           UNIQUE KEY ux_users_email (email), \
           KEY ix_users_name (name)\
         ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4",
    )
    .await;

    let plan = env
        .api
        .structure_sync_plan(StructureSyncRequest {
            source: SyncEndpoint {
                session_id: src.id.clone(),
                scope: Scope::db("src"),
            },
            target: SyncEndpoint {
                session_id: tgt.id.clone(),
                scope: Scope::db("tgt"),
            },
            tables: vec!["users".to_string()],
            include_indexes: true,
        })
        .await
        .expect("plan");

    println!("== same_engine: {}", plan.same_engine);
    for (i, s) in plan.statements.iter().enumerate() {
        println!("-- statement {} [{}] {} \n{}\n", i + 1, s.table, format!("{:?}", s.kind), s.sql);
    }

    let result = env
        .api
        .apply_sync_statements(tgt.id.clone(), plan.statements.iter().map(|s| s.sql.clone()).collect())
        .await
        .expect("apply");
    println!(
        "== apply: applied={} failed={} elapsed={}ms",
        result.applied, result.failed, result.elapsed_ms
    );
    for e in &result.errors {
        println!("!! {e}");
    }
    assert_eq!(result.failed, 0, "structure sync must apply cleanly");

    // ---- scenario 2: FK to a table missing on the target ----------------
    exec(&env.api, &src.id, "DROP TABLE IF EXISTS orders").await;
    exec(&env.api, &tgt.id, "DROP TABLE IF EXISTS orders").await;
    exec(
        &env.api,
        &src.id,
        "CREATE TABLE orders (\
           id BIGINT UNSIGNED NOT NULL AUTO_INCREMENT, \
           user_id BIGINT UNSIGNED NOT NULL, \
           total DECIMAL(12,2) NOT NULL, \
           PRIMARY KEY (id), \
           KEY ix_orders_user (user_id), \
           CONSTRAINT fk_orders_user FOREIGN KEY (user_id) REFERENCES users (id)\
         ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4",
    )
    .await;
    // Target has `users` from scenario 1, so first remove it to force the
    // missing-parent failure a fresh target would hit.
    exec(&env.api, &tgt.id, "DROP TABLE IF EXISTS users").await;

    let plan2 = env
        .api
        .structure_sync_plan(StructureSyncRequest {
            source: SyncEndpoint {
                session_id: src.id.clone(),
                scope: Scope::db("src"),
            },
            target: SyncEndpoint {
                session_id: tgt.id.clone(),
                scope: Scope::db("tgt"),
            },
            tables: vec!["orders".to_string()],
            include_indexes: true,
        })
        .await
        .expect("plan2");
    for (i, s) in plan2.statements.iter().enumerate() {
        println!("-- scenario2 statement {} [{}]\n{}\n", i + 1, s.table, s.sql);
    }
    println!("== scenario2 warnings: {:?}", plan2.warnings);
    assert!(
        plan2
            .warnings
            .iter()
            .any(|w| w.contains("users") && w.contains("foreign key")),
        "expected a missing-parent warning, got {:?}",
        plan2.warnings
    );
    let result2 = env
        .api
        .apply_sync_statements(
            tgt.id.clone(),
            plan2.statements.iter().map(|s| s.sql.clone()).collect(),
        )
        .await
        .expect("apply2");
    println!(
        "== scenario2 apply: applied={} failed={}",
        result2.applied, result2.failed
    );
    for e in &result2.errors {
        println!("!! scenario2: {e}");
    }

    // ---- scenario 3: both tables into an empty target --------------------
    exec(&env.api, &tgt.id, "DROP TABLE IF EXISTS orders").await;
    exec(&env.api, &tgt.id, "DROP TABLE IF EXISTS users").await;
    let plan3 = env
        .api
        .structure_sync_plan(StructureSyncRequest {
            source: SyncEndpoint {
                session_id: src.id.clone(),
                scope: Scope::db("src"),
            },
            target: SyncEndpoint {
                session_id: tgt.id.clone(),
                scope: Scope::db("tgt"),
            },
            tables: vec!["users".to_string(), "orders".to_string()],
            include_indexes: true,
        })
        .await
        .expect("plan3");
    for (i, s) in plan3.statements.iter().enumerate() {
        println!("-- scenario3 statement {} [{}] {}", i + 1, s.table, format!("{:?}", s.kind));
    }
    assert!(plan3.warnings.is_empty(), "unexpected warnings: {:?}", plan3.warnings);
    let result3 = env
        .api
        .apply_sync_statements(
            tgt.id.clone(),
            plan3.statements.iter().map(|s| s.sql.clone()).collect(),
        )
        .await
        .expect("apply3");
    println!(
        "== scenario3 apply: applied={} failed={}",
        result3.applied, result3.failed
    );
    for e in &result3.errors {
        println!("!! scenario3: {e}");
    }
    assert_eq!(result3.failed, 0, "parents-first ordering must avoid FK failures");

    // ---- scenario 4: both tables removed → children dropped first --------
    exec(&env.api, &src.id, "DROP TABLE IF EXISTS orders").await;
    exec(&env.api, &src.id, "DROP TABLE IF EXISTS users").await;
    let plan4 = env
        .api
        .structure_sync_plan(StructureSyncRequest {
            source: SyncEndpoint {
                session_id: src.id.clone(),
                scope: Scope::db("src"),
            },
            target: SyncEndpoint {
                session_id: tgt.id.clone(),
                scope: Scope::db("tgt"),
            },
            tables: vec!["users".to_string(), "orders".to_string()],
            include_indexes: true,
        })
        .await
        .expect("plan4");
    let drop_order: Vec<&str> = plan4
        .statements
        .iter()
        .map(|s| s.table.as_str())
        .collect();
    println!("== scenario4 drop order: {drop_order:?}");
    assert_eq!(
        drop_order.first().copied(),
        Some("orders"),
        "the child table must be dropped before its parent"
    );
    let result4 = env
        .api
        .apply_sync_statements(
            tgt.id.clone(),
            plan4.statements.iter().map(|s| s.sql.clone()).collect(),
        )
        .await
        .expect("apply4");
    println!(
        "== scenario4 apply: applied={} failed={}",
        result4.applied, result4.failed
    );
    assert_eq!(result4.failed, 0, "child-first drop ordering must apply cleanly");

    // ---- scenario 5: target session without a default database -----------
    // (reproduces MySQL 1046 "No database selected" on an unqualified DDL)
    exec(&env.api, &src.id, "DROP TABLE IF EXISTS noscope").await;
    exec(&env.api, &tgt.id, "DROP TABLE IF EXISTS noscope").await;
    exec(
        &env.api,
        &src.id,
        "CREATE TABLE noscope (id INT NOT NULL, PRIMARY KEY (id))",
    )
    .await;

    let mut nodb_input = profile("tgt-nodb", &tgt_url);
    nodb_input.database = None;
    let nodb_profile = env.api.save_profile(nodb_input).unwrap();
    let nodb_session = env
        .api
        .open_session(nodb_profile.id.0.clone(), None)
        .await
        .unwrap();

    let plan5 = env
        .api
        .structure_sync_plan(StructureSyncRequest {
            source: SyncEndpoint {
                session_id: src.id.clone(),
                scope: Scope::db("src"),
            },
            target: SyncEndpoint {
                session_id: nodb_session.id.clone(),
                scope: Scope::db("tgt"),
            },
            tables: vec!["noscope".to_string()],
            include_indexes: true,
        })
        .await
        .expect("plan5");
    println!("== scenario5 statement:\n{}\n", plan5.statements[0].sql);
    assert!(
        plan5.statements[0].sql.contains("`tgt`."),
        "CREATE must be qualified when the target session has no default database"
    );
    let result5 = env
        .api
        .apply_sync_statements(
            nodb_session.id.clone(),
            plan5.statements.iter().map(|s| s.sql.clone()).collect(),
        )
        .await
        .expect("apply5");
    println!(
        "== scenario5 apply: applied={} failed={}",
        result5.applied, result5.failed
    );
    for e in &result5.errors {
        println!("!! scenario5: {e}");
    }
    assert_eq!(result5.failed, 0, "1046 must be fixed by qualification");
}
