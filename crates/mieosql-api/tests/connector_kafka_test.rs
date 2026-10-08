//! Kafka connector integration test.
//!
//! Disabled by default: it needs a real broker and is marked `#[ignore]`, so
//! the regular `cargo test` suite never requires Kafka. Run it with:
//!
//! ```text
//! MIEOSQL_TEST_KAFKA_BROKERS=localhost:9092 \
//!   cargo test -p mieosql-api --test connector_kafka_test -- --ignored --nocapture
//! ```
//!
//! `MIEOSQL_TEST_KAFKA_BROKERS` is a comma-separated `host:port` list (the port
//! may be omitted, in which case 9092 is used). The test picks the first topic
//! returned by `list_objects`; when the cluster has no topics it stops after
//! the metadata assertions with a message.
//!
//! Optional SASL: set `MIEOSQL_TEST_KAFKA_SASL_USERNAME` /
//! `MIEOSQL_TEST_KAFKA_SASL_PASSWORD` (and optionally
//! `MIEOSQL_TEST_KAFKA_SASL_MECHANISM` = `plain` | `scram-sha-256` |
//! `scram-sha-512`) to authenticate. Leave them unset for a PLAINTEXT broker.
//!
//! TLS is intentionally not exercised here: this build cannot install a rustls
//! `ClientConfig` (see the TLS note in `mieosql-drivers/src/kafka.rs`), so the
//! profile uses `SslMode::Disable` against a plaintext listener.

use std::path::PathBuf;

use mieosql_api::{Api, ProfileInput};
use mieosql_core::{
    Engine as EngineId, ErrorCode, ObjectKind, QueryEvent, QueryMode, QueryOptions, QueryStats,
    Scope, SslMode, Value,
};
use mieosql_engine::{Engine, EngineConfig};

struct TestEnv {
    api: Api,
    data_dir: PathBuf,
}

impl Drop for TestEnv {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.data_dir);
    }
}

/// Run one Kafka command and drain its event stream.
async fn run(api: &Api, session_id: &str, command: &str) -> Vec<QueryEvent> {
    let mut stream = api
        .start_query(
            session_id.to_string(),
            command.to_string(),
            Vec::new(),
            Some(QueryOptions {
                mode: QueryMode::Simple,
                fetch_size: 64,
                max_rows: None,
                timeout_ms: Some(15_000),
            }),
        )
        .await
        .expect("query starts");
    let mut events = Vec::new();
    while let Some(event) = stream.events.recv().await {
        events.push(event);
    }
    events
}

fn rows(events: &[QueryEvent]) -> Vec<Vec<Value>> {
    events
        .iter()
        .filter_map(|event| match event {
            QueryEvent::Batch { rows, .. } => Some(rows.clone()),
            _ => None,
        })
        .flatten()
        .collect()
}

fn error_of(events: &[QueryEvent]) -> Option<mieosql_core::ErrorInfo> {
    events.iter().find_map(|event| match event {
        QueryEvent::Error { error, .. } => Some(error.clone()),
        _ => None,
    })
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

#[tokio::test]
#[ignore = "requires MIEOSQL_TEST_KAFKA_BROKERS (comma-separated host:port)"]
async fn kafka_produce_consume_roundtrip() {
    let Some(brokers) = std::env::var("MIEOSQL_TEST_KAFKA_BROKERS")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    else {
        eprintln!("MIEOSQL_TEST_KAFKA_BROKERS not set; skipping Kafka integration test");
        return;
    };

    let mut base = std::env::temp_dir();
    base.push(format!("mieosql-test-kafka-{}", std::process::id()));
    let data_dir = base.join("data");
    std::fs::create_dir_all(&data_dir).expect("create test data dir");
    let engine = Engine::open(EngineConfig {
        data_dir: data_dir.clone(),
    })
    .expect("open engine");
    let api = Api::new(engine);
    let env = TestEnv {
        api: api.clone(),
        data_dir,
    };

    // Optional SASL credentials.
    let username = std::env::var("MIEOSQL_TEST_KAFKA_SASL_USERNAME")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let password = std::env::var("MIEOSQL_TEST_KAFKA_SASL_PASSWORD")
        .ok()
        .filter(|value| !value.is_empty());
    let mut params = std::collections::BTreeMap::new();
    if let Ok(mechanism) = std::env::var("MIEOSQL_TEST_KAFKA_SASL_MECHANISM") {
        let mechanism = mechanism.trim().to_string();
        if !mechanism.is_empty() {
            params.insert("sasl_mechanism".to_string(), mechanism);
        }
    }

    let profile = env
        .api
        .save_profile(ProfileInput {
            id: None,
            name: "kafka-integration".to_string(),
            engine: EngineId::Kafka,
            host: Some(brokers),
            port: None,
            username,
            database: None,
            file: None,
            params,
            ssl_mode: Some(SslMode::Disable),
            ssh: None,
            color: None,
            group: None,
            variant: None,
            password,
            ssh_password: None,
        })
        .expect("save profile");

    let session = env
        .api
        .open_session(profile.id.0.clone(), None)
        .await
        .expect("open Kafka session");

    // ---- server identity -------------------------------------------------
    let info = env
        .api
        .server_info(session.id.clone())
        .await
        .expect("server info");
    assert_eq!(info.engine, EngineId::Kafka);
    assert_eq!(info.version, "Apache Kafka");
    assert!(
        info.version_comment
            .as_deref()
            .is_some_and(|comment| !comment.is_empty()),
        "broker list should be reported"
    );

    // ---- capabilities ----------------------------------------------------
    let capabilities = env
        .api
        .capabilities(session.id.clone())
        .await
        .expect("capabilities");
    assert!(!capabilities.transactions);
    assert!(!capabilities.list_databases);

    // ---- topics ----------------------------------------------------------
    let topics = env
        .api
        .list_objects(
            session.id.clone(),
            Scope::default(),
            Some(ObjectKind::Table),
            None,
        )
        .await
        .expect("list topics");
    assert!(topics.iter().all(|topic| topic.kind == ObjectKind::Table));
    assert!(
        topics
            .iter()
            .all(|topic| topic.comment.as_deref().is_some_and(|c| c.ends_with("partitions"))),
        "every topic should carry a partition-count comment"
    );

    let Some(topic) = topics.first().map(|topic| topic.name.clone()) else {
        eprintln!("cluster has no topics; skipping produce/consume roundtrip");
        return;
    };

    // ---- preview statement (data-grid path) ------------------------------
    let (mut preview_stream, preview_command) = env
        .api
        .preview_table(session.id.clone(), Scope::default(), topic.clone(), Some(5))
        .await
        .expect("preview starts");
    assert!(preview_command.contains(&topic));
    assert!(preview_command.contains("\"from\": \"latest\""));
    while preview_stream.events.recv().await.is_some() {}

    // ---- produce ---------------------------------------------------------
    let marker = format!(
        "mieosql-it-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_millis()
    );
    let produce = format!(
        r#"{{"produce": {{"topic": "{}", "value": "{}"}}}}"#,
        topic, marker
    );
    let events = run(&env.api, &session.id, &produce).await;
    assert!(
        error_of(&events).is_none(),
        "produce failed: {:?}",
        error_of(&events)
    );
    let produced = stats(&events);
    assert_eq!(produced.rows_affected, 1);
    let offset = produced.last_insert_id.expect("broker-assigned offset");

    // ---- consume exactly the produced record -----------------------------
    let consume = format!(
        r#"{{"topic": "{}", "from": {}, "limit": 1}}"#,
        topic, offset
    );
    let events = run(&env.api, &session.id, &consume).await;
    assert!(
        error_of(&events).is_none(),
        "consume failed: {:?}",
        error_of(&events)
    );
    assert!(
        events.iter().any(|event| matches!(
            event,
            QueryEvent::Columns { columns, .. } if columns.len() == 5
        )),
        "consume must emit the five result columns"
    );
    let collected = rows(&events);
    assert_eq!(collected.len(), 1, "expected exactly the produced record");
    let row = &collected[0];
    assert_eq!(row.len(), 5);
    assert_eq!(row[1], Value::Int(offset as i64), "record offset");
    assert_eq!(row[4], Value::Text(marker.clone()), "record value");

    // ---- earliest / latest aliases are accepted --------------------------
    let earliest = format!(r#"{{"topic": "{}", "from": "earliest", "limit": 1}}"#, topic);
    let events = run(&env.api, &session.id, &earliest).await;
    assert!(
        error_of(&events).is_none(),
        "earliest consume failed: {:?}",
        error_of(&events)
    );

    let latest = format!(r#"{{"topic": "{}", "from": "latest", "limit": 1}}"#, topic);
    let events = run(&env.api, &session.id, &latest).await;
    assert!(
        error_of(&events).is_none(),
        "latest consume failed: {:?}",
        error_of(&events)
    );

    // ---- parse failures surface as structured Syntax errors --------------
    let events = run(&env.api, &session.id, "this is not json").await;
    let error = error_of(&events).expect("syntax error expected");
    assert_eq!(error.code, ErrorCode::Syntax);
    assert!(error.hint.is_some(), "syntax errors should carry a hint");

    env.api
        .close_session(session.id.clone())
        .await
        .expect("close session");
}
