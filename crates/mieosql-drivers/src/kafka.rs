//! Apache Kafka driver (`rskafka`, pure Rust client).
//!
//! Query language (v1): JSON commands.
//! - Consume: `{ "topic": "orders", "partition": 0, "from": "latest", "limit": 100 }`
//!   (`from` = `"earliest"` | `"latest"` | integer offset)
//! - Produce: `{ "produce": { "topic": "orders", "key": "k", "value": "hello" } }`
//!
//! # TLS
//!
//! rskafka 0.6 installs TLS through
//! `ClientBuilder::tls_config(Arc<rustls::ClientConfig>)`; this build ships a
//! `rustls` config (webpki roots):
//!
//! - [`SslMode::Disable`] connects in plaintext.
//! - [`SslMode::Prefer`] keeps the opportunistic contract and stays plaintext
//!   (rskafka cannot negotiate TLS opportunistically); logged as a warning.
//! - [`SslMode::Require`] uses TLS **without** certificate verification.
//! - `VerifyCa` / `VerifyFull` use TLS with webpki root verification.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use chrono::{SecondsFormat, Utc};
use rskafka::client::partition::{Compression, OffsetAt, UnknownTopicHandling};
use rskafka::client::{Client, ClientBuilder, Credentials, SaslConfig};
use rskafka::record::Record;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use mieosql_core::{
    Capabilities,
    ColumnDef,
    ColumnMeta,
    ConnectionSpec,
    CreateDbUser,
    DataTypeKind,
    DatabaseRef,
    DbPrincipal,
    DropDbPrincipal,
    Engine,
    ErrorCode,
    ForeignKeyDef,
    GrantRequest,
    IndexDef,
    MieoError,
    MieoResult,
    ObjectKind,
    ObjectRef,
    PlaceholderStyle,
    QueryEvent,
    QueryOptions,
    QueryStats,
    RevokeRequest,
    RoleGrant,
    SchemaRef,
    Scope,
    ServerInfo,
    SslMode,
    Value,
    DbPrincipalDetail,
};

use crate::util::b64_encode;

/// Port used when neither the host string nor the spec carries one.
const DEFAULT_KAFKA_PORT: u16 = 9092;
/// Consume default when the JSON command omits `limit`.
const DEFAULT_CONSUME_LIMIT: u64 = 100;
/// Bytes requested per fetch round-trip (the broker always returns at least one
/// full record, so this only bounds the batch, not correctness).
const FETCH_MAX_BYTES: i32 = 1024 * 1024;
/// How long an empty fetch may wait before the broker answers.
const FETCH_MAX_WAIT_MS: i32 = 500;

const CONSUME_HINT: &str =
    r#"consume: {"topic": "orders", "partition": 0, "from": "latest", "limit": 100}"#;
const PRODUCE_HINT: &str =
    r#"produce: {"produce": {"topic": "orders", "key": "k", "value": "hello"}}"#;

/// Build the rustls client config used for Kafka TLS.
///
/// `verify == false` (SslMode::Require) accepts any server certificate;
/// `verify == true` (VerifyCa/VerifyFull) checks against webpki roots.
fn build_tls_config(verify: bool) -> std::sync::Arc<rustls::ClientConfig> {
    let builder = rustls::ClientConfig::builder_with_provider(
        rustls::crypto::ring::default_provider().into(),
    )
    .with_safe_default_protocol_versions()
    .expect("rustls default protocol versions are always available");

    let config = if verify {
        let mut roots = rustls::RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        builder.with_root_certificates(roots).with_no_client_auth()
    } else {
        builder
            .dangerous()
            .with_custom_certificate_verifier(std::sync::Arc::new(NoVerify))
            .with_no_client_auth()
    };
    std::sync::Arc::new(config)
}

/// Accept-any-certificate verifier for `SslMode::Require`.
#[derive(Debug)]
struct NoVerify;

impl rustls::client::danger::ServerCertVerifier for NoVerify {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

pub struct KafkaDriver {
    spec: ConnectionSpec,
    client: Client,
    /// Resolved bootstrap broker list (`host:port`), used for `server_info`.
    brokers: Vec<String>,
}

pub(crate) fn kafka_capabilities() -> Capabilities {
    Capabilities {
        engine_kind: mieosql_core::EngineKind::MessageBroker,
        placeholder: PlaceholderStyle::Question,
        list_databases: false,
        list_schemas: false,
        schema_is_database: false,
        transactions: false,
        simple_mode: true,
        ddl: false,
        multiple_databases: false,
        user_management: false,
    }
}

impl KafkaDriver {
    pub async fn connect(spec: &ConnectionSpec) -> MieoResult<Self> {
        let brokers = bootstrap_brokers(spec)?;
        let mut builder = ClientBuilder::new(brokers.clone());

        match spec.ssl_mode {
            SslMode::Disable => {}
            SslMode::Prefer => {
                // rskafka cannot negotiate TLS opportunistically: either a
                // `ClientConfig` is installed or the socket stays plaintext.
                tracing::warn!(
                    target: "mieosql::kafka",
                    "Kafka ssl_mode=prefer: rskafka cannot negotiate TLS opportunistically; continuing without TLS"
                );
            }
            SslMode::Require => {
                builder = builder.tls_config(build_tls_config(false));
            }
            SslMode::VerifyCa | SslMode::VerifyFull => {
                builder = builder.tls_config(build_tls_config(true));
            }
        }

        if let Some(username) = spec
            .username
            .as_deref()
            .map(str::trim)
            .filter(|u| !u.is_empty())
        {
            let mechanism =
                normalize_sasl_mechanism(spec.params.get("sasl_mechanism").map(String::as_str));
            let credentials = Credentials::new(
                username.to_string(),
                spec.password.clone().unwrap_or_default(),
            );
            let sasl = match mechanism.as_str() {
                "plain" => SaslConfig::Plain(credentials),
                "scram-sha-256" | "scram-sha256" => SaslConfig::ScramSha256(credentials),
                "scram-sha-512" | "scram-sha512" => SaslConfig::ScramSha512(credentials),
                other => {
                    return Err(MieoError::new(
                        ErrorCode::Unsupported,
                        format!("unsupported Kafka SASL mechanism `{other}`"),
                    )
                    .with_hint("supported mechanisms: plain, scram-sha-256, scram-sha-512"));
                }
            };
            builder = builder.sasl_config(sasl);
        }

        let timeout_secs = spec.connect_timeout_secs.max(1);
        let client = tokio::time::timeout(Duration::from_secs(timeout_secs), builder.build())
            .await
            .map_err(|_| {
                MieoError::new(
                    ErrorCode::Timeout,
                    format!("Kafka connect timed out after {timeout_secs}s"),
                )
            })?
            .map_err(map_kafka_err)?;

        Ok(Self {
            spec: spec.clone(),
            client,
            brokers,
        })
    }

    pub fn spec(&self) -> &ConnectionSpec {
        &self.spec
    }

    pub fn in_transaction(&self) -> bool {
        false
    }

    /// Round-trip a metadata request; succeeds only when a broker answers.
    pub async fn ping(&mut self) -> MieoResult<()> {
        self.client.list_topics().await.map_err(map_kafka_err)?;
        Ok(())
    }

    /// Create a topic through the cluster controller.
    pub async fn create_topic(
        &mut self,
        name: &str,
        partitions: i32,
        replication_factor: i16,
    ) -> MieoResult<()> {
        let controller = self
            .client
            .controller_client()
            .map_err(map_kafka_err)?;
        tokio::time::timeout(
            Duration::from_secs(30),
            controller.create_topic(name.to_string(), partitions, replication_factor, 10_000),
        )
        .await
        .map_err(|_| {
            MieoError::new(
                ErrorCode::Timeout,
                format!("Kafka create_topic `{name}` timed out after 30s"),
            )
        })?
        .map_err(map_kafka_err)
    }

    /// Delete a topic through the cluster controller.
    pub async fn delete_topic(&mut self, name: &str) -> MieoResult<()> {
        let controller = self
            .client
            .controller_client()
            .map_err(map_kafka_err)?;
        tokio::time::timeout(
            Duration::from_secs(30),
            controller.delete_topic(name.to_string(), 10_000),
        )
        .await
        .map_err(|_| {
            MieoError::new(
                ErrorCode::Timeout,
                format!("Kafka delete_topic `{name}` timed out after 30s"),
            )
        })?
        .map_err(map_kafka_err)
    }

    /// Earliest/latest offsets per partition of `topic`, sorted by partition.
    pub async fn topic_watermarks(&mut self, topic: &str) -> MieoResult<Vec<(i32, i64, i64)>> {
        let topics = self.client.list_topics().await.map_err(map_kafka_err)?;
        let Some(info) = topics.into_iter().find(|t| t.name == topic) else {
            return Err(MieoError::new(
                ErrorCode::NotFound,
                format!("Kafka topic `{topic}` not found"),
            ));
        };
        let mut out = Vec::with_capacity(info.partitions.len());
        for partition in info.partitions {
            let partition_client = tokio::time::timeout(
                Duration::from_secs(15),
                self.client.partition_client(
                    topic.to_string(),
                    partition,
                    UnknownTopicHandling::Retry,
                ),
            )
            .await
            .map_err(|_| {
                MieoError::new(
                    ErrorCode::Timeout,
                    format!("Kafka metadata for `{topic}:{partition}` timed out"),
                )
            })?
            .map_err(map_kafka_err)?;
            let earliest = partition_client
                .get_offset(OffsetAt::Earliest)
                .await
                .map_err(map_kafka_err)?;
            let latest = partition_client
                .get_offset(OffsetAt::Latest)
                .await
                .map_err(map_kafka_err)?;
            out.push((partition, earliest, latest));
        }
        out.sort();
        Ok(out)
    }

    /// Produce a single record; returns the (partition, broker offset) it
    /// landed on. An omitted partition uses Kafka's default partitioner.
    pub async fn produce_one(
        &mut self,
        topic: &str,
        partition: Option<i32>,
        key: Option<String>,
        value: String,
    ) -> MieoResult<(i32, i64)> {
        let partition = match partition {
            Some(p) => {
                if p < 0 {
                    return Err(MieoError::new(
                        ErrorCode::Data,
                        "`partition` must be >= 0".to_string(),
                    ));
                }
                p
            }
            None => self.resolve_partition(topic, key.as_deref()).await?,
        };
        let partition_client = tokio::time::timeout(
            Duration::from_secs(15),
            self.client.partition_client(
                topic.to_string(),
                partition,
                UnknownTopicHandling::Retry,
            ),
        )
        .await
        .map_err(|_| {
            MieoError::new(
                ErrorCode::Timeout,
                format!("Kafka produce to `{topic}:{partition}` timed out"),
            )
        })?
        .map_err(map_kafka_err)?;
        let record = Record {
            key: key.map(String::into_bytes),
            value: Some(value.into_bytes()),
            headers: BTreeMap::new(),
            timestamp: Utc::now(),
        };
        let offsets = tokio::time::timeout(
            Duration::from_secs(30),
            partition_client.produce(vec![record], Compression::NoCompression),
        )
        .await
        .map_err(|_| {
            MieoError::new(
                ErrorCode::Timeout,
                format!("Kafka produce to `{topic}:{partition}` timed out"),
            )
        })?
        .map_err(map_kafka_err)?;
        Ok((partition, offsets.first().copied().unwrap_or(0)))
    }

    pub async fn server_info(&mut self) -> MieoResult<ServerInfo> {
        Ok(ServerInfo {
            engine: Engine::Kafka,
            version: "Apache Kafka".to_string(),
            version_comment: Some(self.brokers.join(", ")),
            current_database: self.brokers.first().cloned(),
            current_user: self.spec.username.clone(),
        })
    }

    pub async fn list_databases(&mut self) -> MieoResult<Vec<DatabaseRef>> {
        Ok(Vec::new())
    }

    pub async fn list_schemas(&mut self, database: Option<&str>) -> MieoResult<Vec<SchemaRef>> {
        let _ = database;
        Ok(Vec::new())
    }

    pub async fn list_objects(
        &mut self,
        scope: &Scope,
        kind: Option<ObjectKind>,
        pattern: Option<&str>,
    ) -> MieoResult<Vec<ObjectRef>> {
        let _ = scope;
        if let Some(kind) = kind
            && kind != ObjectKind::Table
        {
            return Ok(Vec::new());
        }
        let topics = self.client.list_topics().await.map_err(map_kafka_err)?;
        let needle = pattern.map(str::to_ascii_lowercase);
        Ok(topics
            .into_iter()
            .filter(|topic| {
                needle
                    .as_deref()
                    .is_none_or(|n| topic.name.to_ascii_lowercase().contains(n))
            })
            .map(|topic| ObjectRef {
                name: topic.name,
                kind: ObjectKind::Table,
                database: None,
                schema: None,
                comment: Some(format!("{} partitions", topic.partitions.len())),
                rows_estimate: None,
                size_bytes: None,
                updated_at: None,
            })
            .collect())
    }

    pub async fn table_columns(
        &mut self,
        scope: &Scope,
        table: &str,
    ) -> MieoResult<Vec<ColumnDef>> {
        let _ = (scope, table);
        Ok(Vec::new())
    }

    pub async fn table_indexes(&mut self, scope: &Scope, table: &str) -> MieoResult<Vec<IndexDef>> {
        let _ = (scope, table);
        Ok(Vec::new())
    }

    pub async fn foreign_keys(
        &mut self,
        scope: &Scope,
        table: &str,
    ) -> MieoResult<Vec<ForeignKeyDef>> {
        let _ = (scope, table);
        Ok(Vec::new())
    }

    pub async fn object_ddl(
        &mut self,
        scope: &Scope,
        kind: ObjectKind,
        name: &str,
    ) -> MieoResult<Option<String>> {
        let _ = (scope, kind, name);
        Ok(None)
    }

    // Kafka broker ACLs live outside the data path: no users/roles here.
    pub async fn list_db_users(&mut self) -> MieoResult<Vec<DbPrincipal>> {
        Ok(Vec::new())
    }

    pub async fn list_db_roles(&mut self) -> MieoResult<Vec<DbPrincipal>> {
        Ok(Vec::new())
    }

    pub async fn db_role_members(&mut self) -> MieoResult<Vec<RoleGrant>> {
        Ok(Vec::new())
    }

    pub async fn db_user_grants(
        &mut self,
        _name: &str,
        _host: Option<&str>,
    ) -> MieoResult<Vec<String>> {
        Ok(Vec::new())
    }

    pub async fn db_principal_detail(
        &mut self,
        _name: &str,
        _host: Option<&str>,
    ) -> MieoResult<DbPrincipalDetail> {
        Err(MieoError::unsupported(
            "this engine has no user management",
        ))
    }

    pub async fn create_db_user(&mut self, _req: &CreateDbUser) -> MieoResult<()> {
        Err(MieoError::unsupported(
            "Kafka has no user management in MieoSQL: broker ACLs are managed outside the client",
        ))
    }

    pub async fn drop_db_principal(&mut self, _req: &DropDbPrincipal) -> MieoResult<()> {
        Err(MieoError::unsupported(
            "Kafka has no user management in MieoSQL: broker ACLs are managed outside the client",
        ))
    }

    pub async fn grant(&mut self, _req: &GrantRequest) -> MieoResult<()> {
        Err(MieoError::unsupported(
            "Kafka has no user management in MieoSQL: broker ACLs are managed outside the client",
        ))
    }

    pub async fn revoke(&mut self, _req: &RevokeRequest) -> MieoResult<()> {
        Err(MieoError::unsupported(
            "Kafka has no user management in MieoSQL: broker ACLs are managed outside the client",
        ))
    }

    pub async fn set_user_password(
        &mut self,
        _name: &str,
        _host: Option<&str>,
        _password: &str,
    ) -> MieoResult<()> {
        Err(MieoError::unsupported(
            "Kafka has no user management in MieoSQL: broker ACLs are managed outside the client",
        ))
    }

    pub async fn use_database(&mut self, database: &str) -> MieoResult<()> {
        let _ = database;
        Ok(())
    }

    pub async fn begin(&mut self) -> MieoResult<()> {
        Err(transactions_unsupported())
    }

    pub async fn commit(&mut self) -> MieoResult<()> {
        Err(transactions_unsupported())
    }

    pub async fn rollback(&mut self) -> MieoResult<()> {
        Err(transactions_unsupported())
    }

    pub async fn run_query(
        &mut self,
        query_id: &str,
        sql: String,
        params: Vec<Value>,
        options: &QueryOptions,
        sink: &mpsc::Sender<QueryEvent>,
        cancel: &CancellationToken,
    ) -> MieoResult<QueryStats> {
        // Kafka commands carry all their data in the JSON text.
        let _ = params;
        match parse_command(&sql)? {
            KafkaCommand::Consume(command) => {
                self.consume(query_id, command, options, sink, cancel).await
            }
            KafkaCommand::Produce(command) => {
                self.produce(query_id, command, options, sink, cancel).await
            }
        }
    }

    /// JSON command to fetch the latest messages of a topic (partition 0).
    pub fn preview_statement(&self, scope: &Scope, table: &str, limit: u64) -> MieoResult<String> {
        let _ = scope;
        Ok(preview_json(table, limit))
    }

    pub fn engine(&self) -> Engine {
        Engine::Kafka
    }

    // ------------------------------------------------------------------
    // Query execution
    // ------------------------------------------------------------------

    async fn consume(
        &mut self,
        query_id: &str,
        command: ConsumeCommand,
        options: &QueryOptions,
        sink: &mpsc::Sender<QueryEvent>,
        cancel: &CancellationToken,
    ) -> MieoResult<QueryStats> {
        let started = Instant::now();
        let deadline = query_deadline(options);
        let limit = command
            .limit
            .min(options.max_rows.unwrap_or(u64::MAX));

        let partition_client = await_or_cancel(
            async {
                self.client
                    .partition_client(
                        command.topic.clone(),
                        command.partition,
                        UnknownTopicHandling::Retry,
                    )
                    .await
                    .map_err(map_kafka_err)
            },
            cancel,
            deadline,
        )
        .await?;

        let start_offset = match command.from {
            ConsumeFrom::Earliest => {
                await_or_cancel(
                    async {
                        partition_client
                            .get_offset(OffsetAt::Earliest)
                            .await
                            .map_err(map_kafka_err)
                    },
                    cancel,
                    deadline,
                )
                .await?
            }
            ConsumeFrom::Latest => {
                await_or_cancel(
                    async {
                        partition_client
                            .get_offset(OffsetAt::Latest)
                            .await
                            .map_err(map_kafka_err)
                    },
                    cancel,
                    deadline,
                )
                .await?
            }
            ConsumeFrom::Offset(offset) => offset,
        };

        send_evt(
            sink,
            QueryEvent::Columns {
                query_id: query_id.to_string(),
                columns: consume_columns(),
            },
        )
        .await?;

        let fetch_size = options.fetch_size.max(1) as usize;
        let mut batch: Vec<Vec<Value>> = Vec::with_capacity(fetch_size.min(4096));
        let mut batch_index = 0u64;
        let mut rows = 0u64;
        let mut next_offset = start_offset;
        let mut last_high_watermark = start_offset;

        while rows < limit {
            if cancel.is_cancelled() {
                return Err(MieoError::cancelled());
            }
            let (records, high_watermark) = await_or_cancel(
                async {
                    partition_client
                        .fetch_records(next_offset, 0..FETCH_MAX_BYTES, FETCH_MAX_WAIT_MS)
                        .await
                        .map_err(map_kafka_err)
                },
                cancel,
                deadline,
            )
            .await?;
            last_high_watermark = high_watermark;
            if records.is_empty() {
                break;
            }
            for record in records {
                if rows >= limit {
                    break;
                }
                rows += 1;
                next_offset = record.offset.saturating_add(1);
                batch.push(record_row(
                    command.partition,
                    record.offset,
                    &record.record,
                ));
                if batch.len() >= fetch_size {
                    batch_index += 1;
                    send_evt(
                        sink,
                        QueryEvent::Batch {
                            query_id: query_id.to_string(),
                            rows: std::mem::take(&mut batch),
                            batch_index,
                        },
                    )
                    .await?;
                    batch.reserve(fetch_size.min(4096));
                }
            }
            if next_offset >= last_high_watermark {
                break;
            }
        }

        if !batch.is_empty() {
            batch_index += 1;
            send_evt(
                sink,
                QueryEvent::Batch {
                    query_id: query_id.to_string(),
                    rows: batch,
                    batch_index,
                },
            )
            .await?;
        }

        Ok(QueryStats {
            rows,
            rows_affected: 0,
            elapsed_ms: started.elapsed().as_millis() as u64,
            truncated: rows >= limit && next_offset < last_high_watermark,
            last_insert_id: None,
        })
    }

    async fn produce(
        &mut self,
        query_id: &str,
        command: ProduceCommand,
        options: &QueryOptions,
        sink: &mpsc::Sender<QueryEvent>,
        cancel: &CancellationToken,
    ) -> MieoResult<QueryStats> {
        let started = Instant::now();
        let deadline = query_deadline(options);
        if cancel.is_cancelled() {
            return Err(MieoError::cancelled());
        }

        let partition = match command.partition {
            Some(partition) => partition,
            None => {
                self.resolve_partition(&command.topic, command.key.as_deref())
                    .await?
            }
        };

        let partition_client = await_or_cancel(
            async {
                self.client
                    .partition_client(
                        command.topic.clone(),
                        partition,
                        UnknownTopicHandling::Retry,
                    )
                    .await
                    .map_err(map_kafka_err)
            },
            cancel,
            deadline,
        )
        .await?;

        let record = Record {
            key: command.key.map(String::into_bytes),
            value: Some(command.value.into_bytes()),
            headers: BTreeMap::new(),
            timestamp: Utc::now(),
        };
        let offsets = await_or_cancel(
            async {
                partition_client
                    .produce(vec![record], Compression::NoCompression)
                    .await
                    .map_err(map_kafka_err)
            },
            cancel,
            deadline,
        )
        .await?;

        // The broker-assigned offset travels as `last_insert_id`, mirroring how
        // SQL drivers report a freshly inserted row id.
        let offset = offsets.first().copied().map(|offset| offset.max(0) as u64);
        send_evt(
            sink,
            QueryEvent::Affected {
                query_id: query_id.to_string(),
                rows_affected: 1,
                last_insert_id: offset,
            },
        )
        .await?;

        Ok(QueryStats {
            rows: 0,
            rows_affected: 1,
            elapsed_ms: started.elapsed().as_millis() as u64,
            truncated: false,
            last_insert_id: offset,
        })
    }

    /// Pick a partition for a produce command that omitted `partition`.
    ///
    /// Keyed records use Kafka's default partitioner (murmur2, seed
    /// `0x9747b28c`) so they land where other Kafka clients would put them;
    /// unkeyed records go to the first partition.
    async fn resolve_partition(&mut self, topic: &str, key: Option<&str>) -> MieoResult<i32> {
        let topics = self.client.list_topics().await.map_err(map_kafka_err)?;
        let info = topics.iter().find(|t| t.name == topic).ok_or_else(|| {
            MieoError::new(
                ErrorCode::NotFound,
                format!("Kafka topic `{topic}` was not found"),
            )
        })?;
        let first = info.partitions.iter().next().copied().ok_or_else(|| {
            MieoError::new(
                ErrorCode::Data,
                format!("Kafka topic `{topic}` has no partitions"),
            )
        })?;
        if let Some(key) = key
            && !info.partitions.is_empty()
        {
            let count = info.partitions.len() as u32;
            let index = (murmur2(key.as_bytes()) & 0x7fff_ffff) % count;
            if let Some(partition) = info.partitions.iter().nth(index as usize) {
                return Ok(*partition);
            }
        }
        Ok(first)
    }
}

// ----------------------------------------------------------------------
// Command model + JSON parsing
// ----------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum KafkaCommand {
    Consume(ConsumeCommand),
    Produce(ProduceCommand),
}

#[derive(Debug, Clone, PartialEq)]
struct ConsumeCommand {
    topic: String,
    partition: i32,
    from: ConsumeFrom,
    limit: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConsumeFrom {
    Earliest,
    Latest,
    Offset(i64),
}

#[derive(Debug, Clone, PartialEq)]
struct ProduceCommand {
    topic: String,
    partition: Option<i32>,
    key: Option<String>,
    value: String,
}

fn syntax_error(message: impl Into<String>) -> MieoError {
    MieoError::new(ErrorCode::Syntax, message)
        .with_hint(format!("expected {CONSUME_HINT} or {PRODUCE_HINT}"))
}

fn parse_command(sql: &str) -> MieoResult<KafkaCommand> {
    let text = sql.trim();
    if text.is_empty() {
        return Err(syntax_error("empty Kafka command"));
    }
    let value: serde_json::Value = serde_json::from_str(text)
        .map_err(|e| syntax_error(format!("invalid Kafka command JSON: {e}")))?;
    let object = value
        .as_object()
        .ok_or_else(|| syntax_error("Kafka command must be a JSON object"))?;
    if object.contains_key("produce") {
        parse_produce(object).map(KafkaCommand::Produce)
    } else {
        parse_consume(object).map(KafkaCommand::Consume)
    }
}

fn parse_consume(
    object: &serde_json::Map<String, serde_json::Value>,
) -> MieoResult<ConsumeCommand> {
    let topic = object
        .get("topic")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|topic| !topic.is_empty())
        .ok_or_else(|| syntax_error("missing non-empty `topic` field"))?
        .to_string();

    let partition = match object.get("partition") {
        None | Some(serde_json::Value::Null) => 0,
        Some(value) => {
            let partition = value
                .as_i64()
                .ok_or_else(|| syntax_error("`partition` must be an integer"))?;
            if partition < 0 {
                return Err(syntax_error("`partition` must be >= 0"));
            }
            partition as i32
        }
    };

    let from = match object.get("from") {
        None | Some(serde_json::Value::Null) => ConsumeFrom::Latest,
        Some(serde_json::Value::String(raw)) => {
            match raw.trim().to_ascii_lowercase().as_str() {
                "earliest" => ConsumeFrom::Earliest,
                "latest" => ConsumeFrom::Latest,
                numeric => ConsumeFrom::Offset(parse_offset(numeric)?),
            }
        }
        Some(serde_json::Value::Number(number)) => {
            let offset = number
                .as_i64()
                .ok_or_else(|| syntax_error("`from` offset must be an integer"))?;
            if offset < 0 {
                return Err(syntax_error("`from` offset must be >= 0"));
            }
            ConsumeFrom::Offset(offset)
        }
        Some(_) => {
            return Err(syntax_error(
                "`from` must be \"earliest\", \"latest\" or a non-negative integer offset",
            ));
        }
    };

    let limit = match object.get("limit") {
        None | Some(serde_json::Value::Null) => DEFAULT_CONSUME_LIMIT,
        Some(serde_json::Value::Number(number)) => number
            .as_u64()
            .ok_or_else(|| syntax_error("`limit` must be a positive integer"))?,
        Some(serde_json::Value::String(raw)) => raw
            .trim()
            .parse::<u64>()
            .map_err(|_| syntax_error("`limit` must be a positive integer"))?,
        Some(_) => return Err(syntax_error("`limit` must be a positive integer")),
    };
    if limit == 0 {
        return Err(syntax_error("`limit` must be >= 1"));
    }

    Ok(ConsumeCommand {
        topic,
        partition,
        from,
        limit,
    })
}

fn parse_offset(raw: &str) -> MieoResult<i64> {
    let offset = raw
        .parse::<i64>()
        .map_err(|_| syntax_error("`from` must be \"earliest\", \"latest\" or a non-negative integer offset"))?;
    if offset < 0 {
        return Err(syntax_error("`from` offset must be >= 0"));
    }
    Ok(offset)
}

fn parse_produce(
    object: &serde_json::Map<String, serde_json::Value>,
) -> MieoResult<ProduceCommand> {
    let produce = object
        .get("produce")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| syntax_error("`produce` must be a JSON object"))?;

    let topic = produce
        .get("topic")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|topic| !topic.is_empty())
        .ok_or_else(|| syntax_error("missing non-empty `topic` inside `produce`"))?
        .to_string();

    let value = produce
        .get("value")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| syntax_error("missing string `value` inside `produce`"))?
        .to_string();

    let partition = match produce.get("partition") {
        None | Some(serde_json::Value::Null) => None,
        Some(raw) => {
            let partition = raw
                .as_i64()
                .ok_or_else(|| syntax_error("`partition` must be an integer"))?;
            if partition < 0 {
                return Err(syntax_error("`partition` must be >= 0"));
            }
            Some(partition as i32)
        }
    };

    let key = match produce.get("key") {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(raw)) => Some(raw.clone()),
        Some(_) => return Err(syntax_error("`key` must be a string")),
    };

    Ok(ProduceCommand {
        topic,
        partition,
        key,
        value,
    })
}

// ----------------------------------------------------------------------
// Record / value mapping
// ----------------------------------------------------------------------

fn consume_columns() -> Vec<ColumnMeta> {
    let specs: [(&str, &str, DataTypeKind, bool); 5] = [
        ("partition", "int32", DataTypeKind::Int, false),
        ("offset", "int64", DataTypeKind::Int, false),
        ("timestamp", "timestamp", DataTypeKind::DateTime, false),
        ("key", "bytes", DataTypeKind::Text, true),
        ("value", "bytes", DataTypeKind::Text, true),
    ];
    specs
        .into_iter()
        .enumerate()
        .map(|(ordinal, (name, native_type, kind, nullable))| ColumnMeta {
            name: name.to_string(),
            ordinal: ordinal as u32,
            native_type: native_type.to_string(),
            kind,
            nullable: Some(nullable),
        })
        .collect()
}

fn record_row(partition: i32, offset: i64, record: &Record) -> Vec<Value> {
    vec![
        Value::Int(partition as i64),
        Value::Int(offset),
        Value::DateTime(record.timestamp.to_rfc3339_opts(SecondsFormat::Millis, true)),
        bytes_value(record.key.as_deref()),
        bytes_value(record.value.as_deref()),
    ]
}

/// Kafka keys/values are untyped bytes: UTF-8 renders as text, anything else as
/// base64 (`Value::Bytes`).
fn bytes_value(bytes: Option<&[u8]>) -> Value {
    match bytes {
        None => Value::Null,
        Some(raw) => match std::str::from_utf8(raw) {
            Ok(text) => Value::Text(text.to_string()),
            Err(_) => Value::Bytes(b64_encode(raw)),
        },
    }
}

// ----------------------------------------------------------------------
// Connection helpers
// ----------------------------------------------------------------------

fn transactions_unsupported() -> MieoError {
    MieoError::unsupported("Apache Kafka transactions are not supported")
}

/// Consume command used by the data-grid preview; the topic is JSON-escaped.
fn preview_json(table: &str, limit: u64) -> String {
    let topic = serde_json::Value::String(table.to_string()).to_string();
    format!("{{ \"topic\": {topic}, \"partition\": 0, \"from\": \"latest\", \"limit\": {limit} }}")
}

fn normalize_sasl_mechanism(raw: Option<&str>) -> String {
    let Some(raw) = raw.map(str::trim).filter(|value| !value.is_empty()) else {
        return "plain".to_string();
    };
    raw.to_ascii_lowercase().replace('_', "-")
}

/// Split `spec.host` on `,` and append the default port where missing.
///
/// Accepts `host`, `host:port`, `[::1]`, `[::1]:9092` and bare IPv6 literals.
fn bootstrap_brokers(spec: &ConnectionSpec) -> MieoResult<Vec<String>> {
    let default_port = spec.port.unwrap_or(DEFAULT_KAFKA_PORT);
    let host = spec.host.as_deref().unwrap_or("localhost");
    let brokers: Vec<String> = host
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(|part| with_default_port(part, default_port))
        .collect();
    if brokers.is_empty() {
        return Err(MieoError::new(
            ErrorCode::Connection,
            "no Kafka bootstrap brokers configured",
        ));
    }
    Ok(brokers)
}

fn with_default_port(broker: &str, default_port: u16) -> String {
    if broker.starts_with('[') {
        return if broker.contains("]:") {
            broker.to_string()
        } else {
            format!("{broker}:{default_port}")
        };
    }
    match broker.matches(':').count() {
        0 => format!("{broker}:{default_port}"),
        1 => broker.to_string(),
        // Bare IPv6 literal.
        _ => format!("[{broker}]:{default_port}"),
    }
}

/// Kafka's default partitioner hash: murmur2 with seed `0x9747b28c`, matching
/// the Java/Go clients' `toPositive(murmur2(key)) % partition_count`.
fn murmur2(data: &[u8]) -> u32 {
    const SEED: u32 = 0x9747_b28c;
    const M: u32 = 0x5bd1_e995;
    const R: u32 = 24;

    let length = data.len() as u32;
    let mut hash = SEED ^ length;
    let rounded = data.len() & !3;
    let mut index = 0;
    while index < rounded {
        let mut k = u32::from_le_bytes([
            data[index],
            data[index + 1],
            data[index + 2],
            data[index + 3],
        ]);
        k = k.wrapping_mul(M);
        k ^= k >> R;
        k = k.wrapping_mul(M);
        hash = hash.wrapping_mul(M);
        hash ^= k;
        index += 4;
    }
    let remaining = data.len() & 3;
    if remaining == 3 {
        hash ^= u32::from(data[rounded + 2]) << 16;
    }
    if remaining >= 2 {
        hash ^= u32::from(data[rounded + 1]) << 8;
    }
    if remaining >= 1 {
        hash ^= u32::from(data[rounded]);
        hash = hash.wrapping_mul(M);
    }
    hash ^= hash >> 13;
    hash = hash.wrapping_mul(M);
    hash ^= hash >> 15;
    hash
}

// ----------------------------------------------------------------------
// Error mapping + async plumbing
// ----------------------------------------------------------------------

fn query_deadline(options: &QueryOptions) -> Option<tokio::time::Instant> {
    options
        .timeout_ms
        .map(|ms| tokio::time::Instant::now() + Duration::from_millis(ms))
}

async fn send_evt(sink: &mpsc::Sender<QueryEvent>, event: QueryEvent) -> MieoResult<()> {
    sink.send(event).await.map_err(|_| MieoError::cancelled())
}

/// Await `future`, aborting on cancellation or once `deadline` passes.
async fn await_or_cancel<F, T>(
    future: F,
    cancel: &CancellationToken,
    deadline: Option<tokio::time::Instant>,
) -> MieoResult<T>
where
    F: std::future::Future<Output = MieoResult<T>>,
{
    let future = future;
    tokio::pin!(future);
    if let Some(deadline) = deadline {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(MieoError::cancelled()),
            _ = tokio::time::sleep_until(deadline) => {
                Err(MieoError::new(ErrorCode::Timeout, "query timed out"))
            }
            result = &mut future => result,
        }
    } else {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(MieoError::cancelled()),
            result = &mut future => result,
        }
    }
}

fn kafka_error_info(
    error: &rskafka::client::error::Error,
) -> (ErrorCode, Option<String>, Option<String>) {
    use rskafka::client::error::Error as KafkaError;
    use rskafka::client::error::ProtocolError;

    match error {
        KafkaError::ServerError {
            protocol_error,
            error_message,
            ..
        } => {
            let code = match protocol_error {
                ProtocolError::UnknownTopicOrPartition
                | ProtocolError::UnknownTopicId
                | ProtocolError::ResourceNotFound => ErrorCode::NotFound,
                ProtocolError::TopicAuthorizationFailed
                | ProtocolError::GroupAuthorizationFailed
                | ProtocolError::ClusterAuthorizationFailed
                | ProtocolError::TransactionalIdAuthorizationFailed => ErrorCode::Permission,
                ProtocolError::SaslAuthenticationFailed
                | ProtocolError::IllegalSaslState
                | ProtocolError::UnsupportedSaslMechanism
                | ProtocolError::UnacceptableCredential
                | ProtocolError::SecurityDisabled => ErrorCode::Auth,
                ProtocolError::RequestTimedOut => ErrorCode::Timeout,
                ProtocolError::OffsetOutOfRange
                | ProtocolError::CorruptMessage
                | ProtocolError::InvalidRecord
                | ProtocolError::InvalidTopicException
                | ProtocolError::InvalidRequest
                | ProtocolError::MessageTooLarge => ErrorCode::Data,
                _ => ErrorCode::Data,
            };
            (
                code,
                Some(format!("{protocol_error:?}")),
                error_message.clone(),
            )
        }
        KafkaError::Connection(inner) => {
            let code = match inner {
                rskafka::ConnectionError::SaslFailed(_) => ErrorCode::Auth,
                _ => ErrorCode::Connection,
            };
            (code, None, None)
        }
        KafkaError::Request(_) => (ErrorCode::Network, None, None),
        KafkaError::InvalidResponse(_) => (ErrorCode::Data, None, None),
        KafkaError::RetryFailed(_) => (ErrorCode::Connection, None, None),
        KafkaError::Timeout => (ErrorCode::Timeout, None, None),
        _ => (ErrorCode::Internal, None, None),
    }
}

fn map_kafka_err(error: rskafka::client::error::Error) -> MieoError {
    let (code, db_code, detail) = kafka_error_info(&error);
    let mut mapped = MieoError::new(code, format!("Kafka error: {error}"));
    if let Some(db_code) = db_code {
        mapped = mapped.with_db_code(db_code);
    }
    if let Some(detail) = detail {
        mapped = mapped.with_detail(detail);
    }
    mapped.with_source(error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn spec_with(host: &str, port: Option<u16>) -> ConnectionSpec {
        ConnectionSpec {
            profile_id: None,
            name: "test".to_string(),
            engine: Engine::Kafka,
            host: Some(host.to_string()),
            port,
            username: None,
            password: None,
            database: None,
            file: None,
            params: BTreeMap::new(),
            ssl_mode: SslMode::Disable,
            variant: None,
            ssh: None,
            ssh_password: None,
            connect_timeout_secs: 5,
        }
    }

    #[test]
    fn parse_consume_defaults() {
        let command = parse_command(r#"{"topic": "orders"}"#).expect("parses");
        assert_eq!(
            command,
            KafkaCommand::Consume(ConsumeCommand {
                topic: "orders".to_string(),
                partition: 0,
                from: ConsumeFrom::Latest,
                limit: DEFAULT_CONSUME_LIMIT,
            })
        );
    }

    #[test]
    fn parse_consume_from_string_and_number() {
        let command = parse_command(r#"{"topic": "t", "from": "earliest", "limit": 5}"#)
            .expect("parses");
        let KafkaCommand::Consume(command) = command else {
            panic!("expected consume");
        };
        assert_eq!(command.from, ConsumeFrom::Earliest);
        assert_eq!(command.limit, 5);

        let command = parse_command(r#"{"topic": "t", "partition": 2, "from": 42, "limit": "7"}"#)
            .expect("parses");
        let KafkaCommand::Consume(command) = command else {
            panic!("expected consume");
        };
        assert_eq!(command.partition, 2);
        assert_eq!(command.from, ConsumeFrom::Offset(42));
        assert_eq!(command.limit, 7);

        let command = parse_command(r#"{"topic": "t", "from": "0042"}"#).expect("parses");
        let KafkaCommand::Consume(command) = command else {
            panic!("expected consume");
        };
        assert_eq!(command.from, ConsumeFrom::Offset(42));
    }

    #[test]
    fn parse_consume_rejects_bad_commands() {
        assert_eq!(parse_command("").unwrap_err().code(), ErrorCode::Syntax);
        assert_eq!(
            parse_command("not json").unwrap_err().code(),
            ErrorCode::Syntax
        );
        assert_eq!(
            parse_command(r#"{"partition": 0}"#).unwrap_err().code(),
            ErrorCode::Syntax
        );
        assert_eq!(
            parse_command(r#"{"topic": "t", "from": "soon"}"#)
                .unwrap_err()
                .code(),
            ErrorCode::Syntax
        );
        assert_eq!(
            parse_command(r#"{"topic": "t", "from": -1}"#)
                .unwrap_err()
                .code(),
            ErrorCode::Syntax
        );
        assert_eq!(
            parse_command(r#"{"topic": "t", "limit": 0}"#)
                .unwrap_err()
                .code(),
            ErrorCode::Syntax
        );
        assert_eq!(
            parse_command(r#"{"topic": "t", "partition": -3}"#)
                .unwrap_err()
                .code(),
            ErrorCode::Syntax
        );
        // Syntax errors carry a hint with both accepted forms.
        let error = parse_command("nope").unwrap_err();
        assert!(error.info.hint.is_some());
    }

    #[test]
    fn parse_produce_forms() {
        let command = parse_command(
            r#"{"produce": {"topic": "orders", "partition": 1, "key": "k", "value": "hello"}}"#,
        )
        .expect("parses");
        assert_eq!(
            command,
            KafkaCommand::Produce(ProduceCommand {
                topic: "orders".to_string(),
                partition: Some(1),
                key: Some("k".to_string()),
                value: "hello".to_string(),
            })
        );

        let command =
            parse_command(r#"{"produce": {"topic": "orders", "value": ""}}"#).expect("parses");
        assert_eq!(
            command,
            KafkaCommand::Produce(ProduceCommand {
                topic: "orders".to_string(),
                partition: None,
                key: None,
                value: String::new(),
            })
        );

        assert_eq!(
            parse_command(r#"{"produce": {"topic": "orders"}}"#)
                .unwrap_err()
                .code(),
            ErrorCode::Syntax
        );
        assert_eq!(
            parse_command(r#"{"produce": {"topic": "orders", "key": 3, "value": "v"}}"#)
                .unwrap_err()
                .code(),
            ErrorCode::Syntax
        );
        assert_eq!(
            parse_command(r#"{"produce": "orders"}"#)
                .unwrap_err()
                .code(),
            ErrorCode::Syntax
        );
    }

    #[test]
    fn bytes_value_maps_text_null_and_base64() {
        assert_eq!(bytes_value(None), Value::Null);
        assert_eq!(bytes_value(Some(b"hello")), Value::Text("hello".to_string()));
        // Invalid UTF-8 becomes base64 (`Value::Bytes` is a base64 string).
        assert_eq!(
            bytes_value(Some(&[0xff, 0xfe])),
            Value::Bytes("//4=".to_string())
        );
    }

    #[test]
    fn record_row_layout() {
        let timestamp = Utc
            .timestamp_millis_opt(1_700_000_000_000)
            .single()
            .expect("valid timestamp");
        let record = Record {
            key: Some(b"k".to_vec()),
            value: Some(vec![0xff, 0xfe]),
            headers: BTreeMap::new(),
            timestamp,
        };
        let row = record_row(3, 42, &record);
        assert_eq!(
            row,
            vec![
                Value::Int(3),
                Value::Int(42),
                Value::DateTime("2023-11-14T22:13:20.000Z".to_string()),
                Value::Text("k".to_string()),
                Value::Bytes("//4=".to_string()),
            ]
        );

        let empty = Record {
            key: None,
            value: None,
            headers: BTreeMap::new(),
            timestamp,
        };
        let row = record_row(0, 0, &empty);
        assert_eq!(row[3], Value::Null);
        assert_eq!(row[4], Value::Null);
    }

    #[test]
    fn bootstrap_brokers_apply_default_ports() {
        let brokers = bootstrap_brokers(&spec_with("a, b:1234,[::1],[::1]:9093", None))
            .expect("brokers parse");
        assert_eq!(
            brokers,
            vec![
                "a:9092".to_string(),
                "b:1234".to_string(),
                "[::1]:9092".to_string(),
                "[::1]:9093".to_string(),
            ]
        );

        let brokers = bootstrap_brokers(&spec_with("broker.internal", Some(19092)))
            .expect("brokers parse");
        assert_eq!(brokers, vec!["broker.internal:19092".to_string()]);

        assert_eq!(
            bootstrap_brokers(&spec_with(" , ", None)).unwrap_err().code(),
            ErrorCode::Connection
        );
    }

    #[test]
    fn sasl_mechanism_normalization() {
        assert_eq!(normalize_sasl_mechanism(None), "plain");
        assert_eq!(normalize_sasl_mechanism(Some("")), "plain");
        assert_eq!(normalize_sasl_mechanism(Some("PLAIN")), "plain");
        assert_eq!(
            normalize_sasl_mechanism(Some("scram_sha_256")),
            "scram-sha-256"
        );
        assert_eq!(
            normalize_sasl_mechanism(Some("SCRAM-SHA-512")),
            "scram-sha-512"
        );
    }

    #[test]
    fn murmur2_matches_kafka_reference_vectors() {
        // Vectors from Apache Kafka's `UtilsTest.testMurmur2` (unsigned).
        assert_eq!(murmur2(b"21"), 3_321_034_988);
        assert_eq!(murmur2(b"abc"), 479_470_107);
        assert_eq!(murmur2(b"foobar"), 3_504_634_814);
        assert_eq!(murmur2(b""), 275_646_681);
    }

    #[test]
    fn preview_statement_roundtrips_through_parser() {
        for table in ["orders", r#"orders "v2""#] {
            let json = preview_json(table, 25);
            let command = parse_command(&json).expect("preview parses");
            let KafkaCommand::Consume(command) = command else {
                panic!("expected consume");
            };
            assert_eq!(command.topic, table);
            assert_eq!(command.from, ConsumeFrom::Latest);
            assert_eq!(command.limit, 25);
        }
    }
}
