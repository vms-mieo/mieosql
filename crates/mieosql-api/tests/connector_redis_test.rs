//! Live integration test for the Redis driver (connector pack v2, workstream C3).
//!
//! Ignored by default; run against a real server with:
//!
//! ```text
//! MIEOSQL_TEST_REDIS_URL=redis://127.0.0.1:6379 \
//!   cargo test -p mieosql-api --test connector_redis_test -- --ignored
//! ```
//!
//! The URL follows `redis://[user:pass@]host:port[/db]` (or `rediss://` for TLS).

use std::collections::BTreeMap;
use std::path::PathBuf;

use mieosql_api::{Api, ProfileInput};
use mieosql_core::{QueryEvent, QueryOptions, Scope, SslMode, Value};
use mieosql_engine::{Engine, EngineConfig};

#[derive(Debug, PartialEq)]
struct ParsedRedisUrl {
    host: String,
    port: u16,
    username: Option<String>,
    password: Option<String>,
    database: Option<String>,
    tls: bool,
}

fn parse_redis_url(raw: &str) -> ParsedRedisUrl {
    let (tls, rest) = if let Some(rest) = raw.strip_prefix("rediss://") {
        (true, rest)
    } else if let Some(rest) = raw.strip_prefix("redis://") {
        (false, rest)
    } else {
        (false, raw)
    };
    let rest = rest.split(['?', '#']).next().unwrap_or(rest);
    let (authority, path) = match rest.split_once('/') {
        Some((authority, path)) => (authority, Some(path)),
        None => (rest, None),
    };
    let (userinfo, host_port) = match authority.rsplit_once('@') {
        Some((userinfo, host_port)) => (Some(userinfo), host_port),
        None => (None, authority),
    };
    let (username, password) = match userinfo {
        Some(info) => match info.split_once(':') {
            Some((user, pass)) => (
                (!user.is_empty()).then(|| percent_decode(user)),
                Some(percent_decode(pass)),
            ),
            None => (Some(percent_decode(info)), None),
        },
        None => (None, None),
    };
    let (host, port) = if let Some(bracketed) = host_port.strip_prefix('[') {
        match bracketed.split_once(']') {
            Some((host, tail)) => {
                let port = tail
                    .strip_prefix(':')
                    .and_then(|p| p.parse().ok())
                    .unwrap_or(6379);
                (host.to_string(), port)
            }
            None => (host_port.to_string(), 6379),
        }
    } else {
        match host_port.rsplit_once(':') {
            Some((host, port)) => (host.to_string(), port.parse().unwrap_or(6379)),
            None => (host_port.to_string(), 6379),
        }
    };
    let database = path.and_then(|p| {
        let p = p.trim_matches('/');
        (!p.is_empty()).then(|| p.to_string())
    });
    ParsedRedisUrl {
        host,
        port,
        username,
        password,
        database,
        tls,
    }
}

fn percent_decode(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(byte) = u8::from_str_radix(&raw[i + 1..i + 3], 16) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn redis_url() -> Option<String> {
    std::env::var("MIEOSQL_TEST_REDIS_URL")
        .ok()
        .map(|url| url.trim().to_string())
        .filter(|url| !url.is_empty())
}

async fn run(
    api: &Api,
    session_id: &str,
    command: &str,
    params: Vec<Value>,
    options: QueryOptions,
) -> Vec<QueryEvent> {
    let mut stream = api
        .start_query(
            session_id.to_string(),
            command.to_string(),
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

fn assert_ok(events: &[QueryEvent]) {
    if let Some(error) = events.iter().find_map(|event| match event {
        QueryEvent::Error { error, .. } => Some(error.clone()),
        _ => None,
    }) {
        panic!("query failed: {error:?}");
    }
}

fn columns(events: &[QueryEvent]) -> Vec<String> {
    events
        .iter()
        .find_map(|event| match event {
            QueryEvent::Columns { columns, .. } => {
                Some(columns.iter().map(|c| c.name.clone()).collect())
            }
            _ => None,
        })
        .unwrap_or_default()
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

fn stats(events: &[QueryEvent]) -> mieosql_core::QueryStats {
    events
        .iter()
        .find_map(|event| match event {
            QueryEvent::Finished { stats, .. } => Some(stats.clone()),
            _ => None,
        })
        .expect("finished event")
}

struct TestEnv {
    api: Api,
    session_id: String,
    data_dir: PathBuf,
    prefix: String,
}

impl Drop for TestEnv {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.data_dir);
    }
}

async fn setup() -> TestEnv {
    let url = parse_redis_url(&redis_url().expect("MIEOSQL_TEST_REDIS_URL is set"));

    let mut data_dir = std::env::temp_dir();
    data_dir.push(format!("mieosql-redis-c3-{}", std::process::id()));
    std::fs::create_dir_all(&data_dir).unwrap();
    let engine = Engine::open(EngineConfig {
        data_dir: data_dir.clone(),
    })
    .unwrap();
    let api = Api::new(engine);

    let profile = api
        .save_profile(ProfileInput {
            id: None,
            name: "redis-c3-test".to_string(),
            engine: mieosql_core::Engine::Redis,
            host: Some(url.host),
            port: Some(url.port),
            username: url.username,
            database: url.database,
            file: None,
            params: BTreeMap::new(),
            ssl_mode: Some(if url.tls {
                SslMode::VerifyFull
            } else {
                SslMode::Disable
            }),
            ssh: None,
            color: None,
            group: None,
            variant: None,
            password: url.password,
            ssh_password: None,
        })
        .unwrap();

    let session = api
        .open_session(profile.id.0.clone(), None)
        .await
        .expect("Redis session opens");

    TestEnv {
        api,
        session_id: session.id,
        data_dir,
        prefix: format!("mieosql:c3:{}", std::process::id()),
    }
}

#[tokio::test]
#[ignore = "requires a live Redis server (MIEOSQL_TEST_REDIS_URL)"]
async fn redis_full_flow() {
    if redis_url().is_none() {
        eprintln!("skipping: MIEOSQL_TEST_REDIS_URL is not set");
        return;
    }
    let env = setup().await;
    let api = &env.api;
    let sid = env.session_id.as_str();
    let prefix = env.prefix.as_str();
    let key = format!("{prefix}:key");
    let hash = format!("{prefix}:hash");
    let list = format!("{prefix}:list");
    let zset = format!("{prefix}:zset");
    let tx_key = format!("{prefix}:tx");
    let bound_key = format!("{prefix}:bound");

    // --- server info + ping ---------------------------------------------
    let info = api.server_info(sid.to_string()).await.unwrap();
    assert_eq!(info.engine, mieosql_core::Engine::Redis);
    assert!(!info.version.is_empty());
    assert!(info.version_comment.is_some());

    let events = run(api, sid, "PING", vec![], QueryOptions::default()).await;
    assert_ok(&events);
    assert_eq!(rows(&events), vec![vec![Value::Text("PONG".into())]]);

    // --- string SET/GET + write bookkeeping ------------------------------
    let events = run(
        api,
        sid,
        &format!("SET {key} hello"),
        vec![],
        QueryOptions::default(),
    )
    .await;
    assert_ok(&events);
    assert_eq!(stats(&events).rows_affected, 1);
    assert!(events.iter().any(|e| matches!(e, QueryEvent::Affected { .. })));

    let events = run(
        api,
        sid,
        &format!("GET {key}"),
        vec![],
        QueryOptions::default(),
    )
    .await;
    assert_ok(&events);
    assert_eq!(columns(&events), vec!["value"]);
    assert_eq!(rows(&events), vec![vec![Value::Text("hello".into())]]);

    // Missing key → a single NULL row.
    let events = run(
        api,
        sid,
        &format!("GET {prefix}:missing"),
        vec![],
        QueryOptions::default(),
    )
    .await;
    assert_ok(&events);
    assert_eq!(rows(&events), vec![vec![Value::Null]]);

    // --- bound parameters ------------------------------------------------
    let events = run(
        api,
        sid,
        &format!("SET {bound_key} ?"),
        vec![Value::Text("bound-value".into())],
        QueryOptions::default(),
    )
    .await;
    assert_ok(&events);
    let events = run(
        api,
        sid,
        &format!("GET {bound_key}"),
        vec![],
        QueryOptions::default(),
    )
    .await;
    assert_ok(&events);
    assert_eq!(rows(&events), vec![vec![Value::Text("bound-value".into())]]);

    // --- hash + HGETALL + PREVIEW ---------------------------------------
    let events = run(
        api,
        sid,
        &format!("HSET {hash} f1 v1 f2 v2"),
        vec![],
        QueryOptions::default(),
    )
    .await;
    assert_ok(&events);
    assert_eq!(stats(&events).rows_affected, 2);

    let events = run(
        api,
        sid,
        &format!("HGETALL {hash}"),
        vec![],
        QueryOptions::default(),
    )
    .await;
    assert_ok(&events);
    assert_eq!(columns(&events), vec!["field", "value"]);
    assert_eq!(rows(&events).len(), 2);

    let events = run(
        api,
        sid,
        &format!("PREVIEW {hash}"),
        vec![],
        QueryOptions::default(),
    )
    .await;
    assert_ok(&events);
    assert_eq!(columns(&events), vec!["field", "value"]);
    assert_eq!(rows(&events).len(), 2);

    // --- list + PREVIEW (index,value) ------------------------------------
    let events = run(
        api,
        sid,
        &format!("RPUSH {list} a b c"),
        vec![],
        QueryOptions::default(),
    )
    .await;
    assert_ok(&events);

    let events = run(
        api,
        sid,
        &format!("PREVIEW {list}"),
        vec![],
        QueryOptions::default(),
    )
    .await;
    assert_ok(&events);
    assert_eq!(columns(&events), vec!["index", "value"]);
    let list_rows = rows(&events);
    assert_eq!(list_rows.len(), 3);
    assert_eq!(list_rows[0], vec![Value::Int(0), Value::Text("a".into())]);
    assert_eq!(list_rows[2], vec![Value::Int(2), Value::Text("c".into())]);

    // --- sorted set + PREVIEW --------------------------------------------
    let events = run(
        api,
        sid,
        &format!("ZADD {zset} 1 one 2 two"),
        vec![],
        QueryOptions::default(),
    )
    .await;
    assert_ok(&events);

    let events = run(
        api,
        sid,
        &format!("PREVIEW {zset}"),
        vec![],
        QueryOptions::default(),
    )
    .await;
    assert_ok(&events);
    assert_eq!(columns(&events), vec!["field", "value"]);
    assert_eq!(rows(&events).len(), 2);

    // --- string / set / stream PREVIEW -----------------------------------
    let events = run(
        api,
        sid,
        &format!("PREVIEW {key}"),
        vec![],
        QueryOptions::default(),
    )
    .await;
    assert_ok(&events);
    assert_eq!(columns(&events), vec!["value"]);
    assert_eq!(rows(&events), vec![vec![Value::Text("hello".into())]]);

    let set = format!("{prefix}:set");
    let events = run(
        api,
        sid,
        &format!("SADD {set} x y"),
        vec![],
        QueryOptions::default(),
    )
    .await;
    assert_ok(&events);
    let events = run(
        api,
        sid,
        &format!("PREVIEW {set}"),
        vec![],
        QueryOptions::default(),
    )
    .await;
    assert_ok(&events);
    assert_eq!(columns(&events), vec!["value"]);
    assert_eq!(rows(&events).len(), 2);

    let stream = format!("{prefix}:stream");
    let events = run(
        api,
        sid,
        &format!("XADD {stream} * f1 v1"),
        vec![],
        QueryOptions::default(),
    )
    .await;
    assert_ok(&events);
    let events = run(
        api,
        sid,
        &format!("PREVIEW {stream}"),
        vec![],
        QueryOptions::default(),
    )
    .await;
    assert_ok(&events);
    assert_eq!(columns(&events), vec!["id", "field", "value"]);
    let stream_rows = rows(&events);
    assert_eq!(stream_rows.len(), 1);
    assert_eq!(stream_rows[0][1], Value::Text("f1".into()));
    assert_eq!(stream_rows[0][2], Value::Text("v1".into()));

    // --- max_rows truncation ---------------------------------------------
    let events = run(
        api,
        sid,
        &format!("LRANGE {list} 0 -1"),
        vec![],
        QueryOptions {
            max_rows: Some(2),
            ..Default::default()
        },
    )
    .await;
    assert_ok(&events);
    assert_eq!(stats(&events).rows, 2);
    assert!(stats(&events).truncated);

    // --- transactions: rollback discards, commit applies ------------------
    api.begin_transaction(sid.to_string()).await.unwrap();
    let events = run(
        api,
        sid,
        &format!("SET {tx_key} rolled-back"),
        vec![],
        QueryOptions::default(),
    )
    .await;
    assert_ok(&events);
    api.rollback_transaction(sid.to_string()).await.unwrap();
    let events = run(
        api,
        sid,
        &format!("GET {tx_key}"),
        vec![],
        QueryOptions::default(),
    )
    .await;
    assert_ok(&events);
    assert_eq!(rows(&events), vec![vec![Value::Null]]);

    api.begin_transaction(sid.to_string()).await.unwrap();
    let events = run(
        api,
        sid,
        &format!("SET {tx_key} committed"),
        vec![],
        QueryOptions::default(),
    )
    .await;
    assert_ok(&events);
    api.commit_transaction(sid.to_string()).await.unwrap();
    let events = run(
        api,
        sid,
        &format!("GET {tx_key}"),
        vec![],
        QueryOptions::default(),
    )
    .await;
    assert_ok(&events);
    assert_eq!(rows(&events), vec![vec![Value::Text("committed".into())]]);

    // --- introspection ----------------------------------------------------
    let databases = api.list_databases(sid.to_string()).await.unwrap();
    assert!(databases.iter().any(|db| db.name == "0"));

    api.use_database(sid.to_string(), "0".to_string())
        .await
        .unwrap();

    let objects = api
        .list_objects(
            sid.to_string(),
            Scope::default(),
            None,
            Some(format!("{prefix}:*")),
        )
        .await
        .unwrap();
    let names: Vec<&str> = objects.iter().map(|o| o.name.as_str()).collect();
    assert!(names.contains(&key.as_str()), "SCAN missed {key}: {names:?}");
    assert!(names.contains(&hash.as_str()), "SCAN missed {hash}");
    assert!(objects.iter().all(|o| o.kind == mieosql_core::ObjectKind::Table));

    let hash_columns = api
        .table_columns(sid.to_string(), Scope::default(), hash.clone())
        .await
        .unwrap();
    let field_names: Vec<&str> = hash_columns.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(field_names, vec!["f1", "f2"]);

    let indexes = api
        .table_indexes(sid.to_string(), Scope::default(), hash.clone())
        .await
        .unwrap();
    assert!(indexes.is_empty());

    // --- structured errors ------------------------------------------------
    let events = run(
        api,
        sid,
        "NOTACOMMAND",
        vec![],
        QueryOptions::default(),
    )
    .await;
    let error = events
        .iter()
        .find_map(|event| match event {
            QueryEvent::Error { error, .. } => Some(error.clone()),
            _ => None,
        })
        .expect("error event");
    assert_eq!(error.code, mieosql_core::ErrorCode::Syntax);
    assert_eq!(error.db_code.as_deref(), Some("ERR"));

    // --- cleanup ----------------------------------------------------------
    let events = run(
        api,
        sid,
        &format!("DEL {key} {hash} {list} {zset} {tx_key} {bound_key} {set} {stream}"),
        vec![],
        QueryOptions::default(),
    )
    .await;
    assert_ok(&events);
    assert_eq!(stats(&events).rows_affected, 8);
}

#[test]
fn parses_redis_urls() {
    assert_eq!(
        parse_redis_url("redis://localhost:6379"),
        ParsedRedisUrl {
            host: "localhost".to_string(),
            port: 6379,
            username: None,
            password: None,
            database: None,
            tls: false,
        }
    );
    assert_eq!(
        parse_redis_url("rediss://default:p%40ss@cache.example.com:6380/3"),
        ParsedRedisUrl {
            host: "cache.example.com".to_string(),
            port: 6380,
            username: Some("default".to_string()),
            password: Some("p@ss".to_string()),
            database: Some("3".to_string()),
            tls: true,
        }
    );
    assert_eq!(
        parse_redis_url("redis://[::1]:6380/0"),
        ParsedRedisUrl {
            host: "::1".to_string(),
            port: 6380,
            username: None,
            password: None,
            database: Some("0".to_string()),
            tls: false,
        }
    );
}
