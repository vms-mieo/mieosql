//! Connection profiles and resolved connection specs.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Stable identifier of a connection profile (UUID v7 string).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(transparent)]
#[ts(export)]
pub struct ProfileId(pub String);

impl std::fmt::Display for ProfileId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<String> for ProfileId {
    fn from(v: String) -> Self {
        ProfileId(v)
    }
}

/// Supported database engines.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum Engine {
    #[serde(rename = "mysql")]
    MySql,
    #[serde(rename = "mariadb")]
    MariaDb,
    Postgres,
    Sqlite,
    #[serde(rename = "sql_server")]
    SqlServer,
    #[serde(rename = "mongodb")]
    MongoDb,
    Redis,
    Kafka,
    #[serde(rename = "clickhouse")]
    ClickHouse,
    Oracle,
    Snowflake,
}

/// Broad engine category — drives UI affordances (SQL vs document vs ...).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum EngineKind {
    /// Relational / SQL engines.
    Sql,
    /// Document stores (MongoDB, AWS DocumentDB).
    Document,
    /// Key-value stores (Redis, ElastiCache, Memorystore).
    KeyValue,
    /// Message brokers (Kafka).
    MessageBroker,
}

impl Engine {
    /// Engines implemented by the current build.
    pub const IMPLEMENTED: &'static [Engine] = &[
        Engine::MySql,
        Engine::MariaDb,
        Engine::Postgres,
        Engine::Sqlite,
        Engine::SqlServer,
        Engine::MongoDb,
        Engine::Redis,
        Engine::Kafka,
        Engine::ClickHouse,
    ];

    pub fn is_implemented(self) -> bool {
        Self::IMPLEMENTED.contains(&self)
    }

    pub fn label(self) -> &'static str {
        match self {
            Engine::MySql => "MySQL",
            Engine::MariaDb => "MariaDB",
            Engine::Postgres => "PostgreSQL",
            Engine::Sqlite => "SQLite",
            Engine::SqlServer => "SQL Server",
            Engine::MongoDb => "MongoDB",
            Engine::Redis => "Redis",
            Engine::Kafka => "Apache Kafka",
            Engine::ClickHouse => "ClickHouse",
            Engine::Oracle => "Oracle",
            Engine::Snowflake => "Snowflake",
        }
    }

    pub fn default_port(self) -> Option<u16> {
        match self {
            Engine::MySql | Engine::MariaDb => Some(3306),
            Engine::Postgres => Some(5432),
            Engine::SqlServer => Some(1433),
            Engine::MongoDb => Some(27017),
            Engine::Redis => Some(6379),
            Engine::Kafka => Some(9092),
            Engine::ClickHouse => Some(8123),
            Engine::Oracle => Some(1521),
            Engine::Sqlite | Engine::Snowflake => None,
        }
    }

    /// Broad category of the engine.
    pub fn kind(self) -> EngineKind {
        match self {
            Engine::MySql
            | Engine::MariaDb
            | Engine::Postgres
            | Engine::Sqlite
            | Engine::SqlServer
            | Engine::ClickHouse
            | Engine::Oracle
            | Engine::Snowflake => EngineKind::Sql,
            Engine::MongoDb => EngineKind::Document,
            Engine::Redis => EngineKind::KeyValue,
            Engine::Kafka => EngineKind::MessageBroker,
        }
    }

    /// Whether the engine speaks SQL and is served by the SQL driver layer.
    pub fn is_sql(self) -> bool {
        self.kind() == EngineKind::Sql
    }

    /// Whether connections are file-based rather than host/port based.
    pub fn is_file_based(self) -> bool {
        matches!(self, Engine::Sqlite)
    }
}

/// TLS policy for a connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum SslMode {
    /// Never use TLS.
    Disable,
    /// Use TLS if the server supports it. (Default — matches server-client conventions.)
    #[default]
    Prefer,
    /// Require TLS, but do not verify the certificate.
    Require,
    /// Require TLS and verify the certificate chain.
    VerifyCa,
    /// Require TLS, verify chain and hostname.
    VerifyFull,
}

/// How to authenticate against the SSH jump host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum SshAuthKind {
    Password,
    PrivateKey,
    Agent,
}

/// SSH tunnel configuration (implemented in a later phase; model is stable).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SshSpec {
    pub host: String,
    #[serde(default = "default_ssh_port")]
    pub port: u16,
    pub username: String,
    pub auth: SshAuthKind,
    /// Path to the private key when `auth == PrivateKey`.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub private_key_path: Option<String>,
    /// Verify the host key against known_hosts (recommended).
    #[serde(default = "default_true")]
    pub host_key_check: bool,
}

fn default_ssh_port() -> u16 {
    22
}

fn default_true() -> bool {
    true
}

/// A saved connection (metadata only — never contains secrets).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ConnectionProfile {
    pub id: ProfileId,
    pub name: String,
    pub engine: Engine,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub host: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub port: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub username: Option<String>,
    /// Default database to open.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub database: Option<String>,
    /// SQLite: path to the database file.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub file: Option<String>,
    /// Extra engine parameters (charset, application_name, ...).
    #[serde(default)]
    pub params: BTreeMap<String, String>,
    #[serde(default)]
    pub ssl_mode: SslMode,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub ssh: Option<SshSpec>,
    /// UI accent color, e.g. `#e5484d`. Purely cosmetic.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub color: Option<String>,
    /// UI grouping folder.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub group: Option<String>,
    /// Connector preset / cloud flavor, e.g. `aurora`, `documentdb`,
    /// `elasticache`, `memorystore`, `redshift`, `azure`. Purely descriptive:
    /// the underlying driver is chosen by `engine`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub variant: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl ConnectionProfile {
    /// Create a fresh profile with a generated id and default port.
    pub fn new(name: impl Into<String>, engine: Engine, now: DateTime<Utc>) -> ConnectionProfile {
        ConnectionProfile {
            id: ProfileId(format!("p_{}", uuid_like_id())),
            name: name.into(),
            engine,
            host: None,
            port: engine.default_port(),
            username: None,
            database: None,
            file: None,
            params: BTreeMap::new(),
            ssl_mode: SslMode::default(),
            ssh: None,
            color: None,
            group: None,
            variant: None,
            created_at: now,
            updated_at: now,
        }
    }
}

/// Time-ordered random id without pulling a uuid dependency into core.
fn uuid_like_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let mut state = now.as_nanos() as u64;
    // xorshift64* for 64 bits of randomness mixed with time.
    state ^= state >> 12;
    state ^= state << 25;
    state ^= state >> 27;
    let rand = state.wrapping_mul(0x2545_F491_4F6C_DD1D);
    format!(
        "{:016x}{:012x}",
        now.as_millis() as u64,
        rand & 0xFFFF_FFFF_FFFF
    )
}

impl ConnectionProfile {
    /// `host:port/database` (or the file path) for display purposes.
    pub fn endpoint_summary(&self) -> String {
        if let Some(file) = &self.file {
            return file.clone();
        }
        format!(
            "{}:{}{}",
            self.host.as_deref().unwrap_or("localhost"),
            self.port.unwrap_or(0),
            self.database
                .as_deref()
                .map(|d| format!("/{d}"))
                .unwrap_or_default()
        )
    }
}

/// Fully resolved connection parameters, including secrets.
///
/// This type never crosses the IPC boundary; `Debug` is redacted.
#[derive(Clone)]
pub struct ConnectionSpec {
    pub profile_id: Option<ProfileId>,
    pub name: String,
    pub engine: Engine,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub username: Option<String>,
    pub password: Option<String>,
    pub database: Option<String>,
    /// SQLite file path.
    pub file: Option<String>,
    pub params: BTreeMap<String, String>,
    pub ssl_mode: SslMode,
    /// Connector preset / cloud flavor (descriptive only).
    pub variant: Option<String>,
    pub ssh: Option<SshSpec>,
    /// SSH password / key passphrase, resolved from the vault.
    pub ssh_password: Option<String>,
    pub connect_timeout_secs: u64,
}

impl std::fmt::Debug for ConnectionSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConnectionSpec")
            .field("profile_id", &self.profile_id)
            .field("name", &self.name)
            .field("engine", &self.engine)
            .field("host", &self.host)
            .field("port", &self.port)
            .field("username", &self.username)
            .field("password", &self.password.as_ref().map(|_| "<redacted>"))
            .field("database", &self.database)
            .field("file", &self.file)
            .field("ssl_mode", &self.ssl_mode)
            .field("variant", &self.variant)
            .field("ssh", &self.ssh.as_ref().map(|s| &s.host))
            .field(
                "ssh_password",
                &self.ssh_password.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

impl ConnectionSpec {
    pub fn from_profile(profile: &ConnectionProfile, password: Option<String>) -> Self {
        Self {
            profile_id: Some(profile.id.clone()),
            name: profile.name.clone(),
            engine: profile.engine,
            host: profile.host.clone(),
            port: profile.port,
            username: profile.username.clone(),
            password,
            database: profile.database.clone(),
            file: profile.file.clone(),
            params: profile.params.clone(),
            ssl_mode: profile.ssl_mode,
            variant: profile.variant.clone(),
            ssh: profile.ssh.clone(),
            ssh_password: None,
            connect_timeout_secs: 30,
        }
    }

    /// `host:port/database` style summary for logs (no secrets).
    pub fn endpoint(&self) -> String {
        if let Some(file) = &self.file {
            return file.clone();
        }
        format!(
            "{}:{}{}",
            self.host.as_deref().unwrap_or("localhost"),
            self.port.unwrap_or(0),
            self.database
                .as_deref()
                .map(|d| format!("/{d}"))
                .unwrap_or_default()
        )
    }
}
